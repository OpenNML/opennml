//! Scene with `STEALTH_GOLDEN` (pattern `fort_run_probed`; re/masking 9, re/healing 9): slot 0 camouflages
//! an archer 1402 (2924) and uncamouflages him again; ranger 1410 and alarm post 1610 camouflage
//! themselves. The assassin 1407 (2922) digs in, stabs two colonists of slot 1 one after the other and
//! digs out again. Pilgrim 2014 (3125, injured) and medic 2303 go into the healing trance, the medic heals an
//! injured female farmer 2302, the pilgrim heals himself; the pilgrim aborts, a walk command in
//! the teardown phase is lost (K-53), finally he heals himself via `0x60`. The finished
//! monastery 1015 heals a saber fighter 1107; the shipyard 1008 does not heal its ship 1108 because an
//! injured soldier stands in front of it in the query (C-242). A third colonist of slot 1 walks up to
//! the camouflaged ranger. A real level start without original data; saving/loading in
//! every ability.
//!
//! The archers of the test world have attack module 17 without melee and ranged combat (flags 0,
//! `ability_testkit`): they search no targets (`search_bands` -1), so they do not shoot at the colonists;
//! the colonists have no attack module. Slot 1 has no unit with
//! `CANCLEARUP`, so the camouflage energy loses nothing through scouting.

use std::sync::LazyLock;

use nml_formats::config::LevelPlayer;
use nml_formats::targets::MatchFilter;

use super::ability_testkit::{self as kit, *};
use super::assassin::msg as assassin_msg;
use super::healing::msg as healing_msg;
use super::masking::msg as masking_msg;
use super::{ModuleKind, assassin, healing, masking};
use crate::command::{Command, CommandKind, NO_SPREAD};
use crate::filter::matches_target;
use crate::id::{NEUTRAL_SLOT, ObjectId};
use crate::message::MessageFilter;
use crate::object::status;
use crate::rules::Rules;
use crate::save::testkit::save_and_load;
use crate::setup::{MatchSetup, new_match};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::tick::step;

/// Setup (tiles). Slot 0: the ability carriers, each group far enough apart that
/// no healing reaches a foreign group (monastery and shipyard 7.5 tiles, single healers
/// 2.5 tiles + radius). Slot 1: two colonists next to the assassin (the first one tile away,
/// edge distance 3584 <= 4608; the second four tiles away), a third west of the ranger.
const ARCHER_TILE: (i32, i32) = (10, 10);
const RANGER_TILE: (i32, i32) = (10, 22);
const POST_TILE: (i32, i32) = (8, 44);
const ASSASSIN_TILE: (i32, i32) = (40, 10);
const VICTIM_TILES: [(i32, i32); 2] = [(41, 10), (44, 10)];
/// The second colonist walks next to the assassin on the west side (edge distance like the first).
const SECOND_VICTIM_TARGET: (i32, i32) = (39, 10);
const WALKER_TILE: (i32, i32) = (20, 22);
/// The third colonist walks to a tile next to the ranger.
const WALKER_TARGET: (i32, i32) = (11, 22);
const PILGRIM_TILE: (i32, i32) = (56, 10);
/// Destination of the walk command in the teardown phase (never reached).
const PILGRIM_TARGET: (i32, i32) = (56, 16);
const MEDIC_TILE: (i32, i32) = (56, 24);
const FARMER_TILE: (i32, i32) = (58, 24);
const MONASTERY_TILE: (i32, i32) = (20, 36);
/// Seven tiles from the monastery (re/healing 9.1).
const SABRE_TILE: (i32, i32) = (27, 36);
/// Shipyard, soldier and ship as in `a_soldier_first_in_the_query_stops_the_shipyard`: the soldier in the
/// bucket of the shipyard (tiles 16-23), the ship in the first ring.
const SHIPYARD_TILE: (i32, i32) = (20, 52);
const SOLDIER_TILE: (i32, i32) = (22, 52);
const SHIP_TILE: (i32, i32) = (26, 52);

/// Command times as `step` calls (not object ticks: two `step` calls are one tick); the command
/// runs in the `step` call with this index.
///
/// `0x1D` camouflage 1402; `0x20` dig in 1407; `0x34` start healing 2014 and 2303.
const START_AT: usize = 0;
/// Slot 1: the second colonist in range, after the first stab ([`FIRST_STAB`]).
const SECOND_IN_RANGE_AT: usize = 80;
/// Slot 1: the third colonist to the ranger.
const WALKER_AT: usize = 200;
/// `0x1E` uncamouflage 1402 (K-79: pseudo ID 1 is here the archer himself, index 0,
/// slot 0, generation 1; the second `0x219` has no effect); `0x35` stop healing 2014.
const STOP_AT: usize = 600;
/// Walk command to 2014 in the teardown animation (between [`STOP_AT`] and [`TRANCE_OVER`]).
const LOST_WALK_AT: usize = 620;
/// `0x21` dig out 1407 and `0x60` self-healing 2014, after the end of the trance; the mana is
/// not enough yet (cost 50, value set of the test world), the self-healing has no effect.
const DIG_OUT_AT: usize = 700;
/// `0x60` self-healing 2014 once more when the mana has regenerated (+10 fraction per tick).
const SELF_HEAL_AT: usize = 1800;
/// Length of the scene in `step` calls.
const STEALTH_STEPS: usize = 3000;
const _: () = assert!(STOP_AT < LOST_WALK_AT && LOST_WALK_AT < TRANCE_OVER);
const _: () = assert!(TRANCE_OVER < DIG_OUT_AT && FIRST_STAB < SECOND_IN_RANGE_AT);

/// Measured `step` calls (debug = release) after which the state holds for the first time.
///
/// Camouflage: the archer after `m_MaskTime` 1600 ms (17 ticks, `0x319` in N + 17), the
/// ranger by itself in the first tick (`0x319` from the tick in N + 18), the alarm post immediately.
const ARCHER_MASKED: usize = 34;
const RANGER_MASKED: usize = 37;
/// Uncamouflage animation from [`STOP_AT`], visible after `m_MaskTime` (K-304).
const ARCHER_VISIBLE: usize = 634;
/// Dig in: `MASKED | WAITING4KILLING` as soon as `0x11C` from the FIFO takes effect (the assassin is in
/// the second tick group, creation order; his FIFO does not run until the second `step`),
/// `m_IsDown` after 1843 ms (19 ticks), first search 500 ms later (6 ticks).
const DUG_IN: usize = 1;
const ASSASSIN_DOWN: usize = 39;
const FIRST_TARGET: usize = 51;
/// Stab `0x61C` (marker 2, 640 ms, 7 ticks), the colonist (another tick group) is dead two `step`
/// calls later; `0x71C` (marker 1, 1180 ms, 12 ticks) camouflages again.
const FIRST_STAB: usize = 65;
const FIRST_DEAD: usize = 67;
const FIRST_REST: usize = 75;
/// Next search after the pause of 5 s (`0x31C` from `0x71C`, 51 ticks); stab and death of the
/// second colonist (tick group of the assassin, created after him) in the same `step`; pause.
const SECOND_TARGET: usize = 177;
const SECOND_DEAD: usize = 191;
const SECOND_REST: usize = 201;
/// Dig out: lock bit in the `step` after the command, dig-out animation 773 ms.
const DUG_OUT: usize = 701;
const GOT_UP: usize = 717;
/// The third colonist next to the ranger.
const WALKER_ARRIVED: usize = 318;
// The measured distances follow the timer rule (N + floor(d/100) + 1 ticks, one tick = 2 `step`).
const _: () = assert!(ARCHER_MASKED - START_AT == 2 * 17);
const _: () = assert!(ASSASSIN_DOWN - DUG_IN == 2 * 19 && FIRST_TARGET - ASSASSIN_DOWN == 2 * 6);
const _: () = assert!(FIRST_STAB - FIRST_TARGET == 2 * 7 && FIRST_REST - FIRST_TARGET == 2 * 12);
const _: () = assert!(SECOND_TARGET - FIRST_REST == 2 * 51);
const _: () = assert!(ARCHER_VISIBLE - STOP_AT == 2 * 17 && GOT_UP - DUG_OUT == 2 * 8);
const _: () =
    assert!(SECOND_DEAD - SECOND_TARGET == 2 * 7 && SECOND_REST - SECOND_TARGET == 2 * 12);
const _: () = assert!(PILGRIM_READY - START_AT == 2 * 40 && TRANCE_OVER - STOP_AT == 2 * 40);
const _: () = assert!(MEDIC_READY - MEDIC_TRANCE == 2 * 46);
/// Healing trance of the pilgrim: `0x11D` in the first `step`, ready after 3999 ms (40 ticks), first
/// healing tick (on himself, C-245) in the next tick; after `0x35` teardown 40 ticks.
const PILGRIM_READY: usize = 80;
const PILGRIM_HEALS: usize = 82;
const TRANCE_OVER: usize = 680;
/// Healing trance of the medic (another tick group): `0x11D` in the second `step`, ready after
/// 4500 ms (46 ticks), first healing tick on the female farmer; full and given up.
const MEDIC_TRANCE: usize = 1;
const MEDIC_READY: usize = 93;
const MEDIC_HEALS: usize = 95;
const FARMER_FULL: usize = 1033;
const FARMER_DROPPED: usize = 1035;
/// The finished healing buildings become ready in their first tick (C-249) and heal from the next;
/// the monastery heals the saber fighter fully.
const MONASTERY_READY: usize = 1;
const SHIPYARD_READY: usize = 1;
const SABRE_FULL: usize = 639;

/// IDs of the scene.
struct StealthScene {
    archer: ObjectId,
    ranger: ObjectId,
    post: ObjectId,
    assassin: ObjectId,
    victims: [ObjectId; 2],
    walker: ObjectId,
    pilgrim: ObjectId,
    medic: ObjectId,
    farmer: ObjectId,
    monastery: ObjectId,
    sabre: ObjectId,
    shipyard: ObjectId,
    soldier: ObjectId,
    ship: ObjectId,
}

/// Rules of the scene: the test world of the abilities ([`kit::rules`], variant rules bit 9/10) with
/// two human players in the `.lvl` ([`scene_rules_from`]).
pub(super) fn scene_rules() -> Rules {
    scene_rules_from(kit::rules())
}

/// `r` with two human players in the `.lvl` (slot 0 and 1), 1000 gold, wood, food each -
/// basis of the golden scenes with `setup::new_match` (also `evoke_tests`).
pub(super) fn scene_rules_from(mut r: Rules) -> Rules {
    for slot in [0, 1] {
        r.level.players.push(LevelPlayer {
            slot,
            name: format!("P{slot}"),
            nation: 2,
            kind: 0,
            resources: vec![(0, 1000), (1, 1000), (2, 1000)],
        });
    }
    r
}

/// Hit points and extra energy (mana).
const HP: u32 = 14;
const MANA: u32 = 15;
/// Fraction of a colonist (160 HP) after a stab of 250.
const STABBED: i32 = (160 - 250) * 100_000 / 160;
/// Effect of the self-healing (`command::ability`).
const SELF_HEAL_EFFECT: &str = "data/effects/selbst_heilen.pef";

fn wound(sim: &mut Sim, id: ObjectId) {
    sim.object_mut(id)
        .unwrap()
        .attributes
        .set_current_fraction(HP, 50_000);
}

/// Scene via a real level start (`setup::new_match`, mode -1), map revealed; slot 0 and
/// 1 are enemies of each other. Upgrades 2924, 2922 and 3125 are unlocked **before** creating
/// (archer and assassin come into being with module 25 and 28 switched on).
/// Pilgrim, female farmer, saber fighter, soldier and ship start with half hit points.
fn stealth_scene() -> (SimState, Rules, StealthScene) {
    let rules = scene_rules();
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).expect("level start");
    crate::fog::reveal_map(&mut state, -1);
    assert!(state.diplomacy.is_enemy(0, 1) && state.diplomacy.is_enemy(1, 0));
    for guid in [MASK_UPGRADE, DIG_IN_UPGRADE, HEAL_UPGRADE] {
        unlock(&mut state, 0, guid);
    }
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        let archer = spawn(&mut sim, ARCHER, 0, ARCHER_TILE);
        let ranger = spawn(&mut sim, RANGER, 0, RANGER_TILE);
        let post = spawn(&mut sim, ALARM_POST, 0, POST_TILE);
        let assassin = spawn(&mut sim, ASSASSIN, 0, ASSASSIN_TILE);
        let pilgrim = spawn(&mut sim, PILGRIM, 0, PILGRIM_TILE);
        let medic = spawn(&mut sim, MEDIC, 0, MEDIC_TILE);
        let farmer = spawn(&mut sim, FARMER, 0, FARMER_TILE);
        let monastery = spawn(&mut sim, MONASTERY, 0, MONASTERY_TILE);
        let sabre = spawn(&mut sim, SABRE, 0, SABRE_TILE);
        let shipyard = spawn(&mut sim, SHIPYARD, 0, SHIPYARD_TILE);
        let soldier = spawn(&mut sim, SABRE, 0, SOLDIER_TILE);
        let ship = spawn(&mut sim, SHIP, 0, SHIP_TILE);
        let victims = VICTIM_TILES.map(|t| spawn(&mut sim, COLONIST, 1, t));
        let walker = spawn(&mut sim, COLONIST, 1, WALKER_TILE);
        for id in [pilgrim, farmer, sabre, soldier, ship] {
            wound(&mut sim, id);
        }
        StealthScene {
            archer,
            ranger,
            post,
            assassin,
            victims,
            walker,
            pilgrim,
            medic,
            farmer,
            monastery,
            sabre,
            shipyard,
            soldier,
            ship,
        }
    };
    (state, rules, scene)
}

fn move_to(units: Vec<ObjectId>, t: (i32, i32)) -> CommandKind {
    CommandKind::Move {
        units,
        x: (t.0 << 12) + 0x800,
        z: (t.1 << 12) + 0x800,
        target: -1,
        formation: 0,
        direction: crate::motion::NO_DIRECTION,
        spread: NO_SPREAD,
        flags: 0,
    }
}

/// Commands in the `step` call `i`.
fn stealth_orders(i: usize, s: &StealthScene) -> Vec<Command> {
    let cmd = |player, kind| Command { player, kind };
    let self_heal = || CommandKind::SelfHeal {
        unit: s.pilgrim,
        button: crate::command::ability::SELF_HEAL_BUTTON,
    };
    match i {
        START_AT => vec![
            cmd(
                0,
                CommandKind::Mask {
                    units: vec![s.archer],
                },
            ),
            cmd(
                0,
                CommandKind::DigIn {
                    units: vec![s.assassin],
                },
            ),
            cmd(
                0,
                CommandKind::StartHealing {
                    units: vec![s.pilgrim, s.medic],
                },
            ),
        ],
        SECOND_IN_RANGE_AT => vec![cmd(1, move_to(vec![s.victims[1]], SECOND_VICTIM_TARGET))],
        WALKER_AT => vec![cmd(1, move_to(vec![s.walker], WALKER_TARGET))],
        STOP_AT => vec![
            cmd(
                0,
                CommandKind::Unmask {
                    units: vec![s.archer],
                },
            ),
            cmd(
                0,
                CommandKind::StopHealing {
                    units: vec![s.pilgrim],
                },
            ),
        ],
        LOST_WALK_AT => vec![cmd(0, move_to(vec![s.pilgrim], PILGRIM_TARGET))],
        DIG_OUT_AT => vec![
            cmd(
                0,
                CommandKind::DigOut {
                    units: vec![s.assassin],
                },
            ),
            cmd(0, self_heal()),
        ],
        SELF_HEAL_AT => vec![cmd(0, self_heal())],
        _ => vec![],
    }
}

/// State assertion at the time of saving: state, rules, IDs of the scene.
type StealthProbe = fn(&mut SimState, &Rules, &StealthScene);

/// Checksum per `step` call of the scene. With `save_at = Some(i)` it is saved after the `step` call `i`,
/// checked against the rules (like the viewer when loading) and loaded again; `at_save`
/// sees the state immediately before.
fn stealth_run_probed(save_at: Option<usize>, at_save: StealthProbe) -> Vec<u64> {
    let (mut state, rules, scene) = stealth_scene();
    let mut out = Vec::with_capacity(STEALTH_STEPS);
    for i in 0..STEALTH_STEPS {
        let orders = stealth_orders(i, &scene);
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

/// Read a savegame and check it against the rules (like the viewer when loading; copy of
/// `fort_tests`, private there; also `evoke_tests`).
pub(super) fn assert_loads(state: &SimState, rules: &Rules) {
    let bytes = crate::save::testkit::save_bytes(state, rules);
    crate::save::SaveGame::from_bytes(&bytes)
        .expect("savegame readable")
        .check(rules)
        .expect("savegame valid");
}

fn st(state: &SimState, id: ObjectId) -> u32 {
    state.object(id, false).expect("object").status
}

fn has(state: &SimState, id: ObjectId, bits: u32) -> bool {
    st(state, id) & bits == bits
}

/// Fraction of the attribute (unclamped).
fn fraction(state: &SimState, id: ObjectId, attr: u32) -> i32 {
    state
        .object(id, false)
        .expect("object")
        .attributes
        .raw_fraction(attr)
}

/// Mana as a value (like `GetCurrentValue(0xF)`, rounded; without upgrades 100 %).
fn mana_value(state: &mut SimState, rules: &Rules, id: ObjectId) -> i32 {
    Sim::new(state, rules).attribute(id, MANA)
}

fn is_full(state: &SimState, id: ObjectId) -> bool {
    state
        .object(id, false)
        .expect("object")
        .attributes
        .is_full(HP)
}

fn tile(state: &SimState, id: ObjectId) -> (i32, i32) {
    state.object(id, false).expect("object").tile
}

/// (`m_IsMasked`, `m_IsBusy`).
fn mask_flags(state: &SimState, id: ObjectId) -> (bool, bool) {
    match &state
        .object(id, false)
        .unwrap()
        .module(masking::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::Masking(m) => (m.is_masked, m.is_busy),
        k => panic!("no camouflage module: {k:?}"),
    }
}

/// (`m_IsDown`, `m_TargetHID`).
fn dig_state(state: &SimState, id: ObjectId) -> (bool, ObjectId) {
    match &state
        .object(id, false)
        .unwrap()
        .module(assassin::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::Assassin(a) => (a.is_down, a.target),
        k => panic!("no assassin module: {k:?}"),
    }
}

/// (`m_ReadyToHeal`, `m_SickPersonHID`).
fn heal_state(state: &SimState, id: ObjectId) -> (bool, ObjectId) {
    match &state
        .object(id, false)
        .unwrap()
        .module(healing::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::Healing(h) => (h.ready, h.sick),
        k => panic!("no healing module: {k:?}"),
    }
}

/// Pending timers `code` of module `index` (list and `m_TimersWS`).
fn timers(state: &SimState, id: ObjectId, index: usize, code: u32) -> usize {
    let b = &state.object(id, false).unwrap().module(index).unwrap().base;
    b.events
        .iter()
        .chain(b.timers_ws.iter())
        .filter(|e| e.msg.code == code)
        .count()
}

fn filter_of(state: &SimState, id: ObjectId) -> MessageFilter {
    state.object(id, false).unwrap().filter.clone()
}

fn filter(mode: u8, codes: &[u32]) -> MessageFilter {
    MessageFilter {
        mode,
        codes: codes.to_vec(),
    }
}

/// Carcasses of the colonists in slot 8 (`StartDying` changes the owner, C-142).
fn colonist_corpses(state: &SimState) -> Vec<ObjectId> {
    state
        .objects()
        .filter(|o| o.guid == COLONIST && o.owner() == NEUTRAL_SLOT && o.status & status::DIED != 0)
        .map(|o| o.id)
        .collect()
}

/// Is `id` a target for slot 1 (`MM_CHECKFOG` on the head filter, player and fog 1)?
fn seen_by_1(state: &mut SimState, rules: &Rules, id: ObjectId) -> bool {
    let fog = MatchFilter {
        mode: 0x200_0000,
        ..MatchFilter::constructed()
    };
    let mut sim = Sim::new(state, rules);
    matches_target(&mut sim, id, &fog, 1, 1)
}

/// Self-healing effect on the pilgrim among the events?
fn self_healed(events: &[SimEvent], pilgrim: ObjectId) -> bool {
    events.iter().any(|e| {
        matches!(e, SimEvent::EffectOnObject { object, path } if *object == pilgrim && path == SELF_HEAL_EFFECT)
    })
}

/// Expected dig-in status `MASKED | WAITING4KILLING | 0x800000` of the assassin after `step` `i`.
fn expected_dig_status(i: usize) -> u32 {
    let dug = status::MASKED | status::WAITING4KILLING;
    match i {
        _ if i < DUG_IN => 0,
        _ if i < FIRST_TARGET => dug,
        _ if i < FIRST_REST => status::WAITING4KILLING,
        _ if i < SECOND_TARGET => dug,
        _ if i < SECOND_REST => status::WAITING4KILLING,
        _ if i < DUG_OUT => dug,
        _ if i < GOT_UP => status::ANIM_LOCKED,
        _ => 0,
    }
}

/// Expected message filter of the pilgrim after `step` `i` (C-247, K-53).
fn expected_pilgrim_filter(i: usize) -> MessageFilter {
    match i {
        _ if i < STOP_AT => filter(0, &[healing_msg::END_REQUEST]),
        _ if i < TRANCE_OVER => filter(0, &[]),
        _ => MessageFilter::default(),
    }
}

// QUIRK K-53, K-79, C-241, C-242, C-245: kept
#[test]
fn the_stealth_scene_shows_every_state() {
    // Counter-check before the golden (all times are `step` calls, measured): 1402 camouflaged after 34
    // steps (17 ticks), visible again after uncamouflaging; 1410 camouflaged by itself, also when
    // the colonist of slot 1 stands next to it (no target for slot 1); alarm post camouflaged
    // immediately and no target for slot 1; assassin stabs the first colonist (HP -250, dead), after
    // 5 s the second, and digs out; monastery heals 1107, shipyard does not heal the ship
    // (C-242); 2303 heals 2302; 2014 starts with mana 0 (C-241), heals itself in the trance;
    // walk command in the teardown phase lost; self-healing without mana has no effect, with mana full.
    let (mut state, rules, s) = stealth_scene();
    // C-241: created healers have mana 0.
    assert_eq!(fraction(&state, s.pilgrim, MANA), 0);
    assert_eq!(fraction(&state, s.medic, MANA), 0);
    // K-79: the pseudo ID `n = 1` of `0x1E` is the archer (index 0, slot 0, gen. 1).
    assert_eq!(s.archer, ObjectId(1));
    // C-242: the soldier stands in the query of the shipyard before the ship.
    {
        let mut sim = Sim::new(&mut state, &rules);
        let o = sim.object(s.shipyard).unwrap();
        let pos = (o.grid.x.0, o.grid.z.0);
        let word = sim.module(s.shipyard, healing::INDEX).unwrap().base.flags;
        let radius = 3 * (sim.attribute(s.shipyard, 18) << 10) / 4;
        let hits = crate::collision::query_radius(
            &mut sim,
            pos,
            &healing::match_filter(word),
            radius,
            0,
            false,
        );
        assert_eq!(hits, [s.soldier, s.ship], "order of discovery");
    }
    let mut corpses = Vec::new();
    let mut pilgrim_hp = fraction(&state, s.pilgrim, HP);
    let mut farmer_hp = fraction(&state, s.farmer, HP);
    let mut sabre_hp = fraction(&state, s.sabre, HP);
    for i in 0..STEALTH_STEPS {
        let orders = stealth_orders(i, &s);
        let mana_before = if i == DIG_OUT_AT || i == SELF_HEAL_AT {
            mana_value(&mut state, &rules, s.pilgrim)
        } else {
            0
        };
        if i == LOST_WALK_AT {
            // Teardown phase: trance on, not ready, filter mode 0 empty.
            assert!(has(&state, s.pilgrim, status::HEAL));
            assert!(!heal_state(&state, s.pilgrim).0);
            assert_eq!(filter_of(&state, s.pilgrim), filter(0, &[]));
        }
        let events = step(&mut state, &rules, &orders);

        // Camouflage: 1402 via the command, camouflage and uncamouflage animation with lock bit.
        let masked = (ARCHER_MASKED..ARCHER_VISIBLE).contains(&i);
        assert_eq!(has(&state, s.archer, status::MASKED), masked, "step {i}");
        let animating = i < ARCHER_MASKED || (STOP_AT..ARCHER_VISIBLE).contains(&i);
        assert_eq!(
            has(&state, s.archer, status::ANIM_LOCKED),
            animating,
            "step {i}"
        );
        assert_eq!(mask_flags(&state, s.archer).1, animating, "step {i}");
        // 1410 without a command, alarm post immediately.
        assert_eq!(
            has(&state, s.ranger, status::MASKED),
            i >= RANGER_MASKED,
            "step {i}"
        );
        assert!(has(&state, s.post, status::MASKED), "step {i}");

        // Assassin: status, `m_IsDown`, target.
        let dig = status::MASKED | status::WAITING4KILLING | status::ANIM_LOCKED;
        assert_eq!(
            st(&state, s.assassin) & dig,
            expected_dig_status(i),
            "step {i}"
        );
        let (down, target) = dig_state(&state, s.assassin);
        assert_eq!(down, (ASSASSIN_DOWN..DUG_OUT).contains(&i), "step {i}");
        let expected_target = match i {
            _ if i < FIRST_TARGET => ObjectId::NONE,
            _ if i < SECOND_TARGET => s.victims[0],
            _ => s.victims[1],
        };
        assert_eq!(target, expected_target, "step {i}");
        // Victims: only the stabs take hit points (the archers do not shoot).
        let first = state.object(s.victims[0], false);
        assert_eq!(first.is_some(), i < FIRST_DEAD, "step {i}");
        if first.is_some() {
            let hp = if i < FIRST_STAB { 100_000 } else { STABBED };
            assert_eq!(fraction(&state, s.victims[0], HP), hp, "step {i}");
        }
        let second = state.object(s.victims[1], false);
        assert_eq!(second.is_some(), i < SECOND_DEAD, "step {i}");
        if second.is_some() {
            assert_eq!(fraction(&state, s.victims[1], HP), 100_000, "step {i}");
        }
        assert_eq!(fraction(&state, s.walker, HP), 100_000, "step {i}");
        // Death: one new carcass per stab with -250 (without armor, C-234); carcasses decay later.
        let fresh: Vec<ObjectId> = colonist_corpses(&state)
            .into_iter()
            .filter(|c| !corpses.contains(c))
            .collect();
        if i == FIRST_DEAD || i == SECOND_DEAD {
            assert_eq!(fresh.len(), 1, "step {i}: new carcass");
            assert_eq!(fraction(&state, fresh[0], HP), STABBED, "step {i}: -250");
        } else {
            assert!(fresh.is_empty(), "step {i}");
        }
        corpses.extend(fresh);

        // The third colonist walks up to the ranger; he stays camouflaged and invisible.
        assert_eq!(
            tile(&state, s.walker) == WALKER_TARGET,
            i >= WALKER_ARRIVED,
            "step {i}"
        );
        if i == WALKER_ARRIVED || i == STEALTH_STEPS - 1 {
            assert!(has(&state, s.ranger, status::MASKED));
            assert!(!seen_by_1(&mut state, &rules, s.ranger), "step {i}");
            assert!(!seen_by_1(&mut state, &rules, s.post), "step {i}");
        }

        // Pilgrim: trance with setup, self-healing (C-245), teardown, walk command lost.
        assert_eq!(
            has(&state, s.pilgrim, status::HEAL),
            i < TRANCE_OVER,
            "step {i}"
        );
        assert_eq!(
            has(&state, s.pilgrim, status::ANIM_LOCKED),
            i < PILGRIM_READY,
            "step {i}"
        );
        let (ready, sick) = heal_state(&state, s.pilgrim);
        assert_eq!(ready, (PILGRIM_READY..STOP_AT).contains(&i), "step {i}");
        let healing_self = (PILGRIM_HEALS..TRANCE_OVER).contains(&i);
        assert_eq!(
            sick,
            if healing_self { s.pilgrim } else { ObjectId(0) },
            "step {i}"
        );
        assert_eq!(
            filter_of(&state, s.pilgrim),
            expected_pilgrim_filter(i),
            "step {i}"
        );
        assert_eq!(tile(&state, s.pilgrim), PILGRIM_TILE, "step {i}: stands");
        assert!(state.object(s.pilgrim, false).unwrap().fifo.is_empty());
        let hp = fraction(&state, s.pilgrim, HP);
        if i == PILGRIM_HEALS {
            assert!(hp > pilgrim_hp, "step {i}: first healing tick");
        } else if i < PILGRIM_HEALS {
            assert_eq!(hp, 50_000, "step {i}: mana 0, then setup");
        }
        // Self-healing: without enough mana nothing, otherwise full and -50 mana.
        if i == DIG_OUT_AT {
            assert!(mana_before < 50, "step {i}: {mana_before}");
            assert!(!self_healed(&events, s.pilgrim));
            assert!(!is_full(&state, s.pilgrim));
            assert_eq!(hp, pilgrim_hp, "step {i}");
        }
        if i == SELF_HEAL_AT {
            assert!(mana_before >= 50, "step {i}: {mana_before}");
            assert!(pilgrim_hp < 100_000, "injured before");
            assert!(self_healed(&events, s.pilgrim));
            assert!(is_full(&state, s.pilgrim));
            assert_eq!(
                mana_value(&mut state, &rules, s.pilgrim),
                mana_before - 50,
                "step {i}"
            );
        } else {
            assert!(!self_healed(&events, s.pilgrim), "step {i}");
        }
        pilgrim_hp = hp;

        // Medic: trance, female farmer healed until full, then given up.
        assert!(
            has(&state, s.medic, status::HEAL) == (i >= MEDIC_TRANCE),
            "step {i}"
        );
        let (ready, sick) = heal_state(&state, s.medic);
        assert_eq!(ready, i >= MEDIC_READY, "step {i}");
        let healing_farmer = (MEDIC_HEALS..FARMER_DROPPED).contains(&i);
        assert_eq!(
            sick,
            if healing_farmer {
                s.farmer
            } else {
                ObjectId(0)
            },
            "step {i}"
        );
        let hp = fraction(&state, s.farmer, HP);
        assert!(hp >= farmer_hp, "step {i}");
        assert_eq!(hp > 50_000, i >= MEDIC_HEALS, "step {i}");
        assert_eq!(is_full(&state, s.farmer), i >= FARMER_FULL, "step {i}");
        farmer_hp = hp;

        // Monastery heals the saber fighter; shipyard and ship stay as they are (C-242).
        let monastery = heal_state(&state, s.monastery).0;
        assert_eq!(monastery, i >= MONASTERY_READY, "step {i}");
        let shipyard = heal_state(&state, s.shipyard).0;
        assert_eq!(shipyard, i >= SHIPYARD_READY, "step {i}");
        let hp = fraction(&state, s.sabre, HP);
        assert!(hp >= sabre_hp, "step {i}");
        assert_eq!(is_full(&state, s.sabre), i >= SABRE_FULL, "step {i}");
        sabre_hp = hp;
        assert_eq!(fraction(&state, s.soldier, HP), 50_000, "step {i}");
        assert_eq!(fraction(&state, s.ship, HP), 50_000, "step {i}");
    }
    assert!(sabre_hp == 100_000 && farmer_hp == 100_000);
    assert_eq!(corpses.len(), 2);
}

/// Measured (camouflage, digging in, assassination, healing trance, area healing and single healing,
/// self-healing).
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
const STEALTH_GOLDEN: u64 = 0x6573_be93_ba82_4fc1;

/// Continuous sequence of the scene without saving, computed once per test process and shared by the
/// save tests.
static STEALTH_SEQUENCE: LazyLock<Vec<u64>> =
    LazyLock::new(|| stealth_run_probed(None, |_, _, _| {}));

#[test]
fn stealth_checksums_match_the_golden_sequence() {
    let mut h = crate::rules::Fnv64::default();
    for c in STEALTH_SEQUENCE.iter() {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, STEALTH_GOLDEN, "{:#018x}", h.0);
}

/// Savegames of the scene (measured, as an assertion in the probes): in the middle of the
/// camouflage animation; between `0x61C` and `0x71C` of the first stab; dug in during the pause with
/// pending `0x31C`; healing trance in setup; teardown phase (filter mode 0 empty).
const SAVE_MASKING: usize = 20;
const SAVE_STABBING: usize = 70;
const SAVE_RESTING: usize = 120;
const SAVE_SETUP: usize = 40;
const SAVE_TEARDOWN: usize = 640;
const _: () = assert!(START_AT < SAVE_MASKING && SAVE_MASKING < ARCHER_MASKED);
const _: () = assert!(FIRST_STAB < SAVE_STABBING && SAVE_STABBING < FIRST_REST);
const _: () = assert!(FIRST_REST < SAVE_RESTING && SAVE_RESTING < SECOND_TARGET);
const _: () = assert!(SAVE_SETUP < PILGRIM_READY);
const _: () = assert!(STOP_AT < SAVE_TEARDOWN && SAVE_TEARDOWN < TRANCE_OVER);

/// Scene with saving/loading after `save_at` against the continuous sequence.
fn assert_survives_saving(save_at: usize, at_save: StealthProbe, what: &str) {
    let saved = stealth_run_probed(Some(save_at), at_save);
    let first_diff = saved
        .iter()
        .zip(STEALTH_SEQUENCE.iter())
        .position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "saved {what}: first deviation in `step`");
}

#[test]
fn stealth_checksums_survive_saving_and_loading() {
    // One separate run per save point, side by side (run time, as with `FORT_GOLDEN`). Each
    // sequence equals the continuous one.
    let cases: [(usize, StealthProbe, &str); 5] = [
        (
            SAVE_MASKING,
            |state, _, s| {
                assert_eq!(mask_flags(state, s.archer), (true, true));
                assert!(!has(state, s.archer, status::MASKED));
                assert!(has(state, s.archer, status::ANIM_LOCKED));
                assert_eq!(
                    timers(state, s.archer, masking::INDEX, masking_msg::FINISHED),
                    1
                );
            },
            "in the middle of the camouflage animation",
        ),
        (
            SAVE_STABBING,
            |state, _, s| {
                let a = s.assassin;
                assert_eq!(
                    st(state, a) & (status::MASKED | status::WAITING4KILLING),
                    status::WAITING4KILLING
                );
                assert_eq!(dig_state(state, a), (true, s.victims[0]));
                assert_eq!(timers(state, a, assassin::INDEX, assassin_msg::ATTACK), 0);
                assert_eq!(
                    timers(state, a, assassin::INDEX, assassin_msg::ATTACK_FINISHED),
                    1
                );
            },
            "between 0x61C and 0x71C",
        ),
        (
            SAVE_RESTING,
            |state, _, s| {
                let a = s.assassin;
                assert!(has(state, a, status::MASKED | status::WAITING4KILLING));
                assert!(dig_state(state, a).0);
                assert_eq!(
                    timers(state, a, assassin::INDEX, assassin_msg::CHECK_TARGET),
                    1
                );
                assert_eq!(filter_of(state, a), filter(0, &[0x21C, 0x110]));
            },
            "dug in with pending 0x31C",
        ),
        (
            SAVE_SETUP,
            |state, _, s| {
                let p = s.pilgrim;
                assert!(has(state, p, status::HEAL | status::ANIM_LOCKED));
                assert!(!heal_state(state, p).0);
                assert_eq!(timers(state, p, healing::INDEX, healing_msg::READY), 1);
                assert_eq!(filter_of(state, p), filter(0, &[healing_msg::END_REQUEST]));
            },
            "healing trance in setup",
        ),
        (
            SAVE_TEARDOWN,
            |state, _, s| {
                let p = s.pilgrim;
                assert!(has(state, p, status::HEAL));
                assert!(!has(state, p, status::ANIM_LOCKED));
                assert!(!heal_state(state, p).0);
                assert_eq!(timers(state, p, healing::INDEX, healing_msg::END), 1);
                assert_eq!(filter_of(state, p), filter(0, &[]));
            },
            "teardown phase",
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
