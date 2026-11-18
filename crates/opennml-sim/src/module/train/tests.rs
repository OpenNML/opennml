//! Tests of the states of train and multipart objects and the event bits, and of the train module:
//! init with consist, queries, drive curve, tick, drive step.

use super::testkit::*;
use super::*;
use crate::fx::Fx;
use crate::module::multipart::{self, MultiHead, MultiPart};
use crate::sim::Sim;
use crate::world::{object_event_bits, set_object_event_bit};

// QUIRK K-243: deviation
#[test]
fn train_and_multipart_states_are_created_with_their_defaults() {
    // Only permanent things: module kind, flags, render bits, direction, module distribution. What
    // init, queries and tick fill later (slaves, master, rear part type, drawbar) does not belong
    // here; the defaults of the structures are checked directly against `default()`.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let loco = spawn(&mut sim, LOCO, 0, (10, 10));
    let tender = spawn(&mut sim, TENDER, 0, (10, 12));
    let wagon = spawn(&mut sim, WAGON, 0, (10, 14));
    let cart = spawn(&mut sim, CART, 0, (20, 10));
    let tail = spawn(&mut sim, CART_TAIL, 0, (20, 12));
    for (id, flags) in [(loco, 512), (tender, 256), (wagon, 256)] {
        let m = sim.module(id, INDEX).expect("train module present");
        assert!(matches!(m.kind, ModuleKind::Train(_)), "{id:?}");
        assert_eq!(m.base.flags, flags, "flags from 340001");
        assert_eq!(train(&sim, id).expect("train state").outside, 0, "{id:?}");
        // Train `|= 3`.
        assert_eq!(sim.object(id).unwrap().render_bits & 3, 3, "{id:?}");
        assert_eq!(sim.object(id).unwrap().direction, Fx(0), "direction 0");
    }
    // The wagon additionally has its quarters with capacity 10 (attribute 0x17).
    let q = sim.module(wagon, 23).expect("quarters of the wagon");
    assert!(matches!(&q.kind, ModuleKind::Quarter(q) if q.max == 10));
    // Front part carries module 11, rear part module 51; rear part `= 3`.
    let m = sim.module(cart, multipart::HEAD).expect("MultiHead");
    assert!(matches!(m.kind, ModuleKind::MultiHead(_)));
    let m = sim.module(tail, multipart::PART).expect("MultiPart");
    assert!(matches!(m.kind, ModuleKind::MultiPart(_)));
    assert_eq!(sim.object(tail).unwrap().render_bits, 3);
    assert!(multipart::part(&sim, cart).is_none() && multipart::head(&sim, tail).is_none());
    assert!(train(&sim, cart).is_none() && train_mut(&mut sim, tail).is_none());

    // Defaults of the structures (K-243: the outside status starts with 0, A7).
    let t = Train::default();
    assert_eq!((t.outside, t.master), (0, ObjectId::NONE));
    assert!(t.slaves.is_empty() && t.status == 0 && t.target.is_none());
    assert!(ModuleKind::Train(Box::new(t)).check((0, 0)).is_ok());
    let h = MultiHead::default();
    assert_eq!(h.linked, ObjectId::NONE);
    let p = MultiPart::default();
    assert_eq!(
        (p.prev, p.next, p.offset),
        (ObjectId::NONE, ObjectId::NONE, 0)
    );
    assert!(ModuleKind::MultiPart(p).check((0, 0)).is_ok());
}

#[test]
fn event_bits_set_and_clear_per_object() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let loco = spawn(&mut sim, LOCO, 0, (10, 10));
    let other = spawn(&mut sim, LOCO, 0, (10, 20));
    assert_eq!(object_event_bits(&sim, loco), 0);
    set_object_event_bit(&mut sim, loco, 8, true);
    assert_eq!(object_event_bits(&sim, loco), 0x100);
    set_object_event_bit(&mut sim, loco, 0, true);
    assert_eq!(object_event_bits(&sim, loco), 0x101);
    set_object_event_bit(&mut sim, loco, 8, false);
    assert_eq!(object_event_bits(&sim, loco), 0x1);
    // Foreign object without an entry.
    assert_eq!(object_event_bits(&sim, other), 0);
    assert!(!sim.state.object_event_bits.contains_key(&other));
    // Clearing without an entry creates it anyway (0x00481610).
    set_object_event_bit(&mut sim, other, 9, false);
    assert_eq!(sim.state.object_event_bits.get(&other), Some(&0));
}

// --- Train module: init with consist, queries, drive curve, tick, drive step ---

use crate::fx::{FxVec3, fixed_mul};
use crate::message::Message;
use crate::messaging::{send, send_mut};
use crate::object::status as object_status;
use crate::rules::Rules;
use crate::state::{SimEvent, SimState};
use crate::tick::step;
use crate::world::destroy_object;

/// Acceleration curve per object tick (re/train-multipart 9.1).
const ACCEL_CURVE: [i32; 24] = [
    0, 51, 153, 256, 358, 460, 563, 665, 768, 870, 972, 1075, 1177, 1280, 1382, 1484, 1587, 1689,
    1792, 1894, 1996, 2099, 2201, 2304,
];
/// Braking ticks from the cruise speed after the start of braking (re/train-multipart 9.1).
const BRAKE_CURVE: [i32; 14] = [
    2211, 2048, 1884, 1720, 1556, 1392, 1228, 1064, 901, 737, 573, 409, 245, 81,
];

/// Track (10, 20) ... (49, 20) direction 0, the locomotive at `tile` (slot 0).
fn scene(tile: (i32, i32)) -> (Rules, SimState, ObjectId) {
    let rules = rules();
    let mut state = state(&rules);
    let loco = {
        let mut sim = Sim::new(&mut state, &rules);
        lay_track(&mut sim, (10, 20), (49, 20));
        spawn_loco(&mut sim, 0, tile)
    };
    (rules, state, loco)
}

/// One object tick (two engine ticks); returns the events of both ticks.
fn object_tick(state: &mut SimState, rules: &Rules) -> Vec<SimEvent> {
    let mut events = step(state, rules, &[]);
    events.extend(step(state, rules, &[]));
    events
}

fn tr(state: &mut SimState, rules: &Rules, id: ObjectId) -> Train {
    train(&Sim::new(state, rules), id)
        .expect("train state")
        .clone()
}

fn grid(state: &SimState, id: ObjectId) -> FxVec3 {
    state.object(id, false).expect("object present").grid
}

fn msg(state: &mut SimState, rules: &Rules, id: ObjectId, code: u32, args: &[i32]) -> i32 {
    send(&mut Sim::new(state, rules), id, &Message::new(code, args))
}

/// Locomotive, tender, wagon.
fn consist(state: &mut SimState, rules: &Rules, loco: ObjectId) -> [ObjectId; 3] {
    let slaves = tr(state, rules, loco).slaves;
    assert_eq!(slaves.len(), 2, "tender and wagon coupled");
    [loco, slaves[0], slaves[1]]
}

fn sound(events: &[SimEvent], id: ObjectId, action: i32) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(e, SimEvent::Sound3d { object, action: a, flags: 0x14, .. }
                if *object == id && *a == action)
        })
        .count()
}

// QUIRK K-240: kept
#[test]
fn the_locomotive_snaps_to_the_nearest_track_and_couples_tender_and_wagon() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let tracks = lay_track(&mut sim, (10, 20), (49, 20));
    // Across the track one tile aside: snaps to track (40, 20), direction 0 (track direction).
    let loco = spawn_dir(&mut sim, LOCO, 0, (40, 21), 90);
    let track = sim.object(tracks[30]).unwrap().grid;
    let o = sim.object(loco).unwrap();
    assert_eq!((o.grid.x, o.grid.z), (track.x, track.z));
    assert_eq!(o.tile, (40, 20));
    assert_eq!(o.direction, Fx(0));
    let (lx, lz) = (o.grid.x.0, o.grid.z.0);
    let t = train(&sim, loco).unwrap().clone();
    assert!(!t.is_slave);
    assert_eq!(t.master, ObjectId::NONE);
    let [tender, wagon] = t.slaves[..] else {
        panic!("slaves {:?}", t.slaves)
    };
    // Creation locomotive -> tender -> wagon (nested, IDs ascending).
    assert!(loco.index() < tender.index() && tender.index() < wagon.index());
    assert_eq!(sim.object(tender).unwrap().guid, TENDER);
    assert_eq!(sim.object(wagon).unwrap().guid, WAGON);
    for s in [tender, wagon] {
        let st = train(&sim, s).unwrap();
        assert_eq!(st.master, loco, "0x621 to {s:?}");
        assert!(st.is_slave && st.slaves.is_empty());
        assert_eq!(sim.object(s).unwrap().direction, Fx(0));
    }
    // Deviation from the original (x' = x*c - z*s): locomotive hitch (0, 0, 100) lies behind the locomotive at
    // direction 0; coupling = hitch point - vvorne01 + vhinten01, SX = 128.
    let g = |id| sim.object(id).unwrap().grid;
    assert_eq!((g(tender).x.0, g(tender).z.0), (lx - 160 * 4096 / 128, lz));
    assert_eq!((g(wagon).x.0, g(wagon).z.0), (lx - 300 * 4096 / 128, lz));
    assert_eq!((t.dir, t.raster_dir), ((4096, 0), (1, 0)));
    for id in [loco, tender, wagon] {
        let o = sim.object(id).unwrap();
        assert_eq!(
            (o.anim.wanted, o.anim.current),
            (1, 1),
            "waiting animation {id:?}"
        );
        assert_eq!(train(&sim, id).unwrap().status, 0);
    }
}

// QUIRK E-146: kept
#[test]
fn train_parts_get_tender_and_wagon() {
    // `GameWorld_CreateObject` enters the name at `0x00486414`, before `GameObject_ApplyPlacement`
    // (`0x0048641C`), whose module init `Train_SetupWagonsAndSmoke` creates tender and wagon and
    // transfers the name with the locomotive as the old ID (`0x00530855`, `0x00530B8C`).
    let rules = rules();
    let mut state = state(&rules);
    let loco = {
        let mut sim = Sim::new(&mut state, &rules);
        lay_track(&mut sim, (10, 20), (49, 20));
        let mut desc = ObjectDesc::new(LOCO, 0, [30.0 * 128.0 + 64.0, 0.0, 20.0 * 128.0 + 64.0]);
        desc.flags |= desc_flags::PLACED;
        crate::world::create_object_named(&mut sim, &mut desc, Some("Express")).unwrap()
    };
    let [_, tender, wagon] = consist(&mut state, &rules, loco);
    let names = &state.names;
    let value = |id: ObjectId| Some(id.as_i32().to_string());
    assert_eq!(names.get("express").map(str::to_owned), value(loco));
    assert_eq!(names.get("expresstender").map(str::to_owned), value(tender));
    assert_eq!(names.get("expresswagon").map(str::to_owned), value(wagon));
    assert_eq!(names.len(), 3);
}

#[test]
fn a_locomotive_without_track_keeps_its_place() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    lay_track(&mut sim, (10, 20), (49, 20));
    // Five tiles beside the track: outside the search radius of 4 tiles.
    let loco = spawn_dir(&mut sim, LOCO, 0, (30, 25), 90);
    let o = sim.object(loco).unwrap();
    assert_eq!(o.tile, (30, 25));
    assert_eq!(
        (o.grid.x.0, o.grid.z.0),
        ((30 << 12) + 0x800, (25 << 12) + 0x800)
    );
    assert_eq!(o.direction, Fx(90 << 12));
    let (lx, lz) = (o.grid.x.0, o.grid.z.0);
    let t = train(&sim, loco).unwrap().clone();
    assert_eq!((t.dir, t.raster_dir), ((0, 4096), (0, 1)));
    // The consist is coupled anyway, behind the locomotive (-z).
    let [tender, wagon] = t.slaves[..] else {
        panic!("slaves {:?}", t.slaves)
    };
    let g = |id| sim.object(id).unwrap().grid;
    assert_eq!((g(tender).x.0, g(tender).z.0), (lx, lz - 5120));
    assert_eq!((g(wagon).x.0, g(wagon).z.0), (lx, lz - 9600));
}

// QUIRK K-241: kept
#[test]
fn departure_accelerates_along_the_curve_and_moves_the_whole_train() {
    // From (30, 20) the look-ahead does not brake until after the start-up.
    let (rules, mut state, loco) = scene((30, 20));
    let ids = consist(&mut state, &rules, loco);
    assert_eq!(msg(&mut state, &rules, loco, 0x121, &[]), 0);
    let t = tr(&mut state, &rules, loco);
    assert_eq!(t.status, status::MOVING | status::FORWARD | status::ACCEL);
    assert_eq!(
        (t.acc_time, t.speed, t.target, t.dir),
        (2400, 0, None, (4096, 0))
    );
    assert!(state.object(loco, false).unwrap().status & object_status::MOVE != 0);
    for id in ids {
        assert_eq!(
            state.object(id, false).unwrap().anim.wanted,
            0,
            "driving animation {id:?}"
        );
    }
    assert_eq!(sound(&state.events, loco, 12), 1, "departure: sound 12");
    for (i, &v) in ACCEL_CURVE.iter().enumerate() {
        let before = ids.map(|id| grid(&state, id));
        object_tick(&mut state, &rules);
        let t = tr(&mut state, &rules, loco);
        assert_eq!(t.speed, v, "tick {}", i + 1);
        for (k, &id) in ids.iter().enumerate() {
            let now = grid(&state, id);
            assert_eq!(
                now.x.0 - before[k].x.0,
                fixed_mul(4096, v),
                "tick {} {id:?}",
                i + 1
            );
            assert_eq!(now.z, before[k].z);
        }
    }
    let t = tr(&mut state, &rules, loco);
    assert_eq!(t.status, status::MOVING | status::FORWARD, "after 24 ticks");
    assert_eq!((t.acc_time, t.next_speed), (0, 2304));
}

// QUIRK K-245: kept
#[test]
fn the_train_brakes_before_the_track_end_and_sets_bit_0() {
    let (rules, mut state, loco) = scene((40, 20));
    let ids = consist(&mut state, &rules, loco);
    msg(&mut state, &rules, loco, 0x121, &[]);
    let mut braked = false;
    let mut ticks = 0;
    while tr(&mut state, &rules, loco).status != 0 {
        object_tick(&mut state, &rules);
        braked |= tr(&mut state, &rules, loco).status & status::BRAKING != 0;
        ticks += 1;
        assert!(ticks < 100, "train does not halt");
    }
    assert!(braked, "halt over the braking curve");
    let t = tr(&mut state, &rules, loco);
    // The halt leaves A = -100.
    assert_eq!((t.speed, t.next_speed, t.acc_time), (0, 0, -100));
    let o = state.object(loco, false).unwrap();
    assert_eq!(o.status & object_status::MOVE, 0);
    // K-245: look-ahead braking distance + 4 tiles - the train halts before the last track (49). The
    // sequence is deterministic (fixed point): 33 object ticks, halt on tile 47 at x = 196144 (~47.89).
    assert_eq!(ticks, 33, "object ticks until the halt");
    assert_eq!(o.tile, (47, 20), "halt tile");
    assert_eq!((o.grid.x.0, o.grid.z.0), (196_144, 83_968), "halt position");
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(object_event_bits(&sim, loco), 1, "event bit 0");
    for id in ids {
        assert_eq!(
            sim.object(id).unwrap().anim.wanted,
            1,
            "standing animation {id:?}"
        );
    }
}

// QUIRK K-241: kept
#[test]
fn stopping_uses_the_brake_curve_and_a_second_stop_restarts_it() {
    let (rules, mut state, loco) = scene((30, 20));
    let ids = consist(&mut state, &rules, loco);
    msg(&mut state, &rules, loco, 0x121, &[]);
    for _ in 0..24 {
        object_tick(&mut state, &rules);
    }
    let t = tr(&mut state, &rules, loco);
    assert_eq!(
        (t.speed, t.status),
        (2304, status::MOVING | status::FORWARD)
    );
    assert_eq!(msg(&mut state, &rules, loco, 0x321, &[]), 0);
    let t = tr(&mut state, &rules, loco);
    let braking = status::MOVING | status::FORWARD | status::BRAKING;
    assert_eq!(
        (t.speed, t.acc_time, t.next_speed, t.status),
        (2211, 1400, 2048, braking)
    );
    assert_eq!(sound(&state.events, loco, 13), 1, "braking: sound 13");
    assert_eq!(
        state.object(loco, false).unwrap().status & object_status::MOVE,
        0
    );
    for id in ids {
        assert_eq!(
            state.object(id, false).unwrap().anim.wanted,
            1,
            "standing animation {id:?}"
        );
    }
    for &v in &BRAKE_CURVE[..3] {
        object_tick(&mut state, &rules);
        assert_eq!(tr(&mut state, &rules, loco).speed, v);
    }
    // A second 0x321 during braking (0x0052FEE0 checks only bit 2): curve anew from the current
    // speed 1884 - TimeForSpeed(1884) = 1200, so v = Dist(1100, 1200) = 1884 once more.
    state.events.clear();
    msg(&mut state, &rules, loco, 0x321, &[]);
    let t = tr(&mut state, &rules, loco);
    assert_eq!(
        (t.speed, t.acc_time, t.next_speed, t.status),
        (1884, 1200, 1720, braking)
    );
    assert_eq!(sound(&state.events, loco, 13), 1);
    let mut speeds = Vec::new();
    while tr(&mut state, &rules, loco).status != 0 {
        object_tick(&mut state, &rules);
        speeds.push(tr(&mut state, &rules, loco).speed);
        assert!(speeds.len() < 30, "train does not halt");
    }
    let mut expected = vec![1884];
    expected.extend_from_slice(&BRAKE_CURVE[3..]);
    expected.push(0);
    assert_eq!(speeds, expected);
}

// QUIRK K-245: kept
#[test]
fn a_target_tile_stops_the_train_without_a_step() {
    let (rules, mut state, loco) = scene((30, 20));
    // Target tile three tiles ahead (command 0x29 with target tile, only from scripts).
    msg(&mut state, &rules, loco, 0x121, &[33, 20]);
    assert_eq!(tr(&mut state, &rules, loco).target, Some((33, 20)));
    let mut brake_tick = None;
    for k in 1..=40 {
        let before = grid(&state, loco);
        object_tick(&mut state, &rules);
        let t = tr(&mut state, &rules, loco);
        if t.status & status::BRAKING != 0 {
            // K-245: the braking command replaces the drive step of this tick.
            assert_eq!(grid(&state, loco), before, "no step in tick {k}");
            // Deterministic: start of braking in the 13th object tick at speed 1064 on tile 32 (x = 131119).
            assert_eq!(t.speed, 1064);
            brake_tick = Some(k);
            break;
        }
    }
    assert_eq!(brake_tick, Some(13), "start of braking at the target tile");
    let o = state.object(loco, false).unwrap();
    assert_eq!((o.tile, o.grid.x.0), ((32, 20), 131_119));
    while tr(&mut state, &rules, loco).status != 0 {
        object_tick(&mut state, &rules);
    }
    // The train halts just before target tile 33 (x = 135129 ~ 32.99).
    let o = state.object(loco, false).unwrap();
    assert_eq!((o.tile, o.grid.x.0), ((32, 20), 135_129), "halt tile");
}

// QUIRK K-245: kept
#[test]
fn departing_without_track_stops_in_the_next_tick() {
    // Locomotive on the last track, direction of travel into the void (re/train-multipart 9.1).
    let (rules, mut state, loco) = scene((49, 20));
    let start = grid(&state, loco);
    msg(&mut state, &rules, loco, 0x121, &[]);
    object_tick(&mut state, &rules);
    let t = tr(&mut state, &rules, loco);
    assert_eq!(
        t.status & status::BRAKING,
        status::BRAKING,
        "tick 1: start of braking"
    );
    assert_eq!(t.speed, 0);
    object_tick(&mut state, &rules);
    let t = tr(&mut state, &rules, loco);
    assert_eq!((t.status, t.speed), (0, 0), "tick 2: halt");
    assert_eq!(grid(&state, loco), start);
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(object_event_bits(&sim, loco) & 1, 1);
}

// QUIRK K-242: kept
#[test]
fn units_on_the_track_are_neither_pushed_nor_hurt() {
    let rules = rules();
    let mut state = state(&rules);
    let (colonist, loco) = {
        let mut sim = Sim::new(&mut state, &rules);
        lay_track(&mut sim, (10, 20), (49, 20));
        let c = spawn(&mut sim, COLONIST, 1, (36, 20));
        (c, spawn_loco(&mut sim, 0, (30, 20)))
    };
    let (cg, hp) = {
        let sim = Sim::new(&mut state, &rules);
        (
            sim.object(colonist).unwrap().grid,
            sim.attribute(colonist, 14),
        )
    };
    msg(&mut state, &rules, loco, 0x121, &[]);
    let mut furthest = 0;
    for _ in 0..80 {
        object_tick(&mut state, &rules);
        furthest = furthest.max(state.object(loco, false).unwrap().tile.0);
        // K-242: no pushing, no damage.
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(sim.object(colonist).unwrap().grid, cg);
        assert_eq!(sim.attribute(colonist, 14), hp);
    }
    assert!(furthest > 36, "the train drives through (up to {furthest})");
    assert_eq!(tr(&mut state, &rules, loco).status, 0);
}

/// The locomotive drives over the edge of the map: backward to -x or forward to +x (map 64 tiles).
fn leave_the_map(backward: bool) {
    let rules = rules();
    let mut state = state(&rules);
    let loco = {
        let mut sim = Sim::new(&mut state, &rules);
        if backward {
            lay_track(&mut sim, (0, 20), (20, 20));
            spawn_loco(&mut sim, 0, (10, 20))
        } else {
            lay_track(&mut sim, (45, 20), (63, 20));
            spawn_loco(&mut sim, 0, (55, 20))
        }
    };
    let ids = consist(&mut state, &rules, loco);
    let code = if backward { 0x221 } else { 0x121 };
    msg(&mut state, &rules, loco, code, &[]);
    let (dir_x, outside) = if backward { (-4096, 1) } else { (4096, 2) };
    let edge = |x: i32| if backward { -x } else { x - (64 << 12) };
    let mut deselected = 0;
    for k in 1..200 {
        let before = ids.map(|id| grid(&state, id));
        let speed = tr(&mut state, &rules, loco).speed;
        let events = object_tick(&mut state, &rules);
        deselected += events
            .iter()
            .filter(|e| matches!(e, SimEvent::Deselected { object } if *object == loco))
            .count();
        let t = tr(&mut state, &rules, loco);
        // Outside, the look-ahead checks nothing: no braking.
        assert_eq!(t.status & status::BRAKING, 0, "tick {k}");
        let lx = grid(&state, loco).x.0;
        if (0..64 << 12).contains(&lx) {
            assert_eq!((t.outside, deselected), (0, 0), "tick {k}");
        } else {
            assert_eq!(t.outside, outside, "tick {k}");
            assert_eq!(deselected, 1, "deselection exactly once (tick {k})");
        }
        if t.status == 0 {
            // K-244: > 15 tiles outside -> two drive steps back, halt outside.
            let over = edge(before[0].x.0);
            assert!(over > 15 << 12, "overshoot {over}");
            assert_eq!(speed, 2304);
            for (i, &id) in ids.iter().enumerate() {
                let now = grid(&state, id);
                assert_eq!(
                    now.x.0,
                    before[i].x.0 - 2 * fixed_mul(dir_x, speed),
                    "{id:?}"
                );
                assert_eq!(now.z, before[i].z);
            }
            assert_eq!(t.speed, 0);
            assert!(edge(lx) > 0, "the locomotive stands outside");
            let sim = Sim::new(&mut state, &rules);
            let bits = object_event_bits(&sim, loco);
            let side = if backward { 1 << 9 } else { 1 << 8 };
            assert_eq!(bits, side | 1, "event bits {bits:#x}");
            assert_eq!(sim.object(loco).unwrap().status & object_status::MOVE, 0);
            for id in ids {
                assert_eq!(
                    sim.object(id).unwrap().anim.wanted,
                    1,
                    "standing animation {id:?}"
                );
            }
            return;
        }
    }
    panic!("the locomotive does not leave the map");
}

// QUIRK K-244: kept
#[test]
fn leaving_the_map_backwards_holds_outside_and_sets_bit_9() {
    leave_the_map(true);
    // Forward accordingly: outside status 2, event bit 8.
    leave_the_map(false);
}

// QUIRK K-240, K-250: kept
#[test]
fn slaves_ignore_departure_and_unknown_codes_answer_0() {
    let (rules, mut state, loco) = scene((40, 20));
    let [_, tender, _] = consist(&mut state, &rules, loco);
    // K-240: departure only for GUID 2613.
    let before = tr(&mut state, &rules, tender);
    assert_eq!(msg(&mut state, &rules, tender, 0x121, &[]), 0);
    assert_eq!(tr(&mut state, &rules, tender), before);
    assert_eq!(tr(&mut state, &rules, loco).status, 0);
    // K-250: unknown codes -> 0; sparks (presentation, deviation from the original) -> 1 without effect.
    let lok = tr(&mut state, &rules, loco);
    for code in [0xC21, 0xD21, 0x21] {
        assert_eq!(msg(&mut state, &rules, loco, code, &[]), 0, "{code:#x}");
    }
    for code in [0x921, 0xA21] {
        assert_eq!(msg(&mut state, &rules, loco, code, &[]), 1, "{code:#x}");
    }
    assert_eq!(tr(&mut state, &rules, loco), lok);
    // 0xB21: travel vector and progress 0 always entered; at standstill -1, while driving next_speed.
    let mut q = Message::new(0xB21, &[7, 7, 7]);
    assert_eq!(
        send_mut(&mut Sim::new(&mut state, &rules), loco, &mut q),
        -1
    );
    assert_eq!(q.args, [4096, 0, 0]);
    msg(&mut state, &rules, loco, 0x121, &[]);
    object_tick(&mut state, &rules);
    object_tick(&mut state, &rules);
    let next = tr(&mut state, &rules, loco).next_speed;
    assert_eq!(next, 153);
    let mut q = Message::new(0xB21, &[]);
    assert_eq!(
        send_mut(&mut Sim::new(&mut state, &rules), loco, &mut q),
        next
    );
    assert_eq!(q.args, [4096, 0, 0]);
}

// QUIRK K-246: kept
#[test]
fn removing_the_locomotive_leaves_its_slaves() {
    let (rules, mut state, loco) = scene((40, 20));
    let [_, tender, wagon] = consist(&mut state, &rules, loco);
    let mut sim = Sim::new(&mut state, &rules);
    destroy_object(&mut sim, loco);
    assert!(sim.object(loco).is_none());
    // K-246: tender and wagon remain, with the old ID as master.
    for s in [tender, wagon] {
        assert!(sim.object(s).is_some(), "{s:?}");
        assert_eq!(train(&sim, s).unwrap().master, loco);
    }
}

// QUIRK K-275: kept
#[test]
fn the_whistle_repeats_every_900_ms_while_moving() {
    let (rules, mut state, loco) = scene((30, 20));
    msg(&mut state, &rules, loco, 0x121, &[]);
    let whistle = |events: &[SimEvent]| {
        events
            .iter()
            .filter_map(|e| match e {
                SimEvent::EffectAttached {
                    object, path, node, ..
                } if *object == loco && path == "data/effects/lok_toeff.pef" => Some(node.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(whistle(&state.events), ["toeff01", "toeff02"], "departure");
    state.events.clear();
    // Timer 900 ms: counts from the tick after being queued and does not fire until more than 900 ms have passed
    // (`Module_UpdateEvents`), so every tenth object tick - as long as bit 2 is set.
    let mut stopped_at = None;
    for k in 1..=80 {
        let moving = tr(&mut state, &rules, loco).status & status::MOVING != 0;
        let events = object_tick(&mut state, &rules);
        let w = whistle(&events);
        let due = k % 10 == 1 && k > 1 && stopped_at.is_none();
        if due && moving {
            assert_eq!(w, ["toeff01", "toeff02"], "tick {k}");
        } else {
            assert!(w.is_empty(), "tick {k}: {w:?}");
        }
        if due && !moving {
            stopped_at = Some(k);
        }
    }
    assert!(
        stopped_at.is_some(),
        "the train halts and the whistling ends"
    );
}

// --- Track blocking in the path grid (re/train-multipart 4.1, 9.2, 13.1, K-207) ---------------

use crate::path::BLOCKED;
use crate::path::grid::DirtyRect;

/// Blocked tiles of row `z` in a layer (only inside the map).
fn blocked_row(state: &SimState, layer: usize, z: i32) -> Vec<i32> {
    (0..64)
        .filter(|&x| state.path.region_id(layer, x, z) == BLOCKED)
        .collect()
}

fn is_blocked(state: &SimState, layer: usize, x: i32, z: i32) -> bool {
    state.path.region_id(layer, x, z) == BLOCKED
}

/// Layers in which the track blocking shows the row of the locomotive exactly: land (0),
/// land+water (1), land+forest (2), all (3). The water layer 4 and its derivatives are blocked in
/// the test land anyway.
const LAND_LAYERS: [usize; 4] = [0, 1, 2, 3];

fn tile_of(state: &SimState, id: ObjectId) -> (i32, i32) {
    state.object(id, false).expect("object present").tile
}

fn last_block_tile(state: &mut SimState, rules: &Rules, id: ObjectId) -> (i32, i32) {
    tr(state, rules, id).last_block_tile
}

/// Track (10, 20) ... (49, 20), `prepare` on the grid before the locomotive, then the locomotive at `tile`.
fn scene_with(tile: (i32, i32), prepare: impl FnOnce(&mut Sim)) -> (Rules, SimState, ObjectId) {
    let rules = rules();
    let mut state = state(&rules);
    let loco = {
        let mut sim = Sim::new(&mut state, &rules);
        lay_track(&mut sim, (10, 20), (49, 20));
        prepare(&mut sim);
        spawn_loco(&mut sim, 0, tile)
    };
    (rules, state, loco)
}

// QUIRK K-207: kept
#[test]
fn init_blocks_twelve_tiles_without_flush() {
    // re/train-multipart 9.2 with r = (1, 0) and locomotive at (40, 20): blocked x = 31 ... 42 (+2 ... -9).
    let (rules, mut state, loco) = scene((40, 20));
    assert_eq!(tr(&mut state, &rules, loco).raster_dir, (1, 0));
    assert_eq!(last_block_tile(&mut state, &rules, loco), (40, 20));
    let window: Vec<i32> = (31..=42).collect();
    for layer in LAND_LAYERS {
        assert_eq!(blocked_row(&state, layer, 20), window, "layer {layer}");
    }
    for layer in 0..9 {
        for x in &window {
            assert!(is_blocked(&state, layer, *x, 20), "layer {layer} x {x}");
        }
    }
    // Tender and wagon block nothing: only the row of the locomotive.
    for layer in LAND_LAYERS {
        for z in (0..64).filter(|&z| z != 20) {
            assert!(blocked_row(&state, layer, z).is_empty(), "z {z}");
        }
    }

    // Without flushing: from a clean state the call takes effect only in the dirty rectangle; the
    // derived layers do not follow until the next flush by another writer.
    let mut sim = Sim::new(&mut state, &rules);
    for x in 31..=42 {
        sim.state.path.mark_cell(u32::MAX, x, 20, false);
    }
    sim.state.path.flush_dirty_rect();
    assert_eq!(sim.state.path.dirty, DirtyRect::default());
    assert!(!is_blocked(sim.state, 5, 43, 20));
    super::blocking::block_track_tiles(&mut sim, loco);
    let d = sim.state.path.dirty;
    assert_eq!(
        (d.mask, d.min_x, d.max_x, d.min_z, d.max_z),
        (u32::MAX, 31, 42, 20, 20),
        "dirty rectangle extended"
    );
    assert!(
        !is_blocked(sim.state, 5, 43, 20),
        "2x2 layer not yet derived"
    );
    sim.state.path.flush_dirty_rect();
    assert!(is_blocked(sim.state, 5, 43, 20), "derived after the flush");
}

/// Ticks until the locomotive changes the tile; returns the new tile.
fn tick_to_next_tile(state: &mut SimState, rules: &Rules, loco: ObjectId) -> (i32, i32) {
    let from = tile_of(state, loco);
    for _ in 0..200 {
        object_tick(state, rules);
        let now = tile_of(state, loco);
        if now != from {
            return now;
        }
    }
    panic!("the locomotive does not change the tile (stays on {from:?})");
}

// QUIRK K-207: kept
#[test]
fn moving_forward_keeps_a_window_of_thirteen_and_frees_a_never_blocked_tile() {
    // K-207: obstacle (e.g. a tree) on (20, 20) = T - 10, never blocked by the train. Init blocks
    // 21 ... 32; the first forward step into tile 31 blocks 33 and frees 20 - in all layers,
    // without terrain classification (the water layer 4 is otherwise blocked everywhere in the test land).
    let (rules, mut state, loco) = scene_with((30, 20), |sim| {
        sim.state.path.mark_cell(u32::MAX, 20, 20, true);
        sim.state.path.flush_dirty_rect();
    });
    for layer in LAND_LAYERS {
        let want: Vec<i32> = (20..=32).collect();
        assert_eq!(blocked_row(&state, layer, 20), want, "init, layer {layer}");
    }
    msg(&mut state, &rules, loco, 0x121, &[]);
    let mut tile = tick_to_next_tile(&mut state, &rules, loco);
    assert_eq!(tile, (31, 20));
    assert_eq!(last_block_tile(&mut state, &rules, loco), (31, 20));
    for layer in LAND_LAYERS {
        let want: Vec<i32> = (21..=33).collect();
        assert_eq!(
            blocked_row(&state, layer, 20),
            want,
            "step 1, layer {layer}"
        );
    }
    for layer in [0, 1, 2, 3, 4, 5, 7] {
        assert!(!is_blocked(&state, layer, 20, 20), "layer {layer} free");
    }
    // Further: window T - 10 ... T + 2 (13 tiles), every tile behind the window free.
    while tile.0 < 36 {
        tile = tick_to_next_tile(&mut state, &rules, loco);
        assert_eq!(last_block_tile(&mut state, &rules, loco), tile);
        for layer in LAND_LAYERS {
            let want: Vec<i32> = (tile.0 - 10..=tile.0 + 2).collect();
            assert_eq!(
                blocked_row(&state, layer, 20),
                want,
                "tile {tile:?}, layer {layer}"
            );
        }
    }
    assert_eq!(
        state.path.dirty,
        DirtyRect::default(),
        "flushed after every step"
    );
}

// QUIRK K-207: kept
#[test]
fn moving_backward_keeps_a_window_of_twelve() {
    // Backward: blocks T' - 9r, frees T' + 3r. Init at 35: 26 ... 37; tile 34: 25 ... 36.
    let (rules, mut state, loco) = scene((35, 20));
    let window: Vec<i32> = (26..=37).collect();
    assert_eq!(blocked_row(&state, 0, 20), window);
    msg(&mut state, &rules, loco, 0x221, &[]);
    let mut tile = tick_to_next_tile(&mut state, &rules, loco);
    assert_eq!(tile, (34, 20));
    while tile.0 > 31 {
        assert_eq!(last_block_tile(&mut state, &rules, loco), tile);
        for layer in LAND_LAYERS {
            let want: Vec<i32> = (tile.0 - 9..=tile.0 + 2).collect();
            assert_eq!(
                blocked_row(&state, layer, 20),
                want,
                "tile {tile:?}, layer {layer}"
            );
        }
        tile = tick_to_next_tile(&mut state, &rules, loco);
    }
    assert_eq!(state.path.dirty, DirtyRect::default());
}

/// Blocked tiles of column `x` (z values) in a layer.
fn blocked_col(state: &SimState, layer: usize, x: i32) -> Vec<i32> {
    (0..64)
        .filter(|&z| state.path.region_id(layer, x, z) == BLOCKED)
        .collect()
}

/// The locomotive near the map edge drives out; `west` backward over edge 0, otherwise forward over
/// edge 63; `along_x` on a track in x direction (z = 20), otherwise in z direction (x = 20).
fn block_near_the_edge(west: bool, along_x: bool) {
    let rules = rules();
    let mut state = state(&rules);
    let pos = |a: i32| if along_x { (a, 20) } else { (20, a) };
    let line = |state: &SimState, layer: usize| {
        if along_x {
            blocked_row(state, layer, 20)
        } else {
            blocked_col(state, layer, 20)
        }
    };
    let loco = {
        let mut sim = Sim::new(&mut state, &rules);
        if west {
            lay_track(&mut sim, pos(0), pos(20));
            spawn_loco(&mut sim, 0, pos(1))
        } else {
            lay_track(&mut sim, pos(45), pos(63));
            spawn_loco(&mut sim, 0, pos(62))
        }
    };
    // The window (+2 ... -9) sticks out: only the tiles inside the map are blocked.
    let init: Vec<i32> = if west {
        (0..=3).collect()
    } else {
        (53..=63).collect()
    };
    for layer in LAND_LAYERS {
        assert_eq!(line(&state, layer), init, "init, layer {layer}");
    }
    let code = if west { 0x221 } else { 0x121 };
    msg(&mut state, &rules, loco, code, &[]);
    for k in 0..300 {
        object_tick(&mut state, &rules);
        if tr(&mut state, &rules, loco).status == 0 {
            break;
        }
        assert!(k < 299, "the locomotive does not leave the map");
    }
    // The clamped tile stays at the edge: the blocking changes only through the first step.
    let want: Vec<i32> = if west {
        (0..=2).collect()
    } else {
        (53..=63).collect()
    };
    for layer in LAND_LAYERS {
        assert_eq!(line(&state, layer), want, "end, layer {layer}");
    }
    assert_eq!(state.path.dirty, DirtyRect::default());
}

// QUIRK K-207: kept
#[test]
fn blocking_outside_the_map_is_ignored() {
    // re/train-multipart 13.1: cells outside write nothing (no panic, the rest as above) - at
    // all four edges (track in x and in z direction).
    for along_x in [true, false] {
        block_near_the_edge(true, along_x);
        block_near_the_edge(false, along_x);
    }
}

// QUIRK K-207: kept
#[test]
fn loading_does_not_block_twice() {
    // Deviation from the original: the path grid is in the savegame; loading does not block again and the last
    // blocked tile stays, otherwise the next step would free the wrong tile.
    let (rules, mut state, loco) = scene((30, 20));
    msg(&mut state, &rules, loco, 0x121, &[]);
    for _ in 0..12 {
        object_tick(&mut state, &rules);
    }
    let tile = tile_of(&state, loco);
    assert!(tile.0 >= 32, "the locomotive is under way: {tile:?}");
    let want: Vec<i32> = (tile.0 - 10..=tile.0 + 2).collect();
    assert_eq!(blocked_row(&state, 0, 20), want);
    let mut loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(last_block_tile(&mut loaded, &rules, loco), tile);
    assert_eq!(blocked_row(&loaded, 0, 20), want);
    assert_eq!(
        crate::save::checksum(&loaded),
        crate::save::checksum(&state),
        "same checksum after loading"
    );
    for k in 0..30 {
        object_tick(&mut state, &rules);
        object_tick(&mut loaded, &rules);
        assert_eq!(
            crate::save::checksum(&loaded),
            crate::save::checksum(&state),
            "object tick {k}"
        );
    }
}

/// Save immediately after init (the dirty rectangle of the blocking is still unflushed and for
/// locomotives at the edge extends beyond the map), load, depart: the checksum sequence is
/// bit-identical to the one without loading. `track` is the track, `start` the locomotive tile.
fn save_right_after_init(track: ((i32, i32), (i32, i32)), start: (i32, i32), code: u32) {
    let rules = rules();
    let mut state = state(&rules);
    let loco = {
        let mut sim = Sim::new(&mut state, &rules);
        lay_track(&mut sim, track.0, track.1);
        spawn_loco(&mut sim, 0, start)
    };
    assert_ne!(
        state.path.dirty,
        DirtyRect::default(),
        "{start:?}: init rectangle unflushed"
    );
    let mut loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.path.dirty, state.path.dirty, "{start:?}");
    assert_eq!(
        crate::save::checksum(&loaded),
        crate::save::checksum(&state),
        "{start:?}: checksum after loading"
    );
    msg(&mut state, &rules, loco, code, &[]);
    msg(&mut loaded, &rules, loco, code, &[]);
    for k in 0..40 {
        object_tick(&mut state, &rules);
        object_tick(&mut loaded, &rules);
        assert_eq!(
            crate::save::checksum(&loaded),
            crate::save::checksum(&state),
            "{start:?}: object tick {k}"
        );
    }
    assert_eq!(
        tr(&mut loaded, &rules, loco).last_block_tile,
        tr(&mut state, &rules, loco).last_block_tile
    );
}

#[test]
fn a_save_right_after_the_init_loads_and_continues_identically() {
    // Map edge left (window -6 ... 5), middle (10, 20) and right (window 54 ... 65); plus the
    // z edges. Up to 11 tiles beyond the edge are permitted in the dirty rectangle.
    save_right_after_init(((0, 20), (20, 20)), (3, 20), 0x121);
    save_right_after_init(((0, 20), (20, 20)), (1, 20), 0x221);
    save_right_after_init(((0, 20), (30, 20)), (10, 20), 0x121);
    save_right_after_init(((45, 20), (63, 20)), (63, 20), 0x121);
    save_right_after_init(((20, 0), (20, 20)), (20, 3), 0x121);
    save_right_after_init(((20, 45), (20, 63)), (20, 63), 0x121);
}

// --- Supply and game types 4/5 (re/train-multipart 3.5) ---

use super::STATION;
use super::drive::loco_outside_map;
use crate::module::building::building;
use crate::rules::ModelBox;
use crate::setup::{find_object, place_supply_objects};
use opennml_formats::gifcon::PlaceObject;

/// Rules with the supply list 0 of nation 0 (slot 0 has nation 0): a station (building, hull box
/// with a broken corner, so that snapping would give the position away) two tiles ahead and a
/// colonist one tile back and three tiles to the side.
fn supply_rules() -> Rules {
    let mut r = rules();
    r.default_objects.supply.insert(
        (0, 0),
        vec![
            PlaceObject {
                guid: STATION,
                dx: 2,
                dz: 0,
            },
            PlaceObject {
                guid: COLONIST,
                dx: -1,
                dz: 3,
            },
        ],
    );
    r.model_boxes.insert(
        STATION,
        ModelBox {
            center: [10.0, 0.0, -20.0],
            half: [200.0, 50.0, 150.0],
        },
    );
    r
}

/// Track and locomotive for the drive over the left (backward) or right edge of the map (map 64
/// tiles); the locomotive receives the command to drive.
fn edge_scene(rules: &Rules, mode: i32, backward: bool) -> (SimState, ObjectId) {
    let mut state = state_in_mode(rules, mode);
    let loco = {
        let mut sim = Sim::new(&mut state, rules);
        if backward {
            lay_track(&mut sim, (0, 20), (20, 20));
            spawn_loco(&mut sim, 0, (10, 20))
        } else {
            lay_track(&mut sim, (45, 20), (63, 20));
            spawn_loco(&mut sim, 0, (55, 20))
        }
    };
    let code = if backward { 0x221 } else { 0x121 };
    msg(&mut state, rules, loco, code, &[]);
    (state, loco)
}

/// The locomotive drives out and halts outside the map (`LeaveMap`).
fn leave_in_mode(rules: &Rules, mode: i32, backward: bool) -> (SimState, ObjectId) {
    let (mut state, loco) = edge_scene(rules, mode, backward);
    for _ in 0..200 {
        object_tick(&mut state, rules);
        let t = tr(&mut state, rules, loco);
        if t.outside != 0 && t.status == 0 {
            return (state, loco);
        }
    }
    panic!("the locomotive does not leave the map");
}

/// Number of objects of type `guid` on slot 0.
fn count_of(state: &SimState, guid: u32) -> usize {
    state.tables[0]
        .objects
        .iter()
        .flatten()
        .filter(|o| o.guid == guid)
        .count()
}

// QUIRK K-35: kept
#[test]
fn leaving_backwards_in_mode_4_loads_supply_and_returns_after_180_s() {
    let rules = supply_rules();
    let (mut state, loco) = edge_scene(&rules, 4, true);
    // No supply inside the map; in the first tick outside, `0x821 {180 000}` loads (outside status 1).
    let mut k = 0;
    while tr(&mut state, &rules, loco).outside == 0 {
        assert!(!tr(&mut state, &rules, loco).has_supply, "tick {k}");
        object_tick(&mut state, &rules);
        k += 1;
        assert!(k < 100, "the locomotive does not leave the map");
    }
    let t = tr(&mut state, &rules, loco);
    // The counter already counts down by dt = 100 ms in the same object tick (re/train-multipart 3.2).
    assert_eq!(
        (t.has_supply, t.outside, t.time_counter),
        (true, 1, 179_900)
    );
    // It runs down by 100 ms per object tick, also while the locomotive still coasts; at <= 0 the
    // train departs forward (counter then 1000): 180 s = 1800 object ticks after loading.
    let (mut waited, mut previous) = (1, t.time_counter);
    loop {
        object_tick(&mut state, &rules);
        waited += 1;
        let t = tr(&mut state, &rules, loco);
        if t.status & status::FORWARD != 0 {
            assert_eq!(t.status, status::FORWARD | status::MOVING | status::ACCEL);
            assert_eq!((t.has_supply, t.outside, t.time_counter), (true, 1, 1000));
            break;
        }
        assert_eq!(t.time_counter, previous - 100, "tick {waited}");
        previous = t.time_counter;
        assert!(waited < 2000, "no self-departure");
    }
    assert_eq!(waited, 1800, "object ticks until departure");
    // The locomotive drives in (the self-departure fails as long as the train drives) and brakes
    // at the end of the track; until the halt inside the map there is no supply.
    let mut ticks = 0;
    while tr(&mut state, &rules, loco).status != 0 {
        assert_eq!(count_of(&state, STATION), 0, "tick {ticks}");
        assert!(tr(&mut state, &rules, loco).has_supply);
        object_tick(&mut state, &rules);
        ticks += 1;
        assert!(ticks < 200, "train does not halt");
    }
    let t = tr(&mut state, &rules, loco);
    assert_eq!(
        (t.has_supply, t.outside, t.time_counter, t.status),
        (false, 0, 0, 0)
    );
    let o = state.object(loco, false).unwrap();
    assert!(
        (0..=64 << 12).contains(&o.grid.x.0) && (0..=64 << 12).contains(&o.grid.z.0),
        "halt in the map"
    );
    let pos = o.world_pos;
    // The supply stands at the locomotive: station two tiles ahead, colonist (-1, +3) tiles.
    let station = find_object(&state, 0, STATION).expect("supply station");
    let colonist = find_object(&state, 0, COLONIST).expect("supply colonist");
    assert_eq!(
        state.object(station, false).unwrap().world_pos,
        [pos[0] + 256.0, pos[1], pos[2]]
    );
    assert_eq!(
        state.object(colonist, false).unwrap().world_pos,
        [pos[0] - 128.0, pos[1], pos[2] + 384.0]
    );
    // After that nothing happens anymore: no second supply, no departure.
    for _ in 0..30 {
        object_tick(&mut state, &rules);
    }
    let t = tr(&mut state, &rules, loco);
    assert_eq!((t.status, t.has_supply, t.time_counter), (0, false, 0));
    assert_eq!(
        (count_of(&state, STATION), count_of(&state, COLONIST)),
        (1, 1)
    );
}

// QUIRK K-35: kept
#[test]
fn supply_objects_are_placed_without_snapping_or_0x3303() {
    let rules = supply_rules();
    let mut state = state_in_mode(&rules, 4);
    let mut sim = Sim::new(&mut state, &rules);
    let loco = spawn_loco(&mut sim, 0, (30, 20));
    // Position with fractions so that the f32 calculation `(SX*dx) as f32 + x` becomes visible.
    let origin = {
        let p = sim.object(loco).unwrap().world_pos;
        [p[0] + 0.25, p[1], p[2] - 0.375]
    };
    assert!(place_supply_objects(&mut sim, 0, origin));
    let station = find_object(sim.state, 0, STATION).expect("station");
    let colonist = find_object(sim.state, 0, COLONIST).expect("colonist");
    // Position = origin + (SX*dx, 0, SZ*dz), without snapping (otherwise the hull box corner would
    // lie on a tile boundary), on the slot of the locomotive.
    let s = sim.object(station).unwrap();
    assert_eq!(s.world_pos, [origin[0] + 256.0, origin[1], origin[2]]);
    assert_eq!((s.id.slot(), s.guid), (0, STATION));
    let c = sim.object(colonist).unwrap();
    assert_eq!(
        c.world_pos,
        [origin[0] - 128.0, origin[1], origin[2] + 384.0]
    );
    // No `0x3303 {1}`: the occupancy rectangle is the footprint. As a counter-check it grows with
    // the message by one tile per side.
    let b = building(&sim, station).expect("building state");
    assert_eq!(b.occupancy_rect, b.rect);
    let rect = b.rect;
    send(&mut sim, station, &Message::new(0x3303, &[1]));
    let grown = building(&sim, station).unwrap().occupancy_rect;
    assert_eq!(grown, [rect[0] - 1, rect[1] - 1, rect[2] + 1, rect[3] + 1]);
    // Only nations 0 ... 5 have supply; a nation without a list places nothing.
    let count = sim.state.tables[0].count;
    sim.state.players[0].nation = 6;
    assert!(!place_supply_objects(&mut sim, 0, origin));
    sim.state.players[0].nation = 3;
    assert!(place_supply_objects(&mut sim, 0, origin));
    assert_eq!(sim.state.tables[0].count, count);
}

// QUIRK K-35: kept
#[test]
fn mode_5_and_forward_exits_load_no_supply() {
    let rules = supply_rules();
    for (mode, backward, outside) in [(5, true, 1), (4, false, 2), (0, true, 1)] {
        let (mut state, loco) = leave_in_mode(&rules, mode, backward);
        let t = tr(&mut state, &rules, loco);
        assert_eq!(
            (t.has_supply, t.time_counter, t.outside),
            (false, 0, outside),
            "game type {mode}, backward {backward}"
        );
        // No counter, no self-departure, no supply - not even after a long wait.
        for _ in 0..2000 {
            object_tick(&mut state, &rules);
        }
        let t = tr(&mut state, &rules, loco);
        assert_eq!((t.status, t.outside), (0, outside), "game type {mode}");
        assert_eq!(count_of(&state, STATION), 0);
    }
}

// QUIRK K-35: kept
#[test]
fn braking_outside_the_map_keeps_the_supply() {
    // "Locomotive in the map" is the unclamped grid position; the clamped tile would always lie in
    // the map. A halt outside (command 0x321 after driving out) unloads nothing.
    let rules = supply_rules();
    let (mut state, loco) = edge_scene(&rules, 4, true);
    while tr(&mut state, &rules, loco).outside == 0 {
        object_tick(&mut state, &rules);
    }
    for _ in 0..3 {
        object_tick(&mut state, &rules);
    }
    msg(&mut state, &rules, loco, 0x321, &[]);
    while tr(&mut state, &rules, loco).status != 0 {
        object_tick(&mut state, &rules);
    }
    let o = state.object(loco, false).unwrap();
    assert!(o.grid.x.0 < 0, "halt outside the map");
    assert!(
        (0..64).contains(&o.tile.0),
        "the clamped tile would lie in the map"
    );
    let t = tr(&mut state, &rules, loco);
    assert_eq!((t.has_supply, t.outside), (true, 1));
    assert!(t.time_counter > 0);
    assert_eq!(count_of(&state, STATION), 0);
}

// QUIRK K-35: kept
#[test]
fn a_save_during_the_supply_wait_continues_identically() {
    // Supply loaded, counter running: saving and loading change neither the departure after 180 s
    // nor the unloading (counter, flag and outside status are in the savegame).
    let rules = supply_rules();
    let (mut state, loco) = edge_scene(&rules, 4, true);
    while tr(&mut state, &rules, loco).outside == 0 {
        object_tick(&mut state, &rules);
    }
    for _ in 0..50 {
        object_tick(&mut state, &rules);
    }
    let mut loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(
        tr(&mut loaded, &rules, loco).time_counter,
        tr(&mut state, &rules, loco).time_counter
    );
    for k in 0..2000 {
        object_tick(&mut state, &rules);
        object_tick(&mut loaded, &rules);
        if k % 25 != 0 && k < 1700 {
            continue;
        }
        assert_eq!(
            crate::save::checksum(&loaded),
            crate::save::checksum(&state),
            "object tick {k}"
        );
    }
    assert_eq!(
        count_of(&loaded, STATION),
        1,
        "supply unloaded after loading"
    );
    assert_eq!(count_of(&state, STATION), 1);
}

// QUIRK K-35: kept
#[test]
fn unloading_uses_the_same_outside_check_as_the_tick_at_the_map_edge() {
    // The original checks `0 <= grid >> 12 < size` (`0x004F69C0`), not `IsGridPosInMap`
    // (`<= size << 12`): at exactly `DIM << 12` the locomotive is outside and unloads nothing.
    let rules = supply_rules();
    let mut state = state_in_mode(&rules, 4);
    let mut sim = Sim::new(&mut state, &rules);
    let loco = spawn_loco(&mut sim, 0, (30, 20));
    let edge = 64 << 12;
    let place = |sim: &mut Sim, x: i32, z: i32| {
        let mut g = sim.object(loco).unwrap().grid;
        (g.x.0, g.z.0) = (x, z);
        crate::world::set_grid_position(sim, loco, g, true);
    };
    let load = |sim: &mut Sim| {
        let t = train_mut(sim, loco).unwrap();
        (t.has_supply, t.outside, t.time_counter) = (true, 1, 5000);
    };
    for (x, z) in [
        (edge, 20 << 12),
        (20 << 12, edge),
        (edge, edge),
        (-1, 20 << 12),
    ] {
        place(&mut sim, x, z);
        assert!(loco_outside_map(&sim, loco), "({x}, {z}) outside");
        load(&mut sim);
        supply::unload_if_due(&mut sim, loco);
        let t = train(&sim, loco).unwrap();
        assert_eq!(
            (t.has_supply, t.outside, t.time_counter),
            (true, 1, 5000),
            "({x}, {z})"
        );
        assert_eq!(count_of(sim.state, STATION), 0, "({x}, {z})");
    }
    // A grid position just before it is in the map: it unloads.
    place(&mut sim, 20 << 12, edge - 1);
    assert!(!loco_outside_map(&sim, loco));
    supply::unload_if_due(&mut sim, loco);
    let t = train(&sim, loco).unwrap();
    assert_eq!((t.has_supply, t.outside, t.time_counter), (false, 0, 0));
    assert_eq!(count_of(sim.state, STATION), 1);
}

// --- TRAIN_GOLDEN: acceleration, halt at the end of the track, backward drive, halt on command and
// a stagecoach with rear part (re/train-multipart 3, 9, 13) ---

use crate::command::{Command, CommandKind, TrainOp};
use crate::motion::NO_DIRECTION;

/// IDs of the train scene.
struct TrainScene {
    loco: ObjectId,
    cart: ObjectId,
}

/// Track (10, 20) ... (49, 20), locomotive (40, 20) and stagecoach (20, 40), both slot 0.
fn train_scene() -> (SimState, Rules, TrainScene) {
    let rules = rules();
    let mut state = state(&rules);
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        lay_track(&mut sim, (10, 20), (49, 20));
        TrainScene {
            loco: spawn_loco(&mut sim, 0, (40, 20)),
            cart: spawn(&mut sim, CART, 0, (20, 40)),
        }
    };
    state.events.clear();
    (state, rules, scene)
}

/// Step of the departure forward including the walk of the stagecoach.
const TRAIN_FORWARD_AT: usize = 0;
/// Step of the backward drive.
const TRAIN_BACKWARD_AT: usize = 200;
/// Step of the halt on command at full speed backward (between start-up and braking look-ahead).
const TRAIN_STOP_AT: usize = 280;
/// Step of a second halt command to the standing locomotive (without effect, still part of the sequence).
const TRAIN_STOP_IDLE_AT: usize = 500;
/// Length of the scene in `step` calls (two per object tick).
const TRAIN_STEPS: usize = 800;

/// Walk of the stagecoach to the center of tile `tile`.
fn cart_walk(cart: ObjectId, tile: (i32, i32)) -> CommandKind {
    CommandKind::Move {
        units: vec![cart],
        x: (tile.0 << 12) + 0x800,
        z: (tile.1 << 12) + 0x800,
        target: -1,
        formation: 0,
        direction: NO_DIRECTION,
        spread: crate::command::NO_SPREAD,
        flags: 0,
    }
}

/// Commands in the `step` call `i`.
fn train_orders(i: usize, s: &TrainScene) -> Vec<Command> {
    let cmd = |kind| Command { player: 0, kind };
    let train = |op| CommandKind::Train {
        train: s.loco,
        op,
        target: None,
    };
    match i {
        TRAIN_FORWARD_AT => vec![
            cmd(train(TrainOp::Forward)),
            cmd(cart_walk(s.cart, (40, 40))),
        ],
        TRAIN_BACKWARD_AT => vec![
            cmd(train(TrainOp::Backward)),
            cmd(cart_walk(s.cart, (20, 52))),
        ],
        TRAIN_STOP_AT | TRAIN_STOP_IDLE_AT => vec![cmd(train(TrainOp::Stop))],
        _ => vec![],
    }
}

/// State assertion at the time of saving.
type TrainProbe = fn(&mut SimState, &Rules, &TrainScene);

/// Checksum per `step` call of the scene; with `save_at = Some(i)` it saves and reloads after the
/// `step` call `i` (`at_save` sees the state immediately before).
fn train_run_probed(save_at: Option<usize>, at_save: TrainProbe) -> Vec<u64> {
    let (mut state, rules, scene) = train_scene();
    let mut out = Vec::with_capacity(TRAIN_STEPS);
    for i in 0..TRAIN_STEPS {
        step(&mut state, &rules, &train_orders(i, &scene));
        if save_at == Some(i) {
            at_save(&mut state, &rules, &scene);
            state = crate::save::testkit::save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

fn train_run(save_at: Option<usize>) -> Vec<u64> {
    train_run_probed(save_at, |_, _, _| {})
}

/// Is a message `code` pending at the module of the object (event or timer)?
fn pending(sim: &Sim, id: ObjectId, index: usize, code: u32) -> bool {
    sim.module(id, index).is_some_and(|m| {
        m.base
            .events
            .iter()
            .chain(&m.base.timers_ws)
            .any(|e| e.msg.code == code)
    })
}

/// (Status, speed, time index) of the locomotive.
fn loco_status(state: &mut SimState, rules: &Rules, s: &TrainScene) -> (u32, i32, i32) {
    let t = train(&Sim::new(state, rules), s.loco).unwrap().clone();
    (t.status, t.speed, t.acc_time)
}

/// Offset of tender and wagon to the locomotive in grid position (x, z).
fn consist_offsets(state: &mut SimState, rules: &Rules, loco: ObjectId) -> Vec<(i32, i32)> {
    let ids = consist(state, rules, loco);
    let at = grid(state, loco);
    ids[1..]
        .iter()
        .map(|&id| {
            let g = grid(state, id);
            (g.x.0 - at.x.0, g.z.0 - at.z.0)
        })
        .collect()
}

#[test]
fn the_train_scene_shows_every_state() {
    // Counter-check before entering the golden: accelerating, halt at the end of the track over the
    // braking curve with event bit 0, backward drive (accelerating, full speed), halt on command at
    // full speed; tender and wagon keep their offset; the rear part of the stagecoach does not appear until
    // after the first step and follows it. Times are `step` calls (two per object tick).
    let (mut state, rules, s) = train_scene();
    let offsets = consist_offsets(&mut state, &rules, s.loco);
    assert_eq!(offsets.len(), 2, "tender and wagon coupled");
    let mut states: Vec<u32> = Vec::new();
    let (mut first_bit, mut first_halt, mut tail_at) = (None, None, None);
    for i in 0..TRAIN_STEPS {
        step(&mut state, &rules, &train_orders(i, &s));
        assert_eq!(
            consist_offsets(&mut state, &rules, s.loco),
            offsets,
            "offset of tender and wagon in step {i}"
        );
        let sim = Sim::new(&mut state, &rules);
        let t = train(&sim, s.loco).unwrap();
        if states.last() != Some(&t.status) {
            states.push(t.status);
            if t.status == 0 && first_halt.is_none() {
                first_halt = Some(i);
            }
        }
        if object_event_bits(&sim, s.loco) & 1 != 0 && first_bit.is_none() {
            first_bit = Some(i);
        }
        let linked = multipart::head(&sim, s.cart).unwrap().linked;
        if tail_at.is_none() && sim.object(linked).is_some() {
            tail_at = Some(i);
        }
    }
    let (acc, moving, brake, fwd, back) = (
        status::ACCEL,
        status::MOVING,
        status::BRAKING,
        status::FORWARD,
        status::BACKWARD,
    );
    assert_eq!(
        states,
        [
            acc | moving | fwd,    // start-up forward
            moving | brake | fwd,  // braking look-ahead at the end of the track
            0,                     // halt (event bit 0)
            acc | moving | back,   // start-up backward
            moving | back,         // full speed backward
            moving | brake | back, // halt on command: braking curve
            0,
        ]
    );
    assert_eq!(first_bit, first_halt, "bit 0 appears with the first halt");
    assert!(first_halt.unwrap() < TRAIN_BACKWARD_AT);
    // Final position: the locomotive stands after the command (not at track start 10), the stagecoach at the target.
    let sim = Sim::new(&mut state, &rules);
    let (t, loco) = (train(&sim, s.loco).unwrap(), sim.object(s.loco).unwrap());
    assert_eq!((t.speed, t.acc_time), (0, -100));
    assert_eq!(
        loco.tile,
        (28, 20),
        "halt on command, not at the end of the track"
    );
    // Rear part: appears after the first step and follows the stagecoach at drawbar distance.
    assert!(tail_at.unwrap() > 0, "rear part not until after init");
    let cart = sim.object(s.cart).unwrap();
    assert_eq!(cart.tile, (20, 52), "stagecoach at the target");
    let tail = multipart::head(&sim, s.cart).unwrap().linked;
    let tail = sim.object(tail).expect("rear part exists");
    let near = crate::collision::fixed_pos_distance(
        (cart.grid.x.0, cart.grid.z.0),
        (tail.grid.x.0, tail.grid.z.0),
    );
    assert!(near < 3 << 12, "rear part follows: distance {near}");
}

/// Measured (acceleration, halt at the end of the track, backward drive, halt on command, stagecoach with rear
/// part).
/// Changed: new state fields (upgrades, counter-upgrades, temporary upgrades).
/// Changed: new state fields (gate guard at the building, area queries), behavior unchanged
/// (counter-check with `serde(skip)`).
/// Changed: world field `headhunters` (serialization, behavior the same).
/// Changed: state field `ki` (AI state, serialization, behavior the same; counter-check with
/// `serde(skip)`).
/// Changed: state fields `ai_marks`, `attack_reports` and `campaign_difficulty` (serialization,
/// behavior the same; counter-check with `serde(skip)`).
/// Changed: state fields `script`, `names` and evocation counter (serialization, behavior the
/// same; counter-check with `serde(skip)`).
/// Changed: event bits 1, 2, 3, 5, 7, kill counter, message sequence `0x2100`/`0x611` and new
/// world fields (counter-check: without setter and with `serde(skip)` the values of the preceding step).
/// Changed: statistics log and timer list (serialization, behavior the same; counter-check with
/// `serde(skip)`).
/// Changed: serialization of `accounting` and `countdowns` added to the value of the preceding
/// step (counter-check: with `serde(skip)` on both, the values of the preceding step).
/// Changed: serialization of new fields (presentation state, mirror); behavior the same
/// (counter-check with `serde(skip)`).
/// Changed: list of idle workers (serialization of `SimState::idle_workers`, FORMAT_VERSION 26);
/// behavior the same (counter-check with `serde(skip)` in debug and release).
/// Changed: flag leader and time limit in the state (serialization of `SimState::flag_leader` and
/// `time_limit`, FORMAT_VERSION 27); behavior the same (counter-check with `serde(skip)`: all values
/// unchanged).
/// Changed: new field `SimState::network` (serialization, FORMAT_VERSION 29; behavior the same;
/// counter-check with `serde(skip)`: all values unchanged).
const TRAIN_GOLDEN: u64 = 0x32f9_e5de_e86e_6f2f;

#[test]
fn train_checksums_match_the_golden_sequence() {
    let seq = train_run(None);
    let mut h = crate::rules::Fnv64::default();
    for c in &seq {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, TRAIN_GOLDEN, "{:#018x}", h.0);
}

/// Savegames of the scene: before the `0x40B` of the stagecoach (after the first step), in the
/// middle of accelerating, in the middle of braking at the end of the track, after the halt
/// (`acc_time = -100`, bit 0), backward at full speed, in the middle of braking on command and
/// after the halt on command.
const TRAIN_SAVE_BEFORE_TAIL: usize = 0;
const TRAIN_SAVE_ACCEL: usize = 20;
const TRAIN_SAVE_BRAKING: usize = 50;
const TRAIN_SAVE_HALTED: usize = 100;
const TRAIN_SAVE_BACKWARD: usize = 260;
const TRAIN_SAVE_STOP_BRAKING: usize = 290;
const TRAIN_SAVE_STOPPED: usize = 400;

#[test]
fn train_checksums_survive_saving_and_loading() {
    let golden = train_run(None);
    // Before the delayed 0x40B: no rear part yet, the message is pending.
    let before_tail = train_run_probed(Some(TRAIN_SAVE_BEFORE_TAIL), |state, rules, s| {
        let sim = Sim::new(state, rules);
        assert_eq!(
            multipart::head(&sim, s.cart).unwrap().linked,
            ObjectId::NONE
        );
        assert!(pending(&sim, s.cart, multipart::HEAD, 0x40B));
    });
    assert_eq!(before_tail, golden, "saved before the 0x40B");
    // In the middle of accelerating (drive curve not at the end).
    let accel = train_run_probed(Some(TRAIN_SAVE_ACCEL), |state, rules, s| {
        let (st, speed, acc) = loco_status(state, rules, s);
        assert_eq!(st, status::ACCEL | status::MOVING | status::FORWARD);
        assert!(speed > 0 && acc > 0, "{speed} {acc}");
    });
    assert_eq!(accel, golden, "saved while accelerating");
    // In the middle of braking before the end of the track.
    let braking = train_run_probed(Some(TRAIN_SAVE_BRAKING), |state, rules, s| {
        let (st, speed, _) = loco_status(state, rules, s);
        assert_eq!(st, status::MOVING | status::BRAKING | status::FORWARD);
        assert!(speed > 0);
    });
    assert_eq!(braking, golden, "saved while braking");
    // After the halt at the end of the track: acc_time = -100, bit 0 set.
    let halted = train_run_probed(Some(TRAIN_SAVE_HALTED), |state, rules, s| {
        assert_eq!(loco_status(state, rules, s), (0, 0, -100));
        assert_eq!(object_event_bits(&Sim::new(state, rules), s.loco) & 1, 1);
    });
    assert_eq!(halted, golden, "saved after the halt");
    // Backward at full speed.
    let backward = train_run_probed(Some(TRAIN_SAVE_BACKWARD), |state, rules, s| {
        let (st, speed, _) = loco_status(state, rules, s);
        assert_eq!(st, status::MOVING | status::BACKWARD);
        assert_eq!(speed, 2304);
    });
    assert_eq!(backward, golden, "saved backward while driving");
    // In the middle of braking on command.
    let stopping = train_run_probed(Some(TRAIN_SAVE_STOP_BRAKING), |state, rules, s| {
        let (st, speed, _) = loco_status(state, rules, s);
        assert_eq!(st, status::MOVING | status::BRAKING | status::BACKWARD);
        assert!(speed > 0);
    });
    assert_eq!(stopping, golden, "saved while braking on command");
    // After the halt on command.
    let stopped = train_run_probed(Some(TRAIN_SAVE_STOPPED), |state, rules, s| {
        assert_eq!(loco_status(state, rules, s), (0, 0, -100));
    });
    assert_eq!(stopped, golden, "saved after the halt on command");
}
