//! `Module_UpdateEvents` (`0x004FD7E0`): advance the timers and watchers of a module and send due
//! messages synchronously to the own object. Every module calls it from its tick.

use crate::id::ObjectId;
use crate::messaging::send;
use crate::sim::Sim;

use super::base::event_flags::{ACTIVE, EQUAL, GREATER, LESS, REPEAT, TIMER, WATCHER};

/// (A) pending group deletion, (B) all events in list order, (C) a deletion
/// noted during (B), (D) append the new timers from `m_TimersWS` from front to back
/// (they count from the next call).
///
/// In (B) timers and watchers are separate branches (`else if`, re/arming-gates 3.D4); an event
/// is never evaluated in both:
/// - Timer (`TIMER`): fires as soon as the accumulated time is **greater** than the delay;
///   repeating ones keep the remainder.
/// - Watcher (`WATCHER`, buildings only, `ModuleEvent_CheckWatcher` `0x004FD310`): `v` is the count
///   of the area query `a` ([`crate::area::AreaQueries::count`], unknown key -> 0); it sends
///   at `v > b` with `GREATER`, `v < b` with `LESS` or `v == b` with `EQUAL`; without `REPEAT`
///   afterwards `flags = 0` (the event is dropped).
///
/// An event that is **inactive** on entry (`ACTIVE` missing) stays in the list unchanged
/// (the original removes only in the active branch); it is reachable only from a corrupted savegame
/// (`save::check_state` does not reject it).
///
/// **K-49:** the original remembers the successor **before** processing. If there was none, the pass
/// ends after this event, even if its message appended a watcher at the back in the meantime
/// (`Module_AddValueWatcher` appends to `m_Events` immediately): that one waits for the
/// next tick. If there was one, the pass runs to the then current end and checks the
/// new watcher in the same tick.
pub fn update_events(sim: &mut Sim, id: ObjectId, index: usize, dt: i32) {
    let Some(m) = sim.module_mut(id, index) else {
        return;
    };
    m.base.apply_clear_events();
    let mut i = 0;
    // K-49: remember the successor before processing (`has_next`).
    while let Some((mut e, has_next)) = sim.module(id, index).and_then(|m| {
        m.base
            .events
            .get(i)
            .map(|e| (e.clone(), i + 1 < m.base.events.len()))
    }) {
        // Original `0x004FD7E0`: only an event that is active on entry is evaluated and, if it is
        // no longer active afterwards, removed (`PtrList_RemoveAt` is in the active branch). An
        // inactive one stays in the list unchanged. This is not reachable (`Module_AddActiveEvent`
        // sets `ACTIVE`), only from a corrupted savegame; `check_state` allows it.
        let was_active = e.flags & ACTIVE != 0;
        if was_active {
            if e.flags & TIMER != 0 {
                e.a = e.a.wrapping_add(dt);
                if e.a > e.b {
                    if let Some(m) = sim.module_mut(id, index) {
                        m.base.cur_prop_time = e.a;
                    }
                    send(sim, id, &e.msg);
                    if e.flags & REPEAT != 0 {
                        e.a = e.a.wrapping_sub(e.b);
                    } else {
                        e.flags = 0;
                    }
                }
            } else if e.flags & WATCHER != 0 {
                let v = sim.state.area_queries.count(e.a);
                let fires = (v > e.b && e.flags & GREATER != 0)
                    || (v < e.b && e.flags & LESS != 0)
                    || (v == e.b && e.flags & EQUAL != 0);
                if fires {
                    send(sim, id, &e.msg);
                    if e.flags & REPEAT == 0 {
                        e.flags = 0;
                    }
                }
            }
        }
        let Some(m) = sim.module_mut(id, index) else {
            return;
        };
        if !was_active || e.flags & ACTIVE != 0 {
            m.base.events[i] = e;
            i += 1;
        } else {
            m.base.events.remove(i);
        }
        if !has_next {
            break;
        }
    }
    let Some(m) = sim.module_mut(id, index) else {
        return;
    };
    m.base.apply_clear_events();
    while let Some(e) = m.base.timers_ws.pop_front() {
        m.base.events.push(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};

    #[test]
    fn timers_fire_after_the_delay_keep_the_rest_and_start_counting_next_time() {
        let rules = Rules::synthetic("GUID 1\nNAME A\nVALUEID 100 6 1 0\nPROPID 0\n", 16);
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let id = create_object(&mut sim, &mut ObjectDesc::new(1, 0, [10.0, 0.0, 10.0])).unwrap();
        // Timers set the carrier (0xA7F) - visible on the object.
        let carrier = |n: i32| Message::new(0xA7F, &[n]);
        let base = &mut sim.module_mut(id, 0).unwrap().base;
        base.post_delayed_message(250, carrier(1), false, 0x80);
        base.post_delayed_message(100, carrier(2), true, 0x10);
        // The first call only takes them over (reverse order), nothing runs yet.
        update_events(&mut sim, id, 0, 100);
        let order: Vec<i32> = sim
            .module(id, 0)
            .unwrap()
            .base
            .events
            .iter()
            .map(|e| e.b)
            .collect();
        assert_eq!(order, [100, 250]);
        assert!(sim.object(id).unwrap().carrier.is_none());
        // 100 is not > 100; at 200 the repeating one fires (remainder 100), after that every tick.
        update_events(&mut sim, id, 0, 100);
        assert!(sim.object(id).unwrap().carrier.is_none());
        update_events(&mut sim, id, 0, 100);
        assert_eq!(sim.object(id).unwrap().carrier, ObjectId(2));
        assert_eq!(sim.module(id, 0).unwrap().base.cur_prop_time, 200);
        // 250: at 300 the one-shot fires and disappears.
        sim.object_mut(id).unwrap().carrier = ObjectId::NONE;
        update_events(&mut sim, id, 0, 100);
        assert_eq!(sim.object(id).unwrap().carrier, ObjectId(1));
        assert_eq!(sim.module(id, 0).unwrap().base.events.len(), 1);
        // Delete group <= 0x10: gone in the next call.
        sim.module_mut(id, 0).unwrap().base.clear_event_group(0x10);
        update_events(&mut sim, id, 0, 100);
        assert!(sim.module(id, 0).unwrap().base.events.is_empty());
    }

    // Watcher tests: building of the fort test world, query with an enemy in the area.
    use super::super::base::ModuleEvent;
    use super::super::base::event_flags::{EQUAL, GREATER, LESS};
    use crate::area::{ENEMY_WATCH, register};
    use crate::module::building::INDEX as BUILDING;
    use crate::module::building::fort_testkit::{self as kit, SOLDIER, TOWER, spawn};

    /// A watcher event (active) on the key `key` with threshold `threshold`.
    fn watcher(flags: u32, key: i32, threshold: i32, msg: Message) -> ModuleEvent {
        ModuleEvent {
            flags: WATCHER | ACTIVE | flags,
            a: key,
            b: threshold,
            msg,
        }
    }

    /// A due timer (fires in the next pass with `dt = 100`).
    fn due_timer(msg: Message) -> ModuleEvent {
        ModuleEvent {
            flags: TIMER | ACTIVE,
            a: 0,
            b: 50,
            msg,
        }
    }

    fn carrier(n: i32) -> Message {
        Message::new(0xA7F, &[n])
    }

    fn events_mut<'a>(sim: &'a mut Sim<'_>, id: ObjectId) -> &'a mut Vec<ModuleEvent> {
        &mut sim.module_mut(id, BUILDING).unwrap().base.events
    }

    #[test]
    fn a_watcher_fires_once_when_the_count_passes_the_threshold() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let tower = spawn(&mut sim, TOWER, 0, (30, 30));
        spawn(&mut sim, SOLDIER, 1, (12, 12));
        let key = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        assert_eq!(sim.state.area_queries.count(key), 1);
        // A query number without effect (response -1): it goes unnoticed.
        assert_eq!(send(&mut sim, tower, &Message::new(0x7F03, &[])), -1);
        let before = sim.object(tower).unwrap().carrier;

        // Without repeat the watcher is gone after firing.
        events_mut(&mut sim, tower).clear();
        events_mut(&mut sim, tower).push(watcher(GREATER, key, 0, carrier(5)));
        update_events(&mut sim, tower, BUILDING, 100);
        assert!(events_mut(&mut sim, tower).is_empty());
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(5));

        // With `REPEAT` it stays and fires again in the next pass.
        sim.object_mut(tower).unwrap().carrier = before;
        events_mut(&mut sim, tower).push(watcher(GREATER | REPEAT, key, 0, carrier(6)));
        update_events(&mut sim, tower, BUILDING, 100);
        assert_eq!(events_mut(&mut sim, tower).len(), 1);
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(6));
        sim.object_mut(tower).unwrap().carrier = before;
        update_events(&mut sim, tower, BUILDING, 100);
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(6));

        // Comparisons at count 1: `LESS 1` and `GREATER 1` and `EQUAL 2` do not match.
        events_mut(&mut sim, tower).clear();
        sim.object_mut(tower).unwrap().carrier = before;
        events_mut(&mut sim, tower).push(watcher(LESS, key, 1, carrier(7)));
        events_mut(&mut sim, tower).push(watcher(GREATER, key, 1, carrier(8)));
        events_mut(&mut sim, tower).push(watcher(EQUAL, key, 2, carrier(9)));
        update_events(&mut sim, tower, BUILDING, 100);
        assert_eq!(events_mut(&mut sim, tower).len(), 3);
        assert_eq!(sim.object(tower).unwrap().carrier, before);
        // `EQUAL` is added to `GREATER` with threshold 1, the third gets threshold 1 and `EQUAL`:
        // both match now, the first (`LESS 1`) still does not.
        events_mut(&mut sim, tower)[1].flags |= EQUAL;
        events_mut(&mut sim, tower)[2].b = 1;
        update_events(&mut sim, tower, BUILDING, 100);
        assert_eq!(events_mut(&mut sim, tower).len(), 1);
        assert_eq!(events_mut(&mut sim, tower)[0].msg, carrier(7));
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(9));

        // Count 0 (list empty): `GREATER 0` does not fire, the watcher stays.
        sim.state
            .area_queries
            .queries
            .get_mut(&key)
            .unwrap()
            .list
            .clear();
        events_mut(&mut sim, tower).clear();
        sim.object_mut(tower).unwrap().carrier = before;
        events_mut(&mut sim, tower).push(watcher(GREATER, key, 0, carrier(5)));
        update_events(&mut sim, tower, BUILDING, 100);
        assert_eq!(events_mut(&mut sim, tower).len(), 1);
        assert_eq!(sim.object(tower).unwrap().carrier, before);
        // An unknown key counts 0 (`EQUAL 0` fires).
        events_mut(&mut sim, tower).clear();
        events_mut(&mut sim, tower).push(watcher(EQUAL, 999, 0, carrier(4)));
        update_events(&mut sim, tower, BUILDING, 100);
        assert!(events_mut(&mut sim, tower).is_empty());
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(4));
    }

    #[test]
    fn timers_keep_their_order_with_watchers_between_them() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let tower = spawn(&mut sim, TOWER, 0, (30, 30));
        spawn(&mut sim, SOLDIER, 1, (12, 12));
        let key = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        // Every firing entry sets the carrier: the final value shows which came last.
        // Timer - watcher - timer: the last timer wins.
        events_mut(&mut sim, tower).extend([
            due_timer(carrier(1)),
            watcher(GREATER, key, 0, carrier(2)),
            due_timer(carrier(3)),
        ]);
        update_events(&mut sim, tower, BUILDING, 100);
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(3));
        assert!(events_mut(&mut sim, tower).is_empty());
        // Watcher - timer: the timer comes last; timer - watcher: the watcher.
        events_mut(&mut sim, tower)
            .extend([watcher(GREATER, key, 0, carrier(4)), due_timer(carrier(5))]);
        update_events(&mut sim, tower, BUILDING, 100);
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(5));
        events_mut(&mut sim, tower)
            .extend([due_timer(carrier(6)), watcher(GREATER, key, 0, carrier(7))]);
        update_events(&mut sim, tower, BUILDING, 100);
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(7));
        // A watcher is never evaluated as a timer: key and threshold (`a`, `b`)
        // stay unchanged when repeating.
        events_mut(&mut sim, tower).push(watcher(GREATER | REPEAT, key, 0, carrier(8)));
        update_events(&mut sim, tower, BUILDING, 100);
        update_events(&mut sim, tower, BUILDING, 100);
        let e = &events_mut(&mut sim, tower)[0];
        assert_eq!((e.a, e.b), (key, 0));
    }

    /// Only what the test can check: a watcher appended at the back **before** the pass is checked in
    /// the same pass. This also holds for a loop without the K-49 remembering; the
    /// actual K-49 case (appending **during** processing, `0x2B03` ->
    /// `REGISTER_ENEMY_WATCH`, re/arming-gates 3.D4) is covered by
    /// `building::watch::tests::a_watcher_added_by_the_last_event_waits_one_tick`.
    #[test]
    fn a_watcher_appended_before_the_pass_is_checked_in_it() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let tower = spawn(&mut sim, TOWER, 0, (30, 30));
        spawn(&mut sim, SOLDIER, 1, (12, 12));
        let key = register(&mut sim, ENEMY_WATCH, [10, 10, 15, 15], 0);
        // Two due timers; the first is not the last entry.
        events_mut(&mut sim, tower).extend([due_timer(carrier(1)), due_timer(carrier(2))]);
        // The watcher is added at the back via `add_value_watcher`.
        sim.module_mut(tower, BUILDING)
            .unwrap()
            .base
            .add_value_watcher(WATCHER | GREATER, key, 0, carrier(3), 0x80);
        update_events(&mut sim, tower, BUILDING, 100);
        // The watcher was behind the timers and fired last; all three are gone.
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(3));
        assert!(events_mut(&mut sim, tower).is_empty());
    }

    /// `Module_UpdateEvents` (`0x004FD7E0`): an event that is inactive on entry stays
    /// unchanged; only an active one that is no longer active afterwards is removed.
    #[test]
    fn an_inactive_event_stays_in_the_list() {
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let tower = spawn(&mut sim, TOWER, 0, (30, 30));
        let inactive = ModuleEvent {
            flags: TIMER,
            a: 0,
            b: 50,
            msg: carrier(1),
        };
        events_mut(&mut sim, tower).extend([inactive.clone(), due_timer(carrier(2))]);
        update_events(&mut sim, tower, BUILDING, 100);
        // The active, due timer is gone, the inactive one unchanged and not fired.
        assert_eq!(sim.object(tower).unwrap().carrier, ObjectId(2));
        assert_eq!(events_mut(&mut sim, tower).as_slice(), [inactive]);
    }
}
