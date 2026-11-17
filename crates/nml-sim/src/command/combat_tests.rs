//! Combat commands: attack `0x11`/`0x71`, behavior `0x12`, garrison attack `0x5B`, hunt/gut `0x16`/`0x17`, follow
//! `0x74` and the right-click assignment (410/411/418/419/427/428).

use crate::combat::testkit as ck;
use crate::id::ObjectId;
use crate::message::{Message, Posted};
use crate::sim::Sim;
use crate::state::SimEvent;

use super::{Command, CommandKind};

#[test]
fn attack_posts_0x111_with_modifier_flags_and_answers_14() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let t = ck::spawn(&mut sim, ck::WARRIOR, 1, (9, 4));
    sim.state.events.clear();
    ck::run(
        &mut sim,
        0,
        CommandKind::Attack {
            units: vec![a],
            target: t,
            pos: None,
            mods: 4 | 1,
        },
    );
    let posted: Vec<_> = sim.object(a).unwrap().fifo.iter().cloned().collect();
    assert_eq!(
        posted,
        vec![Posted::Message {
            msg: Message::new(0x111, &[t.as_i32(), 0x500]),
            abort: true
        }]
    );
    assert!(sim.state.events.contains(&SimEvent::UnitResponse {
        unit: a,
        action: 14,
        owner: 0,
        guid: ck::SABRE
    }));
    // The command does not accept foreign units (A8).
    ck::run(
        &mut sim,
        1,
        CommandKind::Attack {
            units: vec![a],
            target: t,
            pos: None,
            mods: 0,
        },
    );
    assert_eq!(sim.object(a).unwrap().fifo.len(), 1);
}

#[test]
fn attack_with_a_position_walks_there_when_the_target_is_gone() {
    // Command 0x71 (fog memory): target gone at execution -> FINDTO to the position (x = px / 128), deaf.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let a = ck::spawn(&mut Sim::new(&mut state, &rules), ck::SABRE, 0, (4, 4));
    let px = ((12 << 12) + 0x800) * 128;
    let cmd = Command {
        player: 0,
        kind: CommandKind::Attack {
            units: vec![a],
            target: ObjectId::from_i32(0x1234),
            pos: Some([px, 0, ((4 << 12) + 0x800) * 128]),
            mods: 0,
        },
    };
    crate::tick::step(&mut state, &rules, &[cmd]);
    for _ in 0..4 {
        crate::tick::step(&mut state, &rules, &[]);
    }
    let o = state.object(a, false).unwrap();
    assert!(
        o.status & crate::object::status::MOVE != 0,
        "walks to the ground position"
    );
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(
        crate::module::attack::attack(&sim, a).unwrap().stubbornness[2],
        4,
        "normal march (C-13)"
    );
}

#[test]
fn set_behavior_sets_attack_mode_walk_behaviour_and_building_mode() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    state.players[1].control = crate::state::Control::Ai;
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let hq = ck::spawn(&mut sim, ck::HQ, 0, (12, 12));
    ck::run(
        &mut sim,
        0,
        CommandKind::SetBehavior {
            units: vec![a, hq],
            mode: 4,
        },
    );
    let s = crate::module::attack::attack(&sim, a).unwrap();
    assert_eq!((s.aggressiveness, s.mode, s.stubbornness[0]), (4, 4, 1));
    assert_eq!(crate::module::walk::walk(&sim, a).unwrap().tether, 8 << 12);
    assert_eq!(
        crate::messaging::send(&mut sim, hq, &Message::new(0x3603, &[])),
        4
    );
    // AI player: mode 3 -> {4, 3, 2}.
    let k = ck::spawn(&mut sim, ck::SABRE, 1, (20, 20));
    ck::run(
        &mut sim,
        1,
        CommandKind::SetBehavior {
            units: vec![k],
            mode: 3,
        },
    );
    assert_eq!(
        crate::module::attack::attack(&sim, k).unwrap().stubbornness[0],
        2
    );
}

#[test]
fn set_behavior_outside_0_to_4_is_refused_directly_and_queued() {
    // Deviation (exception A8): the original does not check the mode (`0x004762E0`); mode 5 would yield an
    // attack state that loading rejects.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let hq = ck::spawn(&mut sim, ck::HQ, 0, (12, 12));
    let before = *crate::module::attack::attack(&sim, a).unwrap();
    let hq_mode = crate::messaging::send(&mut sim, hq, &Message::new(0x3603, &[]));
    for mode in [5, -1, i32::MAX] {
        let set = CommandKind::SetBehavior {
            units: vec![a, hq],
            mode,
        };
        ck::run(&mut sim, 0, set.clone());
        ck::run(
            &mut sim,
            0,
            CommandKind::Queue {
                cyclic: false,
                inner: Box::new(set),
            },
        );
    }
    let s = crate::module::attack::attack(&sim, a).unwrap();
    assert_eq!(*s, before);
    assert!(sim.object(a).unwrap().commands.is_empty());
    assert_eq!(
        crate::messaging::send(&mut sim, hq, &Message::new(0x3603, &[])),
        hq_mode
    );
}

#[test]
fn garrison_attack_posts_0x1b17_to_the_buildings() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = ck::spawn(&mut sim, ck::HQ, 0, (12, 12));
    let t = ck::spawn(&mut sim, ck::WARRIOR, 1, (16, 12));
    ck::run(
        &mut sim,
        0,
        CommandKind::GarrisonAttack {
            buildings: vec![hq],
            target: t,
        },
    );
    let posted: Vec<_> = sim.object(hq).unwrap().fifo.iter().cloned().collect();
    assert_eq!(
        posted,
        vec![Posted::Message {
            msg: Message::new(0x1B17, &[t.as_i32()]),
            abort: true
        }]
    );
}

#[test]
fn right_click_411_attacks_and_x_mode_attacks_or_marches() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let t = ck::spawn(&mut sim, ck::WARRIOR, 1, (9, 4));
    let pick = crate::targeting::pick_target_action(&mut sim, 0, &[a], Some(t), (0, 0));
    assert_eq!(
        pick,
        Some((
            411,
            CommandKind::Attack {
                units: vec![a],
                target: t,
                pos: None,
                mods: 0
            }
        ))
    );
    let ground = ((20 << 12) + 0x800, (20 << 12) + 0x800);
    assert_eq!(
        crate::targeting::attack_actions(&mut sim, 0, &[a], Some(t), ground),
        vec![CommandKind::Attack {
            units: vec![a],
            target: t,
            pos: None,
            mods: 4
        }]
    );
    let march = crate::targeting::attack_actions(&mut sim, 0, &[a], None, ground);
    assert_eq!(
        march[0],
        CommandKind::SetBehavior {
            units: vec![a],
            mode: 3
        }
    );
    assert!(matches!(&march[1], CommandKind::Move { flags: 4, x, z, .. } if (*x, *z) == ground));
}

#[test]
fn set_behavior_is_queued_per_unit_and_the_other_combat_commands_run_at_once() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
    let b = ck::spawn(&mut sim, ck::SABRE, 0, (6, 4));
    let t = ck::spawn(&mut sim, ck::WARRIOR, 1, (9, 4));
    let inner = CommandKind::SetBehavior {
        units: vec![a, b],
        mode: 4,
    };
    ck::run(
        &mut sim,
        0,
        CommandKind::Queue {
            cyclic: false,
            inner: Box::new(inner),
        },
    );
    for u in [a, b] {
        let q = &sim.object(u).unwrap().commands;
        assert_eq!(q.len(), 1);
        assert_eq!(
            q[0].command.kind,
            CommandKind::SetBehavior {
                units: vec![u],
                mode: 4
            }
        );
        assert_ne!(crate::module::attack::attack(&sim, u).unwrap().mode, 4);
    }
    assert!(super::process_queue(&mut sim, a));
    assert_eq!(crate::module::attack::attack(&sim, a).unwrap().mode, 4);
    assert_ne!(crate::module::attack::attack(&sim, b).unwrap().mode, 4);
    // Attacking with Shift runs at once and without acknowledgment (not direct); the queue of
    // `b` stays.
    sim.state.events.clear();
    ck::run(
        &mut sim,
        0,
        CommandKind::Queue {
            cyclic: false,
            inner: Box::new(CommandKind::Attack {
                units: vec![b],
                target: t,
                pos: None,
                mods: 0,
            }),
        },
    );
    assert_eq!(sim.object(b).unwrap().fifo.len(), 1);
    assert_eq!(sim.object(b).unwrap().commands.len(), 1, "queue stays");
}

#[test]
fn attacking_a_fog_memory_carries_its_position() {
    let rules = ck::rules();
    let mut state = crate::command::testkit::fogged_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let a = ck::spawn(&mut sim, ck::SABRE, 0, (10, 10));
    let t = ck::spawn(&mut sim, ck::HQ, 1, (40, 40));
    let snap = crate::fog::MemorySnapshot::take(sim.object(t).unwrap(), (40, 40), None);
    let (b, cmd) = crate::targeting::memory_target_action(&mut sim, 0, &[a], &snap).unwrap();
    assert_eq!(b, 411);
    let world = |t: i32| crate::command::testkit::world(t) * 4096.0;
    assert_eq!(
        cmd,
        CommandKind::Attack {
            units: vec![a],
            target: t,
            pos: Some([world(40) as i32, 0, world(40) as i32]),
            mods: 0
        }
    );
    assert_eq!(cmd.type_code(), 0x5000_0071);
}

#[test]
fn hunt_and_gut_post_0x216_only_when_direct() {
    // re/commands 1.8 no. 4 (K-81/W-68), C-182: 0x16 and 0x17 are the same; no acknowledgment.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let h = ck::spawn(&mut sim, ck::HUNTER, 0, (4, 4));
    let g = ck::spawn(&mut sim, ck::GOAT, 8, (9, 4));
    let w = ck::spawn(&mut sim, ck::WARRIOR, 1, (9, 8));
    for gut in [false, true] {
        sim.object_mut(h).unwrap().fifo.clear();
        sim.state.events.clear();
        let kind = CommandKind::Hunt {
            animal: g,
            units: vec![h],
            gut,
        };
        assert_eq!(
            kind.type_code(),
            if gut { 0x5000_0016 } else { 0x5000_0017 }
        );
        ck::run(&mut sim, 0, kind);
        let posted: Vec<_> = sim.object(h).unwrap().fifo.iter().cloned().collect();
        assert_eq!(
            posted,
            vec![Posted::Message {
                msg: Message::new(0x216, &[g.as_i32()]),
                abort: true
            }]
        );
        assert!(
            !sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::UnitResponse { .. }))
        );
    }
    // Via the queue (Shift): nothing.
    sim.object_mut(h).unwrap().fifo.clear();
    ck::run(
        &mut sim,
        0,
        CommandKind::Queue {
            cyclic: false,
            inner: Box::new(CommandKind::Hunt {
                animal: g,
                units: vec![h],
                gut: false,
            }),
        },
    );
    assert!(sim.object(h).unwrap().fifo.is_empty());
    assert!(sim.object(h).unwrap().commands.is_empty());
    // A8: foreign unit and target without module 21 -> rejected.
    ck::run(
        &mut sim,
        1,
        CommandKind::Hunt {
            animal: g,
            units: vec![h],
            gut: false,
        },
    );
    ck::run(
        &mut sim,
        0,
        CommandKind::Hunt {
            animal: w,
            units: vec![h],
            gut: false,
        },
    );
    assert!(sim.object(h).unwrap().fifo.is_empty());
}

#[test]
fn follow_posts_0x3900_and_walk_escorts_with_5_degrees() {
    // re/walk 15.4; K-193: tolerance Fixed(180) lies outside 0...180 -> 5 degrees.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (a, t) = {
        let mut sim = Sim::new(&mut state, &rules);
        let a = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
        let t = ck::spawn(&mut sim, ck::SABRE, 0, (12, 4));
        sim.state.events.clear();
        ck::run(
            &mut sim,
            0,
            CommandKind::Follow {
                target: t,
                units: vec![a],
            },
        );
        let posted: Vec<_> = sim.object(a).unwrap().fifo.iter().cloned().collect();
        assert_eq!(
            posted,
            vec![Posted::Message {
                msg: Message::new(0x3900, &[t.as_i32()]),
                abort: true
            }]
        );
        assert!(
            !sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::UnitResponse { .. }))
        );
        (a, t)
    };
    crate::tick::step(&mut state, &rules, &[]);
    let sim = Sim::new(&mut state, &rules);
    let w = crate::module::walk::walk(&sim, a).unwrap();
    assert!(w.escort, "follow permanently (flag 0x2000)");
    assert_eq!(w.quarry, t);
    assert_eq!(w.tolerance, 5 << 12);
    assert_eq!(
        CommandKind::Follow {
            target: t,
            units: vec![]
        }
        .type_code(),
        0x5000_0074
    );
}

#[test]
fn follow_runs_at_once_from_the_queue_and_may_target_the_unit_itself() {
    // execute_queued: "everything else runs at once" without direct (queue stays); the original does not check
    // following itself - no panic.
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let a = {
        let mut sim = Sim::new(&mut state, &rules);
        let a = ck::spawn(&mut sim, ck::SABRE, 0, (4, 4));
        ck::run(
            &mut sim,
            0,
            CommandKind::Queue {
                cyclic: false,
                inner: Box::new(CommandKind::Follow {
                    target: a,
                    units: vec![a],
                }),
            },
        );
        assert_eq!(sim.object(a).unwrap().fifo.len(), 1);
        a
    };
    for _ in 0..30 {
        crate::tick::step(&mut state, &rules, &[]);
    }
    assert!(state.object(a, false).is_some());
}

#[test]
fn right_clicks_418_419_428_gut_hunt_and_follow() {
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (h, g, mate) = {
        let mut sim = Sim::new(&mut state, &rules);
        let h = ck::spawn(&mut sim, ck::HUNTER, 0, (4, 4));
        let g = ck::spawn(&mut sim, ck::GOAT, 8, (9, 4));
        let mate = ck::spawn(&mut sim, ck::SABRE, 0, (4, 9));
        (h, g, mate)
    };
    let pick = |state: &mut crate::state::SimState, t| {
        let mut sim = Sim::new(state, &rules);
        crate::targeting::pick_target_action(&mut sim, 0, &[h], Some(t), (0, 0))
    };
    assert_eq!(
        pick(&mut state, g),
        Some((
            419,
            CommandKind::Hunt {
                animal: g,
                units: vec![h],
                gut: false
            }
        ))
    );
    assert_eq!(
        pick(&mut state, mate),
        Some((
            428,
            CommandKind::Follow {
                target: mate,
                units: vec![h]
            }
        ))
    );
    // Carcass (after 0x215 with ISROT, dead) -> 418 Gut.
    {
        let mut sim = Sim::new(&mut state, &rules);
        sim.object_mut(g)
            .unwrap()
            .attributes
            .set_current_value(14, 0, 0);
    }
    crate::command::testkit::run_until(&mut state, &rules, 40, |sim| {
        sim.object(g).is_some_and(|o| o.render_bits == 0x42)
    });
    assert_eq!(
        pick(&mut state, g),
        Some((
            418,
            CommandKind::Hunt {
                animal: g,
                units: vec![h],
                gut: true
            }
        ))
    );
}
