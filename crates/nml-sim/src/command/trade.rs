//! Command `0x45` "Trade with player" (`GameWorld_CmdTradeWithPlayer` `0x00477B90`, re/trading
//! 2.2): per unit post `{0x12B, ware, target}` with abort. In addition the tribute `0x65`
//! ([`execute_tribute`], re/heroes-resources 3.7).
//!
//! The original checks neither owner nor module nor range and so aborts the order of foreign objects
//! and of objects without module 43 as well (K-289). The reimplementation checks like the partner menu
//! `FUN_0044B9A0` (A8): kind 0 to 3, target 0 to 8, target != sender, the target slot
//! owns a trade building (class bit `TRADEBUILDING` on module 3, not dead or quartered;
//! `FINISHED` is not required, but a construction site no longer has the class bit). Target 8
//! stays allowed if slot 8 owns a trade building (W-235). Foreign units and units
//! without module 43 are skipped, not rejected. The UI of the original does not reach the
//! command in the shipped data (W-245); the trigger is the control channel.

use super::{CommandKind, owns, reject, skip_passed};
use crate::economy::{announce, message};
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::message::Message;
use crate::messaging::post_message;
use crate::module::trading;
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

/// Module 3 (building) and its class bit `TRADEBUILDING` (`MATCHBITS 0x8000003`, 40003 bit
/// `0x8000000`).
const BUILDING: usize = 3;
const TRADE_BUILDING: u32 = 0x0800_0000;

/// Entry point for `0x45` and `0x65` (`command::execute` routes them here). Synchronous, without
/// acknowledgment, the command queues stay (re/trading 2.2, re/heroes-resources 3.7).
pub fn execute(sim: &mut Sim, player: u8, kind: &CommandKind, _direct: bool) {
    match kind {
        CommandKind::TradeWithPlayer {
            ware,
            target,
            units,
        } => {
            if let Err(why) = check_trade(sim, player, *ware, *target) {
                reject(sim, player, "Trade with player", why);
                return;
            }
            execute_trade(sim, player, *ware, *target, units);
        }
        &CommandKind::Tribute {
            from,
            recipients,
            resource,
            amount,
        } => {
            if let Err(why) = check_tribute(player, from, resource, amount) {
                reject(sim, player, "Tribute", why);
                return;
            }
            execute_tribute(sim, from, recipients, resource, amount);
        }
        _ => unreachable!("trade::execute only for 0x45 and 0x65"),
    }
}

/// Check of `0x45` like the partner menu. The search for the trade building
/// is `FUN_00477B30({MM_MODE 2, MATCHBITS 0x8000003}, target)` by hand
/// (`filter::matches` needs `&mut Sim`): an object of the target slot with module 3 and
/// class bit `TRADEBUILDING`, not dead or quartered. As in the filter with `MM_PROPID 0`, a
/// switched-off module 3 counts too (`filter.rs`, type bits). The flag `FINISHED` (`0x400`) is
/// not required; a construction site still does not match because `StartBuild` clears the class bit
/// (`ClearFlags(0xFDCFFFFC)`, re/buildings, `building::flags::CLEARED_BY_START_BUILD`).
pub fn check_trade(sim: &Sim, player: u8, ware: i32, target: i32) -> Result<(), String> {
    if !(0..=3).contains(&ware) {
        return Err(format!("trade kind {ware} invalid (allowed 0 to 3)"));
    }
    if !(0..=8).contains(&target) {
        return Err(format!("trade target {target} invalid (allowed 0 to 8)"));
    }
    if target == i32::from(player) {
        return Err("trade with oneself".into());
    }
    if !has_trade_building_in(sim.state, target) {
        return Err(format!("slot {target} has no trade building"));
    }
    Ok(())
}

/// Does the slot `slot` have a trade building (`FUN_00477B30({MM_MODE 2, MATCHBITS 0x8000003}, slot)` by hand, see
/// [`check_trade`])? Read-only; also for the trade partners of the command bar (`FUN_0044B9A0`).
/// Slots outside the tables: no.
pub fn has_trade_building_in(state: &crate::state::SimState, slot: i32) -> bool {
    usize::try_from(slot)
        .ok()
        .and_then(|s| state.tables.get(s))
        .is_some_and(|t| {
            t.objects.iter().flatten().any(|o| {
                o.status & (status::DIED | status::QUARTERED) == 0
                    && o.has_module(BUILDING, true)
                    && o.module(BUILDING)
                        .is_some_and(|m| m.base.flags & TRADE_BUILDING != 0)
            })
        })
}

/// Execution of `0x45` after the check: per own unit with module 43
/// `PostMessage({0x12B, ware, target}, abort)`. The abort happens only when the FIFO is processed in the
/// object tick of the unit (`AbortAllModules`, then `0x12B`). Foreign units and those without
/// module 43 are skipped (K-289, deviation).
pub fn execute_trade(sim: &mut Sim, player: u8, ware: i32, target: i32, units: &[ObjectId]) {
    for &unit in units {
        if !owns(sim, player, unit) {
            continue;
        }
        if !sim
            .object(unit)
            .is_some_and(|o| o.has_module(trading::INDEX, false))
        {
            skip_passed(
                sim,
                player,
                "Trade with player",
                format!("unit {} without module 43", unit.0),
            );
            continue;
        }
        post_message(
            sim,
            unit,
            Message::new(trading::msg::START, &[ware, target]),
            true,
        );
    }
}

/// Check of `0x65` like the diplomacy dialog (A8): `from` is the sender, the amount is positive and
/// the resource is gold, wood or food (0 to 2). The original checks none of this (C-226); the
/// reimplementation does not check the receiver either (C-231).
pub fn check_tribute(player: u8, from: i32, resource: i32, amount: i32) -> Result<(), String> {
    if from != i32::from(player) {
        return Err(format!("giver {from} is not the sender"));
    }
    if amount <= 0 {
        return Err(format!("amount {amount} not positive"));
    }
    if !(0..=2).contains(&resource) {
        return Err(format!("resource {resource} invalid (allowed 0 to 2)"));
    }
    Ok(())
}

/// `GameWorld_CmdSendTribute` (`0x00478840`, re/heroes-resources 3.7) without command check: a
/// giver outside 0 to 7 pays nothing (`GameEngine_GetPlayer` `0x0042D560` returns null). For
/// every slot `q` 0 to 7 with a bit in `recipients`, except the giver, and regardless of whether the
/// slot is occupied (C-231): if the stock of the giver (`Player_GetResource`, living space
/// clamped) is not enough for the **full** amount, it reports the shortage 2/1/0 for gold/wood/food
/// and ends - later receivers get nothing (C-225, C-227). Otherwise it books `−amount`/`+amount`
/// and reports `TributeSent` (message 230). Resource indices outside 0 to 4 read 0 and book
/// nothing; 3 and 4 as well as negative amounts pass through as in the original (C-226).
pub fn execute_tribute(sim: &mut Sim, from: i32, recipients: u32, resource: i32, amount: i32) {
    let Some(giver) = usize::try_from(from).ok().filter(|&g| g < PLAYER_SLOTS) else {
        return;
    };
    let res = usize::try_from(resource).unwrap_or(usize::MAX);
    for q in 0..PLAYER_SLOTS {
        if q == giver || recipients & (1 << q) == 0 {
            continue;
        }
        if sim.state.resource(giver, res) < amount {
            // `0x00478945…0x004789CD`: message only for 0 to 2, in the original only at the local giver.
            if let Some(&id) = message::MISSING[..3].get(res) {
                announce(sim, from, id, (-1, -1), message::flags::TRIBUTE_LACK);
            }
            return;
        }
        sim.state.players[giver].add_resource(res, amount.wrapping_neg());
        sim.state.players[q].add_resource(res, amount);
        sim.state.events.push(SimEvent::TributeSent {
            giver: giver as u8,
            recipient: q as u8,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, QueuedCommand, enqueue, execute};
    use crate::message::Posted;
    use crate::module::tent::testkit::*;
    use crate::module::{ModuleKind, walk};
    use crate::save::checksum;
    use crate::state::SimEvent;

    /// Tiles: HQ 1000 (slot 1), HQ 1900 and trading office 1907 (slot 2), cart start.
    const HOME: (i32, i32) = (20, 20);
    const PARTNER: (i32, i32) = (44, 20);
    const TRADE_POST_TILE: (i32, i32) = (44, 26);
    const MIDDLE: (i32, i32) = (32, 32);

    fn trade_cmd(player: u8, ware: i32, target: i32, units: Vec<ObjectId>) -> Command {
        Command {
            player,
            kind: CommandKind::TradeWithPlayer {
                ware,
                target,
                units,
            },
        }
    }

    fn move_cmd(player: u8, unit: ObjectId, tile: (i32, i32)) -> Command {
        Command {
            player,
            kind: CommandKind::Move {
                units: vec![unit],
                x: (tile.0 << 12) + 0x800,
                z: (tile.1 << 12) + 0x800,
                target: -1,
                formation: 0,
                direction: crate::motion::NO_DIRECTION,
                spread: crate::command::NO_SPREAD,
                flags: 0,
            },
        }
    }

    /// HQ 1000 (slot 1), HQ 1900 and trading office 1907 (slot 2), cart 1110 (slot 1).
    fn trade_world(sim: &mut Sim) -> ObjectId {
        spawn(sim, SPA_HQ, 1, HOME);
        spawn(sim, ENG_HQ, 2, PARTNER);
        spawn(sim, TRADE_POST, 2, TRADE_POST_TILE);
        spawn(sim, CART, 1, MIDDLE)
    }

    /// Posted `0x12B` in the FIFO: `(ware, target, abort)`.
    fn posted_trades(sim: &Sim, id: ObjectId) -> Vec<(i32, i32, bool)> {
        sim.object(id)
            .unwrap()
            .fifo
            .iter()
            .filter_map(|p| match p {
                Posted::Message { msg, abort } if msg.code == trading::msg::START => {
                    Some((msg.arg(0), msg.arg(1), *abort))
                }
                _ => None,
            })
            .collect()
    }

    fn trade_of(sim: &Sim, id: ObjectId) -> (i32, i32) {
        match &sim.module(id, trading::INDEX).expect("module 43").kind {
            ModuleKind::Trading(t) => (t.trade_ware, t.trade_target),
            k => panic!("no trade: {k:?}"),
        }
    }

    fn walk_replies(sim: &Sim, id: ObjectId) -> (Option<u32>, Option<u32>) {
        let b = &sim.module(id, walk::INDEX).expect("Walk").base;
        (
            b.success.as_ref().map(|m| m.code),
            b.fault.as_ref().map(|m| m.code),
        )
    }

    /// Execute the command (`direct`); checksum before and after.
    fn run(sim: &mut Sim, command: &Command) -> (u64, u64) {
        let before = checksum(sim.state);
        execute(sim, command, true);
        (before, checksum(sim.state))
    }

    #[test]
    fn trade_command_code() {
        let kind = CommandKind::TradeWithPlayer {
            ware: 2,
            target: 8,
            units: vec![ObjectId(0x10800), ObjectId(0x10801)],
        };
        assert_eq!(kind.type_code(), 0x5000_0045);
        let command = Command { player: 1, kind };
        let bytes = postcard::to_stdvec(&command).unwrap();
        assert_eq!(postcard::from_bytes::<Command>(&bytes).unwrap(), command);
    }

    #[test]
    fn trade_command_checks_like_the_menu() {
        // Check like the partner menu: kind 0...3, target 0...8, target != sender, the target owns a
        // trade building; when rejected the checksum stays the same.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let cart = trade_world(&mut sim);
        // Slot 1 itself has a trading office: "target = sender" fails only on `target == player`.
        spawn(&mut sim, TRADE_POST, 1, (10, 30));
        for (ware, target) in [
            (0, 1),  // target = sender
            (0, 9),  // target outside
            (0, -1), // target outside
            (4, 2),  // kind outside
            (-1, 2), // kind outside
            (0, 0),  // slot 0 without trade building
            (0, 8),  // slot 8 without trade building
        ] {
            let (before, after) = run(&mut sim, &trade_cmd(1, ware, target, vec![cart]));
            assert_eq!(before, after, "kind {ware}, target {target}");
            assert_eq!(
                posted_trades(&sim, cart),
                [],
                "kind {ware}, target {target}"
            );
        }
        // A dead or quartered trade building does not count.
        let dead = spawn(&mut sim, TRADE_POST, 0, (10, 40));
        sim.object_mut(dead).unwrap().status |= status::DIED;
        let quartered = spawn(&mut sim, TRADE_POST, 0, (16, 40));
        sim.object_mut(quartered).unwrap().status |= status::QUARTERED;
        let (before, after) = run(&mut sim, &trade_cmd(1, 0, 0, vec![cart]));
        assert_eq!(before, after);

        // Measured: a construction site of the trading office does not count - `StartBuild` clears the
        // class bit `TRADEBUILDING` (`ClearFlags(0xFDCFFFFC)`, re/buildings).
        let unfinished = site(&mut sim, TRADE_POST, 0, (20, 50));
        let m3 = sim.module(unfinished, BUILDING).unwrap();
        assert!(m3.base.enabled && m3.base.flags & TRADE_BUILDING == 0);
        let (before, after) = run(&mut sim, &trade_cmd(1, 0, 0, vec![cart]));
        assert_eq!(before, after);

        // Accepted: target 2 (finished 1907), target 8 with neutral 1907 (W-235).
        let (before, after) = run(&mut sim, &trade_cmd(1, 3, 2, vec![cart]));
        assert_ne!(before, after);
        assert_eq!(posted_trades(&sim, cart), [(3, 2, true)]);
        spawn(&mut sim, TRADE_POST, 8, (10, 50));
        let (before, after) = run(&mut sim, &trade_cmd(1, 0, 8, vec![cart]));
        assert_ne!(before, after);
        assert_eq!(posted_trades(&sim, cart), [(3, 2, true), (0, 8, true)]);
    }

    #[test]
    fn trade_command_skips_foreign_units_and_units_without_module_43() {
        // K-289 (deviation): the foreign cart (slot 2) and the own colonist without module 43
        // get nothing and keep their walk order; only the own cart trades.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let cart = trade_world(&mut sim);
        let foreign = spawn(&mut sim, CART, 2, (40, 40));
        let colonist = spawn(&mut sim, COLONIST, 1, (24, 40));
        crate::tick::step(
            sim.state,
            sim.rules,
            &[
                move_cmd(2, foreign, (50, 50)),
                move_cmd(1, colonist, (10, 50)),
            ],
        );
        run_steps(&mut sim, 4);
        let orders = [walk_replies(&sim, foreign), walk_replies(&sim, colonist)];
        assert!(orders.iter().all(|r| r.0.is_some()), "{orders:?}");

        execute(
            &mut sim,
            &trade_cmd(1, 0, 2, vec![foreign, colonist, cart]),
            true,
        );
        assert_eq!(posted_trades(&sim, foreign), []);
        assert_eq!(posted_trades(&sim, colonist), []);
        assert_eq!(posted_trades(&sim, cart), [(0, 2, true)]);
        let pos = |sim: &Sim, id| sim.object(id).unwrap().grid;
        let before = [pos(&sim, foreign), pos(&sim, colonist)];
        run_steps(&mut sim, 4);
        assert_eq!(
            [walk_replies(&sim, foreign), walk_replies(&sim, colonist)],
            orders
        );
        assert_ne!(pos(&sim, foreign), before[0], "foreign cart keeps walking");
        assert_ne!(pos(&sim, colonist), before[1], "colonist keeps walking");
        assert_eq!(trade_of(&sim, foreign), (0, 0));
        assert_eq!(trade_of(&sim, cart), (0, 2));
    }

    #[test]
    fn trade_command_aborts_the_current_order_when_executed() {
        // re/trading 2.2: `PostMessage(.., abort)` - the unit gets `0x12B` only when the FIFO is
        // processed in its own object tick (at most 2 `step`), before that `AbortAllModules`.
        // No acknowledgment, the command queue stays.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let cart = trade_world(&mut sim);
        crate::tick::step(sim.state, sim.rules, &[move_cmd(1, cart, (24, 44))]);
        run_steps(&mut sim, 4);
        let order = walk_replies(&sim, cart);
        assert!(
            order.0.is_some_and(|c| c & 0xFF != trading::INDEX as u32),
            "{order:?}"
        );
        enqueue(
            &mut sim,
            cart,
            QueuedCommand {
                cyclic: false,
                command: move_cmd(1, cart, (30, 30)),
            },
        );

        execute(&mut sim, &trade_cmd(1, 1, 2, vec![cart]), true);
        // Nothing executed yet: only posted, the walk order is running, the queue stays.
        assert_eq!(posted_trades(&sim, cart), [(1, 2, true)]);
        assert_eq!(trade_of(&sim, cart), (0, 0));
        assert_eq!(walk_replies(&sim, cart), order);
        assert_eq!(sim.object(cart).unwrap().commands.len(), 1);

        let mut n = 0;
        while trade_of(&sim, cart) != (1, 2) {
            assert!(n < 2, "0x12B not processed after 2 steps");
            run_steps(&mut sim, 1);
            n += 1;
        }
        // Aborted and restarted: trip to the own HQ.
        assert_eq!(posted_trades(&sim, cart), []);
        assert_eq!(
            walk_replies(&sim, cart),
            (Some(trading::msg::AT_HOME), Some(trading::msg::HOME_FAILED))
        );
        assert_eq!(sim.object(cart).unwrap().commands.len(), 1);
    }

    fn tribute(from: i32, recipients: u32, resource: i32, amount: i32) -> CommandKind {
        CommandKind::Tribute {
            from,
            recipients,
            resource,
            amount,
        }
    }

    /// Messages and tributes among the events.
    fn tribute_events(sim: &Sim) -> Vec<SimEvent> {
        sim.state
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    SimEvent::PlayerMessage { .. } | SimEvent::TributeSent { .. }
                )
            })
            .cloned()
            .collect()
    }

    fn sent(giver: u8, recipient: u8) -> SimEvent {
        SimEvent::TributeSent { giver, recipient }
    }

    /// Stock (whole units, unclamped) of the resource `res` of slots 0, 1, 2.
    fn stocks(sim: &Sim, res: usize) -> [i32; 3] {
        [0, 1, 2].map(|p| sim.state.players[p].resource_raw(res) >> 12)
    }

    #[test]
    fn tribute_command_code() {
        let kind = tribute(1, 0b1001, 2, 300);
        assert_eq!(kind.type_code(), 0x5000_0065);
        let command = Command { player: 1, kind };
        let bytes = postcard::to_stdvec(&command).unwrap();
        assert_eq!(postcard::from_bytes::<Command>(&bytes).unwrap(), command);
    }

    #[test]
    fn tribute_lack_has_priority_0_and_goes_to_the_giver() {
        // `GameWorld_CmdSendTribute` `0x00478840`: shortage message only with `0x10000`, priority 0, to the
        // giver; the receiver slots see nothing.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.players[0].set_resource(0, 100);
        execute_tribute(&mut sim, 0, 0b0110, 0, 300);
        assert_eq!(
            sim.state.events,
            [SimEvent::PlayerMessage {
                to: Some(0),
                id: message::NO_GOLD,
                tile: (-1, -1),
                flags: 0x0001_0000,
            }]
        );
    }

    #[test]
    fn tribute_follows_the_original() {
        // re/heroes-resources 3.7, 9.3: directly via `execute_tribute` (without command check).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let run = |sim: &mut Sim, from, recipients, resource, amount| {
            sim.state.events.clear();
            execute_tribute(sim, from, recipients, resource, amount);
            tribute_events(sim)
        };

        // Gold 500, the full amount per receiver: slot 1 gets 300, for slot 2 the rest is not enough
        // - shortage message 2 to the giver, end (C-225, C-227).
        sim.state.players[0].set_resource(0, 500);
        let events = run(&mut sim, 0, 0b0110, 0, 300);
        assert_eq!(stocks(&sim, 0), [200, 1300, 1000]);
        assert_eq!(
            events,
            [
                sent(0, 1),
                SimEvent::PlayerMessage {
                    to: Some(0),
                    id: 2,
                    tile: (-1, -1),
                    flags: message::flags::TRIBUTE_LACK,
                }
            ]
        );

        // Wood 1000, bit of the giver skipped: slots 1 and 2 get 300 each.
        let events = run(&mut sim, 0, 0b0111, 1, 300);
        assert_eq!(stocks(&sim, 1), [400, 1300, 1300]);
        assert_eq!(events, [sent(0, 1), sent(0, 2)]);

        // Wood and food: shortage messages 1 and 0 (`message::MISSING`).
        let events = run(&mut sim, 0, 0b0010, 1, 401);
        assert_eq!(stocks(&sim, 1), [400, 1300, 1300]);
        assert_eq!(
            events,
            [SimEvent::PlayerMessage {
                to: Some(0),
                id: 1,
                tile: (-1, -1),
                flags: message::flags::TRIBUTE_LACK,
            }]
        );

        // Food 100, negative amount: the giver gains, the receiver loses (C-226).
        sim.state.players[0].set_resource(2, 100);
        let events = run(&mut sim, 0, 0b0010, 2, -50);
        assert_eq!(stocks(&sim, 2), [150, 950, 1000]);
        assert_eq!(events, [sent(0, 1)]);
        let events = run(&mut sim, 0, 0b0010, 2, 151);
        assert_eq!(stocks(&sim, 2), [150, 950, 1000]);
        assert_eq!(
            events,
            [SimEvent::PlayerMessage {
                to: Some(0),
                id: 0,
                tile: (-1, -1),
                flags: message::flags::TRIBUTE_LACK,
            }]
        );

        // Living space (index 4) 40 below the limit: transferable (C-226), read clamped.
        assert!(sim.state.economy.max_living_space > 40);
        sim.state.players[0].set_resource(4, 40);
        sim.state.players[1].set_resource(4, 0);
        let events = run(&mut sim, 0, 0b0010, 4, 10);
        assert_eq!(stocks(&sim, 4)[..2], [30, 10]);
        assert_eq!(events, [sent(0, 1)]);
        // Clamped read: stock 200 above the limit 125 is not enough for 126, without message.
        sim.state.players[0].set_resource(4, 200);
        let events = run(&mut sim, 0, 0b0010, 4, 126);
        assert_eq!(stocks(&sim, 4)[..2], [200, 10]);
        assert_eq!(events, []);

        // Resource 7 does not exist ("has 0"): nothing, no message; likewise a giver outside
        // 0...7 and receiver bits from 8.
        let before = checksum(sim.state);
        for (from, recipients, resource) in [
            (0, 0b0010, 7),
            (0, 0b0010, -1),
            (9, 0b0010, 0),
            (-1, 0b0010, 0),
            (8, 0b0010, 0),
            (0, 0b1_0000_0000, 0),
            (0, 0b0001, 0),
        ] {
            let events = run(&mut sim, from, recipients, resource, 10);
            assert_eq!(events, [], "{from} {recipients:#b} {resource}");
            assert_eq!(
                checksum(sim.state),
                before,
                "{from} {recipients:#b} {resource}"
            );
        }

        // Unoccupied slot 5 gets the tribute (C-231, `GameEngine_GetPlayer` `0x0042D560`).
        assert!(!sim.state.players[5].slot_active);
        let gold5 = sim.state.players[5].resource_raw(0) >> 12;
        let events = run(&mut sim, 0, 1 << 5, 0, 100);
        assert_eq!(sim.state.players[5].resource_raw(0) >> 12, gold5 + 100);
        assert_eq!(stocks(&sim, 0)[0], 100);
        assert_eq!(events, [sent(0, 5)]);
    }

    #[test]
    fn tribute_commands_check_amount_resource_and_sender() {
        // re/heroes-resources (A8): amount > 0, resource 0...2, `from` = sender; when rejected
        // the checksum stays the same.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        for kind in [
            tribute(0, 0b0010, 2, -50),
            tribute(0, 0b0010, 0, 0),
            tribute(0, 0b0010, 3, 10),
            tribute(0, 0b0010, 4, 10),
            tribute(0, 0b0010, -1, 10),
            tribute(1, 0b0001, 0, 10),
        ] {
            sim.state.events.clear();
            let (before, after) = run(
                &mut sim,
                &Command {
                    player: 0,
                    kind: kind.clone(),
                },
            );
            assert_eq!(before, after, "{kind:?}");
            assert_eq!(tribute_events(&sim), [], "{kind:?}");
        }
        assert_eq!(check_tribute(0, 0, 2, 1), Ok(()));
        let (before, after) = run(
            &mut sim,
            &Command {
                player: 0,
                kind: tribute(0, 0b0110, 1, 100),
            },
        );
        assert_ne!(before, after);
        assert_eq!(stocks(&sim, 1), [800, 1100, 1100]);
    }
}
