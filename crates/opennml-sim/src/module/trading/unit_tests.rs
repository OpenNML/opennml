//! Tests of the trade cycle (re/trading 3.1-3.5, 9.1, 9.2) in the test world
//! [`crate::module::tent::testkit`]: slot 1 SPA with HQ 1000 and cart 1110, slot 2 ENG with HQ
//! 1900 and trading office 1907. Slot 1 has wood 0 (the vectors in 9.2 rely on it).
//! One object tick is two `tick::step`; direct `send` calls run outside the tick and write their
//! events to `sim.state.events`.

use super::*;
use crate::command::{Command, CommandKind};
use crate::message::Message;
use crate::messaging::send;
use crate::module::gather::{self, load};
use crate::module::tent::{self, testkit::*};
use crate::module::walk;
use crate::rules::Rules;
use crate::state::{Control, SimEvent, SimState};
use crate::world::{ObjectDesc, create_object, desc_flags};

/// Tile of the own HQ 1000 (slot 1).
const HOME: (i32, i32) = (20, 20);
/// Docking place of the cart at HQ 1000 in world coordinates (measured: `0xB03` yields grid
/// `(91136, 84992)` = tile 22.25/20.75 with facing direction -180 degrees, i.e. rotation -pi/2). The
/// goal radius of a building target is `max(0x100, base speed)` (re/walk 2.3,
/// `walk/goto.rs`), measured 584 for the cart of the test world (`GetGoalRadius` keeps it, since it
/// is not below half the travel speed). From the tile center (22, 20) (distance
/// 1448) the docking place lies outside; from there `0x32B` does not come until after 10 `step`. Docked and
/// aligned, the FINDTO success comes synchronously (re/trading 3.5).
const DOCK: (f32, f32) = (2848.0, 2656.0);
/// Tile of the partner HQ 1900 (slot 2).
const PARTNER: (i32, i32) = (44, 20);
/// Tile of the trading office 1907 (slot 2).
const TRADE_POST_TILE: (i32, i32) = (44, 26);
/// Between the HQs, outside every goal radius.
const MIDDLE: Place = Place::Tile(32, 32);
/// Upper limit of `step` for a trip between the HQs.
const TRIP: usize = 1500;

/// Test world with wood 0 for slot 1.
fn trade_state(rules: &Rules) -> SimState {
    let mut s = state(rules);
    s.players[1].set_resource(1, 0);
    s
}

/// Where the cart stands.
#[derive(Clone, Copy)]
enum Place {
    /// Docked at the own HQ ([`DOCK`], aligned).
    Docked,
    /// On the center of a tile.
    Tile(i32, i32),
}

/// HQ 1000 (slot 1), HQ 1900 and trading office 1907 (slot 2), cart 1110 (slot 1).
fn trade_world(sim: &mut Sim, cart: Place) -> ObjectId {
    spawn(sim, SPA_HQ, 1, HOME);
    spawn(sim, ENG_HQ, 2, PARTNER);
    spawn(sim, TRADE_POST, 2, TRADE_POST_TILE);
    match cart {
        Place::Tile(x, z) => spawn(sim, CART, 1, (x, z)),
        Place::Docked => {
            let mut desc = ObjectDesc::new(CART, 1, [DOCK.0, 0.0, DOCK.1]);
            desc.rotation = [0.0, -std::f32::consts::FRAC_PI_2, 0.0];
            desc.flags |= desc_flags::PLACED | desc_flags::ROT_Y;
            create_object(sim, &mut desc).expect("cart created")
        }
    }
}

fn trading_of(sim: &Sim, id: ObjectId) -> Trading {
    match &sim.module(id, INDEX).expect("module 43").kind {
        ModuleKind::Trading(t) => t.clone(),
        k => panic!("not trading: {k:?}"),
    }
}

fn set_trading(sim: &mut Sim, id: ObjectId, ware: i32, target: i32) {
    if let Some(ModuleKind::Trading(t)) = sim.module_mut(id, INDEX).map(|m| &mut m.kind) {
        t.trade_ware = ware;
        t.trade_target = target;
    }
}

/// Raw load (`+0x2A8`, `+0x2AC`).
fn load_of(sim: &Sim, id: ObjectId) -> (i32, i32) {
    sim.object(id)
        .map(|o| (o.load_amount, o.load_kind))
        .unwrap()
}

/// Gold and wood of a slot.
fn account(sim: &Sim, slot: usize) -> (i32, i32) {
    (sim.state.resource(slot, 0), sim.state.resource(slot, 1))
}

fn wood_price(sim: &Sim) -> i32 {
    sim.state.economy.wood_price
}

fn food_price(sim: &Sim) -> i32 {
    sim.state.economy.food_price
}

fn start(sim: &mut Sim, cart: ObjectId, ware: i32, target: i32) -> i32 {
    send(sim, cart, &Message::new(msg::START, &[ware, target]))
}

/// Success and failure message that walk holds for the running trip.
fn walk_replies(sim: &Sim, id: ObjectId) -> (Option<u32>, Option<u32>) {
    let b = &sim.module(id, walk::INDEX).expect("walk").base;
    (
        b.success.as_ref().map(|m| m.code),
        b.fault.as_ref().map(|m| m.code),
    )
}

fn walk_dest(sim: &Sim, id: ObjectId) -> ObjectId {
    match &sim.module(id, walk::INDEX).expect("walk").kind {
        ModuleKind::Walk(w) => w.dest,
        k => panic!("not a walk module: {k:?}"),
    }
}

/// Does the position change in `n` further `step`?
fn moves_within(sim: &mut Sim, id: ObjectId, n: usize) -> bool {
    let before = gather::position(sim, id);
    run_steps(sim, n);
    gather::position(sim, id) != before
}

fn credited(events: &[SimEvent]) -> Vec<SimEvent> {
    events
        .iter()
        .filter(|e| matches!(e, SimEvent::Credited { .. }))
        .cloned()
        .collect()
}

/// `step` without commands until `done` holds (at most `max`); count and events.
fn run_until(sim: &mut Sim, max: usize, done: impl Fn(&Sim) -> bool) -> (usize, Vec<SimEvent>) {
    let mut events = Vec::new();
    for n in 1..=max {
        events.extend(crate::tick::step(sim.state, sim.rules, &[]));
        if done(sim) {
            return (n, events);
        }
    }
    panic!("condition not reached after {max} steps");
}

// QUIRK W-233: kept
#[test]
fn the_headquarters_guid_follows_the_nation() {
    // `0x00477C00`: nation 0...5 -> 1300, 1600, 1000, 2500, 1900, 2200; otherwise and slot not in 0...7 -> 0.
    let rules = rules();
    let mut state = state(&rules);
    for (nation, guid) in [
        (0, 1300),
        (1, 1600),
        (2, 1000),
        (3, 2500),
        (4, 1900),
        (5, 2200),
        (6, 0),
        (-1, 0),
    ] {
        state.players[5].nation = nation;
        assert_eq!(
            hq_guid(&Sim::new(&mut state, &rules), 5),
            guid,
            "nation {nation}"
        );
    }
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(
        (hq_guid(&sim, 0), hq_guid(&sim, 1), hq_guid(&sim, 2)),
        (1600, 1000, 1900)
    );
    assert_eq!(
        (hq_guid(&sim, 8), hq_guid(&sim, -1), hq_guid(&sim, 9)),
        (0, 0, 0)
    );
}

#[test]
fn the_trade_messages_answer_like_the_dispatcher() {
    // Dispatcher 2.1: failure messages 0, unknown codes -1.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, MIDDLE);
    let before = load_of(&sim, cart);
    assert_eq!(
        send(&mut sim, cart, &Message::new(msg::PARTNER_FAILED, &[])),
        0
    );
    assert_eq!(
        send(&mut sim, cart, &Message::new(msg::HOME_FAILED, &[])),
        0
    );
    assert_eq!(send(&mut sim, cart, &Message::new(0x92B, &[])), -1);
    assert_eq!(load_of(&sim, cart), before);
}

// QUIRK W-233, W-236, W-242: kept
// QUIRK K-290: deviation
#[test]
fn the_trade_cart_shuttles_like_the_original() {
    // re/trading 9.2 (slot 1 instead of 0): gold 1000, wood 0, wood price 1024.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    sim.state.events.clear();
    assert_eq!(account(&sim, 1), (1000, 0));
    assert_eq!(load_of(&sim, cart).0, 0);
    assert_eq!(wood_price(&sim), 1024);

    // Measured: docked, `0x32B` still runs inside `0x12B` (hypothesis "immediate 0x32B").
    assert_eq!(start(&mut sim, cart, 0, 2), 1);
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
    assert_eq!(wood_price(&sim), 1024);
    assert_eq!(credited(&sim.state.events), [], "empty load: no delivery");
    assert_eq!(
        walk_replies(&sim, cart),
        (Some(msg::AT_PARTNER), Some(msg::PARTNER_FAILED))
    );

    // Arrival at HQ 1900: 800 wood, price 1056.
    let (_, ev) = run_until(&mut sim, TRIP, |s| load_of(s, cart).1 == load::WOOD);
    assert_eq!(load_of(&sim, cart), (800, load::WOOD));
    assert_eq!(wood_price(&sim), 1056);
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(credited(&ev), []);
    assert_eq!(
        walk_replies(&sim, cart),
        (Some(msg::AT_HOME), Some(msg::HOME_FAILED))
    );

    // At home: deliver (exactly one `Credited`), reload.
    let (_, ev) = run_until(&mut sim, TRIP, |s| load_of(s, cart).1 == load::GOLD);
    assert_eq!(account(&sim, 1), (600, 800));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
    assert_eq!(wood_price(&sim), 1056);
    assert_eq!(
        credited(&ev),
        [SimEvent::Credited {
            object: cart,
            amount: 800,
            kind: load::WOOD
        }]
    );

    // Second round: 775 wood, price 1087; at home gold 400, wood 1575.
    run_until(&mut sim, TRIP, |s| load_of(s, cart).1 == load::WOOD);
    assert_eq!(load_of(&sim, cart), (775, load::WOOD));
    assert_eq!(wood_price(&sim), 1087);
    let (_, ev) = run_until(&mut sim, TRIP, |s| load_of(s, cart).1 == load::GOLD);
    assert_eq!(account(&sim, 1), (400, 1575));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
    assert_eq!(
        credited(&ev),
        [SimEvent::Credited {
            object: cart,
            amount: 775,
            kind: load::WOOD
        }]
    );
    assert_eq!(trading_of(&sim, cart).sync_depth, 0);
    // The partner gets nothing (W-236).
    assert_eq!(account(&sim, 2), (1000, 1000));
}

// QUIRK W-236, W-246: kept
#[test]
fn trade_rates_match_the_market() {
    // re/trading 9.1 (price 40 -> 859 after `market_adjust`): `0x22B` directly with a load set;
    // the cart stands aside, the trip home begins (return value 1).
    // (kind, wood price, food price, load) -> (load afterwards, wood price, food price).
    type Case = (i32, i32, i32, (i32, i32), (i32, i32), i32, i32);
    let cases: [Case; 12] = [
        (
            0,
            1024,
            1351,
            (200, load::GOLD),
            (800, load::WOOD),
            1056,
            1351,
        ),
        (
            0,
            1056,
            1351,
            (200, load::GOLD),
            (775, load::WOOD),
            1087,
            1351,
        ),
        (
            0,
            1087,
            1351,
            (200, load::GOLD),
            (753, load::WOOD),
            1117,
            1351,
        ),
        (
            1,
            1024,
            1351,
            (200, load::GOLD),
            (606, load::FOOD),
            1024,
            1375,
        ),
        (
            1,
            1024,
            1375,
            (200, load::GOLD),
            (595, load::FOOD),
            1024,
            1398,
        ),
        (
            2,
            1024,
            1351,
            (200, load::WOOD),
            (50, load::GOLD),
            1016,
            1351,
        ),
        (
            2,
            1016,
            1351,
            (200, load::WOOD),
            (49, load::GOLD),
            1008,
            1351,
        ),
        (
            3,
            1024,
            1351,
            (200, load::FOOD),
            (65, load::GOLD),
            1024,
            1343,
        ),
        (
            3,
            1024,
            1343,
            (200, load::FOOD),
            (65, load::GOLD),
            1024,
            1335,
        ),
        (
            0,
            1024,
            1351,
            (37, load::GOLD),
            (148, load::WOOD),
            1029,
            1351,
        ),
        (0, 1024, 1351, (0, load::NONE), (0, load::WOOD), 1024, 1351),
        (
            0,
            40,
            1351,
            (200, load::GOLD),
            (20480, load::WOOD),
            859,
            1351,
        ),
    ];
    let overflow: Case = (
        0,
        1024,
        1351,
        (512, load::GOLD),
        (-2048, load::WOOD),
        943,
        1351,
    );
    let rules = rules();
    for (ware, wood, food, (a, kind), want, want_wood, want_food) in
        cases.into_iter().chain([overflow])
    {
        let mut state = trade_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let cart = trade_world(&mut sim, MIDDLE);
        set_trading(&mut sim, cart, ware, 2);
        gather::set_carried_load(&mut sim, cart, a, kind);
        sim.state.economy.wood_price = wood;
        sim.state.economy.food_price = food;
        let r = send(&mut sim, cart, &Message::new(msg::AT_PARTNER, &[]));
        let case = format!("kind {ware}, prices {wood}/{food}, load {a}/{kind}");
        assert_eq!(r, 1, "{case}");
        assert_eq!(load_of(&sim, cart), want, "{case}");
        assert_eq!(
            (wood_price(&sim), food_price(&sim)),
            (want_wood, want_food),
            "{case}"
        );
        // No account is touched, the trip home runs.
        assert_eq!(account(&sim, 1), (1000, 0), "{case}");
        assert_eq!(account(&sim, 2), (1000, 1000), "{case}");
        assert_eq!(
            walk_replies(&sim, cart),
            (Some(msg::AT_HOME), Some(msg::HOME_FAILED))
        );
    }
}

// QUIRK W-239: deviation
#[test]
fn an_invalid_ware_at_the_partner_is_a_trade_error() {
    // 3.3: kind > 3 (also negative) -> `TradeError!`, 0, nothing changed.
    let rules = rules();
    for ware in [4, 7, -1] {
        let mut state = trade_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let cart = trade_world(&mut sim, MIDDLE);
        set_trading(&mut sim, cart, ware, 2);
        gather::set_carried_load(&mut sim, cart, 200, load::GOLD);
        assert_eq!(send(&mut sim, cart, &Message::new(msg::AT_PARTNER, &[])), 0);
        assert_eq!(load_of(&sim, cart), (200, load::GOLD));
        assert_eq!((wood_price(&sim), food_price(&sim)), (1024, 1351));
        assert_eq!(walk_replies(&sim, cart), (None, None));
    }
}

// QUIRK W-235: kept
#[test]
fn trade_to_slot_8_stops_with_the_load() {
    // W-235 (9.2): target 8 -> after the first `0x32B` gold 800, load 200 gold, no FINDTO.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    assert_eq!(start(&mut sim, cart, 0, 8), 1);
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
    assert_eq!(walk_replies(&sim, cart), (None, None));
    assert!(!moves_within(&mut sim, cart, 40));
    assert_eq!(
        (account(&sim, 1), load_of(&sim, cart)),
        ((800, 0), (200, load::GOLD))
    );
}

#[test]
fn a_cart_without_home_headquarters_does_not_start() {
    // 3.1: no home HQ -> `Output error message!`, 0; kind and target are stored nevertheless.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    spawn(&mut sim, ENG_HQ, 2, PARTNER);
    let cart = spawn(&mut sim, CART, 1, (22, 20));
    assert_eq!(start(&mut sim, cart, 1, 2), 0);
    let t = trading_of(&sim, cart);
    assert_eq!((t.trade_ware, t.trade_target), (1, 2));
    assert_eq!(walk_replies(&sim, cart), (None, None));
    assert_eq!(account(&sim, 1), (1000, 0));
}

#[test]
fn a_cart_of_slot_8_delivers_nothing() {
    // 3.4: without an owner player (slot 8) -> 0 before anything else.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    trade_world(&mut sim, MIDDLE);
    let cart = spawn(&mut sim, CART, 8, (32, 32));
    set_trading(&mut sim, cart, 0, 2);
    gather::set_carried_load(&mut sim, cart, 300, load::WOOD);
    sim.state.events.clear();
    assert_eq!(send(&mut sim, cart, &Message::new(msg::AT_HOME, &[])), 0);
    assert_eq!(load_of(&sim, cart), (300, load::WOOD));
    assert_eq!(credited(&sim.state.events), []);
    assert_eq!(walk_replies(&sim, cart), (None, None));
}

// QUIRK W-238: kept
#[test]
fn an_empty_account_keeps_driving_empty() {
    // W-238: gold 0 -> empty run, 0 exchanged, the cycle continues.
    let rules = rules();
    let mut state = trade_state(&rules);
    state.players[1].set_resource(0, 0);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    assert_eq!(start(&mut sim, cart, 0, 2), 1);
    assert_eq!(load_of(&sim, cart), (0, load::GOLD));
    assert_eq!(
        walk_replies(&sim, cart),
        (Some(msg::AT_PARTNER), Some(msg::PARTNER_FAILED))
    );
    let (_, mut ev) = run_until(&mut sim, TRIP, |s| load_of(s, cart).1 == load::WOOD);
    assert_eq!(load_of(&sim, cart), (0, load::WOOD));
    assert_eq!(wood_price(&sim), 1024);
    let (_, more) = run_until(&mut sim, TRIP, |s| load_of(s, cart).1 == load::GOLD);
    ev.extend(more);
    assert_eq!(load_of(&sim, cart), (0, load::GOLD));
    assert_eq!(account(&sim, 1), (0, 0));
    assert_eq!(credited(&ev), [], "nothing to deliver");
    // On to the partner.
    assert_eq!(
        walk_replies(&sim, cart),
        (Some(msg::AT_PARTNER), Some(msg::PARTNER_FAILED))
    );
    assert!(moves_within(&mut sim, cart, 20));
}

// QUIRK W-240: kept
#[test]
fn selling_with_a_foreign_load_adjusts_the_food_price() {
    // W-240: kind 2/3 with a load that is not wood corrects the food price by the raw amount;
    // gold counts with its amount. Wood with kind 3 corrects the wood price.
    let rules = rules();
    // (kind, load) -> (load afterwards, wood price, food price)
    for (ware, (a, kind), want, wood, food) in [
        (2, (200, load::GOLD), (200, load::GOLD), 1024, 1343),
        (2, (200, load::FOOD), (65, load::GOLD), 1024, 1343),
        (3, (200, load::WOOD), (50, load::GOLD), 1016, 1351),
        (3, (200, load::GOLD), (200, load::GOLD), 1024, 1343),
    ] {
        let mut state = trade_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let cart = trade_world(&mut sim, MIDDLE);
        set_trading(&mut sim, cart, ware, 2);
        gather::set_carried_load(&mut sim, cart, a, kind);
        assert_eq!(send(&mut sim, cart, &Message::new(msg::AT_PARTNER, &[])), 1);
        let case = format!("kind {ware}, load {a}/{kind}");
        assert_eq!(load_of(&sim, cart), want, "{case}");
        assert_eq!((wood_price(&sim), food_price(&sim)), (wood, food), "{case}");
    }
}

// QUIRK W-239: deviation
#[test]
fn an_invalid_ware_logs_after_delivery() {
    // W-239: `0x12B {7, 2}` stores the kind unchecked; `0x32B` delivers and does not reload
    // (`does not happens!`), the delivered load stays on the cart.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    gather::set_carried_load(&mut sim, cart, 300, load::WOOD);
    sim.state.events.clear();
    assert_eq!(start(&mut sim, cart, 7, 2), 1);
    assert_eq!(trading_of(&sim, cart).trade_ware, 7);
    assert_eq!(account(&sim, 1), (1000, 300));
    assert_eq!(
        credited(&sim.state.events),
        [SimEvent::Credited {
            object: cart,
            amount: 300,
            kind: load::WOOD
        }]
    );
    assert_eq!(load_of(&sim, cart), (300, load::WOOD));
    assert_eq!(walk_replies(&sim, cart), (None, None));
}

// QUIRK W-241: kept
#[test]
fn an_unreachable_hq_ends_the_cycle_silently() {
    // W-241: the search knows no region; HQ 1900 behind a wall across the whole map is still
    // chosen -> walk reports `0x42B`, the cycle ends silently, the load stays.
    let rules = rules();
    let mut state = trade_state(&rules);
    for z in 0..64 {
        state.path.mark_cell(1, 32, z, true);
    }
    state.path.build_regions(u32::MAX);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    // The search without region filter finds HQ 1900 behind the wall (otherwise the test would not
    // distinguish W-241 from "no HQ found").
    let partner = sim.state.tables[2]
        .objects
        .iter()
        .flatten()
        .find(|o| o.guid == ENG_HQ)
        .unwrap()
        .id;
    assert_eq!(find_hq(&mut sim, cart, ENG_HQ, 2), Some(partner));
    // Measured: FINDTO finds no coarse path (`plan_high_path`) and aborts at once; the failure
    // `0x42B` still comes inside `0x12B` (via `0x32B`).
    assert_eq!(start(&mut sim, cart, 0, 2), 1);
    assert_eq!(walk_replies(&sim, cart), (None, None));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(trading_of(&sim, cart).sync_depth, 0);
    assert!(!moves_within(&mut sim, cart, 200));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(walk_replies(&sim, cart), (None, None));
}

// QUIRK W-234: kept
#[test]
fn friends_of_the_same_nation_count_as_home() {
    // W-234: a second SPA player (slot 3) whom slot 1 lists as a friend has a nearer HQ
    // -> home is its HQ; without friendship the own one.
    let rules = rules();
    for friend in [true, false] {
        let mut state = trade_state(&rules);
        let p = &mut state.players[3];
        p.slot_active = true;
        p.control = Control::Human;
        p.nation = 2;
        state.diplomacy.set_friend(1, 3, friend);
        let mut sim = Sim::new(&mut state, &rules);
        let cart = trade_world(&mut sim, MIDDLE);
        let near = spawn(&mut sim, SPA_HQ, 3, (32, 26));
        let own = sim.state.tables[1]
            .objects
            .iter()
            .flatten()
            .find(|o| o.guid == SPA_HQ)
            .unwrap()
            .id;
        set_trading(&mut sim, cart, 0, 2);
        gather::set_carried_load(&mut sim, cart, 200, load::GOLD);
        assert_eq!(send(&mut sim, cart, &Message::new(msg::AT_PARTNER, &[])), 1);
        assert_eq!(
            walk_dest(&sim, cart),
            if friend { near } else { own },
            "friend {friend}"
        );
        // Arrival at home books to the own account (slot 1).
        run_until(&mut sim, TRIP, |s| load_of(s, cart).1 == load::GOLD);
        assert_eq!(account(&sim, 1), (800, 800), "friend {friend}");
        assert_eq!(account(&sim, 3).1, 30000, "friend {friend}");
    }
}

// QUIRK W-237: kept
#[test]
fn the_load_may_exceed_the_capacity() {
    // W-237: 200 gold -> 800 wood; `+0x2A8` stays 800, only attribute 17 is clamped to 200.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, MIDDLE);
    assert_eq!(gather::capacity(&sim, cart), 200);
    set_trading(&mut sim, cart, 0, 2);
    gather::set_carried_load(&mut sim, cart, 200, load::GOLD);
    assert_eq!(send(&mut sim, cart, &Message::new(msg::AT_PARTNER, &[])), 1);
    assert_eq!(load_of(&sim, cart), (800, load::WOOD));
    assert_eq!(sim.attribute(cart, gather::CAPACITY), 200);
}

#[test]
fn a_new_order_ends_the_cycle_with_the_load() {
    // 3.5: a movement command during the trip aborts walk -> `0x42B` -> 0; the load stays, no
    // further cycle.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    assert_eq!(start(&mut sim, cart, 0, 2), 1);
    assert!(moves_within(&mut sim, cart, 20));
    assert_eq!(
        walk_replies(&sim, cart),
        (Some(msg::AT_PARTNER), Some(msg::PARTNER_FAILED))
    );
    let goal = (24, 30);
    let order = Command {
        player: 1,
        kind: CommandKind::Move {
            units: vec![cart],
            x: (goal.0 << 12) + 0x800,
            z: (goal.1 << 12) + 0x800,
            target: -1,
            formation: 0,
            direction: crate::motion::NO_DIRECTION,
            spread: crate::command::NO_SPREAD,
            flags: 0,
        },
    };
    crate::tick::step(sim.state, sim.rules, &[order]);
    // The command takes effect in the next object tick of the cart: walk aborts and sends `0x42B`,
    // then the feedback messages of the movement command apply.
    let (n, _) = run_until(&mut sim, 2, |s| {
        !matches!(walk_replies(s, cart).1, Some(msg::PARTNER_FAILED))
    });
    assert_eq!(n, 1, "measured: one `step` after the command");
    let order_replies = walk_replies(&sim, cart);
    assert!(
        order_replies.0.is_some_and(|c| c & 0xFF != INDEX as u32),
        "{order_replies:?}"
    );
    run_until(&mut sim, TRIP, |s| walk_replies(s, cart) == (None, None));
    assert_eq!(sim.object(cart).unwrap().tile, goal);
    assert!(!moves_within(&mut sim, cart, 200));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(wood_price(&sim), 1024);
    assert_eq!(trading_of(&sim, cart).sync_depth, 0);
}

// QUIRK W-234: kept
// QUIRK K-290: deviation
#[test]
fn trading_with_the_own_slot_stops_after_two_synchronous_jumps() {
    // K-290 (A7): target = own slot, cart at the own HQ. `0x32B` loads 200 gold, `0x22B`
    // (synchronous, depth 2) exchanges it into 800 wood, the next `0x32B` is entered at depth 2 and
    // ends - the original recurses endlessly here.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    sim.state.events.clear();
    assert_eq!(start(&mut sim, cart, 0, 1), 1);
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(load_of(&sim, cart), (800, load::WOOD));
    assert_eq!(wood_price(&sim), 1056);
    assert_eq!(trading_of(&sim, cart).sync_depth, 0);
    assert_eq!(credited(&sim.state.events), []);
    assert_eq!(walk_replies(&sim, cart), (None, None));
    // The cycle stands; later ticks change nothing.
    run_steps(&mut sim, 40);
    assert_eq!(
        (account(&sim, 1), load_of(&sim, cart)),
        ((800, 0), (800, load::WOOD))
    );
}

// QUIRK K-290: deviation
#[test]
fn trading_with_an_allied_slot_of_the_same_nation_stops_after_two_synchronous_jumps() {
    // K-290 (A7) via the player command `0x45`: slot 3 is SPA like slot 1 and has a trading office;
    // slot 1 lists slot 3 as a friend. The only SPA HQ belongs to slot 3, the cart stands docked in
    // front of it: home search (reference 1, friend 3) and partner search (reference 3) find the
    // same HQ, all FINDTO report success synchronously (re/trading 3.2, 3.5). The original
    // recurses endlessly; the reimplementation ends after the guard.
    let rules = rules();
    let mut state = trade_state(&rules);
    let p = &mut state.players[3];
    p.slot_active = true;
    p.control = Control::Human;
    p.nation = 2;
    for index in 0..3 {
        p.set_resource(index, 1000);
    }
    state.diplomacy.set_friend(1, 3, true);
    let mut sim = Sim::new(&mut state, &rules);
    let hq = spawn(&mut sim, SPA_HQ, 3, HOME);
    spawn(&mut sim, TRADE_POST, 3, TRADE_POST_TILE);
    let mut desc = ObjectDesc::new(CART, 1, [DOCK.0, 0.0, DOCK.1]);
    desc.rotation = [0.0, -std::f32::consts::FRAC_PI_2, 0.0];
    desc.flags |= desc_flags::PLACED | desc_flags::ROT_Y;
    let cart = create_object(&mut sim, &mut desc).expect("cart created");
    assert_eq!(find_hq(&mut sim, cart, SPA_HQ, 1), Some(hq), "home");
    assert_eq!(find_hq(&mut sim, cart, SPA_HQ, 3), Some(hq), "partner");
    sim.state.events.clear();
    crate::command::execute(
        &mut sim,
        &Command {
            player: 1,
            kind: CommandKind::TradeWithPlayer {
                ware: 0,
                target: 3,
                units: vec![cart],
            },
        },
        true,
    );
    // The order lies in the FIFO and runs in the next object tick of the cart.
    run_steps(&mut sim, 2);
    let t = trading_of(&sim, cart);
    assert_eq!((t.trade_ware, t.trade_target, t.sync_depth), (0, 3, 0));
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(
        account(&sim, 3),
        (1000, 1000),
        "the partner receives nothing"
    );
    assert_eq!(load_of(&sim, cart), (800, load::WOOD));
    assert_eq!(wood_price(&sim), 1056);
    assert_eq!(credited(&sim.state.events), []);
    assert_eq!(walk_replies(&sim, cart), (None, None));
    // The cycle stands; later ticks change nothing.
    run_steps(&mut sim, 40);
    assert_eq!(
        (
            account(&sim, 1),
            load_of(&sim, cart),
            trading_of(&sim, cart).sync_depth
        ),
        ((800, 0), (800, load::WOOD), 0)
    );
}

// QUIRK K-290: deviation
#[test]
fn handlers_entered_at_depth_two_change_nothing() {
    // A7: `0x22B` and `0x32B` entered at `sync_depth >= 2` return 0 without changing load,
    // account or depth.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    set_trading(&mut sim, cart, 0, 2);
    if let Some(ModuleKind::Trading(t)) = sim.module_mut(cart, INDEX).map(|m| &mut m.kind) {
        t.sync_depth = 2;
    }
    gather::set_carried_load(&mut sim, cart, 300, load::WOOD);
    assert_eq!(send(&mut sim, cart, &Message::new(msg::AT_HOME, &[])), 0);
    assert_eq!(send(&mut sim, cart, &Message::new(msg::AT_PARTNER, &[])), 0);
    assert_eq!(load_of(&sim, cart), (300, load::WOOD));
    assert_eq!(account(&sim, 1), (1000, 0));
    assert_eq!(trading_of(&sim, cart).sync_depth, 2);
}

#[test]
fn a_packed_headquarters_is_not_found_by_the_trade_cart() {
    // The PRI HQ is the chief's tent 1600; packed, it is removed from the collision manager
    // -> `0x32B` finds no partner HQ, the cart stands loaded.
    let rules = rules();
    let mut state = trade_state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, Place::Docked);
    let chief = spawn_snapped(&mut sim, CHIEF_TENT, 0, (5000.0, 5000.0));
    assert_eq!(
        find_hq(&mut sim, cart, CHIEF_TENT, 0),
        Some(chief),
        "found when unpacked"
    );
    assert_eq!(tent::pack(&mut sim, chief), 0);
    run_steps(&mut sim, 50);
    assert_eq!(find_hq(&mut sim, cart, CHIEF_TENT, 0), None);
    assert_eq!(start(&mut sim, cart, 0, 0), 1);
    assert_eq!(account(&sim, 1), (800, 0));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
    assert_eq!(walk_replies(&sim, cart), (None, None));
    assert!(!moves_within(&mut sim, cart, 40));
    assert_eq!(load_of(&sim, cart), (200, load::GOLD));
}

// --- Tax (re/trading 3.6, 9.3) ----------------------------------------------------------------

/// Synthetic tax building: modules 3 and 43, interval 1 s (440001), payer 1100 (440002),
/// maximum tax 10 (440003). The shipped data has no building with module 43 (W-243).
const TAX_HOUSE: u32 = 1990;
/// Like [`TAX_HOUSE`], but interval 0 (440001 = 0).
const TAX_HOUSE_NO_TIMER: u32 = 1991;
/// Payer GUID of the tax buildings.
const PAYER: u32 = 1100;
/// Tile of the tax buildings.
const TAX_TILE: (i32, i32) = (10, 10);

/// Test world with the tax buildings (hull and hit points as test values).
fn tax_rules() -> Rules {
    let types = format!(
        "{TYPES}{}",
        concat!(
            "GUID 1990\nNAME Steuerhaus\nVALUEID 100 6 1 0\n",
            "PROPID 3\nVALUEID 40003 6 1 14336\nVALUEID 40004 1 1 0\n",
            "PROPID 43\nVALUEID 440001 1 1 1\nVALUEID 440002 1 1 1100\nVALUEID 440003 1 1 10\n",
            "GUID 1991\nNAME Steuerhaus_ohne_Takt\nVALUEID 100 6 1 0\n",
            "PROPID 3\nVALUEID 40003 6 1 14336\nVALUEID 40004 1 1 0\n",
            "PROPID 43\nVALUEID 440001 1 1 0\nVALUEID 440002 1 1 1100\nVALUEID 440003 1 1 10\n",
        )
    );
    let mut r = rules_from(&types);
    for guid in [TAX_HOUSE, TAX_HOUSE_NO_TIMER] {
        crate::upgrades::testkit::add_building_shape(&mut r, guid, 100.0);
        r.defaults.entry(guid).or_default().insert(14, 1000);
    }
    r
}

/// Set tax data: maximum tax directly, level via `0x82B`, number of payers in the object counter of
/// slot 1 (`guid_counts` instead of real colonists).
fn set_tax(sim: &mut Sim, id: ObjectId, level: i32, max_tax: i32, count: i32) {
    if let Some(t) = trading_mut(sim, id) {
        t.max_tax = max_tax;
    }
    assert_eq!(
        send(sim, id, &Message::new(msg::SET_TAX_LEVEL, &[level])),
        1
    );
    sim.state.guid_counts[1].insert(PAYER, count);
}

fn gold(sim: &Sim) -> i32 {
    sim.state.resource(1, 0)
}

/// `n` `step`; every change of the gold of slot 1 as `(step, increase)`.
fn gold_changes(sim: &mut Sim, n: usize) -> Vec<(usize, i32)> {
    let mut out = Vec::new();
    let mut before = gold(sim);
    for s in 1..=n {
        run_steps(sim, 1);
        let now = gold(sim);
        if now != before {
            out.push((s, now - before));
            before = now;
        }
    }
    out
}

/// Timers (waiting and running) of module 43.
fn tax_timers(sim: &Sim, id: ObjectId) -> Vec<crate::module::base::ModuleEvent> {
    let b = &sim.module(id, INDEX).expect("module 43").base;
    b.timers_ws.iter().chain(b.events.iter()).cloned().collect()
}

/// Measured: the timer from `Init(finished)` counts from the second object tick and fires as soon
/// as more than 1000 ms have accumulated (`update_events`, "strictly greater"), i.e. in the 12th
/// object tick; afterwards every 10 object ticks = 20 `step`.
const FIRST_TAX_STEP: usize = 23;
const TAX_PERIOD: usize = 20;

// QUIRK W-243: kept
#[test]
fn tax_follows_the_formula() {
    // re/trading 9.3: floor(level * MaxTax * count / 100) (toward zero) per credit.
    let rules = tax_rules();
    for (level, max_tax, count, want) in [
        (50, 10, 7, 35),
        (50, 3, 3, 4),
        (100, 1, 1, 1),
        (33, 7, 3, 6),
    ] {
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = spawn(&mut sim, TAX_HOUSE, 1, TAX_TILE);
        // `Init(finished)` creates exactly one permanent timer `0x62B` of group 0x80.
        let timers = tax_timers(&sim, house);
        assert_eq!(timers.len(), 1);
        let t = &timers[0];
        assert_eq!((t.msg.code, t.b, t.group()), (msg::COLLECT_TAX, 1000, 0x80));
        assert_ne!(t.flags & crate::module::base::event_flags::REPEAT, 0);
        set_tax(&mut sim, house, level, max_tax, count);
        let start = gold(&sim);
        let case = format!("level {level}, MaxTax {max_tax}, count {count}");
        assert_eq!(
            gold_changes(&mut sim, FIRST_TAX_STEP + 2 * TAX_PERIOD),
            [
                (FIRST_TAX_STEP, want),
                (FIRST_TAX_STEP + TAX_PERIOD, want),
                (FIRST_TAX_STEP + 2 * TAX_PERIOD, want),
            ],
            "{case}"
        );
        assert_eq!(gold(&sim), start + 3 * want, "{case}");
        // Directly `0x62B`: the same formula, return value 1.
        assert_eq!(
            send(&mut sim, house, &Message::new(msg::COLLECT_TAX, &[])),
            1
        );
        assert_eq!(gold(&sim), start + 4 * want, "{case}");
    }
}

// QUIRK W-243: kept
#[test]
fn a_cart_never_pays_tax() {
    // W-243: the cart has no module 3 (`matches_class(0x403)` false) -> -1, nothing booked,
    // even with tax data and a running timer.
    let rules = tax_rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = spawn(&mut sim, CART, 1, (30, 30));
    if let Some(t) = trading_mut(&mut sim, cart) {
        t.payer_guids = vec![PAYER as i32];
        t.tax_interval = 1000;
    }
    set_tax(&mut sim, cart, 50, 10, 7);
    let start = gold(&sim);
    assert_eq!(
        send(&mut sim, cart, &Message::new(msg::COLLECT_TAX, &[])),
        -1
    );
    assert_eq!(send(&mut sim, cart, &Message::new(msg::START_TAX, &[])), 1);
    assert_eq!(tax_timers(&sim, cart).len(), 1);
    assert_eq!(gold_changes(&mut sim, 60), []);
    assert_eq!(gold(&sim), start);
}

// QUIRK W-243: kept
#[test]
fn only_a_finished_building_with_an_owner_pays_tax() {
    // 3.6: construction site (module 3 without flag 0x400) and building without owner player (slot 8) -> -1.
    let rules = tax_rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let site = site(&mut sim, TAX_HOUSE, 1, TAX_TILE);
    assert!(tax_timers(&sim, site).is_empty(), "not finished: no timer");
    set_tax(&mut sim, site, 50, 10, 7);
    let neutral = spawn(&mut sim, TAX_HOUSE, 8, (30, 30));
    set_tax(&mut sim, neutral, 50, 10, 7);
    let start = gold(&sim);
    for id in [site, neutral] {
        assert_eq!(send(&mut sim, id, &Message::new(msg::COLLECT_TAX, &[])), -1);
    }
    assert_eq!(gold(&sim), start);
}

// QUIRK W-243: kept
#[test]
fn zero_interval_starts_no_timer() {
    // 9.3: interval 0 -> neither `Init(finished)` nor `0x72B` creates a timer; `0x62B`
    // itself still works.
    let rules = tax_rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let house = spawn(&mut sim, TAX_HOUSE_NO_TIMER, 1, TAX_TILE);
    assert!(tax_timers(&sim, house).is_empty());
    assert_eq!(send(&mut sim, house, &Message::new(msg::START_TAX, &[])), 1);
    assert!(tax_timers(&sim, house).is_empty());
    set_tax(&mut sim, house, 50, 10, 7);
    assert_eq!(gold_changes(&mut sim, 60), []);
    let start = gold(&sim);
    assert_eq!(
        send(&mut sim, house, &Message::new(msg::COLLECT_TAX, &[])),
        1
    );
    assert_eq!(gold(&sim), start + 35);
}

// QUIRK W-243: kept
#[test]
fn each_start_message_adds_a_timer() {
    // W-243: `Init(finished)` (= handler `0x72B`) and every further `0x72B` each create a
    // permanent timer; they fire in the same tick, the tax comes several times.
    let rules = tax_rules();
    for extra in 0..=2 {
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = spawn(&mut sim, TAX_HOUSE, 1, TAX_TILE);
        for _ in 0..extra {
            assert_eq!(send(&mut sim, house, &Message::new(msg::START_TAX, &[])), 1);
        }
        assert_eq!(tax_timers(&sim, house).len(), 1 + extra);
        set_tax(&mut sim, house, 50, 10, 7);
        let want = 35 * (1 + extra as i32);
        assert_eq!(
            gold_changes(&mut sim, FIRST_TAX_STEP + TAX_PERIOD),
            [(FIRST_TAX_STEP, want), (FIRST_TAX_STEP + TAX_PERIOD, want)],
            "{extra} further 0x72B"
        );
    }
}

// QUIRK W-244: kept
#[test]
fn set_tax_level_is_unchecked() {
    // W-244: `0x82B` sets the level unchecked (return value 1); the tax calculates with it.
    let rules = tax_rules();
    let mut state = state(&rules);
    let mut sim = Sim::new(&mut state, &rules);
    let house = spawn(&mut sim, TAX_HOUSE, 1, TAX_TILE);
    assert_eq!(trading_of(&sim, house).tax_level, START_TAX_LEVEL);
    set_tax(&mut sim, house, 150, 10, 7);
    assert_eq!(trading_of(&sim, house).tax_level, 150);
    let start = gold(&sim);
    assert_eq!(
        send(&mut sim, house, &Message::new(msg::COLLECT_TAX, &[])),
        1
    );
    assert_eq!(gold(&sim), start + 105);
    assert_eq!(
        send(&mut sim, house, &Message::new(msg::SET_TAX_LEVEL, &[-5])),
        1
    );
    assert_eq!(trading_of(&sim, house).tax_level, -5);
}

// QUIRK W-242, O-171: kept
#[test]
fn kinds_follow_accounting_md_trade_cart() {
    // Statistic 11 of the cart (`0x0052E77E`, `0x0052E829`, O-171): delivered **food** (raw,
    // without factor) and loaded gold `min(load capacity, gold)`; delivering wood and gold as well
    // as loading wood or food book nothing (W-242).
    let rules = rules();
    let mut state = trade_state(&rules);
    state.accounting.init_record(1, "Cart", 1, 2, 1);
    let mut sim = Sim::new(&mut state, &rules);
    let cart = trade_world(&mut sim, MIDDLE);
    let trade = crate::accounting::kind::TRADE;
    let cases = [
        // (load, kind, trade ware) -> entries
        ((50, load::FOOD), 2, vec![(trade, 50)]),
        ((70, load::WOOD), 0, vec![(trade, 200)]),
        ((30, load::GOLD), 1, vec![(trade, 200)]),
        ((40, load::FOOD), 3, vec![(trade, 40)]),
        ((0, load::FOOD), 2, vec![]),
    ];
    for ((amount, kind), ware, want) in cases {
        sim.state.accounting.records[1].entries.clear();
        sim.state.players[1].set_resource(0, 1000);
        sim.state.players[1].set_resource(1, 1000);
        sim.state.players[1].set_resource(2, 1000);
        gather::set_carried_load(&mut sim, cart, amount, kind);
        set_trading(&mut sim, cart, ware, 2);
        deliver_and_reload(&mut sim, cart);
        let got: Vec<(i32, i32)> = sim.state.accounting.records[1]
            .entries
            .iter()
            .map(|e| (e.kind, e.parm))
            .collect();
        assert_eq!(got, want, "load {amount}/{kind}, ware {ware}");
    }
}
