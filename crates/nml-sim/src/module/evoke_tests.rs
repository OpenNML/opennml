//! Scene with `EVOKE_GOLDEN` (pattern `stealth_tests`; re/ghosts-banish 9, re/evoke-aura 9): the
//! ghost evoker 1411 of slot 0 evokes ghost A from the nearest corpse, A sucks two colonists of
//! slot 1 dry one after the other (attack commands) and dies after 50 suck strikes. A second
//! evocation is aborted in the window `0x21A`...`0x31A` (C-258): ghost B fades out and dies, the
//! costs come back twice. The ghost of the enemy 1411 of slot 1 is banished. The animal evoker 1412
//! calls two wolves; their auras raise the melee attack of the warrior 1400 and the ranged attack of
//! the ranged fighter 1403 once (not stacked, C-288). A warrior of slot 1 kills the first wolf: the
//! reserve is freed, the aura of the second stays (gap of up to three ticks). A real level start
//! without original data; saving/loading in every phase.
//!
//! The test world is [`kit::evoke_rules`]: without priorities no unit seeks targets itself;
//! the colonists have no attack module, the wolf is set to stance 0.

use std::sync::LazyLock;

use super::ability_testkit::{self as kit, *};
use super::evoke_animal::{self, EvokeAnimal};
use super::exec_temp::{self, ExecTempUpgrade};
use super::ghosts::banish;
use super::ghosts::evoke::{self, EvokeGhost};
use super::ghosts::ghost::{self, Ghost};
use super::stealth_tests::{assert_loads, scene_rules_from};
use super::{ModuleKind, spell};
use crate::command::{Command, CommandKind};
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

/// Setup (tiles; created objects face -z, the corpses lie in front of them). Slot 0: the ghost
/// evoker with its two corpses two and three tiles in front of it; the corpse of slot 1
/// five tiles away (ghost within the banish radius 26,624, six and a half tiles). Slot 1: its
/// ghost evoker three tiles behind its corpse (the corpses of slot 0 lie >= 8.9
/// tiles away); the colonists 5.4 tiles from the evoker of slot 0, thus behind its second
/// corpse (C-256: every evoker finds its own as the nearest, even after the death of
/// colonist 1).
const EVOKER_TILE: (i32, i32) = (20, 20);
const CORPSE_TILES: [(i32, i32); 3] = [(20, 18), (20, 17), (24, 23)];
const ENEMY_EVOKER_TILE: (i32, i32) = (24, 26);
const COLONIST_TILES: [(i32, i32); 2] = [(18, 15), (22, 15)];
/// Animal evoker with warrior and ranged fighter beside it (aura radius five tiles, re/evoke-aura 9 V7);
/// the warrior of slot 1 twelve tiles away.
const ANIMAL_EVOKER_TILE: (i32, i32) = (40, 40);
const WARRIOR_TILE: (i32, i32) = (42, 40);
const RANGED_TILE: (i32, i32) = (38, 40);
const ENEMY_WARRIOR_TILE: (i32, i32) = (40, 52);

/// Setup steps: `0x110` to the victims, then two `step` until the owner change to slot 8
/// (phase 0 of the object tick).
const SETUP_STEPS: usize = 2;

/// Command times as `step` calls (not object ticks: two `step` are one tick); the command
/// runs in the `step` call with this index.
///
/// `0x1F` Evoke ghost 1411 (slot 0 and 1), `0x22` Evoke wolf 1412.
const START_AT: usize = 0;
/// Slot 0: let ghost A attack colonist 1, after its `0x41B`.
const FIRST_ATTACK_AT: usize = 10;
/// Slot 0: second `0x1F`, in the `step` after the `0x31A` of the first evocation.
const SECOND_EVOKE_AT: usize = 81;
/// Slot 0: halt in the window `0x21A`...`0x31A` (C-258), after B has appeared (`0x41B`; an
/// earlier halt would let B reappear via `0x41B`, C-265).
const HALT_AT: usize = 92;
/// Slot 0: Banish 547, after the halt (neither `EVOKING` nor `ANIM_LOCKED`).
const BANISH_AT: usize = 93;
/// Slot 0: ghost A on colonist 2, in the `step` after the owner change of colonist 1.
const SECOND_ATTACK_AT: usize = 118;
/// Slot 0: second wolf, in the `step` after the `0x220` of the first.
const SECOND_WOLF_AT: usize = 164;
/// Slot 0: wolf 1 to stance 0 (no retaliation), one step before the attack.
const BEHAVIOR_AT: usize = 400;
/// Slot 1: the warrior attacks wolf 1.
const ENEMY_ATTACK_AT: usize = 401;
/// Length of the scene in `step` calls.
const EVOKE_STEPS: usize = 3000;
const _: () = assert!(START_AT < FIRST_ATTACK_AT && FIRST_ATTACK_AT < SECOND_EVOKE_AT);
const _: () = assert!(SECOND_EVOKE_AT < HALT_AT && HALT_AT < BANISH_AT);
const _: () = assert!(BANISH_AT < SECOND_ATTACK_AT && SECOND_ATTACK_AT < SECOND_WOLF_AT);
const _: () = assert!(SECOND_WOLF_AT < BEHAVIOR_AT && BEHAVIOR_AT + 1 == ENEMY_ATTACK_AT);

/// Measured `step` calls (debug = release) after which the state holds for the first time.
///
/// First evocation (both 1411 already face the corpse, `0x41A` synchronous): `EVOKING` and
/// base value 125 in the first `step` (tick N), ghost in N+3 (`0x21A`), appeared (`0x41B`, 0 ms,
/// C-262) in the tick after, `0x31A` in N+40.
const EVOKED: usize = 0;
const GHOST_A_CREATED: usize = 6;
const GHOST_A_APPEARED: usize = 9;
const FIRST_EVOKE_OVER: usize = 80;
/// Second evocation: tick N' in the `step` after the command, ghost B in N'+3, appeared.
const SECOND_EVOKED: usize = 82;
const GHOST_B_CREATED: usize = 88;
const GHOST_B_APPEARED: usize = 91;
/// Sucking: A flies to colonist 1, first suck strike; colonist 1 dead after 32 hits, ownership to
/// slot 8; first suck strike on colonist 2.
const FIRST_SUCK: usize = 53;
const COLONIST_GONE: usize = 117;
const SECOND_TARGET_SUCK: usize = 161;
/// Banishing: mana immediately (tick N''), banishment in N''+16 (1,500 ms).
const BANISH_PAID: usize = 94;
const ENEMY_GHOST_BANNED: usize = 126;
/// Deaths of the ghosts: B dies first, 34 ticks after the halt (base value 150), then A after the
/// 50th suck strike (175); removed after the fade-out.
const GHOST_B_DEAD: usize = 159;
const GHOST_A_DEAD: usize = 195;
const GHOST_A_REMOVED: usize = 263;
const ENEMY_GHOST_REMOVED: usize = 195;
/// Animal evocation: tick N in the second `step` (tick group of the 1412), wolf 1 in N+81 at `spawn`;
/// the second wolf (`0x22` in the `step` after) 81 ticks later again.
const ANIMAL_EVOKED: usize = 1;
const WOLF1_CREATED: usize = 163;
const SECOND_WOLF_EVOKED: usize = 165;
const WOLF2_CREATED: usize = 327;
/// First aura tick of wolf 1 (E+3).
const FIRST_AURA: usize = 169;
/// Wolf 1: owner change to slot 8 after the last hit of the warrior, callback `0x320` in the
/// death tick after; the aura of wolf 2 raises melee and ranged again in the next tick of wolf 2.
const WOLF1_GONE: usize = 1411;
const WOLF1_RELEASED: usize = 1413;
/// Hits of the warrior of slot 1 on wolf 1 (measured): the first in `step` 595, then
/// alternately 10 and 16 `step` later (26 `step` per pair = 13 ticks); the damage alternates
/// between -2 and -3 HP (fraction -1,250 or -1,875 at 160 HP). The 64th hit kills in
/// [`WOLF1_GONE`].
const WARRIOR_FIRST_HIT: usize = 595;
const WARRIOR_HIT_PAIR: usize = 26;
const WARRIOR_SECOND_HIT: usize = 10;
const WARRIOR_HITS: usize = 64;
const WARRIOR_DAMAGE: [i32; 2] = [-2, -3];
const AURA_BACK: usize = 1414;
/// The three corpses are removed (`0x210` after 30 s, 29 ticks before; re/evoke-aura 3.10, T+303 and
/// T+332): all evocations lie before it.
const CORPSES_REMOVED: usize = 661;
// The measured distances follow the timer rule (N + floor(d/100) + 1 ticks, one tick = 2 `step`).
const _: () = assert!(GHOST_A_CREATED - EVOKED == 2 * 3 && FIRST_EVOKE_OVER - EVOKED == 2 * 40);
const _: () = assert!(GHOST_B_CREATED - SECOND_EVOKED == 2 * 3);
const _: () = assert!(ENEMY_GHOST_BANNED - BANISH_PAID == 2 * 16);
const _: () = assert!(WOLF1_CREATED - ANIMAL_EVOKED == 2 * 81);
const _: () = assert!(WOLF2_CREATED - SECOND_WOLF_EVOKED == 2 * 81);
const _: () = assert!(FIRST_AURA - WOLF1_CREATED == 2 * 3);
const _: () = assert!(AURA_BACK - WOLF1_GONE <= 2 * 3);
const _: () = assert!(ENEMY_ATTACK_AT < WARRIOR_FIRST_HIT);
const _: () = assert!(
    WARRIOR_FIRST_HIT + (WARRIOR_HITS / 2 - 1) * WARRIOR_HIT_PAIR + WARRIOR_SECOND_HIT
        == WOLF1_GONE
);
const _: () = assert!(GHOST_B_APPEARED < HALT_AT && HALT_AT < SECOND_EVOKED + 2 * 40);
const _: () = assert!(GHOST_B_DEAD < GHOST_A_DEAD);
const _: () = assert!(SECOND_EVOKED < CORPSES_REMOVED - 2 * 29);

/// IDs of the scene (setup). The corpses carry the IDs on slot 8.
struct EvokeScene {
    evoker: ObjectId,
    animal_evoker: ObjectId,
    warrior: ObjectId,
    ranged: ObjectId,
    enemy_evoker: ObjectId,
    colonists: [ObjectId; 2],
    enemy_warrior: ObjectId,
    corpses: [ObjectId; 3],
}

/// IDs that do not arise until the run: noted directly after the `step` in which they
/// arise; they survive saving/loading.
#[derive(Default)]
struct Late {
    ghost_a: Option<ObjectId>,
    ghost_b: Option<ObjectId>,
    enemy_ghost: Option<ObjectId>,
    wolves: Vec<ObjectId>,
}

impl Late {
    fn ghost_a(&self) -> ObjectId {
        self.ghost_a.expect("ghost A")
    }

    fn wolf1(&self) -> ObjectId {
        self.wolves[0]
    }

    /// After the `step` call `i`.
    fn observe(&mut self, state: &SimState, s: &EvokeScene, i: usize) {
        match i {
            GHOST_A_CREATED => {
                self.ghost_a = Some(evoke_state(state, s.evoker).ghost);
                self.enemy_ghost = Some(evoke_state(state, s.enemy_evoker).ghost);
            }
            GHOST_B_CREATED => self.ghost_b = Some(evoke_state(state, s.evoker).ghost),
            WOLF1_CREATED | WOLF2_CREATED => {
                let wolf = state
                    .objects()
                    .find(|o| o.guid == WOLF && o.owner() == 0 && !self.wolves.contains(&o.id))
                    .expect("wolf on slot 0")
                    .id;
                self.wolves.push(wolf);
            }
            _ => {}
        }
    }
}

/// Rules of the scene: the evocation world ([`kit::evoke_rules`]) with two human players in
/// the `.lvl` like the scene of `STEALTH_GOLDEN` ([`scene_rules_from`]). In addition the time markers of
/// combat animation 14 of the warrior as in `combat::testkit` (re/combat-melee 8.1: marker 2 = hit
/// after 560 ms): [`kit::evoke_rules`] does not have them, without a hit marker the warrior of
/// slot 1 would never strike.
fn scene_rules() -> Rules {
    let mut r = scene_rules_from(kit::evoke_rules());
    r.set_anim_labels(WARRIOR, 14, &[(1, 1283), (2, 560), (0x16, 1096)]);
    r
}

/// Hit points, extra energy (mana), melee, ranged.
const HP: u32 = 14;
const MANA: u32 = 15;
const NEAR: u32 = 19;
const FAR: u32 = 20;
/// Buttons "Evoke ghost", "Banish ghosts" (1411) and "Evoke wolf" (1412).
const EVOKE_BUTTON: i32 = 436;
const BANISH_BUTTON: i32 = 547;
const WOLF_BUTTON: i32 = 441;

fn set_mana(sim: &mut Sim, id: ObjectId, v: i32) {
    let p = spell::mana_percent(sim, id);
    sim.object_mut(id)
        .expect("object")
        .attributes
        .set_current_value(MANA, v, p);
}

/// Scene via a real level start (`setup::new_match`, mode -1), map revealed; slots 0 and
/// 1 are enemies of each other. Setup in a fixed order: 1. research (slot 0: 2926, 2930,
/// 2931; slot 1: 2930, otherwise `check_ability` silently rejects its `0x1F`), 2. all objects, 3. kill the
/// victims and run [`SETUP_STEPS`] steps until the owner change, 4. set the mana (1411: 150,
/// 1412: 100), 5. then step 0 of the sequence.
fn evoke_scene() -> (SimState, Rules, EvokeScene) {
    let rules = scene_rules();
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).expect("level start");
    crate::fog::reveal_map(&mut state, -1);
    assert!(state.diplomacy.is_enemy(0, 1) && state.diplomacy.is_enemy(1, 0));
    for guid in [
        EVOKE_WOLF_UPGRADE,
        EVOKE_GHOST_UPGRADE,
        BANISH_GHOSTS_UPGRADE,
    ] {
        unlock(&mut state, 0, guid);
    }
    unlock(&mut state, 1, EVOKE_GHOST_UPGRADE);
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        let evoker = spawn(&mut sim, GHOST_EVOKER, 0, EVOKER_TILE);
        let animal_evoker = spawn(&mut sim, ANIMAL_EVOKER, 0, ANIMAL_EVOKER_TILE);
        let warrior = spawn(&mut sim, WARRIOR, 0, WARRIOR_TILE);
        let ranged = spawn(&mut sim, RANGED, 0, RANGED_TILE);
        let enemy_evoker = spawn(&mut sim, GHOST_EVOKER, 1, ENEMY_EVOKER_TILE);
        let colonists = COLONIST_TILES.map(|t| spawn(&mut sim, COLONIST, 1, t));
        let enemy_warrior = spawn(&mut sim, WARRIOR, 1, ENEMY_WARRIOR_TILE);
        let victims = CORPSE_TILES.map(|t| spawn(&mut sim, COLONIST, 1, t));
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
        for (id, mana) in [(evoker, 150), (enemy_evoker, 150), (animal_evoker, 100)] {
            set_mana(&mut sim, id, mana);
        }
        EvokeScene {
            evoker,
            animal_evoker,
            warrior,
            ranged,
            enemy_evoker,
            colonists,
            enemy_warrior,
            corpses,
        }
    };
    (state, rules, scene)
}

fn attack(units: Vec<ObjectId>, target: ObjectId) -> CommandKind {
    CommandKind::Attack {
        units,
        target,
        pos: None,
        mods: 0,
    }
}

/// Commands in the `step` call `i`.
fn evoke_orders(i: usize, s: &EvokeScene, late: &Late) -> Vec<Command> {
    let cmd = |player, kind| Command { player, kind };
    let evoke = |unit| CommandKind::EvokeGhost {
        unit,
        button: EVOKE_BUTTON,
    };
    let wolf = || CommandKind::EvokeAnimal {
        unit: s.animal_evoker,
        art: 0,
        button: WOLF_BUTTON,
    };
    match i {
        START_AT => vec![
            cmd(0, evoke(s.evoker)),
            cmd(0, wolf()),
            cmd(1, evoke(s.enemy_evoker)),
        ],
        FIRST_ATTACK_AT => vec![cmd(0, attack(vec![late.ghost_a()], s.colonists[0]))],
        SECOND_EVOKE_AT => vec![cmd(0, evoke(s.evoker))],
        HALT_AT => vec![cmd(
            0,
            CommandKind::Halt {
                units: vec![s.evoker],
            },
        )],
        BANISH_AT => vec![cmd(
            0,
            CommandKind::Banish {
                unit: s.evoker,
                button: BANISH_BUTTON,
            },
        )],
        SECOND_ATTACK_AT => vec![cmd(0, attack(vec![late.ghost_a()], s.colonists[1]))],
        SECOND_WOLF_AT => vec![cmd(0, wolf())],
        BEHAVIOR_AT => vec![cmd(
            0,
            CommandKind::SetBehavior {
                units: vec![late.wolf1()],
                mode: 0,
            },
        )],
        ENEMY_ATTACK_AT => vec![cmd(1, attack(vec![s.enemy_warrior], late.wolf1()))],
        _ => vec![],
    }
}

/// State assertion at the time of saving: state, rules, IDs of the scene and of the run.
type EvokeProbe = fn(&mut SimState, &Rules, &EvokeScene, &Late);

/// Checksum per `step` call of the scene. With `save_at = Some(i)` it saves after the `step` call `i`,
/// checks against the rules (like the viewer when loading) and reloads; `at_save`
/// sees the state immediately before.
fn evoke_run_probed(save_at: Option<usize>, at_save: EvokeProbe) -> Vec<u64> {
    let (mut state, rules, scene) = evoke_scene();
    let mut late = Late::default();
    let mut out = Vec::with_capacity(EVOKE_STEPS);
    for i in 0..EVOKE_STEPS {
        let orders = evoke_orders(i, &scene, &late);
        step(&mut state, &rules, &orders);
        late.observe(&state, &scene, i);
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

fn dead(state: &SimState, id: ObjectId) -> bool {
    has(state, id, status::DIED)
}

/// Value of the attribute with the upgrade percentages of the owner (like `GetCurrentValue`).
fn value(state: &mut SimState, rules: &Rules, id: ObjectId, attr: u32) -> i32 {
    Sim::new(state, rules).attribute(id, attr)
}

/// `(current, max)` of the hit points.
fn hitpoints(state: &mut SimState, rules: &Rules, id: ObjectId) -> (i32, i32) {
    let sim = Sim::new(state, rules);
    let o = sim.object(id).expect("object");
    let p = sim.type_upgrade_percent(o.owner(), o.guid, HP);
    (
        o.attributes.current_value(HP, p),
        o.attributes.max_value(HP, p),
    )
}

/// Base value of the mana (the mana binding of the ghosts, re/ghosts-banish 3.1).
fn base_mana(state: &SimState, id: ObjectId) -> i32 {
    state
        .object(id, false)
        .expect("object")
        .attributes
        .base(MANA)
}

fn evoke_state(state: &SimState, id: ObjectId) -> EvokeGhost {
    match &state
        .object(id, false)
        .unwrap()
        .module(evoke::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::EvokeGhost(e) => e.clone(),
        k => panic!("not module 26: {k:?}"),
    }
}

fn ghost_state(state: &SimState, id: ObjectId) -> Ghost {
    match &state
        .object(id, false)
        .unwrap()
        .module(ghost::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::Ghost(g) => g.clone(),
        k => panic!("not module 27: {k:?}"),
    }
}

fn animal_state(state: &SimState, id: ObjectId) -> EvokeAnimal {
    match &state
        .object(id, false)
        .unwrap()
        .module(evoke_animal::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::EvokeAnimal(e) => e.clone(),
        k => panic!("not module 32: {k:?}"),
    }
}

fn exec_state(state: &SimState, id: ObjectId) -> ExecTempUpgrade {
    match &state
        .object(id, false)
        .unwrap()
        .module(exec_temp::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::ExecTempUpgrade(e) => e.clone(),
        k => panic!("not module 44: {k:?}"),
    }
}

/// Is a timer `code` pending in the module `index` (`ModuleBase::has_pending_timer`)?
fn pending(state: &SimState, id: ObjectId, index: usize, code: u32) -> bool {
    state
        .object(id, false)
        .unwrap()
        .module(index)
        .unwrap()
        .base
        .has_pending_timer(code)
}

/// Grid position `[x, y, z]`.
fn grid(state: &SimState, id: ObjectId) -> [i32; 3] {
    let g = state.object(id, false).expect("object").grid;
    [g.x.0, g.y.0, g.z.0]
}

/// Alpha sequence of the fade-out: per tick `a = a - 0.03` in `f32` (re/ghosts-banish 9.2).
fn alpha_after(k: usize) -> f32 {
    let mut a = 1.0_f32;
    for _ in 0..k {
        a -= ghost::ALPHA_STEP;
    }
    a
}

/// Expected base value of the mana at the evoker of slot 0 after `step` `i` (re/ghosts-banish 3.1,
/// C-257, C-258; order of the deaths).
fn expected_base(i: usize) -> i32 {
    match i {
        _ if i < SECOND_EVOKED => 125,
        _ if i < HALT_AT => 100,
        _ if i < GHOST_B_DEAD => 125,
        _ if i < GHOST_A_DEAD => 150,
        _ => 175,
    }
}

/// Expected reserve of the animal evoker after `step` `i` (re/evoke-aura 3.1, 3.10).
fn expected_reserve(i: usize) -> i32 {
    match i {
        _ if i < ANIMAL_EVOKED => 0,
        _ if i < SECOND_WOLF_EVOKED => 25,
        _ if i < WOLF1_RELEASED => 50,
        _ => 25,
    }
}

/// The aura of wolf 1 or 2 holds after `step` `i` (after the death of wolf 1 it is absent until the next
/// tick of wolf 2).
fn aura_on(i: usize) -> bool {
    (FIRST_AURA..WOLF1_GONE).contains(&i) || i >= AURA_BACK
}

#[test]
fn the_evoke_scene_shows_every_state() {
    // Counter-check before the golden (all times are `step` calls, measured): ghost A of slot 0
    // after N+3, base value 125; second evocation 100, abort in the window 125, death of the
    // faded-out ghost B 150, death of A (after 50 suck strikes) 175 - B dies first;
    // every value individually. Ghosts suck the colonists (-5/-5); banishment hits only the enemy
    // ghost; wolf in N+81 at spawn with 160 HP; warrior melee 14, ranged fighter ranged 17; two wolves -> 14;
    // death of the first wolf -> reserve free, aura of the second stays (gap <= 3 ticks).
    let (mut state, rules, s) = evoke_scene();
    // Mana is set only after the setup steps; base value of the 1411 still 150 before step 0.
    for id in [s.evoker, s.enemy_evoker] {
        assert_eq!(base_mana(&state, id), 150);
    }
    for (id, mana) in [
        (s.evoker, 150),
        (s.enemy_evoker, 150),
        (s.animal_evoker, 100),
    ] {
        assert_eq!(value(&mut state, &rules, id, MANA), mana);
    }
    for c in s.corpses {
        assert_eq!(c.slot(), NEUTRAL_SLOT);
        assert!(dead(&state, c));
    }
    let mut late = Late::default();
    let mut spawn_pos = None;
    let mut hp_a = 250;
    let mut colonist_hp = [160, 160];
    let mut sucks = [0, 0];
    let mut fade_ticks = 0;
    let mut last_alpha = 1.0_f32;
    let mut wolf1_hp = 0;
    let mut warrior_hits = 0;
    let mut banish_mana = 0;
    for i in 0..EVOKE_STEPS {
        // Preconditions at the command times.
        match i {
            FIRST_ATTACK_AT => {
                let a = late.ghost_a();
                assert_eq!(ghost_state(&state, a).status, ghost::state::IDLE);
                assert!(!has(&state, a, status::ANIM_LOCKED));
            }
            SECOND_EVOKE_AT => {
                assert!(!has(&state, s.evoker, status::EVOKING));
                assert_eq!(evoke_state(&state, s.evoker).ghost, late.ghost_a());
            }
            HALT_AT => {
                let b = late.ghost_b.expect("ghost B");
                assert!(has(&state, s.evoker, status::EVOKING));
                assert_eq!(evoke_state(&state, s.evoker).ghost, b);
                assert_eq!(ghost_state(&state, b).status, ghost::state::IDLE);
            }
            BANISH_AT => {
                assert!(!has(&state, s.evoker, status::EVOKING));
                assert!(!has(&state, s.evoker, status::ANIM_LOCKED));
                let e = late.enemy_ghost.expect("enemy ghost");
                assert!(
                    exists(&state, e) && !dead(&state, e),
                    "ghost of slot 1 present"
                );
                // In the banish radius: sight 52 * 4096 >> 3 = 26,624 (re/ghosts-banish 8 no. 11).
                let (g, b) = (grid(&state, e), grid(&state, s.evoker));
                let (dx, dz) = (i64::from(g[0] - b[0]), i64::from(g[2] - b[2]));
                assert!(dx * dx + dz * dz <= 26_624 * 26_624);
                banish_mana = value(&mut state, &rules, s.evoker, MANA);
            }
            SECOND_ATTACK_AT => {
                assert!(!exists(&state, s.colonists[0]));
                assert!(!dead(&state, s.colonists[1]));
            }
            SECOND_WOLF_AT => {
                assert!(!has(&state, s.animal_evoker, status::EVOKING));
                assert_eq!(late.wolves.len(), 1);
            }
            BEHAVIOR_AT | ENEMY_ATTACK_AT => {
                assert!(!dead(&state, late.wolf1()));
                assert_eq!(late.wolves.len(), 2);
            }
            _ => {}
        }
        let orders = evoke_orders(i, &s, &late);
        step(&mut state, &rules, &orders);
        late.observe(&state, &s, i);

        // Evoker of slot 0: base value, `EVOKING`, corpses (C-256).
        assert_eq!(base_mana(&state, s.evoker), expected_base(i), "step {i}");
        let evoking =
            (EVOKED..FIRST_EVOKE_OVER).contains(&i) || (SECOND_EVOKED..HALT_AT).contains(&i);
        assert_eq!(has(&state, s.evoker, status::EVOKING), evoking, "step {i}");
        let body = evoke_state(&state, s.evoker).dead_body;
        let expected_body = if i < SECOND_EVOKED {
            s.corpses[0]
        } else {
            s.corpses[1]
        };
        assert_eq!(body, expected_body, "step {i}");
        if i == EVOKED {
            assert_eq!(evoke_state(&state, s.enemy_evoker).dead_body, s.corpses[2]);
        }
        // Slot 1: base value 125 until the banishment of its ghost.
        let enemy_base = if i < ENEMY_GHOST_BANNED { 125 } else { 150 };
        assert_eq!(base_mana(&state, s.enemy_evoker), enemy_base, "step {i}");

        // Ghost A: appears in N+3, sucks colonist 1 until death (32 hits), then colonist 2,
        // dies after 50 hits (-5/-5 per hit, in the same `step`).
        let a_exists = (GHOST_A_CREATED..GHOST_A_REMOVED).contains(&i);
        if i < GHOST_A_CREATED {
            // Before N+3 neither of the two evokers has created a ghost.
            assert!(late.ghost_a.is_none() && late.enemy_ghost.is_none());
            for ev in [s.evoker, s.enemy_evoker] {
                assert_eq!(evoke_state(&state, ev).ghost, ObjectId::NONE, "step {i}");
            }
            assert!(!state.objects().any(|o| o.guid == GHOST), "step {i}");
        } else {
            let a = late.ghost_a();
            assert_ne!(a, ObjectId::NONE);
            assert_eq!(exists(&state, a), a_exists, "step {i}");
        }
        if a_exists {
            let a = late.ghost_a();
            let g = ghost_state(&state, a);
            if i == GHOST_A_CREATED {
                assert_eq!(grid(&state, a), evoke_state(&state, s.evoker).dead_body_pos);
            }
            if (GHOST_A_CREATED..GHOST_A_APPEARED).contains(&i) {
                assert_eq!(g.status, ghost::state::APPEARING, "step {i}");
            }
            if (GHOST_A_APPEARED..FIRST_SUCK).contains(&i) {
                assert_eq!(g.status, ghost::state::IDLE, "step {i}");
            }
            let now = if dead(&state, a) {
                0
            } else {
                value(&mut state, &rules, a, HP)
            };
            let mut hit = None;
            for (k, &c) in s.colonists.iter().enumerate() {
                let hp = if exists(&state, c) {
                    value(&mut state, &rules, c, HP)
                } else {
                    0
                };
                if hp != colonist_hp[k] {
                    assert_eq!(colonist_hp[k] - hp, 5, "step {i}: colonist {k}");
                    assert!(hit.is_none());
                    hit = Some(k);
                    sucks[k] += 1;
                    colonist_hp[k] = hp;
                }
            }
            if now != hp_a {
                assert_eq!(hp_a - now, 5, "step {i}: ghost A");
                assert!(hit.is_some(), "step {i}: hit without a target");
                hp_a = now;
            } else {
                assert!(hit.is_none(), "step {i}: target without self-deduction");
            }
            if i == FIRST_SUCK {
                assert_eq!((sucks, g.target), ([1, 0], s.colonists[0]));
            }
            if i == SECOND_TARGET_SUCK {
                assert_eq!((sucks, g.target), ([32, 1], s.colonists[1]));
            }
            assert_eq!(dead(&state, a), i >= GHOST_A_DEAD, "step {i}");
        }
        assert_eq!(
            exists(&state, s.colonists[0]),
            i < COLONIST_GONE,
            "step {i}"
        );
        for c in s.corpses {
            assert_eq!(exists(&state, c), i < CORPSES_REMOVED, "step {i}");
        }
        if i == COLONIST_GONE {
            assert_eq!(sucks[0], 32, "colonist 1 dead after 32 hits");
        }
        if i == GHOST_A_DEAD {
            assert_eq!(sucks, [32, 18], "A dead after 50 hits");
            assert_eq!(colonist_hp[1], 70);
        }

        // Ghost B: appears, fades out after the halt (alpha per tick -0.03 in `f32`), dies.
        if let Some(b) = late.ghost_b {
            assert_eq!(exists(&state, b), i < GHOST_B_DEAD, "step {i}");
            if exists(&state, b) {
                let g = ghost_state(&state, b);
                assert_eq!(value(&mut state, &rules, b, HP), 250, "B is not banished");
                if i < GHOST_B_APPEARED {
                    assert_eq!(g.status, ghost::state::APPEARING, "step {i}");
                } else if i < HALT_AT {
                    assert_eq!(g.status, ghost::state::IDLE, "step {i}");
                } else {
                    assert_eq!(g.status, ghost::state::FADING, "step {i}");
                    if g.alpha != last_alpha {
                        fade_ticks += 1;
                        assert_eq!(g.alpha.to_bits(), alpha_after(fade_ticks).to_bits());
                        last_alpha = g.alpha;
                    }
                }
            }
            if i == GHOST_B_DEAD {
                assert_eq!(fade_ticks, 33, "34th tick: alpha 0, death, removed");
            }
        }

        // Banishment: mana immediately, after 1,500 ms only the enemy ghost that was seen.
        if i == BANISH_PAID {
            assert_eq!(value(&mut state, &rules, s.evoker, MANA), banish_mana - 25);
            assert!(pending(&state, s.evoker, banish::INDEX, banish::msg::NOW));
        }
        if let Some(e) = late.enemy_ghost {
            assert_eq!(exists(&state, e), i < ENEMY_GHOST_REMOVED, "step {i}");
            if exists(&state, e) {
                assert_eq!(dead(&state, e), i >= ENEMY_GHOST_BANNED, "step {i}");
                let hp = value(&mut state, &rules, e, HP);
                let expected = if i < ENEMY_GHOST_BANNED { 250 } else { 0 };
                assert_eq!(hp, expected, "step {i}");
            }
        }

        // Animal evocation: reserve, wolf in N+81 exactly at `spawn` with 160 HP (C-284).
        if i == ANIMAL_EVOKED {
            assert!(has(&state, s.animal_evoker, status::EVOKING));
            spawn_pos = Some(animal_state(&state, s.animal_evoker).spawn);
        }
        assert_eq!(
            animal_state(&state, s.animal_evoker).reserve,
            expected_reserve(i),
            "step {i}"
        );
        if i == WOLF1_CREATED {
            let w = late.wolf1();
            assert_eq!(Some(grid(&state, w)), spawn_pos, "wolf exactly at spawn");
            assert_eq!(hitpoints(&mut state, &rules, w), (160, 160));
            assert!(!has(&state, s.animal_evoker, status::EVOKING));
            wolf1_hp = 160;
        }
        if i == WOLF2_CREATED {
            assert_eq!(hitpoints(&mut state, &rules, late.wolves[1]), (160, 160));
        }
        let wolves = usize::from(i >= WOLF1_CREATED) + usize::from(i >= WOLF2_CREATED);
        assert_eq!(late.wolves.len(), wolves, "step {i}");
        if i >= WOLF1_CREATED {
            let w = late.wolf1();
            assert_eq!(exists(&state, w), i < WOLF1_GONE, "step {i}");
            if exists(&state, w) {
                // Only the hits of the warrior take hit points.
                let hp = value(&mut state, &rules, w, HP);
                assert!(hp <= wolf1_hp, "step {i}");
                if i <= ENEMY_ATTACK_AT {
                    assert_eq!(hp, 160, "step {i}");
                }
                if hp != wolf1_hp {
                    let k = warrior_hits;
                    let at =
                        WARRIOR_FIRST_HIT + k / 2 * WARRIOR_HIT_PAIR + (k % 2) * WARRIOR_SECOND_HIT;
                    assert_eq!(i, at, "hit {k} of the warrior");
                    assert_eq!(hp - wolf1_hp, WARRIOR_DAMAGE[k % 2], "hit {k}, step {i}");
                    warrior_hits += 1;
                }
                wolf1_hp = hp;
            }
        }
        if i == WOLF1_GONE {
            // The last hit kills: the 63 before it are visible.
            assert_eq!(warrior_hits, WARRIOR_HITS - 1);
            let corpse = state
                .objects()
                .find(|o| o.guid == WOLF && o.owner() == NEUTRAL_SLOT)
                .expect("carcass of wolf 1")
                .id;
            assert!(dead(&state, corpse));
            assert!(!dead(&state, late.wolves[1]));
        }

        // Auras: melee 9 -> 14, ranged 11 -> 17; two wolves do not stack (14, not 19).
        let (near, far) = if aura_on(i) { (14, 17) } else { (9, 11) };
        assert_eq!(value(&mut state, &rules, s.warrior, NEAR), near, "step {i}");
        assert_eq!(value(&mut state, &rules, s.ranged, FAR), far, "step {i}");
        if i == AURA_BACK {
            let w2 = late.wolves[1];
            let set = exec_state(&state, w2).influenced.expect("set");
            assert!(set.contains(&s.warrior) && set.contains(&s.ranged));
        }
    }
    assert!(!exists(&state, late.wolf1()) && exists(&state, late.wolves[1]));
}

/// Measured (evoking ghosts, sucking, fading out, banishing, animal evocation with reserve and aura).
/// Changed: meat module via the pending list like `0x004FEC23` (only `step` 327 = [`WOLF2_CREATED`]
/// differs: wolf 2 keeps `m_Enabled` of module 21 until its next object tick).
/// Changed: module 38 gets target point and corpse (serialization, behavior the same; counter-check with
/// `serde(skip)` on `target`/`corpse`: again `0x4a0c_c454_b88a_01c2`).
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
const EVOKE_GOLDEN: u64 = 0xc18f_ae2a_b4d7_140b;

/// Continuous sequence of the scene without saving, computed once per test process and shared by the
/// saving tests.
static EVOKE_SEQUENCE: LazyLock<Vec<u64>> =
    LazyLock::new(|| evoke_run_probed(None, |_, _, _, _| {}));

#[test]
fn evoke_checksums_match_the_golden_sequence() {
    let mut h = crate::rules::Fnv64::default();
    for c in EVOKE_SEQUENCE.iter() {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, EVOKE_GOLDEN, "{:#018x}", h.0);
}

#[test]
fn evoke_names_count_from_00() {
    // Named evokers: every evocation enters `<name>NN` with the module counter before counting
    // up (`FUN_005107D0`/`FUN_005114D0` -> `FUN_0043E310`, `"%02d"`, E-146); the
    // counter also runs without a name (enemy 1411). Ghost B is created and aborted,
    // but counts as well.
    let (mut state, rules, scene) = evoke_scene();
    crate::names::register(&mut state.names, "Geistmeister", scene.evoker);
    crate::names::register(&mut state.names, "Tiermeister", scene.animal_evoker);
    let mut late = Late::default();
    let mut seen = std::collections::BTreeMap::new();
    for i in 0..=WOLF2_CREATED {
        step(&mut state, &rules, &evoke_orders(i, &scene, &late));
        late.observe(&state, &scene, i);
        for key in [
            "geistmeister00",
            "geistmeister01",
            "tiermeister00",
            "tiermeister01",
        ] {
            if let Some(v) = state.names.get(key) {
                seen.entry(key).or_insert_with(|| (i, v.to_owned()));
            }
        }
    }
    let value = |id: ObjectId| id.as_i32().to_string();
    assert_eq!(
        seen.get("geistmeister00"),
        Some(&(GHOST_A_CREATED, value(late.ghost_a())))
    );
    assert_eq!(
        seen.get("geistmeister01"),
        Some(&(GHOST_B_CREATED, value(late.ghost_b.expect("ghost B"))))
    );
    assert_eq!(
        seen.get("tiermeister00"),
        Some(&(WOLF1_CREATED, value(late.wolves[0])))
    );
    assert_eq!(
        seen.get("tiermeister01"),
        Some(&(WOLF2_CREATED, value(late.wolves[1])))
    );
    assert_eq!(evoke_state(&state, scene.evoker).name_counter, 2);
    assert_eq!(evoke_state(&state, scene.enemy_evoker).name_counter, 1);
    assert_eq!(animal_state(&state, scene.animal_evoker).name_counter, 2);
}

/// Save points of the scene (measured, as an assertion in the probes): between `0x41A` and
/// `0x21A`; in the middle of the fade-out of B; during sucking; between `0x11F` and `0x21F`; during
/// the 8 s of the animal evocation; between two aura ticks.
const SAVE_EVOKING: usize = 2;
const SAVE_FADING: usize = 120;
const SAVE_SUCKING: usize = 60;
const SAVE_BANISHING: usize = 100;
const SAVE_ANIMAL: usize = 50;
const SAVE_AURA: usize = 360;
const _: () = assert!(EVOKED < SAVE_EVOKING && SAVE_EVOKING < GHOST_A_CREATED);
const _: () = assert!(HALT_AT < SAVE_FADING && SAVE_FADING < GHOST_B_DEAD);
const _: () = assert!(FIRST_SUCK < SAVE_SUCKING && SAVE_SUCKING < COLONIST_GONE);
const _: () = assert!(BANISH_PAID <= SAVE_BANISHING && SAVE_BANISHING < ENEMY_GHOST_BANNED);
const _: () = assert!(ANIMAL_EVOKED <= SAVE_ANIMAL && SAVE_ANIMAL < WOLF1_CREATED);
const _: () = assert!(WOLF2_CREATED < SAVE_AURA && SAVE_AURA < BEHAVIOR_AT);

/// Scene with saving/loading after `save_at` against the continuous sequence.
fn assert_survives_saving(save_at: usize, at_save: EvokeProbe, what: &str) {
    let saved = evoke_run_probed(Some(save_at), at_save);
    let first_diff = saved
        .iter()
        .zip(EVOKE_SEQUENCE.iter())
        .position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "saved {what}: first deviation in `step`");
}

#[test]
fn evoke_checksums_survive_saving_and_loading() {
    // One run per save point, side by side (run time, as with `STEALTH_GOLDEN`). Every
    // sequence equals the continuous one.
    let cases: [(usize, EvokeProbe, &str); 6] = [
        (
            SAVE_EVOKING,
            |state, _, s, _| {
                // `0x41A` has run (`EVOKING`, corpse bound, base value paid, no
                // `0x41A` is pending any more); `0x21A` and `0x31A` are pending, no ghost yet.
                assert!(has(state, s.evoker, status::EVOKING));
                let e = evoke_state(state, s.evoker);
                assert_eq!((e.dead_body, e.ghost), (s.corpses[0], ObjectId::NONE));
                assert_eq!(base_mana(state, s.evoker), 125);
                assert!(!pending(state, s.evoker, evoke::INDEX, evoke::msg::START));
                assert!(pending(state, s.evoker, evoke::INDEX, evoke::msg::CREATE));
                assert!(pending(
                    state,
                    s.evoker,
                    evoke::INDEX,
                    evoke::msg::STOP_ANIM
                ));
            },
            "between 0x41A and 0x21A",
        ),
        (
            SAVE_FADING,
            |state, _, _, late| {
                let g = ghost_state(state, late.ghost_b.unwrap());
                assert_eq!(g.status, ghost::state::FADING);
                assert!(0.0 < g.alpha && g.alpha < 1.0, "alpha {}", g.alpha);
            },
            "in the middle of the fade-out",
        ),
        (
            SAVE_SUCKING,
            |state, _, s, late| {
                // State 3 is visible between two `step` (Attack/FightFar resend `0x31B`
                // in the tick before the ghost module).
                let g = ghost_state(state, late.ghost_a());
                assert_eq!(
                    (g.status, g.target),
                    (ghost::state::SUCKING, s.colonists[0])
                );
            },
            "during sucking",
        ),
        (
            SAVE_BANISHING,
            |state, _, s, _| {
                assert!(has(state, s.evoker, status::ANIM_LOCKED));
                assert!(pending(state, s.evoker, banish::INDEX, banish::msg::NOW));
                assert!(pending(state, s.evoker, banish::INDEX, banish::msg::END));
            },
            "between 0x11F and 0x21F",
        ),
        (
            SAVE_ANIMAL,
            |state, _, s, _| {
                let ev = s.animal_evoker;
                assert!(has(state, ev, status::EVOKING));
                assert_eq!(animal_state(state, ev).reserve, 25);
                assert!(pending(
                    state,
                    ev,
                    evoke_animal::INDEX,
                    evoke_animal::msg::EVOKE
                ));
            },
            "during the animal evocation",
        ),
        (
            SAVE_AURA,
            |state, _, s, late| {
                for &w in &late.wolves {
                    assert!(pending(
                        state,
                        w,
                        exec_temp::INDEX,
                        exec_temp::msg::AURA_TICK
                    ));
                    assert!(exec_state(state, w).influenced.is_some());
                }
                let set = exec_state(state, late.wolf1()).influenced.unwrap();
                assert!(set.contains(&s.warrior) && set.contains(&s.ranged));
            },
            "between two aura ticks",
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
