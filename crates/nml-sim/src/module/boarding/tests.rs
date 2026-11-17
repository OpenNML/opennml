use super::compute_boarding_points;
use super::testkit::{
    COLONIST, MAYFLOWER, PROP, TRANSPORT, island_state, rules, shore_state, spawn,
};
use crate::sim::Sim;

#[test]
fn boarding_points_are_cell_centres() {
    // re/ships 9.2: colonist (5, 10), transport (14, 10) -> a = tile (9, 14), b = tile (11, 14),
    // as cell centers (k << 12) + 2048. Radius (2100 + 512) >> 12 = 0, inner radius 2.
    let rules = rules();
    let mut state = shore_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = spawn(&mut sim, COLONIST, 1, (5, 10));
    let ship = spawn(&mut sim, TRANSPORT, 1, (14, 10));
    assert_eq!(
        compute_boarding_points(&mut sim, unit, ship),
        Some(((38912, 59392), (47104, 59392)))
    );
}

#[test]
fn a_container_in_the_units_land_or_a_unit_in_the_ships_water_meets_on_the_spot() {
    // Branch 1 (re/ships 3.2): the container stands in the land region of the unit -> a = b = container tile.
    // `create_object` places walk units on a free cell of their layer; a ship on land exists
    // therefore only via a spit of land: the cells (14, 10...12) are free in the land layer, the unit
    // stands on (14, 12), the ship in the water at (14, 10) of the same land region.
    let rules = rules();
    let mut state = shore_state(&rules);
    for z in 10..=12 {
        state.path.mark_cell(1 << 0, 14, z, false);
    }
    state.path.flush_dirty_rect();
    state.path.build_regions(u32::MAX);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = spawn(&mut sim, COLONIST, 1, (14, 12));
    let ship = spawn(&mut sim, TRANSPORT, 1, (14, 10));
    let p = ((14 << 12) + 2048, (10 << 12) + 2048);
    assert_eq!(compute_boarding_points(&mut sim, unit, ship), Some((p, p)));

    // Branch 2: the unit stands in the water region of the ship -> a = b = unit tile. So that
    // branch 1 fails beforehand, it stands on a sandbank: a single land cell (14, 12) with
    // its own land region that also lies in the ship water (land layer 0 and layer 6 free); the
    // ship cell (14, 10) stays locked in the land layer.
    let mut state = shore_state(&rules);
    state.path.mark_cell(1 << 0, 14, 12, false);
    state.path.flush_dirty_rect();
    state.path.build_regions(u32::MAX);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = spawn(&mut sim, COLONIST, 1, (14, 12));
    let ship = spawn(&mut sim, TRANSPORT, 1, (14, 10));
    let p = ((14 << 12) + 2048, (12 << 12) + 2048);
    assert_eq!(compute_boarding_points(&mut sim, unit, ship), Some((p, p)));
}

#[test]
fn no_reachable_shore_gives_no_boarding_points() {
    // Island without ship water in the radius (`island_state`): no candidate of the land region has a cell of
    // the ship region within a radius of two cells -> None.
    let rules = rules();
    let mut state = island_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let unit = spawn(&mut sim, COLONIST, 1, (3, 3));
    let ship = spawn(&mut sim, TRANSPORT, 1, (14, 10));
    assert_eq!(compute_boarding_points(&mut sim, unit, ship), None);
}

// ---------------------------------------------------------------------------------------------
// Loading, meeting point, parking, pickup and unloading (re/ships 3.1, 3.3-3.5, 9.3)
// ---------------------------------------------------------------------------------------------

use super::super::quarter::{self, quarter};
use crate::command::{Command, CommandKind, NO_SPREAD, execute};
use crate::id::ObjectId;
use crate::message::{Message, Posted};
use crate::messaging::{send, send_mut};
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::rules::Rules;
use crate::state::SimState;
use crate::tick::step;

/// Coast world with revealed map (the fog is not part of these tests).
fn scene() -> (Rules, SimState) {
    let rules = rules();
    let mut state = shore_state(&rules);
    crate::fog::reveal_map(&mut state, -1);
    (rules, state)
}

/// Own objects (slot 1) on the tiles.
fn place(state: &mut SimState, rules: &Rules, guid: u32, tile: (i32, i32)) -> ObjectId {
    spawn(&mut Sim::new(state, rules), guid, 1, tile)
}

/// Command `0x13` of slot 1, executed for one engine tick.
fn quarter_cmd(state: &mut SimState, rules: &Rules, ship: ObjectId, units: &[ObjectId]) {
    let kind = CommandKind::Quarter {
        container: ship,
        units: units.to_vec(),
    };
    step(state, rules, &[Command { player: 1, kind }]);
}

/// Engine ticks until `done` holds (at most `max`); return value the count.
fn until(
    state: &mut SimState,
    rules: &Rules,
    max: usize,
    mut done: impl FnMut(&mut Sim) -> bool,
) -> usize {
    for n in 1..=max {
        step(state, rules, &[]);
        if done(&mut Sim::new(state, rules)) {
            return n;
        }
    }
    panic!("not reached after {max} ticks");
}

fn status_of(state: &mut SimState, rules: &Rules, ship: ObjectId) -> i32 {
    quarter(&Sim::new(state, rules), ship).map_or(-9, |q| q.status)
}

fn waiting_of(state: &mut SimState, rules: &Rules, ship: ObjectId) -> Vec<ObjectId> {
    quarter(&Sim::new(state, rules), ship)
        .map(|q| q.waiting.clone())
        .unwrap_or_default()
}

fn parked(state: &mut SimState, rules: &Rules, u: ObjectId) -> bool {
    send(&mut Sim::new(state, rules), u, &Message::new(0x2C00, &[])) == 1
}

fn is_quartered(state: &mut SimState, rules: &Rules, u: ObjectId) -> bool {
    Sim::new(state, rules)
        .object(u)
        .is_some_and(|o| o.status & status::QUARTERED != 0)
}

/// Inbox (FIFO) of the object.
fn fifo_of(state: &mut SimState, rules: &Rules, id: ObjectId) -> Vec<Posted> {
    Sim::new(state, rules)
        .object(id)
        .map(|o| o.fifo.iter().cloned().collect())
        .unwrap_or_default()
}

/// Execute a command of slot 1 without a tick (the FIFO stays for reading).
fn execute_now(state: &mut SimState, rules: &Rules, kind: CommandKind) {
    execute(
        &mut Sim::new(state, rules),
        &Command { player: 1, kind },
        true,
    );
}

fn quarter_kind(ship: ObjectId, units: &[ObjectId]) -> CommandKind {
    CommandKind::Quarter {
        container: ship,
        units: units.to_vec(),
    }
}

/// Move command to the center of the tile `tile`.
fn move_kind(units: &[ObjectId], tile: (i32, i32)) -> CommandKind {
    CommandKind::Move {
        units: units.to_vec(),
        x: (tile.0 << 12) + 0x800,
        z: (tile.1 << 12) + 0x800,
        target: -1,
        formation: 0,
        direction: NO_DIRECTION,
        spread: NO_SPREAD,
        flags: 0,
    }
}

/// Meeting point `a` of the coast world for the colonist at (5, 10) and the ship at (14, 10)
/// (re/ships 9.2) and the goal radius `2100 + 512 + 1024`.
const POINT_A: (i32, i32) = (38912, 59392);
const POINT_B: (i32, i32) = (47104, 59392);
const GOAL_RADIUS: i32 = 3636;

/// Query `0x2017` of the container: (return value, x, z).
fn meeting_point(state: &mut SimState, rules: &Rules, ship: ObjectId) -> (i32, i32, i32) {
    let mut m = Message::new(0x2017, &[-1, -1]);
    let r = send_mut(&mut Sim::new(state, rules), ship, &mut m);
    (r, m.arg(0), m.arg(1))
}

#[test]
fn boarding_runs_meeting_point_parking_and_arrival() {
    // re/ships 3.1/3.3, 9.2: the command posts 0x1E17 {a, ids} (with abort) and FINDTO(b) with
    // success 0x1F17 to the ship, FINDTO(a) with success 0x1617 to the colonist. Afterwards the
    // ship has status 1, meeting point a and waiting list [k1]; k1 walks to a and parks (state 6);
    // the ship sails to b (goal radius 3636); on arrival (0x1F17) k1 is picked up
    // (W-200).
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    execute_now(&mut state, &rules, quarter_kind(ship, &[k1]));
    let goal = |p: (i32, i32)| Message::new(0x200, &[p.0, p.1, GOAL_RADIUS, NO_DIRECTION, -1, 1]);
    let sent = fifo_of(&mut state, &rules, ship);
    let [
        Posted::Message { msg, abort: true },
        Posted::Command { env, abort: false },
    ] = &sent[..]
    else {
        panic!("{sent:?}");
    };
    assert_eq!(
        *msg,
        Message::new(0x1E17, &[POINT_A.0, POINT_A.1, k1.as_i32()])
    );
    assert_eq!(env.msg, goal(POINT_B));
    assert_eq!(env.success, Some(Message::new(0x1F17, &[])));
    assert_eq!((env.success_target, &env.fault), (ObjectId::NONE, &None));
    let walk = fifo_of(&mut state, &rules, k1);
    let [Posted::Command { env, abort: true }] = &walk[..] else {
        panic!("{walk:?}");
    };
    assert_eq!(env.msg, goal(POINT_A));
    assert_eq!(env.success, Some(Message::new(0x1617, &[k1.as_i32()])));
    assert_eq!((env.success_target, &env.fault), (ship, &None));

    step(&mut state, &rules, &[]);
    step(&mut state, &rules, &[]);
    {
        let sim = Sim::new(&mut state, &rules);
        let q = quarter(&sim, ship).unwrap();
        assert_eq!(
            (q.status, q.meeting_point, q.waiting.clone()),
            (1, Some(POINT_A), vec![k1])
        );
    }
    until(&mut state, &rules, 600, |sim| {
        send(sim, k1, &Message::new(0x2C00, &[])) == 1
    });
    assert_eq!(
        status_of(&mut state, &rules, ship),
        1,
        "ship still under way"
    );
    assert!(!is_quartered(&mut state, &rules, k1));
    until(&mut state, &rules, 3000, |sim| {
        quarter(sim, ship).is_some_and(|q| q.status == 2)
    });
    let sim = Sim::new(&mut state, &rules);
    let (s, u) = (sim.object(ship).unwrap(), sim.object(k1).unwrap());
    let q = quarter(&sim, ship).unwrap();
    assert_eq!((q.status, q.cur), (2, 1));
    assert_eq!(q.occupants, [k1]);
    assert!(q.waiting.is_empty());
    assert_ne!(u.status & status::QUARTERED, 0);
    assert_eq!(u.carrier, ship);
    assert_eq!(
        (u.grid.x, u.grid.z),
        (s.grid.x, s.grid.z),
        "jump to the ship position"
    );
    let to_b = crate::collision::fixed_pos_distance((s.grid.x.0, s.grid.z.0), POINT_B);
    assert!(
        to_b <= GOAL_RADIUS,
        "ship within the goal radius around b: {to_b}"
    );
}

#[test]
fn units_arriving_after_the_ship_board_at_once() {
    // re/ships 3.3 (status 2): if the unit is still on its way when the ship arrives, it is
    // skipped and the list cleared; if it reaches a later, it boards at once.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (0, 31));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    quarter_cmd(&mut state, &rules, ship, &[k1]);
    until(&mut state, &rules, 3000, |sim| {
        quarter(sim, ship).is_some_and(|q| q.status == 2)
    });
    assert!(!parked(&mut state, &rules, k1) && !is_quartered(&mut state, &rules, k1));
    assert!(waiting_of(&mut state, &rules, ship).is_empty());
    until(&mut state, &rules, 3000, |sim| {
        sim.object(k1)
            .is_some_and(|o| o.status & status::QUARTERED != 0)
    });
    let sim = Sim::new(&mut state, &rules);
    let q = quarter(&sim, ship).unwrap();
    assert_eq!((q.status, q.occupants.clone()), (2, vec![k1]));
    assert_eq!(sim.object(k1).unwrap().carrier, ship);
}

#[test]
fn a_second_order_during_the_trip_leaves_units_parked() {
    // W-199: at status 1, 0x2017 delivers the meeting point; the second command sends k2 there, but
    // posts no 0x1E17 - k2 is not in the waiting list. If k2 arrives before the ship, it parks and
    // stays parked after 0x1F17; 0x3800 returns 0 because the success has been sent and cleared.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let k2 = place(&mut state, &rules, COLONIST, (8, 14));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    quarter_cmd(&mut state, &rules, ship, &[k1]);
    step(&mut state, &rules, &[]);
    assert_eq!(
        meeting_point(&mut state, &rules, ship),
        (1, POINT_A.0, POINT_A.1)
    );
    execute_now(&mut state, &rules, quarter_kind(ship, &[k2]));
    assert!(
        fifo_of(&mut state, &rules, ship).is_empty(),
        "no second 0x1E17"
    );
    let walk = fifo_of(&mut state, &rules, k2);
    let [Posted::Command { env, abort: true }] = &walk[..] else {
        panic!("{walk:?}");
    };
    assert_eq!(
        env.msg,
        Message::new(
            0x200,
            &[POINT_A.0, POINT_A.1, GOAL_RADIUS, NO_DIRECTION, -1, 1]
        )
    );
    assert_eq!(waiting_of(&mut state, &rules, ship), [k1]);
    until(&mut state, &rules, 600, |sim| {
        send(sim, k2, &Message::new(0x2C00, &[])) == 1
    });
    assert_eq!(
        status_of(&mut state, &rules, ship),
        1,
        "k2 was there before the ship"
    );
    until(&mut state, &rules, 3000, |sim| {
        quarter(sim, ship).is_some_and(|q| q.status == 2)
    });
    step(&mut state, &rules, &[]);
    step(&mut state, &rules, &[]);
    assert!(is_quartered(&mut state, &rules, k1));
    assert!(parked(&mut state, &rules, k2) && !is_quartered(&mut state, &rules, k2));
    let mut q = Message::new(0x3800, &[ship.as_i32()]);
    assert_eq!(send_mut(&mut Sim::new(&mut state, &rules), k2, &mut q), 0);
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(quarter(&sim, ship).unwrap().occupants, [k1]);
}

#[test]
fn a_full_ship_leaves_units_parked() {
    // W-198: the Mayflower (capacity 0) accepts the command; the unit parks at the shore, the ship
    // arrives, `AcceptUnit` refuses (full) and the unit stays there.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let ship = place(&mut state, &rules, MAYFLOWER, (14, 10));
    execute_now(&mut state, &rules, quarter_kind(ship, &[k1]));
    assert_eq!(fifo_of(&mut state, &rules, k1).len(), 1, "command accepted");
    until(&mut state, &rules, 3000, |sim| {
        quarter(sim, ship).is_some_and(|q| q.status == 2)
    });
    for _ in 0..20 {
        step(&mut state, &rules, &[]);
    }
    {
        let sim = Sim::new(&mut state, &rules);
        let q = quarter(&sim, ship).unwrap();
        assert_eq!((q.max, q.cur), (0, 0));
        assert!(q.occupants.is_empty() && q.waiting.is_empty());
    }
    assert!(parked(&mut state, &rules, k1) && !is_quartered(&mut state, &rules, k1));
    for _ in 0..100 {
        step(&mut state, &rules, &[]);
    }
    assert!(parked(&mut state, &rules, k1) && !is_quartered(&mut state, &rules, k1));
}

#[test]
fn touching_the_ship_boards_and_an_empty_list_aborts() {
    // W-201: 0x2317 {u} with the only waiting unit: pickup, removed from the list, the list is empty
    // -> abort, status 0 in the middle of the trip. The meeting point stays in the container but is
    // only delivered at status 1.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    quarter_cmd(&mut state, &rules, ship, &[k1]);
    step(&mut state, &rules, &[]);
    assert_eq!(waiting_of(&mut state, &rules, ship), [k1]);
    let touch = Message::new(0x2317, &[k1.as_i32()]);
    assert_eq!(send(&mut Sim::new(&mut state, &rules), ship, &touch), 1);
    assert!(is_quartered(&mut state, &rules, k1));
    let sim = Sim::new(&mut state, &rules);
    let q = quarter(&sim, ship).unwrap();
    assert_eq!((q.status, q.meeting_point), (0, Some(POINT_A)));
    assert!(q.waiting.is_empty());
    assert_eq!(q.occupants, [k1]);
    assert_eq!(meeting_point(&mut state, &rules, ship).0, 0);
}

#[test]
fn a_failed_trip_keeps_status_and_meeting_point() {
    // W-202: if the path of the ship is blocked after the command (column x = 12 in the water layer),
    // planning fails without a message (no fault envelope): status 1 and meeting point stay, the
    // unit waits parked at the shore. A new command uses the meeting point (0x2017 -> 1, no 0x1E17).
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let k2 = place(&mut state, &rules, COLONIST, (5, 12));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    execute_now(&mut state, &rules, quarter_kind(ship, &[k1]));
    for z in 0..32 {
        state.path.mark_cell(1 << 4, 12, z, true);
    }
    state.path.flush_dirty_rect();
    state.path.build_regions(u32::MAX);
    until(&mut state, &rules, 600, |sim| {
        send(sim, k1, &Message::new(0x2C00, &[])) == 1
    });
    for _ in 0..200 {
        step(&mut state, &rules, &[]);
    }
    {
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(
            sim.object(ship).unwrap().tile,
            (14, 10),
            "the ship did not get away"
        );
        let q = quarter(&sim, ship).unwrap();
        assert_eq!(
            (q.status, q.meeting_point, q.waiting.clone()),
            (1, Some(POINT_A), vec![k1])
        );
    }
    assert!(!is_quartered(&mut state, &rules, k1));
    assert_eq!(
        meeting_point(&mut state, &rules, ship),
        (1, POINT_A.0, POINT_A.1)
    );
    execute_now(&mut state, &rules, quarter_kind(ship, &[k2]));
    assert!(fifo_of(&mut state, &rules, ship).is_empty());
    // The goal of the new command is the meeting point from 0x2017.
    let walk = fifo_of(&mut state, &rules, k2);
    let [Posted::Command { env, abort: true }] = &walk[..] else {
        panic!("{walk:?}");
    };
    assert_eq!((env.msg.arg(0), env.msg.arg(1)), POINT_A);
}

#[test]
fn a_new_order_to_the_ship_clears_its_waiting_list() {
    // A move command to the ship at status 1 aborts (`Quarter::abort`): status 0, waiting list
    // empty. The unit reaches a and parks anyway; without 0x1F17 nobody picks it up.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    quarter_cmd(&mut state, &rules, ship, &[k1]);
    step(&mut state, &rules, &[]);
    assert_eq!(status_of(&mut state, &rules, ship), 1);
    let away = move_kind(&[ship], (14, 20));
    step(
        &mut state,
        &rules,
        &[Command {
            player: 1,
            kind: away,
        }],
    );
    step(&mut state, &rules, &[]);
    assert_eq!(status_of(&mut state, &rules, ship), 0);
    assert!(waiting_of(&mut state, &rules, ship).is_empty());
    until(&mut state, &rules, 600, |sim| {
        send(sim, k1, &Message::new(0x2C00, &[])) == 1
    });
    for _ in 0..200 {
        step(&mut state, &rules, &[]);
    }
    assert!(parked(&mut state, &rules, k1) && !is_quartered(&mut state, &rules, k1));
}

#[test]
fn parked_units_accept_a_new_order() {
    // A parked colonist (walk state 6) gets a walk command: walk leaves the state and
    // walks to the goal.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    quarter_cmd(&mut state, &rules, ship, &[k1]);
    until(&mut state, &rules, 600, |sim| {
        send(sim, k1, &Message::new(0x2C00, &[])) == 1
    });
    let go = move_kind(&[k1], (3, 14));
    step(
        &mut state,
        &rules,
        &[Command {
            player: 1,
            kind: go,
        }],
    );
    step(&mut state, &rules, &[]);
    assert!(!parked(&mut state, &rules, k1));
    until(&mut state, &rules, 600, |sim| {
        sim.object(k1).is_some_and(|o| o.tile == (3, 14))
    });
}

/// Two colonists put into the ship (`AcceptUnit` without distance check, as on arrival).
fn load(state: &mut SimState, rules: &Rules, ship: ObjectId, units: &[ObjectId]) {
    let mut sim = Sim::new(state, rules);
    for &u in units {
        assert_eq!(quarter::accept_unit(&mut sim, ship, u), 0);
    }
}

#[test]
fn unloading_puts_units_on_the_shore() {
    // re/ships 9.3: ship at b with two occupants, `UnquarterAll` -> (39936, 53248) and (39936, 55296);
    // the same points via command `0x15`. Without a landing spot in the radius (ship far out) the
    // unit silently stays inside (W-203).
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let k2 = place(&mut state, &rules, COLONIST, (5, 12));
    let ship = place(&mut state, &rules, TRANSPORT, (11, 14));
    load(&mut state, &rules, ship, &[k1, k2]);
    execute_now(
        &mut state,
        &rules,
        CommandKind::UnquarterAll { container: ship },
    );
    let sim = Sim::new(&mut state, &rules);
    let pos = |u| {
        let o = sim.object(u).unwrap();
        (o.status & status::QUARTERED, (o.grid.x.0, o.grid.z.0))
    };
    assert_eq!(pos(k1), (0, (39936, 53248)));
    assert_eq!(pos(k2), (0, (39936, 55296)));
    let q = quarter(&sim, ship).unwrap();
    assert_eq!((q.cur, q.occupants.len()), (0, 0));

    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let ship = place(&mut state, &rules, TRANSPORT, (22, 14));
    load(&mut state, &rules, ship, &[k1]);
    let mut sim = Sim::new(&mut state, &rules);
    send(&mut sim, ship, &Message::new(0x1917, &[]));
    let q = quarter(&sim, ship).unwrap();
    assert_ne!(sim.object(k1).unwrap().status & status::QUARTERED, 0);
    assert_eq!((q.cur, q.occupants.clone()), (1, vec![k1]));
}

#[test]
fn a_sinking_transport_kills_its_passengers_and_frees_the_waiting() {
    // re/ships 3.7: the sinking ship (0x110) aborts its quarters (status 0, waiting list empty) and
    // sends the occupants 0x110 (they die in their own tick); the waiting unit at the shore stays
    // parked and lives.
    let (rules, mut state) = scene();
    let (p1, p2, w) = (
        place(&mut state, &rules, COLONIST, (5, 10)),
        place(&mut state, &rules, COLONIST, (5, 12)),
        place(&mut state, &rules, COLONIST, (5, 14)),
    );
    let ship = place(&mut state, &rules, TRANSPORT, (11, 14));
    load(&mut state, &rules, ship, &[p1, p2]);
    {
        let mut sim = Sim::new(&mut state, &rules);
        let set = Message::new(0x1E17, &[POINT_A.0, POINT_A.1, w.as_i32()]);
        assert_eq!(send(&mut sim, ship, &set), 0);
        assert_eq!(send(&mut sim, w, &Message::new(0x2D00, &[])), 1);
    }
    assert_eq!(status_of(&mut state, &rules, ship), 1);
    send(
        &mut Sim::new(&mut state, &rules),
        ship,
        &Message::new(0x110, &[]),
    );
    assert_eq!(status_of(&mut state, &rules, ship), 0);
    assert!(waiting_of(&mut state, &rules, ship).is_empty());
    until(&mut state, &rules, 40, |sim| {
        [p1, p2]
            .iter()
            .all(|&u| sim.object(u).is_none_or(|o| o.status & status::DIED != 0))
    });
    assert!(Sim::new(&mut state, &rules).state.object(w, true).is_some());
    assert!(parked(&mut state, &rules, w) && !is_quartered(&mut state, &rules, w));
}

#[test]
fn the_waiting_list_holds_only_own_quartering_units_once() {
    // Deviation W-207: `0x1E17` gets the filtered list (own, `CANQUARTER`, without duplicates). An
    // own object without walk among the IDs of the command stays out (`0x2C00` would be -1 != 0),
    // and the savegame loads.
    use crate::save::{SaveGame, testkit::save_bytes};
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let prop = place(&mut state, &rules, PROP, (6, 12));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    execute_now(&mut state, &rules, quarter_kind(ship, &[k1, prop, k1]));
    let sent = fifo_of(&mut state, &rules, ship);
    let [Posted::Message { msg, abort: true }, Posted::Command { .. }] = &sent[..] else {
        panic!("{sent:?}");
    };
    assert_eq!(
        *msg,
        Message::new(0x1E17, &[POINT_A.0, POINT_A.1, k1.as_i32()])
    );
    assert!(fifo_of(&mut state, &rules, prop).is_empty());
    step(&mut state, &rules, &[]);
    step(&mut state, &rules, &[]);
    assert_eq!(waiting_of(&mut state, &rules, ship), [k1]);
    let load = |state: &SimState| {
        let save = SaveGame::from_bytes(&save_bytes(state, &rules)).expect("loads");
        save.check(&rules).expect("matches the rules");
    };
    load(&state);
    until(&mut state, &rules, 3000, |sim| {
        quarter(sim, ship).is_some_and(|q| q.status == 2)
    });
    assert!(is_quartered(&mut state, &rules, k1));
    assert!(!is_quartered(&mut state, &rules, prop));
    load(&state);
}

#[test]
fn a_running_unit_of_a_second_order_boards_when_it_touches_the_ship() {
    // W-199 and `0x3800`: k2 (second command at status 1) is not in the waiting list but carries the
    // success `0x1617` to the ship. Walk `0x3800 {ship}` returns 1, `{other}` 0; a touch
    // (`0x2317`) picks it up, the waiting list stays untouched.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let k2 = place(&mut state, &rules, COLONIST, (0, 31));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    quarter_cmd(&mut state, &rules, ship, &[k1]);
    step(&mut state, &rules, &[]);
    quarter_cmd(&mut state, &rules, ship, &[k2]);
    step(&mut state, &rules, &[]);
    assert_eq!(waiting_of(&mut state, &rules, ship), [k1]);
    let partner = |state: &mut SimState, who: ObjectId| {
        let mut m = Message::new(0x3800, &[who.as_i32()]);
        send_mut(&mut Sim::new(state, &rules), k2, &mut m)
    };
    assert_eq!(partner(&mut state, ship), 1);
    assert_eq!(partner(&mut state, k1), 0);
    let touch = Message::new(0x2317, &[k2.as_i32()]);
    assert_eq!(send(&mut Sim::new(&mut state, &rules), ship, &touch), 1);
    assert!(is_quartered(&mut state, &rules, k2));
    assert_eq!(waiting_of(&mut state, &rules, ship), [k1]);
    assert_eq!(status_of(&mut state, &rules, ship), 1);
    // Someone without an order to the ship does not board.
    let k3 = place(&mut state, &rules, COLONIST, (3, 3));
    let touch = Message::new(0x2317, &[k3.as_i32()]);
    assert_eq!(send(&mut Sim::new(&mut state, &rules), ship, &touch), 0);
    assert!(!is_quartered(&mut state, &rules, k3));
}

#[test]
fn a_unit_waiting_for_one_ship_is_collected_when_that_ship_arrives_elsewhere() {
    // W-200 with two ships (K-274): k1 is in the waiting list of A but is commanded to B and
    // parks at B's meeting point. When A arrives (0x1F17), it picks up the parked unit over any
    // distance.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 12));
    let a = place(&mut state, &rules, TRANSPORT, (14, 10));
    let b = place(&mut state, &rules, TRANSPORT, (14, 20));
    let point_b = ((9 << 12) + 2048, (20 << 12) + 2048);
    {
        let mut sim = Sim::new(&mut state, &rules);
        let set_a = Message::new(0x1E17, &[POINT_A.0, POINT_A.1, k1.as_i32()]);
        assert_eq!(send(&mut sim, a, &set_a), 0);
        // B already has a meeting point (status 1, without a trip): the command uses it (W-199).
        let set_b = Message::new(0x1E17, &[point_b.0, point_b.1]);
        assert_eq!(send(&mut sim, b, &set_b), 0);
    }
    quarter_cmd(&mut state, &rules, b, &[k1]);
    until(&mut state, &rules, 600, |sim| {
        send(sim, k1, &Message::new(0x2C00, &[])) == 1
    });
    assert!(!is_quartered(&mut state, &rules, k1));
    assert_eq!(waiting_of(&mut state, &rules, a), [k1]);
    let far = {
        let sim = Sim::new(&mut state, &rules);
        let (u, s) = (sim.object(k1).unwrap(), sim.object(a).unwrap());
        crate::collision::fixed_pos_distance((u.grid.x.0, u.grid.z.0), (s.grid.x.0, s.grid.z.0))
    };
    assert!(far > 5 << 12, "k1 stands far from A: {far}");
    send(
        &mut Sim::new(&mut state, &rules),
        a,
        &Message::new(0x1F17, &[]),
    );
    let sim = Sim::new(&mut state, &rules);
    let (u, s) = (sim.object(k1).unwrap(), sim.object(a).unwrap());
    assert_ne!(u.status & status::QUARTERED, 0);
    assert_eq!((u.carrier, u.grid.x, u.grid.z), (a, s.grid.x, s.grid.z));
    assert_eq!(quarter(&sim, a).unwrap().occupants, [k1]);
    assert!(quarter(&sim, a).unwrap().waiting.is_empty());
    assert_eq!(quarter(&sim, b).unwrap().status, 1);
}

#[test]
fn a_waiting_unit_that_dies_before_the_ship_arrives_is_skipped() {
    // `0x1F17` skips IDs that no longer exist (alive); the list is empty afterwards.
    let (rules, mut state) = scene();
    let k1 = place(&mut state, &rules, COLONIST, (5, 10));
    let ship = place(&mut state, &rules, TRANSPORT, (14, 10));
    quarter_cmd(&mut state, &rules, ship, &[k1]);
    step(&mut state, &rules, &[]);
    assert_eq!(waiting_of(&mut state, &rules, ship), [k1]);
    send(
        &mut Sim::new(&mut state, &rules),
        k1,
        &Message::new(0x110, &[]),
    );
    until(&mut state, &rules, 3000, |sim| {
        quarter(sim, ship).is_some_and(|q| q.status == 2)
    });
    let sim = Sim::new(&mut state, &rules);
    let q = quarter(&sim, ship).unwrap();
    assert!(q.waiting.is_empty() && q.occupants.is_empty());
    assert!(sim.state.object(k1, true).is_none(), "k1 has died");
}

// ---------------------------------------------------------------------------------------------
// SHIP_GOLDEN: loading, trip, unloading and broadside (re/ships 3.1-3.6, 9.3)
// ---------------------------------------------------------------------------------------------

use super::super::shoot;
use super::testkit::WARSHIP;

/// IDs of the ship scene.
struct ShipScene {
    transport: ObjectId,
    k1: ObjectId,
    k2: ObjectId,
    warship: ObjectId,
    enemy: ObjectId,
}

/// Scene of the golden on the coast world (revealed), slot 0: transport (14, 10), colonists
/// (5, 10) and (5, 12), warship 1109 (20, 22); slot 1: colonist at the shore (7, 22) within range
/// of the warship. The warship stands in stance 0 and fires only on command.
fn ship_scene() -> (SimState, Rules, ShipScene) {
    let rules = rules();
    let mut state = shore_state(&rules);
    crate::fog::reveal_map(&mut state, -1);
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        let scene = ShipScene {
            transport: spawn(&mut sim, TRANSPORT, 0, (14, 10)),
            k1: spawn(&mut sim, COLONIST, 0, (5, 10)),
            k2: spawn(&mut sim, COLONIST, 0, (5, 12)),
            warship: spawn(&mut sim, WARSHIP, 0, (20, 22)),
            enemy: spawn(&mut sim, COLONIST, 1, (7, 22)),
        };
        send(
            &mut sim,
            scene.warship,
            &crate::module::quarter::attack_mode_msg(0, false),
        );
        scene
    };
    state.events.clear();
    (state, rules, scene)
}

/// Step in which loading and attacking happen.
const SHIP_QUARTER_AT: usize = 0;
/// Sailing of the transport.
const SHIP_SAIL_AT: usize = 400;
/// Unloading.
const SHIP_UNLOAD_AT: usize = 700;
/// Length of the scene in `step` calls (two make one object tick).
const SHIP_STEPS: usize = 1200;

/// Commands in the `step` call `i`.
fn ship_orders(i: usize, s: &ShipScene) -> Vec<Command> {
    let cmd = |kind| Command { player: 0, kind };
    match i {
        SHIP_QUARTER_AT => vec![
            cmd(quarter_kind(s.transport, &[s.k1, s.k2])),
            cmd(CommandKind::Attack {
                units: vec![s.warship],
                target: s.enemy,
                pos: None,
                mods: 0,
            }),
        ],
        SHIP_SAIL_AT => vec![cmd(move_kind(&[s.transport], (11, 6)))],
        SHIP_UNLOAD_AT => vec![cmd(CommandKind::UnquarterAll {
            container: s.transport,
        })],
        _ => vec![],
    }
}

/// State assertion at the time of saving.
type ShipProbe = fn(&mut SimState, &Rules, &ShipScene);

/// Checksum per `step` call of the scene; with `save_at = Some(i)` it is saved after the `step` call `i`
/// and loaded again (`at_save` sees the state immediately before).
fn ship_run_probed(save_at: Option<usize>, at_save: ShipProbe) -> Vec<u64> {
    let (mut state, rules, scene) = ship_scene();
    let mut out = Vec::with_capacity(SHIP_STEPS);
    for i in 0..SHIP_STEPS {
        step(&mut state, &rules, &ship_orders(i, &scene));
        if save_at == Some(i) {
            at_save(&mut state, &rules, &scene);
            state = crate::save::testkit::save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

fn ship_run(save_at: Option<usize>) -> Vec<u64> {
    ship_run_probed(save_at, |_, _, _| {})
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

/// `m_IsBusy` of the shoot module.
fn ship_busy(sim: &Sim, id: ObjectId) -> bool {
    crate::module::is_busy(sim, id, shoot::INDEX)
}

#[test]
fn the_ship_scene_shows_every_state() {
    // Counter-check before entering the golden: transport status 1 -> 2 -> 0 (the move command aborts),
    // both colonists parked, then QUARTERED, after step 700 at the shore; the warship fires a
    // broadside (six balls `backcannonNN`), `is_busy` toggles, the enemy falls. Times are
    // `step` calls (two per object tick).
    let (mut state, rules, s) = ship_scene();
    let mut states: Vec<i32> = Vec::new();
    let (mut parked_while_sailing, mut both_pending, mut busy_edges) = (false, false, Vec::new());
    let mut nodes: Vec<String> = Vec::new();
    for i in 0..SHIP_STEPS {
        let events = step(&mut state, &rules, &ship_orders(i, &s));
        for e in events {
            if let crate::state::SimEvent::ProjectileLaunched { node, .. } = e {
                nodes.push(node);
            }
        }
        let mut sim = Sim::new(&mut state, &rules);
        let q = quarter(&sim, s.transport).unwrap().clone();
        if states.last() != Some(&q.status) {
            states.push(q.status);
        }
        let units_parked = [s.k1, s.k2]
            .iter()
            .all(|&u| send(&mut sim, u, &Message::new(0x2C00, &[])) == 1);
        parked_while_sailing |= q.status == 1 && units_parked;
        both_pending |= pending(&sim, s.warship, shoot::INDEX, 0xD0A)
            && pending(&sim, s.warship, shoot::INDEX, 0xE0A);
        let busy = ship_busy(&sim, s.warship);
        if busy_edges.last() != Some(&busy) {
            busy_edges.push(busy);
        }
        if i == SHIP_SAIL_AT - 1 {
            // Before sailing on: both colonists are in the ship.
            assert_eq!(
                (q.status, q.cur, q.occupants.clone()),
                (2, 2, vec![s.k1, s.k2])
            );
            for u in [s.k1, s.k2] {
                let o = sim.object(u).unwrap();
                assert_ne!(o.status & status::QUARTERED, 0, "{u:?}");
                assert_eq!(o.carrier, s.transport);
            }
        }
        if i == SHIP_UNLOAD_AT - 1 {
            assert_eq!(
                sim.object(s.transport).unwrap().tile,
                (11, 6),
                "at the destination"
            );
            assert_eq!(q.cur, 2, "still on board");
        }
    }
    assert_eq!(states, [1, 2, 0], "status of the transport");
    assert!(parked_while_sailing, "both parked at status 1");
    assert!(both_pending, "0xD0A and 0xE0A pending at the same time");
    assert_eq!(busy_edges, [false, true, false], "is_busy toggles");
    assert_eq!(nodes.len(), 6, "one broadside with six cannons: {nodes:?}");
    assert!(
        nodes.iter().all(|n| n.starts_with("backcannon")),
        "port: {nodes:?}"
    );
    let sim = Sim::new(&mut state, &rules);
    assert!(sim.object(s.enemy).is_none(), "the enemy has fallen");
    let q = quarter(&sim, s.transport).unwrap();
    assert_eq!((q.status, q.cur, q.occupants.len()), (0, 0, 0));
    for u in [s.k1, s.k2] {
        let o = sim.object(u).unwrap();
        assert_eq!(o.status & status::QUARTERED, 0, "{u:?} unloaded");
        assert!(o.tile.0 <= 9, "{u:?} at the shore: {:?}", o.tile);
    }
}

/// Measured (loading, trip, unloading, broadside).
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
const SHIP_GOLDEN: u64 = 0x3e51_34bb_728b_e924;

#[test]
fn ship_checksums_match_the_golden_sequence() {
    let seq = ship_run(None);
    let mut h = crate::rules::Fnv64::default();
    for c in &seq {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, SHIP_GOLDEN, "{:#018x}", h.0);
}

/// Savegames of the scene: status 1 with parked units, in the middle of the broadside (`0xD0A` and
/// `0xE0A` pending), shortly after arrival, on the way on, and after unloading.
const SHIP_SAVE_PARKED: usize = 100;
const SHIP_SAVE_BROADSIDE: usize = 45;
const SHIP_SAVE_BROADSIDE_END: usize = 79;
const SHIP_SAVE_SAILING: usize = 490;
const SHIP_SAVE_UNLOADED: usize = 710;

#[test]
fn ship_checksums_survive_saving_and_loading() {
    let golden = ship_run(None);
    // Status 1 with parked units at the shore: the ship has not arrived yet.
    let parked_run = ship_run_probed(Some(SHIP_SAVE_PARKED), |state, rules, s| {
        let mut sim = Sim::new(state, rules);
        let q = quarter(&sim, s.transport).unwrap();
        assert_eq!(
            (q.status, q.cur, q.waiting.clone()),
            (1, 0, vec![s.k1, s.k2])
        );
        for u in [s.k1, s.k2] {
            assert_eq!(send(&mut sim, u, &Message::new(0x2C00, &[])), 1, "{u:?}");
            assert_eq!(sim.object(u).unwrap().status & status::QUARTERED, 0);
        }
    });
    assert_eq!(parked_run, golden, "saved with parked units");
    // In the middle of the broadside: shot and end message are pending.
    for at in [SHIP_SAVE_BROADSIDE, SHIP_SAVE_BROADSIDE_END] {
        let run = ship_run_probed(Some(at), |state, rules, s| {
            let sim = Sim::new(state, rules);
            assert!(ship_busy(&sim, s.warship));
            assert!(pending(&sim, s.warship, shoot::INDEX, 0xD0A));
            assert!(pending(&sim, s.warship, shoot::INDEX, 0xE0A));
        });
        assert_eq!(run, golden, "saved in the broadside after step {at}");
    }
    // On the way on with occupants on board.
    let sailing = ship_run_probed(Some(SHIP_SAVE_SAILING), |state, rules, s| {
        let sim = Sim::new(state, rules);
        let q = quarter(&sim, s.transport).unwrap();
        assert_eq!((q.status, q.cur), (0, 2));
        assert_ne!(sim.object(s.transport).unwrap().tile, (11, 14), "under way");
    });
    assert_eq!(sailing, golden, "saved on the trip");
    // After unloading.
    let unloaded = ship_run_probed(Some(SHIP_SAVE_UNLOADED), |state, rules, s| {
        let sim = Sim::new(state, rules);
        assert_eq!(quarter(&sim, s.transport).unwrap().cur, 0);
        for u in [s.k1, s.k2] {
            assert_eq!(sim.object(u).unwrap().status & status::QUARTERED, 0);
        }
    });
    assert_eq!(unloaded, golden, "saved after unloading");
}
