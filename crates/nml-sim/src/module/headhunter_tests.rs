//! Scene with `HEADHUNTER_GOLDEN` (pattern `spell_tests`; re/headhunter-robbing 9): headhunters and robbing in a
//! real level start. Slot 0 (SPA) hires the SPA headhunter on the undertaker 2615 of slot 1 (SID) at step 0; during the
//! approach slot 1 takes it over onto the missionary 1113 of slot 0, and slot 0 takes it back before its search
//! (prices 300/350/400 with refunds, 9.1). Then slot 0 hires the SID headhunter on the sheriff 2614, who is quartered
//! in a second HQ 2500 (dynamite on the HQ), and the PRI headhunter on the sharpshooter 2616 on an island (throw from a
//! distance). The undertaker of slot 0 robs twice: early the two colonists killed during setup, late the corpses
//! from the melee kill and the dynamite sequence; the island corpse stays (other region, C-321). Saving and loading
//! in the middle of each flow.
//!
//! Setup: two HQs 2500 at slot 1 (one for hiring, one with the sheriff quartered via `quarter::accept_unit`), the
//! island is the inner area of [`kit::wall_ring`] (placed after `new_match`, before the objects), the nation comes
//! from the `.lvl` players, the corpses lie in the robbing window, the sequence follows after the demolition, the
//! retake happens before the `0x322` of the takeover, the save point has `0x122` in the FIFO, stance 0 (no
//! retaliation), a shared sequence, gold 5000. All times are measured `step` calls (two `step` calls are one object
//! tick; objects of group 0 tick in even steps, those of group 1 in odd `step` calls, projectiles, destruction and
//! change of owner in the odd phase-0 step).

use std::sync::LazyLock;

use super::ability_testkit::{self as kit, *};
use super::attack;
use super::dying;
use super::headhunter::{self as hunter, HeadHunter};
use super::quarter;
use super::robbing;
use super::stealth_tests::{assert_loads, scene_rules_from};
use crate::combat::projectile::{Projectile, ProjectileKind};
use crate::command::headhunter::{COLOR_GREEN, SFX_START, SFX_SUCCESS, TEXT_HIRED, TEXT_SUCCESS};
use crate::command::{Command, CommandKind};
use crate::id::{NEUTRAL_SLOT, ObjectId};
use crate::message::{Message, Posted};
use crate::messaging::send;
use crate::object::status;
use crate::rules::Rules;
use crate::save::testkit::save_and_load;
use crate::setup::{MatchSetup, new_match};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState, TextArg};
use crate::tick::step;

/// Layout (tiles). West: HQ 1000 of slot 0 (spawn of the headhunters) and the missionary;
/// middle/east: robber, colonist corpses, melee kill target, the HQ with the sheriff and the island;
/// southeast: the HQ 2500 for hiring.
const HQ_A_TILE: (i32, i32) = (10, 10);
const HQ_HIRE_TILE: (i32, i32) = (54, 54);
/// Missionary 1113 of slot 0, target of the takeover (at least 20 tiles from the HQ 1000).
const MISSIONARY_TILE: (i32, i32) = (10, 40);
/// Undertaker 2615 of slot 1, target of the melee kill.
const TARGET_TILE: (i32, i32) = (34, 30);
/// HQ 2500 of slot 1 with the quartered sheriff 2614 (dynamite target).
const HQ_QUARTERS_TILE: (i32, i32) = (40, 24);
const SHERIFF_TILE: (i32, i32) = (40, 28);
/// Center of the island (inner area `max(|dx|, |dz|) <= 2`, ring 3) with the sharpshooter 2616.
const ISLAND: (i32, i32) = (30, 38);
const RING_INNER: i32 = 2;
const RING_OUTER: i32 = 3;
/// Robber 2615 of slot 0 and the two colonists of slot 1 who die during setup.
const ROBBER_TILE: (i32, i32) = (30, 30);
const COLONIST_TILES: [(i32, i32); 2] = [(28, 30), (28, 32)];
/// Map size of the test world (tiles).
const MAP_TILES: i32 = 64;

/// Square of the tile distance.
const fn dist2(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0) * (a.0 - b.0) + (a.1 - b.1) * (a.1 - b.1)
}
/// Robbing radius: sight 32 / 4 = 8 tiles (config 18 of the undertaker, re/headhunter-robbing 3.8).
const ROB_RADIUS: i32 = 32 / 4;
// The missionary stands at least 20 tiles from the HQ 1000 (reserve against a melee kill after
// the takeover).
const _: () = assert!(dist2(MISSIONARY_TILE, HQ_A_TILE) >= 20 * 20);
// The colonist corpses lie within the robbing radius of the robber, the melee kill target and the island within the
// radius of the second colonist corpse (he stands there after the first robbing; the island filters
// the region, C-321).
const _: () = {
    let mut k = 0;
    while k < COLONIST_TILES.len() {
        assert!(dist2(COLONIST_TILES[k], ROBBER_TILE) <= ROB_RADIUS * ROB_RADIUS);
        k += 1;
    }
    assert!(dist2(TARGET_TILE, COLONIST_TILES[1]) <= ROB_RADIUS * ROB_RADIUS);
    assert!(dist2(ISLAND, COLONIST_TILES[1]) <= ROB_RADIUS * ROB_RADIUS);
};
// The ring lies entirely on the map.
const _: () = assert!(ISLAND.0 - RING_OUTER >= 0 && ISLAND.0 + RING_OUTER < MAP_TILES);
const _: () = assert!(ISLAND.1 - RING_OUTER >= 0 && ISLAND.1 + RING_OUTER < MAP_TILES);

/// Setup steps (like `SPELL_GOLDEN`): `0x110` to the colonists, then two `step` calls until the
/// change of owner to slot 8.
const SETUP_STEPS: usize = 2;
/// Starting gold of both slots and mana of the robber (config 15).
const START_GOLD: i32 = 5000;
const ROBBER_MANA: i32 = 150;

/// Command times (`step` calls; the command runs in the `step` with this index).
///
/// Step 0: stance 0 for both slots, slot 0 hires the SPA headhunter on 2615.
const HIRE_AT: usize = 0;
/// Takeover by slot 1 (missionary 1113 of slot 0) during the approach; odd step in which the
/// group 0 of the SPA headhunter does not tick: `0x122` stays in the FIFO until the next step.
const TAKEOVER_AT: usize = 31;
/// Retake by slot 0 before the `0x322` of the takeover.
const RETAKE_AT: usize = 40;
/// Slot 0 hires the SID headhunter on the quartered sheriff (dynamite on the HQ).
const DYNAMITE_AT: usize = 50;
/// Slot 0 hires the PRI headhunter on the sharpshooter of the island (throw from a distance).
const THROW_AT: usize = 60;
/// First robbing (the colonists) and second robbing (melee kill and sheriff corpses).
const ROB_EARLY: usize = 70;
const ROB_LATE: usize = 580;
/// Length of the scene (the last flow, the decay of the sheriff corpse, ends at
/// [`SHERIFF_CORPSE_GONE`]).
const HUNTER_STEPS: usize = 2400;

/// Measured `step` calls (debug = release) after which the state first holds.
///
/// SPA headhunter 4100 (group 0): `0x122` in the command step (N1 = 0), search N1 + 11 ticks;
/// takeover delivered in the step after the command, retake in the command step, new search 11
/// ticks later. Walk success (lock, animation 1) and melee kill at time marker 1800 ms (N + 19),
/// way home, vanishing.
const SPA_SEARCH: usize = 22;
const TAKEOVER_HIRED: usize = 32;
const SPA_RESEARCH: usize = 62;
const SPA_KILL_ARRIVAL: usize = 320;
const SPA_KILL: usize = 358;
const TARGET_CORPSE: usize = 359;
const SPA_GONE: usize = 427;
/// SID headhunter 4102 (group 1): created in the command step, `0x122` in the next; search -> state 2
/// (container); walk success -> throw animation 4, throw at time marker 760 ms (N + 8), impact `0xE22`
/// (flight time measured), HQ -110 % and `retry`; the HQ dies in the next step and releases the
/// sheriff; after two `0x322` (C-317) the headhunter walks to the sheriff: melee kill N + 19, way home.
const SID_HIRED: usize = 51;
const SID_SEARCH: usize = 73;
const SID_THROW_ARRIVAL: usize = 337;
const SID_LAUNCH: usize = 353;
const SID_IMPACT: usize = 373;
/// `0xC22` at time marker 1720 ms (N + 18): lock off, second `retry` (C-317).
const SID_THROW_END: usize = 373;
const QUARTERS_DIED: usize = 374;
const SID_SEEK_SHERIFF: usize = 415;
const SID_RESET: usize = 417;
const SID_RESEEK: usize = 459;
const QUARTERS_GONE: usize = 469;
const SID_KILL_ARRIVAL: usize = 497;
const SID_KILL: usize = 535;
const SHERIFF_CORPSE: usize = 537;
const SID_GONE: usize = 835;
/// PRI headhunter 4101 (group 0): `0x122` in the command step; search -> state 1; the approach fails at the
/// ring (`0x922`) -> throw animation 3, throw N + 8, impact `0xD22` kills (no success, C-314);
/// `0xC22` -> `retry`, the search finds nothing any more -> way home, vanishing.
const PRI_SEARCH: usize = 82;
const PRI_THROW: usize = 386;
const PRI_LAUNCH: usize = 402;
const PRI_IMPACT: usize = 417;
const PRI_THROW_END: usize = 422;
const ISLANDER_CORPSE: usize = 419;
const PRI_HOMEWARD: usize = 464;
const PRI_GONE: usize = 759;
/// Robbing: arrival at the corpse (animation 3, lock) and `0x330` at time marker 2499 ms
/// (N + 25), +25 gold each; order colonist 1, colonist 2, melee kill corpse, sheriff corpse.
const ROB_ARRIVALS: [usize; 4] = [102, 186, 672, 822];
const ROBBED: [usize; 4] = [152, 236, 722, 872];
/// Decay (`0x210` after 30 s, then sinking) of the corpses: colonists, island corpse,
/// sheriff corpse.
const COLONIST_CORPSES_GONE: usize = 661;
/// The island corpse becomes robbable (`0x710` at the time marker) and loses it with `0x210` untouched.
const ISLAND_CORPSE_ROBBABLE: usize = 425;
const ISLAND_CORPSE_UNROBBABLE: usize = 1025;
const SHERIFF_CORPSE_GONE: usize = 1201;

// Timeline of the headhunters (ticks of the headhunter modules; one tick = 2 `step` calls).
const _: () = assert!(SPA_SEARCH == HIRE_AT + 2 * 11);
const _: () = assert!(TAKEOVER_AT % 2 == 1 && TAKEOVER_HIRED == TAKEOVER_AT + 1);
const _: () = assert!(SPA_SEARCH < TAKEOVER_AT, "takeover during the approach");
// Retake before the `0x322` of the takeover.
const _: () = assert!(TAKEOVER_AT < RETAKE_AT && RETAKE_AT < TAKEOVER_AT + 2 * 11);
const _: () = assert!(RETAKE_AT < TAKEOVER_HIRED + 2 * 11);
const _: () = assert!(SPA_RESEARCH == RETAKE_AT + 2 * 11);
const _: () = assert!(SPA_KILL == SPA_KILL_ARRIVAL + 2 * 19 && TARGET_CORPSE == SPA_KILL + 1);
const _: () = assert!(SID_HIRED == DYNAMITE_AT + 1 && SID_SEARCH == SID_HIRED + 2 * 11);
const _: () = assert!(SID_LAUNCH == SID_THROW_ARRIVAL + 2 * 8 && SID_LAUNCH < SID_IMPACT);
const _: () = assert!(QUARTERS_DIED == SID_IMPACT + 1);
const _: () = assert!(SID_THROW_END == SID_THROW_ARRIVAL + 2 * 18);
const _: () = assert!(PRI_THROW_END == PRI_THROW + 2 * 18);
const _: () = assert!(SID_KILL == SID_KILL_ARRIVAL + 2 * 19 && SHERIFF_CORPSE == SID_KILL + 2);
const _: () = assert!(PRI_SEARCH == THROW_AT + 2 * 11);
const _: () = assert!(PRI_LAUNCH == PRI_THROW + 2 * 8 && ISLANDER_CORPSE == PRI_IMPACT + 2);
const _: () = assert!(DYNAMITE_AT > RETAKE_AT && THROW_AT > DYNAMITE_AT + 2);
// Robbing: time marker 2499 ms -> `0x330` 25 ticks after the arrival.
const _: () = {
    let mut k = 0;
    while k < 4 {
        assert!(ROBBED[k] == ROB_ARRIVALS[k] + 2 * 25);
        k += 1;
    }
};
// Robbable from T + 15 ticks after death, no longer from `0x210` (T + 300 ticks).
const _: () = assert!(2 * 15 <= SETUP_STEPS + ROB_EARLY);
const _: () = assert!(SETUP_STEPS + ROBBED[1] < 2 * 300);
const _: () = assert!(SID_KILL + 2 * 15 <= ROB_LATE && ROB_LATE > SPA_KILL + 2 * 15);
const _: () = assert!(ROBBED[2] < SPA_KILL + 2 * 300 && ROBBED[3] < SID_KILL + 2 * 300);
const _: () = assert!(SHERIFF_CORPSE_GONE < HUNTER_STEPS);

/// IDs of the scene (setup). The colonist corpses carry their ID on slot 8.
struct HunterScene {
    hq_a: ObjectId,
    hq_hire: ObjectId,
    hq_quarters: ObjectId,
    missionary: ObjectId,
    robber: ObjectId,
    target: ObjectId,
    sheriff: ObjectId,
    islander: ObjectId,
    corpses: [ObjectId; 2],
}

/// IDs that do not arise until the run, recorded right after the `step` in which they arise:
/// the headhunters (from [`SimState::headhunters`]) and the corpses of the three headhunter targets (from
/// `OwnerChanged`); they survive saving/loading.
#[derive(Default)]
struct Late {
    hunters: [Option<ObjectId>; 3],
    target_corpse: Option<ObjectId>,
    sheriff_corpse: Option<ObjectId>,
    islander_corpse: Option<ObjectId>,
}

impl Late {
    /// After each `step` with its events.
    fn observe(&mut self, state: &SimState, s: &HunterScene, events: &[SimEvent]) {
        for (seen, now) in self.hunters.iter_mut().zip(state.headhunters) {
            if seen.is_none() {
                *seen = now;
            }
        }
        for e in events {
            if let SimEvent::OwnerChanged { old, new } = *e {
                if old == s.target {
                    self.target_corpse = Some(new);
                } else if old == s.sheriff {
                    self.sheriff_corpse = Some(new);
                } else if old == s.islander {
                    self.islander_corpse = Some(new);
                }
            }
        }
    }

    fn hunter(&self, art: usize) -> ObjectId {
        self.hunters[art].expect("headhunter hired")
    }
}

/// Rules of the scene: the headhunter world ([`kit::hunter_rules`]) with two human players in the
/// `.lvl` ([`scene_rules_from`], nation 2 for both); slot 1 gets nation 3 (SID) via the
/// `.lvl` players like `setup_players` (`setup.rs`).
fn scene_rules() -> Rules {
    let mut rules = scene_rules_from(kit::hunter_rules());
    rules.level.players[1].nation = 3;
    rules
}

/// Scene via a real level start (`setup::new_match`, mode -1), map revealed; slot 0
/// (SPA) and 1 (SID) are enemies of each other. Setup in a fixed order: 1. ring of the island (before the
/// objects), 2. gold of both slots 5000, research 3326 for slot 0, 3. all objects,
/// the sheriff quartered, 4. kill the colonists and [`SETUP_STEPS`] steps until the
/// change of owner, 5. mana of the robber, 6. then step 0 of the sequence.
fn hunter_scene() -> (SimState, Rules, HunterScene) {
    let rules = scene_rules();
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).expect("level start");
    crate::fog::reveal_map(&mut state, -1);
    assert!(state.diplomacy.is_enemy(0, 1) && state.diplomacy.is_enemy(1, 0));
    wall_ring(&mut state, ISLAND, RING_INNER, RING_OUTER);
    for p in [0, 1] {
        state.players[p].set_resource(0, START_GOLD);
    }
    unlock(&mut state, 0, ROB_UPGRADE);
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        let hq_a = spawn(&mut sim, HQ_SPA, 0, HQ_A_TILE);
        let hq_hire = spawn(&mut sim, HQ_SID, 1, HQ_HIRE_TILE);
        let hq_quarters = spawn(&mut sim, HQ_SID, 1, HQ_QUARTERS_TILE);
        let missionary = spawn(&mut sim, MISSIONARY, 0, MISSIONARY_TILE);
        let robber = spawn(&mut sim, UNDERTAKER, 0, ROBBER_TILE);
        let target = spawn(&mut sim, UNDERTAKER, 1, TARGET_TILE);
        let sheriff = spawn(&mut sim, SID_SHERIFF, 1, SHERIFF_TILE);
        assert_eq!(quarter::accept_unit(&mut sim, hq_quarters, sheriff), 0);
        let islander = spawn(&mut sim, SHARPSHOOTER, 1, ISLAND);
        let victims = COLONIST_TILES.map(|t| spawn(&mut sim, COLONIST, 1, t));
        for v in victims {
            send(&mut sim, v, &Message::new(0x110, &[]));
        }
        let mut events = Vec::new();
        for _ in 0..SETUP_STEPS {
            events.extend(step(sim.state, sim.rules, &[]));
        }
        let corpses = victims.map(|v| {
            events
                .iter()
                .find_map(|e| match e {
                    SimEvent::OwnerChanged { old, new } if *old == v => Some(*new),
                    _ => None,
                })
                .expect("corpse on slot 8")
        });
        let p = super::spell::mana_percent(&sim, robber);
        sim.object_mut(robber)
            .expect("robber")
            .attributes
            .set_current_value(MANA, ROBBER_MANA, p);
        HunterScene {
            hq_a,
            hq_hire,
            hq_quarters,
            missionary,
            robber,
            target,
            sheriff,
            islander,
            corpses,
        }
    };
    (state, rules, scene)
}

/// Commands in the `step` call `i` (buttons and values like the command bar).
fn hunter_orders(i: usize, s: &HunterScene) -> Vec<Command> {
    let cmd = |player, kind| Command { player, kind };
    let rob = || {
        cmd(
            0,
            CommandKind::RobCorpses {
                unit: s.robber,
                button: ROB_BUTTON,
            },
        )
    };
    match i {
        HIRE_AT => vec![
            // Stance 0 for all units with an attack module (the quartered sheriff has none).
            cmd(
                1,
                CommandKind::SetBehavior {
                    units: vec![s.target, s.islander],
                    mode: 0,
                },
            ),
            cmd(
                0,
                CommandKind::SetBehavior {
                    units: vec![s.missionary, s.robber],
                    mode: 0,
                },
            ),
            cmd(0, hire_command(0, 1, UNDERTAKER as i32, s.hq_a)),
        ],
        TAKEOVER_AT => vec![cmd(1, hire_command(0, 0, MISSIONARY as i32, s.hq_hire))],
        RETAKE_AT => vec![cmd(0, hire_command(0, 1, UNDERTAKER as i32, s.hq_a))],
        DYNAMITE_AT => vec![cmd(0, hire_command(2, 1, SID_SHERIFF as i32, s.hq_a))],
        THROW_AT => vec![cmd(0, hire_command(1, 1, SHARPSHOOTER as i32, s.hq_a))],
        ROB_EARLY | ROB_LATE => vec![rob()],
        _ => vec![],
    }
}

/// State assertion at the save time: state, rules, IDs of the scene and of the run.
type HunterProbe = fn(&mut SimState, &Rules, &HunterScene, &Late);

/// Checksum per `step` call of the scene. With `save_at = Some(i)` the state is saved after the `step` call `i`,
/// checked against the rules (like the viewer when loading) and reloaded; `at_save`
/// sees the state immediately before.
fn hunter_run_probed(save_at: Option<usize>, at_save: HunterProbe) -> Vec<u64> {
    let (mut state, rules, scene) = hunter_scene();
    let mut late = Late::default();
    let mut out = Vec::with_capacity(HUNTER_STEPS);
    for i in 0..HUNTER_STEPS {
        let orders = hunter_orders(i, &scene);
        let events = step(&mut state, &rules, &orders);
        late.observe(&state, &scene, &events);
        if save_at == Some(i) {
            at_save(&mut state, &rules, &scene, &late);
            assert_loads(&state, &rules);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

/// Hit points and extra energy (mana).
const HP: u32 = 14;
const MANA: u32 = 15;

fn exists(state: &SimState, id: ObjectId) -> bool {
    state.object(id, false).is_some()
}

fn has(state: &SimState, id: ObjectId, bits: u32) -> bool {
    state.object(id, false).expect("object").status & bits == bits
}

fn locked(state: &SimState, id: ObjectId) -> bool {
    has(state, id, status::ANIM_LOCKED)
}

fn moving(state: &SimState, id: ObjectId) -> bool {
    has(state, id, status::MOVE)
}

/// Value of the attribute with the upgrade percentages of the owner (like `GetCurrentValue`).
fn value(state: &mut SimState, rules: &Rules, id: ObjectId, attr: u32) -> i32 {
    Sim::new(state, rules).attribute(id, attr)
}

fn gold(state: &SimState) -> [i32; 2] {
    [state.resource(0, 0), state.resource(1, 0)]
}

/// Module 34 of the headhunter `id`.
fn hunter_of(state: &SimState, id: ObjectId) -> HeadHunter {
    match &state
        .object(id, false)
        .and_then(|o| o.module(hunter::INDEX))
        .expect("module 34")
        .kind
    {
        super::ModuleKind::HeadHunter(h) => h.clone(),
        k => panic!("not module 34: {k:?}"),
    }
}

/// `(customer, paid, next price, target player, target GUID)` of the headhunter.
fn terms(state: &SimState, id: ObjectId) -> (i32, i32, i32, i32, i32) {
    let h = hunter_of(state, id);
    (
        h.customer,
        h.customer_cost,
        h.cur_reward,
        h.target_player,
        h.target_guid,
    )
}

/// `(state, target_hid, last_target)` of the headhunter.
fn hunt_state(state: &SimState, id: ObjectId) -> (i32, ObjectId, ObjectId) {
    let h = hunter_of(state, id);
    (h.state, h.target_hid, h.last_target)
}

/// Is a timer `code` pending in module `index` (`ModuleBase::has_pending_timer`)?
fn pending(state: &SimState, id: ObjectId, index: usize, code: u32) -> bool {
    state
        .object(id, false)
        .and_then(|o| o.module(index))
        .is_some_and(|m| m.base.has_pending_timer(code))
}

/// Success and fault feedback message of the walk (module 0) as codes.
fn replies(state: &SimState, id: ObjectId) -> (Option<u32>, Option<u32>) {
    let b = &state.object(id, false).unwrap().module(0).unwrap().base;
    let code = |m: &Option<Message>| m.as_ref().map(|m| m.code);
    (code(&b.success), code(&b.fault))
}

/// The throws in the projectile list.
fn throws(state: &SimState) -> Vec<Projectile> {
    state
        .projectiles
        .list
        .iter()
        .filter(|p| p.kind == ProjectileKind::Throw)
        .cloned()
        .collect()
}

/// The corpse `id` is robbable (`CANROBBED` in the flags of module 16).
fn robbable(state: &SimState, id: ObjectId) -> bool {
    state
        .object(id, false)
        .and_then(|o| o.module(dying::INDEX))
        .is_some_and(|m| m.base.flags & dying::CANROBBED != 0)
}

/// The corpse in the pending `0x330 {corpse}` of the robber.
fn rob_timer(state: &SimState, id: ObjectId) -> Option<ObjectId> {
    let m = state.object(id, false)?.module(robbing::INDEX)?;
    m.base
        .events
        .iter()
        .chain(&m.base.timers_ws)
        .find(|e| e.msg.code == robbing::msg::ROB)
        .map(|e| ObjectId::from_i32(e.msg.arg(0)))
}

/// The attack module `0x11` of the robber ticks.
fn attack_ticks(state: &SimState, id: ObjectId) -> bool {
    state
        .object(id, false)
        .is_some_and(|o| o.tick_list.contains(&(robbing::ATTACK as u8)))
}

/// Expected gold of both slots after `step` `i` (9.1): hiring 300 (SPA), takeover 350 with
/// refund 300 to slot 0 on delivery, retake 400 with refund 350 in the
/// command step (group 0), SID and PRI 300 each (new headhunters, config 12), +25 per corpse.
fn expected_gold(i: usize) -> [i32; 2] {
    let mut g = [START_GOLD; 2];
    let mut book = |at: usize, p: usize, v: i32| {
        if i >= at {
            g[p] += v;
        }
    };
    book(HIRE_AT, 0, -300);
    book(TAKEOVER_AT, 1, -350);
    book(TAKEOVER_HIRED, 0, 300);
    book(RETAKE_AT, 0, -400);
    book(RETAKE_AT, 1, 350);
    book(DYNAMITE_AT, 0, -300);
    book(THROW_AT, 0, -300);
    for at in ROBBED {
        book(at, 0, 25);
    }
    g
}

/// Messages 10010/10011 and 2D sounds of the step.
fn announcements(events: &[SimEvent]) -> Vec<SimEvent> {
    events
        .iter()
        .filter(|e| matches!(e, SimEvent::TextMessage { .. } | SimEvent::Sound2d { .. }))
        .cloned()
        .collect()
}

/// Sound and message 10010 to everyone when hiring.
fn hired(player: u8, guid: u32) -> Vec<SimEvent> {
    vec![
        SimEvent::Sound2d {
            name: SFX_START.into(),
        },
        SimEvent::TextMessage {
            to: None,
            text: TEXT_HIRED,
            args: vec![TextArg::PlayerName(player), TextArg::TypeName(guid)],
            color: COLOR_GREEN,
            duration_ms: 6000,
        },
    ]
}

/// Message 10011 and sound to everyone on success (3.4).
fn succeeded(guid: u32) -> Vec<SimEvent> {
    vec![
        SimEvent::TextMessage {
            to: None,
            text: TEXT_SUCCESS,
            args: vec![TextArg::TypeName(guid)],
            color: COLOR_GREEN,
            duration_ms: 3000,
        },
        SimEvent::Sound2d {
            name: SFX_SUCCESS.into(),
        },
    ]
}

/// Center of the tile in fixed point (`<< 12`).
fn fx(t: i32) -> i32 {
    (t << 12) + 0x800
}

#[test]
fn the_hunter_scene_shows_every_state() {
    // Counter-check before the golden, each value individually (all times `step` calls, measured):
    // prices 300/350/400 with refunds; melee kill with price 500 and customer -1; way home and
    // vanishing (table empty); HQ -110 % through dynamite, no success, then melee kill on the freed
    // sheriff; island target killed from a distance; robbing +25 gold per corpse,
    // attack module off afterwards; no retaliation.
    let (mut state, rules, s) = hunter_scene();
    // Setup: nations, corpses, quarters, island, gold, mana.
    assert_eq!((state.players[0].nation, state.players[1].nation), (2, 3));
    for c in s.corpses {
        assert_eq!(c.slot(), NEUTRAL_SLOT);
        assert!(has(&state, c, status::DIED));
    }
    assert!(has(&state, s.sheriff, status::QUARTERED));
    let (outside, inside) = {
        let mut sim = Sim::new(&mut state, &rules);
        let outside = crate::world::region_id_at(&mut sim, s.robber, -1, -1, -1);
        let inside = crate::world::region_id_at(&mut sim, s.robber, -1, fx(ISLAND.0), fx(ISLAND.1));
        (outside, inside)
    };
    assert_ne!(inside, crate::path::BLOCKED);
    assert_ne!(inside, outside, "island: other region");
    assert_eq!(gold(&state), [START_GOLD; 2]);
    assert_eq!(value(&mut state, &rules, s.robber, MANA), ROBBER_MANA);

    let mut late = Late::default();
    let mut successes = Vec::new();
    for i in 0..HUNTER_STEPS {
        let orders = hunter_orders(i, &s);
        let events = step(&mut state, &rules, &orders);
        late.observe(&state, &s, &events);
        let news = announcements(&events);

        // Gold of both slots in every step (differences per command step).
        assert_eq!(gold(&state), expected_gold(i), "step {i}: gold");

        // No retaliation: no unit (except corpses) fights or has an attack target.
        for o in state.objects().filter(|o| o.id.slot() != NEUTRAL_SLOT) {
            assert_eq!(
                o.status & status::FIGHTING,
                0,
                "step {i}: {:?} fights",
                o.id
            );
        }
        {
            let sim = Sim::new(&mut state, &rules);
            for u in [s.missionary, s.robber, s.target, s.islander] {
                if let Some(a) = sim.object(u).and(attack::attack(&sim, u)) {
                    assert!(a.target.is_none(), "step {i}: {u:?} attacks");
                    if i == HIRE_AT {
                        let quiet = quarter::attack_mode_msg(0, false);
                        assert_eq!((a.aggressiveness, a.mode), (quiet.arg(0), quiet.arg(1)));
                    }
                }
            }
        }
        // The missionary survives.
        assert_eq!(
            value(&mut state, &rules, s.missionary, HP),
            1000,
            "step {i}"
        );

        // Messages 10011 only at the two melee kills.
        if news
            .iter()
            .any(|e| matches!(e, SimEvent::TextMessage { text, .. } if *text == TEXT_SUCCESS))
        {
            successes.push(i);
        }

        // --- SPA headhunter 4100: hiring, takeover, retake, melee kill, way home ---
        if i == HIRE_AT {
            let spa = late.hunter(0);
            let o = state.object(spa, false).unwrap();
            assert_eq!((o.guid, spa.slot()), (HUNTER_SPA, NEUTRAL_SLOT));
            assert_eq!(terms(&state, spa), (0, 300, 350, 1, UNDERTAKER as i32));
            assert!(pending(&state, spa, hunter::INDEX, hunter::msg::THINK));
            assert!(state.tick_groups[0].contains(&spa), "group 0");
            assert_eq!(news, hired(0, HUNTER_SPA));
        }
        if let Some(spa) = late.hunters[0].filter(|_| i < SPA_GONE) {
            let (st, hid, last) = hunt_state(&state, spa);
            match i {
                _ if i < SPA_SEARCH => assert_eq!(st, 0, "step {i}"),
                SPA_SEARCH => {
                    assert_eq!((st, hid, last), (1, s.target, s.target));
                    assert!(moving(&state, spa));
                }
                TAKEOVER_AT => {
                    assert!(moving(&state, spa), "takeover during the approach");
                    assert_eq!(terms(&state, spa).0, 0, "not yet delivered");
                    assert_eq!(news, hired(1, HUNTER_SPA));
                }
                TAKEOVER_HIRED => {
                    assert_eq!(terms(&state, spa), (1, 350, 400, 0, MISSIONARY as i32));
                    assert_eq!(st, 0);
                    assert!(pending(&state, spa, hunter::INDEX, hunter::msg::THINK));
                }
                RETAKE_AT => {
                    assert_eq!(terms(&state, spa), (0, 400, 450, 1, UNDERTAKER as i32));
                    assert_eq!(news, hired(0, HUNTER_SPA));
                }
                SPA_RESEARCH => assert_eq!((st, last), (1, s.target)),
                SPA_KILL_ARRIVAL => {
                    assert_eq!(state.object(spa, false).unwrap().anim.wanted, 1);
                    assert!(pending(&state, spa, hunter::INDEX, hunter::msg::KILL));
                }
                SPA_KILL => {
                    assert_eq!(news, succeeded(HUNTER_SPA));
                    assert_eq!(terms(&state, spa), (-1, 400, 500, -1, -1));
                    assert!(moving(&state, spa), "way home");
                    assert_eq!(
                        replies(&state, spa),
                        (Some(hunter::msg::VANISH), Some(hunter::msg::VANISH))
                    );
                    assert!(value(&mut state, &rules, s.target, HP) <= 0, "-110 %");
                }
                _ => {}
            }
            if (TAKEOVER_HIRED..RETAKE_AT).contains(&i) {
                assert_eq!(terms(&state, spa).0, 1, "step {i}: customer slot 1");
            }
            assert_eq!(
                locked(&state, spa),
                (SPA_KILL_ARRIVAL..SPA_KILL).contains(&i),
                "step {i}"
            );
        }
        if i == TARGET_CORPSE {
            assert!(!exists(&state, s.target));
            assert_eq!(late.target_corpse.unwrap().slot(), NEUTRAL_SLOT);
        }
        assert_eq!(
            state.headhunters[0].is_some(),
            i < SPA_GONE,
            "step {i}: SPA registered"
        );
        assert_eq!(exists(&state, late.hunter(0)), i < SPA_GONE, "step {i}");

        // --- SID headhunter 4102: dynamite on the HQ with the sheriff, then melee kill ---
        if i == DYNAMITE_AT {
            let sid = late.hunter(2);
            assert_eq!(state.object(sid, false).unwrap().guid, HUNTER_SID);
            assert_eq!(terms(&state, sid), (-1, 300, 300, 0, 0), "created");
            assert!(state.tick_groups[1].contains(&sid), "group 1");
            assert_eq!(news, hired(0, HUNTER_SID));
        }
        if let Some(sid) = late.hunters[2].filter(|_| (SID_HIRED..SID_GONE).contains(&i)) {
            let (st, hid, last) = hunt_state(&state, sid);
            match i {
                SID_HIRED => assert_eq!(terms(&state, sid), (0, 300, 350, 1, SID_SHERIFF as i32)),
                SID_SEARCH => {
                    assert_eq!((st, hid, last), (2, s.sheriff, s.hq_quarters), "container");
                }
                SID_THROW_ARRIVAL => {
                    assert_eq!(state.object(sid, false).unwrap().anim.wanted, 4);
                    assert!(pending(
                        &state,
                        sid,
                        hunter::INDEX,
                        hunter::msg::THROW_BUILDING
                    ));
                    assert!(pending(&state, sid, hunter::INDEX, hunter::msg::THROW_END));
                }
                SID_IMPACT => {
                    // (1) Impact `0xE22`: HQ -110 %, no message, price stays, `retry`.
                    assert!(
                        value(&mut state, &rules, s.hq_quarters, HP) <= 0,
                        "HQ -110 %"
                    );
                    assert!(news.is_empty(), "no success");
                    assert_eq!(terms(&state, sid), (0, 300, 350, 1, SID_SHERIFF as i32));
                    assert_eq!(st, 0, "retry aus 0xE22");
                }
                QUARTERS_DIED => {
                    // (2) The HQ dies, the sheriff is released alive.
                    assert!(has(&state, s.hq_quarters, status::DIED));
                    assert!(!has(&state, s.sheriff, status::QUARTERED));
                    assert!(!has(&state, s.sheriff, status::DIED));
                    assert!(value(&mut state, &rules, s.sheriff, HP) > 0);
                }
                SID_SEEK_SHERIFF => assert_eq!((st, hid, last), (1, s.sheriff, s.sheriff)),
                SID_RESET => assert_eq!(st, 0, "second 0x322 resets (C-317)"),
                SID_RESEEK => assert_eq!((st, last), (1, s.sheriff)),
                SID_KILL_ARRIVAL => {
                    assert_eq!(state.object(sid, false).unwrap().anim.wanted, 1);
                    assert!(pending(&state, sid, hunter::INDEX, hunter::msg::KILL));
                }
                SID_KILL => {
                    // (3) Melee kill on the sheriff: success, price 500, customer -1, way home.
                    assert_eq!(news, succeeded(HUNTER_SID));
                    assert_eq!(terms(&state, sid), (-1, 300, 500, -1, -1));
                    assert_eq!(
                        replies(&state, sid),
                        (Some(hunter::msg::VANISH), Some(hunter::msg::VANISH))
                    );
                }
                _ => {}
            }
            if (SID_HIRED..SID_KILL).contains(&i) {
                assert_eq!(
                    hunter_of(&state, sid).cur_reward,
                    350,
                    "step {i}: no success"
                );
            }
            assert_eq!(
                locked(&state, sid),
                (SID_THROW_ARRIVAL..SID_THROW_END).contains(&i)
                    || (SID_KILL_ARRIVAL..SID_KILL).contains(&i),
                "step {i}"
            );
        }
        let sid_throws = throws(&state)
            .into_iter()
            .filter(|p| Some(p.shooter) == late.hunters[2])
            .count();
        assert_eq!(
            sid_throws,
            usize::from((SID_LAUNCH..SID_IMPACT).contains(&i)),
            "step {i}: dynamite on the HQ"
        );
        assert_eq!(exists(&state, s.hq_quarters), i < QUARTERS_GONE, "step {i}");
        if i < QUARTERS_DIED {
            assert!(has(&state, s.sheriff, status::QUARTERED), "step {i}");
        }
        if i == SHERIFF_CORPSE {
            assert!(!exists(&state, s.sheriff));
            assert_eq!(late.sheriff_corpse.unwrap().slot(), NEUTRAL_SLOT);
        }
        if i >= DYNAMITE_AT {
            assert_eq!(
                state.headhunters[2].is_some(),
                i < SID_GONE,
                "step {i}: SID"
            );
        }

        // --- PRI headhunter 4101: throw from a distance onto the island ---
        if i == THROW_AT {
            let pri = late.hunter(1);
            assert_eq!(state.object(pri, false).unwrap().guid, HUNTER_PRI);
            assert_eq!(terms(&state, pri), (0, 300, 350, 1, SHARPSHOOTER as i32));
            assert!(state.tick_groups[0].contains(&pri), "group 0");
            assert_eq!(news, hired(0, HUNTER_PRI));
        }
        if let Some(pri) = late.hunters[1].filter(|_| (THROW_AT..PRI_GONE).contains(&i)) {
            let (st, hid, last) = hunt_state(&state, pri);
            match i {
                PRI_SEARCH => {
                    assert_eq!((st, hid, last), (1, s.islander, s.islander));
                    let mut sim = Sim::new(&mut state, &rules);
                    let own = crate::world::region_id_at(&mut sim, pri, -1, -1, -1);
                    let island =
                        crate::world::region_id_at(&mut sim, pri, -1, fx(ISLAND.0), fx(ISLAND.1));
                    assert_ne!(
                        own, island,
                        "island separated in the walk layer of the headhunter"
                    );
                }
                PRI_THROW => {
                    // Walk fault `0x922` in state 1 -> throw animation 3 (C-313).
                    assert!(!moving(&state, pri));
                    assert_eq!(state.object(pri, false).unwrap().anim.wanted, 3);
                    assert!(pending(&state, pri, hunter::INDEX, hunter::msg::THROW_UNIT));
                }
                PRI_LAUNCH => {
                    let [p] = throws(&state)
                        .into_iter()
                        .filter(|p| p.shooter == pri)
                        .collect::<Vec<_>>()
                        .try_into()
                        .expect("one throw");
                    assert_eq!(
                        p.damage,
                        Message::new(hunter::msg::IMPACT_UNIT, &[0]),
                        "impact 0xD22"
                    );
                    let d = crate::combat::hit_zone::vector3_length([
                        p.target_pos[0] - p.start[0],
                        p.target_pos[1] - p.start[1],
                        p.target_pos[2] - p.start[2],
                    ]);
                    assert!(d / 128.0 > RING_OUTER as f32, "from a distance");
                }
                PRI_IMPACT => {
                    assert!(value(&mut state, &rules, s.islander, HP) <= 0, "-110 %");
                    assert!(news.is_empty(), "no success (C-314)");
                }
                PRI_HOMEWARD => {
                    // The search no longer finds an object (corpse on slot 8) -> way home.
                    assert_eq!(
                        replies(&state, pri),
                        (Some(hunter::msg::VANISH), Some(hunter::msg::VANISH))
                    );
                    assert!(moving(&state, pri));
                }
                _ => {}
            }
            assert_eq!(
                terms(&state, pri),
                (0, 300, 350, 1, SHARPSHOOTER as i32),
                "step {i}: no success"
            );
            assert_eq!(
                locked(&state, pri),
                (PRI_THROW..PRI_THROW_END).contains(&i),
                "step {i}"
            );
        }
        let pri_throws = throws(&state)
            .into_iter()
            .filter(|p| Some(p.shooter) == late.hunters[1])
            .count();
        assert_eq!(
            pri_throws,
            usize::from((PRI_LAUNCH..PRI_IMPACT).contains(&i)),
            "step {i}: throw onto the island"
        );
        if i == ISLANDER_CORPSE {
            assert!(!exists(&state, s.islander));
            assert_eq!(late.islander_corpse.unwrap().slot(), NEUTRAL_SLOT);
        }
        if i >= THROW_AT {
            assert_eq!(
                state.headhunters[1].is_some(),
                i < PRI_GONE,
                "step {i}: PRI"
            );
        }
        if i >= SID_GONE {
            assert_eq!(state.headhunters, [None; 3], "step {i}: table empty");
        }

        // --- Robbing ---
        assert_eq!(
            attack_ticks(&state, s.robber),
            i < ROB_EARLY,
            "step {i}: attack module of the robber"
        );
        if i == ROB_EARLY || i == ROB_LATE {
            assert_eq!(value(&mut state, &rules, s.robber, MANA), ROBBER_MANA - 10);
        }
        if i == ROB_LATE - 1 {
            assert_eq!(
                value(&mut state, &rules, s.robber, MANA),
                ROBBER_MANA,
                "recovered"
            );
        }
        let robbing_now = (0..4).find(|&k| (ROB_ARRIVALS[k]..ROBBED[k]).contains(&i));
        assert_eq!(locked(&state, s.robber), robbing_now.is_some(), "step {i}");
        if let Some(k) = robbing_now {
            let corpse = [
                Some(s.corpses[0]),
                Some(s.corpses[1]),
                late.target_corpse,
                late.sheriff_corpse,
            ][k];
            assert_eq!(rob_timer(&state, s.robber), corpse, "step {i}: corpse {k}");
        }
        if let Some(c) = late.islander_corpse {
            // The island corpse stays (other region, C-321): robbable from the time marker
            // of the dying animation until the decay, never robbed.
            assert_eq!(
                robbable(&state, c),
                (ISLAND_CORPSE_ROBBABLE..ISLAND_CORPSE_UNROBBABLE).contains(&i),
                "step {i}: island corpse"
            );
        }
        for c in s.corpses {
            assert_eq!(exists(&state, c), i < COLONIST_CORPSES_GONE, "step {i}");
        }
        if let Some(c) = late.sheriff_corpse {
            assert_eq!(exists(&state, c), i < SHERIFF_CORPSE_GONE, "step {i}");
        }
    }
    assert_eq!(successes, [SPA_KILL, SID_KILL], "successes");
    assert!(exists(&state, s.missionary) && exists(&state, s.hq_hire));
    assert!(exists(&state, s.hq_a));
}

/// Measured (hiring, takeover, melee kill, dynamite, ranged throw, way home, robbing).
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
const HEADHUNTER_GOLDEN: u64 = 0xc5e3_4c43_407d_12d1;

/// Continuous sequence of the scene without saving, computed once per test process and shared by the
/// saving tests.
static HUNTER_SEQUENCE: LazyLock<Vec<u64>> =
    LazyLock::new(|| hunter_run_probed(None, |_, _, _, _| {}));

#[test]
fn hunter_checksums_match_the_golden_sequence() {
    let mut h = crate::rules::Fnv64::default();
    for c in HUNTER_SEQUENCE.iter() {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, HEADHUNTER_GOLDEN, "{:#018x}", h.0);
}

/// Save points of the scene (measured, as assertions in the probes): during the search
/// (timer `0x322`); between command and `0x122` (takeover); in the melee kill window; with
/// dynamite in flight; on the way home; between arrival at the corpse and `0x330`.
const SAVE_SEARCH: usize = 10;
const SAVE_HIRE: usize = TAKEOVER_AT;
const SAVE_NEAR_KILL: usize = 340;
const SAVE_DYNAMITE: usize = 363;
const SAVE_HOMEWARD: usize = 400;
const SAVE_ROBBING: usize = 120;
const _: () = assert!(HIRE_AT < SAVE_SEARCH && SAVE_SEARCH < SPA_SEARCH);
const _: () = assert!(SAVE_HIRE + 1 == TAKEOVER_HIRED);
const _: () = assert!(SPA_KILL_ARRIVAL < SAVE_NEAR_KILL && SAVE_NEAR_KILL < SPA_KILL);
const _: () = assert!(SID_LAUNCH < SAVE_DYNAMITE && SAVE_DYNAMITE < SID_IMPACT);
const _: () = assert!(SPA_KILL < SAVE_HOMEWARD && SAVE_HOMEWARD < SPA_GONE);
const _: () = assert!(ROB_ARRIVALS[0] < SAVE_ROBBING && SAVE_ROBBING < ROBBED[0]);

/// Scene with saving/loading after `save_at` against the continuous sequence.
fn assert_survives_saving(save_at: usize, at_save: HunterProbe, what: &str) {
    let saved = hunter_run_probed(Some(save_at), at_save);
    let first_diff = saved
        .iter()
        .zip(HUNTER_SEQUENCE.iter())
        .position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "saved {what}: first deviation in `step`");
}

#[test]
fn hunter_checksums_survive_saving_and_loading() {
    // One run per save point, side by side (run time, like `SPELL_GOLDEN`). Each
    // sequence equals the continuous one.
    let cases: [(usize, HunterProbe, &str); 6] = [
        (
            SAVE_SEARCH,
            |state, _, s, late| {
                let spa = late.hunter(0);
                assert_eq!(hunt_state(state, spa).0, 0);
                assert_eq!(terms(state, spa), (0, 300, 350, 1, UNDERTAKER as i32));
                assert!(pending(state, spa, hunter::INDEX, hunter::msg::THINK));
                assert!(!moving(state, spa));
                assert!(has(state, s.sheriff, status::QUARTERED));
            },
            "during the search (0x322)",
        ),
        (
            SAVE_HIRE,
            |state, _, _, late| {
                // `0x122 {0, 1113, 1}` is in the FIFO, the headhunter does not have it yet.
                let spa = late.hunter(0);
                let o = state.object(spa, false).unwrap();
                let hire = Message::new(hunter::msg::HIRE, &[0, MISSIONARY as i32, 1]);
                assert!(
                    o.fifo
                        .iter()
                        .any(|p| matches!(p, Posted::Message { msg, .. } if *msg == hire)),
                    "0x122 in the FIFO"
                );
                assert_eq!(terms(state, spa).0, 0);
                assert!(moving(state, spa));
            },
            "between command and 0x122",
        ),
        (
            SAVE_NEAR_KILL,
            |state, _, s, late| {
                let spa = late.hunter(0);
                assert!(locked(state, spa));
                assert!(pending(state, spa, hunter::INDEX, hunter::msg::KILL));
                assert_eq!(hunt_state(state, spa), (1, s.target, s.target));
            },
            "in the melee kill window",
        ),
        (
            SAVE_DYNAMITE,
            |state, _, s, late| {
                let sid = late.hunter(2);
                let [p] = throws(state).try_into().expect("one throw in flight");
                assert_eq!((p.shooter, p.target), (sid, ObjectId::NONE));
                assert_eq!(p.damage, Message::new(hunter::msg::IMPACT_BUILDING, &[0]));
                assert!(0 < p.remaining_ms && p.remaining_ms < p.flight_ms);
                assert_eq!(hunt_state(state, sid).2, s.hq_quarters);
            },
            "with dynamite in flight",
        ),
        (
            SAVE_HOMEWARD,
            |state, _, _, late| {
                let spa = late.hunter(0);
                assert!(moving(state, spa));
                assert_eq!(
                    replies(state, spa),
                    (Some(hunter::msg::VANISH), Some(hunter::msg::VANISH))
                );
                assert_eq!(terms(state, spa).0, -1);
            },
            "on the way home",
        ),
        (
            SAVE_ROBBING,
            |state, _, s, _| {
                assert!(locked(state, s.robber));
                assert_eq!(rob_timer(state, s.robber), Some(s.corpses[0]));
                assert!(robbable(state, s.corpses[0]));
                assert!(!attack_ticks(state, s.robber));
            },
            "between arrival at the corpse and 0x330",
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

// --- Action 380 from scripts (E-168, E-218) ---

use crate::command::headhunter::{COLOR_BUTTON_BASE, TEXT_NO_GOLD, hire_price};
use crate::command::{Outcome, execute};
use crate::script::actions::{Issued, MsgValue, ScriptMsg};
use crate::script::command::dispatch_command;
use crate::setup::player_color;

/// Action 380 as `actions::message` builds it: `{customer, target, GUID, kind}`.
fn n380(sim: &mut Sim, customer: i32, art: i32, target: i32, guid: i32) -> Vec<Issued> {
    let values = [customer, target, guid, art].map(MsgValue::Int).to_vec();
    dispatch_command(
        sim,
        &ScriptMsg {
            number: 380,
            values,
        },
    )
    .expect("command action 380")
}

fn issued(outcome: Outcome) -> Vec<Issued> {
    vec![Issued {
        code: 0x2C,
        outcome,
    }]
}

/// The headhunter of kind 0 (SPA 4100), if there is one.
fn spa_hunter(state: &SimState) -> Option<ObjectId> {
    hunter::current(state, 0)
}

#[test]
fn n380_hires_over_the_customers_hq_and_pays_gold() {
    // `kamp3-13` form `n380;customer;kind;target;GUID`: customer 3, kind 0, target 6, GUID 1715. Slot 3 gets
    // the nation SPA and a headquarters 1000; slot 6 is not a playing slot of the scene (the
    // script does not check the target menu). Only the customer pays, and he is the client of the headhunter.
    let (mut state, rules, _) = hunter_scene();
    state.players[3].nation = 2;
    state.players[3].set_resource(0, START_GOLD);
    let hq3 = {
        let mut sim = Sim::new(&mut state, &rules);
        spawn(&mut sim, HQ_SPA, 3, (50, 8))
    };
    let mut sim = Sim::new(&mut state, &rules);
    let price = hire_price(&sim, 0);
    let before: Vec<i32> = (0..8).map(|p| sim.state.resource(p, 0)).collect();
    assert_eq!(n380(&mut sim, 3, 0, 6, 1715), issued(Outcome::Accepted));
    let gold = |sim: &Sim| (0..8).map(|p| sim.state.resource(p, 0)).collect::<Vec<_>>();
    let mut expected = before;
    expected[3] -= price;
    assert_eq!(gold(&sim), expected, "only slot 3 pays");
    let hh = spa_hunter(sim.state).expect("headhunter created");
    let events = std::mem::take(&mut sim.state.events);
    assert_eq!(announcements(&events), hired(3, HUNTER_SPA));
    drop(sim);
    // The order `0x122` is in the FIFO of the headhunter; the headhunter stands behind the HQ of the
    // slot table and receives it after two steps.
    for _ in 0..2 {
        step(&mut state, &rules, &[]);
    }
    let h = hunter_of(&state, hh);
    assert_eq!(
        (h.customer, h.target_player, h.target_guid),
        (3, 6, 1715),
        "{h:?}"
    );
    assert!(state.object(hq3, false).is_some());
}

#[test]
fn n380_hires_where_the_bar_refuses() {
    // Action 380 with customer 0 (SPA, HQ 1000), kind 0, target 1 and the
    // GUID 1715 of an elite of the nation PRI: slot 1 plays SID, the command bar would not allow this target GUID
    // (target GUID menu); the script hires anyway (E-218).
    let (mut state, rules, s) = hunter_scene();
    let mut sim = Sim::new(&mut state, &rules);
    let price = hire_price(&sim, 0);
    assert!(price > 0);
    let menu = crate::command::headhunter::hire_targets(&sim, 0, 1);
    assert!(!menu.contains(&1715), "counter-check: {menu:?}");
    let leiste = execute(
        &mut sim,
        &Command {
            player: 0,
            kind: kit::hire_command(0, 1, 1715, s.hq_a),
        },
        true,
    );
    assert!(matches!(leiste, Outcome::Rejected(_)), "{leiste:?}");
    assert_eq!(
        sim.state.resource(0, 0),
        START_GOLD,
        "no gold for the command bar"
    );
    assert!(spa_hunter(sim.state).is_none());

    assert_eq!(n380(&mut sim, 0, 0, 1, 1715), issued(Outcome::Accepted));
    assert_eq!(
        sim.state.resource(0, 0),
        START_GOLD - price,
        "gold of the customer"
    );
    assert_eq!(
        sim.state.resource(1, 0),
        START_GOLD,
        "only the customer pays"
    );
    let hh = spa_hunter(sim.state).expect("headhunter created");
    assert_eq!(sim.object(hh).unwrap().guid, HUNTER_SPA);
    let events = std::mem::take(&mut sim.state.events);
    assert_eq!(announcements(&events), hired(0, HUNTER_SPA));
    // The order `0x122` reaches the headhunter in the next object tick: target slot 1, GUID 1715, customer 0.
    drop(sim);
    step(&mut state, &rules, &[]);
    let h = hunter_of(&state, hh);
    assert_eq!(
        (h.customer, h.target_player, h.target_guid),
        (0, 1, 1715),
        "{h:?}"
    );
}

#[test]
fn n380_without_hq_does_nothing() {
    let (mut state, rules, s) = hunter_scene();
    let before_gold = gold(&state);
    let mut sim = Sim::new(&mut state, &rules);
    // Slot 2 has no headquarters (and the nation 0 looks for 1300).
    assert_eq!(n380(&mut sim, 2, 0, 1, 2615), Vec::new());
    // Slots outside 0 to 7: silent as well (the load check of the `.abl` rejects them).
    for k in [-1, 8, 9, i32::MAX] {
        assert_eq!(n380(&mut sim, k, 0, 1, 2615), Vec::new(), "customer {k}");
    }
    assert!(spa_hunter(sim.state).is_none());
    assert!(
        sim.state
            .events
            .iter()
            .all(|e| !matches!(e, SimEvent::TextMessage { .. }))
    );
    // The headquarters falls: nothing more (slot 0 has only this one).
    crate::world::destroy_object(&mut sim, s.hq_a);
    assert_eq!(n380(&mut sim, 0, 0, 1, 2615), Vec::new());
    drop(sim);
    assert_eq!(gold(&state), before_gold);
}

#[test]
fn n380_ignores_the_button_lock() {
    // E-218: the script does not check lock, button and target player menu. The color buttons of the target player
    // 1 are locked: the command bar rejects, the script hires.
    let (mut state, rules, s) = hunter_scene();
    let mut sim = Sim::new(&mut state, &rules);
    let color = player_color(&sim, 1);
    sim.state
        .button_locks
        .insert(COLOR_BUTTON_BASE + color as u32, 1);
    let leiste = execute(
        &mut sim,
        &Command {
            player: 0,
            kind: kit::hire_command(0, 1, UNDERTAKER as i32, s.hq_a),
        },
        true,
    );
    assert!(matches!(leiste, Outcome::Rejected(_)), "{leiste:?}");
    assert!(spa_hunter(sim.state).is_none());
    assert_eq!(
        n380(&mut sim, 0, 0, 1, UNDERTAKER as i32),
        issued(Outcome::Accepted)
    );
    assert!(spa_hunter(sim.state).is_some());
    // Gold and message 10002 stay: a customer without gold gets the message and pays nothing.
    sim.state.players[0].set_resource(0, 0);
    sim.state.events.clear();
    assert_eq!(
        n380(&mut sim, 0, 1, 1, UNDERTAKER as i32),
        issued(Outcome::Accepted),
        "the gold check is in the command, not when sending"
    );
    assert_eq!(sim.state.resource(0, 0), 0);
    assert!(sim.state.events.iter().any(|e| matches!(
        e,
        SimEvent::TextMessage { to: Some(0), text, .. } if *text == TEXT_NO_GOLD
    )));
    // Kind 3 is invalid and stays so on every path.
    let out = n380(&mut sim, 0, 3, 1, UNDERTAKER as i32);
    assert!(
        matches!(
            &out[..],
            [Issued {
                code: 0x2C,
                outcome: Outcome::Rejected(_)
            }]
        ),
        "{out:?}"
    );
}

#[test]
fn n380_with_target_outside_0_to_7_hires() {
    // E-218: the target player is not checked (`0x0048A8F0`): 9, 8, -1 and `i32::MAX` also hire,
    // pay the price and enter the target player. The target search finds nothing for a slot without objects,
    // and the headhunter goes home.
    for z in [9, 8, -1, i32::MAX] {
        let (mut state, rules, _) = hunter_scene();
        let mut sim = Sim::new(&mut state, &rules);
        let price = hire_price(&sim, 0);
        assert_eq!(
            n380(&mut sim, 0, 0, z, 1715),
            issued(Outcome::Accepted),
            "target {z}"
        );
        let hh = spa_hunter(sim.state).expect("headhunter created");
        assert_eq!(sim.state.resource(0, 0), START_GOLD - price, "target {z}");
        drop(sim);
        for _ in 0..2 {
            step(&mut state, &rules, &[]);
        }
        assert_eq!(hunter_of(&state, hh).target_player, z, "target {z}");
        for _ in 0..6 {
            step(&mut state, &rules, &[]);
        }
        assert_loads(&state, &rules);
        // The search needs eleven ticks after the order.
        for _ in 0..20 {
            step(&mut state, &rules, &[]);
        }
        // No object of kind 1715 with owner z (slot 8 holds only neutral objects of other kinds): the target search
        // finds nothing, the headhunter goes home.
        assert_eq!(
            replies(&state, hh),
            (Some(hunter::msg::VANISH), Some(hunter::msg::VANISH)),
            "target {z}"
        );
        assert!(moving(&state, hh), "target {z}");
        assert_loads(&state, &rules);
    }
}

/// Command `0x2C` via the script path with the sender `sender` (without the search of 380).
fn script_hire(sim: &mut Sim, sender: u8, building: ObjectId, art: i32, target: i32) -> Outcome {
    let kind = kit::hire_command(art, target, UNDERTAKER as i32, building);
    crate::player::submit_script(
        sim,
        &Command {
            player: sender,
            kind,
        },
    )
}

#[test]
fn script_hire_without_the_menu_button_works() {
    // E-218 "button": the headquarters 1000 has no button 4100 in the menu. The command bar rejects; the
    // script hires. (`SELECTHEADHUNTER` is not one of the checked button kinds of
    // `button_locked`: a lock of the kind exists only as a missing button.)
    let (mut state, mut rules, s) = hunter_scene();
    rules.toolbar.menus.remove(&(HQ_SPA as i32));
    let mut sim = Sim::new(&mut state, &rules);
    let bar = execute(
        &mut sim,
        &Command {
            player: 0,
            kind: kit::hire_command(0, 1, UNDERTAKER as i32, s.hq_a),
        },
        true,
    );
    assert!(
        matches!(&bar, Outcome::Rejected(w) if w.contains("button")),
        "{bar:?}"
    );
    assert!(spa_hunter(sim.state).is_none());
    assert_eq!(script_hire(&mut sim, 0, s.hq_a, 0, 1), Outcome::Accepted);
    assert!(spa_hunter(sim.state).is_some());
}

#[test]
fn script_hire_over_a_foreign_building_is_paid_by_its_owner() {
    // Sender 0, building of slot 1: the command bar rejects ("not own"), the script hires. The
    // price is paid by the owner of the building, and he is the client (not the sender).
    let (mut state, rules, s) = hunter_scene();
    let mut sim = Sim::new(&mut state, &rules);
    let price = hire_price(&sim, 0);
    let bar = execute(
        &mut sim,
        &Command {
            player: 0,
            kind: kit::hire_command(0, 0, UNDERTAKER as i32, s.hq_hire),
        },
        true,
    );
    assert!(matches!(bar, Outcome::Rejected(_)), "{bar:?}");
    assert_eq!(script_hire(&mut sim, 0, s.hq_hire, 0, 0), Outcome::Accepted);
    assert_eq!(
        sim.state.resource(0, 0),
        START_GOLD,
        "the sender pays nothing"
    );
    assert_eq!(
        sim.state.resource(1, 0),
        START_GOLD - price,
        "the owner pays"
    );
    let hh = spa_hunter(sim.state).expect("headhunter created");
    let announced = announcements(&std::mem::take(&mut sim.state.events));
    assert_eq!(announced, hired(1, HUNTER_SPA), "message names the payer");
    drop(sim);
    step(&mut state, &rules, &[]);
    assert_eq!(hunter_of(&state, hh).customer, 1);
}

#[test]
fn script_hire_over_an_unfinished_building_is_not_blocked_by_the_state() {
    // Unfinished: the command bar rejects, the script path does not check the construction state (E-218). Whether the
    // headhunter is created is decided by the spawning of the building (`SPAWN_UNIT`); the gold goes in every
    // case to the owner (C-311).
    let (mut state, rules, s) = hunter_scene();
    let mut sim = Sim::new(&mut state, &rules);
    let m = sim
        .module_mut(s.hq_a, crate::module::building::INDEX)
        .expect("building module");
    m.base.flags &= !crate::module::building::flags::CONSTRUCTED_FINISHED;
    let bar = execute(
        &mut sim,
        &Command {
            player: 0,
            kind: kit::hire_command(0, 1, UNDERTAKER as i32, s.hq_a),
        },
        true,
    );
    assert!(matches!(bar, Outcome::Rejected(_)), "{bar:?}");
    assert_eq!(sim.state.resource(0, 0), START_GOLD);
    assert_eq!(script_hire(&mut sim, 0, s.hq_a, 0, 1), Outcome::Accepted);
    assert!(sim.state.resource(0, 0) < START_GOLD, "gold deducted");
}

#[test]
fn script_hire_with_a_missing_building_returns_before_paying() {
    // Without a headhunter of the kind the original looks up the building and returns before the payment
    // (re/headhunter-robbing 3.1): rejected, no gold deducted, no headhunter.
    let (mut state, rules, s) = hunter_scene();
    let mut sim = Sim::new(&mut state, &rules);
    crate::world::destroy_object(&mut sim, s.hq_a);
    let gone = s.hq_a;
    assert!(sim.object(gone).is_none());
    let out = script_hire(&mut sim, 0, gone, 0, 1);
    assert!(
        matches!(&out, Outcome::Rejected(w) if w.contains("not present")),
        "{out:?}"
    );
    assert_eq!(sim.state.resource(0, 0), START_GOLD);
    assert!(spa_hunter(sim.state).is_none());
    // With a headhunter already present the building is not looked up: the slot from the
    // ID pays the current price of the headhunter.
    let (mut state, rules, s) = hunter_scene();
    let mut sim = Sim::new(&mut state, &rules);
    assert_eq!(script_hire(&mut sim, 0, s.hq_a, 0, 1), Outcome::Accepted);
    drop(sim);
    step(&mut state, &rules, &[]);
    let mut sim = Sim::new(&mut state, &rules);
    crate::world::destroy_object(&mut sim, s.hq_a);
    assert!(sim.object(s.hq_a).is_none());
    let (before0, before7) = (sim.state.resource(0, 0), sim.state.resource(7, 0));
    assert_eq!(script_hire(&mut sim, 7, s.hq_a, 0, 1), Outcome::Accepted);
    assert!(
        sim.state.resource(0, 0) < before0,
        "slot 0 from the ID pays"
    );
    assert_eq!(sim.state.resource(7, 0), before7, "the sender pays nothing");
}

#[test]
fn script_hire_from_slot_8_or_higher_is_free() {
    // E-218: a payer from slot 8 on has no money table in the original (`0x0048A8F0`): no gold check,
    // no message 10002, no payment; headhunter, order, sound and message 10010 follow as usual.
    let (mut state, rules, s) = hunter_scene();
    let mut sim = Sim::new(&mut state, &rules);
    let hq = crate::world::change_owner(&mut sim, s.hq_hire, 8).expect("change of owner");
    assert_eq!(script_hire(&mut sim, 0, hq, 0, 0), Outcome::Accepted);
    assert_eq!(gold_of_players(&sim), [START_GOLD, START_GOLD]);
    assert!(!sim.state.events.iter().any(|e| matches!(
        e,
        SimEvent::TextMessage { text, .. } if *text == TEXT_NO_GOLD
    )));
    let hh = spa_hunter(sim.state).expect("headhunter created");
    drop(sim);
    for _ in 0..2 {
        step(&mut state, &rules, &[]);
    }
    assert_eq!(hunter_of(&state, hh).customer, 8);

    // With a headhunter present the building is not looked up: the ID names slot 31.
    // (The slot of the ID has 5 bits; 31 is not a slot table.)
    let ghost = ObjectId::new(1, 31, 1);
    let mut sim = Sim::new(&mut state, &rules);
    assert!(sim.object(ghost).is_none());
    assert_eq!(script_hire(&mut sim, 0, ghost, 0, 0), Outcome::Accepted);
    assert_eq!(gold_of_players(&sim), [START_GOLD, START_GOLD]);
    drop(sim);
    for _ in 0..8 {
        step(&mut state, &rules, &[]);
    }
    assert_eq!(hunter_of(&state, hh).customer, 31);
    assert_loads(&state, &rules);
}

fn gold_of_players(sim: &Sim) -> [i32; 2] {
    [sim.state.resource(0, 0), sim.state.resource(1, 0)]
}
