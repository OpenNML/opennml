use super::testkit::*;
use super::*;
use crate::command::{Command, CommandKind, execute};
use crate::fx::{Fx, FxVec3};
use crate::module::walk::{walk, walk_mut};
use crate::motion::NO_DIRECTION;
use crate::object::status;
use crate::rules::Rules;
use crate::save::testkit::save_and_load;
use crate::state::SimState;
use crate::tick::step;

/// Creates a group by hand, without `create_group` (base cases without a command).
fn manual(sim: &mut Sim, kind: u8) -> i32 {
    let id = sim.state.groups.next_id;
    sim.state.groups.next_id += 1;
    sim.state
        .groups
        .map
        .set(key(id), Group::new(id, kind, (0, 0, NO_DIRECTION), false));
    id
}

#[test]
fn adding_members_sets_their_group_and_leaves_the_old_one() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (a, b) = (
        spawn(&mut sim, INFANTRY, (5, 5)),
        spawn(&mut sim, INFANTRY, (6, 5)),
    );
    let g1 = manual(&mut sim, 0);
    assert!(add_member(&mut sim, g1, a) && add_member(&mut sim, g1, b));
    assert!(
        !add_member(&mut sim, g1, a),
        "already a member -> 3, not accepted"
    );
    assert_eq!(walk(&sim, a).unwrap().group, g1);
    let g2 = manual(&mut sim, 0);
    assert!(add_member(&mut sim, g2, a));
    // a leaves g1 silently (no 0x1600 {-1}), g1 keeps b.
    assert_eq!(
        (
            walk(&sim, a).unwrap().group,
            find(&sim, g1).unwrap().members.clone()
        ),
        (g2, vec![b])
    );
}

#[test]
fn the_last_member_leaving_deletes_the_group() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, INFANTRY, (5, 5));
    let g = manual(&mut sim, 0);
    add_member(&mut sim, g, a);
    assert!(remove_unit(&mut sim, g, a, a, false));
    assert!(find(&sim, g).is_none());
    assert_eq!(walk(&sim, a).unwrap().group, NO_GROUP);
    assert!(!remove_unit(&mut sim, g, a, a, false), "group missing -> 0");
}

#[test]
fn type_5_matches_the_slowest_default_speed_and_restores_it_on_leaving() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (a, c) = (
        spawn(&mut sim, INFANTRY, (5, 5)),
        spawn(&mut sim, CANNON, (8, 5)),
    );
    let g = manual(&mut sim, 5);
    add_member(&mut sim, g, a);
    add_member(&mut sim, g, c);
    let slow = default_speed(&sim, c);
    assert!(slow < default_speed(&sim, a));
    assert_eq!(
        (
            find(&sim, g).unwrap().matched_speed,
            find(&sim, g).unwrap().matched_count
        ),
        (slow, 1)
    );
    broadcast_speed(&mut sim, g);
    assert_eq!(walk(&sim, a).unwrap().speed.cruising, slow);
    // The cannon leaves: counter 0 -> determine again (infantry) and send; the cannon gets
    // 0x1700 {-1, 0} (own speed).
    remove_unit(&mut sim, g, c, c, false);
    let fast = default_speed(&sim, a);
    assert_eq!(find(&sim, g).unwrap().matched_speed, fast);
    assert_eq!(walk(&sim, a).unwrap().speed.cruising, fast);
    assert_eq!(walk(&sim, c).unwrap().speed.cruising, slow);
    // Type 0: matching off.
    let g0 = manual(&mut sim, 0);
    add_member(&mut sim, g0, c);
    assert_eq!(find(&sim, g0).unwrap().matched_count, -1);
}

#[test]
fn a_faster_unit_leaving_a_type_5_group_gets_its_own_speed_back() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (a, c) = (
        spawn(&mut sim, INFANTRY, (5, 5)),
        spawn(&mut sim, CANNON, (8, 5)),
    );
    let g = manual(&mut sim, 5);
    add_member(&mut sim, g, a);
    add_member(&mut sim, g, c);
    broadcast_speed(&mut sim, g);
    let (slow, fast) = (default_speed(&sim, c), default_speed(&sim, a));
    assert_eq!(walk(&sim, a).unwrap().speed.cruising, slow);
    // a is faster than the matched speed: the counter stays, but a gets 0x1700 {-1, 0} and
    // continues at its own speed.
    assert!(remove_unit(&mut sim, g, a, a, false));
    assert_eq!(walk(&sim, a).unwrap().speed.cruising, fast);
    let g = find(&sim, g).unwrap();
    assert_eq!((g.matched_speed, g.matched_count), (slow, 1));
    assert_eq!(walk(&sim, c).unwrap().speed.cruising, slow);
}

#[test]
fn type_0_spreads_targets_around_the_centroid_and_compresses() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Six tiles apart: each unit is its own pack (Fixed(4)).
    let u: Vec<_> = [10, 16, 22]
        .iter()
        .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
        .collect();
    execute(&mut sim, &mv(&u, (30, 20), 0, NO_DIRECTION), true);
    let g = find(&sim, 0).expect("group 0");
    assert_eq!((g.kind, g.members.len(), g.max_dist), (0, 3, 24576));
    // Command order = selection reversed: the leader (head) is the unit selected last.
    assert_eq!(g.members, vec![u[2], u[1], u[0]]);
    // s = min(1, Div(2*3*1024, Mul(SqrtInt(3) = 7094, 24576) = 42564)) = 588; Mul(24576, 588) = 3528.
    let (ax, az) = (tile_center(30), tile_center(20));
    for (m, dx) in u.iter().zip([-3528, 0, 3528]) {
        assert_eq!(g.targets.get(m.0), Some(&(ax + dx, az, NO_DIRECTION)));
    }
    for &m in &u {
        let (x, z, flags) = posted_find_to(&sim, m).expect("FINDTO");
        assert_eq!(flags & 8, 8, "group mode");
        assert_eq!(
            Some(&(x, z, NO_DIRECTION)),
            find(&sim, 0).unwrap().targets.get(m.0)
        );
        assert_eq!(walk(&sim, m).unwrap().group, 0);
    }
}

#[test]
fn type_5_keeps_the_offsets_uncompressed() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = [10, 16, 22]
        .iter()
        .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
        .collect();
    execute(&mut sim, &mv(&u, (30, 20), 5, NO_DIRECTION), true);
    let g = find(&sim, 0).unwrap();
    assert_eq!((g.type_id(), g.matched_count), (0, 3));
    assert_eq!(
        g.targets.get(u[0].0),
        Some(&(tile_center(30) - 24576, tile_center(20), NO_DIRECTION))
    );
}

#[test]
fn type_0_with_all_units_on_one_spot_keeps_them_together() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, INFANTRY, (10, 10));
    let b = spawn(&mut sim, INFANTRY, (10, 10));
    execute(&mut sim, &mv(&[a, b], (20, 20), 0, NO_DIRECTION), true);
    let g = find(&sim, 0).unwrap();
    assert_eq!(g.max_dist, 0);
    assert_eq!(g.targets.get(a.0), g.targets.get(b.0));
}

#[test]
fn a_formation_of_7_or_more_drops_the_command() {
    // K-210: no group, no FINDTO; the ID is consumed anyway (`mgr.next++`).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = [
        spawn(&mut sim, INFANTRY, (10, 10)),
        spawn(&mut sim, INFANTRY, (16, 10)),
    ];
    execute(&mut sim, &mv(&u, (20, 20), 7, NO_DIRECTION), true);
    assert!(sim.state.groups.map.is_empty());
    assert_eq!(sim.state.groups.next_id, 1);
    assert!(u.iter().all(|&m| posted_find_to(&sim, m).is_none()));
}

#[test]
fn a_second_move_to_the_same_selection_realigns_with_the_old_offsets() {
    // K-215: same members -> realign, no new group, offsets of the formation setup.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = [10, 16, 22]
        .iter()
        .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
        .collect();
    execute(&mut sim, &mv(&u, (30, 20), 0, NO_DIRECTION), true);
    execute(&mut sim, &mv(&u, (40, 30), 0, NO_DIRECTION), true);
    assert_eq!(sim.state.groups.next_id, 1);
    let g = find(&sim, 0).unwrap();
    // Type 0: vf2C calls `Group_MoveInFormation`, which sets `marching` back to 0.
    assert!(!g.marching);
    // Members in command order (selection reversed): u[0] has the offset -24576.
    assert_eq!(
        g.targets.get(u[0].0),
        Some(&(tile_center(40) - 3528, tile_center(30), NO_DIRECTION))
    );
}

#[test]
fn a_new_formation_with_direction_keeps_the_old_one() {
    // K-213: other formation, but with a march direction -> old group (type 0) realigns.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = [10, 16]
        .iter()
        .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
        .collect();
    execute(&mut sim, &mv(&u, (30, 20), 0, NO_DIRECTION), true);
    execute(&mut sim, &mv(&u, (30, 30), 3, 90 << 12), true);
    assert_eq!(
        (sim.state.groups.next_id, find(&sim, 0).unwrap().kind),
        (1, 0)
    );
    assert_eq!(find(&sim, 0).unwrap().anchor.2, 90 << 12);
    // Without a direction a new group is formed (ID 1): the column (type 3) inherits the old
    // direction, the members switch over silently, the empty old group is deleted. (That the
    // command inherits the old direction is also shown by
    // `a_new_formation_without_direction_inherits_the_old_direction`.)
    execute(&mut sim, &mv(&u, (30, 30), 3, NO_DIRECTION), true);
    assert_eq!(sim.state.groups.next_id, 2);
    assert!(find(&sim, 0).is_none());
    let g = find(&sim, 1).expect("column as group 1");
    assert_eq!((g.kind, g.anchor.2), (3, 90 << 12));
    assert!(u.iter().all(|&m| walk(&sim, m).unwrap().group == 1));
}

#[test]
fn a_single_move_takes_the_unit_out_of_its_group() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = [10, 16]
        .iter()
        .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
        .collect();
    execute(&mut sim, &mv(&u, (30, 20), 0, NO_DIRECTION), true);
    execute(&mut sim, &mv(&u[..1], (5, 5), 0, NO_DIRECTION), true);
    assert_eq!(walk(&sim, u[0]).unwrap().group, NO_GROUP);
    assert_eq!(find(&sim, 0).unwrap().members, vec![u[1]]);
    // A FINDTO without flag 8/0x10 (attack, quarters ...) also takes the unit out.
    let env = crate::message::Envelope::bare(crate::module::walk::find_to_msg(
        tile_center(3),
        tile_center(3),
        -1,
        NO_DIRECTION,
        0,
        false,
        0x201,
    ));
    crate::messaging::dispatch_command(&mut sim, u[1], &env);
    assert!(find(&sim, 0).is_none(), "last member left -> deleted");
}

#[test]
fn a_new_formation_without_direction_inherits_the_old_direction() {
    // K-213 second half (`0x0056DE90`): other formation without march direction -> the command inherits
    // the direction of the old anchor and forms a new group (type 5: `type_id()` 0 != 5).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = [10, 16]
        .iter()
        .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
        .collect();
    execute(&mut sim, &mv(&u, (30, 20), 0, 90 << 12), true);
    assert_eq!(find(&sim, 0).unwrap().anchor.2, 90 << 12);
    execute(&mut sim, &mv(&u, (30, 30), 5, NO_DIRECTION), true);
    assert_eq!(sim.state.groups.next_id, 2);
    // The members switch silently into the new group, the old one is empty and deleted.
    assert!(find(&sim, 0).is_none());
    let g = find(&sim, 1).expect("new group 1");
    assert_eq!((g.kind, g.anchor.2), (5, 90 << 12));
    assert!(g.targets.get(u[0].0).is_some_and(|t| t.2 == 90 << 12));
    assert!(u.iter().all(|&m| walk(&sim, m).unwrap().group == 1));
}

#[test]
fn group_members_keep_their_group_and_keep_walking_after_some_ticks() {
    // The hooks in `walk/goto.rs`: FINDTO/PATH with flag 8/0x10 leave the members in the
    // group (`apply_move_flags`), the leader distributes the path (vf14/vf18).
    let rules = rules();
    let mut state = state(&rules);
    let u: Vec<_> = {
        let mut sim = Sim::new(&mut state, &rules);
        [10, 16, 22]
            .iter()
            .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
            .collect()
    };
    step(&mut state, &rules, &[mv(&u, (30, 20), 0, NO_DIRECTION)]);
    for _ in 0..20 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    let g = find(&sim, 0).expect("group still exists");
    assert_eq!(g.members, vec![u[2], u[1], u[0]]);
    for &m in &u {
        assert_eq!(walk(&sim, m).unwrap().group, 0);
        assert_ne!(sim.object(m).unwrap().status & status::MOVE, 0, "in motion");
    }
}

#[test]
fn a_leader_path_the_group_refuses_aborts_the_member() {
    // re/walk 2.6: PATH with flag 8 -> `on_leader_path`; without a pack for the leader (false) the
    // walk aborts, with a pack it starts (projection without waypoint shift).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (a, b) = (
        spawn(&mut sim, INFANTRY, (5, 5)),
        spawn(&mut sim, INFANTRY, (5, 8)),
    );
    let g = manual(&mut sim, 0);
    add_member(&mut sim, g, a);
    add_member(&mut sim, g, b);
    find_mut(&mut sim, g).unwrap().packs.set(b.0, Vec::new());
    let path = |tile: (i32, i32)| {
        let (x, z) = (tile_center(tile.0), tile_center(tile.1));
        // [end x, z, 0 (independent), flags, radius, direction, tolerance]
        crate::message::Envelope::bare(Message::new(0x500, &[x, z, 0, 8, -1, NO_DIRECTION, -1]))
    };
    assert_eq!(
        crate::messaging::dispatch_command(&mut sim, a, &path((20, 5))),
        0,
        "no pack -> vf18 refuses -> abort"
    );
    assert_eq!(
        sim.object(a).unwrap().status & status::MOVE,
        0,
        "not in motion"
    );
    assert_eq!(
        crate::messaging::dispatch_command(&mut sim, b, &path((20, 8))),
        1
    );
    assert_ne!(sim.object(b).unwrap().status & status::MOVE, 0);
    assert_eq!(walk(&sim, a).unwrap().group, g);
    assert_eq!(walk(&sim, b).unwrap().group, g);
}

#[test]
fn can_join_needs_four_tiles_and_free_cells_in_between() {
    // re/groups-formations 9: leader (0,0), member (8192,0): k = 4, checked (2048,0), (4096,0), (6144,0).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let f = spawn(&mut sim, INFANTRY, (10, 10));
    let near = spawn(&mut sim, INFANTRY, (12, 10));
    let far = spawn(&mut sim, INFANTRY, (14, 10));
    assert!(none::can_join(&mut sim, f, near, 0));
    // Exactly four tiles still work (d = 0x4000 is not > 0x4000) ...
    assert!(none::can_join(&mut sim, f, far, 0));
    // ... four tiles + 4 > Fixed(4) -> own leader.
    let mut p = sim.object(far).unwrap().grid;
    p.x.0 += 4;
    crate::world::set_grid_position(&mut sim, far, p, true);
    assert!(!none::can_join(&mut sim, f, far, 0));
    // A blocked cell in between separates the pack.
    sim.state.path.mark_cell(1, 11, 10, true);
    assert!(!none::can_join(&mut sim, f, near, 0));
    assert!(
        none::can_join(&mut sim, f, f, 0),
        "distance 0 -> k = 0 -> yes"
    );
}

#[test]
fn nearby_units_form_one_pack_and_only_the_leader_plans() {
    let rules = rules();
    let mut state = state(&rules);
    let u: Vec<ObjectId> = {
        let mut sim = Sim::new(&mut state, &rules);
        let u: Vec<_> = [10, 11, 12]
            .iter()
            .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
            .collect();
        execute(&mut sim, &mv(&u, (30, 12), 0, NO_DIRECTION), true);
        let g = find(&sim, 0).unwrap();
        // Leader = head of the command order (selected last); the others walk along.
        assert_eq!(g.packs.keys(), vec![u[2].0]);
        assert_eq!(g.packs.get(u[2].0), Some(&vec![u[1], u[0]]));
        assert!(posted_find_to(&sim, u[2]).is_some());
        assert!(posted_find_to(&sim, u[1]).is_none() && posted_find_to(&sim, u[0]).is_none());
        u
    };
    // The leader plans in the next object tick; its PATH distributes the path (posted PATH 0x500/0x600).
    for _ in 0..4 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    for &m in &u[..2] {
        let w = walk(&sim, m).unwrap();
        assert!(!w.high_path.is_empty(), "follower {m:?} has a path");
        assert_eq!(w.group, 0);
    }
    // Feedback messages only once (first pack), discarded afterwards.
    assert!(find(&sim, 0).unwrap().success.is_none());
}

#[test]
fn a_pack_whose_leader_does_not_plan_stays_put() {
    // K-218: followers get no FINDTO of their own.
    let rules = rules();
    let mut state = state(&rules);
    let (u, start) = {
        let mut sim = Sim::new(&mut state, &rules);
        let u: Vec<_> = [10, 11]
            .iter()
            .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
            .collect();
        execute(&mut sim, &mv(&u, (30, 12), 0, NO_DIRECTION), true);
        // The leader loses its command before it plans (like an abort by combat).
        sim.object_mut(u[1]).unwrap().fifo.clear();
        let start = sim.object(u[0]).unwrap().grid;
        (u, start)
    };
    for _ in 0..60 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(sim.object(u[0]).unwrap().grid, start);
    assert!(walk(&sim, u[0]).unwrap().high_path.is_empty());
}

#[test]
fn path_and_find_to_with_flag_0x10_keep_the_member_in_its_group() {
    // re/walk 2.3 (`0x0056F280`): flag 0x08 (group mode) or 0x10 (path distribution to the
    // pack members) leaves the unit in the group; without both it leaves.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = [5, 8, 11, 14]
        .iter()
        .map(|&x| spawn(&mut sim, INFANTRY, (x, 5)))
        .collect();
    let g = manual(&mut sim, 0);
    for &m in &u {
        add_member(&mut sim, g, m);
    }
    let (x, z) = (tile_center(30), tile_center(5));
    let path = |flags: i32| {
        crate::message::Envelope::bare(Message::new(0x500, &[x, z, 0, flags, -1, NO_DIRECTION, -1]))
    };
    let find_to = |flags: i32| {
        crate::message::Envelope::bare(crate::module::walk::find_to_msg(
            x,
            z,
            -1,
            NO_DIRECTION,
            0,
            false,
            flags,
        ))
    };
    crate::messaging::dispatch_command(&mut sim, u[0], &path(0x10));
    crate::messaging::dispatch_command(&mut sim, u[1], &find_to(0x10));
    for &m in &u[..2] {
        assert_eq!(walk(&sim, m).unwrap().group, g, "{m:?} stays in the group");
    }
    assert_eq!(find(&sim, g).unwrap().members.len(), 4);
    // Counter-check: without flag 0x08/0x10 both leave.
    crate::messaging::dispatch_command(&mut sim, u[2], &path(0));
    crate::messaging::dispatch_command(&mut sim, u[3], &find_to(0x04));
    for &m in &u[2..] {
        assert_eq!(walk(&sim, m).unwrap().group, NO_GROUP, "{m:?} has left");
    }
    assert_eq!(find(&sim, g).unwrap().members, vec![u[0], u[1]]);
}

#[test]
fn a_group_path_does_not_shift_waypoints_but_a_single_path_does() {
    // re/walk 2.6: `PropertyWalk_StartWalking(multi, ...)` with multi = (flag 0x08 == 0). If the course
    // runs into a blocked cell, `project_waypoints` shifts the waypoint by a quarter tile away
    // (only with multi); group members (flag 8) leave the waypoint where it is.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, INFANTRY, (10, 10));
    let b = spawn(&mut sim, INFANTRY, (10, 20));
    let c = spawn(&mut sim, INFANTRY, (10, 30));
    let g = manual(&mut sim, 0);
    add_member(&mut sim, g, a);
    find_mut(&mut sim, g).unwrap().packs.set(a.0, Vec::new());
    // Blocked (layer 0) on the rows of a and b, not on that of c.
    sim.state.path.mark_cell(1, 15, 10, true);
    sim.state.path.mark_cell(1, 15, 20, true);
    let path = |row: i32, flags: i32| {
        let (x, z) = (tile_center(20), tile_center(row));
        crate::message::Envelope::bare(Message::new(0x500, &[x, z, 0, flags, -1, NO_DIRECTION, -1]))
    };
    let end = |sim: &Sim, id: ObjectId| {
        let p = *walk(sim, id).unwrap().high_path.back().expect("path end");
        (p.x, p.z)
    };
    assert_eq!(
        crate::messaging::dispatch_command(&mut sim, a, &path(10, 8)),
        1
    );
    assert_eq!(
        crate::messaging::dispatch_command(&mut sim, b, &path(20, 0)),
        1
    );
    assert_eq!(
        crate::messaging::dispatch_command(&mut sim, c, &path(30, 0)),
        1
    );
    assert_eq!(
        end(&sim, a),
        (tile_center(20), tile_center(10)),
        "group: not shifted"
    );
    assert_eq!(
        end(&sim, b),
        (tile_center(20) - 0x400, tile_center(20)),
        "single path: a quarter tile away from the blocked cell"
    );
    assert_eq!(
        end(&sim, c),
        (tile_center(20), tile_center(30)),
        "free: not shifted"
    );
}

#[test]
fn a_member_joins_the_first_leader_in_map_order_that_fits() {
    // Two leaders, more than four tiles apart; C lies in between and fits both. The search runs in
    // `packs.keys()` = MfcMap order (bucket ascending), not in insertion order.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    // Spawn order determines the IDs: B -> slot 0 (bucket 0), A -> slot 1 (bucket 16).
    let b = spawn(&mut sim, INFANTRY, (15, 10));
    let a = spawn(&mut sim, INFANTRY, (10, 10));
    let c = spawn(&mut sim, INFANTRY, (12, 10));
    // Command order = selection reversed: A, B, C.
    execute(&mut sim, &mv(&[c, b, a], (30, 12), 0, NO_DIRECTION), true);
    let g = find(&sim, 0).unwrap();
    assert_eq!(g.members, vec![a, b, c]);
    assert!(none::can_join(&mut sim, a, c, 0) && none::can_join(&mut sim, b, c, 0));
    assert!(!none::can_join(&mut sim, a, b, 0), "5 tiles: two leaders");
    let g = find(&sim, 0).unwrap();
    // Precondition: MfcMap order != insertion order of the leaders (A before B).
    assert_eq!(g.packs.keys(), vec![b.0, a.0]);
    assert_eq!(
        g.packs.get(b.0),
        Some(&vec![c]),
        "C joins the first leader in map order"
    );
    assert_eq!(g.packs.get(a.0), Some(&Vec::new()));
}

#[test]
fn a_member_of_another_layer_stays_its_own_leader_despite_being_close() {
    // `Group_BuildPacks` compares the layers (`Group_GetMemberLayer`); the amphibian (layer 1) does
    // not walk in the pack of the infantry (layer 0), although it stands right next to it.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, INFANTRY, (10, 10));
    let w = spawn(&mut sim, AMPHIBIAN, (11, 10));
    let i2 = spawn(&mut sim, INFANTRY, (12, 10));
    assert_eq!(walk(&sim, a).unwrap().layer, 0);
    assert_eq!(walk(&sim, w).unwrap().layer, 1);
    assert!(
        none::can_join(&mut sim, a, w, 1),
        "distance and cells alone would have allowed it"
    );
    // Members: i2, w, a (selection a, w, i2 reversed).
    execute(&mut sim, &mv(&[a, w, i2], (30, 12), 0, NO_DIRECTION), true);
    let g = find(&sim, 0).unwrap();
    assert_eq!(g.members, vec![i2, w, a]);
    // i2 leads; w (layer 1) stays its own leader; a (layer 0) joins i2.
    assert_eq!(g.packs.get(i2.0), Some(&vec![a]));
    assert_eq!(g.packs.get(w.0), Some(&Vec::new()));
    assert_eq!(g.packs.keys().len(), 2);
}

fn line_of(sim: &mut Sim, inf: i32, pike: i32, warriors: i32) -> Vec<ObjectId> {
    let mut u = Vec::new();
    for (guid, n) in [(INFANTRY, inf), (PIKEMAN, pike), (WARRIOR, warriors)] {
        for _ in 0..n {
            let x = 10 + u.len() as i32;
            u.push(spawn(sim, guid, (x, 10)));
        }
    }
    u
}

#[test]
fn a_line_puts_pikemen_first_and_warriors_into_the_rear_group() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = line_of(&mut sim, 4, 2, 2);
    execute(&mut sim, &mv(&u, (30, 30), 1, NO_DIRECTION), true);
    let g = find(&sim, 0).expect("line");
    let f = g.formation.as_ref().unwrap();
    assert_eq!(g.members.len(), 6);
    assert_eq!(f.ranks[5].len() + f.ranks[4].len(), 6);
    assert_eq!(f.rear_members.len(), 2);
    let rear = find(&sim, f.rear).expect("rear group");
    assert_eq!(rear.kind, 0);
    // All carry the ID of the main group (the rear group too).
    assert!(u.iter().all(|&m| walk(&sim, m).unwrap().group == 0));
    assert_eq!(member_count(&sim, 0), 8);
    // Pikemen first (rank descending): the first two places of row 1.
    let (ax, az) = (tile_center(30), tile_center(30));
    let pike = f.ranks[5][0];
    // 6 rank members: per = 3, d = 512 -> start z = Mul(-1536, sp) = -3072, first place -2048.
    assert_eq!(
        g.targets.get(pike.0).map(|t| (t.0 - ax, t.1 - az)),
        Some((0, -2048))
    );
    // K-214: the rear group aims at the click point when forming (depth still 0).
    assert_eq!(rear.anchor, (ax, az, NO_DIRECTION));
    assert_eq!(f.depth, 3072);
    // The warriors are not in the targets of the main group; their FINDTO comes from the rear group
    // (pack leader = head of the rear group, the second warrior walks along in the pack).
    let (w_lead, w_follow) = (rear.members[0], rear.members[1]);
    assert_eq!(rear.members.len(), 2);
    assert!(u[6..].contains(&w_lead) && u[6..].contains(&w_follow));
    assert!(u[6..].iter().all(|m| g.targets.get(m.0).is_none()));
    let (x, z, flags) = posted_find_to(&sim, w_lead).expect("warrior leader walks");
    assert_eq!(flags & 8, 8, "group mode of the rear group");
    assert_eq!(Some(&(x, z, NO_DIRECTION)), rear.targets.get(w_lead.0));
    assert!(posted_find_to(&sim, w_follow).is_none());
    // The rank members get their FINDTO from the main group (per pack the leader).
    assert!(!g.packs.keys().is_empty());
    assert!(g.packs.keys().iter().all(|&l| {
        let (x, z, _) = posted_find_to(&sim, ObjectId(l)).expect("pack leader");
        Some(&(x, z, g.anchor.2)) == g.targets.get(l)
    }));
}

#[test]
fn only_warriors_in_a_formation_do_not_move_at_all() {
    // K-256 (re/groups-formations 12.9, corrected in re/groups-formations 9): 0 rank members ->
    // `Group_HandleSingleMember` reports success, vf34 never runs; the group exists, the rear group
    // gets no command.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = line_of(&mut sim, 0, 0, 6);
    execute(&mut sim, &mv(&u, (30, 30), 1, NO_DIRECTION), true);
    let g = find(&sim, 0).expect("main group stays");
    let rear = g.formation.as_ref().unwrap().rear;
    assert_eq!((g.members.len(), member_count(&sim, 0)), (0, 6));
    assert_eq!(find(&sim, rear).unwrap().members.len(), 6);
    assert!(u.iter().all(|&m| walk(&sim, m).unwrap().group == 0));
    assert!(
        u.iter().all(|&m| posted_find_to(&sim, m).is_none()),
        "nobody walks"
    );
}

#[test]
fn a_formation_place_outside_the_map_drops_the_command() {
    // K-212: blocked place or one far outside the map -> whole command lapses; rank 0 walks anyway.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = line_of(&mut sim, 4, 0, 1);
    execute(&mut sim, &mv(&u, (0, 0), 1, NO_DIRECTION), true);
    assert!(
        u[..4]
            .iter()
            .all(|&m| walk(&sim, m).unwrap().group == NO_GROUP)
    );
    assert!(u[..4].iter().all(|&m| posted_find_to(&sim, m).is_none()));
    assert!(
        posted_find_to(&sim, u[4]).is_some(),
        "warrior in the rear group walks"
    );
    assert!(
        sim.state.groups.map.is_empty(),
        "main and rear group dissolved"
    );
    // Type 0 does not check (K-208): the same click without a formation distributes the targets anyway.
    execute(&mut sim, &mv(&u[..4], (0, 0), 0, NO_DIRECTION), true);
    let gid = sim.state.groups.next_id - 1;
    let g = find(&sim, gid).expect("type 0 group exists");
    assert_eq!((g.kind, g.members.len(), g.targets.len()), (0, 4, 4));
    assert!(u[..4].iter().all(|&m| walk(&sim, m).unwrap().group == gid));
    let leaders = g.packs.keys();
    assert!(!leaders.is_empty());
    assert!(
        leaders
            .iter()
            .all(|&l| posted_find_to(&sim, ObjectId(l)).is_some()),
        "every pack leader gets its FINDTO"
    );
}

#[test]
fn a_formation_place_in_water_drops_the_command() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = line_of(&mut sim, 4, 0, 0);
    // Block the cell of the first place (land layer 0): 4 members -> per = 2, start z = az - 2048,
    // first place (az, az - 1024) = cell (30, 30).
    sim.state.path.mark_cell(1, 30, 30, true);
    execute(&mut sim, &mv(&u, (30, 30), 1, NO_DIRECTION), true);
    assert!(sim.state.groups.map.is_empty());
}

#[test]
fn a_formation_with_one_ranked_member_leaves_its_rear_group_standing() {
    // K-256: +0x2C == 1 -> special case, the rear group gets no command.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = line_of(&mut sim, 1, 0, 2);
    execute(&mut sim, &mv(&u, (30, 30), 1, NO_DIRECTION), true);
    assert!(posted_find_to(&sim, u[0]).is_some());
    assert!(posted_find_to(&sim, u[1]).is_none() && posted_find_to(&sim, u[2]).is_none());
}

#[test]
fn the_rear_group_empties_without_breaking_the_formation() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = line_of(&mut sim, 3, 0, 1);
    execute(&mut sim, &mv(&u, (30, 30), 1, NO_DIRECTION), true);
    let rear = find(&sim, 0).unwrap().formation.as_ref().unwrap().rear;
    remove_unit(&mut sim, 0, u[3], u[3], false);
    assert!(find(&sim, rear).is_none(), "empty rear group deleted");
    let f = find(&sim, 0).unwrap().formation.clone().unwrap();
    assert_eq!(
        (f.rear, f.rear_members.len(), member_count(&sim, 0)),
        (NO_GROUP, 0, 3)
    );
    assert_eq!(walk(&sim, u[3]).unwrap().group, NO_GROUP);
}

#[test]
fn spread_maps_1_to_2_onto_1_to_3_5() {
    // re/groups-formations 9: drag length 1.4 -> 5734 -> sp = 4096 + Mul(1638, 10240) = 8191; unchanged outside 1...2.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = line_of(&mut sim, 3, 0, 0);
    execute(&mut sim, &mv(&u, (30, 30), 1, NO_DIRECTION), true);
    let spacing = |sim: &Sim| find(sim, 0).unwrap().formation.as_ref().unwrap().spacing;
    formation::apply_spread(&mut sim, 0, 5734);
    assert_eq!(spacing(&sim), 8191);
    formation::apply_spread(&mut sim, 0, 0x2001);
    assert_eq!(spacing(&sim), 8191);
    formation::apply_spread(&mut sim, 0, 0x1000);
    assert_eq!(spacing(&sim), 0x1000);
}

#[test]
fn layers_merge_by_class_and_largest_size() {
    // Infantry 0x09 and start layer 0x39 -> class 0x08, size 1 -> layer 0; ship (0x22) does not fit.
    assert_eq!(formation::merge_layer(3, 0), Some(0));
    assert_eq!(formation::merge_layer(0, 6), None);
    assert_eq!(
        formation::merge_layer(0, 7),
        Some(5),
        "land 1x1 + land/water 2x2 -> land 2x2 (0x0A)"
    );
}

#[test]
fn a_type_6_formation_of_two_drops_the_command() {
    // K-210: type 6 also takes rank 0 (no rear group); the geometry always fails from two
    // members on (`0x00532A90`), the command lapses.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u = line_of(&mut sim, 1, 0, 1);
    execute(&mut sim, &mv(&u, (30, 30), 6, NO_DIRECTION), true);
    assert!(sim.state.groups.map.is_empty());
    assert!(u.iter().all(|&m| walk(&sim, m).unwrap().group == NO_GROUP));
    assert!(u.iter().all(|&m| posted_find_to(&sim, m).is_none()));
}

/// Forms a line of 4 infantrymen, waits until all stand still, then a second command -> march.
fn marching_line(state: &mut SimState, rules: &Rules, to: (i32, i32)) -> Vec<ObjectId> {
    let u: Vec<ObjectId> = {
        let mut sim = Sim::new(state, rules);
        let u: Vec<_> = (0..4)
            .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
            .collect();
        execute(&mut sim, &mv(&u, (12, 20), 1, NO_DIRECTION), true);
        u
    };
    for _ in 0..300 {
        step(state, rules, &[]);
    }
    let mut sim = Sim::new(state, rules);
    execute(&mut sim, &mv(&u, to, 1, NO_DIRECTION), true);
    u
}

#[test]
fn all_members_report_and_get_their_march_path() {
    // re/groups-formations 3.5 step 2/3: `0x2A00` goes to all, posted; after two object ticks (four
    // `step`s: the two tick groups alternate) all have reported and received their `0x1200`
    // (state 5 or already 10/0xB), targets = last frame. In the second `step` the leader plans
    // (vf18, frame) and reports itself in the same object tick.
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    let reported = |state: &mut SimState| {
        let sim = Sim::new(state, &rules);
        find(&sim, 0).unwrap().formation.as_ref().unwrap().reported
    };
    step(&mut state, &rules, &[]);
    step(&mut state, &rules, &[]);
    assert!(
        (1..4).contains(&reported(&mut state)),
        "leader reports immediately"
    );
    step(&mut state, &rules, &[]);
    step(&mut state, &rules, &[]);
    let sim = Sim::new(&mut state, &rules);
    let g = find(&sim, 0).unwrap();
    let f = g.formation.as_ref().unwrap();
    assert_eq!(f.reported as usize, g.members.len());
    assert_eq!(f.reported, 4);
    assert_eq!(g.targets.len(), 4);
    for &m in &u {
        let w = walk(&sim, m).unwrap();
        assert!(matches!(w.state, 5 | 0xA | 0xB), "{m:?} {}", w.state);
        assert_eq!(w.speed.cur, g.matched_speed);
        let t = g.targets.get(m.0).unwrap();
        let end = w.high_path.back().unwrap();
        assert_eq!(
            (end.x, end.z),
            (t.0, t.1),
            "path end = place in the last frame"
        );
    }
}

#[test]
fn the_march_reports_distributes_and_advances_in_lockstep() {
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    let mut seen_10 = false;
    let mut seen_5 = false;
    for _ in 0..800 {
        step(&mut state, &rules, &[]);
        let sim = Sim::new(&mut state, &rules);
        for &m in &u {
            let s = walk(&sim, m).unwrap().state;
            seen_5 |= s == 5;
            seen_10 |= s == 10;
            if matches!(s, 5 | 0xA | 0xB) {
                assert_eq!(
                    walk(&sim, m).unwrap().group,
                    0,
                    "march state only with a group"
                );
            }
        }
    }
    assert!(seen_5 && seen_10, "states 5 and 10 passed through");
    let sim = Sim::new(&mut state, &rules);
    let g = find(&sim, 0).unwrap();
    let f = g.formation.as_ref().unwrap();
    assert!(f.frames.is_empty(), "all frames processed");
    // All stand at their last frame place (= targets, target radius from the PATH) and have reported
    // success (state 1).
    for &m in &u {
        let o = sim.object(m).unwrap();
        let t = g.targets.get(m.0).unwrap();
        let d = crate::fx::fixed_hypot(o.grid.x.0 - t.0, o.grid.z.0 - t.1);
        assert!(d < 0x800, "{m:?} {d}");
        assert_eq!(walk(&sim, m).unwrap().state, 1);
    }
    // Speed in the march = smallest DefaultSpeed (all equal).
    assert_eq!(g.matched_speed, default_speed(&sim, u[0]));
}

#[test]
fn a_member_that_cannot_move_stalls_the_march() {
    // K-221: m_CanMove = 0 -> 0x2A00 aborts instead of reporting; reported stays below members.
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    walk_mut(&mut Sim::new(&mut state, &rules), u[1])
        .unwrap()
        .can_move = false;
    for _ in 0..40 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    let f = find(&sim, 0).unwrap().formation.clone().unwrap();
    assert!(f.reported < 4);
    assert!(u.iter().all(|&m| walk(&sim, m).unwrap().state != 5));
}

#[test]
fn a_straggler_chases_at_one_and_a_half_speed_and_rejoins() {
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    // Move the first one back by half a tile in the middle of the march and let it replan (`0x1000`,
    // like after a failed step): from state 5 -> `EnterFormationWalk` (re/groups-formations 3.6) ->
    // state 0xB with 1.5x. Moving alone is not enough in the reimplementation: the next step sets
    // the unit back on its precomputed trajectory.
    let mut chased = false;
    let mut rejoined = false;
    for i in 0..800 {
        step(&mut state, &rules, &[]);
        let mut sim = Sim::new(&mut state, &rules);
        let (s, cur, cruising) = walk(&sim, u[0])
            .map(|w| (w.state, w.speed.cur, w.speed.cruising))
            .unwrap();
        if i == 30 && s == 5 {
            let o = sim.object(u[0]).unwrap();
            let p = FxVec3::new(Fx(o.grid.x.0 - 2048), o.grid.y, o.grid.z);
            crate::world::set_grid_position(&mut sim, u[0], p, true);
            send(&mut sim, u[0], &Message::new(0x1000, &[-1]));
        }
        if s == 0xB {
            chased = true;
            assert!(cur > cruising, "1.5x while catching up");
        } else if chased && s == 5 {
            rejoined = true;
        }
    }
    assert!(chased && rejoined);
}

#[test]
fn halt_mid_march_leaves_no_formation_state() {
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    for _ in 0..30 {
        step(&mut state, &rules, &[]);
    }
    let halt = Command {
        player: 0,
        kind: CommandKind::Halt { units: u.clone() },
    };
    step(&mut state, &rules, &[halt]);
    step(&mut state, &rules, &[]);
    let sim = Sim::new(&mut state, &rules);
    assert!(
        u.iter()
            .all(|&m| !matches!(walk(&sim, m).unwrap().state, 5 | 0xA | 0xB))
    );
    // Savegame stays loadable (load check).
    let bytes = crate::save::testkit::save_bytes(&state, &rules);
    assert!(crate::save::SaveGame::from_bytes(&bytes).is_ok());
}

#[test]
fn a_march_with_a_target_direction_ends_turned_on_the_last_frame() {
    // Flow: second command with direction 90 degrees. At the end a final turn around the inner wing end
    // (re/groups-formations 12.3): anchor = pivot != path end, direction 90 degrees, targets with
    // 90 degrees. All frames are processed, all stand at their place in the last frame and report
    // success.
    let rules = rules();
    let mut state = state(&rules);
    let u: Vec<ObjectId> = {
        let mut sim = Sim::new(&mut state, &rules);
        let u: Vec<_> = (0..4)
            .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
            .collect();
        execute(&mut sim, &mv(&u, (12, 20), 1, NO_DIRECTION), true);
        u
    };
    for _ in 0..300 {
        step(&mut state, &rules, &[]);
    }
    {
        let mut sim = Sim::new(&mut state, &rules);
        execute(&mut sim, &mv(&u, (40, 20), 1, 90 << 12), true);
    }
    for i in 0..800 {
        step(&mut state, &rules, &[]);
        if i == 1 {
            let sim = Sim::new(&mut state, &rules);
            let g = find(&sim, 0).unwrap();
            assert!(!g.formation.as_ref().unwrap().frames.is_empty());
            assert_eq!(g.anchor.2, 90 << 12);
            assert_ne!(
                (g.anchor.0, g.anchor.1),
                (tile_center(40), tile_center(20)),
                "pivot instead of path end"
            );
        }
    }
    let sim = Sim::new(&mut state, &rules);
    let g = find(&sim, 0).unwrap();
    assert!(
        g.formation.as_ref().unwrap().frames.is_empty(),
        "all frames processed"
    );
    for &m in &u {
        let o = sim.object(m).unwrap();
        let t = g.targets.get(m.0).unwrap();
        assert_eq!(t.2, 90 << 12);
        let d = crate::fx::fixed_hypot(o.grid.x.0 - t.0, o.grid.z.0 - t.1);
        assert!(d < 0x800, "{m:?} {d}");
        assert_eq!(walk(&sim, m).unwrap().state, 1);
    }
}

#[test]
fn a_waiting_member_leaving_without_abort_chases_without_a_group_and_loads() {
    // Member waits patiently (`0x2E00` in state 5 -> `EnterFormationWalk`, state 8 with previous
    // state 0xB). A new line with a blocked place (K-212) takes it in and dissolves: `0x1600 {-1}` in
    // state 8 does not abort. After the wait it is in 0xB without a group. This state must be savable
    // and loadable and continues identically; the state winds down (abort or success at the path
    // end), without a panic.
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    let m = u[0];
    let mut i = 0;
    loop {
        step(&mut state, &rules, &[]);
        i += 1;
        let sim = Sim::new(&mut state, &rules);
        let w = walk(&sim, m).unwrap();
        if w.state == 5 && w.trajectory.len() > 6 && i > 20 {
            break;
        }
        assert!(i < 100, "march runs");
    }
    {
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(send(&mut sim, m, &Message::new(0x2E00, &[])), 1);
        let w = walk(&sim, m).unwrap();
        assert_eq!((w.state, w.restore_state, w.group), (8, 0xB, 0));
        let extra = spawn(&mut sim, INFANTRY, (5, 5));
        // Places around (50, 50) blocked -> the line lapses (K-212).
        for x in 47..=53 {
            for z in 47..=53 {
                sim.state.path.mark_cell(1, x, z, true);
            }
        }
        execute(&mut sim, &mv(&[m, extra], (50, 50), 1, NO_DIRECTION), true);
        assert!(find(&sim, 1).is_none(), "new line lapsed");
        let w = walk(&sim, m).unwrap();
        assert_eq!((w.state, w.restore_state, w.group), (8, 0xB, NO_GROUP));
        assert!(!find(&sim, 0).unwrap().members.contains(&m));
    }
    // Waiting (23 object ticks) and replanning from previous state 0xB.
    let mut n = 0;
    loop {
        step(&mut state, &rules, &[]);
        n += 1;
        let sim = Sim::new(&mut state, &rules);
        if walk(&sim, m).unwrap().state == 0xB {
            break;
        }
        assert!(n < 200, "state 0xB without a group reached");
    }
    let bytes = crate::save::testkit::save_bytes(&state, &rules);
    let mut loaded = crate::save::SaveGame::from_bytes(&bytes)
        .expect("0xB without a group loads")
        .state;
    for _ in 0..600 {
        step(&mut state, &rules, &[]);
        step(&mut loaded, &rules, &[]);
        assert_eq!(
            crate::save::checksum(&state),
            crate::save::checksum(&loaded)
        );
    }
    let sim = Sim::new(&mut state, &rules);
    let w = walk(&sim, m).unwrap();
    assert_eq!(w.group, NO_GROUP);
    assert!(!matches!(w.state, 5 | 0xA | 0xB), "wound down: {}", w.state);
}

fn cmd(kind: CommandKind) -> Command {
    Command { player: 0, kind }
}

fn responses(sim: &Sim) -> usize {
    sim.state
        .events
        .iter()
        .filter(|e| matches!(e, crate::state::SimEvent::UnitResponse { .. }))
        .count()
}

#[test]
fn scatter_moves_units_radially_one_and_a_half_times_out() {
    // re/groups-formations 3.9: (0,0) and (4096,0) -> c = (2048,0) -> targets (-1024,0) and (5120,0).
    // Here shifted by (20,20).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = spawn(&mut sim, INFANTRY, (20, 20));
    let b = spawn(&mut sim, INFANTRY, (21, 20));
    let c = tile_center(20) + 2048;
    execute(
        &mut sim,
        &cmd(CommandKind::Scatter { units: vec![a, b] }),
        true,
    );
    assert_eq!(
        posted_find_to(&sim, a).map(|p| (p.0, p.1)),
        Some((c - 3072, tile_center(20)))
    );
    assert_eq!(
        posted_find_to(&sim, b).map(|p| (p.0, p.1)),
        Some((c + 3072, tile_center(20)))
    );
    // FINDTO flags 0x201 (re/groups-formations 3.9).
    assert_eq!(posted_find_to(&sim, a).map(|p| p.2), Some(0x201));
    // K-220: response sound per unit.
    assert_eq!(responses(&sim), 2);
}

#[test]
fn scatter_leaves_a_unit_on_the_centroid_in_place_and_in_its_group() {
    // re/groups-formations 3.9: d == 0 -> neither command nor leaving; a single unit (k = 1) does nothing.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = (0..3)
        .map(|i| spawn(&mut sim, INFANTRY, (20 + i, 20)))
        .collect();
    execute(&mut sim, &mv(&u, (40, 40), 0, NO_DIRECTION), true);
    for &m in &u {
        sim.object_mut(m).unwrap().fifo.clear();
    }
    execute(
        &mut sim,
        &cmd(CommandKind::Scatter { units: u.clone() }),
        true,
    );
    assert!(posted_find_to(&sim, u[1]).is_none(), "at the centroid");
    assert_eq!(walk(&sim, u[1]).unwrap().group, 0);
    assert_eq!(find(&sim, 0).unwrap().members, vec![u[1]]);
    assert!(posted_find_to(&sim, u[0]).is_some() && posted_find_to(&sim, u[2]).is_some());
    sim.object_mut(u[1]).unwrap().fifo.clear();
    execute(
        &mut sim,
        &cmd(CommandKind::Scatter { units: vec![u[1]] }),
        true,
    );
    assert!(posted_find_to(&sim, u[1]).is_none(), "k = 1");
}

#[test]
fn scatter_mid_march_dissolves_the_group() {
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    for _ in 0..20 {
        step(&mut state, &rules, &[]);
    }
    let mut sim = Sim::new(&mut state, &rules);
    execute(
        &mut sim,
        &cmd(CommandKind::Scatter { units: u.clone() }),
        true,
    );
    assert!(find(&sim, 0).is_none());
    assert!(u.iter().all(|&m| walk(&sim, m).unwrap().group == NO_GROUP));
}

#[test]
fn set_formation_reforms_the_selection_around_its_centroid() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = (0..4)
        .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
        .collect();
    execute(
        &mut sim,
        &cmd(CommandKind::SetFormation {
            units: u.clone(),
            formation: 3,
        }),
        true,
    );
    let g = find(&sim, 0).expect("column");
    assert_eq!(g.kind, 3);
    // Anchor -1 -> centroid, direction "none" -> 0 (re/groups-formations 12.9).
    assert_eq!(g.anchor, (g.centroid.0, g.centroid.1, 0));
    // Acknowledgment once (via `CmdMoveSelection`).
    assert_eq!(responses(&sim), 1);
    // n = 1: no effect.
    execute(
        &mut sim,
        &cmd(CommandKind::SetFormation {
            units: u[..1].to_vec(),
            formation: 1,
        }),
        true,
    );
    assert_eq!(find(&sim, 0).unwrap().kind, 3);
    assert_eq!(responses(&sim), 1);
}

#[test]
fn set_formation_0_dissolves_a_formation() {
    // re/groups-formations 12.10: type 0 with anchor -1 fails in vf34 -> group falls apart.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = (0..4)
        .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
        .collect();
    execute(
        &mut sim,
        &cmd(CommandKind::SetFormation {
            units: u.clone(),
            formation: 1,
        }),
        true,
    );
    assert_eq!(find(&sim, 0).map(|g| g.kind), Some(1));
    execute(
        &mut sim,
        &cmd(CommandKind::SetFormation {
            units: u.clone(),
            formation: 0,
        }),
        true,
    );
    assert!(sim.state.groups.map.is_empty());
    assert!(u.iter().all(|&m| walk(&sim, m).unwrap().group == NO_GROUP));
}

#[test]
fn set_formation_with_the_same_formation_leaves_the_rear_group_standing() {
    // K-222 clarified statically: realigning with anchor -1 moves neither formation nor rear group.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let mut u: Vec<_> = (0..3)
        .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
        .collect();
    u.extend((0..2).map(|i| spawn(&mut sim, WARRIOR, (10 + i, 12))));
    execute(
        &mut sim,
        &cmd(CommandKind::SetFormation {
            units: u.clone(),
            formation: 1,
        }),
        true,
    );
    let rear = find(&sim, 0).unwrap().formation.as_ref().unwrap().rear;
    assert!(find(&sim, rear).is_some(), "rear group of the warriors");
    for &m in &u {
        sim.object_mut(m).unwrap().fifo.clear();
    }
    execute(
        &mut sim,
        &cmd(CommandKind::SetFormation {
            units: u.clone(),
            formation: 1,
        }),
        true,
    );
    assert!(find(&sim, rear).is_some(), "rear group stays");
    assert!(u.iter().all(|&m| posted_find_to(&sim, m).is_none()));
}

#[test]
fn set_formation_and_scatter_from_the_queue_run_at_once_without_clearing() {
    // Non-queueable commands (`0x0048DD00`): immediately, without `direct` (queue stays).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = (0..2)
        .map(|i| spawn(&mut sim, INFANTRY, (20 + i, 20)))
        .collect();
    let queued = |kind| {
        cmd(CommandKind::Queue {
            cyclic: false,
            inner: Box::new(kind),
        })
    };
    execute(
        &mut sim,
        &queued(mv(&u, (30, 30), 0, NO_DIRECTION).kind),
        true,
    );
    assert_eq!(sim.object(u[0]).unwrap().commands.len(), 1);
    execute(
        &mut sim,
        &queued(CommandKind::Scatter { units: u.clone() }),
        true,
    );
    assert!(posted_find_to(&sim, u[0]).is_some());
    assert_eq!(sim.object(u[0]).unwrap().commands.len(), 1, "not cleared");
    assert_eq!(responses(&sim), 0, "no response sound without direct");
    execute(
        &mut sim,
        &queued(CommandKind::SetFormation {
            units: u.clone(),
            formation: 1,
        }),
        true,
    );
    assert_eq!(find(&sim, 0).map(|g| g.kind), Some(1));
    assert_eq!(sim.object(u[1]).unwrap().commands.len(), 1, "not cleared");
}

#[test]
fn a_unit_changing_owner_leaves_its_group() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let u: Vec<_> = [10, 16]
        .iter()
        .map(|&x| spawn(&mut sim, INFANTRY, (x, 10)))
        .collect();
    execute(&mut sim, &mv(&u, (30, 20), 0, NO_DIRECTION), true);
    assert_eq!(find(&sim, 0).unwrap().members, vec![u[1], u[0]]);
    let new = crate::world::change_owner(&mut sim, u[0], 1).expect("new ID");
    assert_eq!(walk(&sim, new).unwrap().group, NO_GROUP);
    assert_eq!(find(&sim, 0).unwrap().members, vec![u[1]]);
}

#[test]
fn a_member_dying_mid_march_stalls_the_others_at_the_next_frame() {
    // K-258: leaving (`0x00429640`/`0x00406F80`) does not remove the member from `ready`; the
    // frame never advances again, the others wait in state 10.
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    for _ in 0..40 {
        step(&mut state, &rules, &[]);
    }
    let alive = [u[0], u[1], u[3]];
    let offsets = |state: &mut SimState| {
        let sim = Sim::new(state, &rules);
        let g = find(&sim, 0).unwrap();
        alive.map(|m| g.offsets.get(m.0).copied())
    };
    let before = offsets(&mut state);
    // Dying = change of owner to slot 8 (`QueueOwnerChange(hid, 8)`).
    let dead = {
        let mut sim = Sim::new(&mut state, &rules);
        crate::world::change_owner(&mut sim, u[2], 8).expect("slot 8")
    };
    for _ in 0..900 {
        step(&mut state, &rules, &[]);
    }
    {
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(walk(&sim, dead).unwrap().group, NO_GROUP);
        let g = find(&sim, 0).unwrap();
        let f = g.formation.as_ref().unwrap();
        assert_eq!(g.members.len(), 3);
        assert!(f.ready.contains(&u[2]), "the dead one stays in ready");
        assert!(!f.frames.is_empty());
        assert!(alive.iter().all(|&m| walk(&sim, m).unwrap().state == 10));
    }
    // A new command to the rest resolves the standstill: they are exactly the members ->
    // realign the same group (`has_same_members`), `ready = members` (re/groups-formations 12.9).
    {
        let mut sim = Sim::new(&mut state, &rules);
        execute(&mut sim, &mv(&alive, (40, 30), 1, NO_DIRECTION), true);
        let g = find(&sim, 0).expect("same group");
        assert_eq!(g.formation.as_ref().unwrap().ready, g.members);
        assert!(!g.formation.as_ref().unwrap().ready.contains(&u[2]));
    }
    // K-215: realigning does not recompute places, the gap of the dead one stays.
    assert_eq!(offsets(&mut state), before);
    assert!(before.iter().all(Option::is_some));
    for _ in 0..800 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    let g = find(&sim, 0).unwrap();
    assert!(
        g.formation.as_ref().unwrap().frames.is_empty(),
        "all frames processed"
    );
    for &m in &alive {
        let o = sim.object(m).unwrap();
        let t = g.targets.get(m.0).unwrap();
        let d = crate::fx::fixed_hypot(o.grid.x.0 - t.0, o.grid.z.0 - t.1);
        assert!(d < 0x800, "{m:?} {d}");
        assert_eq!(walk(&sim, m).unwrap().state, 1);
    }
}

#[test]
fn a_formation_marches_around_a_wall() {
    // March around a wall (x = 25, z = 14...26), target direction 90 degrees and 0 degrees.
    // Without collision partners a straggler hangs in planning -> collision -> `0x1000` (state 4,
    // previous state 0xB) or the line waits at the frame (measured with `p3` switched off). With
    // collision partners (re/groups-formations 4.3/12.7, re/collision B4.5) the members ignore
    // each other: all frames are processed, nobody leaves, all stand at their place in the last
    // frame and report success (state 1).
    for dir in [90 << 12, 0] {
        let rules = rules();
        let mut state = state(&rules);
        for z in 14..=26 {
            state.path.mark_cell(1, 25, z, true);
        }
        state.path.flush_dirty_rect();
        let u: Vec<ObjectId> = {
            let mut sim = Sim::new(&mut state, &rules);
            let u: Vec<_> = (0..4)
                .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
                .collect();
            execute(&mut sim, &mv(&u, (12, 20), 1, NO_DIRECTION), true);
            u
        };
        for _ in 0..300 {
            step(&mut state, &rules, &[]);
        }
        {
            let mut sim = Sim::new(&mut state, &rules);
            execute(&mut sim, &mv(&u, (40, 20), 1, dir), true);
        }
        let (mut ticks, mut started) = (0, false);
        loop {
            step(&mut state, &rules, &[]);
            ticks += 1;
            let sim = Sim::new(&mut state, &rules);
            let g = find(&sim, 0).expect("line stays");
            let frames = g.formation.as_ref().unwrap().frames.len();
            started |= frames > 0;
            if started && frames == 0 && u.iter().all(|&m| walk(&sim, m).unwrap().state == 1) {
                break;
            }
            assert!(
                ticks < 3000,
                "march ({dir}) does not end: {:?}",
                u.iter()
                    .map(|&m| {
                        let w = walk(&sim, m).unwrap();
                        (w.state, w.restore_state, w.group, w.high_path.len())
                    })
                    .collect::<Vec<_>>()
            );
        }
        let sim = Sim::new(&mut state, &rules);
        let g = find(&sim, 0).unwrap();
        assert_eq!(g.members.len(), 4, "nobody left");
        for &m in &u {
            assert_eq!(walk(&sim, m).unwrap().group, 0);
            let o = sim.object(m).unwrap();
            let t = g.targets.get(m.0).unwrap();
            assert_eq!(t.2, dir);
            let d = crate::fx::fixed_hypot(o.grid.x.0 - t.0, o.grid.z.0 - t.1);
            assert!(d < 0x800, "{m:?} {d}");
            assert!(o.grid.x.0 > tile_center(25), "behind the wall");
        }
    }
}

/// Line of 4 infantrymen and 2 pikemen, plus 2 warriors (rear group). Tick 0: formation setup at
/// (20, 30). Tick 260: second command to the same selection to (45, 40) - a bar of blocked
/// cells at x = 28...32, z = 25...38 forces a curve around its north end.
///
/// The bar is five cells wide, not one: with x = 30 alone the leader passes the north end at 0.56 tiles,
/// the frame places of the line lie up to 0.95 tiles to the side of it - that is, in the blocked cell
/// (30, 25). The original does not check the places of the frames (re/groups-formations 0 no. 7, 3.2,
/// 4.1, 12.3: only the formation setup checks). With x = 30 the march does not end (K-259, test
/// `members_running_into_a_blocked_frame_place_drop_out_and_a_rejoiner_stalls_the_march`): the
/// members running into the wall drop out of `ready` at the first `0x1000` via `EnterFormationWalk`,
/// the rest advances without them; but one comes back via `Rejoin` with places of discarded
/// frames in the HighPath, lands on one of them in 0xB **in `ready`** and never reaches its place
/// exactly - the others wait forever in state 10 (like K-258). A march that never ends
/// is not suitable as the golden sequence of the normal case.
fn formation_scene() -> (SimState, Rules, Vec<ObjectId>) {
    formation_scene_with_bar(28..=32)
}

/// Scene like `formation_scene`, bar at `bar` x z = 25...38.
fn formation_scene_with_bar(
    bar: std::ops::RangeInclusive<i32>,
) -> (SimState, Rules, Vec<ObjectId>) {
    let rules = rules();
    let mut state = state(&rules);
    for x in bar {
        for z in 25..=38 {
            state.path.mark_cell(0x1FF, x, z, true);
        }
    }
    state.path.flush_dirty_rect();
    let u = {
        let mut sim = Sim::new(&mut state, &rules);
        let mut u = Vec::new();
        for (guid, n) in [(INFANTRY, 4), (PIKEMAN, 2), (WARRIOR, 2)] {
            for _ in 0..n {
                let x = 10 + u.len() as i32;
                u.push(spawn(&mut sim, guid, (x, 10)));
            }
        }
        u
    };
    (state, rules, u)
}

const SECOND_ORDER_AT: usize = 260;
const STEPS: usize = 1200;

/// Commands of the scene in tick `i`: formation setup in tick 0, second command in `SECOND_ORDER_AT`.
fn formation_orders(i: usize, u: &[ObjectId]) -> Vec<Command> {
    match i {
        0 => vec![mv(u, (20, 30), 1, NO_DIRECTION)],
        SECOND_ORDER_AT => vec![mv(u, (45, 40), 1, NO_DIRECTION)],
        _ => vec![],
    }
}

/// State assertion at the save time: state, rules, units of the scene.
type Probe = fn(&mut SimState, &Rules, &[ObjectId]);

/// Checksum per tick of the scene. With `save_at = Some(i)` the state is saved after tick `i` and
/// reloaded; `at_save` sees the state immediately before.
fn formation_run(save_at: Option<usize>, at_save: Probe) -> Vec<u64> {
    let (mut state, rules, u) = formation_scene();
    let mut out = Vec::with_capacity(STEPS);
    for i in 0..STEPS {
        step(&mut state, &rules, &formation_orders(i, &u));
        if save_at == Some(i) {
            at_save(&mut state, &rules, &u);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

fn formation_checksums(save_at: Option<usize>) -> Vec<u64> {
    formation_run(save_at, |_, _, _| {})
}

/// Measured (line with a rear group, march with a curve).
/// Changed: new state fields (event bits, last track, meeting point), behavior unchanged.
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
const FORMATION_GOLDEN: u64 = 0xc843_0b94_e493_c12c;

#[test]
fn formation_checksums_match_the_golden_sequence() {
    let seq = formation_checksums(None);
    let mut h = crate::rules::Fnv64::default();
    for c in &seq {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, FORMATION_GOLDEN, "{:#018x}", h.0);
}

#[test]
fn the_formation_scene_marches_through_all_states() {
    // Counter-check before entering the golden: the scene shows what it should show.
    let (mut state, rules, u) = formation_scene();
    let mut seen = std::collections::BTreeSet::new();
    let mut done_at = None;
    for i in 0..STEPS {
        step(&mut state, &rules, &formation_orders(i, &u));
        let sim = Sim::new(&mut state, &rules);
        for &m in &u {
            seen.insert(walk(&sim, m).unwrap().state);
        }
        let frames_left = find(&sim, 0)
            .and_then(|g| g.formation.as_ref().map(|f| f.frames.len()))
            .unwrap();
        if i > SECOND_ORDER_AT
            && done_at.is_none()
            && frames_left == 0
            && u.iter().all(|&m| walk(&sim, m).unwrap().state == 1)
        {
            done_at = Some(i);
        }
    }
    assert!(seen.contains(&5) && seen.contains(&10), "{seen:?}");
    assert!(done_at.is_some(), "march ends within {STEPS} ticks");
    let sim = Sim::new(&mut state, &rules);
    let g = find(&sim, 0).expect("line");
    assert_eq!(g.anchor.0 >> 12, 45);
    assert_eq!(g.members.len(), 6, "nobody left");
    // All rank members stand near their target.
    for &m in &g.members {
        let t = g.targets.get(m.0).unwrap();
        let o = sim.object(m).unwrap();
        assert!(crate::fx::fixed_hypot(o.grid.x.0 - t.0, o.grid.z.0 - t.1) < 0x1000);
    }
    // The warriors belong to the rear group, not to the targets of the main group. The second
    // command has no target direction: `vf50` (`0x00429170`) then returns the anchor itself
    // (re/groups-formations 3.2), so the warriors stand left and right of the click point instead of
    // `depth` behind it. Only a target direction would push them to the back (`rear_point`).
    let f = g.formation.as_ref().unwrap();
    let rear = find(&sim, f.rear).expect("rear group");
    assert_eq!(rear.members.len(), 2);
    assert_eq!(rear.anchor, (g.anchor.0, g.anchor.1, NO_DIRECTION));
    for &w in &rear.members {
        assert!(u[6..].contains(&w) && g.targets.get(w.0).is_none());
        let t = rear.targets.get(w.0).unwrap();
        let o = sim.object(w).unwrap();
        assert!(crate::fx::fixed_hypot(o.grid.x.0 - t.0, o.grid.z.0 - t.1) < 0x1000);
        assert_eq!(t.1, g.anchor.1, "same row as the click point");
        assert_eq!((t.0 - g.anchor.0).abs(), 2048, "one place left/right");
    }
}

#[test]
fn formation_checksums_survive_saving_and_loading_mid_march() {
    let reference = formation_checksums(None);
    // Formation setup, reporting (shortly after the second command), lockstep, catching up, end. For each
    // save point the state in which it is saved is asserted (measured on the scene: reporting
    // tick 261/262, states 5 and 10 from 263, frame change until 802, end 822).
    let cases: [(usize, &str, Probe); 6] = [
        (
            5,
            "Formation setup: the line walks in state 2",
            |s, r, u| {
                let sim = Sim::new(s, r);
                assert!(u.iter().all(|&m| walk(&sim, m).unwrap().state == 2));
                assert!(find(&sim, 0).unwrap().formation.is_some());
            },
        ),
        (
            SECOND_ORDER_AT + 1,
            "Reporting: reported < members",
            |s, r, _| {
                let sim = Sim::new(s, r);
                let g = find(&sim, 0).unwrap();
                let f = g.formation.as_ref().unwrap();
                assert!(
                    f.reported >= 1 && (f.reported as usize) < g.members.len(),
                    "{} of {}",
                    f.reported,
                    g.members.len()
                );
                assert_eq!(f.frames.len(), 6);
            },
        ),
        (
            SECOND_ORDER_AT + 3,
            "Lockstep: states 5 and 10 at once",
            |s, r, u| {
                let sim = Sim::new(s, r);
                let st: Vec<_> = u.iter().map(|&m| walk(&sim, m).unwrap().state).collect();
                assert!(st.contains(&5) && st.contains(&10), "{st:?}");
                let g = find(&sim, 0).unwrap();
                let f = g.formation.as_ref().unwrap();
                assert_eq!(f.reported as usize, g.members.len());
                assert!(f.arrived > 0);
            },
        ),
        (
            SECOND_ORDER_AT + 60,
            "March: frames open, members in state 5",
            |s, r, u| {
                let sim = Sim::new(s, r);
                let g = find(&sim, 0).unwrap();
                assert_eq!(g.formation.as_ref().unwrap().frames.len(), 5);
                assert!(u[..6].iter().all(|&m| walk(&sim, m).unwrap().state == 5));
            },
        ),
        (
            SECOND_ORDER_AT + 200,
            "around the curve: frames open, states 5 and 10",
            |s, r, u| {
                let sim = Sim::new(s, r);
                let st: Vec<_> = u.iter().map(|&m| walk(&sim, m).unwrap().state).collect();
                let g = find(&sim, 0).unwrap();
                assert!(!g.formation.as_ref().unwrap().frames.is_empty());
                assert!(st.iter().any(|&x| matches!(x, 5 | 10)), "{st:?}");
            },
        ),
        (
            STEPS - 10,
            "End: frames empty, all in state 1",
            |s, r, u| {
                let sim = Sim::new(s, r);
                let g = find(&sim, 0).unwrap();
                assert!(g.formation.as_ref().unwrap().frames.is_empty());
                assert!(u.iter().all(|&m| walk(&sim, m).unwrap().state == 1));
            },
        ),
    ];
    for (at, what, probe) in cases {
        let seq = formation_run(Some(at), probe);
        assert_eq!(seq, reference, "saving/loading in tick {at} ({what})");
    }
}

/// March of the line from `marching_line` (target (40, 20)); in tick 30 the first one is set back by
/// half a tile and replanned (like `a_straggler_chases_at_one_and_a_half_speed_and_rejoins`):
/// it catches up in state 0xB, the others wait in 10.
fn straggler_run(save_at: Option<usize>, at_save: Probe) -> Vec<u64> {
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    let mut out = Vec::new();
    for i in 0..800 {
        step(&mut state, &rules, &[]);
        if i == 30 {
            let mut sim = Sim::new(&mut state, &rules);
            assert_eq!(walk(&sim, u[0]).unwrap().state, 5, "injection in the march");
            let o = sim.object(u[0]).unwrap();
            let p = FxVec3::new(Fx(o.grid.x.0 - 2048), o.grid.y, o.grid.z);
            crate::world::set_grid_position(&mut sim, u[0], p, true);
            send(&mut sim, u[0], &Message::new(0x1000, &[-1]));
        }
        if save_at == Some(i) {
            at_save(&mut state, &rules, &u);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

#[test]
fn a_straggler_chase_survives_saving_and_loading() {
    let reference = straggler_run(None, |_, _, _| {});
    // Measured: state 4 from tick 30, catching up (0xB) ticks 34...73, lockstep afterwards, state 10
    // shortly before the end (395).
    let cases: [(usize, &str, Probe); 4] = [
        (30, "Planning failure: state 4", |s, r, u| {
            let sim = Sim::new(s, r);
            assert_eq!(walk(&sim, u[0]).unwrap().state, 4);
        }),
        (34, "Catching up begins: 0xB", |s, r, u| {
            let sim = Sim::new(s, r);
            assert_eq!(walk(&sim, u[0]).unwrap().state, 0xB);
        }),
        (
            55,
            "Catching up in the middle: 0xB, without lockstep",
            |s, r, u| {
                let sim = Sim::new(s, r);
                assert_eq!(walk(&sim, u[0]).unwrap().state, 0xB);
                let g = find(&sim, 0).unwrap();
                assert!(!g.formation.as_ref().unwrap().ready.contains(&u[0]));
                assert!(u[1..].iter().all(|&m| walk(&sim, m).unwrap().state == 5));
            },
        ),
        (395, "Arrival: state 10 next to 5", |s, r, u| {
            let sim = Sim::new(s, r);
            let st: Vec<_> = u.iter().map(|&m| walk(&sim, m).unwrap().state).collect();
            assert!(st.contains(&5) && st.contains(&10), "{st:?}");
        }),
    ];
    for (at, what, probe) in cases {
        let seq = straggler_run(Some(at), probe);
        assert_eq!(seq, reference, "saving/loading in tick {at} ({what})");
    }
}

#[test]
fn a_chase_dropping_its_chase_point_falls_back_to_the_next_high_point() {
    // `TickFormationChase` (`0x00536DC0`) deletes the chase point only with `CPtrList::RemoveHead`
    // (`0x00536E5B`-`0x00536E70`) and leaves `+0x27C` (`next_hp`) in place. If it then returns
    // without a projection (near, `Rejoin == 0`), `TickWalking` (`0x0053AEB0`) falls back to the new
    // head instead of turning on the spot without a waypoint and ending in state 1 via
    // `OnTargetReached` (otherwise a hang in `ready`).
    let rules = rules();
    let mut state = state(&rules);
    let u = marching_line(&mut state, &rules, (40, 20));
    for _ in 0..20 {
        step(&mut state, &rules, &[]);
    }
    let mut sim = Sim::new(&mut state, &rules);
    // The `ready` head is its own reference: `ChaseSlot(r, 0)` = own position -> near; it is already
    // in `ready` -> `Rejoin == 0` -> return without projection.
    let r = find(&sim, 0).unwrap().formation.as_ref().unwrap().ready[0];
    let (slot, p) = {
        let w = walk_mut(&mut sim, r).unwrap();
        assert_eq!(w.state, 5, "in lockstep");
        let slot = *w.high_path.front().unwrap();
        let p = w.point(slot.x, slot.z.wrapping_add(0x4000));
        w.high_path.push_front(p);
        w.next_hp = Some(p.id);
        w.chase_pos = true;
        w.ticks_since_chase = 18;
        w.state = 0xB;
        (slot, p)
    };
    crate::module::walk::tick(&mut sim, r);
    let w = walk(&sim, r).unwrap();
    assert_eq!(w.state, 0xB);
    assert!(!w.chase_pos);
    assert_eq!(
        w.high_path.front().map(|h| h.id),
        Some(slot.id),
        "chase point deleted"
    );
    assert!(w.high_path.iter().all(|h| h.id != p.id));
    assert_eq!(
        w.next_hp,
        Some(slot.id),
        "TickWalking falls back to the new head"
    );
    // The march ends: all frames processed, all at their place, state 1.
    let mut done = false;
    for _ in 0..2000 {
        step(&mut state, &rules, &[]);
        let sim = Sim::new(&mut state, &rules);
        let frames = find(&sim, 0)
            .unwrap()
            .formation
            .as_ref()
            .unwrap()
            .frames
            .len();
        if frames == 0 && u.iter().all(|&m| walk(&sim, m).unwrap().state == 1) {
            done = true;
            break;
        }
    }
    let sim = Sim::new(&mut state, &rules);
    assert!(
        done,
        "march ends: {:?}",
        u.iter()
            .map(|&m| {
                let w = walk(&sim, m).unwrap();
                (m, w.state, w.high_path.len(), w.next_hp)
            })
            .collect::<Vec<_>>()
    );
    let g = find(&sim, 0).unwrap();
    for &m in &u {
        let o = sim.object(m).unwrap();
        let t = g.targets.get(m.0).unwrap();
        let d = crate::fx::fixed_hypot(o.grid.x.0 - t.0, o.grid.z.0 - t.1);
        assert!(d < 0x800, "{m:?} {d}");
    }
}

#[test]
fn members_running_into_a_blocked_frame_place_drop_out_and_a_rejoiner_stalls_the_march() {
    // K-259: the scene of `formation_scene` with the bar only at x = 30. Frame places lie in
    // the blocked cell (30, 25); the original does not check them.
    // 1. Whoever runs into it gets `0x1000` from state 5 -> `EnterFormationWalk` (`0x00535430`) ->
    //    `Leave` (`0x0042A730`): out of `ready`, state 4 with previous state 0xB; the rest advances
    //    without it (`AdvanceFrame` `0x0042A080`).
    // 2. A dropped-out member comes back via `Rejoin` (`0x0042A030`) while its HighPath still contains
    //    places of discarded frames (nothing clears them), walks in state 5 to one of them and
    //    lands there in 0xB **in `ready`** (`OnFormationHighPointReached` `0x00538180` without `Leave`).
    // 3. After that it never reaches its place exactly: the chase point (`ChaseSlot(id, 1)`) lies
    //    next to the place, after reaching it `TickFormationChase(1)` returns near with `Rejoin == 0`
    //    without a projection, the trajectory to the stale place at the wall - an endless loop. The
    //    others wait forever in state 10 (like K-258).
    let (mut state, rules, u) = formation_scene_with_bar(30..=30);
    let mut left = Vec::new();
    let mut frames_after_leave = None;
    let mut stale_rejoin = false;
    let mut misarrival_in_ready = false;
    let mut prev: Vec<i32> = vec![0; u.len()];
    for i in 0..3000 {
        step(&mut state, &rules, &formation_orders(i, &u));
        let sim = Sim::new(&mut state, &rules);
        let g = find(&sim, 0).unwrap();
        let f = g.formation.as_ref().unwrap();
        for (k, &m) in u[..6].iter().enumerate() {
            let w = walk(&sim, m).unwrap();
            if i > SECOND_ORDER_AT && prev[k] == 5 && w.state == 4 && w.restore_state == 0xB {
                assert!(!f.ready.contains(&m), "{m:?} leaves `ready`");
                if left.is_empty() {
                    frames_after_leave = Some(f.frames.len());
                }
                left.push(m);
            }
            if left.contains(&m) && prev[k] == 0xB && w.state == 5 {
                // `Rejoin`: more waypoints than open frames -> places of discarded frames.
                stale_rejoin |= w.high_path.len() > f.frames.len();
            }
            if stale_rejoin && prev[k] == 5 && w.state == 0xB && f.ready.contains(&m) {
                misarrival_in_ready = true;
            }
            prev[k] = w.state;
        }
    }
    // Four members drop out at the wall, the rest advances without them.
    assert_eq!(left, vec![u[5], u[4], u[2], u[1]]);
    let sim = Sim::new(&mut state, &rules);
    let g = find(&sim, 0).unwrap();
    let f = g.formation.as_ref().unwrap();
    assert!(f.frames.len() < frames_after_leave.unwrap());
    assert!(stale_rejoin && misarrival_in_ready);
    // State after 3000 ticks: two aborted at the wall (outside `ready`, no effect on the
    // frame), the returner u[1] in 0xB in `ready`, the others wait in the last frame in 10.
    assert_eq!(f.frames.len(), 1, "last frame stays open");
    for m in [u[2], u[5]] {
        assert_eq!(walk(&sim, m).unwrap().state, 1);
        assert!(!f.ready.contains(&m));
    }
    assert_eq!(f.ready.len(), 4);
    assert_eq!(f.arrived, 3);
    for &m in &f.ready {
        let w = walk(&sim, m).unwrap();
        assert_eq!(w.state, if m == u[1] { 0xB } else { 10 }, "{m:?}");
    }
    let slot = f.frames.front().unwrap().get(u[1].0).copied().unwrap();
    let w = walk(&sim, u[1]).unwrap();
    assert!(
        w.high_path
            .iter()
            .any(|h| crate::fx::fixed_hypot(h.x - slot.0, h.z - slot.1) > 8 << 12),
        "stale place at the wall in the HighPath"
    );
    // The loop stays: u[1] never reaches its place, nobody advances the frame.
    for _ in 0..300 {
        step(&mut state, &rules, &[]);
        let sim = Sim::new(&mut state, &rules);
        let f = find(&sim, 0).unwrap().formation.clone().unwrap();
        assert_eq!((f.frames.len(), f.arrived), (1, 3));
        assert_eq!(walk(&sim, u[1]).unwrap().state, 0xB);
    }
}

/// Scene for a destroyed member: units and formation number of the command.
type DestroyScene = fn(&mut SimState, &Rules) -> (Vec<ObjectId>, i32);

/// Line of 4 infantrymen in the march (`marching_line`).
fn destroy_in_march(state: &mut SimState, rules: &Rules) -> (Vec<ObjectId>, i32) {
    (marching_line(state, rules, (40, 20)), 1)
}

/// Line of 3 infantrymen and two warriors in the rear group, on the way to the formation setup (with only
/// one warrior it would walk as a single case without flag 8 and leave, `handle_single_member`).
fn destroy_in_rear(state: &mut SimState, rules: &Rules) -> (Vec<ObjectId>, i32) {
    let mut sim = Sim::new(state, rules);
    let u = line_of(&mut sim, 3, 0, 2);
    execute(&mut sim, &mv(&u, (30, 30), 1, NO_DIRECTION), true);
    let rear = &find(&sim, 0)
        .unwrap()
        .formation
        .as_ref()
        .unwrap()
        .rear_members;
    assert!(rear.len() == 2 && u[3..].iter().all(|m| rear.contains(m)));
    (u, 1)
}

/// Type 0 group of two infantrymen.
fn destroy_in_type_0(state: &mut SimState, rules: &Rules) -> (Vec<ObjectId>, i32) {
    let mut sim = Sim::new(state, rules);
    let u = vec![
        spawn(&mut sim, INFANTRY, (10, 10)),
        spawn(&mut sim, INFANTRY, (11, 10)),
    ];
    execute(&mut sim, &mv(&u, (30, 30), 0, NO_DIRECTION), true);
    (u, 0)
}

const DESTROY_AT: usize = 40;
const REORDER_AT: usize = 300;

/// Checksum per tick: in tick `DESTROY_AT` `u[victim]` is removed via `destroy_object` (without
/// leaving, like the carcass of a canoe, K-261), in `REORDER_AT` the rest get a new
/// command. With `save_at = Some(i)` it is saved and loaded after tick `i`.
fn destroyed_member_run(
    scene: DestroyScene,
    victim: usize,
    save_at: Option<usize>,
    at_save: Probe,
) -> Vec<u64> {
    let rules = rules();
    let mut state = state(&rules);
    let (u, kind) = scene(&mut state, &rules);
    let alive: Vec<_> = u.iter().copied().filter(|&m| m != u[victim]).collect();
    let mut out = Vec::new();
    for i in 0..600 {
        if i == DESTROY_AT {
            let mut sim = Sim::new(&mut state, &rules);
            crate::world::destroy_object(&mut sim, u[victim]);
        }
        let orders = if i == REORDER_AT {
            vec![mv(&alive, (40, 40), kind, NO_DIRECTION)]
        } else {
            vec![]
        };
        step(&mut state, &rules, &orders);
        if save_at == Some(i) {
            at_save(&mut state, &rules, &u);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

/// A group lists a member whose object is missing (main or rear group).
fn lists_a_destroyed_member(s: &mut SimState, r: &Rules, _: &[ObjectId]) {
    let sim = Sim::new(s, r);
    let listed = sim.state.groups.map.iter().any(|(_, g)| {
        let rear = g.formation.iter().flat_map(|f| f.rear_members.iter());
        g.members
            .iter()
            .chain(rear)
            .any(|&m| sim.object(m).is_none())
    });
    assert!(listed, "destroyed member stays listed");
}

#[test]
fn a_destroyed_member_stays_listed_and_the_save_loads_and_continues() {
    // K-261: `destroy_object` does not leave the group; the savegame loads anyway and continues
    // bit-identically - in the main group (in the march, type 0) and in the rear group.
    let scenes: [(&str, DestroyScene, usize); 3] = [
        ("line in the march", destroy_in_march, 2),
        ("rear group", destroy_in_rear, 3),
        ("type 0", destroy_in_type_0, 0),
    ];
    for (name, scene, victim) in scenes {
        let reference = destroyed_member_run(scene, victim, None, |_, _, _| {});
        for at in [DESTROY_AT, DESTROY_AT + 100, REORDER_AT - 1] {
            let seq = destroyed_member_run(scene, victim, Some(at), lists_a_destroyed_member);
            assert_eq!(seq, reference, "{name}: saving/loading in tick {at}");
        }
        // After the new command to the rest (the dead ID can stay in the old group).
        for at in [REORDER_AT + 1, 590] {
            let seq = destroyed_member_run(scene, victim, Some(at), |_, _, _| {});
            assert_eq!(seq, reference, "{name}: saving/loading in tick {at}");
        }
    }
    // The rest are no longer the same members (the dead ID counts) -> new group; the old one keeps
    // only the dead ID and is never deleted (`member_count` > 0).
    destroyed_member_run(destroy_in_march, 2, Some(590), |s, r, _| {
        let sim = Sim::new(s, r);
        assert!(sim.state.groups.map.iter().any(|(_, g)| {
            !g.members.is_empty() && g.members.iter().all(|&m| sim.object(m).is_none())
        }));
    });
}

#[test]
fn a_second_move_to_mounted_riders_realigns_the_group_of_their_horses() {
    // re/riding 13.2: `IsMember` maps riders to the carrier -> the same command to the same
    // riders realigns the group of the horses (same ID, no new one).
    use crate::module::riding::testkit as rk;
    let rules = rk::rules();
    let mut state = rk::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let mut riders = Vec::new();
    let mut horses = Vec::new();
    for x in [10, 16] {
        let r = rk::spawn(&mut sim, rk::COLONIST, 0, (x, 10));
        let h = rk::spawn(&mut sim, rk::HORSE, 0, (x, 10));
        rk::link(&mut sim, r, h);
        riders.push(r);
        horses.push(h);
    }
    let outsider = rk::spawn(&mut sim, rk::COLONIST, 0, (30, 30));
    execute(&mut sim, &mv(&riders, (30, 20), 0, NO_DIRECTION), true);
    assert_eq!(sim.state.groups.next_id, 1);
    let g = find(&sim, 0).unwrap();
    assert!(horses.iter().all(|h| g.members.contains(h)));
    assert!(riders.iter().all(|r| !g.members.contains(r)));
    assert!(riders.iter().all(|&r| is_member(&sim, 0, r)));
    assert!(horses.iter().all(|&h| is_member(&sim, 0, h)));
    assert!(!is_member(&sim, 0, outsider));
    execute(&mut sim, &mv(&riders, (40, 30), 0, NO_DIRECTION), true);
    assert_eq!(sim.state.groups.next_id, 1, "no new group");
    assert_eq!(find(&sim, 0).unwrap().members.len(), 2);
}

/// Two canoes with occupants; a go command to the occupants forms the group of the canoes
/// (`is_member` maps occupants to the carrier), in tick `SINK_AT` the first canoe sinks.
/// Checksums per tick; with `save_at = Some(i)` save and load after tick `i`.
fn sunk_canoe_run(save_at: Option<usize>) -> Vec<u64> {
    use crate::module::riding::testkit as rk;
    const SINK_AT: usize = 5;
    let rules = rk::rules();
    let mut state = rk::state(&rules);
    let (passengers, canoes) = {
        let mut sim = Sim::new(&mut state, &rules);
        let mut passengers = Vec::new();
        let mut canoes = Vec::new();
        for x in [10, 16] {
            let w = rk::spawn(&mut sim, rk::WARRIOR, 0, (x, 10));
            let c = rk::spawn(&mut sim, rk::CANOE, 0, (x, 10));
            rk::link(&mut sim, w, c);
            passengers.push(w);
            canoes.push(c);
        }
        (passengers, canoes)
    };
    let mut out = Vec::new();
    for i in 0..50 {
        let orders = if i == 0 {
            vec![mv(&passengers, (30, 20), 0, NO_DIRECTION)]
        } else {
            vec![]
        };
        if i == SINK_AT {
            let mut sim = Sim::new(&mut state, &rules);
            crate::messaging::send(
                &mut sim,
                canoes[0],
                &crate::message::Message::new(0x110, &[]),
            );
        }
        step(&mut state, &rules, &orders);
        if i == 0 {
            let sim = Sim::new(&mut state, &rules);
            assert!(
                canoes
                    .iter()
                    .all(|c| find(&sim, 0).unwrap().members.contains(c))
            );
        }
        if i > SINK_AT + 2 {
            let sim = Sim::new(&mut state, &rules);
            assert!(sim.object(canoes[0]).is_none(), "canoe destroyed");
            assert!(
                find(&sim, 0).unwrap().members.contains(&canoes[0]),
                "sunk canoe stays a member (K-261)"
            );
        }
        if save_at == Some(i) {
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

#[test]
fn a_sunk_canoe_stays_in_its_group_and_the_save_loads() {
    // K-261 with a real canoe death (re/riding 13.3: no leaving on destruction): two canoes with
    // occupants, one sinks -> its ID stays in `members`; saving/loading succeeds and continues
    // bit-identically (checksums over 50 ticks equal with and without loading).
    let reference = sunk_canoe_run(None);
    for at in [3, 6, 10, 30] {
        assert_eq!(
            sunk_canoe_run(Some(at)),
            reference,
            "saving/loading in tick {at}"
        );
    }
}
