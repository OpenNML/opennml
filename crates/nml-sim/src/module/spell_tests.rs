//! Scene with `SPELL_GOLDEN` (pattern `evoke_tests`; re/spells 9, re/special-attacks 9): all spells
//! and special attacks in a real level start. Slot 0 casts at step 0 explosion (medicine man 1 on a colonist group),
//! storm (medicine man 2 on monastery A), drain mana (missionary against an enemy
//! medicine man), scout area (chief), whirlwind (smith 1 among three enemies), damage
//! building (smith 2 against monastery C) and reincarnation (medicine man 3 at a corpse);
//! after the damage of the explosion follow dynamite (patriot against building B), melee kill
//! (shadow warrior) and ranged kill (pistoleer). Saving/loading in the middle of every
//! flow.
//!
//! Setup: separate buildings for storm, dynamite and
//! hammering (each more than five tiles from each other and from the explosion point; max HP 10,000 from
//! [`kit::spell_rules`]), a third medicine man only for the reincarnation and a
//! second smith for the hammering (mana 100 each), the reincarnation early (the corpse decays
//! T + 303 ticks after death), kill targets with max HP above 450 within sight/3 of the actor, the
//! missionary at 600 of 1000 HP. The units of slot 1 get stance 0 at step 0 (no
//! retaliation); the spell world has no target priorities, nobody looks for targets on their own.

use std::sync::LazyLock;

use super::ModuleKind;
use super::ability_testkit::{self as kit, *};
use super::attack;
use super::special::{bomberman, killer, special_move};
use super::spell::{self, Spell};
use super::stealth_tests::{assert_loads, scene_rules_from};
use crate::combat::projectile::ProjectileKind;
use crate::command::{Command, CommandKind};
use crate::fog::{CLEAR_UP, EXPLORE, SEE, VisionSource};
use crate::id::{NEUTRAL_SLOT, ObjectId};
use crate::message::Message;
use crate::messaging::send;
use crate::object::status;
use crate::rules::Rules;
use crate::save::testkit::save_and_load;
use crate::setup::{MatchSetup, new_match};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::tick::step;

/// Layout (tiles). West: explosion, storm, dynamite, hammering and whirlwind in rows of
/// twelve tiles each; east: drain, melee kill, ranged kill, scout area, reincarnation. The actors of
/// slot 0 face +x (direction 0) toward their targets.
const EXPLODER_TILE: (i32, i32) = (6, 8);
/// Colonist group of slot 1; the target point of the explosion is the position of the first one.
const COLONIST_TILES: [(i32, i32); 3] = [(12, 8), (13, 8), (12, 9)];
const STORMER_TILE: (i32, i32) = (6, 20);
/// Monastery A, target and target point of the storm.
const STORM_MONASTERY_TILE: (i32, i32) = (14, 20);
const PATRIOT_TILE: (i32, i32) = (6, 32);
/// Building B (monastery), target of the dynamite.
const DYNAMITE_BUILDING_TILE: (i32, i32) = (16, 32);
const HAMMERER_TILE: (i32, i32) = (6, 44);
/// Monastery C, target of the hammering.
const HAMMER_MONASTERY_TILE: (i32, i32) = (16, 44);
/// Smith 1 with three enemies (riflemen 1403, 1000 HP) on the neighboring tiles at 0, 45
/// and 90 degrees (facing 0 degrees).
const WHIRLER_TILE: (i32, i32) = (8, 56);
const FOE_OFFSETS: [(i32, i32); 3] = [(1, 0), (1, 1), (0, 1)];
/// Missionary and the enemy medicine man (drain target).
const MISSIONARY_TILE: (i32, i32) = (34, 8);
const DRAIN_TARGET_TILE: (i32, i32) = (44, 8);
/// Shadow warrior and his target, a pure ranged fighter 1403 (bonus "close against ranged").
const SHADOW_TILE: (i32, i32) = (34, 20);
const KILL_TARGET_TILE: (i32, i32) = (40, 20);
/// Pistoleer and his target, an enemy missionary 1113 (no bonus without module 18).
const PISTOLERO_TILE: (i32, i32) = (34, 32);
const SNIPE_TARGET_TILE: (i32, i32) = (37, 38);
const CHIEF_TILE: (i32, i32) = (34, 44);
/// Tile of the vision source (scout area with the UI values 30/5000/1).
const SCOUT_TILE: (i32, i32) = (50, 50);
/// Medicine man 3 and the corpse of a colonist of slot 1 five tiles in front of him.
const REINCARNATOR_TILE: (i32, i32) = (34, 56);
const CORPSE_TILE: (i32, i32) = (39, 56);

/// Square of the tile distance.
const fn dist2(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0) * (a.0 - b.0) + (a.1 - b.1) * (a.1 - b.1)
}
/// Radius of explosion and storm (5 tiles, re/spells 1.3/1.4) and sight of the actors (shipped data).
const WORLD_EFFECT_TILES: i32 = 5;
const MEDICINE_MAN_SIGHT: i32 = 52;
const SHADOW_SIGHT: i32 = 40;
const PISTOLERO_SIGHT: i32 = 40;
// Storm, dynamite and hammering targets are each more than 5 tiles from each other and from the explosion point.
const _: () = {
    let buildings = [
        STORM_MONASTERY_TILE,
        DYNAMITE_BUILDING_TILE,
        HAMMER_MONASTERY_TILE,
    ];
    let r2 = WORLD_EFFECT_TILES * WORLD_EFFECT_TILES;
    let mut a = 0;
    while a < 3 {
        assert!(dist2(buildings[a], COLONIST_TILES[0]) > r2);
        let mut b = a + 1;
        while b < 3 {
            assert!(dist2(buildings[a], buildings[b]) > r2);
            b += 1;
        }
        a += 1;
    }
};
// In the explosion radius only the colonists: all other units of slot 1 and the corpse are farther
// than 5 tiles from the target point.
const _: () = {
    let others = [
        DRAIN_TARGET_TILE,
        KILL_TARGET_TILE,
        SNIPE_TARGET_TILE,
        CORPSE_TILE,
        (WHIRLER_TILE.0 + 1, WHIRLER_TILE.1 + 1),
    ];
    let mut k = 0;
    while k < others.len() {
        assert!(dist2(others[k], COLONIST_TILES[0]) > WORLD_EFFECT_TILES * WORLD_EFFECT_TILES);
        k += 1;
    }
};
// The corpse lies within sight/4 of the third medicine man; kill targets within
// sight/3 of the actor (`MM_METHODINDEX 3` of 548/546).
const _: () = assert!(dist2(CORPSE_TILE, REINCARNATOR_TILE) * 16 <= MEDICINE_MAN_SIGHT.pow(2));
const _: () = assert!(dist2(KILL_TARGET_TILE, SHADOW_TILE) * 9 <= SHADOW_SIGHT.pow(2));
const _: () = assert!(dist2(SNIPE_TARGET_TILE, PISTOLERO_TILE) * 9 <= PISTOLERO_SIGHT.pow(2));
// The target points of the spells lie within sight/3 (reproducible via the targeting
// mode).
const _: () = assert!(dist2(COLONIST_TILES[0], EXPLODER_TILE) * 9 <= MEDICINE_MAN_SIGHT.pow(2));
const _: () = assert!(dist2(STORM_MONASTERY_TILE, STORMER_TILE) * 9 <= MEDICINE_MAN_SIGHT.pow(2));

/// Setup steps (like `EVOKE_GOLDEN`): `0x110` to the victim, then two `step` until the
/// change of owner to slot 8.
const SETUP_STEPS: usize = 2;

/// Command times as `step` calls (two `step` are one object tick; objects of group 0
/// tick in even `step`, those of group 1 in odd `step`, projectiles in the odd
/// phase-0 step). The command runs in the `step` call with this index.
///
/// Step 0: slot 1 stance 0; slot 0 explosion, drain, scout area, storm,
/// whirlwind, damage building, reincarnation.
const START_AT: usize = 0;
/// Dynamite, melee kill, ranged kill: in the `step` after the damage of the explosion (measured).
const SPECIALS_AT: usize = 54;
/// Length of the scene in `step` calls (2400; the last flow, the explosion of the
/// dynamite, is at [`BLAST`]).
const SPELL_STEPS: usize = 2400;

/// Measured `step` calls (debug = release) after which the state first holds. The distances
/// follow the measured ticks (object ticks, `N` = tick of the start).
///
/// Explosion (medicine man 1, group 0; the facing is already right, `0x226` in tick N = step 0):
/// `0x326` in N+22 (mana 100 -> 20, entry in the list), damage in the fifth effect tick (phase-0
/// step after it), `0x426` in N+44.
const EXPLOSION_EFFECT: usize = 44;
const EXPLODED: usize = 53;
const EXPLOSION_OVER: usize = 88;
/// Storm (medicine man 2, group 1, N = step 1): `0x326` in N+22, pulses in every second
/// effect tick from N+1 (35 pulses, -2 % = -200 each), gone in N+70.
const STORM_EFFECT: usize = 45;
const FIRST_PULSE: usize = 47;
const PULSE_EVERY: usize = 4;
const PULSES: usize = 35;
const STORM_GONE: usize = 185;
const STORM_OVER: usize = 89;
/// Drain mana (missionary, group 1; the command runs synchronously in step 0): `0x326` with
/// `0xB7F` in N+22 (N = step 1): mana of the target 100 -> 0, HP 600 -> 700.
const DRAINED: usize = 45;
const DRAIN_OVER: usize = 89;
/// Scout area: vision source from the command step on, 5000 ms at 50 ms each per `step` (re/vision 4.2).
const SCOUT_GONE: usize = 100;
/// Whirlwind (smith 1, group 0, N = step 0): sectors 0, 45, 90 degrees in N+13, N+14, N+15
/// (-60 % = -600 each), end `0x431` in N+33.
const WHIRL_HITS: [usize; 3] = [26, 28, 30];
const WHIRL_END: usize = 66;
/// Hammering (smith 2, group 1): arrival N after the walk (mana 100 -> 40), hit N+1 (-40 % =
/// -4000), end N+34.
const HAMMER_ARRIVAL: usize = 147;
const HAMMER_HIT: usize = 149;
const HAMMER_END: usize = 215;
/// Reincarnation (medicine man 3, group 0, N = step 0): `0x326` in N+22 - war wolf at the
/// corpse, mana 100 -> 60, reserve 40; the corpse is destroyed in the phase-0 step after it.
const REINCARNATED: usize = 44;
const CORPSE_GONE: usize = 45;
const REINCARNATION_OVER: usize = 88;
/// Melee kill (shadow warrior, group 0): arrival N (knife animation 5), hit N+11 (-300 * 1.5
/// = -450, mana 200 -> 150).
const KILL_ARRIVAL: usize = 126;
const KILL_HIT: usize = 148;
/// Ranged kill (pistoleer, group 1): turn, fire animation in N, shot N+17 (-300,
/// mana 200 -> 160), end N+38.
const SNIPE_AIM: usize = 57;
const SNIPE_SHOT: usize = 91;
const SNIPE_END: usize = 133;
/// Dynamite (patriot, group 1): laying from the arrival N, charge in N+54 (mana 100 -> 30, way back),
/// explosion 99 ticks later (-4000 -20 % = -3200); the patriot stands on his tile again.
const LAYING: usize = 149;
const CHARGE_PLACED: usize = 257;
const BLAST: usize = 455;
const PATRIOT_HOME: usize = 345;
// The measured distances follow the ticks (one tick = 2 `step`).
const _: () = assert!(EXPLOSION_EFFECT - START_AT == 2 * 22);
const _: () = assert!(EXPLODED - EXPLOSION_EFFECT == 2 * 4 + 1);
const _: () = assert!(EXPLOSION_OVER - START_AT == 2 * 44);
const _: () = assert!(STORM_EFFECT - (START_AT + 1) == 2 * 22);
const _: () = assert!(FIRST_PULSE - STORM_EFFECT == 2 && PULSE_EVERY == 2 * 2);
const _: () = assert!(STORM_GONE - STORM_EFFECT == 2 * 70);
const _: () = assert!(FIRST_PULSE + (PULSES - 1) * PULSE_EVERY < STORM_GONE);
const _: () = assert!(FIRST_PULSE + PULSES * PULSE_EVERY > STORM_GONE);
const _: () = assert!(STORM_OVER - (START_AT + 1) == 2 * 44);
const _: () = assert!(DRAINED - (START_AT + 1) == 2 * 22 && DRAIN_OVER == STORM_OVER);
const _: () = assert!(SCOUT_GONE - START_AT == 5000 / 50);
const _: () = assert!(WHIRL_HITS[0] - START_AT == 2 * 13 && WHIRL_HITS[2] - START_AT == 2 * 15);
const _: () = assert!(WHIRL_HITS[1] - START_AT == 2 * 14 && WHIRL_END - START_AT == 2 * 33);
const _: () = assert!(HAMMER_HIT - HAMMER_ARRIVAL == 2 && HAMMER_END - HAMMER_ARRIVAL == 2 * 34);
const _: () = assert!(REINCARNATED - START_AT == 2 * 22 && CORPSE_GONE == REINCARNATED + 1);
const _: () = assert!(REINCARNATION_OVER - START_AT == 2 * 44);
const _: () = assert!(SPECIALS_AT == EXPLODED + 1 && SPECIALS_AT < KILL_ARRIVAL);
const _: () = assert!(KILL_HIT - KILL_ARRIVAL == 2 * 11);
const _: () = assert!(SPECIALS_AT < SNIPE_AIM);
const _: () = assert!(SNIPE_SHOT - SNIPE_AIM == 2 * 17 && SNIPE_END - SNIPE_AIM == 2 * 38);
const _: () = assert!(SPECIALS_AT < LAYING && CHARGE_PLACED - LAYING == 2 * 54);
const _: () = assert!(BLAST - CHARGE_PLACED == 2 * 99);
const _: () = assert!(CHARGE_PLACED < PATRIOT_HOME && BLAST < SPELL_STEPS);
// The reincarnation takes effect long before the decay of the corpse (T + 303 ticks after death,
// dying.rs; the death is [`SETUP_STEPS`] before step 0).
const _: () = assert!(SETUP_STEPS + REINCARNATED < 2 * 300);

/// IDs of the scene (setup). The corpse carries its ID on slot 8.
struct SpellScene {
    exploder: ObjectId,
    stormer: ObjectId,
    reincarnator: ObjectId,
    missionary: ObjectId,
    chief: ObjectId,
    patriot: ObjectId,
    shadow: ObjectId,
    pistolero: ObjectId,
    whirler: ObjectId,
    hammerer: ObjectId,
    colonists: [ObjectId; 3],
    storm_monastery: ObjectId,
    dynamite_building: ObjectId,
    hammer_monastery: ObjectId,
    drain_target: ObjectId,
    kill_target: ObjectId,
    snipe_target: ObjectId,
    foes: [ObjectId; 3],
    corpse: ObjectId,
}

impl SpellScene {
    /// The units of slot 1 (stance 0 at step 0).
    fn enemy_units(&self) -> Vec<ObjectId> {
        let mut v = self.colonists.to_vec();
        v.extend([self.drain_target, self.kill_target, self.snipe_target]);
        v.extend(self.foes);
        v
    }
}

/// IDs that do not arise until the run: the war wolf, kept right after the `step` in which
/// it arises; it survives saving/loading.
#[derive(Default)]
struct Late {
    wolf: Option<ObjectId>,
}

impl Late {
    /// After the `step` call `i`.
    fn observe(&mut self, state: &SimState, i: usize) {
        if i == REINCARNATED {
            let wolves: Vec<ObjectId> = state
                .objects()
                .filter(|o| o.guid == BATTLE_WOLF)
                .map(|o| o.id)
                .collect();
            assert_eq!(wolves.len(), 1, "one war wolf");
            self.wolf = Some(wolves[0]);
        }
    }
}

/// Rules of the scene: the spell world ([`kit::spell_rules`], monastery with 10,000 HP)
/// with two human players in the `.lvl` like the scenes of `STEALTH_GOLDEN` and
/// `EVOKE_GOLDEN` ([`scene_rules_from`]).
fn scene_rules() -> Rules {
    scene_rules_from(kit::spell_rules())
}

/// Hit points and extra energy (mana).
const HP: u32 = 14;
const MANA: u32 = 15;

fn set_mana(sim: &mut Sim, id: ObjectId, v: i32) {
    let p = spell::mana_percent(sim, id);
    sim.object_mut(id)
        .expect("object")
        .attributes
        .set_current_value(MANA, v, p);
}

fn set_hp(sim: &mut Sim, id: ObjectId, v: i32) {
    let o = sim.object(id).expect("object");
    let p = sim.type_upgrade_percent(o.owner(), o.guid, HP);
    sim.object_mut(id)
        .expect("object")
        .attributes
        .set_current_value(HP, v, p);
}

/// Mana of the actors of slot 0 and of the drain target after the setup.
const START_MANA: [i32; 11] = [100, 100, 100, 200, 100, 100, 200, 200, 100, 100, 100];

/// Scene via a real level start (`setup::new_match`, mode -1), map revealed; slots 0 and
/// 1 are enemies of each other. Setup in a fixed order: 1. research (slot 0: all
/// [`SPELL_UPGRADE_GUIDS`]), 2. all objects (actors facing +x), 3. kill the victim and
/// [`SETUP_STEPS`] steps until the change of owner, 4. set mana and the HP of the missionary, 5.
/// then step 0 of the sequence.
fn spell_scene() -> (SimState, Rules, SpellScene) {
    let rules = scene_rules();
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).expect("level start");
    crate::fog::reveal_map(&mut state, -1);
    assert!(state.diplomacy.is_enemy(0, 1) && state.diplomacy.is_enemy(1, 0));
    for &guid in SPELL_UPGRADE_GUIDS {
        unlock(&mut state, 0, guid);
    }
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        let exploder = spawn(&mut sim, MEDICINE_MAN, 0, EXPLODER_TILE);
        let stormer = spawn(&mut sim, MEDICINE_MAN, 0, STORMER_TILE);
        let reincarnator = spawn(&mut sim, MEDICINE_MAN, 0, REINCARNATOR_TILE);
        let missionary = spawn(&mut sim, MISSIONARY, 0, MISSIONARY_TILE);
        let chief = spawn(&mut sim, CHIEF, 0, CHIEF_TILE);
        let patriot = spawn(&mut sim, PATRIOT, 0, PATRIOT_TILE);
        let shadow = spawn(&mut sim, SHADOW_WARRIOR, 0, SHADOW_TILE);
        let pistolero = spawn(&mut sim, PISTOLERO, 0, PISTOLERO_TILE);
        let whirler = spawn(&mut sim, SMITH, 0, WHIRLER_TILE);
        let hammerer = spawn(&mut sim, SMITH, 0, HAMMERER_TILE);
        for id in [
            exploder,
            stormer,
            reincarnator,
            missionary,
            shadow,
            pistolero,
            whirler,
            hammerer,
        ] {
            crate::world::set_direction(&mut sim, id, 0);
        }
        let colonists = COLONIST_TILES.map(|t| spawn(&mut sim, COLONIST, 1, t));
        let storm_monastery = spawn(&mut sim, MONASTERY, 1, STORM_MONASTERY_TILE);
        let dynamite_building = spawn(&mut sim, MONASTERY, 1, DYNAMITE_BUILDING_TILE);
        let hammer_monastery = spawn(&mut sim, MONASTERY, 1, HAMMER_MONASTERY_TILE);
        let drain_target = spawn(&mut sim, MEDICINE_MAN, 1, DRAIN_TARGET_TILE);
        let kill_target = spawn(&mut sim, RANGED, 1, KILL_TARGET_TILE);
        let snipe_target = spawn(&mut sim, MISSIONARY, 1, SNIPE_TARGET_TILE);
        let foes = FOE_OFFSETS.map(|(dx, dz)| {
            let t = (WHIRLER_TILE.0 + dx, WHIRLER_TILE.1 + dz);
            spawn(&mut sim, RANGED, 1, t)
        });
        let victim = spawn(&mut sim, COLONIST, 1, CORPSE_TILE);
        send(&mut sim, victim, &Message::new(0x110, &[]));
        let mut events = Vec::new();
        for _ in 0..SETUP_STEPS {
            events.extend(step(sim.state, sim.rules, &[]));
        }
        let corpse = events
            .iter()
            .find_map(|e| match e {
                SimEvent::OwnerChanged { old, new } if *old == victim => Some(*new),
                _ => None,
            })
            .expect("corpse on slot 8");
        let manned = [
            exploder,
            stormer,
            reincarnator,
            missionary,
            chief,
            patriot,
            shadow,
            pistolero,
            whirler,
            hammerer,
            drain_target,
        ];
        for (id, mana) in manned.into_iter().zip(START_MANA) {
            set_mana(&mut sim, id, mana);
        }
        set_hp(&mut sim, missionary, 600);
        SpellScene {
            exploder,
            stormer,
            reincarnator,
            missionary,
            chief,
            patriot,
            shadow,
            pistolero,
            whirler,
            hammerer,
            colonists,
            storm_monastery,
            dynamite_building,
            hammer_monastery,
            drain_target,
            kill_target,
            snipe_target,
            foes,
            corpse,
        }
    };
    (state, rules, scene)
}

/// World position of the tile center at height 0.
fn at_tile(t: (i32, i32)) -> [f32; 3] {
    [world(t.0), 0.0, world(t.1)]
}

/// `0x36` with the target point `p` as `f32` bits (like the UI for 480/481).
fn cast_at(caster: ObjectId, art: i32, p: [f32; 3], button: i32) -> CommandKind {
    CommandKind::CastSpell {
        caster,
        art,
        anim: p[0].to_bits() as i32,
        idx: p[2].to_bits() as i32,
        button,
    }
}

/// Commands in the `step` call `i` (buttons and values like the command bar).
fn spell_orders(i: usize, s: &SpellScene) -> Vec<Command> {
    let cmd = |player, kind| Command { player, kind };
    match i {
        START_AT => vec![
            cmd(
                1,
                CommandKind::SetBehavior {
                    units: s.enemy_units(),
                    mode: 0,
                },
            ),
            cmd(0, cast_at(s.exploder, 1, at_tile(COLONIST_TILES[0]), 480)),
            cmd(
                0,
                CommandKind::DrainMana {
                    unit: s.missionary,
                    target: s.drain_target,
                    button: 541,
                },
            ),
            cmd(
                0,
                CommandKind::ScoutArea {
                    unit: s.chief,
                    x: SCOUT_TILE.0,
                    z: SCOUT_TILE.1,
                    size: 30,
                    duration_ms: 5000,
                    flag: 1,
                    button: 484,
                },
            ),
            cmd(0, cast_at(s.stormer, 2, at_tile(STORM_MONASTERY_TILE), 481)),
            cmd(
                0,
                CommandKind::Whirlwind {
                    unit: s.whirler,
                    button: 542,
                },
            ),
            cmd(
                0,
                CommandKind::DamageBuilding {
                    unit: s.hammerer,
                    target: s.hammer_monastery,
                    button: 551,
                },
            ),
            cmd(
                0,
                CommandKind::CastSpell {
                    caster: s.reincarnator,
                    art: 3,
                    anim: 0,
                    idx: 0,
                    button: 482,
                },
            ),
        ],
        SPECIALS_AT => vec![
            cmd(
                0,
                CommandKind::DynamiteBuilding {
                    unit: s.patriot,
                    target: s.dynamite_building,
                    button: 454,
                },
            ),
            cmd(
                0,
                CommandKind::KillUnit {
                    unit: s.shadow,
                    target: s.kill_target,
                    button: 548,
                },
            ),
            cmd(
                0,
                CommandKind::SnipeUnit {
                    unit: s.pistolero,
                    target: s.snipe_target,
                    button: 546,
                },
            ),
        ],
        _ => vec![],
    }
}

/// State assertion at the time of saving: state, rules, IDs of the scene and of the run.
type SpellProbe = fn(&mut SimState, &Rules, &SpellScene, &Late);

/// Checksum per `step` call of the scene. With `save_at = Some(i)` the state is saved after the `step` call `i`,
/// checked against the rules (like the viewer when loading) and loaded again; `at_save`
/// sees the state immediately before.
fn spell_run_probed(save_at: Option<usize>, at_save: SpellProbe) -> Vec<u64> {
    let (mut state, rules, scene) = spell_scene();
    let mut late = Late::default();
    let mut out = Vec::with_capacity(SPELL_STEPS);
    for i in 0..SPELL_STEPS {
        let orders = spell_orders(i, &scene);
        step(&mut state, &rules, &orders);
        late.observe(&state, i);
        if save_at == Some(i) {
            at_save(&mut state, &rules, &scene, &late);
            assert_loads(&state, &rules);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

fn exists(state: &SimState, id: ObjectId) -> bool {
    state.object(id, false).is_some()
}

fn has(state: &SimState, id: ObjectId, bits: u32) -> bool {
    state.object(id, false).expect("object").status & bits == bits
}

fn locked(state: &SimState, id: ObjectId) -> bool {
    has(state, id, status::ANIM_LOCKED)
}

/// Value of the attribute with the upgrade percentages of the owner (like `GetCurrentValue`).
fn value(state: &mut SimState, rules: &Rules, id: ObjectId, attr: u32) -> i32 {
    Sim::new(state, rules).attribute(id, attr)
}

fn spell_state(state: &SimState, id: ObjectId) -> Spell {
    match &state
        .object(id, false)
        .unwrap()
        .module(spell::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::Spell(s) => (**s).clone(),
        k => panic!("no module 38: {k:?}"),
    }
}

/// Is a timer `code` pending in module `index` (`ModuleBase::has_pending_timer`)?
fn pending(state: &SimState, id: ObjectId, index: usize, code: u32) -> bool {
    state
        .object(id, false)
        .unwrap()
        .module(index)
        .unwrap()
        .base
        .has_pending_timer(code)
}

/// Entries of kind `kind` in the projectile list: remaining time, pulse counter or animation, position.
fn world_effects(state: &SimState, kind: ProjectileKind) -> Vec<(i32, i32, [f32; 3])> {
    state
        .projectiles
        .list
        .iter()
        .filter(|p| p.kind == kind)
        .map(|p| (p.remaining_ms, p.zone, p.target_pos))
        .collect()
}

fn busy_bomber(state: &mut SimState, rules: &Rules, id: ObjectId) -> bool {
    let sim = Sim::new(state, rules);
    bomberman::bomberman(&sim, id).expect("module 36").busy
}

fn busy_smith(state: &mut SimState, rules: &Rules, id: ObjectId) -> bool {
    let sim = Sim::new(state, rules);
    special_move::special_move(&sim, id)
        .expect("module 49")
        .busy
}

/// Vision sources on the tile `tile`.
fn sources_at(state: &SimState, tile: (i32, i32)) -> Vec<VisionSource> {
    state
        .fog
        .sources
        .iter()
        .filter(|s| (s.x, s.z_or_hid) == tile)
        .cloned()
        .collect()
}

/// Pulses of the storm up to and including `step` `i`.
fn pulses(i: usize) -> i32 {
    if i < FIRST_PULSE {
        0
    } else {
        (((i - FIRST_PULSE) / PULSE_EVERY + 1).min(PULSES)) as i32
    }
}

/// Expected hit points after `step` `i`: targets with their hits, all other units
/// unharmed.
fn expected_hp(s: &SpellScene, i: usize) -> Vec<(ObjectId, i32, &'static str)> {
    let hit = |at: usize, before: i32, after: i32| if i < at { before } else { after };
    let mut v = vec![
        (s.storm_monastery, 10_000 - 200 * pulses(i), "monastery A"),
        (s.dynamite_building, hit(BLAST, 10_000, 6800), "building B"),
        (
            s.hammer_monastery,
            hit(HAMMER_HIT, 10_000, 6000),
            "monastery C",
        ),
        (s.drain_target, 800, "drain target"),
        (s.kill_target, hit(KILL_HIT, 1000, 550), "melee kill target"),
        (
            s.snipe_target,
            hit(SNIPE_SHOT, 1000, 700),
            "ranged kill target",
        ),
        (s.missionary, hit(DRAINED, 600, 700), "missionary"),
    ];
    for &c in &s.colonists {
        v.push((c, hit(EXPLODED, 160, 40), "colonist"));
    }
    for (k, &f) in s.foes.iter().enumerate() {
        v.push((f, hit(WHIRL_HITS[k], 1000, 400), "enemy of the smith"));
    }
    for (id, hp) in [
        (s.exploder, 800),
        (s.stormer, 800),
        (s.reincarnator, 800),
        (s.chief, 1000),
        (s.patriot, 1500),
        (s.shadow, 1200),
        (s.pistolero, 1200),
        (s.whirler, 1200),
        (s.hammerer, 1200),
    ] {
        v.push((id, hp, "actor"));
    }
    v
}

#[test]
fn the_spell_scene_shows_every_state() {
    // Counter-check before the golden checksums, every value individually (all times are `step` calls, measured):
    // explosion -75 % on the colonists, mana 20; storm 35 pulses on the monastery; drain: target 0,
    // missionary +100 HP; vision source 30 x 30 for 5 s; dynamite -3200 on the monastery, patriot back at
    // the start position; kill -450 (bonus "close against ranged") / -300; whirlwind three hits -60 %;
    // hammering -40 %; war wolf 3420 at the corpse, reserve 40.
    let (mut state, rules, s) = spell_scene();
    let managed = [
        s.exploder,
        s.stormer,
        s.reincarnator,
        s.missionary,
        s.chief,
        s.patriot,
        s.shadow,
        s.pistolero,
        s.whirler,
        s.hammerer,
        s.drain_target,
    ];
    for (id, mana) in managed.into_iter().zip(START_MANA) {
        assert_eq!(value(&mut state, &rules, id, MANA), mana);
    }
    assert_eq!(s.corpse.slot(), NEUTRAL_SLOT);
    assert!(has(&state, s.corpse, status::DIED));
    // The target point of the explosion is the position of the first colonist (height 0).
    let target = {
        let sim = Sim::new(&mut state, &rules);
        crate::combat::hit_zone::world_pos(&sim, s.colonists[0]).unwrap()
    };
    let explosion_at = at_tile(COLONIST_TILES[0]);
    assert_eq!([target[0], target[2]], [explosion_at[0], explosion_at[2]]);
    let storm_at = at_tile(STORM_MONASTERY_TILE);
    let corpse_pos = {
        let g = state.object(s.corpse, false).unwrap().grid;
        let w = crate::world::grid_to_world(g, rules.map.cell);
        [w[0], 0.0, w[2]]
    };
    let mut late = Late::default();
    for i in 0..SPELL_STEPS {
        // Preconditions at the command times.
        if i == SPECIALS_AT {
            assert!(world_effects(&state, ProjectileKind::Explosion).is_empty());
            assert_eq!(value(&mut state, &rules, s.colonists[0], HP), 40);
        }
        let orders = spell_orders(i, &s);
        step(&mut state, &rules, &orders);
        late.observe(&state, i);

        // Slot 1 has stance 0 (no retaliation).
        if i == START_AT {
            let quiet = crate::module::quarter::attack_mode_msg(0, false);
            let sim = Sim::new(&mut state, &rules);
            for u in s.enemy_units() {
                if let Some(a) = attack::attack(&sim, u) {
                    assert_eq!((a.aggressiveness, a.mode), (quiet.arg(0), quiet.arg(1)));
                }
            }
        }

        // Hit points of everyone involved.
        for (id, hp, what) in expected_hp(&s, i) {
            assert_eq!(value(&mut state, &rules, id, HP), hp, "step {i}: {what}");
        }

        // Explosion: cast in step 0 (target point as `f32`), effect N+22 (mana 20), damage in the
        // fifth effect tick; lock until N+44.
        if i == START_AT {
            let sp = spell_state(&state, s.exploder);
            assert_eq!((sp.art, sp.button, sp.cost), (1, 480, 80));
            assert_eq!(sp.target, explosion_at);
        }
        if i < EXPLOSION_EFFECT {
            assert_eq!(value(&mut state, &rules, s.exploder, MANA), 100, "step {i}");
        }
        if i == EXPLOSION_EFFECT {
            assert_eq!(value(&mut state, &rules, s.exploder, MANA), 20);
        }
        let explosions = world_effects(&state, ProjectileKind::Explosion);
        if (EXPLOSION_EFFECT..EXPLODED).contains(&i) {
            assert_eq!(explosions.len(), 1, "step {i}");
            assert_eq!(explosions[0].2, explosion_at);
        } else {
            assert!(explosions.is_empty(), "step {i}");
        }
        assert_eq!(locked(&state, s.exploder), i < EXPLOSION_OVER, "step {i}");

        // Storm: effect N+22 (mana 20), 35 pulses of -200 each on monastery A (see `expected_hp`),
        // gone in N+70; buildings B and C lie outside.
        if i == START_AT + 1 {
            let sp = spell_state(&state, s.stormer);
            assert_eq!((sp.art, sp.button, sp.target), (2, 481, storm_at));
        }
        if i == STORM_EFFECT {
            assert_eq!(value(&mut state, &rules, s.stormer, MANA), 20);
        }
        let storms = world_effects(&state, ProjectileKind::Storm);
        if (STORM_EFFECT..STORM_GONE).contains(&i) {
            assert_eq!(storms.len(), 1, "step {i}");
            assert_eq!(storms[0].2, storm_at);
        } else {
            assert!(storms.is_empty(), "step {i}");
        }
        assert_eq!(
            locked(&state, s.stormer),
            (START_AT + 1..STORM_OVER).contains(&i),
            "step {i}"
        );
        if i == STORM_GONE {
            assert_eq!(pulses(i), 35);
            assert_eq!(value(&mut state, &rules, s.storm_monastery, HP), 3000);
        }

        // Drain mana: the whole mana of the target becomes HP of the missionary (+100), cost 50.
        let drained = value(&mut state, &rules, s.drain_target, MANA);
        if i < DRAINED {
            assert_eq!(drained, 100, "step {i}");
        }
        if i == DRAINED {
            assert_eq!(drained, 0);
            assert_eq!(value(&mut state, &rules, s.missionary, MANA), 150);
        }
        assert_eq!(locked(&state, s.missionary), i < DRAIN_OVER, "step {i}");

        // Scout area: mana 100 -> 40 in the command step, vision source 30 (quarter tiles) for 5 s.
        if i == START_AT {
            assert_eq!(value(&mut state, &rules, s.chief, MANA), 40);
        }
        let sources = sources_at(&state, SCOUT_TILE);
        if i < SCOUT_GONE {
            let source = VisionSource {
                x: SCOUT_TILE.0,
                z_or_hid: SCOUT_TILE.1,
                size: 30,
                player: 0,
                remaining_ms: 5000 - 50 * (i as i32 + 1),
                flags: EXPLORE | SEE | CLEAR_UP,
            };
            assert_eq!(sources, [source], "step {i}");
        } else {
            assert!(sources.is_empty(), "step {i}");
        }

        // Whirlwind: cost 75 in the command (mana 25), three hits, end N+33.
        if i == START_AT {
            assert_eq!(value(&mut state, &rules, s.whirler, MANA), 25);
        }
        assert_eq!(
            busy_smith(&mut state, &rules, s.whirler),
            i < WHIRL_END,
            "step {i}"
        );

        // Hammering: mana not until arrival (100 -> 40), hit N+1, end N+34.
        if i < HAMMER_ARRIVAL {
            assert_eq!(value(&mut state, &rules, s.hammerer, MANA), 100, "step {i}");
        }
        if i == HAMMER_ARRIVAL {
            assert_eq!(value(&mut state, &rules, s.hammerer, MANA), 40);
        }
        assert_eq!(
            busy_smith(&mut state, &rules, s.hammerer),
            (HAMMER_ARRIVAL..HAMMER_END).contains(&i),
            "step {i}"
        );

        // Reincarnation: corpse remembered at the cast, war wolf in N+22 at the corpse, mana 60,
        // reserve 40; the corpse is destroyed.
        if i == START_AT {
            let sp = spell_state(&state, s.reincarnator);
            assert_eq!((sp.art, sp.button, sp.corpse), (3, 482, s.corpse));
            assert_eq!(sp.target, corpse_pos);
        }
        let reserve = spell_state(&state, s.reincarnator).reserve;
        assert_eq!(reserve, if i < REINCARNATED { 0 } else { 40 }, "step {i}");
        if i == REINCARNATED {
            assert_eq!(value(&mut state, &rules, s.reincarnator, MANA), 60);
            let wolf = late.wolf.expect("war wolf");
            let w = state.object(wolf, false).unwrap();
            assert_eq!((w.guid, wolf.slot()), (BATTLE_WOLF, 0));
            assert_eq!(w.tile, CORPSE_TILE, "at the corpse");
            assert_eq!(
                (w.world_pos[0], w.world_pos[2]),
                (corpse_pos[0], corpse_pos[2])
            );
        }
        assert_eq!(exists(&state, s.corpse), i < CORPSE_GONE, "step {i}");
        let wolves = state.objects().filter(|o| o.guid == BATTLE_WOLF).count();
        assert_eq!(wolves, usize::from(i >= REINCARNATED), "step {i}");
        assert_eq!(
            locked(&state, s.reincarnator),
            i < REINCARNATION_OVER,
            "step {i}"
        );

        // Melee kill: lock from the arrival to the hit, mana not until the hit (-50).
        assert_eq!(
            locked(&state, s.shadow),
            (KILL_ARRIVAL..KILL_HIT).contains(&i),
            "step {i}"
        );
        if i == KILL_ARRIVAL {
            assert!(pending(
                &state,
                s.shadow,
                killer::INDEX,
                killer::msg::KILL_HIT
            ));
        }
        if i < KILL_HIT {
            assert_eq!(value(&mut state, &rules, s.shadow, MANA), 200, "step {i}");
        }
        if i == KILL_HIT {
            assert_eq!(value(&mut state, &rules, s.shadow, MANA), 150);
        }

        // Ranged kill: lock from the fire animation to the end, mana at the shot (-40).
        assert_eq!(
            locked(&state, s.pistolero),
            (SNIPE_AIM..SNIPE_END).contains(&i),
            "step {i}"
        );
        if i < SNIPE_SHOT {
            assert_eq!(
                value(&mut state, &rules, s.pistolero, MANA),
                200,
                "step {i}"
            );
        }
        if i == SNIPE_SHOT {
            assert_eq!(value(&mut state, &rules, s.pistolero, MANA), 160);
        }

        // Dynamite: laying, charge in N+54 (mana 30), explosion 99 ticks later, back to the
        // start.
        assert_eq!(
            busy_bomber(&mut state, &rules, s.patriot),
            (LAYING..CHARGE_PLACED).contains(&i),
            "step {i}"
        );
        if i < CHARGE_PLACED {
            assert_eq!(value(&mut state, &rules, s.patriot, MANA), 100, "step {i}");
        }
        if i == CHARGE_PLACED {
            assert_eq!(value(&mut state, &rules, s.patriot, MANA), 30);
        }
        let charges = world_effects(&state, ProjectileKind::Dynamite);
        assert_eq!(
            charges.len(),
            usize::from((CHARGE_PLACED..BLAST).contains(&i)),
            "step {i}"
        );
        let patriot_tile = state.object(s.patriot, false).unwrap().tile;
        if i == LAYING {
            assert_ne!(patriot_tile, PATRIOT_TILE, "at the building");
        }
        if i == PATRIOT_HOME - 1 {
            assert_ne!(patriot_tile, PATRIOT_TILE, "still on the way");
        }
        if i >= PATRIOT_HOME {
            assert_eq!(
                patriot_tile, PATRIOT_TILE,
                "step {i}: back at the start position"
            );
        }
    }
}

/// Measured (explosion, storm, drain mana, scout area, dynamite, melee/ranged kill, whirlwind,
/// hammering, reincarnation).
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
const SPELL_GOLDEN: u64 = 0x5561_8d31_b9df_8dc8;

/// Continuous sequence of the scene without saving, computed once per test process and shared by the saving tests.
static SPELL_SEQUENCE: LazyLock<Vec<u64>> =
    LazyLock::new(|| spell_run_probed(None, |_, _, _, _| {}));

#[test]
fn spell_checksums_match_the_golden_sequence() {
    let mut h = crate::rules::Fnv64::default();
    for c in SPELL_SEQUENCE.iter() {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, SPELL_GOLDEN, "{:#018x}", h.0);
}

/// Save points of the scene (measured, as an assertion in the probes): between cast and `0x326`
/// (target point `f32`); in the middle of the explosion remaining time; in the middle of the storm
/// (pulse 0 and 1); during laying; during the fuse time; between arrival and hit of the killer; between two
/// whirlwind sectors; after the reincarnation (reserve).
const SAVE_CASTING: usize = 20;
const SAVE_EXPLOSION: usize = 48;
const SAVE_STORM_ZONE_0: usize = 61;
const SAVE_STORM_ZONE_1: usize = 63;
const SAVE_LAYING: usize = 200;
const SAVE_FUSE: usize = 300;
const SAVE_KILLER: usize = 135;
const SAVE_WHIRL: usize = 27;
const SAVE_REINCARNATED: usize = 100;
const _: () = assert!(START_AT < SAVE_CASTING && SAVE_CASTING < EXPLOSION_EFFECT);
const _: () = assert!(EXPLOSION_EFFECT < SAVE_EXPLOSION && SAVE_EXPLOSION < EXPLODED);
const _: () = assert!(FIRST_PULSE < SAVE_STORM_ZONE_0 && SAVE_STORM_ZONE_1 < STORM_GONE);
const _: () = assert!(SAVE_STORM_ZONE_0 + 2 == SAVE_STORM_ZONE_1);
const _: () = assert!(LAYING < SAVE_LAYING && SAVE_LAYING < CHARGE_PLACED);
const _: () = assert!(CHARGE_PLACED < SAVE_FUSE && SAVE_FUSE < BLAST);
const _: () = assert!(KILL_ARRIVAL < SAVE_KILLER && SAVE_KILLER < KILL_HIT);
const _: () = assert!(WHIRL_HITS[0] < SAVE_WHIRL && SAVE_WHIRL < WHIRL_HITS[1]);
const _: () = assert!(REINCARNATED < SAVE_REINCARNATED);

/// Scene with saving/loading after `save_at` against the continuous sequence.
fn assert_survives_saving(save_at: usize, at_save: SpellProbe, what: &str) {
    let saved = spell_run_probed(Some(save_at), at_save);
    let first_diff = saved
        .iter()
        .zip(SPELL_SEQUENCE.iter())
        .position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "saved {what}: first deviation in `step`");
}

/// Storm entry in the pulse step `zone` (0: pulse in the next effect tick, 1: pause).
fn storm_probe(state: &SimState, s: &SpellScene, zone: i32) {
    let storms = world_effects(state, ProjectileKind::Storm);
    assert_eq!(storms.len(), 1);
    let (rest, z, at) = storms[0];
    assert!(0 < rest && rest < 7000, "remaining {rest}");
    assert_eq!((z, at), (zone, at_tile(STORM_MONASTERY_TILE)));
    assert_eq!(
        state.projectiles.list[0].shooter, s.stormer,
        "caster remembered"
    );
}

#[test]
fn spell_checksums_survive_saving_and_loading() {
    // One run per save point, side by side (run time, like `EVOKE_GOLDEN`). Each
    // sequence equals the continuous one.
    let cases: [(usize, SpellProbe, &str); 9] = [
        (
            SAVE_CASTING,
            |state, _, s, _| {
                // Target point remembered as `f32`, `0x326` is pending, no entry yet.
                assert!(locked(state, s.exploder));
                let sp = spell_state(state, s.exploder);
                assert_eq!((sp.art, sp.target), (1, at_tile(COLONIST_TILES[0])));
                assert!(pending(state, s.exploder, spell::INDEX, spell::msg::EFFECT));
                assert!(state.projectiles.list.is_empty());
            },
            "between cast and 0x326",
        ),
        (
            SAVE_EXPLOSION,
            |state, _, _, _| {
                let explosions = world_effects(state, ProjectileKind::Explosion);
                assert_eq!(explosions.len(), 1);
                let rest = explosions[0].0;
                assert!(0 < rest && rest < 500, "remaining {rest}");
            },
            "in the middle of the explosion remaining time",
        ),
        (
            SAVE_STORM_ZONE_0,
            |state, _, s, _| storm_probe(state, s, 0),
            "in the middle of the storm (pulse 0)",
        ),
        (
            SAVE_STORM_ZONE_1,
            |state, _, s, _| storm_probe(state, s, 1),
            "in the middle of the storm (pulse 1)",
        ),
        (
            SAVE_LAYING,
            |state, rules, s, _| {
                assert!(busy_bomber(state, rules, s.patriot));
                assert!(locked(state, s.patriot));
                assert!(pending(
                    state,
                    s.patriot,
                    bomberman::INDEX,
                    bomberman::msg::FIRE_BUILDING
                ));
            },
            "during laying",
        ),
        (
            SAVE_FUSE,
            |state, rules, s, _| {
                assert!(!busy_bomber(state, rules, s.patriot));
                let charges = world_effects(state, ProjectileKind::Dynamite);
                assert_eq!(charges.len(), 1);
                assert!(0 < charges[0].0 && charges[0].0 < 10_000);
                let p = &state.projectiles.list[0];
                assert_eq!((p.shooter, p.target), (s.patriot, s.dynamite_building));
            },
            "during the fuse time",
        ),
        (
            SAVE_KILLER,
            |state, rules, s, _| {
                assert!(locked(state, s.shadow));
                assert!(pending(
                    state,
                    s.shadow,
                    killer::INDEX,
                    killer::msg::KILL_HIT
                ));
                let sim = Sim::new(state, rules);
                assert_eq!(
                    killer::killer(&sim, s.shadow).unwrap().target,
                    s.kill_target
                );
            },
            "between arrival and hit of the killer",
        ),
        (
            SAVE_WHIRL,
            |state, rules, s, _| {
                assert!(busy_smith(state, rules, s.whirler));
                assert!(pending(
                    state,
                    s.whirler,
                    special_move::INDEX,
                    special_move::msg::HIT
                ));
                assert_eq!(value(state, rules, s.foes[0], HP), 400);
                assert_eq!(value(state, rules, s.foes[1], HP), 1000);
            },
            "between two whirlwind sectors",
        ),
        (
            SAVE_REINCARNATED,
            |state, _, s, late| {
                assert_eq!(spell_state(state, s.reincarnator).reserve, 40);
                assert!(exists(state, late.wolf.expect("war wolf")));
            },
            "after the reincarnation",
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
