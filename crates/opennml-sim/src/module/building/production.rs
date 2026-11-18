//! Production, research and trade of the building module (re/buildings): the queue
//! (`0xE03` enqueue, `0xF03` progress, `0x1003` list, `0x1103` cancel), the production tick
//! with payment at the **start** (otherwise waiting tick by tick), the special cases of the paddocks,
//! shipyards and the faster training, the trade in the trading post (paid at the **end**) and the
//! release of the counters in the destructor. The spawning of the finished unit is in [`super::rally`].
//! Also the debug tool "complete immediately" ([`complete_now`]) via the same start and
//! completion path.
//!
//! A finished rail order (2518) places via [`super::track::place_next_track_segment`] a
//! construction site behind the last track; the return value is ignored (W-206: without a track the
//! payment is lost).
//!
//! A finished research reports "research done" and applies the upgrade immediately
//! (`Player_ApplyUpgrade`, [`crate::upgrades::apply_upgrade`], re/upgrades 3.1).
//!
//! Research and trade write the statistic (kinds 2 and 11, [`crate::accounting`]); the
//! training writes it in [`super::rally`]. UI notification
//! (`GameObject_NotifyUiIfSelected`): only the cancel reports itself as
//! [`SimEvent::ProductionCancelled`]; enqueueing, start and completion are derived by the host from the state
//! (re/game-ui N.12, N.14.13).

use opennml_formats::gifcon::button_kind;

use crate::accounting::{add_statistic, kind as stat};
use crate::economy::{
    add_owner_object_costs_percent, add_owner_pending_guid_count, announce,
    can_owner_afford_object, guid_count, market_adjust, market_convert, message,
    pending_guid_count, resource,
};
use crate::id::ObjectId;
use crate::message::Message;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::{MAX_QUEUE, building, building_mut, clear_flags, flags, rally, set_flags, track};

/// Paddocks: the paddock GUID becomes a horse color in turn when enqueueing (one counter
/// `m_LastHorse` for both paddocks).
pub const PADDOCK_A: u32 = 3423;
pub const PADDOCK_B: u32 = 3424;
/// Colors of paddock 3423 or 3424 in counter sequence 0 -> 1 -> 2 -> 0.
pub const HORSES_A: [u32; 3] = [3406, 3409, 3410];
pub const HORSES_B: [u32; 3] = [3417, 3418, 3419];
/// Rails (station, [`super::track`]).
pub const RAILS: u32 = 2518;
/// If this GUID counts in the owner's object counter (with groups), every production runs at half speed
/// (-50 instead of -100 ms per building tick).
pub const SLOW_PRODUCTION: u32 = 3374;
/// Shipyard upgrade: the ships 2009/2010 then need 7/10 of the time.
pub const SHIPYARD_UPGRADE: u32 = 3129;
pub const SHIPS: [u32; 2] = [2009, 2010];
/// Trade GUIDs (`Building_IsTradeGuid` `0x00505150`).
pub mod trade {
    pub const BUY_WOOD: u32 = 490;
    pub const BUY_FOOD: u32 = 491;
    pub const SELL_WOOD: u32 = 492;
    pub const SELL_FOOD: u32 = 493;
    /// Goods per trade.
    pub const AMOUNT: i32 = 100;
    /// Duration of a trade in seconds.
    pub const SECONDS: i32 = 8;
}
/// New attempt after this remaining time (ms) when the spawn or the payment fails.
pub const RETRY_MS: i32 = 100;
/// Toolbar kinds of which only one order may be "in progress".
const ONCE_KINDS: [u32; 3] = [
    button_kind::UPGRADE,
    button_kind::ONLYONCE,
    button_kind::MAKEONLY_ONCE,
];

/// `Building_IsTradeGuid` (`0x00505150`): 490 ... 493.
pub fn is_trade_guid(guid: u32) -> bool {
    (trade::BUY_WOOD..=trade::SELL_FOOD).contains(&guid)
}

/// Toolbar kind of the entry with `BUTTONMSG = guid` (`GameWorld_LookupToolbarEntry` `0x0047E440`).
fn toolbar_kind(sim: &Sim, guid: u32) -> Option<u32> {
    sim.rules.toolbar.lookup(guid as i32).map(|e| e.kind())
}

/// `GameObject_IsUpgradeGuid` (`0x004670A0`) = `GameWorld_IsToolbarClass(guid, UPGRADE)`.
fn is_upgrade_guid(sim: &Sim, guid: u32) -> bool {
    toolbar_kind(sim, guid) == Some(button_kind::UPGRADE)
}

/// `GameObject_GetOwnerConfigValue` (`0x00466930`): `PlayerConfig_GetValue` of the owner slot.
fn owner_config(sim: &Sim, id: ObjectId, guid: u32, cfg: u32) -> i32 {
    sim.rules.config_value(id.slot(), guid, cfg)
}

/// Book population (config 22) and living space (config 27) of the order with the owner, `sign` +1
/// at the start, -1 on cancel and demolition (config values, not the attributes - quirk: the
/// completion of a building books from the attributes).
fn book_population(sim: &mut Sim, id: ObjectId, guid: u32, sign: i32) {
    let pop = owner_config(sim, id, guid, 22).wrapping_mul(sign);
    let space = owner_config(sim, id, guid, 27).wrapping_mul(sign);
    if let Some(p) = sim.state.players.get_mut(usize::from(id.slot())) {
        p.add_resource(resource::POPULATION, pop);
        p.add_resource(resource::LIVING_SPACE, space);
    }
}

/// Paddock GUID in turn to a horse color (state 0 -> 1 -> 2 -> 0); another counter value (only
/// via a savegame) leaves the GUID as it is.
fn paddock_horse(last_horse: &mut i32, colors: [u32; 3], guid: u32) -> u32 {
    let (next, g) = match *last_horse {
        0 => (1, colors[0]),
        1 => (2, colors[1]),
        2 => (0, colors[2]),
        _ => return guid,
    };
    *last_horse = next;
    g
}

/// `PropertyBuilding_StartMakeEntry` (`0x00504EC0`): take over the order, not yet paid.
fn start_make_entry(sim: &mut Sim, id: ObjectId, guid: u32) {
    if let Some(b) = building_mut(sim, id) {
        b.make_guid = guid;
        b.make_time = 0;
        b.make_start = 0;
    }
}

/// `PropertyBuilding_EnqueueProduction` (`0x00508670`, `0xE03 {guid}`): at most
/// [`MAX_QUEUE`] waiting orders (plus the running one). Paddocks become the next
/// horse color (even if the order is rejected afterward). A GUID of the toolbar kind
/// `UPGRADE`/`ONLYONCE`/`MAKEONLY_ONCE` is rejected as long as the counter "in progress" of the
/// owner for it is > 0 (only this counter, not the object counter). Otherwise "in progress" +1 (also
/// for trade); if nothing is running, the order becomes the running one (flag `MAKINGGUID`), otherwise it is
/// appended at the back; the message "Funds missing" may appear again. No check for finished,
/// sleeping or cost. Return value 1 or 0.
pub fn enqueue(sim: &mut Sim, id: ObjectId, guid: u32) -> i32 {
    let Some(b) = building_mut(sim, id) else {
        return 0;
    };
    if b.make_queue.len() >= MAX_QUEUE {
        return 0;
    }
    let guid = match guid {
        PADDOCK_A => paddock_horse(&mut b.last_horse, HORSES_A, guid),
        PADDOCK_B => paddock_horse(&mut b.last_horse, HORSES_B, guid),
        g => g,
    };
    let slot = i32::from(id.slot());
    if toolbar_kind(sim, guid).is_some_and(|k| ONCE_KINDS.contains(&k))
        && pending_guid_count(sim, slot, guid) > 0
    {
        return 0;
    }
    add_owner_pending_guid_count(sim.state, id, guid, 1);
    let running = building(sim, id).is_some_and(|b| b.make_guid != 0);
    if running {
        if let Some(b) = building_mut(sim, id) {
            b.make_queue.push_back(guid);
        }
    } else {
        set_flags(sim, id, flags::MAKINGGUID);
        start_make_entry(sim, id, guid);
    }
    if let Some(b) = building_mut(sim, id) {
        b.msg_sent = false;
    }
    1
}

/// `PropertyBuilding_TickProduction` (`0x0050C210`), per building tick with a running order (not
/// dead): nothing when sleeping. If the order is running (remaining time > 0), it drops by 100 ms (-50 if
/// [`SLOW_PRODUCTION`] counts in the owner's object counter); when it reaches 0, it is completed
/// ([`finish_entry`]) - if that fails, new attempt with remaining time [`RETRY_MS`] - and the next
/// waiting one moves up (none: flag `MAKINGGUID` off, end). Then in the same tick the start:
/// trade takes 8 s without payment; otherwise the owner must be able to bear the cost
/// (`CanOwnerAffordObject`, silent if "Funds missing" has already been reported), otherwise the order stays
/// unpaid (recheck in the next tick, no advancing). Paid are 100 % of the
/// cost, population and living space from the config; run time config 13 seconds (shipyard ships with
/// upgrade 7/10, truncated), in ms as remaining and total time.
pub fn tick_production(sim: &mut Sim, id: ObjectId) {
    let Some(b) = building(sim, id) else {
        return;
    };
    if b.is_sleeping {
        return;
    }
    let slot = i32::from(id.slot());
    if b.make_time > 0 {
        let d = if guid_count(sim, slot, SLOW_PRODUCTION) != 0 {
            50
        } else {
            100
        };
        let Some(b) = building_mut(sim, id) else {
            return;
        };
        b.make_time = b.make_time.wrapping_sub(d);
        if b.make_time > 0 {
            return;
        }
        let guid = b.make_guid;
        if !finish_entry(sim, id, guid) {
            if let Some(b) = building_mut(sim, id) {
                b.make_time = RETRY_MS;
            }
            return;
        }
        let Some(b) = building_mut(sim, id) else {
            return;
        };
        b.make_guid = 0;
        match b.make_queue.pop_front() {
            Some(next) => b.make_guid = next,
            None => {
                b.make_time = 0;
                b.make_start = 0;
                clear_flags(sim, id, flags::MAKINGGUID);
                return;
            }
        }
    }
    start_entry(sim, id);
}

/// Start of the running order (second part of `PropertyBuilding_TickProduction`).
fn start_entry(sim: &mut Sim, id: ObjectId) {
    let Some(b) = building(sim, id) else {
        return;
    };
    let (guid, silent) = (b.make_guid, b.msg_sent);
    let secs = if is_trade_guid(guid) {
        trade::SECONDS
    } else {
        if !pay_start(sim, id, guid, silent) {
            if let Some(b) = building_mut(sim, id) {
                b.make_time = 0;
                b.make_start = 0;
            }
            return;
        }
        let mut t = owner_config(sim, id, guid, 13);
        if SHIPS.contains(&guid) && guid_count(sim, i32::from(id.slot()), SHIPYARD_UPGRADE) != 0 {
            t = t.wrapping_mul(7) / 10;
        }
        t
    };
    if let Some(b) = building_mut(sim, id) {
        b.make_time = secs.wrapping_mul(1000);
        b.make_start = b.make_time;
    }
}

/// Start check and payment of an order (except trade) from `PropertyBuilding_TickProduction`:
/// if the owner cannot bear the cost (`CanOwnerAffordObject`, message unless `silent`),
/// "Funds missing" is noted as reported and `false` returned; otherwise reset, 100 % of the
/// cost deducted, population and living space booked from the config.
fn pay_start(sim: &mut Sim, id: ObjectId, guid: u32, silent: bool) -> bool {
    if !can_owner_afford_object(sim, id, guid, silent) {
        if let Some(b) = building_mut(sim, id) {
            b.msg_sent = true;
        }
        return false;
    }
    if let Some(b) = building_mut(sim, id) {
        b.msg_sent = false;
    }
    add_owner_object_costs_percent(sim, id, guid, -100);
    book_population(sim, id, guid, 1);
    true
}

/// Completion of the running order (part of `PropertyBuilding_TickProduction`), return value
/// `false` = new attempt:
/// - Rails 2518: "in progress" -1, then `PlaceNextTrackSegment` ([`track::place_next_track_segment`],
///   return value ignored as in the original: without a track or at the map edge the order lapses silently).
/// - Research (toolbar kind `UPGRADE`): `PropertyBuilding_FinishResearch` (`0x00504EE0`) - message
///   7 to the owner, then `Player_ApplyUpgrade(owner, GUID, not incremental)`
///   ([`crate::upgrades::apply_upgrade`]: counter +1, list, percentages, variant change), afterward the
///   statistic "upgrade researched" (kind 2); "in progress" stays +1 (quirk: blocks new
///   enqueueing).
/// - Trade: pay ([`pay_trade`]), then goods and price ([`execute_trade`]); "in progress" stays
///   (quirk).
/// - Otherwise spawn the unit ([`rally::spawn_produced_unit`]), then "in progress" -1.
fn finish_entry(sim: &mut Sim, id: ObjectId, guid: u32) -> bool {
    if guid == RAILS {
        add_owner_pending_guid_count(sim.state, id, guid, -1);
        track::place_next_track_segment(sim, id, guid);
    } else if is_upgrade_guid(sim, guid) {
        let tile = sim.object(id).map_or((0, 0), |o| o.tile);
        announce(
            sim,
            i32::from(id.slot()),
            message::RESEARCH_DONE,
            tile,
            message::flags::RESEARCH,
        );
        crate::upgrades::apply_upgrade(sim, i32::from(id.slot()), guid, false);
        // Statistic "upgrade researched" (`0x0050C2C1`, after `PropertyBuilding_FinishResearch`).
        add_statistic(
            sim,
            i32::from(id.slot()),
            stat::UPGRADE_RESEARCHED,
            guid as i32,
        );
    } else if is_trade_guid(guid) {
        if !pay_trade(sim, id, guid) {
            return false;
        }
        execute_trade(sim, id, guid);
    } else {
        if !rally::spawn_produced_unit(sim, id, guid, id.slot()) {
            return false;
        }
        add_owner_pending_guid_count(sim.state, id, guid, -1);
    }
    true
}

/// `PropertyBuilding_PayTrade` (`0x00505170`) at the end of the trade: buying costs gold
/// `Convert(price, 100)`, selling 100 gold of goods `Convert(price, −100)` (wood or food).
/// Without a player it counts as paid. If the balance is not enough, once per order message 2/1/0
/// ("gold/wood/food missing") at the tile of the building and `false`.
fn pay_trade(sim: &mut Sim, id: ObjectId, guid: u32) -> bool {
    let e = &sim.state.economy;
    let (res, amount) = match guid {
        trade::BUY_WOOD => (resource::GOLD, market_convert(e.wood_price, trade::AMOUNT)),
        trade::BUY_FOOD => (resource::GOLD, market_convert(e.food_price, trade::AMOUNT)),
        trade::SELL_WOOD => (resource::WOOD, market_convert(e.wood_price, -trade::AMOUNT)),
        trade::SELL_FOOD => (resource::FOOD, market_convert(e.food_price, -trade::AMOUNT)),
        _ => return true,
    };
    let max = e.max_living_space;
    let slot = usize::from(id.slot());
    let Some(p) = sim.state.players.get_mut(slot) else {
        return true;
    };
    if p.resource(res, max) >= amount {
        p.add_resource(res, amount.wrapping_neg());
        return true;
    }
    let Some(b) = building_mut(sim, id) else {
        return false;
    };
    if !b.msg_sent {
        b.msg_sent = true;
        let tile = sim.object(id).map_or((0, 0), |o| o.tile);
        announce(
            sim,
            i32::from(id.slot()),
            message::MISSING[res],
            tile,
            message::flags::TRADE,
        );
    }
    false
}

/// `PropertyBuilding_ExecuteTrade` (`0x00505310`, only with a player): credit the goods or 100 gold,
/// book the trade statistic (kind 11) with the owner and then adjust the price with the state **after** the
/// payment (buy `+100`, sell `−goods`, each `/ 25`). Booked is, when buying, the gold value
/// `Market_Convert…Gold(100)` with the price **before** the adjustment (`0x00505371`, `0x005053AE`), when
/// selling 100 gold (`0x005053E0`, `0x0050541E`; O-171).
fn execute_trade(sim: &mut Sim, id: ObjectId, guid: u32) {
    let state = &mut *sim.state;
    let Some(p) = state.players.get_mut(usize::from(id.slot())) else {
        return;
    };
    let e = &mut state.economy;
    let parm = match guid {
        trade::BUY_WOOD => {
            p.add_resource(resource::WOOD, trade::AMOUNT);
            let gold = market_convert(e.wood_price, trade::AMOUNT);
            market_adjust(&mut e.wood_price, trade::AMOUNT);
            gold
        }
        trade::BUY_FOOD => {
            p.add_resource(resource::FOOD, trade::AMOUNT);
            let gold = market_convert(e.food_price, trade::AMOUNT);
            market_adjust(&mut e.food_price, trade::AMOUNT);
            gold
        }
        trade::SELL_WOOD => {
            p.add_resource(resource::GOLD, trade::AMOUNT);
            let n = market_convert(e.wood_price, -trade::AMOUNT);
            market_adjust(&mut e.wood_price, n.wrapping_neg());
            trade::AMOUNT
        }
        trade::SELL_FOOD => {
            p.add_resource(resource::GOLD, trade::AMOUNT);
            let n = market_convert(e.food_price, -trade::AMOUNT);
            market_adjust(&mut e.food_price, n.wrapping_neg());
            trade::AMOUNT
        }
        _ => return,
    };
    // The original books before the price adjustment; the value is computed above with the old price, and
    // the adjustment does not touch the log - the later booking gives the same.
    add_statistic(sim, i32::from(id.slot()), stat::TRADE, parm);
}

/// Progress `1 − rest/total` as `f32` like the original (`FILD`/`FIDIV`/`FSUBR 1.0f`/`FSTP`
/// at 24-bit precision of the x87: the quotient of the exact integers rounded once to `f32`,
/// the difference likewise). Up to `2^24` ms both integers are exact in `f32`; above that the
/// quotient is computed in `f64` (double rounding possible in the last bit, run times above 4.6 hours).
pub fn progress_fraction(rest: i32, total: i32) -> f32 {
    const EXACT: i32 = 1 << 24;
    let q = if (-EXACT..=EXACT).contains(&rest) && (-EXACT..=EXACT).contains(&total) {
        rest as f32 / total as f32
    } else {
        (f64::from(rest) / f64::from(total)) as f32
    };
    1.0 - q
}

/// `0xF03` (`0x00504AB0`): without a running order 0 (nothing written); otherwise `args[0]` = GUID,
/// `args[1]` = progress as bits of the `f32` ([`progress_fraction`], 0 before the start), return value 1.
pub fn on_progress(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let Some(b) = building(sim, id) else {
        return 0;
    };
    if b.make_guid == 0 {
        return 0;
    }
    let bits = if b.make_start == 0 {
        0
    } else {
        progress_fraction(b.make_time, b.make_start).to_bits() as i32
    };
    m.set_arg(0, b.make_guid as i32);
    m.set_arg(1, bits);
    1
}

/// `0x1003` (`0x005062C0`): `argc` = number of waiting orders (quirk: always, even if
/// the buffer was too small - then the arguments stay unwritten and the return value is 0);
/// otherwise the GUIDs in order to `args[0..]`, return value 1.
pub fn on_list(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let Some(b) = building(sim, id) else {
        return 0;
    };
    let n = b.make_queue.len();
    let fits = m.args.len() >= n;
    m.args.resize(n, 0);
    if !fits {
        return 0;
    }
    for (a, &g) in m.args.iter_mut().zip(&b.make_queue) {
        *a = g as i32;
    }
    1
}

/// `0x1103 {n}` (`0x00507960`, command `0x0F`): `n ≥ 1` takes the `n`-th **waiting** order
/// out ("in progress" -1; outside the queue or `n < 0`: 0). `n = 0` cancels the running one:
/// except for trade "in progress" -1 (quirk: also without a running order) and, if already
/// paid, refund of the **remaining share** `rest · 100 / total` % (whole percent, truncated per
/// resource) as well as population and living space back; the next waiting one is taken over,
/// but does not start until the next building tick. Return value 1.
///
/// Every success path reports [`SimEvent::ProductionCancelled`] after lowering "in progress"
/// (`GameObject_NotifyUiIfSelected(0)`: `0x00507A9F` waiting, `0x00507A29` running without,
/// `0x00507A56` with successor), also `n = 0` without a running order; the failure
/// (`0x00507A79`) reports nothing.
pub fn cancel(sim: &mut Sim, id: ObjectId, n: i32) -> i32 {
    let Some(b) = building_mut(sim, id) else {
        return 0;
    };
    if n != 0 {
        let Some(i) = usize::try_from(n)
            .ok()
            .map(|n| n - 1)
            .filter(|&i| i < b.make_queue.len())
        else {
            return 0;
        };
        let guid = b.make_queue.remove(i).unwrap_or(0);
        b.msg_sent = false;
        add_owner_pending_guid_count(sim.state, id, guid, -1);
        sim.state
            .events
            .push(SimEvent::ProductionCancelled { building: id });
        return 1;
    }
    let (guid, rest, total) = (b.make_guid, b.make_time, b.make_start);
    if !is_trade_guid(guid) {
        add_owner_pending_guid_count(sim.state, id, guid, -1);
        if total != 0 {
            refund_running(sim, id, guid, rest, total);
        }
    }
    take_next(sim, id);
    sim.state
        .events
        .push(SimEvent::ProductionCancelled { building: id });
    1
}

/// Clear the running order and take over the next waiting one (end of `0x1103 {0}`):
/// it does not start until the next building tick; without a waiting one the production ends (`MAKINGGUID`
/// off).
fn take_next(sim: &mut Sim, id: ObjectId) {
    let Some(b) = building_mut(sim, id) else {
        return;
    };
    b.make_guid = 0;
    b.make_time = 0;
    b.make_start = 0;
    match b.make_queue.pop_front() {
        None => clear_flags(sim, id, flags::MAKINGGUID),
        Some(next) => {
            b.msg_sent = false;
            start_make_entry(sim, id, next);
        }
    }
}

/// Debug "complete immediately" (command `DebugCompleteOrder`, no counterpart in the original):
/// complete order `n` (0 = running, `n ≥ 1` = the `n`-th waiting one like `0x1103`) immediately via the normal
/// flow of `PropertyBuilding_TickProduction`. Like enqueueing and cancelling, the
/// message "Funds missing" may appear again (`m_IsMsgSent` back), then:
/// 1. Not yet paid (waiting or running before the start) and not trade: start check and
///    payment ([`pay_start`], with message); if it fails, the order stays unchanged.
/// 2. Completion ([`finish_entry`]): unit at the exit and to the rally point, research reports,
///    trade does not pay until here (`PayTrade`, without goods message and rejection) and executes; the
///    counters "in progress" as at the normal end.
/// 3. If the completion fails (trade without goods, spawn impossible), the order stays; a
///    start payment just made is refunded entirely (nothing booked).
/// 4. Otherwise the order leaves the queue; the running one hands over to the next waiting one like
///    `0x1103 {0}` (start in the next building tick).
///
/// Error (log): no such order or the reason of the failure.
pub fn complete_now(sim: &mut Sim, id: ObjectId, n: i32) -> Result<(), String> {
    let Some(b) = building_mut(sim, id) else {
        return Err(format!("{} has no building module", id.0));
    };
    let (guid, paid) = match n {
        0 => (b.make_guid, b.make_start != 0),
        n => {
            let waiting = usize::try_from(n)
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| b.make_queue.get(i).copied());
            (waiting.unwrap_or(0), false)
        }
    };
    if guid == 0 {
        return Err(format!("no order with index {n} in {}", id.0));
    }
    b.msg_sent = false;
    let pays_now = !paid && !is_trade_guid(guid);
    if pays_now && !pay_start(sim, id, guid, false) {
        return Err(format!("funds for {guid} missing"));
    }
    if !finish_entry(sim, id, guid) {
        if pays_now {
            refund_running(sim, id, guid, 1, 1);
        }
        return Err(if is_trade_guid(guid) {
            format!("goods for trade {guid} missing")
        } else {
            format!("spawn of {guid} failed")
        });
    }
    if n == 0 {
        take_next(sim, id);
    } else if let Some(b) = building_mut(sim, id) {
        let i = usize::try_from(n - 1).unwrap_or(0);
        b.make_queue.remove(i);
    }
    Ok(())
}

/// Refund the remaining share of a paid order: cost at `rest · 100 / total` % (overflow and
/// division like `IMUL`/`IDIV`; the exception of the original at `MIN / −1` does not exist here),
/// population and living space back entirely.
fn refund_running(sim: &mut Sim, id: ObjectId, guid: u32, rest: i32, total: i32) {
    let pct = rest.wrapping_mul(100).wrapping_div(total);
    add_owner_object_costs_percent(sim, id, guid, pct);
    book_population(sim, id, guid, -1);
}

/// `PropertyBuilding_ReleasePendingCounts` (`0x00508240`, in the destructor): the running order
/// "in progress" -1 (here also for trade) and, if paid, refund of the remaining share like on
/// cancel; every waiting one "in progress" -1, the queue becomes empty.
pub fn release_pending_counts(sim: &mut Sim, id: ObjectId) {
    let Some(b) = building_mut(sim, id) else {
        return;
    };
    let (guid, rest, total) = (b.make_guid, b.make_time, b.make_start);
    let waiting: Vec<u32> = b.make_queue.drain(..).collect();
    if guid != 0 {
        add_owner_pending_guid_count(sim.state, id, guid, -1);
        if total != 0 {
            refund_running(sim, id, guid, rest, total);
        }
    }
    for g in waiting {
        add_owner_pending_guid_count(sim.state, id, g, -1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economy::resource::{FOOD, GOLD, LIVING_SPACE, POPULATION, WOOD};
    use crate::messaging::{send, send_mut};
    use crate::module::building::{INDEX, has_flags, msg};
    use crate::rules::Rules;
    use crate::state::{SimEvent, SimState};
    use crate::world::{ObjectDesc, create_object, desc_flags, destroy_object};
    use opennml_formats::gifcon::ToolbarButton;

    /// Colonist 1100 (stand only), governor 1112, SPA headquarters 1000.
    const TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1112\nNAME SPA_Gouverneur\nVALUEID 100 6 1 0\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n";

    /// `default.dat` values: colonist 60 food, 15 s, population 1; governor 200 food +
    /// 150 gold, 20 s (`MAKEONLY_ONCE_MSG`).
    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 32);
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        r.defaults.insert(1100, set(&[(10, 60), (13, 15), (22, 1)]));
        r.defaults
            .insert(1112, set(&[(10, 200), (12, 150), (13, 20), (22, 1)]));
        r.toolbar.entries.insert(
            1112,
            ToolbarButton {
                msg: 1112,
                flags: button_kind::MAKEONLY_ONCE,
                ..ToolbarButton::default()
            },
        );
        r
    }

    fn place_hq(sim: &mut Sim, x: f32) -> ObjectId {
        let mut desc = ObjectDesc::new(1000, 0, [x, 0.0, 2000.0]);
        desc.flags |= desc_flags::PLACED;
        create_object(sim, &mut desc).unwrap()
    }

    /// Account of slot 0 in whole units: gold, wood, food, population, living space.
    fn account(sim: &mut Sim, v: [i32; 5]) {
        sim.state.players[0].resources_fx = v.map(|x| x << 12);
    }

    fn res(sim: &Sim, i: usize) -> i32 {
        sim.state.resource(0, i)
    }

    fn enqueue_msg(sim: &mut Sim, b: ObjectId, guid: u32) -> i32 {
        send(sim, b, &Message::new(msg::ENQUEUE, &[guid as i32]))
    }

    fn cancel_msg(sim: &mut Sim, b: ObjectId, n: i32) -> i32 {
        send(sim, b, &Message::new(msg::CANCEL, &[n]))
    }

    /// One building tick (`PropertyBuilding_Tick(100)`).
    fn tick(sim: &mut Sim, b: ObjectId) {
        crate::module::tick(sim, b, INDEX, 100);
    }

    fn bld<'s>(sim: &'s Sim, id: ObjectId) -> &'s crate::module::building::Building {
        building(sim, id).unwrap()
    }

    fn messages(sim: &Sim, id: i32) -> usize {
        sim.state
            .events
            .iter()
            .filter(|e| matches!(e, SimEvent::PlayerMessage { id: m, .. } if *m == id))
            .count()
    }

    /// Flag words of the message `id`.
    fn message_flags(sim: &Sim, id: i32) -> Vec<u32> {
        sim.state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::PlayerMessage { id: m, flags, .. } if *m == id => Some(*flags),
                _ => None,
            })
            .collect()
    }

    // QUIRK W-91: kept
    #[test]
    fn colonists_are_paid_at_the_start_and_wait_for_money() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [0, 0, 100, 0, 5]);
        assert_eq!(enqueue_msg(&mut sim, hq, 1100), 1);
        assert_eq!(enqueue_msg(&mut sim, hq, 1100), 1);
        // The first is immediately the running one (unpaid), the second waits; "in progress" 2.
        let b = bld(&sim, hq);
        assert_eq!((b.make_guid, b.make_queue.len()), (1100, 1));
        assert_eq!((b.make_time, b.make_start), (0, 0));
        assert!(has_flags(&sim, hq, flags::MAKINGGUID));
        assert_eq!(pending_guid_count(&sim, 0, 1100), 2);
        // Tick 1: start, -60 food, population +1, 15 s.
        tick(&mut sim, hq);
        assert_eq!((res(&sim, FOOD), res(&sim, POPULATION)), (40, 1));
        let b = bld(&sim, hq);
        assert_eq!((b.make_time, b.make_start), (15_000, 15_000));
        for _ in 2..=150 {
            tick(&mut sim, hq);
        }
        assert_eq!(bld(&sim, hq).make_time, 100);
        assert_eq!(guid_count(&sim, 0, 1100), 0);
        // Tick 151: finished - object counter +1, "in progress" -1; the second cannot pay (40 < 60).
        tick(&mut sim, hq);
        assert_eq!(guid_count(&sim, 0, 1100), 1);
        assert_eq!(pending_guid_count(&sim, 0, 1100), 1);
        let b = bld(&sim, hq);
        assert_eq!((b.make_guid, b.make_time, b.make_start), (1100, 0, 0));
        assert!(b.msg_sent && b.make_queue.is_empty());
        assert_eq!(messages(&sim, message::NO_FOOD), 1);
        // Keep waiting, silent.
        tick(&mut sim, hq);
        tick(&mut sim, hq);
        assert_eq!(messages(&sim, message::NO_FOOD), 1);
        assert_eq!((res(&sim, FOOD), bld(&sim, hq).make_time), (40, 0));
        // With 20 more food it starts in the next tick.
        sim.state.players[0].add_resource(FOOD, 20);
        tick(&mut sim, hq);
        assert_eq!((res(&sim, FOOD), res(&sim, POPULATION)), (0, 2));
        assert_eq!(bld(&sim, hq).make_time, 15_000);
        assert!(!bld(&sim, hq).msg_sent);
    }

    // QUIRK W-92: kept
    #[test]
    fn cancelling_the_running_entry_refunds_the_rest_share() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [0, 0, 100, 0, 5]);
        enqueue_msg(&mut sim, hq, 1100);
        for _ in 0..51 {
            tick(&mut sim, hq);
        }
        assert_eq!(bld(&sim, hq).make_time, 10_000);
        // 0xF03: GUID and `1 − 10000/15000` as f32.
        let mut m = Message::new(msg::PROGRESS, &[]);
        assert_eq!(send_mut(&mut sim, hq, &mut m), 1);
        assert_eq!(m.args, [1100, 0x3EAA_AAAA]);
        // 10000 * 100 / 15000 = 66 % -> 60 * 66 / 100 = 39 food back, population -1.
        assert_eq!(cancel_msg(&mut sim, hq, 0), 1);
        assert_eq!((res(&sim, FOOD), res(&sim, POPULATION)), (79, 0));
        assert_eq!(pending_guid_count(&sim, 0, 1100), 0);
        let b = bld(&sim, hq);
        assert_eq!((b.make_guid, b.make_time, b.make_start), (0, 0, 0));
        assert!(!has_flags(&sim, hq, flags::MAKINGGUID));
        // Without a running order: 0xF03 returns 0 and writes nothing.
        let mut m = Message::new(msg::PROGRESS, &[7]);
        assert_eq!(send_mut(&mut sim, hq, &mut m), 0);
        assert_eq!(m.args, [7]);
        // Quirk: cancel without a running order counts "in progress" of GUID 0 down.
        assert_eq!(cancel_msg(&mut sim, hq, 0), 1);
        assert_eq!(sim.state.pending_counts[0][&0], -1);
    }

    #[test]
    fn waiting_entries_are_listed_and_cancelled_by_position() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [1000, 0, 1000, 0, 50]);
        for g in [1100, 1100, 1112, 1100] {
            assert_eq!(enqueue_msg(&mut sim, hq, g), 1);
        }
        // Before the start: progress 0.
        let mut m = Message::new(msg::PROGRESS, &[]);
        assert_eq!(send_mut(&mut sim, hq, &mut m), 1);
        assert_eq!(m.args, [1100, 0]);
        // 0x1003: buffer too small -> only `argc`; otherwise the waiting GUIDs.
        let mut m = Message::new(msg::LIST, &[7]);
        assert_eq!(send_mut(&mut sim, hq, &mut m), 0);
        assert_eq!(m.args, [7, 0, 0]);
        let mut m = Message::new(msg::LIST, &[0; 5]);
        assert_eq!(send_mut(&mut sim, hq, &mut m), 1);
        assert_eq!(m.args, [1100, 1112, 1100]);
        // Take out the second waiting one; outside and negative: 0.
        assert_eq!(cancel_msg(&mut sim, hq, 2), 1);
        assert_eq!(cancel_msg(&mut sim, hq, 3), 0);
        assert_eq!(cancel_msg(&mut sim, hq, -1), 0);
        assert_eq!(Vec::from(bld(&sim, hq).make_queue.clone()), [1100, 1100]);
        assert_eq!(pending_guid_count(&sim, 0, 1112), 0);
        assert_eq!(pending_guid_count(&sim, 0, 1100), 3);
        // Cancel of the (unpaid) running one: the next moves up, starts in the next tick.
        assert_eq!(cancel_msg(&mut sim, hq, 0), 1);
        assert_eq!(res(&sim, FOOD), 1000);
        let b = bld(&sim, hq);
        assert_eq!((b.make_guid, b.make_time, b.make_queue.len()), (1100, 0, 1));
        tick(&mut sim, hq);
        assert_eq!((res(&sim, FOOD), bld(&sim, hq).make_time), (940, 15_000));
    }

    /// Cancel messages to the UI (`ProductionCancelled`) since the last call.
    fn take_cancelled(sim: &mut Sim) -> Vec<ObjectId> {
        std::mem::take(&mut sim.state.events)
            .into_iter()
            .filter_map(|e| match e {
                SimEvent::ProductionCancelled { building } => Some(building),
                _ => None,
            })
            .collect()
    }

    // QUIRK O-395: kept
    #[test]
    fn cancel_reports_the_ui_rebuild_on_every_success_path() {
        // `0x00507960`: Notify(0) after "in progress" -1 on all three success paths (waiting `0x00507A9F`, running
        // without successor `0x00507A29`, with successor `0x00507A56`), also `n = 0` without an order; the failure
        // `0x00507A79` reports nothing.
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [1000, 0, 1000, 0, 50]);
        for g in [1100, 1112, 1100] {
            assert_eq!(enqueue_msg(&mut sim, hq, g), 1);
        }
        take_cancelled(&mut sim);
        // Failure: index outside, negative.
        assert_eq!(cancel_msg(&mut sim, hq, 3), 0);
        assert_eq!(cancel_msg(&mut sim, hq, -1), 0);
        assert!(
            take_cancelled(&mut sim).is_empty(),
            "failure reports nothing"
        );
        // Waiting entry: the event comes, "in progress" is already lowered.
        assert_eq!(cancel_msg(&mut sim, hq, 1), 1);
        assert_eq!(pending_guid_count(&sim, 0, 1112), 0);
        assert_eq!(take_cancelled(&mut sim), [hq]);
        // Running one with successor.
        tick(&mut sim, hq);
        assert_eq!(cancel_msg(&mut sim, hq, 0), 1);
        assert_eq!(bld(&sim, hq).make_guid, 1100);
        assert_eq!(take_cancelled(&mut sim), [hq]);
        // Running one without successor.
        assert_eq!(cancel_msg(&mut sim, hq, 0), 1);
        assert_eq!(pending_guid_count(&sim, 0, 1100), 0);
        assert!(!has_flags(&sim, hq, flags::MAKINGGUID));
        assert_eq!(take_cancelled(&mut sim), [hq]);
        // `n = 0` without a running order: nevertheless return value 1 and message.
        assert_eq!(cancel_msg(&mut sim, hq, 0), 1);
        assert_eq!(take_cancelled(&mut sim), [hq]);
    }

    // QUIRK W-95: kept
    #[test]
    fn the_queue_holds_26_waiting_plus_one_and_once_kinds_only_once() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        for _ in 0..27 {
            assert_eq!(enqueue_msg(&mut sim, hq, 1100), 1);
        }
        assert_eq!(bld(&sim, hq).make_queue.len(), MAX_QUEUE);
        assert_eq!(enqueue_msg(&mut sim, hq, 1100), 0);
        assert_eq!(pending_guid_count(&sim, 0, 1100), 27);
        // `MAKEONLY_ONCE`: a second order is rejected as long as one is "in progress" -
        // also in another building.
        let other = place_hq(&mut sim, 3000.0);
        assert_eq!(enqueue_msg(&mut sim, other, 1112), 1);
        assert_eq!(enqueue_msg(&mut sim, other, 1112), 0);
        assert_eq!(cancel_msg(&mut sim, hq, 26), 1);
        assert_eq!(enqueue_msg(&mut sim, hq, 1112), 0);
        // Only the counter "in progress" counts, not the object counter.
        cancel_msg(&mut sim, other, 0);
        sim.state.add_guid_count(0, 1112, 1);
        assert_eq!(enqueue_msg(&mut sim, other, 1112), 1);
    }

    // QUIRK W-19: kept
    #[test]
    fn paddocks_turn_into_horse_colours_with_one_shared_counter() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        for g in [3423, 3423, 3424, 3424, 3423, 4711] {
            assert_eq!(enqueue_msg(&mut sim, hq, g), 1);
        }
        let b = bld(&sim, hq);
        assert_eq!(b.make_guid, 3406);
        assert_eq!(
            Vec::from(b.make_queue.clone()),
            [3409, 3419, 3417, 3409, 4711]
        );
        assert_eq!(b.last_horse, 2);
        assert_eq!(pending_guid_count(&sim, 0, 3409), 2);
        assert_eq!(pending_guid_count(&sim, 0, 3423), 0);
        // A foreign counter value (only from a savegame) leaves the paddock GUID as it is.
        building_mut(&mut sim, hq).unwrap().last_horse = 7;
        enqueue_msg(&mut sim, hq, 3424);
        assert_eq!(bld(&sim, hq).make_queue.back(), Some(&3424));
    }

    // QUIRK W-19: kept
    #[test]
    fn slow_production_halves_the_pace_and_ships_use_the_shipyard_upgrade() {
        let mut rules = rules();
        rules
            .defaults
            .insert(2009, [(13, 48)].into_iter().collect());
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [0, 0, 100, 0, 5]);
        sim.state.add_guid_count(0, SLOW_PRODUCTION, 1);
        enqueue_msg(&mut sim, hq, 1100);
        for _ in 0..300 {
            tick(&mut sim, hq);
        }
        assert_eq!(
            (guid_count(&sim, 0, 1100), bld(&sim, hq).make_time),
            (0, 50)
        );
        tick(&mut sim, hq);
        assert_eq!(guid_count(&sim, 0, 1100), 1);
        // Shipyard upgrade: 48 s * 7 / 10 = 33 s.
        sim.state.add_guid_count(0, SHIPYARD_UPGRADE, 1);
        enqueue_msg(&mut sim, hq, 2009);
        tick(&mut sim, hq);
        assert_eq!(bld(&sim, hq).make_start, 33_000);
    }

    /// Ticks of a trade after its start (end 80 ticks later).
    fn run_trade(sim: &mut Sim, b: ObjectId) {
        for _ in 0..80 {
            tick(sim, b);
        }
    }

    // QUIRK W-97: kept
    #[test]
    fn trades_pay_at_the_end_and_move_the_prices() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [1000, 1000, 1000, 0, 5]);
        for g in [490, 490, 491, 492, 493] {
            enqueue_msg(&mut sim, hq, g);
        }
        tick(&mut sim, hq);
        assert_eq!((bld(&sim, hq).make_start, res(&sim, GOLD)), (8000, 1000));
        for _ in 0..79 {
            tick(&mut sim, hq);
        }
        assert_eq!(res(&sim, GOLD), 1000);
        // End of the first: -25 gold, +100 wood, wood price 1028; the next starts immediately.
        tick(&mut sim, hq);
        assert_eq!((res(&sim, GOLD), res(&sim, WOOD)), (975, 1100));
        assert_eq!(sim.state.economy.wood_price, 1028);
        assert_eq!(bld(&sim, hq).make_time, 8000);
        run_trade(&mut sim, hq);
        assert_eq!((res(&sim, GOLD), sim.state.economy.wood_price), (950, 1032));
        run_trade(&mut sim, hq);
        assert_eq!((res(&sim, GOLD), res(&sim, FOOD)), (918, 1100));
        assert_eq!(sim.state.economy.food_price, 1355);
        run_trade(&mut sim, hq);
        assert_eq!((res(&sim, WOOD), res(&sim, GOLD)), (1200 - 396, 1018));
        assert_eq!(sim.state.economy.wood_price, 1017);
        run_trade(&mut sim, hq);
        assert_eq!((res(&sim, FOOD), res(&sim, GOLD)), (1100 - 302, 1118));
        assert_eq!(sim.state.economy.food_price, 1343);
        // Quirk: trade never counts "in progress" back.
        assert_eq!(pending_guid_count(&sim, 0, 490), 2);
        assert!(!has_flags(&sim, hq, flags::MAKINGGUID));
        assert_eq!(res(&sim, LIVING_SPACE), 5);
    }

    // QUIRK W-97: kept
    #[test]
    fn trades_without_money_retry_every_100_ms_and_announce_once() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [0, 0, 0, 0, 5]);
        enqueue_msg(&mut sim, hq, 490);
        tick(&mut sim, hq);
        run_trade(&mut sim, hq);
        let b = bld(&sim, hq);
        assert_eq!((b.make_time, b.make_guid, b.msg_sent), (100, 490, true));
        tick(&mut sim, hq);
        assert_eq!(bld(&sim, hq).make_time, 100);
        assert_eq!(messages(&sim, message::NO_GOLD), 1);
        // `PropertyBuilding_PayTrade` `0x00505170`: flag `0x01010000` (priority 1).
        assert_eq!(message_flags(&sim, message::NO_GOLD), [0x0101_0000]);
        assert_eq!(res(&sim, WOOD), 0);
        // With 25 gold the next attempt succeeds.
        sim.state.players[0].add_resource(GOLD, 25);
        tick(&mut sim, hq);
        assert_eq!((res(&sim, GOLD), res(&sim, WOOD)), (0, 100));
        assert_eq!(bld(&sim, hq).make_guid, 0);
    }

    // QUIRK W-96: kept
    #[test]
    fn destroying_the_building_releases_the_counts_and_refunds_the_running_entry() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [0, 0, 100, 0, 5]);
        enqueue_msg(&mut sim, hq, 1100);
        enqueue_msg(&mut sim, hq, 1100);
        for _ in 0..51 {
            tick(&mut sim, hq);
        }
        destroy_object(&mut sim, hq);
        assert_eq!((res(&sim, FOOD), res(&sim, POPULATION)), (79, 0));
        assert_eq!(pending_guid_count(&sim, 0, 1100), 0);
    }

    #[test]
    fn research_done_carries_priority_2() {
        // `PropertyBuilding_FinishResearch` `0x00504EE0`: flag `0x02010000` (priority 2, enqueue).
        let mut rules = rules();
        rules.defaults.insert(3000, [(13, 1)].into_iter().collect());
        rules.toolbar.entries.insert(
            3000,
            ToolbarButton {
                msg: 3000,
                flags: button_kind::UPGRADE,
                ..ToolbarButton::default()
            },
        );
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        enqueue_msg(&mut sim, hq, 3000);
        for _ in 0..11 {
            tick(&mut sim, hq);
        }
        assert_eq!(message_flags(&sim, message::RESEARCH_DONE), [0x0201_0000]);
    }

    #[test]
    fn research_finishes_with_its_effect_and_rails_without() {
        let mut rules = rules();
        rules.defaults.insert(3000, [(13, 1)].into_iter().collect());
        rules
            .defaults
            .insert(RAILS, [(13, 1)].into_iter().collect());
        rules.toolbar.entries.insert(
            3000,
            ToolbarButton {
                msg: 3000,
                flags: button_kind::UPGRADE,
                ..ToolbarButton::default()
            },
        );
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        enqueue_msg(&mut sim, hq, 3000);
        enqueue_msg(&mut sim, hq, RAILS);
        for _ in 0..11 {
            tick(&mut sim, hq);
        }
        // Research finished: message 7, counter +1 and entry in the list (`Player_ApplyUpgrade`);
        // "in progress" stays and blocks (quirk).
        assert_eq!(messages(&sim, message::RESEARCH_DONE), 1);
        assert_eq!(guid_count(&sim, 0, 3000), 1);
        assert_eq!(sim.state.applied_upgrades[0], [3000]);
        assert_eq!(pending_guid_count(&sim, 0, 3000), 1);
        assert_eq!(enqueue_msg(&mut sim, hq, 3000), 0);
        assert_eq!(bld(&sim, hq).make_guid, RAILS);
        // Rails without an own track (W-206, `track.rs`): "in progress" -1, no object counter, nothing is created.
        for _ in 0..10 {
            tick(&mut sim, hq);
        }
        assert_eq!(pending_guid_count(&sim, 0, RAILS), 0);
        assert_eq!(guid_count(&sim, 0, RAILS), 0);
        assert_eq!(sim.state.applied_upgrades[0], [3000]);
        assert_eq!(bld(&sim, hq).make_guid, 0);
    }

    #[test]
    fn completing_now_finishes_research_and_once_kinds_like_the_normal_end() {
        let mut rules = rules();
        rules.defaults.insert(3000, [(13, 1)].into_iter().collect());
        rules.toolbar.entries.insert(
            3000,
            ToolbarButton {
                msg: 3000,
                flags: button_kind::UPGRADE,
                ..ToolbarButton::default()
            },
        );
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [1000, 0, 1000, 0, 5]);
        for g in [1100, 3000, 1112] {
            enqueue_msg(&mut sim, hq, g);
        }
        // Research (waiting 1): message 7, effect (counter, list), "in progress" stays +1 and
        // blocks (quirk).
        assert_eq!(complete_now(&mut sim, hq, 1), Ok(()));
        assert_eq!(messages(&sim, message::RESEARCH_DONE), 1);
        assert_eq!(guid_count(&sim, 0, 3000), 1);
        assert_eq!(sim.state.applied_upgrades[0], [3000]);
        assert_eq!(pending_guid_count(&sim, 0, 3000), 1);
        assert_eq!(enqueue_msg(&mut sim, hq, 3000), 0);
        // Governor (`MAKEONLY_ONCE`, waiting 1): 200 food + 150 gold at the start, object counter +1.
        assert_eq!(complete_now(&mut sim, hq, 1), Ok(()));
        assert_eq!((res(&sim, FOOD), res(&sim, GOLD)), (800, 850));
        assert_eq!(guid_count(&sim, 0, 1112), 1);
        assert_eq!(pending_guid_count(&sim, 0, 1112), 0);
        assert_eq!(sim.state.applied_upgrades[0], [3000]);
        // Without a waiting order: index 1 rejected; the running one (unpaid) is completed.
        assert!(complete_now(&mut sim, hq, 1).is_err());
        assert_eq!(complete_now(&mut sim, hq, 0), Ok(()));
        assert_eq!((res(&sim, FOOD), res(&sim, POPULATION)), (740, 2));
        assert_eq!(bld(&sim, hq).make_guid, 0);
        assert!(!has_flags(&sim, hq, flags::MAKINGGUID));
        assert!(complete_now(&mut sim, hq, 0).is_err());
    }

    #[test]
    fn completing_now_refunds_the_start_payment_when_the_output_fails() {
        // 1101: cost and population like the colonist, but without a type - `create_object` fails.
        let mut rules = rules();
        rules
            .defaults
            .insert(1101, [(10, 60), (13, 15), (22, 1)].into_iter().collect());
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place_hq(&mut sim, 1000.0);
        account(&mut sim, [0, 0, 100, 0, 5]);
        enqueue_msg(&mut sim, hq, 1100);
        building_mut(&mut sim, hq)
            .unwrap()
            .make_queue
            .push_back(1101);
        add_owner_pending_guid_count(sim.state, hq, 1101, 1);
        let before = sim.state.players[0].resources_fx;
        let units = sim.state.object_count();
        // Start payment succeeds (60 <= 100), the spawn fails: everything back, the order stays.
        assert_eq!(
            complete_now(&mut sim, hq, 1),
            Err("spawn of 1101 failed".into())
        );
        assert_eq!(sim.state.players[0].resources_fx, before);
        assert_eq!((res(&sim, FOOD), res(&sim, POPULATION)), (100, 0));
        assert_eq!(res(&sim, LIVING_SPACE), 5);
        let b = bld(&sim, hq);
        assert_eq!((b.make_guid, b.make_time, b.make_start), (1100, 0, 0));
        assert_eq!(b.make_queue, [1101]);
        assert!(!b.msg_sent);
        assert_eq!(pending_guid_count(&sim, 0, 1101), 1);
        assert_eq!(pending_guid_count(&sim, 0, 1100), 1);
        assert_eq!(guid_count(&sim, 0, 1101), 0);
        assert_eq!(sim.state.object_count(), units);
        assert_eq!(messages(&sim, message::NO_FOOD), 0);
    }

    /// Upgrade test world: two players with 1000 gold, wood and food each, a
    /// finished headquarters and a colonist of slot 0.
    fn upgrade_world(rules: &Rules) -> (SimState, ObjectId, ObjectId) {
        use crate::upgrades::testkit as uk;
        let mut state = uk::state(rules);
        let mut sim = Sim::new(&mut state, rules);
        let hq = uk::spawn(&mut sim, uk::HQ, 0, (20, 20));
        let colonist = uk::spawn(&mut sim, uk::COLONIST, 0, (24, 24));
        (state, hq, colonist)
    }

    // QUIRK W-211: kept
    #[test]
    fn research_applies_the_upgrade_when_it_ends() {
        use crate::module::gather::unit_delay;
        let rules = crate::upgrades::testkit::rules();
        let (mut state, hq, colonist) = upgrade_world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(enqueue_msg(&mut sim, hq, 2800), 1);
        assert_eq!(pending_guid_count(&sim, 0, 2800), 1);
        // Tick 1: start, -50 food, -100 wood (cost of the research); 15 s.
        tick(&mut sim, hq);
        assert_eq!((res(&sim, FOOD), res(&sim, WOOD)), (950, 900));
        assert_eq!(bld(&sim, hq).make_time, 15_000);
        // Nothing takes effect before the end.
        for _ in 2..=150 {
            tick(&mut sim, hq);
        }
        assert_eq!(bld(&sim, hq).make_time, 100);
        assert_eq!(guid_count(&sim, 0, 2800), 0);
        assert!(sim.state.applied_upgrades[0].is_empty());
        assert_eq!(sim.type_upgrade_percent(0, 1100, 32), 0);
        assert_eq!(sim.attribute(colonist, 32), 60);
        assert_eq!(unit_delay(sim.attribute(colonist, 32), 0), (1000, 0));
        // Tick 151: finished - message 7, object counter 1, "in progress" stays 1, list and percentages.
        tick(&mut sim, hq);
        assert_eq!(messages(&sim, message::RESEARCH_DONE), 1);
        assert_eq!(guid_count(&sim, 0, 2800), 1);
        assert_eq!(pending_guid_count(&sim, 0, 2800), 1);
        assert_eq!(sim.state.applied_upgrades[0], [2800]);
        assert_eq!(sim.type_upgrade_percent(0, 1100, 32), 25);
        assert_eq!(sim.type_upgrade_percent(1, 1100, 32), 0);
        // Wood rate 75 (re/gathering): gathering step 800 ms without remainder.
        assert_eq!(sim.attribute(colonist, 32), 75);
        assert_eq!(unit_delay(sim.attribute(colonist, 32), 0), (800, 0));
        assert_eq!(bld(&sim, hq).make_guid, 0);
    }

    // QUIRK W-211: kept
    #[test]
    fn a_follow_up_research_unlocks_only_after_the_end() {
        use crate::command::validate::check_production;
        use crate::module::gather::unit_delay;
        let rules = crate::upgrades::testkit::rules();
        let (mut state, hq, colonist) = upgrade_world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // 2801 requires 2800 (`REQUIRED_GUID`): only after the end of research 2800 (W-211).
        let locked = Err("prerequisite for 2801 missing".to_string());
        assert_eq!(check_production(&sim, 0, hq, 2801), locked);
        enqueue_msg(&mut sim, hq, 2800);
        for _ in 0..150 {
            tick(&mut sim, hq);
        }
        assert_eq!(check_production(&sim, 0, hq, 2801), locked);
        tick(&mut sim, hq);
        assert_eq!(check_production(&sim, 0, hq, 2801), Ok(()));
        // Research 2801: -100 food, -200 wood, 30 s (301 ticks with the start).
        assert_eq!(enqueue_msg(&mut sim, hq, 2801), 1);
        for _ in 0..301 {
            tick(&mut sim, hq);
        }
        assert_eq!(sim.state.applied_upgrades[0], [2800, 2801]);
        assert_eq!(guid_count(&sim, 0, 2801), 1);
        assert_eq!((res(&sim, FOOD), res(&sim, WOOD)), (850, 700));
        // Percentages 25 + 30 = 55, wood rate 93: gathering steps with leak (re/gathering 11.3).
        assert_eq!(sim.type_upgrade_percent(0, 1100, 32), 55);
        let rate = sim.attribute(colonist, 32);
        assert_eq!(rate, 93);
        let mut leak = 0;
        let mut steps = Vec::new();
        for _ in 0..5 {
            let (d, l) = unit_delay(rate, leak);
            steps.push(d);
            leak = l;
        }
        assert_eq!(steps, [600, 600, 700, 600, 700]);
    }

    // QUIRK W-211: kept
    #[test]
    fn cancelling_research_allows_enqueueing_again() {
        let rules = crate::upgrades::testkit::rules();
        let (mut state, hq, _) = upgrade_world(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        enqueue_msg(&mut sim, hq, 2800);
        // As long as 2800 is "in progress", the building does not accept it again.
        assert_eq!(enqueue_msg(&mut sim, hq, 2800), 0);
        for _ in 0..51 {
            tick(&mut sim, hq);
        }
        assert_eq!(bld(&sim, hq).make_time, 10_000);
        assert_eq!((res(&sim, FOOD), res(&sim, WOOD)), (950, 900));
        // Cancel: rest 10000/15000 -> 66 % back (33 food, 66 wood), "in progress" -1.
        assert_eq!(cancel_msg(&mut sim, hq, 0), 1);
        assert_eq!((res(&sim, FOOD), res(&sim, WOOD)), (983, 966));
        assert_eq!(pending_guid_count(&sim, 0, 2800), 0);
        assert_eq!(guid_count(&sim, 0, 2800), 0);
        assert!(sim.state.applied_upgrades[0].is_empty());
        assert_eq!(bld(&sim, hq).make_guid, 0);
        // Enqueueable again, runs and takes effect at the end.
        assert_eq!(enqueue_msg(&mut sim, hq, 2800), 1);
        for _ in 0..151 {
            tick(&mut sim, hq);
        }
        assert_eq!(sim.state.applied_upgrades[0], [2800]);
    }

    #[test]
    fn progress_follows_the_x87_single_precision_rounding() {
        assert_eq!(progress_fraction(10_000, 15_000).to_bits(), 0x3EAA_AAAA);
        assert_eq!(progress_fraction(15_000, 15_000), 0.0);
        assert_eq!(progress_fraction(0, 15_000), 1.0);
        assert_eq!(progress_fraction(100, 8000), 1.0 - 100.0 / 8000.0);
        assert!(is_trade_guid(490) && is_trade_guid(493));
        assert!(!is_trade_guid(489) && !is_trade_guid(494));
    }
}
