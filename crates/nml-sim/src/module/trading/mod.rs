//! Module 43 `TRADING` (`PropertyTrading_Construct` `0x0052E1E0`, VTable `0x006263E8`, `0x90` bytes,
//! re/trading 1.2): the trade cart (command `0x45`, shuttle traffic between two headquarters) and the
//! tax level. The state is the ware and target of the running trade trip, the payer GUIDs of the tax
//! (440002), `m_MaxTax` (440003), `m_TaxInterval` in ms (440001 * 1000) and `m_TaxLevel` (the tax
//! level, start 50). Behavior: the trade cycle `0x12B`/`0x32B`/`0x22B` (re/trading 3.1-3.5) with the
//! recursion guard (A7) and the tax `0x62B`/`0x72B`/`0x82B` with a timer in the tick (re/trading 3.6).
//! The command `0x45` is in [`crate::command::trade`].

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::accounting::{add_statistic, kind::TRADE as TRADE_STAT};
use crate::collision::find_nearest_matching;
use crate::economy::{guid_count, market_adjust, market_convert};
use crate::filter::matches_class;
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::message::{Envelope, Message};
use crate::messaging::dispatch_command;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::events::update_events;
use super::gather::{capacity, load, position, set_carried_load};
use super::stand::parse_int_list;
use super::{ModuleKind, init_flags};

/// Module index of Trading.
pub const INDEX: usize = 0x2B;
/// `VALUEID` of the tax interval in seconds (`0x6B6C1`).
pub const VALUE_TAX_INTERVAL: u32 = 440001;
/// `VALUEID` of the payer GUIDs (`0x6B6C2`).
pub const VALUE_PAYERS: u32 = 440002;
/// `VALUEID` of the maximum tax (`0x6B6C3`).
pub const VALUE_MAX_TAX: u32 = 440003;
/// Start value of the tax level (`PropertyTrading_Init` `0x0052EC88`).
pub const START_TAX_LEVEL: i32 = 50;

#[cfg(test)]
mod unit_tests;

/// Messages of trading (query dispatcher `0x0052EBB0`, re/trading 2.1).
pub mod msg {
    /// `{kind, target}`: start trading (`0x0052EA10`); 1, 0 without a home HQ.
    pub const START: u32 = 0x12B;
    /// Walk success at the partner HQ: exchange, drive home (`0x0052E410`); 1/0.
    pub const AT_PARTNER: u32 = 0x22B;
    /// Walk success at the home HQ: deliver, load, go to the partner (`0x0052E6C0`); 1/0.
    pub const AT_HOME: u32 = 0x32B;
    /// Walk failure on the way to the partner (`0x0052E2A0`): nothing, 0.
    pub const PARTNER_FAILED: u32 = 0x42B;
    /// Walk failure on the way home (`0x0052E2A0`): nothing, 0.
    pub const HOME_FAILED: u32 = 0x52B;
    /// Collect tax (`0x0052E2B0`).
    pub const COLLECT_TAX: u32 = 0x62B;
    /// Create the permanent tax timer (`0x0052E380`).
    pub const START_TAX: u32 = 0x72B;
    /// `{level}`: set the tax level (`0x0052E3C0`).
    pub const SET_TAX_LEVEL: u32 = 0x82B;
}

/// Depth of synchronously nested `0x22B`/`0x32B` from which a handler ends immediately (A7,
/// re/trading 10.4 F3).
const MAX_SYNC_DEPTH: u8 = 2;

/// Walk-Befehl `FINDTO`.
const FIND_TO: u32 = 0x200;

/// Timer group of the tax timer (`0x0052E380`).
const TAX_TIMER_GROUP: u8 = 0x80;

/// Class "finished building" for `GameObject_MatchesClass` in `0x0052E2C3` (module 3 with flag
/// `0x400`).
const FINISHED_BUILDING: u32 = 0x403;

/// `CPropertyTrading`; in the savegame the fields of `0x0052ECB0` (re/trading 5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trading {
    /// `m_TradeWare` (`+0x70`): trade kind 0 to 3 (buy wood, buy food, sell wood, sell food). Unset in
    /// the original until `0x12B`, 0 in the reimplementation.
    pub trade_ware: i32,
    /// `m_TradeTarget` (`+0x74`): target slot 0 to 8.
    pub trade_target: i32,
    /// `m_PayerGUIDs` (`+0x78`, 440002).
    pub payer_guids: Vec<i32>,
    /// `m_MaxTax` (`+0x84`, 440003).
    pub max_tax: i32,
    /// `m_TaxInterval` (`+0x88`) in ms: 440001 * 1000 (`IMUL` without overflow protection, hence
    /// `wrapping_mul`; a negative data value gives a negative interval).
    pub tax_interval: i32,
    /// `m_TaxLevel` (`+0x8C`, tax level 0 to 100, start 50).
    pub tax_level: i32,
    /// Recursion guard of the synchronous feedback messages `0x22B`/`0x32B` (re/trading 10.4 F3):
    /// never != 0 beyond a tick, hence not in the savegame.
    #[serde(skip)]
    pub sync_depth: u8,
}

/// `PropertyTrading_Init` (`0x0052EC40`, re/trading 1.4): payer list from 440002, `m_MaxTax` from
/// 440003, `m_TaxInterval = 440001 * 1000`, `m_TaxLevel = 50`. The original does not set ware and
/// target (0 in the reimplementation). A finished object (`ready`) starts the tax timer like `0x72B`
/// (`FUN_0052E380`, 3.6).
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc, ready: bool) {
    let flags = init_flags(INDEX, desc);
    let int = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let payers = parse_int_list(
        &desc.values.get(&VALUE_PAYERS).cloned().unwrap_or_default(),
        &[],
    );
    let (max_tax, interval) = (int(VALUE_MAX_TAX), int(VALUE_TAX_INTERVAL));
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        if let ModuleKind::Trading(t) = &mut m.kind {
            *t = Trading {
                payer_guids: payers,
                max_tax,
                tax_interval: interval.wrapping_mul(1000),
                tax_level: START_TAX_LEVEL,
                ..Trading::default()
            };
        }
    }
    if ready {
        start_tax_timer(sim, id);
    }
}

/// Tick (slot 13): only `Module_UpdateEvents` (re/trading 3.6), which lets the tax timer run.
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
}

/// `GameWorld_GetHeadquartersGuid` (`0x00477C00`, re/trading 3.2): HQ type by the nation of the
/// slot (`GameEngine_GetPlayer`, only 0 to 7): 0 -> 1300, 1 -> 1600, 2 -> 1000, 3 -> 2500,
/// 4 -> 1900, 5 -> 2200; otherwise 0 (finds nothing).
pub fn hq_guid(sim: &Sim, slot: i32) -> u32 {
    let Some(p) = usize::try_from(slot)
        .ok()
        .filter(|&s| s < PLAYER_SLOTS)
        .and_then(|s| sim.state.players.get(s))
    else {
        return 0;
    };
    match p.nation {
        0 => 1300,
        1 => 1600,
        2 => 1000,
        3 => 2500,
        4 => 1900,
        5 => 2200,
        _ => 0,
    }
}

/// `PropertyTrading_Query` (`0x0052EBB0`, re/trading 2.1); unknown codes -1.
pub fn query(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    match m.code {
        msg::START => on_start_trade(sim, id, m),
        msg::AT_PARTNER => guarded(sim, id, on_arrived_at_partner),
        msg::AT_HOME => guarded(sim, id, deliver_and_reload),
        msg::PARTNER_FAILED | msg::HOME_FAILED => 0,
        msg::COLLECT_TAX => collect_tax(sim, id),
        msg::START_TAX => start_tax_timer(sim, id),
        msg::SET_TAX_LEVEL => set_tax_level(sim, id, m.arg(0)),
        _ => -1,
    }
}

/// `0x72B`/`Init(finished)` (`FUN_0052E380`, re/trading 3.6): for `m_TaxInterval > 0` a permanent
/// timer `0x62B` of group `0x80`; 1. Every call creates another one (W-243).
fn start_tax_timer(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(interval) = trading(sim, id).map(|t| t.tax_interval) else {
        return -1;
    };
    if interval > 0
        && let Some(m) = sim.module_mut(id, INDEX)
    {
        m.base.post_delayed_message(
            interval,
            Message::new(msg::COLLECT_TAX, &[]),
            true,
            TAX_TIMER_GROUP,
        );
    }
    1
}

/// `0x62B` (`0x0052E2B0`, re/trading 3.6): only a finished building (`MatchesClass(0x403)`, not
/// dead or quartered) with an owner player (slot 0 to 7), otherwise -1 (W-243: the cart never
/// pays). Gold `+= level * MaxTax * sum of counts(payers) / 100` (`IMUL` without overflow
/// protection, division toward zero); 1.
fn collect_tax(sim: &mut Sim, id: ObjectId) -> i32 {
    if !matches_class(sim, id, FINISHED_BUILDING) {
        return -1;
    }
    let slot = usize::from(id.slot());
    if sim.state.players.get(slot).is_none() {
        return -1;
    }
    let Some((payers, level, max_tax)) =
        trading(sim, id).map(|t| (t.payer_guids.clone(), t.tax_level, t.max_tax))
    else {
        return -1;
    };
    // `Player_GetGuidCount` (`0x0047DD90`) per payer GUID, groups resolved.
    let count = payers.iter().fold(0i32, |c, &g| {
        c.wrapping_add(guid_count(sim, slot as i32, g as u32))
    });
    let tax = level.wrapping_mul(max_tax).wrapping_mul(count) / 100;
    sim.state.players[slot].add_resource(0, tax);
    1
}

/// `0x82B {level}` (`0x0052E3C0`, re/trading 3.6): set the tax level unchecked (W-244); 1.
fn set_tax_level(sim: &mut Sim, id: ObjectId, level: i32) -> i32 {
    let Some(t) = trading_mut(sim, id) else {
        return -1;
    };
    log::debug!("Tax level set from {} to {level}", t.tax_level);
    t.tax_level = level;
    1
}

fn trading<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Trading> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Trading(t) => Some(t),
        _ => None,
    }
}

fn trading_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Trading> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Trading(t) => Some(t),
        _ => None,
    }
}

/// Recursion guard (A7, re/trading 3.5, 10.4 F3): `0x22B`/`0x32B` increase `sync_depth` on entry and
/// decrease it on exit. If a handler is entered at depth >= 2, two jumps were synchronous; from
/// here the original recurses endlessly until the stack overflows (K-290). The reimplementation
/// then ends like "no building found": 0, nothing changed, the cycle stops. A normal cycle never
/// reaches depth 2 and stays bit-identical.
fn guarded(sim: &mut Sim, id: ObjectId, handler: fn(&mut Sim, ObjectId) -> i32) -> i32 {
    let Some(depth) = trading(sim, id).map(|t| t.sync_depth) else {
        return -1;
    };
    if depth >= MAX_SYNC_DEPTH {
        log::debug!("Trade cart {id:?}: synchronous recursion ended (A7)");
        return 0;
    }
    if let Some(t) = trading_mut(sim, id) {
        t.sync_depth = depth + 1;
    }
    let r = handler(sim, id);
    if let Some(t) = trading_mut(sim, id) {
        t.sync_depth = t.sync_depth.saturating_sub(1);
    }
    r
}

/// Building search (re/trading 3.2, all three places the same): nearest object of type `guid` of a
/// player whom `reference` lists as a friend (`MM_MODE 0x818` = `CHECKGUID | ANDALL |
/// HL_FRIENDLIST`), over the whole map (radius -1), edge distance `+0x1B0`, without region filter
/// (W-241).
fn find_hq(sim: &mut Sim, id: ObjectId, guid: u32, reference: i32) -> Option<ObjectId> {
    let f = MatchFilter {
        mode: 0x818,
        guid: guid as i32,
        ..MatchFilter::constructed()
    };
    let (pos, hard) = (
        position(sim, id),
        sim.object(id).map_or(0, |o| o.hard_radius),
    );
    find_nearest_matching(sim, pos, &f, -1, reference, hard)
}

/// `FINDTO {hq, -1, 1}` as a command envelope to the own walk module, synchronous
/// (`GameObject_DispatchPostedCommand` `0x00473690`, like `gather::goto_delivery_building`).
fn drive_to(sim: &mut Sim, id: ObjectId, hq: ObjectId, success: u32, fault: u32) {
    let env = Envelope {
        msg: Message::new(FIND_TO, &[hq.as_i32(), -1, 1]),
        success: Some(Message::new(success, &[])),
        fault: Some(Message::new(fault, &[])),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, id, &env);
}

/// Search the home HQ (reference: the own slot, 3.1/3.3) and drive there: success `0x32B`, failure `0x52B`.
/// Without a home HQ `Output error message!` and 0.
fn drive_home(sim: &mut Sim, id: ObjectId) -> i32 {
    let slot = i32::from(id.slot());
    let guid = hq_guid(sim, slot);
    let Some(hq) = find_hq(sim, id, guid, slot) else {
        log::debug!("Output error message!");
        return 0;
    };
    drive_to(sim, id, hq, msg::AT_HOME, msg::HOME_FAILED);
    1
}

/// `PropertyTrading_OnStartTrade` (`0x0052EA10`, re/trading 3.1): store kind and target unchecked
/// (W-239), then go to the own HQ. If the cart is already within its target radius, `0x32B` still
/// runs inside this call (3.5).
fn on_start_trade(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let (ware, target) = (m.arg(0), m.arg(1));
    let Some(t) = trading_mut(sim, id) else {
        return -1;
    };
    t.trade_ware = ware;
    t.trade_target = target;
    drive_home(sim, id)
}

/// `PropertyTrading_OnArrivedAtPartner` (`0x0052E410`, re/trading 3.3): exchange the load at the
/// market rate, then home. Kind 0/1 buys wood/food for the gold value of the load (price rises by
/// ware/25), kind 2/3 sells (price falls by raw amount/25; W-240: every load except wood
/// corrects the food price). The partner gets nothing (W-236), the new load may exceed the load
/// capacity (W-237), gold from 512 overflows in `market_convert` (W-246). Kind > 3 ->
/// `TradeError!`, 0. Without a home HQ the exchanged load stays on the cart.
fn on_arrived_at_partner(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(ware) = trading(sim, id).map(|t| t.trade_ware) else {
        return -1;
    };
    let Some((a, kind)) = sim.object(id).map(|o| (o.load_amount, o.load_kind)) else {
        return -1;
    };
    let econ = &mut sim.state.economy;
    // Gold value of the load at the current rate; `a` stays the raw amount.
    let g = match kind {
        load::WOOD => market_convert(econ.wood_price, a),
        load::FOOD => market_convert(econ.food_price, a),
        _ => a,
    };
    // Jump table `0x0052E6B0` (`JA` for kind > 3, also negative).
    let (n, k) = match ware {
        0 => {
            let w = market_convert(econ.wood_price, g.wrapping_neg());
            market_adjust(&mut econ.wood_price, w);
            (w, load::WOOD)
        }
        1 => {
            let f = market_convert(econ.food_price, g.wrapping_neg());
            market_adjust(&mut econ.food_price, f);
            (f, load::FOOD)
        }
        2 | 3 => {
            if kind == load::WOOD {
                market_adjust(&mut econ.wood_price, a.wrapping_neg());
            } else {
                market_adjust(&mut econ.food_price, a.wrapping_neg());
            }
            (g, load::GOLD)
        }
        _ => {
            log::debug!("TradeError!");
            return 0;
        }
    };
    set_carried_load(sim, id, n, k);
    drive_home(sim, id)
}

/// `PropertyTradeCart_DeliverAndReload` (`0x0052E6C0`, re/trading 3.4): without an owner player
/// (slot 8) 0 before anything else. Otherwise credit a load `a > 0` to the owner
/// ([`SimEvent::Credited`] for the floating number; statistic 11 only for food, `0x0052E77E`),
/// then load `min(load capacity, stock)` of the trade ware - for gold with statistic 11 over the
/// loaded amount (`0x0052E829`; O-171, W-242) - and debit it immediately (empty account: empty
/// run, W-238) and drive to the partner HQ (reference: the target, W-234). Invalid kind:
/// `does not happens!` not until after the delivery, the load stays (W-239). No partner HQ (target 8,
/// W-235): the load stays, the cart stands still.
fn deliver_and_reload(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some((ware, target)) = trading(sim, id).map(|t| (t.trade_ware, t.trade_target)) else {
        return -1;
    };
    let Some((a, kind)) = sim.object(id).map(|o| (o.load_amount, o.load_kind)) else {
        return -1;
    };
    let slot = usize::from(id.slot());
    if sim.state.players.get(slot).is_none() {
        return 0;
    }
    if a > 0 {
        // `Player_AddResource`: kinds 0 to 4, otherwise silent.
        sim.state.events.push(SimEvent::Credited {
            object: id,
            amount: a,
            kind,
        });
        if let Ok(i) = usize::try_from(kind) {
            sim.state.players[slot].add_resource(i, a);
        }
        if kind == load::FOOD {
            add_statistic(sim, slot as i32, TRADE_STAT, a);
        }
    }
    // `AttributeSet_GetMaxValue(17)` including upgrade percent; `CMP/JL`: for load capacity < stock the
    // load capacity, otherwise the stock (table `0x0052EA00`).
    let cap = capacity(sim, id);
    let (res, k) = match ware {
        0 | 1 => (0, load::GOLD),
        2 => (1, load::WOOD),
        3 => (2, load::FOOD),
        _ => {
            log::debug!("does not happens!");
            return 0;
        }
    };
    let n = cap.min(sim.state.resource(slot, res));
    if k == load::GOLD {
        add_statistic(sim, slot as i32, TRADE_STAT, n);
    }
    sim.state.players[slot].add_resource(res, n.wrapping_neg());
    set_carried_load(sim, id, n, k);
    let guid = hq_guid(sim, target);
    let Some(hq) = find_hq(sim, id, guid, target) else {
        log::debug!("Output error message!");
        return 0;
    };
    drive_to(sim, id, hq, msg::AT_PARTNER, msg::PARTNER_FAILED);
    1
}
