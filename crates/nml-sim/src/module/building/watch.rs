//! Enemy watch, neutralizing and capturing of the wall group (re/arming-gates 3.D1/3.D2).
//! The master of a group registers via [`update_wall_area_watch`] an
//! area query ([`crate::area`]) over the hull rectangle of master and members and attaches a
//! watcher (`count > 0` -> `0x2703`). An enemy in the area puts the whole group to slot 8
//! ([`on_abandon_to_neutral`]); afterward a capture watch checks every second whether all listed
//! objects of the slots 0-7 belong to one slot ([`on_check_capture_area`]), and gives the group to it
//! ([`capture_by_player`]), which gets an enemy watch again after one second.
//!
//! Callers: `0x1B03` to buildings created finished (`construction::on_set_slave_list`, way a) and the
//! build step on a finished, intact group (`construction::forward_build_tick`, way b; W-271),
//! the timers `0x2B03`/`0x2A03`/`0x2903` and the watcher `0x2703` via `building::query`; the
//! destructor unregisters the query (`building::destruct`).

use super::{GROUP, building, building_mut, msg};
use crate::area::{self, CAPTURE_WATCH, ENEMY_WATCH};
use crate::id::{NEUTRAL_SLOT, ObjectId};
use crate::message::Message;
use crate::module::base::event_flags::{GREATER, WATCHER};
use crate::sim::Sim;
use crate::world::queue_owner_change;

/// Delay of the watch timers (`0x2A03`, `0x2903`, `0x2B03`): 1000 ms, group `0x80`, without
/// repetition (re/arming-gates 2.D).
const WATCH_DELAY_MS: i32 = 1000;

/// `PropertyBuilding_UpdateWallAreaWatch` (`0x00507EB0`, re/arming-gates 3.D1): without members or with a
/// running query (`check_area_id ≠ −1`) nothing (0). Otherwise compute the rectangle from the tiles
/// (`o.tile`, `GO+0x328/+0x32C`) of the master and the **existing** members (`{min x, min z, max x,
/// max z}`) once and [`register_enemy_area_watch`]. Later members (gate via `0x2103`)
/// do not change the rectangle.
pub fn update_wall_area_watch(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(b) = building(sim, id) else {
        return 0;
    };
    if b.slaves.is_empty() || b.check_area_id != -1 {
        return 0;
    }
    let slaves = b.slaves.clone();
    let Some((x, z)) = sim.object(id).map(|o| o.tile) else {
        return 0;
    };
    let [mut x0, mut z0, mut x1, mut z1] = [x, z, x, z];
    for s in slaves {
        if let Some((x, z)) = sim.object(s).map(|o| o.tile) {
            x0 = x0.min(x);
            x1 = x1.max(x);
            z0 = z0.min(z);
            z1 = z1.max(z);
        }
    }
    if let Some(b) = building_mut(sim, id) {
        b.check_area_rect = [x0, z0, x1, z1];
    }
    register_enemy_area_watch(sim, id)
}

/// `PropertyBuilding_RegisterEnemyAreaWatch` (`0x00507DA0`, also `0x2B03`; re/arming-gates 3.D1):
/// filter mode `0xA410` (enemy of the owner, alive, no ghost), register the query over
/// `check_area_rect` for the owner slot (initial fill), store the key in
/// `check_area_id` and attach a watcher `count > 0` -> `0x2703` (group `0x80`) **immediately at the end** of the
/// events (`Module_AddValueWatcher`, K-49). An old query is **not** removed:
/// `UpdateWallAreaWatch` calls only at -1, `CaptureByPlayer` unregisters beforehand. If a
/// build step hits the second after capturing, its query including the watcher remains without a building
/// (W-275). Return value 0.
pub fn register_enemy_area_watch(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(rect) = building(sim, id).map(|b| b.check_area_rect) else {
        return 0;
    };
    let key = area::register(sim, ENEMY_WATCH, rect, i32::from(id.slot()));
    if let Some(b) = building_mut(sim, id) {
        b.check_area_mode = ENEMY_WATCH;
        b.check_area_id = key;
    }
    if let Some(m) = sim.module_mut(id, super::INDEX) {
        m.base.add_value_watcher(
            WATCHER | GREATER,
            key,
            0,
            Message::new(msg::ABANDON_TO_NEUTRAL, &[]),
            GROUP,
        );
    }
    0
}

/// `PropertyBuilding_OnMsgAbandonToNeutral` (`0x00506C60`, `0x2703` from the watcher; re/arming-gates 3.D2,
/// W-272): queue master and then every member in list order for slot 8 (the change runs
/// in `process_owner_changes`; the gate opens there with sound, W-270), unregister the query,
/// `check_area_id = −1` and `0x2A03` after 1000 ms. Return value 0.
pub fn on_abandon_to_neutral(sim: &mut Sim, id: ObjectId) -> i32 {
    hand_over(sim, id, NEUTRAL_SLOT);
    unregister_own(sim, id);
    post(sim, id, msg::REGISTER_CAPTURE_WATCH);
    0
}

/// `PropertyBuilding_OnMsgRegisterCaptureWatch` (`0x00508CF0`, `0x2A03`; re/arming-gates 3.D2):
/// filter mode `0x4A010` (objects of the slots 0-7, alive, no ghost) register over the same rectangle for the
/// owner slot, `0x2903` after 1000 ms; no watcher. An old query is not
/// removed (like [`register_enemy_area_watch`]). Return value 0.
pub fn on_register_capture_watch(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(rect) = building(sim, id).map(|b| b.check_area_rect) else {
        return 0;
    };
    let key = area::register(sim, CAPTURE_WATCH, rect, i32::from(id.slot()));
    if let Some(b) = building_mut(sim, id) {
        b.check_area_mode = CAPTURE_WATCH;
        b.check_area_id = key;
    }
    post(sim, id, msg::CHECK_CAPTURE_AREA);
    0
}

/// `PropertyBuilding_OnMsgCheckCaptureArea` (`0x00507CD0`, `0x2903`; re/arming-gates 3.D2, W-272): if
/// the query exists, its list is not empty and all entries belong to the slot of the first
/// (`GameObject_GetOwnerSlot`, here the slot of the ID), that slot captures
/// ([`capture_by_player`]); otherwise `0x2903` after 1000 ms. Listed corpses belong to slot 8 (K-296):
/// if only they are in the list, slot 8 "captures". Return value 0.
pub fn on_check_capture_area(sim: &mut Sim, id: ObjectId) -> i32 {
    let key = building(sim, id).map_or(-1, |b| b.check_area_id);
    let owner = sim.state.area_queries.get(key).and_then(|q| {
        let p = q.list.first()?.slot();
        q.list.iter().all(|o| o.slot() == p).then_some(p)
    });
    match owner {
        Some(p) => capture_by_player(sim, id, p),
        None => post(sim, id, msg::CHECK_CAPTURE_AREA),
    }
    0
}

/// `PropertyBuilding_CaptureByPlayer` (`0x00506E10`; re/arming-gates 3.D2): unregister the query,
/// `check_area_id = −1`, queue master and members for `p` (`queue_owner_change` skips the
/// same slot: for `p = 8` nothing changes, K-296) and `0x2B03` after 1000 ms (enemy watch of the
/// new owner).
pub fn capture_by_player(sim: &mut Sim, id: ObjectId, p: u8) {
    unregister_own(sim, id);
    hand_over(sim, id, p);
    post(sim, id, msg::REGISTER_ENEMY_WATCH);
}

/// Queue master, then the members in list order for `slot` (`GameWorld_QueueOwnerChange`
/// with every stored ID).
fn hand_over(sim: &mut Sim, id: ObjectId, slot: u8) {
    let slaves = building(sim, id).map_or_else(Vec::new, |b| b.slaves.clone());
    queue_owner_change(sim, id, slot);
    for s in slaves {
        queue_owner_change(sim, s, slot);
    }
}

/// `FUN_004B68E0(check_area_id)` and `check_area_id = −1`.
fn unregister_own(sim: &mut Sim, id: ObjectId) {
    let key = building(sim, id).map_or(-1, |b| b.check_area_id);
    area::unregister(sim, key);
    if let Some(b) = building_mut(sim, id) {
        b.check_area_id = -1;
    }
}

/// `Module_PostDelayedMessage(1000, {code}, 0, 0x80)`.
fn post(sim: &mut Sim, id: ObjectId, code: u32) {
    if let Some(m) = sim.module_mut(id, super::INDEX) {
        m.base
            .post_delayed_message(WATCH_DELAY_MS, Message::new(code, &[]), false, GROUP);
    }
}

#[cfg(test)]
mod tests {
    use super::super::fort_testkit::*;
    use super::super::gate::{SFX_DOOR, close_door};
    use super::super::{INDEX, flags, has_flags};
    use super::*;
    use crate::command::CommandKind;
    use crate::messaging::{send, send_mut};
    use crate::module::base::ModuleEvent;
    use crate::module::base::event_flags::{ACTIVE, TIMER};
    use crate::state::SimEvent;

    /// Gate on the south wall (tiles 11 ... 14 of row 15), as in the gate tests.
    const SOUTH_GATE: (f32, f32) = (13.0 * 128.0, 15.5 * 128.0);
    /// Hull rectangle of the 6 x 6 ring around (10 ... 15, 10 ... 15) (re/arming-gates 9.D).
    const FORT_RECT: [i32; 4] = [10, 10, 15, 15];
    /// Parts of the ring with gate: 4 corners + 16 - 4 straight pieces + gate.
    const PARTS_WITH_GATE: usize = 17;

    fn step(sim: &mut Sim) -> Vec<SimEvent> {
        crate::tick::step(sim.state, sim.rules, &[])
    }

    /// Steps (`tick::step`) until `cond` holds; the events end up in `events`.
    fn steps_until(
        sim: &mut Sim,
        max: usize,
        events: &mut Vec<SimEvent>,
        cond: impl Fn(&Sim) -> bool,
    ) -> usize {
        for n in 1..=max {
            events.extend(step(sim));
            if cond(sim) {
                return n;
            }
        }
        panic!("condition not reached after {max} steps");
    }

    fn objects(sim: &Sim, guids: &[u32]) -> Vec<ObjectId> {
        sim.state
            .objects()
            .filter(|o| guids.contains(&o.guid))
            .map(|o| o.id)
            .collect()
    }

    /// Parts of the group (corners, wall pieces, gate) in slot `slot`.
    fn parts_of(sim: &Sim, slot: u8) -> Vec<ObjectId> {
        objects(sim, &[CORNER, WALL, GATE])
            .into_iter()
            .filter(|id| id.slot() == slot && !sim.state.destroy_queue.contains(id))
            .collect()
    }

    /// The master (corner at (10, 10)) under its current ID.
    fn master(sim: &Sim) -> ObjectId {
        objects(sim, &[CORNER])
            .into_iter()
            .find(|&c| sim.object(c).unwrap().tile == (10, 10))
            .expect("master corner")
    }

    fn gate(sim: &Sim) -> ObjectId {
        objects(sim, &[GATE])[0]
    }

    fn area_id(sim: &Sim, id: ObjectId) -> i32 {
        building(sim, id).unwrap().check_area_id
    }

    /// The only query of the world (key, mode, player, list).
    fn only_query(sim: &Sim) -> (i32, u32, i32, Vec<ObjectId>) {
        let qs = &sim.state.area_queries.queries;
        assert_eq!(qs.len(), 1, "exactly one query: {qs:?}");
        let (&k, q) = qs.iter().next().unwrap();
        (k, q.mode, q.player, q.list.clone())
    }

    fn watchers(sim: &Sim, id: ObjectId) -> Vec<ModuleEvent> {
        sim.module(id, INDEX)
            .unwrap()
            .base
            .events
            .iter()
            .filter(|e| e.flags & WATCHER != 0)
            .cloned()
            .collect()
    }

    /// Elapsed time of the timer with the message `code` (in `events`).
    fn timer_elapsed(sim: &Sim, id: ObjectId, code: u32) -> Option<i32> {
        sim.module(id, INDEX)?
            .base
            .events
            .iter()
            .find(|e| e.flags & TIMER != 0 && e.msg.code == code)
            .map(|e| e.a)
    }

    /// The watcher of the enemy watch for the key `key` (re/arming-gates 3.D1).
    fn enemy_watcher(key: i32) -> ModuleEvent {
        ModuleEvent {
            flags: u32::from(GROUP) << 24 | WATCHER | GREATER | ACTIVE,
            a: key,
            b: 0,
            msg: Message::new(msg::ABANDON_TO_NEUTRAL, &[]),
        }
    }

    /// 6 x 6 ring (way a) of slot 0 with a closed gate on the south wall; `inner` is a finished
    /// building of slot 0 at (12, 12). The four wall pieces under the gate are destroyed afterward.
    fn gated_fort(sim: &mut Sim, inner: Option<u32>) -> Option<ObjectId> {
        finished_ring(sim, 0, (10, 10), 4, 4);
        let gate = spawn_at(sim, GATE, 0, SOUTH_GATE, 0.0);
        close_door(sim, gate, false);
        let inner = inner.map(|guid| spawn(sim, guid, 0, (12, 12)));
        run_steps(sim, 2);
        assert_eq!(parts_of(sim, 0).len(), PARTS_WITH_GATE);
        inner
    }

    /// An enemy is placed at (14, 13); steps (from the placing) until the watcher has sent `0x2703`
    /// (the query is gone), and until the whole group stands at slot 8.
    fn enemy_enters(sim: &mut Sim, events: &mut Vec<SimEvent>) -> (ObjectId, usize, usize) {
        let enemy = spawn(sim, SOLDIER, 1, (14, 13));
        assert_eq!(only_query(sim).3, [enemy], "enemy listed");
        let abandoned = steps_until(sim, 2, events, |s| s.state.area_queries.queries.is_empty());
        let neutral = |s: &Sim| parts_of(s, NEUTRAL_SLOT).len() == PARTS_WITH_GATE;
        let neutral = if neutral(sim) {
            abandoned
        } else {
            abandoned + steps_until(sim, 2, events, neutral)
        };
        (enemy, abandoned, neutral)
    }

    /// Steps until the next firing of `0x2903` at the master `m`: the timer first starts running
    /// (`a > 0`) and after firing stands newly created at 0 again.
    fn next_capture_check(sim: &mut Sim, m: ObjectId, events: &mut Vec<SimEvent>) -> usize {
        let elapsed = |s: &Sim| timer_elapsed(s, m, msg::CHECK_CAPTURE_AREA);
        let running = steps_until(sim, 4, events, |s| elapsed(s).is_some_and(|a| a > 0));
        running + steps_until(sim, 30, events, |s| elapsed(s) == Some(0))
    }

    // QUIRK W-271: kept
    #[test]
    fn a_finished_ring_from_the_loader_is_watched_at_once() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ring = finished_ring(&mut sim, 0, (10, 10), 4, 4);
        let b = building(&sim, ring[0]).unwrap();
        assert_eq!(b.check_area_rect, FORT_RECT);
        assert_eq!(b.check_area_mode, ENEMY_WATCH);
        assert_eq!(b.check_area_id, 1);
        assert_eq!(only_query(&sim), (1, ENEMY_WATCH, 0, Vec::new()));
        assert_eq!(watchers(&sim, ring[0]), [enemy_watcher(1)]);
        // The members do not watch.
        assert!(ring[1..].iter().all(|&s| area_id(&sim, s) == -1));
        assert!(ring[1..].iter().all(|&s| watchers(&sim, s).is_empty()));
        // A second `0x1B03` to the watched master changes nothing (`check_area_id ≠ −1`).
        let mut args = vec![19];
        args.extend(ring[1..].iter().map(|id| id.as_i32()));
        send(&mut sim, ring[0], &Message::new(msg::SET_SLAVE_LIST, &args));
        assert_eq!(only_query(&sim).0, 1);
        assert_eq!(watchers(&sim, ring[0]).len(), 1);
    }

    /// Build a ring via the command `0x18` with `n` colonists west of the ring and let `steps` steps
    /// run. Return value: step of the completion of the last part and step of the first
    /// query. As long as a part is unfinished, nobody watches.
    fn build_ring(sim: &mut Sim, n: i32, steps: usize) -> (Option<usize>, Option<usize>) {
        let workers = (0..n)
            .map(|i| spawn(sim, COLONIST, 0, (8, 12 + i)))
            .collect();
        give_wood(sim, 0, 1000);
        run(
            sim,
            0,
            CommandKind::PlaceBuildings {
                parts: fort_ring((10, 10), 4, 4),
                workers,
            },
        );
        assert_eq!(links_of(sim, master(sim)).len(), 19);
        let finished = |s: &Sim| {
            parts_of(s, 0)
                .iter()
                .all(|&p| has_flags(s, p, flags::CONSTRUCTED_FINISHED))
        };
        let (mut done, mut watched) = (None, None);
        for k in 1..=steps {
            step(sim);
            if done.is_none() && finished(sim) {
                done = Some(k);
            }
            if watched.is_none() && !sim.state.area_queries.queries.is_empty() {
                assert!(finished(sim), "watch before completion");
                watched = Some(k);
            }
        }
        (done, watched)
    }

    // QUIRK W-271: kept
    #[test]
    fn a_player_ring_is_watched_after_the_next_build_step() {
        // Way b (W-271): the command `0x18` creates all parts unfinished (`ObjDesc.flags = 0xD3`), its
        // `0x1B03` to the master registers nothing (`is_loading` false). Only a build step that
        // reaches the finished, intact group (`ForwardBuildTick` at the master) registers the watch.
        let rules = rules();
        // Two colonists (measured): the last part is finished in step 1227; the other
        // worker sends its build step to a finished part in the next step, which registers
        // the watch via the master.
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(build_ring(&mut sim, 2, 1300), (Some(1227), Some(1228)));
        let m = master(&sim);
        assert_eq!(building(&sim, m).unwrap().check_area_rect, FORT_RECT);
        assert_eq!(only_query(&sim), (1, ENEMY_WATCH, 0, Vec::new()));
        assert_eq!(watchers(&sim, m), [enemy_watcher(1)]);

        // One colonist alone (measured): whoever finishes the last part ends up in
        // `FindNextConstructionSite` (`complete`), a further step to the group does not come -
        // the ring stays unwatched until a build step reaches the master.
        let mut state = super::super::fort_testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(build_ring(&mut sim, 1, 3000), (Some(2781), None));
        let m = master(&sim);
        send_mut(&mut sim, m, &mut Message::new(msg::BUILD_TICK, &[1000, 0]));
        assert_eq!(only_query(&sim), (1, ENEMY_WATCH, 0, Vec::new()));
        assert_eq!(building(&sim, m).unwrap().check_area_rect, FORT_RECT);
    }

    // QUIRK K-49: kept
    #[test]
    fn a_watcher_added_by_the_last_event_waits_one_tick() {
        // K-49 (re/arming-gates 3.D4): the enemy already stands in the area.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let enemy = spawn(&mut sim, SOLDIER, 1, (12, 12));
        let m = bare_master(&mut sim);
        let due = timer(msg::REGISTER_ENEMY_WATCH, 0);
        // Case 1: the due timer is the only (last) event.
        sim.module_mut(m, INDEX).unwrap().base.events = vec![due.clone()];
        crate::module::tick(&mut sim, m, INDEX, 100);
        let key = area_id(&sim, m);
        assert_ne!(key, -1, "enemy watch registered");
        assert_eq!(sim.state.area_queries.get(key).unwrap().list, [enemy]);
        assert_eq!(watchers(&sim, m), [enemy_watcher(key)]);
        assert!(
            sim.state.owner_changes.is_empty(),
            "checked in the same tick"
        );
        crate::module::tick(&mut sim, m, INDEX, 100);
        assert_eq!(sim.state.owner_changes.first(), Some(&(m, NEUTRAL_SLOT)));
        assert_eq!(area_id(&sim, m), -1);
        assert!(watchers(&sim, m).is_empty());

        // Case 2: behind it a timer that is not due -> `0x2703` in the same tick.
        let mut state = super::super::fort_testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        spawn(&mut sim, SOLDIER, 1, (12, 12));
        let m = bare_master(&mut sim);
        let later = timer(msg::CLEAR_FIRES, 100_000);
        sim.module_mut(m, INDEX).unwrap().base.events = vec![due, later];
        crate::module::tick(&mut sim, m, INDEX, 100);
        assert_eq!(sim.state.owner_changes.first(), Some(&(m, NEUTRAL_SLOT)));
        assert_eq!(area_id(&sim, m), -1);
        assert!(sim.state.area_queries.queries.is_empty());
        assert!(watchers(&sim, m).is_empty(), "watcher fired and removed");
        assert_eq!(
            timer_elapsed(&sim, m, msg::REGISTER_CAPTURE_WATCH),
            Some(0),
            "0x2A03 created"
        );
    }

    /// Master of a finished ring without watch and without events (the watch of way a unregistered).
    fn bare_master(sim: &mut Sim) -> ObjectId {
        let m = finished_ring(sim, 0, (10, 10), 4, 4)[0];
        let key = area_id(sim, m);
        area::unregister(sim, key);
        building_mut(sim, m).unwrap().check_area_id = -1;
        let base = &mut sim.module_mut(m, INDEX).unwrap().base;
        base.events.clear();
        base.timers_ws.clear();
        m
    }

    /// Timer of the building group with delay `delay` (nothing elapsed yet).
    fn timer(code: u32, delay: i32) -> ModuleEvent {
        ModuleEvent {
            flags: u32::from(GROUP) << 24 | TIMER | ACTIVE,
            a: 0,
            b: delay,
            msg: Message::new(code, &[]),
        }
    }

    /// Tick group of the object (0 ticks in the first, 1 in the second `step` of an object tick).
    fn group(sim: &Sim, id: ObjectId) -> i8 {
        sim.object(id).unwrap().tick_group
    }

    /// Flow up to the capture query (W-272, re/arming-gates 9.D): fort with `inner` of slot 0, enemy
    /// at (14, 13), neutralizing, 11 ticks later `0x2A03`. Return value master (slot 8), the
    /// inner building and the enemy.
    fn neutral_fort(sim: &mut Sim, inner: u32) -> (ObjectId, ObjectId, ObjectId) {
        let inner = gated_fort(sim, Some(inner)).unwrap();
        let mut events = Vec::new();
        let (enemy, _, _) = enemy_enters(sim, &mut events);
        let m = master(sim);
        steps_until(sim, 30, &mut events, |s| {
            !s.state.area_queries.queries.is_empty()
        });
        assert_eq!(only_query(sim).3, [inner, enemy]);
        (m, inner, enemy)
    }

    // QUIRK W-272: kept
    #[test]
    fn an_enemy_in_the_fort_turns_the_whole_ring_neutral() {
        // W-272, re/arming-gates 9.D: own house in the fort, closed gate on the south wall.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = gated_fort(&mut sim, Some(HOUSE)).unwrap();
        let (old_master, old_gate) = (master(&sim), gate(&sim));
        assert!(has_flags(&sim, old_gate, flags::TORCLOSE));
        // The master ticks in the first `step` of an object tick (group 0): `0x2703` in the first
        // `step` after placing the enemy, the change of owner in the second (phase 0).
        assert_eq!(group(&sim, old_master), 0);
        let mut events = Vec::new();
        let (enemy, abandoned, neutral) = enemy_enters(&mut sim, &mut events);
        assert_eq!((abandoned, neutral), (1, 2));
        // All 17 parts (16 pieces and corners, gate) on slot 8; the group stands again with new
        // IDs: master, 16 members in the old order (the gate last).
        let m = master(&sim);
        assert_ne!(m, old_master);
        assert_eq!(m.slot(), NEUTRAL_SLOT);
        assert!(parts_of(&sim, 0).is_empty());
        let links = links_of(&sim, m);
        assert_eq!(links.len(), PARTS_WITH_GATE - 1);
        assert!(links.iter().all(|&s| master_of(&sim, s) == m));
        assert!(links.iter().all(|s| s.slot() == NEUTRAL_SLOT));
        let g = gate(&sim);
        assert_eq!(links.last(), Some(&g));
        // Query and watcher gone, `0x2A03` waits.
        assert_eq!(area_id(&sim, m), -1);
        assert!(sim.state.area_queries.queries.is_empty());
        assert!(watchers(&sim, m).is_empty());
        assert_eq!(timer_elapsed(&sim, m, msg::REGISTER_CAPTURE_WATCH), Some(0));
        // The closed gate opened with sound in the process (W-270); the house stays with slot 0.
        assert!(has_flags(&sim, g, flags::TOROPEN) && !has_flags(&sim, g, flags::TORCLOSE));
        let sounds = events
            .iter()
            .filter(|e| matches!(e, SimEvent::Sound { object, sound } if *object == g && *sound == SFX_DOOR))
            .count();
        assert_eq!(sounds, 1);
        assert!(sim.object(house).is_some());
        // Savegame with a waiting `0x2A03` (without a query) loads.
        crate::save::testkit::save_and_load(sim.state, &rules);
        // 11 ticks (22 `step`) after `0x2703` the capture query (initial fill: house and
        // enemy). The new master stayed in group 0 on the change of owner.
        assert_eq!(group(&sim, m), 0);
        let registered = neutral
            + steps_until(&mut sim, 30, &mut events, |s| {
                !s.state.area_queries.queries.is_empty()
            });
        assert_eq!(registered - abandoned, 22);
        let (key, mode, player, list) = only_query(&sim);
        assert_eq!((mode, player), (CAPTURE_WATCH, i32::from(NEUTRAL_SLOT)));
        assert_eq!(list, [house, enemy]);
        assert_eq!(area_id(&sim, m), key);
        assert_eq!(building(&sim, m).unwrap().check_area_mode, CAPTURE_WATCH);
        assert!(
            watchers(&sim, m).is_empty(),
            "capture watch without a watcher"
        );
        // A further 11 ticks (22 `step`) later `0x2903`: two owners -> again in 1000 ms.
        assert_eq!(next_capture_check(&mut sim, m, &mut events), 22);
        assert_eq!(only_query(&sim).0, key);
        assert!(sim.state.owner_changes.is_empty());
    }

    // QUIRK W-272: kept
    #[test]
    fn two_owners_in_the_fort_keep_it_neutral() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (m, house, enemy) = neutral_fort(&mut sim, HOUSE);
        let mut events = Vec::new();
        for _ in 0..3 {
            assert_eq!(next_capture_check(&mut sim, m, &mut events), 22);
            assert_eq!(only_query(&sim).3, [house, enemy]);
            assert!(sim.state.owner_changes.is_empty());
            assert_eq!(parts_of(&sim, NEUTRAL_SLOT).len(), PARTS_WITH_GATE);
        }
        // Savegame with a running capture query and a waiting `0x2903` loads.
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert_eq!(loaded.area_queries, sim.state.area_queries);
    }

    // QUIRK W-272: kept
    #[test]
    fn the_last_owner_standing_captures_the_fort() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (m, house, enemy) = neutral_fort(&mut sim, HOUSE);
        let mut events = Vec::new();
        assert_eq!(next_capture_check(&mut sim, m, &mut events), 22);
        let neutral_group = group(&sim, m);
        // House destroyed and removed: only the enemy is still listed.
        crate::world::destroy_object(&mut sim, house);
        assert_eq!(only_query(&sim).3, [enemy]);
        // Next `0x2903` (22 `step` later) -> `CaptureByPlayer(1)`: query gone, the group
        // goes to slot 1 in the next phase 0 `step`.
        let captured = steps_until(&mut sim, 30, &mut events, |s| {
            s.state.area_queries.queries.is_empty()
        });
        assert_eq!(captured, 22);
        assert_eq!(sim.state.owner_changes.len(), PARTS_WITH_GATE);
        let owned = captured
            + steps_until(&mut sim, 2, &mut events, |s| {
                parts_of(s, 1).len() == PARTS_WITH_GATE
            });
        assert_eq!(owned, captured + 1);
        let m1 = master(&sim);
        assert_eq!(m1.slot(), 1);
        assert_eq!(area_id(&sim, m1), -1);
        let links = links_of(&sim, m1);
        assert_eq!(links.len(), PARTS_WITH_GATE - 1);
        assert!(links.iter().all(|&s| master_of(&sim, s) == m1));
        assert!(
            has_flags(&sim, gate(&sim), flags::TOROPEN),
            "the gate stays open"
        );
        assert_eq!(timer_elapsed(&sim, m1, msg::REGISTER_ENEMY_WATCH), Some(0));
        // 11 ticks after `CaptureByPlayer` the enemy watch for slot 1 (the own soldier does not
        // count). The master came from group 0 into group 1 on the change of owner and has ticked
        // one `step` later since: 22 + 1 `step`.
        assert_eq!((neutral_group, group(&sim, m1)), (0, 1));
        let watched = owned
            + steps_until(&mut sim, 30, &mut events, |s| {
                !s.state.area_queries.queries.is_empty()
            });
        assert_eq!(watched - captured, 23);
        let (key, mode, player, list) = only_query(&sim);
        assert_eq!((mode, player, list), (ENEMY_WATCH, 1, Vec::new()));
        assert_eq!(area_id(&sim, m1), key);
        assert_eq!(watchers(&sim, m1), [enemy_watcher(key)]);
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert_eq!(loaded.area_queries, sim.state.area_queries);
    }

    // QUIRK W-272: kept
    #[test]
    fn a_foreign_tower_in_the_fort_blocks_the_capture() {
        // re/arming-gates 8 D3: towers are not members, keep their owner and count in the
        // capture list.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let (m, tower, enemy) = neutral_fort(&mut sim, TOWER);
        assert_eq!(tower.slot(), 0);
        assert!(has_flags(&sim, tower, flags::CONSTRUCTED_FINISHED));
        assert!(!links_of(&sim, m).contains(&tower));
        let mut events = Vec::new();
        for _ in 0..3 {
            assert_eq!(next_capture_check(&mut sim, m, &mut events), 22);
            assert_eq!(only_query(&sim).3, [tower, enemy]);
            assert_eq!(parts_of(&sim, NEUTRAL_SLOT).len(), PARTS_WITH_GATE);
        }
        crate::world::destroy_object(&mut sim, tower);
        steps_until(&mut sim, 30, &mut events, |s| {
            parts_of(s, 1).len() == PARTS_WITH_GATE
        });
    }

    // QUIRK K-296: kept
    #[test]
    fn corpses_only_hand_the_ring_to_slot_8_and_it_stays_capturable() {
        // K-296, corrected: corpses belong to slot 8 and stay listed; only corpses -> slot 8
        // "captures" (no change of owner) and gets an enemy watch. It triggers on every object
        // of the slots 0-7, because `GameWorld_HasEnemyRelation` (`0x00476290`) returns true for `(8, q)` -
        // the group does not stay neutral forever.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        gated_fort(&mut sim, None);
        let mut events = Vec::new();
        let (enemy, _, _) = enemy_enters(&mut sim, &mut events);
        let m = master(&sim);
        steps_until(&mut sim, 30, &mut events, |s| {
            !s.state.area_queries.queries.is_empty()
        });
        assert_eq!(only_query(&sim).3, [enemy]);
        // The enemy dies in the fort: `DIED`, possession to slot 8, it stays listed (new ID).
        send(&mut sim, enemy, &Message::new(0x110, &[]));
        steps_until(&mut sim, 2, &mut events, |s| s.object(enemy).is_none());
        let corpse = only_query(&sim).3;
        assert_eq!(corpse.len(), 1);
        assert_eq!(corpse[0].slot(), NEUTRAL_SLOT);
        // `0x2903`: only slot 8 listed -> `CaptureByPlayer(8)`: query gone, no change of owner.
        steps_until(&mut sim, 30, &mut events, |s| {
            s.state.area_queries.queries.is_empty()
        });
        assert!(sim.state.owner_changes.is_empty());
        assert_eq!(master(&sim), m, "same ID");
        assert_eq!(timer_elapsed(&sim, m, msg::REGISTER_ENEMY_WATCH), Some(0));
        // 11 ticks later the enemy watch for slot 8; the corpse (`DIED`) does not hit the filter.
        steps_until(&mut sim, 30, &mut events, |s| {
            !s.state.area_queries.queries.is_empty()
        });
        let (key, mode, player, list) = only_query(&sim);
        assert_eq!((mode, player, list), (ENEMY_WATCH, 8, Vec::new()));
        assert_eq!(watchers(&sim, m), [enemy_watcher(key)]);
        run_steps(&mut sim, 40);
        assert_eq!(
            only_query(&sim).0,
            key,
            "without an object in the area nothing triggers"
        );
        // A colonist of slot 0 enters the fort: enemy of slot 8 -> `0x2703` in the next
        // building tick (without change of owner), then the capture watch again.
        let colonist = spawn(&mut sim, COLONIST, 0, (12, 12));
        assert_eq!(only_query(&sim).3, [colonist]);
        let abandoned = steps_until(&mut sim, 2, &mut events, |s| {
            s.state.area_queries.queries.is_empty()
        });
        assert!(abandoned <= 2);
        assert!(sim.state.owner_changes.is_empty());
        assert_eq!(timer_elapsed(&sim, m, msg::REGISTER_CAPTURE_WATCH), Some(0));
        steps_until(&mut sim, 30, &mut events, |s| {
            !s.state.area_queries.queries.is_empty()
        });
        let (_, mode, _, list) = only_query(&sim);
        assert_eq!((mode, list), (CAPTURE_WATCH, vec![colonist]));
        // One owner -> slot 0 captures.
        steps_until(&mut sim, 30, &mut events, |s| {
            parts_of(s, 0).len() == PARTS_WITH_GATE
        });
    }

    // QUIRK W-275: kept
    #[test]
    fn a_repair_step_in_the_capture_gap_leaks_a_watch() {
        // W-275: `CaptureByPlayer` sets `check_area_id = −1` and registers the
        // enemy watch only after 1000 ms (`0x2B03`). If in this second a build step reaches the
        // finished, intact group, `UpdateWallAreaWatch` already registers a watch; `0x2B03`
        // then overwrites `check_area_id` without unregistering the first - the query A remains without a
        // building (the original does not remove it); its watcher still hangs on the master and triggers.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ring = finished_ring(&mut sim, 0, (10, 10), 4, 4);
        // As after `0x2703`: the watcher of the first watch has fired and is gone.
        sim.module_mut(ring[0], INDEX).unwrap().base.events.clear();
        capture_by_player(&mut sim, ring[0], 1);
        assert!(sim.state.area_queries.queries.is_empty());
        crate::world::process_owner_changes(&mut sim);
        let m = master(&sim);
        assert_eq!(m.slot(), 1);
        give_wood(&mut sim, 1, 1000);
        // A damaged member is repaired: as long as it is not intact, nobody watches.
        let slave = links_of(&sim, m)[4];
        sim.object_mut(slave)
            .unwrap()
            .attributes
            .set_current_fraction(14, 99_000);
        for _ in 0..100 {
            if sim.object(slave).unwrap().attributes.is_full(14) {
                break;
            }
            send_mut(
                &mut sim,
                slave,
                &mut Message::new(msg::BUILD_TICK, &[1000, 0]),
            );
            assert!(sim.state.area_queries.queries.is_empty());
        }
        // The next step on the intact member goes to the master -> watch A.
        send_mut(
            &mut sim,
            slave,
            &mut Message::new(msg::BUILD_TICK, &[1000, 0]),
        );
        let (a, mode, player, _) = only_query(&sim);
        assert_eq!((mode, player), (ENEMY_WATCH, 1));
        assert_eq!(area_id(&sim, m), a);
        // `0x2B03` (building ticks of the master) -> watch B overwrites `check_area_id`.
        for _ in 0..20 {
            if area_id(&sim, m) != a {
                break;
            }
            crate::module::tick(&mut sim, m, INDEX, 100);
        }
        let b = area_id(&sim, m);
        assert_eq!(b, a + 1);
        let keys: Vec<i32> = sim.state.area_queries.queries.keys().copied().collect();
        assert_eq!(keys, [a, b], "query A without building");
        assert_eq!(watchers(&sim, m), [enemy_watcher(a), enemy_watcher(b)]);
        // The savegame loads (a query without a building is reachable).
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert_eq!(loaded.area_queries, sim.state.area_queries);
    }

    // QUIRK W-272: kept
    #[test]
    fn a_destroyed_master_unregisters_its_watch() {
        // `PropertyBuilding_Destruct` (`0x00508400`) unregisters the query.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ring = finished_ring(&mut sim, 0, (10, 10), 4, 4);
        assert_eq!(only_query(&sim).0, 1);
        // The whole group dies via `0x1E03` at the master.
        for &id in &ring {
            sim.object_mut(id)
                .unwrap()
                .attributes
                .set_current_fraction(14, 0);
        }
        send(&mut sim, ring[0], &Message::new(msg::FINALLY_DEATH, &[]));
        assert!(sim.state.destroy_queue.contains(&ring[0]));
        assert_eq!(only_query(&sim).0, 1, "still there until destroyed");
        run_steps(&mut sim, 2);
        assert!(sim.object(ring[0]).is_none());
        assert!(sim.state.area_queries.queries.is_empty());
        let loaded = crate::save::testkit::save_and_load(sim.state, &rules);
        assert!(loaded.area_queries.queries.is_empty());
    }
}
