//! Commands mount/dismount (`0x0A`/`0x0B`), board/disembark (`0x0C`/`0x0D`) and
//! dive/surface (`0x1B`/`0x1C`; re/riding 2.5, 13.7, re/swim-dive 2.3).
//! Like the other handlers: per unit in command order `PostMessage(msg, abort)`, with
//! `direct` then empty the command queue; only mount and board acknowledge.

use nml_formats::targets::MatchFilter;

use super::economy::{acknowledge, clear_queue, own_units};
use super::{CommandKind, reject, skip_passed};
use crate::collision::query_radius;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::post_message;
use crate::module::riding::RIDING;
use crate::object::status;
use crate::sim::Sim;

/// Mount (`0x10D`), dismount (`0x20D`), board (`0x10E`), disembark (`0x20E`), dive
/// (`0x118`) and surface (`0x218`).
const MOUNT: u32 = 0x10D;
const DISMOUNT: u32 = 0x20D;
const BOARD: u32 = 0x10E;
const LEAVE: u32 = 0x20E;
const DIVE: u32 = 0x118;
const SURFACE: u32 = 0x218;

/// Radius of the horse search of `0x0A`: `5 · 0x1000` (five tiles, fixed point 20.12).
const HORSE_SEARCH_RADIUS: i32 = 5 * 4096;

/// Mounted (`ISMOUNTED`) or in the boat (`ISMOUNTEDONBOAT`): `status & 0xA000`.
const MOUNTED_BITS: u32 = status::ISMOUNTED | status::ISMOUNTEDONBOAT;

/// Filter of the horse search: `{0x2016, 0, 0x4000, 0x8000, 0}` = `CHECKNOTSTATUS(CARRIING)` and
/// walk flag `SADDLE` and walk present (re/riding 2.5, 13.7).
fn horse_filter() -> MatchFilter {
    MatchFilter {
        mode: 0x2016,
        mode2: 0,
        status: status::CARRIING,
        match_bits: 0x8000,
        ..MatchFilter::constructed()
    }
}

/// The six commands; `kind` is one of them.
pub(super) fn execute(sim: &mut Sim, player: u8, kind: &CommandKind, direct: bool) {
    match kind {
        CommandKind::Mount { horse, units } => mount(sim, player, *horse, units, direct),
        CommandKind::BoardBoat { boat, units } => board_boat(sim, player, *boat, units, direct),
        CommandKind::Dismount { units } => post_all(sim, player, units, DISMOUNT, direct),
        CommandKind::LeaveBoat { units } => post_all(sim, player, units, LEAVE, direct),
        CommandKind::Dive { units } => post_all(sim, player, units, DIVE, direct),
        CommandKind::Surface { units } => post_all(sim, player, units, SURFACE, direct),
        _ => {}
    }
}

/// `0x0B` (`GameWorld_CmdDismount` `0x00475B10`), `0x0D` (`0x00475CD0`), `0x1B` (`0x00476960`) and
/// `0x1C` (`0x004769E0`): per unit `PostMessage({code}, abort)` without any check of the
/// module; with `direct` only empty the queue, **no** response sound.
fn post_all(sim: &mut Sim, player: u8, units: &[ObjectId], code: u32, direct: bool) {
    for u in own_units(sim, player, units) {
        post_message(sim, u, Message::new(code, &[]), true);
        if direct {
            clear_queue(sim, u);
        }
    }
}

/// `0x0C` (`GameWorld_CmdBoardBoat` `0x0047D2A0`): per unit `{0x10E, [boat]}` with abort, with
/// `direct` response sound `0xC` and empty the queue. The original does not check the target (K-224:
/// a foreign canoe works too).
///
/// Deviation (A8): if the target does not exist, `ObjectId::NONE` goes into the message instead of the ID.
/// For the original both are the same (`mount_start` aborts without an object, only the abort of the
/// running modules and the acknowledgment remain), but `mount_start` remembers the ID as the mount.
/// The reason was the former load check "mount ID invalid": an ID outside the object tables could be
/// saved but no longer loaded. That check no longer exists; the deviation stays.
fn board_boat(sim: &mut Sim, player: u8, boat: ObjectId, units: &[ObjectId], direct: bool) {
    let boat = if sim.object(boat).is_some() {
        boat
    } else {
        log::debug!(
            "Command \"Board\" from slot {player}: boat {} not present",
            boat.0
        );
        ObjectId::NONE
    };
    for u in own_units(sim, player, units) {
        post_message(sim, u, Message::new(BOARD, &[boat.as_i32()]), true);
        if direct {
            acknowledge(sim, u);
        }
    }
}

/// `0x0A` (`GameWorld_CmdMountHorse` `0x0047CFB0`, re/riding 13.7): the list is initially the
/// horse, if it does not carry. If it is shorter than the number of IDs in the command, it is
/// emptied and filled with all free saddle walkers around the horse (any owner).
/// Per unit with active module 13, without `0xA000` and with a non-empty list: take the head,
/// `{0x10D, [head]}` with abort, with `direct` response sound and empty the queue. Skipped
/// units leave the list as it is.
///
/// If the horse is missing, everything stays: the original checks it (`0x0047CFB0`: `GetObjectById` returns
/// nothing -> end without effect, no null pointer, E-154); the reimplementation reports this as a rejection
/// with a reason. That also avoids an ID without an object in the rider. On the script path (`script_path`,
/// 337) only the ownership of the riders is omitted ([`own_units`]); `Mount` with an already mounted horse
/// searches for a free one in the vicinity (`CARRIING`), a target without a saddle goes to `mount_start`.
fn mount(sim: &mut Sim, player: u8, horse: ObjectId, units: &[ObjectId], direct: bool) {
    let Some(h) = sim.object(horse) else {
        reject(
            sim,
            player,
            "Mount",
            format!("horse {} not present", horse.0),
        );
        return;
    };
    let (pos, carried) = ((h.grid.x.0, h.grid.z.0), h.status & status::CARRIING != 0);
    let mut list: Vec<ObjectId> = if carried { Vec::new() } else { vec![horse] };
    if list.len() < units.len() {
        list = query_radius(sim, pos, &horse_filter(), HORSE_SEARCH_RADIUS, -1, false);
    }
    for u in own_units(sim, player, units) {
        let ready = sim
            .object(u)
            .is_some_and(|o| o.has_module(RIDING, false) && o.status & MOUNTED_BITS == 0);
        if !ready || list.is_empty() {
            skip_passed(sim, player, "Mount", format!("unit {} cannot mount", u.0));
            continue;
        }
        let head = list.remove(0);
        post_message(sim, u, Message::new(MOUNT, &[head.as_i32()]), true);
        if direct {
            acknowledge(sim, u);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit::run;
    use crate::command::{Command, QueuedCommand, execute as execute_command};
    use crate::id::ObjectId;
    use crate::message::Posted;
    use crate::module::riding::testkit::*;
    use crate::object::status;
    use crate::state::{SimEvent, SimState};

    /// `(code, arguments, abort)` of the posted messages of a unit.
    fn posted(sim: &Sim, u: ObjectId) -> Vec<(u32, Vec<i32>, bool)> {
        sim.object(u)
            .unwrap()
            .fifo
            .iter()
            .map(|p| match p {
                Posted::Message { msg, abort } => (msg.code, msg.args.clone(), *abort),
                Posted::Command { env, abort } => (env.msg.code, env.msg.args.clone(), *abort),
            })
            .collect()
    }

    /// Command acknowledgments (`SimEvent::UnitResponse`) in order.
    fn responses(sim: &Sim) -> Vec<(ObjectId, i32)> {
        sim.state
            .events
            .iter()
            .filter_map(|e| match *e {
                SimEvent::UnitResponse { unit, action, .. } => Some((unit, action)),
                _ => None,
            })
            .collect()
    }

    /// One entry in the command queue (filler that the command is meant to empty).
    fn fill_queue(sim: &mut Sim, u: ObjectId) {
        let entry = QueuedCommand {
            cyclic: false,
            command: Command {
                player: 0,
                kind: CommandKind::Halt { units: vec![u] },
            },
        };
        crate::command::enqueue(sim, u, entry);
        assert_eq!(sim.object(u).unwrap().commands.len(), 1);
    }

    fn queue_len(sim: &Sim, u: ObjectId) -> usize {
        sim.object(u).unwrap().commands.len()
    }

    #[test]
    fn mount_sends_each_unit_to_one_horse_of_the_list() {
        // re/riding 13.7: two colonists, horse A (target, free) and horse B (2 tiles further): the list
        // [A] is shorter than n = 2, so it is emptied and refilled by a circle search around A; unit 1
        // gets the head, unit 2 the next one.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c1 = spawn(&mut sim, COLONIST, 0, (10, 10));
        let c2 = spawn(&mut sim, COLONIST, 0, (10, 12));
        let a = spawn(&mut sim, HORSE, 0, (14, 10));
        let b = spawn(&mut sim, HORSE, 0, (16, 10));
        fill_queue(&mut sim, c1);
        fill_queue(&mut sim, c2);
        run(
            &mut sim,
            0,
            CommandKind::Mount {
                horse: a,
                units: vec![c1, c2],
            },
        );
        assert_eq!(posted(&sim, c1), [(0x10D, vec![a.as_i32()], true)]);
        assert_eq!(posted(&sim, c2), [(0x10D, vec![b.as_i32()], true)]);
        // Direct: response 12 per unit in command order, queues empty.
        assert_eq!(responses(&sim), [(c1, 12), (c2, 12)]);
        assert_eq!((queue_len(&sim, c1), queue_len(&sim, c2)), (0, 0));
    }

    #[test]
    fn mount_with_enough_horses_in_the_list_does_not_search() {
        // n = 1: the list [A] is not shorter than n; the nearer horse B is ignored.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        let a = spawn(&mut sim, HORSE, 0, (20, 10));
        let _b = spawn(&mut sim, HORSE, 0, (11, 10));
        run(
            &mut sim,
            0,
            CommandKind::Mount {
                horse: a,
                units: vec![c],
            },
        );
        assert_eq!(posted(&sim, c), [(0x10D, vec![a.as_i32()], true)]);
    }

    // QUIRK K-224: kept
    #[test]
    fn mount_skips_carried_horses_and_takes_foreign_ones() {
        // The target A already carries (CARRIING): list empty, the circle search takes the free horse B
        // of a foreign player (any owner, K-224).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        let rider = spawn(&mut sim, DRAGOON, 0, (14, 10));
        let a = spawn(&mut sim, HORSE, 0, (14, 10));
        link(&mut sim, rider, a);
        let b = spawn(&mut sim, HORSE, 1, (16, 10));
        let far = spawn(&mut sim, HORSE, 0, (30, 30));
        run(
            &mut sim,
            0,
            CommandKind::Mount {
                horse: a,
                units: vec![c],
            },
        );
        assert_eq!(posted(&sim, c), [(0x10D, vec![b.as_i32()], true)]);
        assert!(posted(&sim, rider).is_empty() && posted(&sim, far).is_empty());
    }

    #[test]
    fn mount_skips_mounted_units_and_units_without_the_riding_module() {
        // `status & 0xA000` (mounted or in the boat) or no active module 13: skipped, the
        // list stays, the next unit takes the head. No response for skipped ones.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let mounted = spawn(&mut sim, COLONIST, 0, (10, 10));
        let boat_rider = spawn(&mut sim, COLONIST, 0, (10, 11));
        let no_module = spawn(&mut sim, WARRIOR, 0, (10, 12));
        let disabled = spawn(&mut sim, COLONIST, 0, (10, 13));
        let c = spawn(&mut sim, COLONIST, 0, (10, 14));
        let a = spawn(&mut sim, HORSE, 0, (14, 10));
        let mount = spawn(&mut sim, HORSE, 0, (30, 30));
        link(&mut sim, mounted, mount);
        sim.object_mut(boat_rider)
            .unwrap()
            .set_status_flags(status::ISMOUNTEDONBOAT, true);
        sim.module_mut(disabled, 13).unwrap().base.enabled = false;
        run(
            &mut sim,
            0,
            CommandKind::Mount {
                horse: a,
                units: vec![mounted, boat_rider, no_module, disabled, c],
            },
        );
        for u in [mounted, boat_rider, no_module, disabled] {
            assert!(posted(&sim, u).is_empty(), "{u:?}");
        }
        assert_eq!(posted(&sim, c), [(0x10D, vec![a.as_i32()], true)]);
        assert_eq!(responses(&sim), [(c, 12)]);
    }

    #[test]
    fn mount_with_more_units_than_horses_leaves_the_rest_without_orders() {
        // The list becomes empty: the third unit gets nothing (and no response).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c: Vec<_> = (0..3)
            .map(|i| spawn(&mut sim, COLONIST, 0, (10, 10 + 2 * i)))
            .collect();
        let a = spawn(&mut sim, HORSE, 0, (14, 10));
        let b = spawn(&mut sim, HORSE, 0, (16, 10));
        run(
            &mut sim,
            0,
            CommandKind::Mount {
                horse: a,
                units: c.clone(),
            },
        );
        assert_eq!(posted(&sim, c[0]), [(0x10D, vec![a.as_i32()], true)]);
        assert_eq!(posted(&sim, c[1]), [(0x10D, vec![b.as_i32()], true)]);
        assert!(posted(&sim, c[2]).is_empty());
        assert_eq!(responses(&sim), [(c[0], 12), (c[1], 12)]);
    }

    #[test]
    fn mount_from_the_queue_neither_acknowledges_nor_clears() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let c = spawn(&mut sim, COLONIST, 0, (10, 10));
        let a = spawn(&mut sim, HORSE, 0, (14, 10));
        fill_queue(&mut sim, c);
        let cmd = Command {
            player: 0,
            kind: CommandKind::Mount {
                horse: a,
                units: vec![c],
            },
        };
        execute_command(&mut sim, &cmd, false);
        assert_eq!(posted(&sim, c), [(0x10D, vec![a.as_i32()], true)]);
        assert!(responses(&sim).is_empty());
        assert_eq!(queue_len(&sim, c), 1);
    }

    // QUIRK K-224: kept
    #[test]
    fn board_boat_posts_without_checking_the_target() {
        // 0x0C: {0x10E, [boat]} per unit without a check (also a foreign canoe, K-224), response 12.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w1 = spawn(&mut sim, WARRIOR, 0, (10, 10));
        let w2 = spawn(&mut sim, DRAGOON, 0, (10, 12));
        let foreign = spawn(&mut sim, CANOE, 1, (14, 10));
        fill_queue(&mut sim, w1);
        run(
            &mut sim,
            0,
            CommandKind::BoardBoat {
                boat: foreign,
                units: vec![w1, w2],
            },
        );
        let expected = [(0x10E, vec![foreign.as_i32()], true)];
        assert_eq!(posted(&sim, w1), expected);
        assert_eq!(posted(&sim, w2), expected);
        assert_eq!(responses(&sim), [(w1, 12), (w2, 12)]);
        assert_eq!(queue_len(&sim, w1), 0);
    }

    #[test]
    fn dismount_leave_dive_and_surface_post_and_clear_the_queue_without_a_response() {
        // 0x0B {0x20D}, 0x0D {0x20E}, 0x1B {0x118}, 0x1C {0x218}: post with abort; with direct
        // the queue is empty, no UnitResponse; without direct the queue stays.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        type Make = fn(Vec<ObjectId>) -> CommandKind;
        let kinds: [(Make, u32); 4] = [
            (|units| CommandKind::Dismount { units }, 0x20D),
            (|units| CommandKind::LeaveBoat { units }, 0x20E),
            (|units| CommandKind::Dive { units }, 0x118),
            (|units| CommandKind::Surface { units }, 0x218),
        ];
        for (n, (make, code)) in kinds.into_iter().enumerate() {
            let a = spawn(&mut sim, WARRIOR, 0, (10, 10 + 4 * n as i32));
            let b = spawn(&mut sim, DRAGOON, 0, (14, 10 + 4 * n as i32));
            for u in [a, b] {
                fill_queue(&mut sim, u);
            }
            run(&mut sim, 0, make(vec![a, b]));
            for u in [a, b] {
                assert_eq!(posted(&sim, u), [(code, vec![], true)], "{code:#x}");
                assert_eq!(queue_len(&sim, u), 0, "{code:#x}");
            }
            // From the queue (not direct): message yes, queue stays.
            let c = spawn(&mut sim, WARRIOR, 0, (18, 10 + 4 * n as i32));
            fill_queue(&mut sim, c);
            let cmd = Command {
                player: 0,
                kind: make(vec![c]),
            };
            execute_command(&mut sim, &cmd, false);
            assert_eq!(posted(&sim, c), [(code, vec![], true)]);
            assert_eq!(queue_len(&sim, c), 1);
        }
        assert!(responses(&sim).is_empty());
    }

    #[test]
    fn foreign_units_are_skipped() {
        // A8: units of other players get nothing (owns).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let mine = spawn(&mut sim, COLONIST, 0, (10, 10));
        let foreign = spawn(&mut sim, COLONIST, 1, (10, 12));
        let horse = spawn(&mut sim, HORSE, 0, (14, 10));
        let canoe = spawn(&mut sim, CANOE, 0, (16, 10));
        let units = vec![foreign, mine];
        let all = [
            CommandKind::Mount {
                horse,
                units: units.clone(),
            },
            CommandKind::Dismount {
                units: units.clone(),
            },
            CommandKind::BoardBoat {
                boat: canoe,
                units: units.clone(),
            },
            CommandKind::LeaveBoat {
                units: units.clone(),
            },
            CommandKind::Dive {
                units: units.clone(),
            },
            CommandKind::Surface { units },
        ];
        for kind in all {
            run(&mut sim, 0, kind);
            assert!(posted(&sim, foreign).is_empty());
            sim.object_mut(mine).unwrap().fifo.clear();
        }
        assert!(responses(&sim).iter().all(|&(u, _)| u == mine));
    }

    fn assert_loads(state: &SimState, rules: &crate::rules::Rules) {
        let bytes = crate::save::testkit::save_bytes(state, rules);
        if let Err(e) = crate::save::SaveGame::from_bytes(&bytes) {
            panic!("savegame does not load: {e:#}");
        }
    }

    // QUIRK K-224: kept
    #[test]
    fn mount_and_board_boat_with_missing_or_invalid_ids_still_save_and_load() {
        // `mount_start` accepts every ID from `0x10D`/`0x10E`; the former load check rejected
        // IDs outside the object tables ("mount ID invalid", no longer exists). No command
        // may produce a savegame that can be saved but not loaded.
        let rules = rules();
        let ids = [
            ObjectId::new(1000, 0, 1), // index outside the table of slot 0
            ObjectId::new(0, 11, 1),   // slot 11 does not exist
            ObjectId::new(500, 0, 1),  // in the table, but no object
            ObjectId::NONE,
        ];
        for bad in ids {
            for board in [false, true] {
                let mut state = state(&rules);
                let w = {
                    let mut sim = Sim::new(&mut state, &rules);
                    let w = spawn(&mut sim, PRI_WARRIOR, 0, (10, 10));
                    let kind = if board {
                        CommandKind::BoardBoat {
                            boat: bad,
                            units: vec![w],
                        }
                    } else {
                        CommandKind::Mount {
                            horse: bad,
                            units: vec![w],
                        }
                    };
                    run(&mut sim, 0, kind);
                    w
                };
                for _ in 0..6 {
                    crate::tick::step(&mut state, &rules, &[]);
                }
                assert!(state.object(w, false).is_some());
                assert_loads(&state, &rules);
            }
        }
    }

    #[test]
    fn board_boat_on_a_non_boat_target_still_saves_and_loads() {
        // The original does not check the target of `0x0C` (K-224): a building, the own object
        // or a horse as "boat" must not leave anything invalid.
        let rules = rules();
        for pick in 0..3 {
            let mut state = state(&rules);
            {
                let mut sim = Sim::new(&mut state, &rules);
                let w = spawn(&mut sim, PRI_WARRIOR, 0, (10, 10));
                let hq = spawn(&mut sim, HQ, 0, (14, 10));
                let horse = spawn(&mut sim, HORSE, 0, (10, 14));
                let boat = [hq, w, horse][pick];
                run(
                    &mut sim,
                    0,
                    CommandKind::BoardBoat {
                        boat,
                        units: vec![w],
                    },
                );
            }
            for _ in 0..80 {
                crate::tick::step(&mut state, &rules, &[]);
                assert_loads(&state, &rules);
            }
        }
    }
}
