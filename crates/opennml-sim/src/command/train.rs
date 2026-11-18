//! Train commands `0x29` forward, `0x2A` backward, `0x2B` halt (`GameWorld_CmdTrainForward`
//! `0x00476F80`, `GameWorld_CmdTrainBackward` `0x00476FF0`, `GameWorld_CmdTrainStop` `0x00477060`;
//! re/train-multipart 2.2, 13.3).

use super::{TrainOp, owns, reject};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::module::train;
use crate::sim::Sim;

/// Messages to the locomotive: depart forward, backward, brake (`PropertyTrain_Query`).
const FORWARD: u32 = 0x121;
const BACKWARD: u32 = 0x221;
const STOP: u32 = 0x321;

/// The three train commands: `GetObjectById(arg0)`, then **synchronous** `GameObject_SendMessage` - no
/// queueing, no response, no emptying of the command queue (so not with `direct` either).
/// `len == 1` sends `{0x121}`, otherwise `{0x121, a, b}` (target tile); the halt sends `{0x321}`.
///
/// Deviation (A8): the original does not check the sender (the UI filters own
/// objects). The reimplementation admits only the own, selectable object with a train module (`owns`, module 33);
/// everything else is discarded. Tender and wagons accept the message and ignore it.
///
/// **Script path** (actions 163/164, re/commands 9): ownership is omitted - [`owns`] lets
/// every existing object through there (`GetObjectById(id, 0)`, also foreign, dying and
/// quartered ones); the train module remains a prerequisite (the original sends to non-locomotives without
/// effect, the reimplementation reports the rejection with a reason).
pub(super) fn execute(
    sim: &mut Sim,
    player: u8,
    id: ObjectId,
    op: TrainOp,
    target: Option<(i32, i32)>,
) {
    if !owns(sim, player, id) {
        return;
    }
    if train::train(sim, id).is_none() {
        reject(
            sim,
            player,
            "Train command",
            format!("object {} has no train module", id.0),
        );
        return;
    }
    let msg = match (op, target) {
        (TrainOp::Stop, _) => Message::new(STOP, &[]),
        (TrainOp::Forward, None) => Message::new(FORWARD, &[]),
        (TrainOp::Forward, Some((a, b))) => Message::new(FORWARD, &[a, b]),
        (TrainOp::Backward, None) => Message::new(BACKWARD, &[]),
        (TrainOp::Backward, Some((a, b))) => Message::new(BACKWARD, &[a, b]),
    };
    send(sim, id, &msg);
}

#[cfg(test)]
mod tests {
    use super::super::{Command, CommandKind, QueuedCommand, enqueue, execute as execute_command};
    use super::*;
    use crate::module::train::status;
    use crate::module::train::testkit::{COLONIST, LOCO, TENDER, lay_track, rules, spawn, state};
    use crate::module::train::train;
    use crate::state::SimEvent;

    fn run(
        sim: &mut Sim,
        player: u8,
        id: ObjectId,
        op: TrainOp,
        target: Option<(i32, i32)>,
        direct: bool,
    ) {
        let kind = CommandKind::Train {
            train: id,
            op,
            target,
        };
        execute_command(sim, &Command { player, kind }, direct);
    }

    /// Train state, command queue and message queue of an object.
    fn snapshot(sim: &Sim, id: ObjectId) -> (Option<crate::module::train::Train>, usize, usize) {
        let o = sim.object(id).expect("object present");
        (train(sim, id).cloned(), o.commands.len(), o.fifo.len())
    }

    fn fill_queue(sim: &mut Sim, id: ObjectId) {
        let entry = QueuedCommand {
            cyclic: false,
            command: Command {
                player: 0,
                kind: CommandKind::Halt { units: vec![id] },
            },
        };
        enqueue(sim, id, entry);
    }

    #[test]
    fn train_commands_send_synchronously_to_own_locomotives() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        lay_track(&mut sim, (10, 20), (49, 20));
        let loco = spawn(&mut sim, LOCO, 0, (30, 20));
        let back = spawn(&mut sim, LOCO, 0, (30, 25));
        let aimed = spawn(&mut sim, LOCO, 0, (30, 30));
        let foreign = spawn(&mut sim, LOCO, 1, (30, 35));
        let tender = spawn(&mut sim, TENDER, 0, (30, 40));
        let colonist = spawn(&mut sim, COLONIST, 0, (20, 20));
        assert_eq!(sim.object(back).unwrap().guid, LOCO);
        for id in [loco, back, aimed, foreign, colonist] {
            fill_queue(&mut sim, id);
        }

        // Forward without target (`len == 1`): immediately status 0xB, no queueing, no response, the
        // command queue stays (also with `direct`).
        let fifo_before = sim.object(loco).unwrap().fifo.len();
        run(&mut sim, 0, loco, TrainOp::Forward, None, true);
        let t = train(&sim, loco).unwrap();
        assert_eq!(
            t.status,
            status::FORWARD | status::MOVING | status::ACCEL,
            "immediately 0xB"
        );
        assert_eq!(t.target, None);
        assert_eq!(sim.object(loco).unwrap().commands.len(), 1);
        assert_eq!(sim.object(loco).unwrap().fifo.len(), fifo_before);
        assert!(
            !sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::UnitResponse { .. })),
            "no response"
        );

        // Halt: braking in the same call (`0x321`).
        run(&mut sim, 0, loco, TrainOp::Stop, None, true);
        let t = train(&sim, loco).unwrap();
        assert_eq!(
            t.status,
            status::FORWARD | status::MOVING | status::BRAKING,
            "braking: 0xE"
        );
        assert_eq!(sim.object(loco).unwrap().commands.len(), 1);

        // Backward: status 0x13.
        run(&mut sim, 0, back, TrainOp::Backward, None, false);
        assert_eq!(
            train(&sim, back).unwrap().status,
            status::BACKWARD | status::MOVING | status::ACCEL
        );

        // With target (`len ≠ 1`): `0x121 {a, b}`; a target at the halt is not used.
        run(&mut sim, 0, aimed, TrainOp::Forward, Some((35, 30)), true);
        let t = train(&sim, aimed).unwrap();
        assert_eq!(t.target, Some((35, 30)));
        assert_eq!(t.status, status::FORWARD | status::MOVING | status::ACCEL);

        // Foreign locomotive (A8), tender and object without module 33: discarded or ineffective.
        for (id, player) in [(foreign, 0), (colonist, 0), (tender, 0)] {
            let before = snapshot(&sim, id);
            run(&mut sim, player, id, TrainOp::Forward, None, true);
            run(&mut sim, player, id, TrainOp::Backward, Some((1, 2)), true);
            run(&mut sim, player, id, TrainOp::Stop, None, true);
            assert_eq!(snapshot(&sim, id), before, "{id:?}");
        }
        assert_eq!(train(&sim, foreign).unwrap().status, 0);
        // The owner itself may (counter-check to "foreign").
        run(&mut sim, 1, foreign, TrainOp::Forward, None, true);
        assert_eq!(
            train(&sim, foreign).unwrap().status,
            status::FORWARD | status::MOVING | status::ACCEL
        );
        // Invalid slot and missing object: discarded.
        run(&mut sim, 9, foreign, TrainOp::Stop, None, true);
        run(&mut sim, 0, ObjectId::NONE, TrainOp::Forward, None, true);
        run(
            &mut sim,
            0,
            ObjectId(0x7FFF_0000),
            TrainOp::Forward,
            None,
            true,
        );
        assert_eq!(
            train(&sim, foreign).unwrap().status & status::BRAKING,
            0,
            "slot 9 does not brake"
        );
    }

    #[test]
    fn a_wrapped_train_command_runs_at_once() {
        // The wrapper `0x7B` does not queue train commands (only move etc. per unit): immediate execution.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let loco = spawn(&mut sim, LOCO, 0, (30, 20));
        let kind = CommandKind::Queue {
            cyclic: false,
            inner: Box::new(CommandKind::Train {
                train: loco,
                op: TrainOp::Backward,
                target: None,
            }),
        };
        execute_command(&mut sim, &Command { player: 0, kind }, true);
        assert_eq!(
            train(&sim, loco).unwrap().status,
            status::BACKWARD | status::MOVING | status::ACCEL
        );
        assert!(sim.object(loco).unwrap().commands.is_empty());
    }
}
