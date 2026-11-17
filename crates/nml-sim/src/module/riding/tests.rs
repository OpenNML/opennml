//! Tests of state, parameters and animation queries of modules 13/14 (re/riding 1, 2.3).

use super::testkit::*;
use super::*;
use crate::command::{Command, CommandKind};
use crate::fx::fixed_mul;
use crate::group::testkit::mv;
use crate::message::Message;
use crate::messaging::{dispatch_command, post_command, send};
use crate::module::walk::{self, find_to_msg, walk_mut};
use crate::motion::NO_DIRECTION;
use crate::motion::speed::TIME_UNIT;
use crate::rules::Rules;
use crate::save::testkit::save_and_load;
use crate::state::SimState;
use crate::tick::step;

/// Tile center as grid coordinate.
fn center(t: i32) -> i32 {
    (t << 12) + 0x800
}

/// FINDTO (`0x200` walk, `0x100` run) to tile `to` into the FIFO, without feedback.
fn go(sim: &mut Sim, unit: ObjectId, to: (i32, i32), run: bool) {
    let msg = find_to_msg(center(to.0), center(to.1), -1, NO_DIRECTION, 0, run, 0x201);
    post_command(sim, unit, Envelope::bare(msg), true);
}

/// Rider and horse on the same tile, linked; `(rider, mount)`.
fn rider_on_horse(sim: &mut Sim, rider: u32, tile: (i32, i32)) -> (ObjectId, ObjectId) {
    let r = spawn(sim, rider, 0, tile);
    let h = spawn(sim, HORSE, 0, tile);
    link(sim, r, h);
    (r, h)
}

/// Position (x, z, direction) of the object.
fn pose(state: &SimState, id: ObjectId) -> (i32, i32, i32) {
    let o = state.object(id, false).expect("object present");
    (o.grid.x.0, o.grid.z.0, o.direction.0)
}

#[test]
fn riding_reads_its_lists_and_flags_and_the_boat_init_reads_150xxx() {
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let d = spawn(&mut sim, DRAGOON, 0, (10, 10));
    let r = riding(&sim, d, RIDING).unwrap();
    assert!(!r.boat);
    assert_eq!(r.anims[list::WALK], vec![6]);
    assert_eq!(r.anims[list::DIE], vec![3]);
    assert_eq!(
        (r.next_mount_anim, r.lance_available, r.mount_target),
        (-1, true, ObjectId::NONE)
    );
    let w = spawn(&mut sim, WARRIOR, 0, (12, 10));
    let b = riding(&sim, w, RIDING_BOAT).unwrap();
    assert!(b.boat);
    assert_eq!(b.anims[list::WALK], vec![19]);
    assert_eq!(b.anims[list::STAND], vec![18]);
    assert_eq!(b.anims[list::DIE], vec![20]);
    assert!(b.anims[list::RUN].is_empty() && b.anims[list::HORSE_FIGHT].is_empty());
    assert_eq!(sim.module(w, RIDING_BOAT).unwrap().base.flags, 0); // 150001 not read
}

#[test]
fn getanim_maps_kinds_returns_0_for_unknown_kinds_and_minus_1_for_empty_lists() {
    // re/riding 9 T11; K-228; deviation from the original (empty list -> -1 instead of division by 0).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let d = spawn(&mut sim, DRAGOON, 0, (10, 10));
    assert_eq!(send(&mut sim, d, &Message::new(0x50D, &[8])), 3);
    assert_eq!(send(&mut sim, d, &Message::new(0x50D, &[6, 3])), 6); // 140003[3 % 1]
    assert_eq!(send(&mut sim, d, &Message::new(0x50D, &[11])), 0);
    assert_eq!(send(&mut sim, d, &Message::new(0x50D, &[10])), -1);
    let w = spawn(&mut sim, WARRIOR, 0, (12, 10));
    assert_eq!(send(&mut sim, w, &Message::new(0x50E, &[7, 0])), 18);
    assert_eq!(send(&mut sim, w, &Message::new(0x50E, &[6])), -1);
    assert_eq!(send(&mut sim, w, &Message::new(0x50D, &[7])), -1); // Warrior has no module 13
}

#[test]
fn the_lance_rule_and_the_variant_switch_follow_read_params() {
    // re/riding 1.1 (lance rule) and 13.6 (K-71: both modules read 140001...140015).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let d = spawn(&mut sim, DRAGOON, 0, (10, 10));
    let w = spawn(&mut sim, WARRIOR, 0, (12, 10));
    let lance = nml_formats::property::ModuleDesc {
        index: 13,
        mask: u32::MAX,
        values: [(140_001, "256".to_string()), (140_002, "9 8".to_string())].into(),
    };
    read_params(&mut sim, d, RIDING, &lance);
    let m = sim.module(d, RIDING).unwrap();
    assert_eq!(m.base.flags, HAS_LANCE | LANCE_AVAILABLE);
    assert_eq!(riding(&sim, d, RIDING).unwrap().anims[list::WALK], [9, 8]);
    // Without lance readiness (an earlier throw) the flag stays off.
    riding_mut(&mut sim, d, RIDING).unwrap().lance_available = false;
    sim.module_mut(d, RIDING).unwrap().base.flags = 0;
    read_params(&mut sim, d, RIDING, &lance);
    assert_eq!(sim.module(d, RIDING).unwrap().base.flags, HAS_LANCE);
    // The boat descriptor does not have the values: flags 0, lists stay (K-71).
    let boat = nml_formats::property::ModuleDesc {
        index: 14,
        mask: u32::MAX,
        values: Default::default(),
    };
    sim.module_mut(w, RIDING_BOAT).unwrap().base.flags = 0x300;
    read_params(&mut sim, w, RIDING_BOAT, &boat);
    assert_eq!(sim.module(w, RIDING_BOAT).unwrap().base.flags, 0);
    assert_eq!(
        riding(&sim, w, RIDING_BOAT).unwrap().anims[list::WALK],
        [19]
    );
}

#[test]
fn constants_follow_the_module_and_on_message_keeps_the_replies_of_mount_and_dismount() {
    assert_eq!(
        (
            Riding::new(false).status_bit(),
            Riding::new(true).status_bit()
        ),
        (0x2000, 0x8000)
    );
    let (r, b) = (Riding::new(false), Riding::new(true));
    assert_eq!((r.fault_msg(), r.success_msg()), (0x30D, 0x40D));
    assert_eq!((b.fault_msg(), b.success_msg()), (0x30E, 0x40E));
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let d = spawn(&mut sim, DRAGOON, 0, (10, 10));
    let env = Envelope {
        msg: Message::new(0x10D, &[1]),
        success: Some(Message::new(0x40D, &[1])),
        fault: Some(Message::new(0x30D, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    assert!(crate::module::on_message(&mut sim, d, RIDING, &env));
    let base = &sim.module(d, RIDING).unwrap().base;
    assert_eq!(base.success, env.success);
    assert_eq!(base.fault, env.fault);
    // ID 1 is the Dragoon itself: the approach ends synchronously, DoMount fails as without a
    // target (K-269, otherwise it would carry itself) and sets the search timer; after that
    // MountStart sets `going_to_horse` without an exemption zone (K-267).
    assert_eq!(d, ObjectId(1));
    assert_eq!(base.timers_ws.front().map(|e| e.msg.code), Some(0xA0D));
    assert_eq!(
        sim.object(d).unwrap().status & (status::ISMOUNTED | 0x1000),
        0
    );
    assert!(riding(&sim, d, RIDING).unwrap().going_to_horse);
    // Another code (lance, deviation from the original) is accepted and leaves the messages as they are.
    let lance = Envelope::bare(Message::new(0xC0D, &[1]));
    assert!(crate::module::on_message(&mut sim, d, RIDING, &lance));
    assert_eq!(sim.module(d, RIDING).unwrap().base.success, env.success);
}

#[test]
fn a_mounted_rider_moves_only_with_its_mount() {
    // re/walk 2.1, re/riding 4.1: walk command to the rider -> the horse plans; the rider's walk
    // stays idle (state 1 without a path), its position follows that of the horse (rider-mount link).
    let rules = rules();
    let mut state = state(&rules);
    let (r, h) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (r, h) = rider_on_horse(&mut sim, COLONIST, (10, 10));
        go(&mut sim, r, (20, 10), false);
        (r, h)
    };
    let start = pose(&state, h);
    let mut horse_planned = false;
    for takt in 0..40 {
        step(&mut state, &rules, &[]);
        step(&mut state, &rules, &[]);
        assert_eq!(
            pose(&state, r),
            pose(&state, h),
            "tick {takt}: rider follows the horse"
        );
        let sim = Sim::new(&mut state, &rules);
        let rw = walk::walk(&sim, r).unwrap();
        assert_eq!(
            (rw.state, rw.high_path.len(), rw.trajectory.len()),
            (1, 0, 0),
            "tick {takt}: the rider never plans itself"
        );
        let hw = walk::walk(&sim, h).unwrap();
        horse_planned |= hw.state != 1 || !hw.high_path.is_empty() || !hw.trajectory.is_empty();
    }
    assert!(horse_planned, "the horse received the command");
    assert!(pose(&state, h).0 > start.0, "the horse has walked");
    assert_eq!(pose(&state, h).1, start.1);
}

#[test]
fn queries_reach_the_mount_except_0x2000_and_0x2100() {
    // re/walk 2.1, re/riding 4.1; K-231: 0x1A00 to the rider selects the run animation of the horse;
    // 0x2100 takes effect on the rider and on the horse.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (r, h) = rider_on_horse(&mut sim, COLONIST, (10, 10));
    // 0xD00: speed of the horse (227), not the own one of the colonist (183).
    walk_mut(&mut sim, h).unwrap().speed.cur = 227;
    walk_mut(&mut sim, r).unwrap().speed.cur = 183;
    let own = walk::walk(&sim, r).unwrap().speed.cur;
    let mount = walk::walk(&sim, h).unwrap().speed.cur;
    assert_ne!(own, mount);
    assert_eq!(send(&mut sim, r, &Message::new(0xD00, &[])), mount);
    // 0x1400 on the canoe occupant: layer of the canoe (6), not its own (1).
    let w = spawn(&mut sim, WARRIOR, 0, (14, 10));
    let c = spawn(&mut sim, CANOE, 0, (14, 10));
    link(&mut sim, w, c);
    assert_eq!(send(&mut sim, w, &Message::new(0x1400, &[])), 6);
    assert_eq!(walk::walk(&sim, w).unwrap().layer, 1);
    // 0x2000: the own rank number (rider 10012, occupant 10011), never from the mount.
    {
        let wk = walk_mut(&mut sim, r).unwrap();
        (wk.formation_num, wk.formation_num_mounted) = (4, 1);
        let wk = walk_mut(&mut sim, w).unwrap();
        (wk.formation_num, wk.formation_num_mounted) = (4, 1);
        let wk = walk_mut(&mut sim, h).unwrap();
        (wk.formation_num, wk.formation_num_mounted) = (7, 7);
    }
    assert_eq!(send(&mut sim, r, &Message::new(0x2000, &[])), 1);
    assert_eq!(send(&mut sim, w, &Message::new(0x2000, &[])), 4);
    // 0x2100 {1, 0, -1, 0, 0}: behavior on the rider AND on the horse.
    for id in [r, h] {
        let wk = walk_mut(&mut sim, id).unwrap();
        wk.mobile_home = true;
        wk.tether = 5000;
    }
    send(&mut sim, r, &Message::new(0x2100, &[1, 0, -1, 0, 0]));
    for id in [r, h] {
        let wk = walk::walk(&sim, id).unwrap();
        assert!(!wk.mobile_home && wk.tether == -1, "{id:?}");
    }
    // 0x1A00: the horse selects its run animation (10001 = 3), the rider does not (K-231).
    sim.object_mut(h).unwrap().anim.wanted = -1;
    sim.object_mut(r).unwrap().anim.wanted = -1;
    send(&mut sim, r, &Message::new(0x1A00, &[]));
    assert_eq!(sim.object(h).unwrap().anim.wanted, 3);
    assert_eq!(sim.object(r).unwrap().anim.wanted, -1);
}

#[test]
fn without_a_mount_on_message_clears_the_rider_bits_and_handles_it_itself() {
    // re/walk 2.1: rider bit without a target -> 0x2000/0x8000 off, the rider plans itself; a query
    // without a target clears nothing.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let r = spawn(&mut sim, COLONIST, 0, (10, 10));
    let bits = status::ISMOUNTED | status::ISMOUNTEDONBOAT;
    sim.object_mut(r).unwrap().set_status_flags(bits, true);
    assert_eq!(walk::mount_of(&sim, r), None);
    let cur = walk::walk(&sim, r).unwrap().speed.cur;
    assert_eq!(send(&mut sim, r, &Message::new(0xD00, &[])), cur);
    assert_eq!(
        sim.object(r).unwrap().status & bits,
        bits,
        "query clears nothing"
    );
    // FINDTO via dispatch_command: bits off, the rider plans itself.
    let msg = find_to_msg(center(20), center(10), -1, NO_DIRECTION, 0, false, 0x201);
    assert_eq!(dispatch_command(&mut sim, r, &Envelope::bare(msg)), 1);
    assert_eq!(sim.object(r).unwrap().status & bits, 0);
    let w = walk::walk(&sim, r).unwrap();
    assert!(
        w.state != 1 || !w.high_path.is_empty(),
        "the rider plans itself"
    );
    // Target 0 and a vanished target count as missing.
    let r2 = spawn(&mut sim, COLONIST, 0, (12, 12));
    sim.object_mut(r2)
        .unwrap()
        .set_status_flags(status::ISMOUNTED, true);
    sim.object_mut(r2).unwrap().target = ObjectId(0);
    assert_eq!(walk::mount_of(&sim, r2), None);
    let h = spawn(&mut sim, HORSE, 0, (12, 12));
    link(&mut sim, r2, h);
    assert_eq!(walk::mount_of(&sim, r2), Some(h));
    sim.object_mut(r2).unwrap().target = ObjectId::from_i32(0x7FFF_FF01);
    assert_eq!(walk::mount_of(&sim, r2), None);
}

#[test]
fn a_mounted_rider_takes_the_walk_animation_from_riding_with_the_mounts_tempo() {
    // re/riding 4.1: rider animation 0x50D {5} (walk) or {6} (horse runs), speed from the
    // horse, no random start; canoe occupant 0x50E {5}. Test value: Colonist 140002 = 2 (walk),
    // 140003 = 27 (run), so that both branches are observable.
    let rules = rules();
    for (run, expected) in [(false, 2), (true, 27)] {
        let mut state = state(&rules);
        let (r, h) = {
            let mut sim = Sim::new(&mut state, &rules);
            let (r, h) = rider_on_horse(&mut sim, COLONIST, (10, 10));
            go(&mut sim, r, (30, 10), run);
            (r, h)
        };
        let mut seen = false;
        for _ in 0..60 {
            step(&mut state, &rules, &[]);
            let sim = Sim::new(&mut state, &rules);
            let (ro, ho) = (sim.object(r).unwrap(), sim.object(h).unwrap());
            if ho.status & status::MOVE != 0 && ho.status & 0x10 == 0 && ro.anim.wanted == expected
            {
                let hw = walk::walk(&sim, h).unwrap();
                assert_eq!(
                    hw.speed.running, run,
                    "the horse runs exactly in the run branch"
                );
                assert_eq!(ro.anim.speed, fixed_mul(hw.speed.cur, TIME_UNIT) as f32);
                assert_eq!(ro.anim.start_ms, 0, "no random start");
                seen = true;
                break;
            }
        }
        assert!(seen, "rider animation {expected} (run: {run}) not selected");
    }
    // Canoe occupant: 0x50E {5} = 150002 (19), speed from the canoe.
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let w = spawn(&mut sim, WARRIOR, 0, (14, 10));
    let c = spawn(&mut sim, CANOE, 0, (14, 10));
    link(&mut sim, w, c);
    sim.object_mut(c)
        .unwrap()
        .set_status_flags(status::MOVE, true);
    walk_mut(&mut sim, c).unwrap().speed.cur = 200;
    walk_mut(&mut sim, w).unwrap().speed.cur = 100;
    walk::frame_animation(&mut sim, w);
    let cur = walk::walk(&sim, c).unwrap().speed.cur;
    assert_ne!(cur, walk::walk(&sim, w).unwrap().speed.cur);
    let o = sim.object(w).unwrap();
    assert_eq!(o.anim.wanted, 19);
    assert_eq!(o.anim.speed, fixed_mul(cur, TIME_UNIT) as f32);
}

/// The mount animation is set to 7 (arrived), so that "without restart" and the new request can be
/// told apart.
fn prime_mount(sim: &mut Sim, id: ObjectId) {
    sim.object_mut(id)
        .unwrap()
        .set_animation_speed(7, 0, 64.0, false, true, false);
}

fn assert_mount_anim(sim: &Sim, id: ObjectId, wanted: i32) {
    let a = &sim.object(id).unwrap().anim;
    assert_eq!((a.wanted, a.current, a.speed), (wanted, 7, 128.0), "{id:?}");
}

#[test]
fn melee_and_aim_set_the_horse_animation_from_list_140011() {
    // C-72, re/combat-melee 2.1, re/combat-ranged 3.1/3.3: the mount animation comes from the
    // list 140011 of the **own** riding module, speed 128, without restart.
    let mut rules = rules();
    rules.set_anim_labels(DRAGOON, 5, &[(1, 1000), (2, 400)]);
    rules.set_anim_labels(WARRIOR, 7, &[(1, 1000), (2, 400)]);
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (d, h) = rider_on_horse(&mut sim, DRAGOON, (10, 10));
    let t = spawn(&mut sim, COLONIST, 1, (14, 10));
    let swing = |sim: &mut Sim, who: ObjectId| {
        let msg = Message::new(0x112, &[t.as_i32(), 0]);
        assert_eq!(dispatch_command(sim, who, &Envelope::bare(msg)), 1);
    };
    // Melee while mounted, list 140011 empty -> -1; with "9" -> 9.
    prime_mount(&mut sim, h);
    swing(&mut sim, d);
    assert_eq!(sim.object(d).unwrap().anim.wanted, 5, "rider: 140006");
    assert_mount_anim(&sim, h, -1);
    riding_mut(&mut sim, d, RIDING).unwrap().anims[list::HORSE_FIGHT] = vec![9];
    prime_mount(&mut sim, h);
    swing(&mut sim, d);
    assert_mount_anim(&sim, h, 9);
    // A mount without a rider stays untouched.
    let (d2, h2) = (
        spawn(&mut sim, DRAGOON, 0, (10, 14)),
        spawn(&mut sim, HORSE, 0, (10, 14)),
    );
    prime_mount(&mut sim, h2);
    swing(&mut sim, d2);
    let a = &sim.object(h2).unwrap().anim;
    assert_eq!((a.wanted, a.speed), (7, 64.0), "untouched");
    // In the boat, {10} also goes to module 13 of the occupant (missing -> -1).
    let w = spawn(&mut sim, WARRIOR, 0, (18, 10));
    let c = spawn(&mut sim, CANOE, 0, (18, 10));
    link(&mut sim, w, c);
    riding_mut(&mut sim, w, RIDING_BOAT).unwrap().anims[list::FIGHT_NEAR] = vec![7];
    assert_eq!(send(&mut sim, w, &Message::new(0x50D, &[10])), -1);
    prime_mount(&mut sim, c);
    swing(&mut sim, w);
    assert_eq!(sim.object(w).unwrap().anim.wanted, 7);
    assert_mount_anim(&sim, c, -1);
    // Aiming (arrow): only with ISMOUNTED does the horse get 0x50D {10}; shooter selects 140007.
    let aim = Message::new(0x10A, &[t.as_i32(), -1]);
    prime_mount(&mut sim, h);
    sim.object_mut(d).unwrap().anim.wanted = -1;
    assert_eq!(send(&mut sim, d, &aim), 0);
    assert_eq!(sim.object(d).unwrap().anim.wanted, 5);
    assert_mount_anim(&sim, h, 9);
    // Aiming (rifle, AimGun at the very beginning).
    sim.module_mut(d, crate::module::shoot::INDEX)
        .unwrap()
        .base
        .flags = crate::module::shoot::flag::GUN;
    prime_mount(&mut sim, h);
    assert_eq!(send(&mut sim, d, &aim), 0);
    assert_mount_anim(&sim, h, 9);
    // On the canoe (ISMOUNTEDONBOAT), aiming stays without a mount animation.
    let c2 = spawn(&mut sim, CANOE, 0, (22, 10));
    link(&mut sim, d2, c2);
    sim.module_mut(d2, crate::module::shoot::INDEX)
        .unwrap()
        .base
        .flags = crate::module::shoot::flag::ARROW;
    prime_mount(&mut sim, c2);
    assert_eq!(send(&mut sim, d2, &aim), 0);
    let a = &sim.object(c2).unwrap().anim;
    assert_eq!((a.wanted, a.speed), (7, 64.0), "boat untouched");
}

#[test]
fn a_mounted_shooter_takes_its_shoot_animation_from_riding() {
    // re/combat-ranged 2.3 (`GetAnim`): when mounted `0x50D {kind, selector}`, not
    // `{2, kind, selector}`, which would always give kind 2 (present).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (d, _) = rider_on_horse(&mut sim, DRAGOON, (10, 10));
    use crate::module::shoot::get_anim;
    assert_eq!(get_anim(&mut sim, d, 1, 0), 5, "shot: 140007");
    assert_eq!(get_anim(&mut sim, d, 1, 3), 5);
    assert_eq!(get_anim(&mut sim, d, 2, 0), -1, "present: 140008 empty");
    assert_eq!(get_anim(&mut sim, d, 3, 0), -1, "reload: 140009 empty");
    riding_mut(&mut sim, d, RIDING).unwrap().anims[list::PRESENT] = vec![11, 12];
    assert_eq!(get_anim(&mut sim, d, 2, 3), 12);
    // In the boat: `0x50E` (150006 = shot).
    let w = spawn(&mut sim, WARRIOR, 0, (18, 10));
    let c = spawn(&mut sim, CANOE, 0, (18, 10));
    link(&mut sim, w, c);
    assert_eq!(get_anim(&mut sim, w, 1, 0), -1);
    riding_mut(&mut sim, w, RIDING_BOAT).unwrap().anims[list::FIGHT_FAR] = vec![4];
    assert_eq!(get_anim(&mut sim, w, 1, 0), 4);
}

// --- Mounting and boarding (re/riding 3.1, 3.4, 3.6, 3.7) ---

/// One object tick of each object (two engine ticks).
fn object_tick(state: &mut SimState, rules: &crate::rules::Rules) {
    step(state, rules, &[]);
    step(state, rules, &[]);
}

/// Engine ticks until `done` holds (at most `limit`); returns the number of ticks.
fn steps_until(
    state: &mut SimState,
    rules: &crate::rules::Rules,
    limit: usize,
    done: impl Fn(&mut Sim) -> bool,
) -> usize {
    for n in 1..=limit {
        step(state, rules, &[]);
        if done(&mut Sim::new(state, rules)) {
            return n;
        }
    }
    panic!("condition not reached after {limit} engine ticks");
}

/// Link as after `DoMount`: bits, references on both sides, riding module points to the mount.
fn assert_linked(sim: &Sim, rider: ObjectId, mount: ObjectId, index: usize) {
    let (r, m) = (sim.object(rider).unwrap(), sim.object(mount).unwrap());
    let bit = riding(sim, rider, index).unwrap().status_bit();
    assert_eq!(
        r.status & (bit | 0x1000),
        bit,
        "rider bit, exemption zone off"
    );
    assert_ne!(m.status & status::CARRIING, 0, "mount carries");
    assert_eq!(
        (r.target, m.target),
        (mount, rider),
        "references on both sides"
    );
    let rd = riding(sim, rider, index).unwrap();
    assert_eq!(
        (
            rd.mount_target,
            rd.going_to_horse,
            rd.busy,
            rd.next_mount_anim
        ),
        (mount, false, false, -1)
    );
    assert_eq!(
        r.exempt.object,
        ObjectId(0),
        "exemption object 0 instead of -1 (K-225)"
    );
    assert_eq!(
        (r.grid.x, r.grid.z),
        (m.grid.x, m.grid.z),
        "rider at the position"
    );
    assert_eq!(r.direction, m.direction);
}

/// Sight with mounty bonus (32 + 20 % = 38); the modifier carries the key `0x7E`: replacing it with
/// 0 % restores 32.
fn assert_mounty_bonus(sim: &mut Sim, id: ObjectId) {
    assert_eq!(sim.attribute(id, 18), 38, "sight with mounty bonus");
    let o = sim.object_mut(id).unwrap();
    let mut probe = o.attributes.clone();
    probe.set_modifier(18, 0, true, 0x7E);
    std::mem::swap(&mut o.attributes, &mut probe);
    assert_eq!(sim.attribute(id, 18), 32, "key 0x7E");
    let o = sim.object_mut(id).unwrap();
    std::mem::swap(&mut o.attributes, &mut probe);
}

#[test]
fn a_translated_dragoon_is_created_mounted() {
    // re/riding 9 T1: create_object 2008 -> rider 2007 + horse 3410 at the same position, synchronously
    // linked (`0x40D` from `create_object`), mounty bonus, stand animation 140004[0] = 7.
    use crate::command::testkit::world;
    use crate::world::{ObjectDesc, create_object, desc_flags};
    let mut rules = rules();
    // Idle only sets animations with time marker 1 (stand.rs `update_idle_anim`).
    rules.set_anim_labels(DRAGOON, 7, &[(1, 1000)]);
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let mut desc = ObjectDesc::new(DRAGOON_ON_HORSE, 0, [world(10), 0.0, world(10)]);
    desc.flags |= desc_flags::PLACED;
    let r = create_object(&mut sim, &mut desc).unwrap();
    let h = sim.object(r).unwrap().target;
    assert_eq!(sim.object(r).unwrap().guid, DRAGOON);
    assert_eq!(sim.object(h).map(|o| o.guid), Some(HORSE));
    assert_linked(&sim, r, h, RIDING);
    assert_mounty_bonus(&mut sim, r);
    assert_eq!(sim.object(r).unwrap().anim.wanted, 7, "stand 0x50D {{7}}");
    // The horse follows (`0x2100 {1, h, ...}`).
    assert!(walk::walk(&sim, h).unwrap().try_to_follow);
}

/// Colonist (10, 10) with 10 gold load and horse (14, 10) of slot `slot`; post
/// `{0x10D, [horse]}` with abort. Returns `(rider, horse)`.
fn colonist_and_horse(sim: &mut Sim, slot: i32) -> (ObjectId, ObjectId) {
    let r = spawn(sim, COLONIST, 0, (10, 10));
    let h = spawn(sim, HORSE, slot, (14, 10));
    crate::module::gather::set_carried_load(sim, r, 10, 1);
    let msg = Message::new(0x10D, &[h.as_i32()]);
    assert!(crate::messaging::post_message(sim, r, msg, true));
    (r, h)
}

fn mounted(sim: &Sim, r: ObjectId) -> bool {
    sim.object(r)
        .is_some_and(|o| o.status & status::ISMOUNTED != 0)
}

#[test]
fn a_colonist_walks_to_its_own_horse_and_mounts_on_arrival() {
    // re/riding 9 T3: approach via FINDTO_UNIT, arrival `0x40D` -> link, load gone (W-205),
    // stand animation 15, no timer 0x70D (K-223).
    let mut rules = rules();
    rules.set_anim_labels(COLONIST, 15, &[(1, 1000)]);
    let mut state = state(&rules);
    let (r, h) = colonist_and_horse(&mut Sim::new(&mut state, &rules), 0);
    // First engine tick: the colonist (group 0) starts the approach.
    step(&mut state, &rules, &[]);
    {
        let sim = Sim::new(&mut state, &rules);
        let o = sim.object(r).unwrap();
        assert_ne!(o.status & 0x1000, 0, "exemption zone");
        assert_eq!(o.exempt.object, h);
        assert!(riding(&sim, r, RIDING).unwrap().going_to_horse);
        assert_eq!(riding(&sim, r, RIDING).unwrap().mount_target, h);
        let w = walk::walk(&sim, r).unwrap();
        assert_eq!(w.quarry, h, "FINDTO_UNIT on the horse");
        assert_eq!((w.min_attack_radius, w.max_attack_radius), (0, 2048));
        let base = &sim.module(r, walk::INDEX).unwrap().base;
        assert_eq!(base.success, Some(Message::new(0x40D, &[h.as_i32()])));
        assert_eq!(base.fault, Some(Message::new(0x30D, &[])));
        assert!(!mounted(&sim, r));
    }
    steps_until(&mut state, &rules, 400, |sim| mounted(sim, r));
    let mut sim = Sim::new(&mut state, &rules);
    assert_linked(&sim, r, h, RIDING);
    assert_mounty_bonus(&mut sim, r);
    let o = sim.object(r).unwrap();
    assert_eq!((o.load_amount, o.load_kind), (0, -1), "load gone (W-205)");
    assert_eq!(o.anim.wanted, 15, "stand 140004[0]");
    let m = &sim.module(r, RIDING).unwrap().base;
    assert!(
        m.events.is_empty() && m.timers_ws.is_empty(),
        "no timer 0x70D (K-223)"
    );
}

#[test]
fn mounting_a_foreign_horse_takes_it_over_one_engine_half_later() {
    // re/riding 9 T4, K-224: arrival -> `0x47F {0, R, 0x40D}` to the horse; the change of owner in phase 0
    // calls `0x40D {new}` back -> link with the new ID, one engine half after T3.
    let rules = rules();
    let mut own = state(&rules);
    let (r0, _) = colonist_and_horse(&mut Sim::new(&mut own, &rules), 0);
    let t3 = steps_until(&mut own, &rules, 400, |sim| mounted(sim, r0));
    let mut state = state(&rules);
    let (r, h) = colonist_and_horse(&mut Sim::new(&mut state, &rules), 1);
    let arrival = steps_until(&mut state, &rules, 400, |sim| {
        sim.object(h).is_some_and(|o| o.owner_change_target == r)
    });
    assert_eq!(arrival, t3, "arrival as in T3");
    {
        let sim = Sim::new(&mut state, &rules);
        let o = sim.object(h).unwrap();
        assert_eq!(o.owner_change_msg, 0x40D);
        assert_eq!(sim.state.owner_changes, [(h, 0)]);
        assert!(!mounted(&sim, r), "no link yet");
        assert!(riding(&sim, r, RIDING).unwrap().going_to_horse);
    }
    step(&mut state, &rules, &[]);
    let mut sim = Sim::new(&mut state, &rules);
    assert!(sim.object(h).is_none(), "old ID no longer valid");
    let new = sim.object(r).unwrap().target;
    assert_eq!(new.slot(), 0);
    assert_eq!(sim.object(new).map(|o| o.guid), Some(HORSE));
    assert_linked(&sim, r, new, RIDING);
    assert_mounty_bonus(&mut sim, r);
}

#[test]
fn a_horse_taken_by_another_rider_starts_the_search() {
    // re/riding 9 T5, K-226: target already carries -> DoMount -1, timer 0xA0D (100 ms, group 0x80)
    // in module 13; in the 2nd `update_events` after the takeover the rider searches within the radius
    // `attr18 << 10` for a free SADDLE animal (any owner).
    let rules = rules();
    for second in [true, false] {
        let mut state = state(&rules);
        let (r, h, h2) = {
            let mut sim = Sim::new(&mut state, &rules);
            let r = spawn(&mut sim, COLONIST, 0, (10, 10));
            let h = spawn(&mut sim, HORSE, 0, (14, 10));
            let other = spawn(&mut sim, COLONIST, 0, (14, 10));
            link(&mut sim, other, h);
            // Foreign horse 3 tiles beside the target (within sight radius 8).
            let h2 = second.then(|| spawn(&mut sim, HORSE, 2, (14, 13)));
            // Failure message of the envelope: 0xA7F {77} sets the carrier (observable).
            let env = Envelope {
                msg: Message::new(0x10D, &[h.as_i32()]),
                success: None,
                fault: Some(Message::new(0xA7F, &[77])),
                success_target: ObjectId::NONE,
                fault_target: ObjectId::NONE,
            };
            assert!(crate::messaging::post_command(&mut sim, r, env, true));
            (r, h, h2)
        };
        let timer = |sim: &Sim| {
            let b = &sim.module(r, RIDING).unwrap().base;
            b.events
                .iter()
                .chain(b.timers_ws.iter())
                .find(|e| e.msg.code == 0xA0D)
                .cloned()
        };
        steps_until(&mut state, &rules, 400, |sim| timer(sim).is_some());
        {
            let sim = Sim::new(&mut state, &rules);
            let e = timer(&sim).unwrap();
            assert_eq!(
                (e.b, e.group(), e.flags & 2),
                (100, 0x80, 0),
                "100 ms, once"
            );
            let o = sim.object(r).unwrap();
            assert_eq!(o.status & (status::ISMOUNTED | 0x1000), 0);
            assert_eq!(o.target, ObjectId::NONE, "no link");
            assert!(!riding(&sim, r, RIDING).unwrap().going_to_horse);
        }
        // Takeover in the same object tick; the next `update_events` counts 100 (not > 100).
        object_tick(&mut state, &rules);
        assert!(timer(&Sim::new(&mut state, &rules)).is_some(), "not yet");
        object_tick(&mut state, &rules);
        let sim = Sim::new(&mut state, &rules);
        assert!(timer(&sim).is_none(), "fired");
        let rd = riding(&sim, r, RIDING).unwrap();
        let o = sim.object(r).unwrap();
        match h2 {
            Some(h2) => {
                assert_eq!((rd.mount_target, rd.going_to_horse), (h2, true));
                assert_eq!((o.exempt.object, o.status & 0x1000), (h2, 0x1000));
                assert_ne!(o.carrier, ObjectId(77));
            }
            None => {
                assert_eq!((rd.mount_target, rd.going_to_horse), (h, false));
                assert_eq!(o.status & 0x1000, 0);
                assert_eq!(o.carrier, ObjectId(77), "abort with failure message");
            }
        }
    }
}

#[test]
fn mounting_a_rider_or_boarding_a_passenger_fails_like_a_carrying_target() {
    // K-269 (A7, extended): if the target is itself a rider (`0x2000`) or occupant (`0x8000`), DoMount
    // fails as with a target with `CARRIING` (timer 0xA0D in the own module). Otherwise both would refer
    // to each other (two-element ring): the walk forwarding would run R -> X -> R ... without end, and
    // the savegame would not load.
    let rules = rules();
    let bits = status::ISMOUNTED | status::ISMOUNTEDONBOAT;
    for boat in [false, true] {
        let mut state = state(&rules);
        let (r, carrier, index) = {
            let mut sim = Sim::new(&mut state, &rules);
            let (carrier, guid, code, index) = if boat {
                let (w, _) = warrior_in_canoe(&mut sim, (14, 10), 0);
                (w, WARRIOR, 0x10E, RIDING_BOAT)
            } else {
                let (c, _) = rider_on_horse(&mut sim, COLONIST, (14, 10));
                (c, COLONIST, 0x10D, RIDING)
            };
            let r = spawn(&mut sim, guid, 0, (10, 10));
            let msg = Message::new(code, &[carrier.as_i32()]);
            assert!(crate::messaging::post_message(&mut sim, r, msg, true));
            (r, carrier, index)
        };
        let timer = move |sim: &Sim| {
            let b = &sim.module(r, index).unwrap().base;
            b.events
                .iter()
                .chain(b.timers_ws.iter())
                .any(|e| e.msg.code == 0xA0D)
        };
        // Stop also on a link, so that no tick runs in the ring.
        steps_until(&mut state, &rules, 400, |sim| {
            timer(sim) || sim.object(r).unwrap().status & bits != 0
        });
        {
            let sim = Sim::new(&mut state, &rules);
            let (o, c) = (sim.object(r).unwrap(), sim.object(carrier).unwrap());
            assert_eq!((o.status & bits, o.target), (0, ObjectId::NONE), "no link");
            assert_ne!(c.status & bits, 0, "the target stays rider or occupant");
            assert_eq!(c.status & status::CARRIING, 0, "the target does not carry");
            assert!(timer(&sim), "timer 0xA0D (boat: {boat})");
        }
        assert_loads(&state, &rules);
        // Walk queries and a walk command terminate on both.
        let mut sim = Sim::new(&mut state, &rules);
        for id in [r, carrier] {
            for code in [0x2600, 0x1400, 0xA00] {
                send(&mut sim, id, &Message::new(code, &[0, 0]));
            }
        }
        assert!(walks_itself(&mut sim, r), "the failed one walks itself");
    }
}

#[test]
fn a_warrior_boards_its_canoe_and_gets_the_boat_bit_and_the_bonus() {
    // re/riding 9 T6, C-218: `0x10E` -> approach with 0x30E/0x40E -> link with 0x8000; sight +20 %;
    // stand 0x50E {7} = 18; a walk command to the warrior goes to the canoe (layer 6).
    let mut rules = rules();
    rules.set_anim_labels(WARRIOR, 18, &[(1, 1000)]);
    let mut state = state(&rules);
    let (w, c) = {
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, WARRIOR, 0, (10, 10));
        let c = spawn(&mut sim, CANOE, 0, (14, 10));
        let msg = Message::new(0x10E, &[c.as_i32()]);
        assert!(crate::messaging::post_message(&mut sim, w, msg, true));
        (w, c)
    };
    step(&mut state, &rules, &[]);
    {
        let sim = Sim::new(&mut state, &rules);
        assert!(riding(&sim, w, RIDING_BOAT).unwrap().going_to_horse);
        let base = &sim.module(w, walk::INDEX).unwrap().base;
        assert_eq!(base.success, Some(Message::new(0x40E, &[c.as_i32()])));
        assert_eq!(base.fault, Some(Message::new(0x30E, &[])));
    }
    steps_until(&mut state, &rules, 400, |sim| {
        sim.object(w)
            .is_some_and(|o| o.status & status::ISMOUNTEDONBOAT != 0)
    });
    let mut sim = Sim::new(&mut state, &rules);
    assert_linked(&sim, w, c, RIDING_BOAT);
    assert_eq!(sim.object(w).unwrap().status & status::ISMOUNTED, 0);
    assert_mounty_bonus(&mut sim, w);
    assert_eq!(sim.object(w).unwrap().anim.wanted, 18, "stand 150003[0]");
    // Walk command: the canoe takes it, layer 6 is asked. The canoe stands on land
    // (layer 6 blocked): it frees itself first (CHECKPOS) and saves the command for that.
    let env = Envelope {
        msg: find_to_msg(center(20), center(10), -1, NO_DIRECTION, 0, false, 0x201),
        success: Some(Message::new(0xA7F, &[5])),
        fault: None,
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(&mut sim, w, &env);
    assert_eq!(walk::walk(&sim, c).unwrap().backup, Some(env.clone()));
    assert_eq!(walk::walk(&sim, w).unwrap().backup, None);
    assert_eq!(sim.module(w, walk::INDEX).unwrap().base.success, None);
    assert_eq!(send(&mut sim, w, &Message::new(0x1400, &[])), 6);
}

#[test]
fn a_pure_boat_type_ignores_the_failed_mount_search() {
    // K-226: the failed-attempt timer is in the own module (14) but always fires `0xA0D` at
    // module 13 - the WID warrior does not have it (answer -1); the PRI warrior (13 + 14) searches for a
    // horse.
    let rules = rules();
    let mut state = state(&rules);
    let (wid, pri, horse, horse_pri) = {
        let mut sim = Sim::new(&mut state, &rules);
        let wid = spawn(&mut sim, WARRIOR, 0, (10, 10));
        let c = spawn(&mut sim, CANOE, 0, (14, 10));
        let passenger = spawn(&mut sim, WARRIOR, 0, (14, 10));
        link(&mut sim, passenger, c);
        let horse = spawn(&mut sim, HORSE, 0, (12, 12));
        let pri = spawn(&mut sim, PRI_WARRIOR, 0, (10, 40));
        let c2 = spawn(&mut sim, CANOE, 0, (14, 40));
        let passenger2 = spawn(&mut sim, WARRIOR, 0, (14, 40));
        link(&mut sim, passenger2, c2);
        let horse_pri = spawn(&mut sim, HORSE, 1, (12, 42));
        for (who, boat) in [(wid, c), (pri, c2)] {
            let msg = Message::new(0x10E, &[boat.as_i32()]);
            assert!(crate::messaging::post_message(&mut sim, who, msg, true));
        }
        (wid, pri, horse, horse_pri)
    };
    let timer = |sim: &Sim, who: ObjectId| {
        let b = &sim.module(who, RIDING_BOAT).unwrap().base;
        b.events
            .iter()
            .chain(b.timers_ws.iter())
            .any(|e| e.msg.code == 0xA0D)
    };
    steps_until(&mut state, &rules, 400, |sim| {
        timer(sim, wid) && timer(sim, pri)
    });
    for _ in 0..4 {
        object_tick(&mut state, &rules);
    }
    let sim = Sim::new(&mut state, &rules);
    assert!(!timer(&sim, wid) && !timer(&sim, pri), "both fired");
    let o = sim.object(wid).unwrap();
    assert_eq!(o.status & (status::ISMOUNTEDONBOAT | 0x1000), 0);
    assert_ne!(
        walk::walk(&sim, wid).unwrap().quarry,
        horse,
        "WID does not search for a horse"
    );
    assert_eq!(sim.object(horse).unwrap().target, ObjectId::NONE);
    // The PRI warrior walks to the horse (Riding, not RidingBoat).
    let rd = riding(&sim, pri, RIDING).unwrap();
    assert_eq!((rd.mount_target, rd.going_to_horse), (horse_pri, true));
    assert_eq!(sim.object(pri).unwrap().exempt.object, horse_pri);
    assert!(!riding(&sim, pri, RIDING_BOAT).unwrap().going_to_horse);
}

#[test]
fn a_vanished_target_during_the_approach_aborts_without_a_link() {
    // Target destroyed during the approach -> walk failure or DoMount -1 and unsuccessful search;
    // afterwards neither bits nor reference, exemption zone off.
    let rules = rules();
    let mut state = state(&rules);
    let (r, h) = colonist_and_horse(&mut Sim::new(&mut state, &rules), 0);
    step(&mut state, &rules, &[]);
    {
        let mut sim = Sim::new(&mut state, &rules);
        assert!(riding(&sim, r, RIDING).unwrap().going_to_horse);
        crate::world::destroy_object(&mut sim, h);
    }
    for _ in 0..10 {
        object_tick(&mut state, &rules);
    }
    let sim = Sim::new(&mut state, &rules);
    let o = sim.object(r).unwrap();
    assert_eq!(
        o.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT | 0x1000),
        0
    );
    assert_eq!(o.target, ObjectId::NONE);
    assert_eq!(o.exempt.object, ObjectId(0), "K-225");
    assert!(!riding(&sim, r, RIDING).unwrap().going_to_horse);
}

#[test]
fn the_owner_change_callback_after_the_riders_death_mounts_the_corpse() {
    // K-266: T4 setup, the rider dies between `0x47F` and phase 0. Its modules are not switched off until
    // the end of T+1, DoMount does not check DIED: the callback puts the carcass on the taken-over
    // horse. The savegame loads; without a carcass the object tick clears CARRIING on the horse.
    use crate::save::SaveGame;
    use crate::state::SimEvent;
    let rules = rules();
    let mut state = state(&rules);
    let (r, h) = colonist_and_horse(&mut Sim::new(&mut state, &rules), 1);
    steps_until(&mut state, &rules, 400, |sim| {
        sim.object(h).is_some_and(|o| o.owner_change_target == r)
    });
    send(
        &mut Sim::new(&mut state, &rules),
        r,
        &Message::new(0x110, &[]),
    );
    assert_ne!(state.object(r, false).unwrap().status & status::DIED, 0);
    let events = step(&mut state, &rules, &[]);
    let renamed = |old: ObjectId| {
        events.iter().find_map(|e| match e {
            SimEvent::OwnerChanged { old: o, new } if *o == old => Some(*new),
            _ => None,
        })
    };
    // First the horse (slot 0, callback), then the corpse (slot 8, C-142).
    let (horse, corpse) = (renamed(h).unwrap(), renamed(r).unwrap());
    let sim = Sim::new(&mut state, &rules);
    let (c, m) = (sim.object(corpse).unwrap(), sim.object(horse).unwrap());
    assert_ne!(c.status & status::DIED, 0);
    assert_ne!(c.status & status::ISMOUNTED, 0, "carcass mounted");
    assert_ne!(m.status & status::CARRIING, 0);
    assert_eq!((c.target, m.target), (horse, corpse));
    assert_eq!(riding(&sim, corpse, RIDING).unwrap().mount_target, horse);
    let bytes = crate::save::testkit::save_bytes(&state, &rules);
    assert!(SaveGame::from_bytes(&bytes).is_ok(), "savegame loads");
    crate::world::destroy_object(&mut Sim::new(&mut state, &rules), corpse);
    object_tick(&mut state, &rules);
    let o = state.object(horse, false).unwrap();
    assert_eq!(o.status & status::CARRIING, 0, "object tick step 11");
}

#[test]
fn a_rider_posts_dismount_every_tick_while_its_player_counts_3377() {
    // re/riding 9 T9, K-229: every riding tick of a mounted object posts `{0x20D}` with abort as long as
    // the player counts 3377; canoe occupants never; a PRI type with both modules twice.
    use crate::message::Posted;
    let rules = rules();
    let mut state = state(&rules);
    let (d, w, p) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (d, _) = rider_on_horse(&mut sim, DRAGOON, (10, 10));
        let w = spawn(&mut sim, WARRIOR, 0, (14, 10));
        let c = spawn(&mut sim, CANOE, 0, (14, 10));
        link(&mut sim, w, c);
        let (p, _) = rider_on_horse(&mut sim, PRI_WARRIOR, (18, 10));
        (d, w, p)
    };
    let posts = |state: &SimState, id: ObjectId| -> usize {
        let fifo = &state.object(id, false).unwrap().fifo;
        assert!(fifo.iter().all(|e| matches!(
            e,
            Posted::Message { msg, abort: true } if msg.code == 0x20D && msg.args.is_empty()
        )));
        fifo.len()
    };
    object_tick(&mut state, &rules);
    assert_eq!((posts(&state, d), posts(&state, p)), (0, 0), "without 3377");
    state.add_guid_count(0, 3377, 1);
    for _ in 0..3 {
        object_tick(&mut state, &rules);
        assert_eq!(posts(&state, d), 1, "post per tick");
        assert_eq!(posts(&state, w), 0, "boat occupants never");
        assert_eq!(posts(&state, p), 2, "Riding and RidingBoat both post");
        // Empty the FIFO: the next tick posts again (independent of dismounting).
        for id in [d, p] {
            state.object_mut(id, false).unwrap().fifo.clear();
        }
    }
}

#[test]
fn a_counter_upgrade_3377_makes_enemy_riders_dismount() {
    // K-229 with a real counter: slot 1 starts 3377 via `counter::start` (without cost, config 13
    // missing -> 10 s, W-229); the enemy 0 counts 3377, its mounted unit posts `{0x20D}` and
    // dismounts, the own mounted unit of slot 1 does not. After 100 phase-0 steps the counter is 0
    // again.
    use crate::message::Posted;
    use crate::upgrades::counter;
    let rules = rules();
    let mut state = state(&rules);
    let (d, own) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (d, _) = rider_on_horse(&mut sim, DRAGOON, (10, 10));
        let own = spawn(&mut sim, DRAGOON, 1, (20, 10));
        let horse = spawn(&mut sim, HORSE, 1, (20, 10));
        link(&mut sim, own, horse);
        let source = spawn(&mut sim, WARRIOR, 1, (30, 10));
        assert!(counter::start(&mut sim, 3377, source));
        (d, own)
    };
    assert_eq!(state.guid_counts[0].get(&3377), Some(&1));
    assert_eq!(state.guid_counts[1].get(&3377), None, "enemies only");
    assert_eq!(state.counter_upgrades[0].rest_ms, 10_000);
    let posted = |state: &SimState, id: ObjectId| {
        state
            .object(id, false)
            .unwrap()
            .fifo
            .iter()
            .any(|e| matches!(e, Posted::Message { msg, abort: true } if msg.code == 0x20D))
    };
    let mounted = |state: &SimState, id: ObjectId| {
        state.object(id, false).unwrap().status & status::ISMOUNTED != 0
    };
    object_tick(&mut state, &rules);
    assert!(posted(&state, d) && !posted(&state, own));
    assert_eq!(state.phase, 0);
    let n = steps_until(&mut state, &rules, 190, |sim| {
        sim.object(d)
            .is_some_and(|o| o.status & status::ISMOUNTED == 0)
    });
    assert!(mounted(&state, own), "the own rider stays up");
    let ended = steps_until(&mut state, &rules, 400, |sim| {
        sim.state.counter_upgrades.is_empty()
    });
    // Start before the first step: the 100th phase-0 step is the 200th step.
    assert_eq!(2 + n + ended, 200);
    assert_eq!(state.guid_counts[0].get(&3377).copied().unwrap_or(0), 0);
}

#[test]
fn the_dead_mount_animation_branch_finishes_through_0x70d() {
    // re/riding 3.1 (dead branch, K-223): with `next_mount_anim` 0 and an animation in 140012
    // the rider jumps, plays it locked and ends mounting via `0x70D` after time marker 1;
    // only then comes the mounty bonus.
    let mut rules = rules();
    rules.set_anim_labels(COLONIST, 9, &[(1, 300)]);
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let r = spawn(&mut sim, COLONIST, 0, (10, 10));
    let h = spawn(&mut sim, HORSE, 0, (11, 10));
    {
        let rd = riding_mut(&mut sim, r, RIDING).unwrap();
        rd.mount_target = h;
        rd.next_mount_anim = 0;
        rd.anims[list::MOUNT_LEFT] = vec![9];
    }
    assert_eq!(mount::do_mount(&mut sim, r, RIDING), 1);
    let o = sim.object(r).unwrap();
    assert_ne!(o.status & status::ANIM_LOCKED, 0);
    assert_eq!(o.anim.wanted, 9);
    assert_eq!(o.grid, sim.object(h).unwrap().grid);
    assert!(riding(&sim, r, RIDING).unwrap().busy);
    let t = sim
        .module(r, RIDING)
        .unwrap()
        .base
        .timers_ws
        .front()
        .cloned();
    assert_eq!(t.map(|e| (e.msg.code, e.b)), Some((0x70D, 300)));
    assert_eq!(sim.attribute(r, 18), 32, "bonus not until FinishMount");
    assert_eq!(send(&mut sim, r, &Message::new(0x70D, &[])), 1);
    assert!(!riding(&sim, r, RIDING).unwrap().busy);
    let (o, m) = (sim.object(r).unwrap(), sim.object(h).unwrap());
    assert_eq!(
        o.status & (status::ANIM_LOCKED | status::ISMOUNTED),
        status::ISMOUNTED
    );
    assert_eq!(
        (o.target, m.target, m.status & status::CARRIING),
        (h, r, status::CARRIING)
    );
    assert_eq!(sim.attribute(r, 18), 38);
}

/// Savegame of the state loads (structure check including the link).
fn assert_loads(state: &SimState, rules: &crate::rules::Rules) {
    let bytes = crate::save::testkit::save_bytes(state, rules);
    if let Err(e) = crate::save::SaveGame::from_bytes(&bytes) {
        panic!("savegame does not load: {e:#}");
    }
}

#[test]
fn mounting_clears_the_marks_of_the_mount() {
    // E-57 (re/ki-queries 5.9, re/riding 3.1): after `AbortAllModules`, `DoMount` clears the whole
    // AI mark of the mount (`GameObject_ClearField04` `0x00467FA0`); the rider keeps its
    // bits. A failure (target already carries) leaves the mark.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let r = spawn(&mut sim, COLONIST, 0, (10, 10));
    let h = spawn(&mut sim, HORSE, 0, (11, 10));
    sim.object_mut(r).unwrap().ai_marks = 0b1010;
    sim.object_mut(h).unwrap().ai_marks = 0b101;
    riding_mut(&mut sim, r, RIDING).unwrap().mount_target = h;
    sim.object_mut(h)
        .unwrap()
        .set_status_flags(status::CARRIING, true);
    assert_eq!(mount::do_mount(&mut sim, r, RIDING), -1);
    assert_eq!(sim.object(h).unwrap().ai_marks, 0b101, "failure");
    sim.object_mut(h)
        .unwrap()
        .set_status_flags(status::CARRIING, false);
    assert_eq!(mount::do_mount(&mut sim, r, RIDING), 1);
    assert!(mounted(&sim, r));
    assert_eq!(sim.object(h).unwrap().ai_marks, 0);
    assert_eq!(sim.object(r).unwrap().ai_marks, 0b1010);
}

#[test]
fn mounting_within_reach_ends_synchronously_and_keeps_going_to_horse() {
    // K-267: if the horse is within reach and in view (same position and direction), the walk reports
    // the success still in `dispatch_command`; DoMount links and clears `going_to_horse`
    // and `0x1000`, after that `mount_start` sets `going_to_horse` again. The savegame loads; an
    // abort clears the flag (the link stays).
    let rules = rules();
    let mut state = state(&rules);
    let (r, h) = {
        let mut sim = Sim::new(&mut state, &rules);
        let r = spawn(&mut sim, COLONIST, 0, (10, 10));
        let h = spawn(&mut sim, HORSE, 0, (10, 10));
        let env = Envelope::bare(Message::new(0x10D, &[h.as_i32()]));
        assert_eq!(dispatch_command(&mut sim, r, &env), 1);
        let o = sim.object(r).unwrap();
        assert_eq!(o.status & (status::ISMOUNTED | 0x1000), status::ISMOUNTED);
        assert_eq!((o.target, sim.object(h).unwrap().target), (h, r));
        assert!(riding(&sim, r, RIDING).unwrap().going_to_horse, "K-267");
        (r, h)
    };
    assert_loads(&state, &rules);
    let mut sim = Sim::new(&mut state, &rules);
    assert!(abort(&mut sim, r, RIDING));
    assert!(!riding(&sim, r, RIDING).unwrap().going_to_horse);
    assert_eq!(
        sim.object(r).unwrap().target,
        h,
        "abort does not release the link"
    );
}

#[test]
fn a_target_dying_during_the_approach_leaves_a_loadable_state() {
    // The own horse dies during the approach (ownership to slot 8, C-142). The old ID
    // disappears with the change: the walk loses its target, the rider stays free.
    // No panic, every intermediate state loads.
    let rules = rules();
    let mut state = state(&rules);
    let (r, h) = colonist_and_horse(&mut Sim::new(&mut state, &rules), 0);
    step(&mut state, &rules, &[]);
    send(
        &mut Sim::new(&mut state, &rules),
        h,
        &Message::new(0x110, &[]),
    );
    for _ in 0..20 {
        object_tick(&mut state, &rules);
        assert_loads(&state, &rules);
    }
    let sim = Sim::new(&mut state, &rules);
    assert!(sim.object(h).is_none(), "carcass changed its ID");
    let o = sim.object(r).unwrap();
    assert_eq!(o.status & (status::ISMOUNTED | 0x1000), 0);
    assert_eq!(o.target, ObjectId::NONE);
    assert!(!riding(&sim, r, RIDING).unwrap().going_to_horse);
}

#[test]
fn a_foreign_horse_destroyed_after_0x47f_leaves_the_rider_waiting() {
    // The foreign horse is destroyed after `0x47F` before the change of owner: the
    // change finds nothing, no callback. The rider stays in the approach state (exemption zone,
    // vanished ID) until an abort releases it. The savegame loads.
    let rules = rules();
    let mut state = state(&rules);
    let (r, h) = colonist_and_horse(&mut Sim::new(&mut state, &rules), 1);
    steps_until(&mut state, &rules, 400, |sim| {
        sim.object(h).is_some_and(|o| o.owner_change_target == r)
    });
    crate::world::destroy_object(&mut Sim::new(&mut state, &rules), h);
    object_tick(&mut state, &rules);
    assert!(state.owner_changes.is_empty());
    assert_loads(&state, &rules);
    let mut sim = Sim::new(&mut state, &rules);
    let o = sim.object(r).unwrap();
    assert_eq!(o.status & (status::ISMOUNTED | 0x1000), 0x1000);
    let rd = riding(&sim, r, RIDING).unwrap();
    assert_eq!((rd.mount_target, rd.going_to_horse), (h, true));
    assert!(abort(&mut sim, r, RIDING));
    assert_eq!(sim.object(r).unwrap().status & 0x1000, 0);
    assert_eq!(sim.object(r).unwrap().exempt.object, ObjectId(0));
}

// --- Dismounting and disembarking (re/riding 3.2, 8.7, 8.13, 8.15, 13.1) ---

/// Dragoon on horse via placement 2008 (with mounty bonus) at `tile`; both look in direction
/// `dir` (fixed-point degrees), the horse has the soft radius `soft`.
fn placed_dragoon(sim: &mut Sim, tile: (i32, i32), dir: i32, soft: i32) -> (ObjectId, ObjectId) {
    use crate::command::testkit::world;
    use crate::world::{ObjectDesc, create_object, desc_flags, set_direction};
    let mut desc = ObjectDesc::new(DRAGOON_ON_HORSE, 0, [world(tile.0), 0.0, world(tile.1)]);
    desc.flags |= desc_flags::PLACED;
    let r = create_object(sim, &mut desc).unwrap();
    let h = sim.object(r).unwrap().target;
    // The horse carries: the direction is passed on to the rider.
    set_direction(sim, h, dir);
    sim.object_mut(h).unwrap().soft_radius = soft;
    (r, h)
}

/// Dismounting synchronously as from `OnMessage` (`{0x20D}` or `{0x20E}` as envelope); return 1 =
/// dismounted.
fn dismount_now(sim: &mut Sim, r: ObjectId, code: u32) -> i32 {
    dispatch_command(sim, r, &Envelope::bare(Message::new(code, &[])))
}

/// Link released: rider without bits and reference, mount without `CARRIING` and reference.
fn assert_unlinked(sim: &Sim, r: ObjectId, m: ObjectId) {
    let (o, t) = (sim.object(r).unwrap(), sim.object(m).unwrap());
    assert_eq!(
        o.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT | status::ANIM_LOCKED),
        0
    );
    assert_eq!(t.status & status::CARRIING, 0);
    assert_eq!((o.target, t.target), (ObjectId::NONE, ObjectId::NONE));
}

#[test]
fn dismounting_searches_the_circle_from_the_left_side() {
    // re/riding 9 T2: Dragoon (hard 512) on horse (soft 1500), viewing direction 0 -> radius 2012,
    // first attempt (x + 0, z + 2012) (cos 90 degrees = 0, sin 90 degrees = 4096). Post `{0x20D}` with
    // abort -> in the next object tick there (keepY); horse free with walk behavior `{0, 0, -1, 0, 0}`;
    // no new animation on the rider (D-215, re/riding 8.15); home = dismount point.
    let mut rules = rules();
    rules.set_anim_labels(DRAGOON, 7, &[(1, 1000)]);
    let mut state = state(&rules);
    let (r, h, start, anim) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (r, h) = placed_dragoon(&mut sim, (10, 10), 0, 1500);
        assert_mounty_bonus(&mut sim, r);
        walk_mut(&mut sim, r).unwrap().home = (center(3), center(3), NO_DIRECTION);
        let o = sim.object(r).unwrap();
        let start = (o.grid.x.0, o.grid.y.0, o.grid.z.0);
        let anim = o.anim.wanted;
        assert!(crate::messaging::post_message(
            &mut sim,
            r,
            Message::new(0x20D, &[]),
            true
        ));
        (r, h, start, anim)
    };
    object_tick(&mut state, &rules);
    assert_loads(&state, &rules);
    let sim = Sim::new(&mut state, &rules);
    let o = sim.object(r).unwrap();
    assert_eq!(
        (o.grid.x.0, o.grid.y.0, o.grid.z.0),
        (start.0, start.1, start.2 + 2012),
        "first circle point on the left, height stays"
    );
    assert_unlinked(&sim, r, h);
    assert_eq!(sim.attribute(r, 18), 32, "sight without modifier 0x7E");
    assert_eq!(
        o.anim.wanted, anim,
        "no stand animation without a dismount animation"
    );
    assert_eq!(
        walk::walk(&sim, r).unwrap().home,
        (start.0, start.2 + 2012, NO_DIRECTION),
        "home = dismount point (0x2500)"
    );
    let w = walk::walk(&sim, h).unwrap();
    assert_eq!(
        (w.try_to_follow, w.mobile_home, w.home.0, w.tether),
        (false, false, -1, -1),
        "horse: 0x2100 {{0, 0, -1, 0, 0}}"
    );
    let p = sim.object(h).unwrap();
    assert_eq!(
        (p.grid.x.0, p.grid.z.0),
        (start.0, start.2),
        "horse stays put"
    );
}

#[test]
fn dismounting_skips_angles_outside_the_map_in_steps_of_ten_degrees() {
    // K-268: viewing direction 90 degrees -> first attempt at 180 degrees (x - 2012). The rider stands at
    // x = 600: the angles 180...260 degrees lie (with the footprint +-512) outside the map on the left,
    // 270 degrees is the first with all five footprint points inside -> (600, z - 2012).
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (r, h) = placed_dragoon(&mut sim, (0, 30), 90 << 12, 1500);
    let z = center(30);
    crate::world::set_grid_position(
        &mut sim,
        h,
        crate::fx::FxVec3::new(crate::fx::Fx(600), crate::fx::Fx(0), crate::fx::Fx(z)),
        true,
    );
    assert_eq!(sim.object(r).unwrap().grid.x.0, 600, "the rider follows");
    // At 260 degrees the corner -512 sticks out over the edge: cos 260 degrees * 2012 = -349.
    assert!(600 + fixed_mul(crate::math::cos_deg(260 << 12), 2012) - 512 < 0);
    assert_eq!(dismount_now(&mut sim, r, 0x20D), 1);
    let o = sim.object(r).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), (600, z - 2012));
    assert_unlinked(&sim, r, h);
}

#[test]
fn dismounting_ignores_obstacles_and_units_at_the_first_angle() {
    // K-268: while searching, the rider still carries `0x2000` and is exempt from collision -
    // blocked cells and units at the first circle point do not count, it dismounts there.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (r, h) = placed_dragoon(&mut sim, (10, 10), 0, 1500);
    let (x, z) = {
        let o = sim.object(r).unwrap();
        (o.grid.x.0, o.grid.z.0 + 2012)
    };
    sim.state.path.mark_cell(0x1FF, x >> 12, z >> 12, true);
    let blocker = spawn(&mut sim, COLONIST, 1, (x >> 12, z >> 12));
    let layer = send(&mut sim, h, &Message::new(0xB00, &[])) as usize;
    assert!(
        crate::collision::query_cost_at(&mut sim, x, z, Some(blocker), layer, false, false) >= 40,
        "for a free unit the point is blocked"
    );
    assert_eq!(dismount_now(&mut sim, r, 0x20D), 1);
    let o = sim.object(r).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), (x, z));
    assert_unlinked(&sim, r, h);
}

#[test]
fn dismounting_without_a_free_spot_stays_mounted() {
    // K-230: if all 36 circle points lie outside the map (horse with soft radius
    // 70 tiles on the 64-tile map), dismounting fails silently: returns 0, link, bonus
    // and position stay.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (r, h) = placed_dragoon(&mut sim, (32, 32), 0, 70 << 12);
    let before = sim.object(r).unwrap().grid;
    assert_eq!(dismount_now(&mut sim, r, 0x20D), 0);
    assert_linked(&sim, r, h, RIDING);
    assert_mounty_bonus(&mut sim, r);
    assert_eq!(sim.object(r).unwrap().grid, before);
}

#[test]
fn leaving_a_pri_canoe_uses_dummy01_and_a_wid_canoe_the_circle() {
    // re/riding 13.1: PRI canoe with `dummy01` (rest pose 0; 11.81; -24.95) -> exit at
    // `local_offset_to_grid`: direction 0 -> a = -90 degrees, lz = (-24 << 12) / 128 = -768 ->
    // (x + 768, z), without circle search. WID canoe without an entry -> circle on the layer of the canoe
    // (`0xB00`: 1x1 water layer 4) with radius hard(warrior) 512 + soft(canoe) 4096 (T7).
    let mut rules = rules();
    rules
        .mount_nodes
        .insert(PRI_CANOE, [0.0, 11.806_807, -24.950_59]);
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let p = spawn(&mut sim, PRI_WARRIOR, 0, (10, 10));
    let pc = spawn(&mut sim, PRI_CANOE, 0, (10, 10));
    link(&mut sim, p, pc);
    crate::world::set_direction(&mut sim, pc, 0);
    let g = sim.object(pc).unwrap().grid;
    assert_eq!(dismount_now(&mut sim, p, 0x20E), 1);
    let o = sim.object(p).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), (g.x.0 + 768, g.z.0));
    assert_eq!(
        crate::footprint::local_offset_to_grid(
            rules.mount_nodes[&PRI_CANOE],
            g,
            crate::fx::Fx(0),
            rules.map.cell
        ),
        (g.x.0 + 768, g.z.0)
    );
    assert_unlinked(&sim, p, pc);

    let w = spawn(&mut sim, WARRIOR, 0, (30, 30));
    let c = spawn(&mut sim, CANOE, 0, (30, 30));
    link(&mut sim, w, c);
    crate::world::set_direction(&mut sim, c, 0);
    sim.object_mut(c).unwrap().soft_radius = 4096;
    assert_eq!(
        send(&mut sim, c, &Message::new(0xB00, &[])),
        4,
        "1x1 layer of the canoe (water, for the 2x2 layer 6)"
    );
    let g = sim.object(c).unwrap().grid;
    assert_eq!(dismount_now(&mut sim, w, 0x20E), 1);
    let o = sim.object(w).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), (g.x.0, g.z.0 + 4608));
    assert_unlinked(&sim, w, c);
}

#[test]
fn the_release_goes_through_the_dying_module() {
    // re/riding 8.7, re/combat-death 2.8: `0x810` to the dying module of the rider releases the
    // mount - leave group (`0x1500`), abort, `0x2100 {0, 0, -1, 0, 0}`, `ANIM_LOCKED` and
    // `CARRIING` off, references -1. The rider bit stays (not cleared until `FinishDismount`).
    // Without a target: returns 0, reference -1 anyway.
    use crate::group::{GroupOrder, NO_GROUP, create_group, find, is_member};
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (r, h) = placed_dragoon(&mut sim, (10, 10), 0, 1024);
    let order = GroupOrder {
        units: vec![r],
        x: center(10),
        z: center(10),
        formation: 0,
        direction: 0,
        spread: 0,
        run: false,
    };
    // The rider stands in the group for its horse (`Group_AddMember`).
    let gid = create_group(&mut sim, &order).unwrap();
    assert!(is_member(&sim, gid, r) && find(&sim, gid).unwrap().members == [h]);
    assert_eq!(send(&mut sim, h, &Message::new(0x1500, &[])), gid);
    let walk_to = find_to_msg(center(20), center(10), -1, NO_DIRECTION, 0, false, 0x201);
    dispatch_command(&mut sim, h, &Envelope::bare(walk_to));
    assert!(
        walk::walk(&sim, h).unwrap().state > 1,
        "horse has a walk order"
    );
    sim.object_mut(h)
        .unwrap()
        .set_status_flags(status::ANIM_LOCKED, true);
    assert_eq!(send(&mut sim, r, &Message::new(0x810, &[])), 1);
    let (o, m) = (sim.object(r).unwrap(), sim.object(h).unwrap());
    assert_eq!(o.status & status::ISMOUNTED, status::ISMOUNTED);
    assert_eq!((o.target, m.target), (ObjectId::NONE, ObjectId::NONE));
    assert_eq!(m.status & (status::CARRIING | status::ANIM_LOCKED), 0);
    assert_eq!(
        walk::walk(&sim, h).unwrap().state,
        1,
        "aborted (`StopMoving`)"
    );
    assert_eq!(walk::walk(&sim, h).unwrap().group, NO_GROUP);
    assert!(find(&sim, gid).is_none(), "empty group deleted");
    let w = walk::walk(&sim, h).unwrap();
    assert_eq!(
        (w.try_to_follow, w.mobile_home, w.tether),
        (false, false, -1)
    );
    assert_eq!(
        send(&mut sim, r, &Message::new(0x810, &[])),
        0,
        "without target"
    );
}

#[test]
fn the_dead_dismount_animation_branch_finishes_through_0x80d() {
    // re/riding 3.2 (dead branch, 140014 empty in the shipped data): with a dismount animation the
    // mount aborts, rider and mount play it, the rider is locked and busy; `0x80D {x, z}`
    // after time marker 1 dismounts and sets the stand animation (`0x102`, re/riding 8.15).
    let mut rules = rules();
    rules.set_anim_labels(DRAGOON, 9, &[(1, 400)]);
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (r, h) = placed_dragoon(&mut sim, (10, 10), 0, 1500);
    riding_mut(&mut sim, r, RIDING).unwrap().anims[list::UNMOUNT] = vec![9];
    let (x, z) = {
        let o = sim.object(r).unwrap();
        (o.grid.x.0, o.grid.z.0 + 2012)
    };
    assert_eq!(dismount_now(&mut sim, r, 0x20D), 1);
    let o = sim.object(r).unwrap();
    assert_eq!(
        o.status & (status::ISMOUNTED | status::ANIM_LOCKED),
        status::ISMOUNTED | status::ANIM_LOCKED
    );
    assert_eq!(o.anim.wanted, 9);
    assert_eq!(
        sim.object(h).unwrap().anim.wanted,
        9,
        "same animation on the horse"
    );
    assert!(riding(&sim, r, RIDING).unwrap().busy);
    let t = sim
        .module(r, RIDING)
        .unwrap()
        .base
        .timers_ws
        .front()
        .cloned();
    assert_eq!(
        t.map(|e| (e.msg, e.b)),
        Some((Message::new(0x80D, &[x, z]), 400))
    );
    assert_eq!(send(&mut sim, r, &Message::new(0x80D, &[x, z])), 1);
    let o = sim.object(r).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), (x, z));
    assert_eq!(o.anim.wanted, 0, "stand 30001[0] via 0x102");
    assert!(!riding(&sim, r, RIDING).unwrap().busy);
    assert_unlinked(&sim, r, h);
}

// --- Death, canoe sinking and quarters in the link (re/combat-death 2.8, 2.9; re/riding 3.3, 4.4,
// 4.5, 13.3, 13.4) ---

/// Two `step`s: one object tick, surely including an engine tick with phase 0 (change of owner, destroying).
fn through_phase_0(state: &mut SimState, rules: &crate::rules::Rules) {
    object_tick(state, rules);
}

/// Trigger dying (`0x110` synchronously to the dying module); returns the query result.
fn kill(sim: &mut Sim, id: ObjectId) -> i32 {
    send(sim, id, &Message::new(0x110, &[]))
}

/// The walk module accepts a walk command itself (state > 1).
fn walks_itself(sim: &mut Sim, id: ObjectId) -> bool {
    let to = find_to_msg(center(30), center(30), -1, NO_DIRECTION, 0, false, 0x201);
    dispatch_command(sim, id, &Envelope::bare(to));
    walk::walk(sim, id).unwrap().state > 1
}

/// Warrior in the WID canoe (linked, canoe looks in direction `dir`) at `tile`.
fn warrior_in_canoe(sim: &mut Sim, tile: (i32, i32), dir: i32) -> (ObjectId, ObjectId) {
    let w = spawn(sim, WARRIOR, 0, tile);
    let c = spawn(sim, CANOE, 0, tile);
    link(sim, w, c);
    crate::world::set_direction(sim, c, dir);
    (w, c)
}

/// Objects of a type at the grid position `(x, z)`.
fn count_at(sim: &Sim, guid: u32, x: i32, z: i32) -> usize {
    sim.state
        .objects()
        .filter(|o| o.guid == guid && o.grid.x.0 == x && o.grid.z.0 == z)
        .count()
}

#[test]
fn a_dying_rider_frees_its_horse() {
    // re/combat-death 2.8: rider dies (0x110) -> mount free via release_mount (abort, leave group,
    // 0x2100 {0,0,-1,0,0}, CARRIING off, target -1), rider bits off; dying animation
    // of the rider from 0x50D {8} (Dragoon: 3); the horse lives and walks free.
    let rules = rules();
    let mut state = state(&rules);
    let (r, h) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (r, h) = placed_dragoon(&mut sim, (10, 10), 0, 1024);
        // Make the own dying animation (170002) distinguishable: then 3 comes only from 0x50D {8}.
        if let Some(crate::module::ModuleKind::Dying(d)) = sim
            .module_mut(r, crate::module::dying::INDEX)
            .map(|m| &mut m.kind)
        {
            d.die_anim = 0;
        }
        let before = sim.object(r).unwrap().grid;
        assert_eq!(kill(&mut sim, r), 0);
        let (o, m) = (sim.object(r).unwrap(), sim.object(h).unwrap());
        assert_ne!(o.status & status::DIED, 0);
        assert_eq!(o.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT), 0);
        assert_eq!(o.anim.wanted, 3, "dying animation 0x50D {{8}} = 140005[0]");
        assert_eq!(o.grid, before);
        assert_eq!((o.target, m.target), (ObjectId::NONE, ObjectId::NONE));
        assert_eq!(m.status & (status::CARRIING | status::DIED), 0);
        let w = walk::walk(&sim, h).unwrap();
        assert_eq!(
            (w.try_to_follow, w.mobile_home, w.tether),
            (false, false, -1),
            "0x2100 {{0, 0, -1, 0, 0}}"
        );
        (r, h)
    };
    through_phase_0(&mut state, &rules);
    assert_loads(&state, &rules);
    let mut sim = Sim::new(&mut state, &rules);
    assert!(sim.object(r).is_none(), "carcass on slot 8");
    assert!(walks_itself(&mut sim, h), "the horse walks free");
}

#[test]
fn a_dying_horse_frees_the_rider_and_keeps_bonus_and_link() {
    // T8/C-153: horse dies -> horse CARRIING off, target -1; rider 0x2000 off, rider.target
    // and mount_target stay the ID of the horse, sight modifier 0x7E stays (re/riding 8.6); rider
    // stands at the horse position. After phase 0 the carcass has a new ID (slot 8): the old one
    // no longer exists, the rider still refers to it. Its walk plans itself again.
    let rules = rules();
    let mut state = state(&rules);
    let (r, h) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (r, h) = placed_dragoon(&mut sim, (10, 10), 0, 1024);
        assert_eq!(kill(&mut sim, h), 0);
        let (o, m) = (sim.object(r).unwrap(), sim.object(h).unwrap());
        assert_ne!(m.status & status::DIED, 0);
        assert_eq!(m.status & status::CARRIING, 0);
        assert_eq!(m.target, ObjectId::NONE);
        assert_eq!(o.status & (status::ISMOUNTED | status::DIED), 0);
        assert_eq!(o.target, h, "rider refers to the carcass");
        assert_eq!((o.grid.x, o.grid.z), (m.grid.x, m.grid.z), "horse position");
        assert_eq!(riding(&sim, r, RIDING).unwrap().mount_target, h);
        assert_mounty_bonus(&mut sim, r);
        (r, h)
    };
    assert_loads(&state, &rules);
    through_phase_0(&mut state, &rules);
    assert_loads(&state, &rules);
    let mut sim = Sim::new(&mut state, &rules);
    assert!(sim.object(h).is_none(), "old ID gone after phase 0");
    assert_eq!(
        sim.object(r).unwrap().target,
        h,
        "reference stays (original)"
    );
    assert_eq!(riding(&sim, r, RIDING).unwrap().mount_target, h);
    assert_mounty_bonus(&mut sim, r);
    assert!(
        walks_itself(&mut sim, r),
        "walk command reaches the rider walk"
    );
}

#[test]
fn dismounting_after_the_horses_death_jumps_off_the_corpse_until_phase_0() {
    // K-270: `DismountStart` does not check rider bits. Before phase 0 the carcass
    // exists under the old ID: `0x20D` "dismounts" - the rider jumps to the first circle point, the
    // release hits the carcass (no effect), the mounty bonus goes. After phase 0 the target is
    // missing: returns 0 without effect. Both without panic, every state loads.
    let rules = rules();
    let mut state = state(&rules);
    let (r, h, start) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (r, h) = placed_dragoon(&mut sim, (10, 10), 0, 1024);
        let g = sim.object(r).unwrap().grid;
        kill(&mut sim, h);
        (r, h, g)
    };
    // Before phase 0 (own copy of the state).
    let mut early = state.clone();
    {
        let mut sim = Sim::new(&mut early, &rules);
        assert_eq!(dismount_now(&mut sim, r, 0x20D), 1);
        let o = sim.object(r).unwrap();
        // Radius hard(Dragoon) 512 + soft(carcass) 1024; viewing direction 0 -> (x, z + 1536).
        assert_eq!((o.grid.x.0, o.grid.z.0), (start.x.0, start.z.0 + 1536));
        assert_eq!(o.target, ObjectId::NONE);
        assert_eq!(sim.attribute(r, 18), 32, "mounty bonus removed");
        let m = sim.object(h).unwrap();
        assert_eq!((m.target, m.status & status::CARRIING), (ObjectId::NONE, 0));
    }
    assert_loads(&early, &rules);
    through_phase_0(&mut early, &rules);
    assert_loads(&early, &rules);
    // After phase 0: the target is missing.
    through_phase_0(&mut state, &rules);
    let mut sim = Sim::new(&mut state, &rules);
    let before = sim.object(r).unwrap().grid;
    assert_eq!(dismount_now(&mut sim, r, 0x20D), 0);
    assert_eq!(sim.object(r).unwrap().grid, before);
    assert_eq!(sim.object(r).unwrap().target, h);
    assert_mounty_bonus(&mut sim, r);
    assert_loads(&state, &rules);
}

#[test]
fn a_dying_passenger_kills_the_boat() {
    // C-148, re/riding 13.4: `0x110` on the occupant goes to the canoe, the return value is that of the canoe
    // (0). The occupant gets event bit 2, the statistics, sound and sight source and
    // becomes unselectable, but not DIED; the canoe dies. Its `unlink` lets it off via `0x70E {1}`
    // and makes it selectable again (re/combat-death 2.8) - so in the end it is selectable.
    use crate::state::SimEvent;
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (w, c) = warrior_in_canoe(&mut sim, (10, 10), 0);
    let tile = sim.object(w).unwrap().tile;
    sim.state.events.clear();
    let sources = sim.state.fog.sources.len();
    assert_eq!(kill(&mut sim, w), 0, "answer of the canoe");
    let (o, k) = (sim.object(w).unwrap(), sim.object(c).unwrap());
    assert_eq!(o.status & status::DIED, 0, "occupant not dead");
    assert_ne!(k.status & status::DIED, 0, "the canoe dies");
    assert!(o.selectable, "made selectable again by the sinking canoe");
    assert_eq!(o.status & status::ISMOUNTEDONBOAT, 0);
    let sounds: Vec<ObjectId> = sim
        .state
        .events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Sound3d {
                object,
                action: 0x65,
                ..
            } => Some(*object),
            _ => None,
        })
        .collect();
    assert_eq!(
        sounds,
        [w, c],
        "death sound first on the occupant, then on the canoe"
    );
    let new_sources: Vec<_> = sim.state.fog.sources[sources..]
        .iter()
        .map(|s| (s.x, s.z_or_hid, s.size, s.remaining_ms))
        .collect();
    assert_eq!(
        new_sources,
        [(tile.0, tile.1, 8, 10_000), (tile.0, tile.1, 8, 10_000)],
        "sight source of the occupant and of the canoe"
    );
}

#[test]
fn damage_to_a_passenger_goes_to_its_canoe() {
    // C-135 with a real canoe: `0x17F` on the occupant goes unchanged to the canoe.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (w, c) = warrior_in_canoe(&mut sim, (10, 10), 0);
    let a = spawn(&mut sim, WARRIOR, 1, (12, 10));
    let msg = crate::combat::damage::damage_message(&mut sim, a, -9);
    send(&mut sim, w, &msg);
    let hp = |sim: &Sim, id: ObjectId| sim.object(id).unwrap().attributes.current_fraction(14);
    assert_eq!(hp(&sim, w), 100_000, "occupant unhurt");
    assert!(hp(&sim, c) < 100_000, "the canoe takes the damage");
}

#[test]
fn a_sinking_canoe_leaves_its_passenger_and_a_new_paddler() {
    // K-264/K-265: WID canoe with warrior dies -> canoe: CARRIING off, target -1, queue_destroy, no
    // change of owner; warrior: 0x8000 off, selectable, position unchanged (no offset by 2048),
    // animation 20 (0x70E: 150004[0]) with 0x800000 and busy, after time marker 1 0x80E -> both off.
    // New object GUID 1400, owner of the canoe, color of the owner slot
    // (`GameWorld_GetPlayerColor`, `0x0050ED43`), position = canoe position, ONWATER, one-shot animation =
    // dying animation of the canoe (3). After phase 0: canoe gone; two warriors at the point.
    let mut rules = rules();
    rules.set_anim_labels(WARRIOR, 20, &[(1, 300)]);
    let mut state = state(&rules);
    let (w, c, g, paddler) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (w, c) = warrior_in_canoe(&mut sim, (10, 10), 90 << 12);
        sim.object_mut(w).unwrap().selectable = false;
        sim.object_mut(c).unwrap().color = 5;
        let (g, rot) = {
            let k = sim.object(c).unwrap();
            (k.grid, k.rotation[1])
        };
        let created = sim.state.events.len();
        assert_eq!(kill(&mut sim, c), 0);
        let k = sim.object(c).unwrap();
        assert_eq!((k.status & status::CARRIING, k.target), (0, ObjectId::NONE));
        assert!(sim.state.destroy_queue.contains(&c));
        assert!(sim.state.owner_changes.is_empty(), "no change of owner");
        let o = sim.object(w).unwrap();
        assert_eq!(o.status & status::ISMOUNTEDONBOAT, 0);
        assert!(o.selectable);
        assert_eq!((o.grid.x, o.grid.z), (g.x, g.z), "no offset (K-264)");
        assert_eq!(o.target, c, "the occupant keeps its reference");
        assert_eq!(o.anim.wanted, 20, "150004[0]");
        assert_ne!(o.status & status::ANIM_LOCKED, 0);
        assert!(riding(&sim, w, RIDING_BOAT).unwrap().busy);
        let paddler = sim.state.events[created..]
            .iter()
            .find_map(|e| match e {
                crate::state::SimEvent::Created { id } => Some(*id),
                _ => None,
            })
            .expect("paddler created");
        let p = sim.object(paddler).unwrap();
        assert_eq!((p.guid, p.id.slot()), (WARRIOR, 0));
        assert_eq!(p.color, crate::setup::player_color(&sim, 0), "player color");
        assert_ne!(p.color, 5, "not the color of the canoe");
        assert_eq!((p.grid.x, p.grid.z), (g.x, g.z));
        assert_eq!(p.rotation[1].to_bits(), rot.to_bits());
        assert_eq!(p.anim.wanted, 3, "dying animation of the canoe");
        assert_ne!(
            p.status & status::ANIM_LOCKED,
            0,
            "one-shot animation 0x502"
        );
        assert_eq!(p.attributes.current_value(14, 0), 100, "full hit points");
        assert!(p.selectable && p.status & status::DIED == 0);
        assert_ne!(
            sim.module(paddler, 2).unwrap().base.flags & crate::module::stand::ONWATER,
            0
        );
        (w, c, g, paddler)
    };
    through_phase_0(&mut state, &rules);
    assert_loads(&state, &rules);
    let sim = Sim::new(&mut state, &rules);
    assert!(sim.object(c).is_none(), "canoe destroyed");
    assert!(sim.object(paddler).is_some());
    assert_eq!(count_at(&sim, WARRIOR, g.x.0, g.z.0), 2, "two warriors");
    // Time marker 300 ms: the timer counts from the second tick and fires at > 300, so in the
    // fifth object tick after the sinking.
    for _ in 0..3 {
        object_tick(&mut state, &rules);
    }
    assert_ne!(
        state.object(w, false).unwrap().status & status::ANIM_LOCKED,
        0
    );
    object_tick(&mut state, &rules);
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(
        sim.object(w).unwrap().status & status::ANIM_LOCKED,
        0,
        "0x80E"
    );
    assert!(!riding(&sim, w, RIDING_BOAT).unwrap().busy);
}

#[test]
fn an_empty_sinking_canoe_still_creates_a_paddler() {
    // K-265: PRI canoe 1712 without occupant dies -> new 1700 at the canoe position.
    let rules = rules();
    let mut state = state(&rules);
    let g = {
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, PRI_CANOE, 0, (20, 20));
        let g = sim.object(c).unwrap().grid;
        kill(&mut sim, c);
        g
    };
    through_phase_0(&mut state, &rules);
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(count_at(&sim, PRI_WARRIOR, g.x.0, g.z.0), 1);
    assert_eq!(count_at(&sim, PRI_CANOE, g.x.0, g.z.0), 0);
}

#[test]
fn the_dead_passenger_branch_of_sink_canoe_moves_half_a_tile_back() {
    // K-264, dead branch of `0x0050EBA0`: reachable only if the canoe still has a target (never
    // after `unlink`). Called directly: `0x302`, selectable, position - 2048 * (cos, sin) of the
    // canoe direction (90 degrees -> z - 2048), height stays.
    let rules = rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let (w, c) = warrior_in_canoe(&mut sim, (10, 10), 90 << 12);
    sim.object_mut(w).unwrap().selectable = false;
    let g = sim.object(w).unwrap().grid;
    crate::module::dying::sink_canoe(&mut sim, c);
    let o = sim.object(w).unwrap();
    assert_eq!((o.grid.x.0, o.grid.z.0), (g.x.0, g.z.0 - 2048));
    assert!(o.selectable);
    assert_ne!(
        sim.module(w, 2).unwrap().base.flags & crate::module::stand::ONWATER,
        0
    );
    assert!(sim.state.destroy_queue.contains(&c));
}

#[test]
fn quartering_dismounts_riders_and_refuses_pure_boat_passengers() {
    // re/riding 4.5/W-204: mounted Dragoon in front of an own building with space -> dismounting
    // synchronously via 0x20D, then quartering; horse stays free outside. WID warrior in canoe ->
    // rejected (no module 13). PRI warrior in canoe -> disembarks via Riding: 0x2000 cleared instead
    // of 0x8000, canoe free, 0x8000 stays until the next object tick (step 11 clears it, target -1).
    use crate::module::quarter::accept_unit;
    let rules = rules();
    let mut state = state(&rules);
    let (pri, pc) = {
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (30, 30));
        let hg = sim.object(hq).unwrap().grid;
        let (r, h) = placed_dragoon(&mut sim, (20, 30), 0, 1024);
        let horse_at = sim.object(h).unwrap().grid;
        assert_eq!(accept_unit(&mut sim, hq, r), 0, "dismounted and quartered");
        let (o, m) = (sim.object(r).unwrap(), sim.object(h).unwrap());
        assert_eq!(o.status & status::ISMOUNTED, 0);
        assert_ne!(o.status & status::QUARTERED, 0);
        assert_eq!((o.carrier, o.grid.x, o.grid.z), (hq, hg.x, hg.z));
        assert_eq!(m.status & (status::QUARTERED | status::CARRIING), 0);
        assert_eq!(m.grid, horse_at, "horse stays outside");

        let (w, c) = warrior_in_canoe(&mut sim, (20, 34), 0);
        assert_eq!(accept_unit(&mut sim, hq, w), 1, "pure boat type rejected");
        assert_eq!(sim.object(w).unwrap().status & status::QUARTERED, 0);
        let (o, k) = (sim.object(w).unwrap(), sim.object(c).unwrap());
        assert_ne!(o.status & status::ISMOUNTEDONBOAT, 0, "stays in the canoe");
        assert_eq!((o.target, k.target), (c, w));

        let pri = spawn(&mut sim, PRI_WARRIOR, 0, (20, 38));
        let pc = spawn(&mut sim, PRI_CANOE, 0, (20, 38));
        link(&mut sim, pri, pc);
        assert_eq!(accept_unit(&mut sim, hq, pri), 0);
        let (o, k) = (sim.object(pri).unwrap(), sim.object(pc).unwrap());
        assert_eq!(
            o.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT | status::QUARTERED),
            status::ISMOUNTEDONBOAT | status::QUARTERED,
            "0x2000 cleared instead of 0x8000 (re/riding 8.8)"
        );
        assert_eq!(o.target, ObjectId::NONE);
        assert_eq!((k.status & status::CARRIING, k.target), (0, ObjectId::NONE));
        (pri, pc)
    };
    assert_loads(&state, &rules);
    object_tick(&mut state, &rules);
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(
        sim.object(pri).unwrap().status & status::ISMOUNTEDONBOAT,
        0,
        "step 11"
    );
    assert_eq!(sim.object(pc).unwrap().status & status::CARRIING, 0);
}

#[test]
fn reachable_link_states_load() {
    // Saving/loading after: horse death (rider refers to the carcass without a bit, before and after
    // phase 0), canoe sinking (occupant refers to a destroyed object), W-204 (0x8000 with
    // target -1 until the next tick): all load.
    let rules = rules();
    let mut state = state(&rules);
    let (w, c, pri) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (_, h) = placed_dragoon(&mut sim, (10, 10), 0, 1024);
        kill(&mut sim, h);
        let (w, c) = warrior_in_canoe(&mut sim, (20, 10), 0);
        kill(&mut sim, c);
        let hq = spawn(&mut sim, HQ, 0, (30, 30));
        let pri = spawn(&mut sim, PRI_WARRIOR, 0, (20, 30));
        let pc = spawn(&mut sim, PRI_CANOE, 0, (20, 30));
        link(&mut sim, pri, pc);
        assert_eq!(crate::module::quarter::accept_unit(&mut sim, hq, pri), 0);
        (w, c, pri)
    };
    let o = state.object(pri, false).unwrap();
    assert_eq!(
        (o.status & status::ISMOUNTEDONBOAT, o.target),
        (status::ISMOUNTEDONBOAT, ObjectId::NONE)
    );
    assert_loads(&state, &rules);
    step(&mut state, &rules, &[]);
    assert_loads(&state, &rules);
    step(&mut state, &rules, &[]);
    assert!(state.object(c, false).is_none());
    assert_eq!(state.object(w, false).unwrap().target, c);
    assert_loads(&state, &rules);
}

/// Group of two canoes with occupants and a horse with rider (walk command to occupants and
/// rider, `is_member` maps them to the carriers, re/riding 13.2). After that in `step` calls:
/// canoe 1 sinks with occupant (K-264/K-265), the horse dies, the occupant of canoe 2 dies (the
/// canoe dies for it, C-148), second walk command to all three. Every state loads; with
/// `reload` it is saved and loaded anew after each step. Returns the checksums per step.
fn sinking_group_run(reload: bool) -> Vec<u64> {
    const SINK_AT: usize = 4;
    const HORSE_DIES_AT: usize = 8;
    const PASSENGER_DIES_AT: usize = 12;
    const SECOND_MOVE_AT: usize = 16;
    let rules = rules();
    let mut state = state(&rules);
    let (w1, c1, w2, c2, r, h) = {
        let mut sim = Sim::new(&mut state, &rules);
        let (w1, c1) = warrior_in_canoe(&mut sim, (10, 10), 0);
        let (w2, c2) = warrior_in_canoe(&mut sim, (14, 10), 0);
        let (r, h) = rider_on_horse(&mut sim, COLONIST, (18, 10));
        (w1, c1, w2, c2, r, h)
    };
    let units = [w1, w2, r];
    let start = [w1, r].map(|id| tile_of(&state, id));
    let mut out = Vec::new();
    for i in 0..80 {
        let victim = match i {
            SINK_AT => Some(c1),
            HORSE_DIES_AT => Some(h),
            PASSENGER_DIES_AT => Some(w2),
            _ => None,
        };
        if let Some(v) = victim {
            kill(&mut Sim::new(&mut state, &rules), v);
        }
        let orders = match i {
            0 => vec![mv(&units, (30, 20), 0, NO_DIRECTION)],
            SECOND_MOVE_AT => vec![mv(&units, (30, 30), 0, NO_DIRECTION)],
            _ => vec![],
        };
        step(&mut state, &rules, &orders);
        if i == 0 {
            let sim = Sim::new(&mut state, &rules);
            let g = crate::group::find(&sim, 0).expect("group 0");
            assert!(
                [c1, c2, h].iter().all(|m| g.members.contains(m)),
                "carriers instead of occupants and riders"
            );
        }
        assert_loads(&state, &rules);
        if reload {
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    let sim = Sim::new(&mut state, &rules);
    assert!(
        sim.object(c1).is_none() && sim.object(c2).is_none(),
        "both canoes gone"
    );
    let bits = status::ISMOUNTED | status::ISMOUNTEDONBOAT;
    for id in units {
        assert_eq!(sim.object(id).unwrap().status & bits, 0, "{id:?} free");
    }
    for (id, from) in [w1, r].into_iter().zip(start) {
        assert_ne!(
            tile_of(sim.state, id),
            from,
            "{id:?} walks itself after the 2nd command"
        );
    }
    out
}

#[test]
fn a_canoe_sinking_in_a_group_with_a_dying_horse_and_passenger_keeps_every_state_loadable() {
    // Counter-check: sinking, horse death and occupant death in a group; every state loads,
    // saving/loading in every step continues bit-identically.
    let reference = sinking_group_run(false);
    assert_eq!(sinking_group_run(true), reference);
}

/// IDs of the scene of `RIDING_GOLDEN`.
struct Scene {
    colonist: ObjectId,
    horse: ObjectId,
    warrior: ObjectId,
    canoe: ObjectId,
    diver: ObjectId,
}

/// Scene of the golden checksums on `lake_rules(-100.0)` (lake 20...40, land 16.0 above the water
/// level), slot 0: Colonist (8, 8) and horse (12, 8); WID warrior (18, 30) and canoe
/// (21, 30) at the shore; diver (24, 24) in the lake.
fn riding_scene() -> (SimState, Rules, Scene) {
    riding_scene_with_horse_slot(0)
}

/// Scene like [`riding_scene`], the horse belongs to slot `slot` (1: foreign horse, takeover
/// via `0x47F`).
fn riding_scene_with_horse_slot(slot: i32) -> (SimState, Rules, Scene) {
    let rules = lake_rules(-100.0);
    let mut state = state(&rules);
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        let scene = Scene {
            colonist: spawn(&mut sim, COLONIST, 0, (8, 8)),
            horse: spawn(&mut sim, HORSE, slot, (12, 8)),
            warrior: spawn(&mut sim, WARRIOR, 0, (18, 30)),
            canoe: spawn(&mut sim, CANOE, 0, (21, 30)),
            diver: spawn(&mut sim, DIVER, 0, (24, 24)),
        };
        // Full stock as in re/swim-dive 9.2; spawn value 0 is K-236.
        sim.object_mut(scene.diver)
            .unwrap()
            .attributes
            .set_current_fraction(28, 100_000);
        scene
    };
    (state, rules, scene)
}

/// Step in which mounting, boarding and diving are commanded.
const MOUNT_AT: usize = 0;
/// Walking of the colonist (to (10, 50)) and of the warrior (to (35, 35)).
const WALK_AT: usize = 120;
/// Surfacing of the diver.
const SURFACE_AT: usize = 240;
/// Dismounting of the colonist and disembarking of the warrior.
const RELEASE_AT: usize = 520;
/// Length of the scene in `step` calls (two make one object tick).
const RIDING_STEPS: usize = 800;

/// Commands in the `step` call `i` (not object tick: two `step`s make one tick).
fn riding_orders(i: usize, s: &Scene) -> Vec<Command> {
    let cmd = |kind| Command { player: 0, kind };
    match i {
        MOUNT_AT => vec![
            cmd(CommandKind::Mount {
                horse: s.horse,
                units: vec![s.colonist],
            }),
            cmd(CommandKind::BoardBoat {
                boat: s.canoe,
                units: vec![s.warrior],
            }),
            cmd(CommandKind::Dive {
                units: vec![s.diver],
            }),
        ],
        WALK_AT => vec![
            mv(&[s.colonist], (10, 50), 1, NO_DIRECTION),
            mv(&[s.warrior], (35, 35), 1, NO_DIRECTION),
        ],
        SURFACE_AT => vec![cmd(CommandKind::Surface {
            units: vec![s.diver],
        })],
        RELEASE_AT => vec![
            cmd(CommandKind::Dismount {
                units: vec![s.colonist],
            }),
            cmd(CommandKind::LeaveBoat {
                units: vec![s.warrior],
            }),
        ],
        _ => vec![],
    }
}

/// State assertion at the time of saving: state, rules, IDs of the scene.
type Probe = fn(&mut SimState, &Rules, &Scene);

/// Checksum per `step` call of the scene with the horse of slot `horse_slot`. With
/// `save_at = Some(i)` it is saved and loaded anew after the `step` call `i`; `at_save` sees
/// the state immediately before.
fn riding_run_probed(horse_slot: i32, save_at: Option<usize>, at_save: Probe) -> Vec<u64> {
    let (mut state, rules, scene) = riding_scene_with_horse_slot(horse_slot);
    let mut out = Vec::with_capacity(RIDING_STEPS);
    for i in 0..RIDING_STEPS {
        step(&mut state, &rules, &riding_orders(i, &scene));
        if save_at == Some(i) {
            at_save(&mut state, &rules, &scene);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

/// Checksum sequence of the scene with an own horse.
fn riding_run(save_at: Option<usize>) -> Vec<u64> {
    riding_run_probed(0, save_at, |_, _, _| {})
}

/// State of the diving module (0 up, 1 down, 2 sinking, 3 rising).
fn dive_state(sim: &Sim, id: ObjectId) -> i32 {
    crate::module::diving::diving(sim, id).unwrap().status
}

/// Tile of the object.
fn tile_of(state: &SimState, id: ObjectId) -> (i32, i32) {
    let o = state.object(id, false).expect("object present");
    (o.grid.x.0 >> 12, o.grid.z.0 >> 12)
}

#[test]
fn the_riding_scene_shows_every_state() {
    // Counter-check before entering the golden: the scene shows what it should show. Times
    // are `step` calls (two per object tick).
    let (mut state, rules, s) = riding_scene();
    let (mut approach, mut mounted_at, mut boarded_at) = (None, None, None);
    let mut dive_states: Vec<i32> = Vec::new();
    for i in 0..RIDING_STEPS {
        step(&mut state, &rules, &riding_orders(i, &s));
        let sim = Sim::new(&mut state, &rules);
        let c = sim.object(s.colonist).unwrap();
        let w = sim.object(s.warrior).unwrap();
        if riding(&sim, s.colonist, RIDING).unwrap().going_to_horse && approach.is_none() {
            // Approach: FINDTO_UNIT on the horse runs, no link yet.
            assert_eq!(walk::walk(&sim, s.colonist).unwrap().quarry, s.horse);
            assert_eq!(c.status & status::ISMOUNTED, 0);
            approach = Some(i);
        }
        if c.status & status::ISMOUNTED != 0 && mounted_at.is_none() {
            assert_eq!((c.target, c.status & status::ISMOUNTEDONBOAT), (s.horse, 0));
            mounted_at = Some(i);
        }
        if w.status & status::ISMOUNTEDONBOAT != 0 && boarded_at.is_none() {
            assert_eq!(w.target, s.canoe);
            boarded_at = Some(i);
        }
        let d = dive_state(&sim, s.diver);
        if dive_states.last() != Some(&d) {
            dive_states.push(d);
        }
        // Before releasing: the colonist rides, the warrior travels by canoe, the diver is up.
        if i == RELEASE_AT - 1 {
            let on_horse = c.status & (status::ISMOUNTED | status::SWIMMING);
            assert_eq!(on_horse, status::ISMOUNTED);
            assert_eq!(w.status & status::ISMOUNTEDONBOAT, status::ISMOUNTEDONBOAT);
            assert_eq!(tile_of(sim.state, s.canoe), (35, 35), "canoe at the target");
            assert_eq!(
                tile_of(sim.state, s.warrior),
                (35, 35),
                "occupant at the canoe"
            );
            let h = tile_of(sim.state, s.horse);
            assert_eq!(tile_of(sim.state, s.colonist), h, "rider on the horse");
            assert!(h.1 > 8, "the horse carries the rider south: {h:?}");
            assert_eq!(sim.object(s.diver).unwrap().status & status::DIVING, 0);
        }
    }
    let sim = Sim::new(&mut state, &rules);
    // Colonist: approach, then `0x2000`; warrior: `0x8000` (before the walk command); diver:
    // sinks, down, rises, up.
    let (approach, mounted_at) = (approach.unwrap(), mounted_at.unwrap());
    assert!(
        approach < mounted_at && mounted_at < WALK_AT,
        "{approach} {mounted_at}"
    );
    assert!(boarded_at.unwrap() < WALK_AT, "{boarded_at:?}");
    assert_eq!(dive_states, [2, 1, 3, 0]);
    // After `RELEASE_AT`: colonist free on land, warrior swims in the lake, canoe and horse free.
    let c = sim.object(s.colonist).unwrap();
    let free = c.status & (status::ISMOUNTED | status::SWIMMING);
    assert_eq!((free, c.target), (0, ObjectId::NONE));
    assert!(
        tile_of(sim.state, s.colonist).0 < 20,
        "land west of the lake"
    );
    let w = sim.object(s.warrior).unwrap();
    let swims = w.status & (status::ISMOUNTEDONBOAT | status::SWIMMING);
    assert_eq!((swims, w.target), (status::SWIMMING, ObjectId::NONE));
    assert_eq!(tile_of(sim.state, s.warrior), (35, 35));
    for mount in [s.horse, s.canoe] {
        let carrying = sim.object(mount).unwrap().status & status::CARRIING;
        assert_eq!(carrying, 0, "{mount:?}");
    }
    let d = sim.object(s.diver).unwrap();
    assert_eq!(
        d.status & (status::DIVING | status::SWIMMING),
        status::SWIMMING
    );
}

/// Measured (mounting, riding, canoe, diving, dismounting, swimming).
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
const RIDING_GOLDEN: u64 = 0xcc80_e737_711f_0a14;

#[test]
fn riding_checksums_match_the_golden_sequence() {
    let seq = riding_run(None);
    let mut h = crate::rules::Fnv64::default();
    for c in &seq {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, RIDING_GOLDEN, "{:#018x}", h.0);
}

/// Savegames of the scene: in the approach, mounted/in the canoe/dived, diver surfaces.
const SAVE_IN_APPROACH: usize = 20;
const SAVE_ALL_ENGAGED: usize = 100;
const SAVE_SURFACING: usize = 244;
/// Foreign horse: the change of owner is queued but not yet processed.
const SAVE_OWNER_CHANGE: usize = 50;

#[test]
fn riding_checksums_survive_saving_and_loading() {
    let golden = riding_run(None);
    // In the approach: FINDTO_UNIT on the horse runs, no link.
    let in_approach = riding_run_probed(0, Some(SAVE_IN_APPROACH), |state, rules, s| {
        let sim = Sim::new(state, rules);
        assert!(riding(&sim, s.colonist, RIDING).unwrap().going_to_horse);
        assert_eq!(walk::walk(&sim, s.colonist).unwrap().quarry, s.horse);
        assert_eq!(
            sim.object(s.colonist).unwrap().status & status::ISMOUNTED,
            0
        );
    });
    assert_eq!(in_approach, golden, "saved in the approach");
    // All three in a link or dived (state 1).
    let engaged = riding_run_probed(0, Some(SAVE_ALL_ENGAGED), |state, rules, s| {
        let sim = Sim::new(state, rules);
        let (c, w, d) = (
            sim.object(s.colonist).unwrap(),
            sim.object(s.warrior).unwrap(),
            sim.object(s.diver).unwrap(),
        );
        assert_eq!((c.target, w.target), (s.horse, s.canoe));
        assert_ne!(c.status & status::ISMOUNTED, 0);
        assert_ne!(w.status & status::ISMOUNTEDONBOAT, 0);
        assert_ne!(d.status & status::DIVING, 0);
        assert_eq!(dive_state(&sim, s.diver), 1);
    });
    assert_eq!(engaged, golden, "saved mounted, in the canoe, dived");
    // The diver rises (state 3, still under water).
    let surfacing = riding_run_probed(0, Some(SAVE_SURFACING), |state, rules, s| {
        let sim = Sim::new(state, rules);
        let d = sim.object(s.diver).unwrap();
        assert_eq!(dive_state(&sim, s.diver), 3);
        assert_eq!(d.status & status::DIVING, 0);
        assert!(d.height_offset < 0);
    });
    assert_eq!(surfacing, golden, "saved while surfacing");
}

#[test]
fn riding_checksums_survive_saving_in_the_dive_descent() {
    // Counter-check: saving while the diver sinks (state 2: `step` calls `MOUNT_AT` ...
    // `MOUNT_AT + 9`, after that it is down), at the start, in the middle and at the end of the
    // descent; the sequence continues bit-identically.
    let golden = riding_run(None);
    for at in [MOUNT_AT, MOUNT_AT + 5, MOUNT_AT + 9] {
        let saved = riding_run_probed(0, Some(at), |state, rules, s| {
            let sim = Sim::new(state, rules);
            assert_eq!(dive_state(&sim, s.diver), 2, "diver sinks");
            assert!(sim.object(s.diver).unwrap().status & status::DIVING != 0);
        });
        assert_eq!(saved, golden, "saved in the descent after step {at}");
    }
}

#[test]
fn riding_checksums_survive_saving_with_a_foreign_horse_mid_takeover() {
    // Foreign horse (slot 1): the change of owner is queued in the object tick and is not processed until
    // phase 0 of the next step (`tick.rs:67`). Saving in between and not saving give the same
    // sequence.
    let unsaved = riding_run_probed(1, None, |_, _, _| {});
    let saved = riding_run_probed(1, Some(SAVE_OWNER_CHANGE), |state, _, s| {
        assert_eq!(state.owner_changes.len(), 1);
        assert_eq!(state.owner_changes[0].0, s.horse);
        let horse = state.object(s.horse, false).unwrap();
        assert_eq!(horse.owner_change_target, s.colonist);
    });
    assert_eq!(saved, unsaved, "saved between queuing and processing");
    assert_ne!(
        unsaved,
        riding_run(None),
        "the foreign horse changes the flow"
    );
}
