//! Network commands of the AI (re/ki-queries 7.3): dying `0x10`, occupancy border `0x59`,
//! the behavior commands `0x68`-`0x6C` and crediting resources `0x7E`. The original hardly checks them
//! (the sender is the AI layer, humans have no producer); here A8 applies (only own
//! objects and the own slot) and A7 only for the tether (`0x6A`/`0x6C`) and stance. The
//! control kind of the slot stays as in the original: not human (`PLAYER_TYPE ≠ 1`).

use super::economy::MAX_BORDER;
use super::validate::own_building;
use super::{CommandKind, owns, reject, script_path};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::{post_message, send};
use crate::module::building::{self, msg as bmsg};
use crate::module::quarter::{ai_attack_mode_msg, ai_walk_flags_msg};
use crate::sim::Sim;
use crate::state::{Control, SimEvent};
use crate::upgrades::objects_of_slot;

/// Largest tether of `0x6A`/`0x6C` (A7): [`crate::module::walk::MAX_TETHER`] (derivation there). Values
/// below -2 are allowed: the message turns them into -1 like the original. The test
/// `max_ai_tether_keeps_the_seek_arithmetic_in_range` proves the value.
pub const MAX_AI_TETHER: i32 = crate::module::walk::MAX_TETHER;

/// Entry point for the commands of this module (`command::execute` routes them here).
pub(super) fn execute(sim: &mut Sim, player: u8, kind: &CommandKind) {
    match kind {
        CommandKind::Die { units } => die(sim, player, units),
        CommandKind::DebugDestroy { objects } => debug_destroy(sim, player, objects),
        CommandKind::OccupancyBorder { building, border } => {
            occupancy_border(sim, player, *building, *border);
        }
        CommandKind::AiPlayerAttackMode {
            slot,
            aggressiveness,
            stubbornness,
        } => {
            const WHAT: &str = "AI attack mode";
            if let Some(slot) = ai_slot(sim, player, WHAT, *slot) {
                let msg = ai_attack_mode_msg(*aggressiveness, *stubbornness);
                send_to_slot(sim, slot, &[msg]);
            }
        }
        CommandKind::AiPlayerBehavior { slot, mode } => {
            const WHAT: &str = "AI behavior";
            let Some(slot) = ai_slot(sim, player, WHAT, *slot) else {
                return;
            };
            // The original stores every mode (E-99) and only distributes 0 ... 4; for a stance not in 0...4 the
            // original leaves words of the messages `0x611`/`0x2100` unset (heap leftovers, not
            // deterministic, C-341).
            if !(0..=4).contains(mode) {
                reject(sim, player, WHAT, format!("mode {mode} (allowed 0 to 4)"));
                return;
            }
            // `GameWorld_CmdAiSetPlayerBehavior` (`0x0047BC70`): store the mode (`FUN_00478B20`),
            // then distribute it (`FUN_0047BA50`, shared with the script actions 203/204): per object
            // `0x2100` before `0x611`, for control kind != human (always here, [`ai_slot`]) and mode 3/2
            // additionally the AI attack mode; buildings get no `0x3503` (E-100).
            sim.state.combat.behavior_mode[usize::from(slot)] = *mode;
            crate::world::apply_player_behavior(sim, i32::from(slot), *mode);
        }
        CommandKind::AiPlayerWalkFlags {
            slot,
            mobile_home,
            tether,
            follow,
            abort,
        } => {
            const WHAT: &str = "AI walk behavior";
            if let Some(slot) = ai_slot(sim, player, WHAT, *slot)
                && check_tether(sim, player, WHAT, *tether)
            {
                let msg = ai_walk_flags_msg(*mobile_home, *tether, *follow, *abort);
                send_to_slot(sim, slot, &[msg]);
            }
        }
        CommandKind::AiObjectAttackMode {
            object,
            aggressiveness,
            stubbornness,
        } => {
            const WHAT: &str = "AI object attack mode";
            if check_object(sim, player, WHAT, *object) {
                send(
                    sim,
                    *object,
                    &ai_attack_mode_msg(*aggressiveness, *stubbornness),
                );
            }
        }
        CommandKind::AiObjectWalkFlags {
            object,
            mobile_home,
            tether,
            follow,
            abort,
        } => {
            const WHAT: &str = "AI object walk behavior";
            if check_object(sim, player, WHAT, *object) && check_tether(sim, player, WHAT, *tether)
            {
                send(
                    sim,
                    *object,
                    &ai_walk_flags_msg(*mobile_home, *tether, *follow, *abort),
                );
            }
        }
        CommandKind::AddResource {
            slot,
            resource,
            amount,
        } => {
            const WHAT: &str = "Credit resource";
            let Some(slot) = ai_slot(sim, player, WHAT, *slot) else {
                return;
            };
            // `Player_AddResource` (`0x004FAE30`) only knows the indices 0 ... 4.
            let Some(index) = usize::try_from(*resource).ok().filter(|&i| i < 5) else {
                reject(
                    sim,
                    player,
                    WHAT,
                    format!("resource {resource} does not exist (allowed 0 to 4)"),
                );
                return;
            };
            // Without limit, also negative and also population and living space (E-105); no message.
            sim.state.players[usize::from(slot)].add_resource(index, *amount);
        }
        _ => unreachable!("ai::execute only for 0x10, 0x59, 0x68 ... 0x6C, 0x7E and 0xF5"),
    }
}

/// `0x10` dying (`0x00480D90`, E-106): remove each object from the selection
/// (`GameWorld_DeselectObject`), then `P({0x110}, abort)` to the dying module. The original
/// checks nothing; here only own, living, non-quartered objects (A8, [`owns`]), the rest
/// are ignored. On the script path (actions 34 and 193, E-149, E-214) every present
/// object counts: foreign, dying and quartered units are deselected and get `0x110`; for
/// a building the message has no receiver and only aborts the modules (`0x110` only affects
/// units with a dying module).
fn die(sim: &mut Sim, player: u8, units: &[ObjectId]) {
    for &unit in units {
        if !owns(sim, player, unit) {
            continue;
        }
        kill_unit(sim, unit);
    }
}

/// Effect of `0x10` on an object whose ownership is already checked: remove from the selection, then
/// `{0x110}` with abort.
fn kill_unit(sim: &mut Sim, unit: ObjectId) {
    sim.state.events.push(SimEvent::Deselected { object: unit });
    post_message(sim, unit, Message::new(0x110, &[]), true);
}

/// Debug "destroy objects" (no counterpart in the original): only own,
/// living, non-quartered objects of the sender (A8, [`owns`]; the rest are ignored).
/// Units like [`die`]; buildings via the normal destruction (`PropertyBuilding_Destroy` `0x00506300`,
/// event bit 2 first, statistics, collapse after 4.5 s). The building then collapses like after any
/// attack; the name in the name table stays on the dead ID like after any destruction
/// (E-145). Which case applies is decided by the building module of the object.
fn debug_destroy(sim: &mut Sim, player: u8, objects: &[ObjectId]) {
    // Under `Origin::Script` `owns` only counts "present": the debug aid must never come from a script
    // action (no counterpart in the original), otherwise it would destroy foreign objects.
    if script_path(sim) {
        reject(sim, player, "Destroy", "not from scripts");
        return;
    }
    for &object in objects {
        if !owns(sim, player, object) {
            continue;
        }
        if building::building(sim, object).is_some() {
            building::destroy::destroy(sim, object);
        } else {
            kill_unit(sim, object);
        }
    }
}

/// `0x59` (`0x00478EC0`, E-104): `S({0x3303, border})`. The original checks nothing (the AI sends
/// -2/-1 to its main house); here an own building, a border of +-[`MAX_BORDER`] and a
/// moved rectangle that stays within the limits of the load check (A7).
fn occupancy_border(sim: &mut Sim, player: u8, building_id: ObjectId, border: i32) {
    const WHAT: &str = "Occupancy border";
    if !own_building(sim, player, building_id) {
        reject(
            sim,
            player,
            WHAT,
            format!("building {building_id} not present or not own"),
        );
        return;
    }
    if !(-MAX_BORDER..=MAX_BORDER).contains(&border) {
        reject(
            sim,
            player,
            WHAT,
            format!("border {border} outside {} ... {MAX_BORDER}", -MAX_BORDER),
        );
        return;
    }
    let Some(r) = building::building(sim, building_id).map(|b| b.occupancy_rect) else {
        return;
    };
    // Like `OnMsgExpandOccupancy`: smaller by `border` at the left/top, larger at the right/bottom.
    let moved = [
        r[0].wrapping_sub(border),
        r[1].wrapping_sub(border),
        r[2].wrapping_add(border),
        r[3].wrapping_add(border),
    ];
    if !building::rect_within_map(&moved, sim.state.path.width, sim.state.path.height) {
        reject(
            sim,
            player,
            WHAT,
            format!("border {border} would push the rectangle beyond the edge of the map"),
        );
        return;
    }
    send(
        sim,
        building_id,
        &Message::new(bmsg::EXPAND_OCCUPANCY, &[border]),
    );
}

/// The slot `slot` of the command is the sender and not a human slot; otherwise rejection of the
/// whole command and `None`. This also puts the slot itself in 0 ... 7.
fn ai_slot(sim: &Sim, player: u8, what: &str, slot: i32) -> Option<u8> {
    if slot != i32::from(player) {
        reject(sim, player, what, format!("slot {slot} is not the sender"));
        return None;
    }
    if sim
        .state
        .players
        .get(usize::from(player))
        .is_none_or(|p| p.control == Control::Human)
    {
        reject(sim, player, what, format!("slot {slot} is a human slot"));
        return None;
    }
    Some(player)
}

/// `0x6B`/`0x6C` (E-103): the slot comes from the ID `(id >> 11) & 31`; it must be the sender
/// (A8) and not a human slot. The object must be present, a dead or quartered one
/// too (the original finds it without a dead filter).
fn check_object(sim: &Sim, player: u8, what: &str, object: ObjectId) -> bool {
    if object.slot() != player {
        reject(
            sim,
            player,
            what,
            format!("object {object} does not belong to slot {player}"),
        );
        return false;
    }
    if sim.object(object).is_none() {
        reject(sim, player, what, format!("object {object} not present"));
        return false;
    }
    ai_slot(sim, player, what, i32::from(player)).is_some()
}

/// Tether at most [`MAX_AI_TETHER`] (A7, E-102); there is no limit downward.
fn check_tether(sim: &Sim, player: u8, what: &str, tether: i32) -> bool {
    if tether > MAX_AI_TETHER {
        reject(
            sim,
            player,
            what,
            format!("tether {tether} above the limit {MAX_AI_TETHER}"),
        );
        return false;
    }
    true
}

/// The messages in sequence to all objects of the slot table of the slot (table order, without module check:
/// buildings, occupants and dying ones included, `upgrades::objects_of_slot`).
fn send_to_slot(sim: &mut Sim, slot: u8, msgs: &[Message]) {
    for id in objects_of_slot(sim.state, slot) {
        for msg in msgs {
            send(sim, id, msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit as ck;
    use crate::command::{Command, Outcome, execute as run_command, testkit, validate};
    use crate::fx::fixed_hypot;
    use crate::id::ObjectId;
    use crate::message::{Message, Posted};
    use crate::messaging::send;
    use crate::module::attack::attack;
    use crate::module::building::building;
    use crate::module::quarter::{ai_attack_mode_msg, ai_walk_flags_msg};
    use crate::module::walk::walk;
    use crate::object::status;
    use crate::rules::{BuildingShape, ModelBox, Rules};
    use crate::save::checksum;
    use crate::state::{Control, SimEvent, SimState};

    /// Building with dying (16) and attack module (17): the tower of the scene.
    const TOWER: u32 = 9001;
    const TOWER_TYPE: &str = "GUID 9001\nNAME Testturm\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n\
PROPID 16\nVALUEID 170001 6 1 0\nVALUEID 170002 6 1 0\n\
PROPID 17\nVALUEID 180001 6 1 256\nVALUEID 180002 6 1 0\n";

    fn rules() -> Rules {
        let mut r = ck::rules_with(TOWER_TYPE);
        r.defaults.insert(
            TOWER,
            [(11, 300), (13, 40), (14, 2500)].into_iter().collect(),
        );
        r.building_shapes.insert(
            TOWER,
            BuildingShape {
                center: [0.0; 3],
                half: [100.0, 50.0, 100.0],
                free_nodes: Vec::new(),
                helper_nodes: Default::default(),
            },
        );
        r.model_boxes.insert(
            TOWER,
            ModelBox {
                center: [0.0; 3],
                half: [100.0, 50.0, 100.0],
            },
        );
        r
    }

    /// Slot 0 human, slot 1 AI, slot 2 passive AI.
    fn state(rules: &Rules) -> SimState {
        let mut s = ck::state(rules);
        s.players[0].control = Control::Human;
        s.players[1].control = Control::Ai;
        s.players[2].control = Control::Passive;
        s
    }

    fn go(sim: &mut Sim, player: u8, kind: CommandKind) -> Outcome {
        run_command(sim, &Command { player, kind }, true)
    }

    fn assert_rejected_with(outcome: Outcome, keyword: &str) {
        match outcome {
            Outcome::Rejected(why) => {
                assert!(why.contains(keyword), "\"{why}\" without \"{keyword}\"")
            }
            other => panic!("expected Rejected (\"{keyword}\"), got {other:?}"),
        }
    }

    /// Aggressiveness, stance and stubbornness of the attack module.
    fn attack_fields(sim: &Sim, id: ObjectId) -> (i32, i32, i32) {
        let a = attack(sim, id).expect("attack module");
        (a.aggressiveness, a.mode, a.stubbornness[0])
    }

    #[test]
    fn builders_make_the_messages_of_the_original() {
        // `FUN_004E5610`: {0x611, a, a == 1 ? 0 : 3, b, 0}.
        assert_eq!(ai_attack_mode_msg(4, 2), Message::new(0x611, &[4, 3, 2, 0]));
        assert_eq!(ai_attack_mode_msg(1, 7), Message::new(0x611, &[1, 0, 7, 0]));
        assert_eq!(ai_attack_mode_msg(0, 0), Message::new(0x611, &[0, 3, 0, 0]));
        // `FUN_004E5760`: {0x2100, follow, mobile, mobile && tether >= -2 ? tether : -1, abort, 0}.
        assert_eq!(
            ai_walk_flags_msg(true, -2, true, false),
            Message::new(0x2100, &[1, 1, -2, 0, 0])
        );
        assert_eq!(
            ai_walk_flags_msg(true, 500, false, true),
            Message::new(0x2100, &[0, 1, 500, 1, 0])
        );
        assert_eq!(
            ai_walk_flags_msg(true, -3, false, false),
            Message::new(0x2100, &[0, 1, -1, 0, 0])
        );
        assert_eq!(
            ai_walk_flags_msg(false, 500, true, true),
            Message::new(0x2100, &[1, 0, -1, 1, 0])
        );
    }

    #[test]
    fn die_kills_own_units_and_rejects_foreign_quartered_and_dead() {
        let rules = rules();
        let mut state = state(&rules);
        let (own, mixed, quartered, foreign, tower, dying);
        {
            let mut sim = Sim::new(&mut state, &rules);
            own = ck::spawn(&mut sim, ck::SABRE, 1, (10, 10));
            mixed = ck::spawn(&mut sim, ck::SABRE, 1, (12, 10));
            quartered = ck::spawn(&mut sim, ck::SABRE, 1, (5, 55));
            let hq = ck::spawn(&mut sim, ck::HQ, 1, (8, 55));
            // Far away so that the fighters do not slay each other.
            foreign = ck::spawn(&mut sim, ck::SABRE, 0, (55, 5));
            tower = ck::spawn(&mut sim, TOWER, 1, (30, 30));
            // A dying unit (`DIED` set, object still there) no longer accepts a command.
            dying = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
            sim.object_mut(dying).unwrap().status |= status::DIED;
            // Move in like after the arrival (`0x1617`).
            send(&mut sim, hq, &Message::new(0x1617, &[quartered.as_i32()]));
            assert_ne!(sim.object(quartered).unwrap().status & status::QUARTERED, 0);
            sim.state.events.clear();

            // An own unit: removed from the selection, then `{0x110}` with abort.
            assert_eq!(
                go(&mut sim, 1, CommandKind::Die { units: vec![own] }),
                Outcome::Accepted
            );
            assert!(
                sim.state
                    .events
                    .contains(&SimEvent::Deselected { object: own })
            );
            let posted: Vec<_> = sim.object(own).unwrap().fifo.iter().cloned().collect();
            assert_eq!(
                posted,
                vec![Posted::Message {
                    msg: Message::new(0x110, &[]),
                    abort: true
                }]
            );

            // Foreign, quartered, missing: nothing happens, no event, no message.
            sim.state.events.clear();
            for units in [
                vec![foreign],
                vec![quartered],
                vec![dying],
                vec![ObjectId::new(900, 1, 3)],
                vec![ObjectId::NONE],
            ] {
                let before = checksum(sim.state);
                let outcome = go(&mut sim, 1, CommandKind::Die { units });
                assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
                assert_eq!(before, checksum(sim.state));
            }
            assert!(sim.state.events.is_empty());
            assert!(sim.object(foreign).unwrap().fifo.is_empty());
            assert!(sim.object(quartered).unwrap().fifo.is_empty());

            // Mixed: the own one takes effect, the foreign one does not.
            let outcome = go(
                &mut sim,
                1,
                CommandKind::Die {
                    units: vec![mixed, foreign],
                },
            );
            assert!(matches!(outcome, Outcome::Partial(_)), "{outcome:?}");
            assert_eq!(
                sim.state.events,
                vec![SimEvent::Deselected { object: mixed }]
            );
            assert!(sim.object(foreign).unwrap().fifo.is_empty());

            // An own building with a dying module also dies (no building check, E-106).
            assert_eq!(
                go(&mut sim, 1, CommandKind::Die { units: vec![tower] }),
                Outcome::Accepted
            );
        }
        // The dying ones die out and are dismantled; a building with a dying module likewise.
        let gone = |sim: &Sim, id| {
            sim.object(id)
                .is_none_or(|o| o.status & status::DIED != 0 || validate::is_dead(sim, id))
        };
        let n = testkit::run_until(&mut state, &rules, 600, |sim| {
            [own, mixed, tower].iter().all(|&id| gone(sim, id))
        });
        assert!(n > 0);
        let sim = Sim::new(&mut state, &rules);
        // The foreign and the quartered fighter are still alive.
        for u in [foreign, quartered] {
            assert!(sim.object(u).is_some_and(|o| o.status & status::DIED == 0));
        }
        // A dead unit no longer accepts a command.
        let mut sim = sim;
        assert_rejected_with(
            go(&mut sim, 1, CommandKind::Die { units: vec![own] }),
            "not present, foreign, quartered or dead",
        );
    }

    #[test]
    fn occupancy_border_shrinks_the_rectangle() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = ck::spawn(&mut sim, ck::HQ, 1, (30, 30));
        let twin = ck::spawn(&mut sim, ck::HQ, 1, (10, 10));
        let foreign = ck::spawn(&mut sim, ck::HQ, 0, (50, 50));
        let unit = ck::spawn(&mut sim, ck::SABRE, 1, (20, 20));
        let rect = |sim: &Sim, id| building(sim, id).unwrap().occupancy_rect;
        let r = rect(&sim, hq);
        assert!(
            sim.state.occupancy.test(r[0], r[1]),
            "initial position occupied"
        );

        // Like `0x3303 {−2}`: the rectangle shrinks by two per side, the occupancy stays.
        send(&mut sim, twin, &Message::new(0x3303, &[-2]));
        assert_eq!(
            go(
                &mut sim,
                1,
                CommandKind::OccupancyBorder {
                    building: hq,
                    border: -2
                }
            ),
            Outcome::Accepted
        );
        assert_eq!(rect(&sim, hq), [r[0] + 2, r[1] + 2, r[2] - 2, r[3] - 2]);
        let t = rect(&sim, twin);
        assert_eq!(
            rect(&sim, hq).map(|v| v - rect(&sim, hq)[0]),
            t.map(|v| v - t[0])
        );
        assert!(sim.state.occupancy.test(r[0], r[1]), "occupancy bits stay");

        // Growing up to the limit, after that it is over.
        assert_eq!(
            go(
                &mut sim,
                1,
                CommandKind::OccupancyBorder {
                    building: hq,
                    border: 20
                }
            ),
            Outcome::Accepted
        );
        assert_eq!(rect(&sim, hq)[0], r[0] + 2 - 20);

        // Foreign, no building, missing, border outside +-MAX_BORDER: rejected, state the same.
        let max = crate::command::economy::MAX_BORDER;
        let cases = [
            (foreign, 1),
            (foreign, 1000),
            (unit, 1),
            (ObjectId::new(900, 1, 3), 1),
            (hq, max + 1),
            (hq, -max - 1),
            (hq, 1000),
            (hq, i32::MAX),
            (hq, i32::MIN),
        ];
        for (building, border) in cases {
            let before = checksum(sim.state);
            let outcome = go(
                &mut sim,
                1,
                CommandKind::OccupancyBorder { building, border },
            );
            assert!(
                matches!(outcome, Outcome::Rejected(_)),
                "{building:?} {border}: {outcome:?}"
            );
            assert_eq!(before, checksum(sim.state), "{building:?} {border}");
        }
        assert_rejected_with(
            go(
                &mut sim,
                1,
                CommandKind::OccupancyBorder {
                    building: hq,
                    border: max + 1,
                },
            ),
            "border",
        );
    }

    #[test]
    fn occupancy_border_keeps_the_rectangle_inside_the_map() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // A building at the map edge: the load check allows rectangles up to one mask width
        // beyond the map; a second step beyond that is rejected.
        let hq = ck::spawn(&mut sim, ck::HQ, 1, (2, 2));
        let max = crate::command::economy::MAX_BORDER;
        let r = building(&sim, hq).unwrap().occupancy_rect;
        assert_eq!(
            go(
                &mut sim,
                1,
                CommandKind::OccupancyBorder {
                    building: hq,
                    border: max
                }
            ),
            Outcome::Accepted
        );
        assert_eq!(building(&sim, hq).unwrap().occupancy_rect[0], r[0] - max);
        let before = checksum(sim.state);
        assert_rejected_with(
            go(
                &mut sim,
                1,
                CommandKind::OccupancyBorder {
                    building: hq,
                    border: max,
                },
            ),
            "map",
        );
        assert_eq!(before, checksum(sim.state));
    }

    #[test]
    fn ai_attack_mode_reaches_every_object_of_the_slot() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u1 = ck::spawn(&mut sim, ck::SABRE, 1, (10, 10));
        let u2 = ck::spawn(&mut sim, ck::SABRE, 1, (12, 10));
        let tower = ck::spawn(&mut sim, TOWER, 1, (30, 30));
        let inmate = ck::spawn(&mut sim, ck::SABRE, 1, (14, 10));
        sim.object_mut(inmate).unwrap().status |= status::QUARTERED;
        let other = ck::spawn(&mut sim, ck::SABRE, 2, (16, 10));
        let human = ck::spawn(&mut sim, ck::SABRE, 0, (18, 10));
        let before_other = attack_fields(&sim, other);
        let before_human = attack_fields(&sim, human);

        let kind = |a, b| CommandKind::AiPlayerAttackMode {
            slot: 1,
            aggressiveness: a,
            stubbornness: b,
        };
        assert_eq!(go(&mut sim, 1, kind(4, 2)), Outcome::Accepted);
        for id in [u1, u2, tower, inmate] {
            // {0x611, 4, 3, 2, 0}: aggressiveness 4, stance 3, stubbornness 2.
            assert_eq!(attack_fields(&sim, id), (4, 3, 2), "{id:?}");
        }
        // Other slots stay untouched.
        assert_eq!(attack_fields(&sim, other), before_other);
        assert_eq!(attack_fields(&sim, human), before_human);

        // a = 1: middle word (stance) 0.
        assert_eq!(go(&mut sim, 1, kind(1, 0)), Outcome::Accepted);
        assert_eq!(attack_fields(&sim, u1), (1, 0, 0));

        // Human slot and foreign slot: rejected.
        let human_kind = CommandKind::AiPlayerAttackMode {
            slot: 0,
            aggressiveness: 4,
            stubbornness: 2,
        };
        assert_rejected_with(go(&mut sim, 0, human_kind), "human");
        assert_rejected_with(go(&mut sim, 2, kind(4, 2)), "is not the sender");
        for slot in [-1, 8, 31, 256, i32::MIN] {
            let bad_slot = CommandKind::AiPlayerAttackMode {
                slot,
                aggressiveness: 4,
                stubbornness: 2,
            };
            assert_rejected_with(go(&mut sim, 1, bad_slot), "is not the sender");
        }
        // Aggressiveness and stubbornness pass through unchecked like in the original (E-101); stance 3,
        // because no `a` equals 1.
        for (a, b) in [(5, 2), (-1, 2), (i32::MAX, 0), (4, -1), (4, i32::MIN)] {
            assert_eq!(go(&mut sim, 1, kind(a, b)), Outcome::Accepted, "{a} {b}");
            assert_eq!(attack_fields(&sim, u1), (a, 3, b), "{a} {b}");
        }
        // There is no upper limit of the stubbornness either.
        assert_eq!(go(&mut sim, 1, kind(0, i32::MAX)), Outcome::Accepted);
        assert_eq!(attack_fields(&sim, u2), (0, 3, i32::MAX));
        // Passive AI is not human control.
        assert_eq!(
            go(
                &mut sim,
                2,
                CommandKind::AiPlayerAttackMode {
                    slot: 2,
                    aggressiveness: 2,
                    stubbornness: 1
                }
            ),
            Outcome::Accepted
        );
        assert_eq!(attack_fields(&sim, other), (2, 3, 1));
    }

    #[test]
    fn ai_behavior_differs_from_set_behavior() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = ck::spawn(&mut sim, ck::SABRE, 1, (10, 10));
        let tower = ck::spawn(&mut sim, TOWER, 1, (30, 30));
        let tower_mode = building(&sim, tower).unwrap().attack_mode;
        let behavior = |mode| CommandKind::AiPlayerBehavior { slot: 1, mode };

        // Mode 3: `0x2100` (3), `0x611 {4, 3, 1, 0}` and finally `{0x611, 4, 3, 2, 0}`.
        assert_eq!(go(&mut sim, 1, behavior(3)), Outcome::Accepted);
        assert_eq!(sim.state.combat.behavior_mode[1], 3);
        assert_eq!(attack_fields(&sim, unit), (4, 3, 2));
        let w = walk(&sim, unit).unwrap();
        assert!(w.try_to_follow && !w.mobile_home);
        assert_eq!(w.tether, -1);
        // Buildings get no `0x3503`, but `0x611`.
        assert_eq!(building(&sim, tower).unwrap().attack_mode, tower_mode);
        assert_eq!(attack_fields(&sim, tower), (4, 3, 2));

        // Mode 2: finally `{0x611, 4, 3, 1, 0}`, walk behavior 2 (mobile home, tether -2 = sight * 1024).
        assert_eq!(go(&mut sim, 1, behavior(2)), Outcome::Accepted);
        assert_eq!(sim.state.combat.behavior_mode[1], 2);
        assert_eq!(attack_fields(&sim, unit), (4, 3, 1));
        assert!(walk(&sim, unit).unwrap().mobile_home);
        assert_eq!(
            walk(&sim, unit).unwrap().tether,
            sim.attribute(unit, 18) << 10
        );

        // Mode 4 and 0: only `0x2100(mode)` and `0x611(mode)`, nothing additional.
        assert_eq!(go(&mut sim, 1, behavior(4)), Outcome::Accepted);
        assert_eq!(attack_fields(&sim, unit), (4, 4, 1));
        assert_eq!(walk(&sim, unit).unwrap().tether, 8 << 12);
        assert_eq!(building(&sim, tower).unwrap().attack_mode, tower_mode);
        assert_eq!(go(&mut sim, 1, behavior(0)), Outcome::Accepted);
        assert_eq!(attack_fields(&sim, unit), (1, 0, 4));

        // A unit created afterwards takes over the mode.
        assert_eq!(go(&mut sim, 1, behavior(3)), Outcome::Accepted);
        let fresh = ck::spawn(&mut sim, ck::SABRE, 1, (20, 20));
        assert_eq!(attack(&sim, fresh).unwrap().mode, 3);

        // Mode outside 0 ... 4 (E-99), human slot and foreign slot: rejected, mode stays.
        let before = checksum(sim.state);
        for mode in [5, 9, -1, i32::MAX, i32::MIN] {
            assert_rejected_with(go(&mut sim, 1, behavior(mode)), "mode");
        }
        assert_rejected_with(
            go(
                &mut sim,
                0,
                CommandKind::AiPlayerBehavior { slot: 0, mode: 3 },
            ),
            "human",
        );
        assert_rejected_with(go(&mut sim, 2, behavior(3)), "is not the sender");
        assert_eq!(before, checksum(sim.state));
        assert_eq!(sim.state.combat.behavior_mode[1], 3);
    }

    #[test]
    fn ai_walk_flags_set_follow_home_tether_abort() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = ck::spawn(&mut sim, ck::SABRE, 1, (10, 10));
        let human = ck::spawn(&mut sim, ck::SABRE, 0, (18, 10));
        let human_before = walk(&sim, human).unwrap().behaviour();
        let flags = |mobile_home, tether, follow, abort| CommandKind::AiPlayerWalkFlags {
            slot: 1,
            mobile_home,
            tether,
            follow,
            abort,
        };

        // {a = 1, b = -2, c = 1, d = 0}: mobile home, tether -2 (sight * 1024), follow, do not abort.
        assert_eq!(
            go(&mut sim, 1, flags(true, -2, true, false)),
            Outcome::Accepted
        );
        let w = walk(&sim, unit).unwrap();
        assert!(w.mobile_home && w.try_to_follow);
        assert_eq!(w.tether, 32 << 10);
        let pos = (w.home.0, w.home.1);
        assert_ne!(pos, (-1, -1));
        // A fixed tether stays what it is; below -2 the message makes -1.
        assert_eq!(
            go(&mut sim, 1, flags(true, 5000, false, false)),
            Outcome::Accepted
        );
        assert_eq!(walk(&sim, unit).unwrap().tether, 5000);
        assert!(!walk(&sim, unit).unwrap().try_to_follow);
        assert_eq!(
            go(&mut sim, 1, flags(true, -3, true, false)),
            Outcome::Accepted
        );
        assert_eq!(walk(&sim, unit).unwrap().tether, -1);
        assert_eq!(
            go(&mut sim, 1, flags(true, i32::MIN, true, false)),
            Outcome::Accepted
        );
        assert_eq!(walk(&sim, unit).unwrap().tether, -1);
        // a = 0: no home, tether -1, whatever b says.
        assert_eq!(
            go(&mut sim, 1, flags(false, 5000, true, false)),
            Outcome::Accepted
        );
        let w = walk(&sim, unit).unwrap();
        assert!(!w.mobile_home);
        assert_eq!((w.tether, w.home.0, w.home.1), (-1, -1, -1));

        // The tether ends at `MAX_AI_TETHER`, human slot and foreign slot are rejected.
        assert_eq!(
            go(&mut sim, 1, flags(true, MAX_AI_TETHER, true, false)),
            Outcome::Accepted
        );
        assert_eq!(walk(&sim, unit).unwrap().tether, MAX_AI_TETHER);
        let before = checksum(sim.state);
        for tether in [MAX_AI_TETHER + 1, i32::MAX] {
            assert_rejected_with(go(&mut sim, 1, flags(true, tether, true, false)), "tether");
            // Also without mobile home: the command applies to every value (A7).
            assert_rejected_with(go(&mut sim, 1, flags(false, tether, true, false)), "tether");
        }
        assert_eq!(before, checksum(sim.state));
        let human_flags = CommandKind::AiPlayerWalkFlags {
            slot: 0,
            mobile_home: true,
            tether: 0,
            follow: true,
            abort: false,
        };
        assert_rejected_with(go(&mut sim, 0, human_flags), "human");
        assert_rejected_with(
            go(&mut sim, 2, flags(true, 0, true, false)),
            "is not the sender",
        );
        assert_eq!(walk(&sim, human).unwrap().behaviour(), human_before);
    }

    /// The limit `MAX_AI_TETHER` keeps `dist ± tether` in `module/walk/seek.rs` within `i32`: `dist` comes
    /// from `Fixed_Hypot`, and its result stays below `MAX_SEEK_DISTANCE` for all inputs
    /// (`fixed_div(hi, 0x1000)` lies in `[−2²¹, 2²¹]` because of the wrap).
    #[test]
    fn max_ai_tether_keeps_the_seek_arithmetic_in_range() {
        use crate::module::walk::MAX_SEEK_DISTANCE;
        // All inputs count, also those with divisor 0 in `Fixed_Hypot` (hi around 2^22, `i32::MIN`): they end
        // with 0 (E-118).
        let mut worst = 0;
        let mut seen = 0;
        let mut check = |a: i32, b: i32| {
            worst = worst.max(fixed_hypot(a, b));
            seen += 1;
        };
        // Grid over all differences of a map of 1024 tiles (-2^22 ... 2^22), plus the wrap limits
        // of `fixed_div` (2^21, 2^22, ...) and large values.
        let step = (1 << 16) + 12_345;
        let mut dx = -(1 << 22);
        while dx <= 1 << 22 {
            let mut dz = -(1 << 22);
            while dz <= 1 << 22 {
                check(dx, dz);
                dz += step;
            }
            dx += step;
        }
        for hi in [
            (1 << 21) - 1,
            1 << 21,
            (1 << 21) + 1,
            3 << 20,
            (1 << 22) - 5000,
            (1 << 22) - 1,
            1 << 22,
            (1 << 22) + 5000,
            1 << 23,
            3 << 22,
            i32::MAX,
            i32::MIN + 1,
        ] {
            for lo in [0, 1, 4095, 4096, hi / 3, hi / 2, hi] {
                for (a, b) in [(hi, lo), (lo, hi), (-hi, lo), (hi, -lo)] {
                    check(a, b);
                }
            }
        }
        for (a, b) in [
            (i32::MIN, i32::MIN),
            (i32::MIN, 0),
            (0, i32::MIN),
            (i32::MIN, 1 << 22),
        ] {
            check(a, b);
        }
        // Random-like values (linear congruential, fixed).
        let mut x: u32 = 0x4E4D_4C33;
        for _ in 0..20_000 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let a = x as i32;
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            check(a, x as i32);
        }
        assert!(seen > 10_000, "{seen}");
        assert!(worst > 0 && worst <= MAX_SEEK_DISTANCE, "{worst}");
        // `tether + dist` does not overflow for any distance; `dist − tether` (both >= 0) not anyway.
        assert!(MAX_AI_TETHER.checked_add(worst).is_some());
        assert!(MAX_AI_TETHER.checked_add(MAX_SEEK_DISTANCE).is_some());
    }

    #[test]
    fn ai_object_commands_reach_only_their_object() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u1 = ck::spawn(&mut sim, ck::SABRE, 1, (10, 10));
        let u2 = ck::spawn(&mut sim, ck::SABRE, 1, (12, 10));
        let dead = ck::spawn(&mut sim, ck::SABRE, 1, (14, 10));
        let inmate = ck::spawn(&mut sim, ck::SABRE, 1, (16, 10));
        let foreign = ck::spawn(&mut sim, ck::SABRE, 2, (18, 10));
        let human = ck::spawn(&mut sim, ck::SABRE, 0, (20, 10));
        sim.object_mut(dead).unwrap().status |= status::DIED;
        sim.object_mut(inmate).unwrap().status |= status::QUARTERED;
        let untouched = attack_fields(&sim, u2);

        let attack_kind = |object, a, b| CommandKind::AiObjectAttackMode {
            object,
            aggressiveness: a,
            stubbornness: b,
        };
        assert_eq!(go(&mut sim, 1, attack_kind(u1, 4, 2)), Outcome::Accepted);
        assert_eq!(attack_fields(&sim, u1), (4, 3, 2));
        assert_eq!(attack_fields(&sim, u2), untouched);
        // The original also accepts quartered and dead objects (E-103); for the dead one the
        // modules are off, the message fizzles there.
        assert_eq!(
            go(&mut sim, 1, attack_kind(inmate, 1, 3)),
            Outcome::Accepted
        );
        assert_eq!(attack_fields(&sim, inmate), (1, 0, 3));
        assert_eq!(go(&mut sim, 1, attack_kind(dead, 1, 3)), Outcome::Accepted);

        // Foreign object, human slot, missing object: rejected, state the same.
        let before = checksum(sim.state);
        assert_rejected_with(
            go(&mut sim, 1, attack_kind(foreign, 4, 2)),
            "not belong to slot 1",
        );
        assert_rejected_with(go(&mut sim, 0, attack_kind(human, 4, 2)), "human");
        assert_rejected_with(
            go(&mut sim, 1, attack_kind(ObjectId::new(900, 1, 3), 4, 2)),
            "not present",
        );
        assert_rejected_with(go(&mut sim, 1, attack_kind(ObjectId::NONE, 4, 2)), "object");
        assert_eq!(before, checksum(sim.state));
        // Values outside 0...4 or negative stubbornness pass through (E-101).
        for (a, b) in [(5, 2), (-1, 2), (4, -1)] {
            assert_eq!(
                go(&mut sim, 1, attack_kind(u1, a, b)),
                Outcome::Accepted,
                "{a} {b}"
            );
            assert_eq!(attack_fields(&sim, u1), (a, 3, b), "{a} {b}");
        }

        let flag_kind = |object, tether| CommandKind::AiObjectWalkFlags {
            object,
            mobile_home: true,
            tether,
            follow: true,
            abort: false,
        };
        let w2 = walk(&sim, u2).unwrap().behaviour();
        assert_eq!(go(&mut sim, 1, flag_kind(u1, 7000)), Outcome::Accepted);
        let w = walk(&sim, u1).unwrap();
        assert!(w.mobile_home && w.try_to_follow);
        assert_eq!(w.tether, 7000);
        assert_eq!(
            walk(&sim, u2).unwrap().behaviour(),
            w2,
            "other objects stay"
        );
        assert_eq!(go(&mut sim, 1, flag_kind(inmate, -2)), Outcome::Accepted);
        let before = checksum(sim.state);
        assert_eq!(
            go(&mut sim, 1, flag_kind(u1, MAX_AI_TETHER)),
            Outcome::Accepted
        );
        let after_max = checksum(sim.state);
        assert_ne!(before, after_max);
        assert_rejected_with(go(&mut sim, 1, flag_kind(u1, MAX_AI_TETHER + 1)), "tether");
        assert_eq!(after_max, checksum(sim.state));
        assert_rejected_with(
            go(&mut sim, 1, flag_kind(foreign, 0)),
            "not belong to slot 1",
        );
        assert_rejected_with(go(&mut sim, 0, flag_kind(human, 0)), "human");
    }

    #[test]
    fn add_resource_credits_without_limit() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let add = |slot, resource, amount| CommandKind::AddResource {
            slot,
            resource,
            amount,
        };
        let fx = |sim: &Sim, i: usize| sim.state.players[1].resources_fx[i];
        let gold = fx(&sim, 0);
        assert_eq!(go(&mut sim, 1, add(1, 0, 50)), Outcome::Accepted);
        assert_eq!(fx(&sim, 0), gold + 50 * 4096);
        assert_eq!(sim.state.resource(1, 0), 30050);
        // Negative subtracts, also below zero; population (3) and living space (4) are included.
        assert_eq!(go(&mut sim, 1, add(1, 0, -50)), Outcome::Accepted);
        assert_eq!(fx(&sim, 0), gold);
        assert_eq!(go(&mut sim, 1, add(1, 1, -40_000)), Outcome::Accepted);
        assert_eq!(sim.state.resource(1, 1), -10_000);
        assert_eq!(go(&mut sim, 1, add(1, 2, 7)), Outcome::Accepted);
        assert_eq!(go(&mut sim, 1, add(1, 3, 11)), Outcome::Accepted);
        assert_eq!(go(&mut sim, 1, add(1, 4, 13)), Outcome::Accepted);
        assert_eq!(fx(&sim, 3), 11 << 12);
        assert_eq!(fx(&sim, 4), 13 << 12);
        // No upper limit and no panic case: the account calculates with overflow like the original.
        assert_eq!(go(&mut sim, 1, add(1, 0, i32::MAX)), Outcome::Accepted);
        assert_eq!(go(&mut sim, 1, add(1, 0, i32::MIN)), Outcome::Accepted);
        // The AI takes the same path as the control from outside.
        sim.origin = crate::command::Origin::Ki;
        assert_eq!(go(&mut sim, 1, add(1, 2, 1)), Outcome::Accepted);
        sim.origin = crate::command::Origin::External;

        // Index outside 0 ... 4, foreign slot, human slot, slot outside: rejected.
        let before = checksum(sim.state);
        for resource in [5, -1, 9, i32::MAX] {
            assert_rejected_with(go(&mut sim, 1, add(1, resource, 50)), "resource");
        }
        assert_rejected_with(go(&mut sim, 1, add(2, 0, 50)), "is not the sender");
        assert_rejected_with(go(&mut sim, 0, add(0, 0, 50)), "human");
        for slot in [-1, 8, 11, 256] {
            assert_rejected_with(go(&mut sim, 1, add(slot, 0, 50)), "is not the sender");
        }
        assert_eq!(before, checksum(sim.state));
    }

    #[test]
    fn debug_destroy_kills_own_units_and_destroys_own_buildings() {
        use crate::module::building::destroy::destroy;
        let rules = rules();
        let mut state = state(&rules);
        let (unit, hq, mixed_unit, foreign_unit, foreign_hq, quartered, dying);
        {
            let mut sim = Sim::new(&mut state, &rules);
            unit = ck::spawn(&mut sim, ck::SABRE, 1, (10, 10));
            mixed_unit = ck::spawn(&mut sim, ck::SABRE, 1, (12, 10));
            hq = ck::spawn(&mut sim, ck::HQ, 1, (30, 30));
            // Far away so that the fighters do not slay each other.
            foreign_unit = ck::spawn(&mut sim, ck::SABRE, 0, (55, 5));
            foreign_hq = ck::spawn(&mut sim, ck::HQ, 0, (50, 50));
            quartered = ck::spawn(&mut sim, ck::SABRE, 1, (5, 55));
            let shelter = ck::spawn(&mut sim, ck::HQ, 1, (8, 55));
            send(
                &mut sim,
                shelter,
                &Message::new(0x1617, &[quartered.as_i32()]),
            );
            assert_ne!(sim.object(quartered).unwrap().status & status::QUARTERED, 0);
            dying = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
            sim.object_mut(dying).unwrap().status |= status::DIED;
            crate::names::register(&mut sim.state.names, "Haupthaus", hq);
            sim.state.events.clear();

            // Foreign, quartered, dying and missing objects: rejected, nothing happens.
            for objects in [
                vec![foreign_unit],
                vec![foreign_hq],
                vec![quartered],
                vec![dying],
                vec![ObjectId::new(900, 1, 3)],
                vec![ObjectId::NONE],
            ] {
                let before = checksum(sim.state);
                let outcome = go(&mut sim, 1, CommandKind::DebugDestroy { objects });
                assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
                assert_eq!(before, checksum(sim.state));
            }
            assert!(sim.state.events.is_empty());
            assert_eq!(crate::world::object_event_bits(&sim, foreign_hq), 0);

            // Never from a script action: not for foreign or own objects either.
            for objects in [vec![foreign_unit], vec![foreign_hq], vec![unit]] {
                let before = checksum(sim.state);
                let cmd = Command {
                    player: 1,
                    kind: CommandKind::DebugDestroy { objects },
                };
                match crate::player::submit_script(&mut sim, &cmd) {
                    Outcome::Rejected(why) => assert!(why.contains("not from scripts"), "{why}"),
                    other => panic!("expected Rejected, got {other:?}"),
                }
                assert_eq!(before, checksum(sim.state));
            }
            assert!(sim.state.events.is_empty());
            assert_eq!(sim.origin, crate::command::Origin::External);

            // An own unit like `Die`: from the selection, `{0x110}` with abort.
            assert_eq!(
                go(
                    &mut sim,
                    1,
                    CommandKind::DebugDestroy {
                        objects: vec![unit]
                    }
                ),
                Outcome::Accepted
            );
            assert!(
                sim.state
                    .events
                    .contains(&SimEvent::Deselected { object: unit })
            );
            let posted: Vec<_> = sim.object(unit).unwrap().fifo.iter().cloned().collect();
            assert_eq!(
                posted,
                vec![Posted::Message {
                    msg: Message::new(0x110, &[]),
                    abort: true
                }]
            );

            // Mixed: the own one takes effect, the foreign one does not.
            let outcome = go(
                &mut sim,
                1,
                CommandKind::DebugDestroy {
                    objects: vec![mixed_unit, foreign_unit],
                },
            );
            assert!(matches!(outcome, Outcome::Partial(_)), "{outcome:?}");
            assert!(sim.object(foreign_unit).unwrap().fifo.is_empty());

            // An own building: the normal destruction (event bit 2, dead, no longer selectable).
            assert_eq!(crate::world::object_event_bits(&sim, hq) & 4, 0);
            assert_eq!(
                go(&mut sim, 1, CommandKind::DebugDestroy { objects: vec![hq] }),
                Outcome::Accepted
            );
            assert_eq!(crate::world::object_event_bits(&sim, hq) & 4, 4);
            let o = sim.object(hq).unwrap();
            assert_ne!(o.status & status::DIED, 0);
            assert!(!o.selectable);
            // Destroying twice does not work: the dead building no longer belongs to the sender.
            assert!(matches!(
                go(&mut sim, 1, CommandKind::DebugDestroy { objects: vec![hq] }),
                Outcome::Rejected(_)
            ));
        }

        // Counter-check "as normal": the same state, the building via the normal destruction
        // (`destroy`), the units via `Die` - the sequence of the checksums is the same, the
        // name also stays on the dead ID like after any destruction (E-145).
        let mut normal = self::state(&rules);
        {
            let mut sim = Sim::new(&mut normal, &rules);
            let u = ck::spawn(&mut sim, ck::SABRE, 1, (10, 10));
            let m = ck::spawn(&mut sim, ck::SABRE, 1, (12, 10));
            let h = ck::spawn(&mut sim, ck::HQ, 1, (30, 30));
            ck::spawn(&mut sim, ck::SABRE, 0, (55, 5));
            ck::spawn(&mut sim, ck::HQ, 0, (50, 50));
            let q = ck::spawn(&mut sim, ck::SABRE, 1, (5, 55));
            let shelter = ck::spawn(&mut sim, ck::HQ, 1, (8, 55));
            send(&mut sim, shelter, &Message::new(0x1617, &[q.as_i32()]));
            let d = ck::spawn(&mut sim, ck::SABRE, 1, (5, 5));
            sim.object_mut(d).unwrap().status |= status::DIED;
            crate::names::register(&mut sim.state.names, "Haupthaus", h);
            go(&mut sim, 1, CommandKind::Die { units: vec![u, m] });
            destroy(&mut sim, h);
        }
        for _ in 0..700 {
            crate::tick::step(&mut state, &rules, &[]);
            crate::tick::step(&mut normal, &rules, &[]);
            assert_eq!(checksum(&state), checksum(&normal));
        }
        let sim = Sim::new(&mut state, &rules);
        for id in [unit, mixed_unit, hq] {
            assert!(sim.object(id).is_none(), "{id:?} is dismantled");
        }
        assert_eq!(
            crate::names::first_id(&sim.state.names, "haupthaus"),
            Some(hq),
            "the name points to the dead ID like after any destruction"
        );
        assert_eq!(crate::world::object_event_bits(&sim, hq) & 4, 4);
        // The foreign and the quartered unit as well as the foreign house are still alive.
        for id in [foreign_unit, foreign_hq, quartered] {
            assert!(Sim::new(&mut state, &rules).object(id).is_some());
        }
    }
}
