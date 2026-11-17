//! Scene with `TENT_GOLDEN` (pattern `upgrade_run_probed`): a dwelling tent of the Prairie
//! Indians is packed, unpacked on another tile, packed again and unpacked on the same tile (W-89);
//! next to it a Spanish trade cart shuttles to the English headquarters (re/trading 9.2), and after
//! two trade cycles slot 1 pays tribute. A real level start with time-of-day upgrades; save/load in
//! the middle of every flow.

use std::sync::LazyLock;

use nml_formats::config::LevelPlayer;

use super::testkit::*;
use super::transport;
use super::{INDEX, Tent, status};
use crate::command::{Command, CommandKind};
use crate::id::ObjectId;
use crate::message::Message;
use crate::module::ModuleKind;
use crate::module::gather::load;
use crate::module::{trading, walk};
use crate::rules::Rules;
use crate::save::testkit::save_and_load;
use crate::setup::{MatchSetup, new_match};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::tick::step;
use crate::world::{ObjectDesc, create_object, desc_flags};

/// Snapped position of the dwelling tent (re/tents 9.2; grid tile 38/38).
const SNAP: (f32, f32) = (5000.0, 5000.0);
/// First unpacking: grid position of the tent plus these whole tiles.
const MOVE: (i32, i32) = (-8, -6);
/// Tile of HQ 1000 (slot 1).
const HOME: (i32, i32) = (20, 20);
/// Docking place of the cart at HQ 1000 in world coordinates, rotation -pi/2 (measured in
/// `trading::unit_tests`, `DOCK`): once docked, the FINDTO success arrives synchronously.
const DOCK: (f32, f32) = (2848.0, 2656.0);
/// Tile of the partner HQ 1900 (slot 2).
const PARTNER: (i32, i32) = (44, 20);
/// Tile of the trading office 1907 (slot 2).
const TRADE_POST_TILE: (i32, i32) = (44, 26);
/// Tribute: slot 1 pays slot 2 (bit 2) 300 wood.
const TRIBUTE_RECIPIENTS: u32 = 0b100;
const TRIBUTE_WOOD: i32 = 300;

/// IDs of the scene (the travois do not appear until packing).
struct Scene {
    tent: ObjectId,
    cart: ObjectId,
}

/// Rules of the scene: the test world of the tents with three human players in the `.lvl`:
/// slot 0 PRI (nation 1), slot 1 SPA (nation 2) with wood 0 (the vectors of re/trading 9.2
/// compute with it), slot 2 ENG (nation 4); otherwise 1000 gold, wood, food each. Time of day 0
/// (default, the counters `4500 + 10k` come anyway).
fn scene_rules() -> Rules {
    let mut r = rules();
    for (slot, nation, wood) in [(0, 1, 1000), (1, 2, 0), (2, 4, 1000)] {
        r.level.players.push(LevelPlayer {
            slot,
            name: format!("P{slot}"),
            nation,
            kind: 0,
            resources: vec![(0, 1000), (1, wood), (2, 1000)],
        });
    }
    r
}

/// Scene via a real level start (`setup::new_match`, mode -1, time-of-day upgrades), map
/// revealed. Slot 0: dwelling tent 1601 (snapped); slot 1: HQ 1000 and the cart 1110
/// docked; slot 2: HQ 1900 and trading office 1907.
fn tent_scene() -> (SimState, Rules, Scene) {
    let rules = scene_rules();
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).expect("level start");
    crate::fog::reveal_map(&mut state, -1);
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
        spawn(&mut sim, SPA_HQ, 1, HOME);
        spawn(&mut sim, ENG_HQ, 2, PARTNER);
        spawn(&mut sim, TRADE_POST, 2, TRADE_POST_TILE);
        let mut desc = ObjectDesc::new(CART, 1, [DOCK.0, 0.0, DOCK.1]);
        desc.rotation = [0.0, -std::f32::consts::FRAC_PI_2, 0.0];
        desc.flags |= desc_flags::PLACED | desc_flags::ROT_Y;
        let cart = create_object(&mut sim, &mut desc).expect("cart created");
        Scene { tent, cart }
    };
    (state, rules, scene)
}

/// Pack the tent and start trading (`0x45` buy wood from slot 2).
const PACK_AT: usize = 0;
/// First unpacking on the tile [`MOVE`] next to it, in the `step` after the end of the fade-out.
const UNPACK_AT: usize = 49;
/// Pack again, in the `step` after the end of the fade-in.
const REPACK_AT: usize = 269;
/// Unpacking on the same tile (W-89), in the `step` after the end of the second fade-out.
const UNPACK_SAME_AT: usize = 319;
/// Tribute, after two full trade cycles and before the third delivery.
const TRIBUTE_AT: usize = 1200;
/// Length of the scene in `step` calls (two make one object tick).
const TENT_STEPS: usize = 3000;

/// Measured `step` calls of the scene (debug = release): end of the fade-out, arrival of the
/// travois (`0x223` -> fade-in), end of the fade-in (`0x2F03`), the same for the second
/// cycle, and the phase-0 steps in which the travois are removed (type counter -1, -2).
/// The crossfade takes 25 object ticks; its first tick falls into the command step.
const PACKED: usize = 48;
const ARRIVED: usize = 219;
const PLACED: usize = 268;
const FIRST_TRAVOIS_GONE: usize = 269;
const REPACKED: usize = 318;
const ARRIVED_SAME: usize = 346;
const PLACED_SAME: usize = 396;
const SECOND_TRAVOIS_GONE: usize = 397;
/// Trade (measured): arrival at the partner (wood loaded) and at home (delivered, gold loaded) per
/// cycle.
const AT_PARTNER_1: usize = 288;
const AT_HOME_1: usize = 578;
const AT_PARTNER_2: usize = 868;
const AT_HOME_2: usize = 1158;
const AT_PARTNER_3: usize = 1448;
const AT_HOME_3: usize = 1738;

fn tent_of(state: &SimState, id: ObjectId) -> Tent {
    let o = state.object(id, false).expect("tent");
    match &o.module(INDEX).expect("module 42").kind {
        ModuleKind::Tent(t) => t.clone(),
        k => panic!("not a tent: {k:?}"),
    }
}

/// Grid position `(x, z)` of the object plus `(dx, dz)` whole tiles.
fn grid_plus(state: &SimState, id: ObjectId, (dx, dz): (i32, i32)) -> (i32, i32) {
    let g = state.object(id, false).expect("object").grid;
    (g.x.0 + (dx << 12), g.z.0 + (dz << 12))
}

/// Destination of the first unpacking: the original grid position of the tent plus [`MOVE`].
fn first_target(state: &SimState, s: &Scene) -> (i32, i32) {
    grid_plus(state, s.tent, MOVE)
}

/// Commands in the `step` call `i` (not object tick: two `step`s make one tick). The travois of
/// the unpacking is the most recently created one of the tent (`m_TravoisHID`).
fn tent_orders(i: usize, s: &Scene, state: &SimState) -> Vec<Command> {
    let cmd = |player, kind| Command { player, kind };
    let unpack = |d| {
        let (x, z) = grid_plus(state, s.tent, d);
        let unit = tent_of(state, s.tent).travois;
        cmd(0, CommandKind::UnpackTent { unit, x, z })
    };
    match i {
        PACK_AT => vec![
            cmd(
                1,
                CommandKind::TradeWithPlayer {
                    ware: 0,
                    target: 2,
                    units: vec![s.cart],
                },
            ),
            cmd(0, CommandKind::PackTent { tent: s.tent }),
        ],
        UNPACK_AT => vec![unpack(MOVE)],
        REPACK_AT => vec![cmd(0, CommandKind::PackTent { tent: s.tent })],
        UNPACK_SAME_AT => vec![unpack((0, 0))],
        TRIBUTE_AT => vec![cmd(
            1,
            CommandKind::Tribute {
                from: 1,
                recipients: TRIBUTE_RECIPIENTS,
                resource: 1,
                amount: TRIBUTE_WOOD,
            },
        )],
        _ => vec![],
    }
}

/// State assertion at the save point: state, rules, IDs of the scene.
type TentProbe = fn(&mut SimState, &Rules, &Scene);

/// Checksum per `step` call of the scene. With `save_at = Some(i)` it saves after the `step` call
/// `i`, checks against the rules (like the viewer at loading) and reloads; `at_save` sees the
/// state immediately before.
fn tent_run_probed(save_at: Option<usize>, at_save: TentProbe) -> Vec<u64> {
    let (mut state, rules, scene) = tent_scene();
    let mut out = Vec::with_capacity(TENT_STEPS);
    for i in 0..TENT_STEPS {
        let orders = tent_orders(i, &scene, &state);
        step(&mut state, &rules, &orders);
        if save_at == Some(i) {
            at_save(&mut state, &rules, &scene);
            assert_loads(&state, &rules);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

/// Read the savegame and check it against the rules (like the viewer at loading; copy from
/// `upgrades::tests`, private there).
fn assert_loads(state: &SimState, rules: &Rules) {
    let bytes = crate::save::testkit::save_bytes(state, rules);
    crate::save::SaveGame::from_bytes(&bytes)
        .expect("savegame readable")
        .check(rules)
        .expect("savegame valid");
}

/// Is `id` in a bucket of the collision manager (all or movers)?
fn in_collision(state: &SimState, id: ObjectId) -> bool {
    let c = &state.collision;
    c.all.iter().chain(c.movers.iter()).any(|l| l.contains(&id))
}

fn travois_count(state: &SimState) -> i32 {
    state.guid_counts[0].get(&TRAVOIS).copied().unwrap_or(0)
}

/// Load of the cart raw (`+0x2A8`, `+0x2AC`).
fn load_of(state: &SimState, cart: ObjectId) -> (i32, i32) {
    let o = state.object(cart, false).expect("cart");
    (o.load_amount, o.load_kind)
}

/// Gold and wood of a slot.
fn account(state: &SimState, slot: usize) -> (i32, i32) {
    (state.resource(slot, 0), state.resource(slot, 1))
}

/// Success message that walk holds for the running trip.
fn walk_success(state: &SimState, id: ObjectId) -> Option<Message> {
    let o = state.object(id, false)?;
    o.module(walk::INDEX)?.base.success.clone()
}

/// Expected tent status in the `step` call `i`.
fn expected_status(i: usize) -> i32 {
    match i {
        _ if i < PACKED => status::FADE_OUT,
        _ if i < ARRIVED => status::REST,
        _ if i < PLACED => status::FADE_IN,
        _ if i < REPACK_AT => status::REST,
        _ if i < REPACKED => status::FADE_OUT,
        _ if i < ARRIVED_SAME => status::REST,
        _ if i < PLACED_SAME => status::FADE_IN,
        _ => status::REST,
    }
}

#[test]
fn the_tent_scene_shows_every_state() {
    // Counter-check before the golden checksums: tent after the first unpacking on the new tile,
    // in the collision manager; after the second on the same tile not (W-89); type counter 1717 =
    // -2 not until the phase-0 step after the second `0x2F03`; trade balances after two cycles as
    // in re/trading 9.2 (gold 400, wood 1575, price 1087) and afterwards wood -300 for slot 1,
    // +300 for slot 2. Times are `step` calls.
    let (mut state, rules, s) = tent_scene();
    let home_tile = state.object(s.tent, false).unwrap().grid;
    let target = first_target(&state, &s);
    assert_eq!(
        (home_tile.x.0 >> 12, home_tile.z.0 >> 12),
        (38, 38),
        "snapped"
    );
    assert!(in_collision(&state, s.tent));
    assert_eq!(account(&state, 1), (1000, 0));
    assert_eq!(account(&state, 2), (1000, 1000));
    assert_eq!(state.economy.wood_price, 1024);
    let mut travois = Vec::new();
    for i in 0..TENT_STEPS {
        let orders = tent_orders(i, &s, &state);
        if i == UNPACK_AT || i == UNPACK_SAME_AT {
            // Unpacking hits the tent just packed: fade-out finished, hidden.
            let t = tent_of(&state, s.tent);
            assert_eq!(t.status, status::REST, "step {i}");
            assert!(!state.object(s.tent, false).unwrap().selectable);
            travois.push(t.travois);
        }
        let events = step(&mut state, &rules, &orders);
        let t = tent_of(&state, s.tent);
        let o = state.object(s.tent, false).unwrap();
        assert_eq!(t.status, expected_status(i), "step {i}");
        // Position: the snapped one until the first arrival, then the target - also after the
        // second unpacking on the same tile.
        let pos = (o.grid.x.0, o.grid.z.0);
        if i < ARRIVED {
            assert_eq!(pos, (home_tile.x.0, home_tile.z.0), "step {i}");
        } else {
            assert_eq!(pos, target, "step {i}");
        }
        // Collision manager: packing removes it, the first unpacking (tile change) registers it
        // again, the second on the same tile does not (W-89).
        let collides = (ARRIVED..REPACK_AT).contains(&i);
        assert_eq!(in_collision(&state, s.tent), collides, "step {i}");
        // Selectable when it rests and stands.
        let standing = i >= PLACED && !(REPACK_AT..PLACED_SAME).contains(&i);
        assert_eq!(o.selectable, standing, "step {i}");
        // Type counter 1717: never counted without "finished", -1 per removed travois (W-248).
        let gone = match i {
            _ if i < FIRST_TRAVOIS_GONE => 0,
            _ if i < SECOND_TRAVOIS_GONE => -1,
            _ => -2,
        };
        assert_eq!(travois_count(&state), gone, "step {i}");
        // Trade: account, load and price at the measured turning points.
        let trade = (account(&state, 1), load_of(&state, s.cart));
        match i {
            PACK_AT => assert_eq!(trade, ((800, 0), (200, load::GOLD))),
            AT_PARTNER_1 => {
                assert_eq!(trade, ((800, 0), (800, load::WOOD)));
                assert_eq!(state.economy.wood_price, 1056);
            }
            AT_HOME_1 => assert_eq!(trade, ((600, 800), (200, load::GOLD))),
            AT_PARTNER_2 => {
                assert_eq!(trade, ((600, 800), (775, load::WOOD)));
                assert_eq!(state.economy.wood_price, 1087);
            }
            AT_HOME_2 => assert_eq!(trade, ((400, 1575), (200, load::GOLD))),
            _ => {}
        }
        if i == TRIBUTE_AT - 1 {
            assert_eq!(
                account(&state, 1),
                (400, 1575),
                "two cycles (re/trading 9.2)"
            );
            assert_eq!(
                account(&state, 2),
                (1000, 1000),
                "the partner receives nothing"
            );
            assert_eq!(state.economy.wood_price, 1087);
        }
        let tributes: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, SimEvent::TributeSent { .. }))
            .cloned()
            .collect();
        if i == TRIBUTE_AT {
            assert_eq!(
                tributes,
                [SimEvent::TributeSent {
                    giver: 1,
                    recipient: 2
                }]
            );
            assert_eq!(account(&state, 1), (400, 1575 - TRIBUTE_WOOD));
            assert_eq!(account(&state, 2), (1000, 1000 + TRIBUTE_WOOD));
        } else {
            assert_eq!(tributes, [], "step {i}");
        }
        if i == AT_PARTNER_3 {
            assert_eq!(trade, ((400, 1275), (753, load::WOOD)));
        }
        if i == AT_HOME_3 {
            assert_eq!(trade, ((200, 1275 + 753), (200, load::GOLD)));
            assert_eq!(account(&state, 2), (1000, 1300));
        }
    }
    // Two different travois, both removed; the tent remembers the second.
    assert_eq!(travois.len(), 2);
    assert_ne!(travois[0], travois[1]);
    assert!(travois.iter().all(|&t| state.object(t, false).is_none()));
    assert_eq!(tent_of(&state, s.tent).travois, travois[1]);
    // At the end the tent stands visible on the target tile, outside the collision manager, and the
    // target search does not find it (W-89).
    let o = state.object(s.tent, false).unwrap();
    assert!(o.selectable && o.render_bits != 0);
    let filter = nml_formats::targets::MatchFilter {
        mode: 0x10 | 0x8,
        guid: DWELLING_TENT as i32,
        ..nml_formats::targets::MatchFilter::default()
    };
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(
        crate::collision::find_nearest_matching(&mut sim, target, &filter, -1, 0, -1),
        None
    );
}

/// Measured (packing and moving a tent, trade cycles, tribute).
/// Changed: crossfade with two f32 roundings (24-bit x87).
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
const TENT_GOLDEN: u64 = 0x278d_a461_d627_2f0f;

/// Continuous sequence of the scene without saving, computed once per test process and shared by
/// the save tests.
static TENT_SEQUENCE: LazyLock<Vec<u64>> = LazyLock::new(|| tent_run_probed(None, |_, _, _| {}));

#[test]
fn tent_checksums_match_the_golden_sequence() {
    let mut h = crate::rules::Fnv64::default();
    for c in TENT_SEQUENCE.iter() {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, TENT_GOLDEN, "{:#018x}", h.0);
}

/// Save points of the scene (measured): in the middle of the fade-out, while the travois drives to
/// the destination, in the middle of the fade-in (tile change), in the middle of the second
/// fade-in on the same tile (W-89), cart with gold on the way to the partner, after the tribute.
const SAVE_FADING_OUT: usize = 20;
const SAVE_TRAVOIS_WALKING: usize = 120;
const SAVE_FADING_IN: usize = 240;
const SAVE_FADING_IN_SAME_TILE: usize = 370;
const SAVE_CART_TO_PARTNER: usize = 700;
const SAVE_AFTER_TRIBUTE: usize = 1300;

/// Scene with save/load after `save_at` against the continuous sequence.
fn assert_survives_saving(save_at: usize, at_save: TentProbe, what: &str) {
    let saved = tent_run_probed(Some(save_at), at_save);
    let first_diff = saved
        .iter()
        .zip(TENT_SEQUENCE.iter())
        .position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "saved {what}: first deviation in `step`");
}

#[test]
fn tent_checksums_survive_saving_and_loading() {
    // One run per save point; the runs go side by side (run time, like the separate tests of
    // `UPGRADE_GOLDEN`). Every sequence equals the continuous one.
    let cases: [(usize, TentProbe, &str); 6] = [
        (
            SAVE_FADING_OUT,
            |state, _, s| {
                let t = tent_of(state, s.tent);
                assert_eq!(t.status, status::FADE_OUT);
                assert!((1..super::FADE_MS).contains(&t.counter), "{}", t.counter);
                let flags = state
                    .object(t.travois, false)
                    .and_then(|o| o.module(transport::INDEX))
                    .map_or(0, |m| m.base.flags);
                assert_ne!(flags & transport::TRANSPORTING_TENT, 0);
            },
            "in the middle of the fade-out",
        ),
        (
            SAVE_TRAVOIS_WALKING,
            |state, _, s| {
                let t = tent_of(state, s.tent);
                assert_eq!(t.status, status::REST);
                assert!(!state.object(s.tent, false).unwrap().selectable);
                let (x, z) = first_target(state, s);
                let arrived = Message::new(transport::msg::ARRIVED, &[x, z]);
                assert_eq!(walk_success(state, t.travois), Some(arrived));
            },
            "while the travois drives",
        ),
        (
            SAVE_FADING_IN,
            |state, _, s| {
                let t = tent_of(state, s.tent);
                assert_eq!(t.status, status::FADE_IN);
                assert!((1..super::FADE_MS).contains(&t.counter), "{}", t.counter);
                assert!(in_collision(state, s.tent));
                let travois = state.object(t.travois, false).expect("travois still there");
                assert!(!travois.selectable);
            },
            "in the middle of the fade-in",
        ),
        (
            SAVE_FADING_IN_SAME_TILE,
            |state, _, s| {
                let t = tent_of(state, s.tent);
                assert_eq!(t.status, status::FADE_IN);
                assert!(!in_collision(state, s.tent), "W-89");
                assert_eq!(travois_count(state), -1);
            },
            "in the middle of the fade-in on the same tile",
        ),
        (
            SAVE_CART_TO_PARTNER,
            |state, _, s| {
                assert_eq!(load_of(state, s.cart), (200, load::GOLD));
                let code = walk_success(state, s.cart).map(|m| m.code);
                assert_eq!(code, Some(trading::msg::AT_PARTNER));
            },
            "cart loaded, on the way to the partner",
        ),
        (
            SAVE_AFTER_TRIBUTE,
            |state, _, s| {
                assert_eq!(account(state, 1), (400, 1575 - TRIBUTE_WOOD));
                assert_eq!(account(state, 2), (1000, 1000 + TRIBUTE_WOOD));
                assert_eq!(travois_count(state), -2);
                assert!(!in_collision(state, s.tent));
            },
            "after the tribute",
        ),
    ];
    std::thread::scope(|scope| {
        let runs: Vec<_> = cases
            .into_iter()
            .map(|(save_at, at_save, what)| {
                scope.spawn(move || assert_survives_saving(save_at, at_save, what))
            })
            .collect();
        for run in runs {
            if let Err(panic) = run.join() {
                std::panic::resume_unwind(panic);
            }
        }
    });
}
