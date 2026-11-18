//! Combat flows over several ticks (re/combat-melee 8.2, re/combat-death 2.4) and the golden
//! checksum sequence of the combat scene.

use crate::combat::testkit::*;
use crate::command::{Command, CommandKind};
use crate::id::ObjectId;
use crate::module::ModuleKind;
use crate::object::status;
use crate::rules::{Fnv64, Rules};
use crate::save::{SaveGame, checksum};
use crate::setup::{MatchSetup, start_package};
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::tick::step;

/// Attacker (slot 0) and victim (slot 1) center to center `gap`; both turned toward each other. The
/// victim stands on stance 0 ("CEASE FIRE"): it does not fight back.
fn duel(rules: &Rules, attacker: u32, victim: u32, gap: i32) -> (SimState, ObjectId, ObjectId) {
    let mut state = state(rules);
    let (a, v) = {
        let mut sim = Sim::new(&mut state, rules);
        let x = (10 << 12) + 0x800;
        let mk = |sim: &mut Sim, g, s, fx: i32| {
            crate::world::create_object(
                sim,
                &mut crate::world::ObjectDesc::new(g, s, [fx as f32 / 32.0, 0.0, x as f32 / 32.0]),
            )
            .unwrap()
        };
        let a = mk(&mut sim, attacker, 0, x);
        let v = mk(&mut sim, victim, 1, x + gap);
        sim.object_mut(a).unwrap().direction = crate::fx::Fx(0);
        crate::messaging::send(
            &mut sim,
            v,
            &crate::module::quarter::attack_mode_msg(0, false),
        );
        (a, v)
    };
    (state, a, v)
}

fn attack(a: ObjectId, v: ObjectId) -> Command {
    Command {
        player: 0,
        kind: CommandKind::Attack {
            units: vec![a],
            target: v,
            pos: None,
            mods: 0,
        },
    }
}

/// Runs until the death of the victim; return value (tick of the first swing S, tick of the death T, hits).
///
/// Death: `DIED` on the victim or its change of owner to the corpse (`OwnerChanged { old: v, .. }`,
/// at the end of the same step; afterwards the old ID no longer exists). The loop then reads the fraction
/// of the fatal hit from the corpse (new ID, same attributes).
fn until_death(state: &mut SimState, rules: &Rules, a: ObjectId, v: ObjectId) -> (u32, u32, usize) {
    step(state, rules, &[attack(a, v)]);
    let (mut s, mut hits, mut last) = (None, 0, 100_000);
    for _ in 0..2 * 1200 {
        let events = step(state, rules, &[]);
        if s.is_none()
            && state
                .object(a, false)
                .is_some_and(|o| o.status & status::FIGHTING != 0)
        {
            s = Some(state.tick);
        }
        let corpse = events.iter().find_map(|e| match e {
            SimEvent::OwnerChanged { old, new } if *old == v => Some(*new),
            _ => None,
        });
        let Some(o) = state.object(corpse.unwrap_or(v), false) else {
            return (s.expect("swing begun"), state.tick, hits);
        };
        let f = o.attributes.current_fraction(14);
        if f != last {
            hits += 1;
            last = f;
        }
        if corpse.is_some() || o.status & status::DIED != 0 {
            return (s.expect("swing begun"), state.tick, hits);
        }
    }
    panic!("no death");
}

#[test]
fn a_sabre_kills_a_standing_warrior_in_16_swings() {
    // re/combat-melee 8.2: -6250 per hit, 16 hits, fatal hit S + 199.
    let rules = rules();
    let (mut state, a, v) = duel(&rules, SABRE, WARRIOR, 3072);
    let (s, t, hits) = until_death(&mut state, &rules, a, v);
    assert_eq!(hits, 16);
    // The death follows in the victim's own object tick after the hit (C-140): T in {S+199, S+200}.
    assert!((s + 199..=s + 200).contains(&t), "S = {s}, T = {t}");
}

#[test]
fn a_warrior_needs_78_hits_on_a_pikeman_with_the_sword_bonus() {
    // re/combat-melee 8.2: -1037/-1555 per partial hit, 78 hits in 39 swings, S + 505.
    let rules = rules();
    let (mut state, a, v) = duel(&rules, WARRIOR, PIKEMAN, 3072);
    let (s, t, hits) = until_death(&mut state, &rules, a, v);
    assert_eq!(hits, 78);
    assert!((s + 505..=s + 506).contains(&t), "S = {s}, T = {t}");
}

#[test]
fn after_the_kill_an_area_defender_searches_and_takes_the_next_target() {
    // Stance 4 ("DEFEND AREA", re/combat-attack 3.1) keeps searching after the death of the target.
    let rules = rules();
    let (mut state, a, v) = duel(&rules, SABRE, WARRIOR, 3072);
    let next = {
        let mut sim = Sim::new(&mut state, &rules);
        crate::messaging::send(
            &mut sim,
            a,
            &crate::module::quarter::attack_mode_msg(4, false),
        );
        spawn(&mut sim, WARRIOR, 1, (16, 10))
    };
    until_death(&mut state, &rules, a, v);
    for _ in 0..2 * 40 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(
        crate::module::attack::attack(&sim, a).unwrap().target,
        next,
        "search after the death (stance 4)"
    );
}

#[test]
fn stances_decide_who_searches_and_who_strikes_back() {
    // Stance 3 searches in the phase grid HID % 1500 and advances; stance 0 does nothing; stance 2
    // (default) fights back immediately.
    let rules = rules();
    let mut state = state(&rules);
    let (off, idle, enemy) = {
        let mut sim = Sim::new(&mut state, &rules);
        let off = spawn(&mut sim, WARRIOR, 0, (10, 10));
        let idle = spawn(&mut sim, WARRIOR, 0, (10, 30));
        let enemy = spawn(&mut sim, SABRE, 1, (16, 10));
        let _enemy2 = spawn(&mut sim, SABRE, 1, (16, 30));
        crate::messaging::send(
            &mut sim,
            off,
            &crate::module::quarter::attack_mode_msg(3, false),
        );
        crate::messaging::send(
            &mut sim,
            idle,
            &crate::module::quarter::attack_mode_msg(0, false),
        );
        (off, idle, enemy)
    };
    for _ in 0..2 * 16 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(
        crate::module::attack::attack(&sim, off).unwrap().target,
        enemy,
        "offensive stance searches (<= 15 ticks)"
    );
    assert_eq!(
        crate::module::attack::attack(&sim, idle).unwrap().target,
        ObjectId::NONE,
        "cease-fire stance"
    );
    // Stance 2: the attacked saber fighter strikes back.
    for _ in 0..2 * 60 {
        step(&mut state, &rules, &[]);
    }
    let sim = Sim::new(&mut state, &rules);
    assert_eq!(
        crate::module::attack::attack(&sim, enemy).unwrap().target,
        off
    );
}

#[test]
fn a_commanded_attack_is_deaf_until_the_target_is_gone() {
    // Persistence [1] = 4 after the attack command: no switch, even if a nearer one arrives.
    let rules = rules();
    let (mut state, a, v) = duel(&rules, SABRE, WARRIOR, 3 * 4096);
    {
        let mut sim = Sim::new(&mut state, &rules);
        crate::messaging::send(
            &mut sim,
            a,
            &crate::module::quarter::attack_mode_msg(3, false),
        );
        spawn(&mut sim, SABRE, 1, (11, 10));
    }
    step(&mut state, &rules, &[attack(a, v)]);
    for _ in 0..2 * 40 {
        step(&mut state, &rules, &[]);
        let sim = Sim::new(&mut state, &rules);
        let s = crate::module::attack::attack(&sim, a).unwrap();
        assert_eq!(s.target, v);
    }
}

/// Combat scene of the golden checksums: 2 saber fighters (slot 0, OFFENSIVE) against 2 warriors (slot 1,
/// default DEFENSIVE), 6 tiles apart; 1400 engine steps (700 ticks) until after the corpses
/// are removed.
fn combat_scene() -> (Rules, SimState) {
    let rules = rules();
    let mut state = state(&rules);
    {
        let mut sim = Sim::new(&mut state, &rules);
        for (g, slot, tile) in [
            (SABRE, 0, (10, 10)),
            (SABRE, 0, (10, 12)),
            (WARRIOR, 1, (16, 10)),
            (WARRIOR, 1, (16, 12)),
        ] {
            let id = spawn(&mut sim, g, slot, tile);
            if slot == 0 {
                crate::messaging::send(
                    &mut sim,
                    id,
                    &crate::module::quarter::attack_mode_msg(3, false),
                );
            }
        }
    }
    state.events.clear();
    (rules, state)
}

const COMBAT_STEPS: usize = 1400;

#[test]
fn the_combat_checksum_sequence_is_fixed_in_debug_and_release() {
    let (rules, mut state) = combat_scene();
    let mut h = Fnv64::default();
    let mut deaths = 0;
    for _ in 0..COMBAT_STEPS {
        for e in step(&mut state, &rules, &[]) {
            if let SimEvent::OwnerChanged { new, .. } = e
                && new.slot() == 8
            {
                deaths += 1;
            }
        }
        h.write(&checksum(&state).to_le_bytes());
    }
    assert!(deaths >= 2, "the scene must have deaths");
    // E-61: only AI slots report; the scene has none, its attack reports stay empty.
    assert!(
        state
            .attack_reports
            .iter()
            .all(crate::mfc::MfcMap::is_empty)
    );
    assert_eq!(h.0, COMBAT_GOLDEN, "checksum sequence {h:016x?}", h = h.0);
}

/// Checksum over the checksums of `the_combat_checksum_sequence_is_fixed_in_debug_and_release`
/// (`crates/opennml-sim/src/combat/flows.rs`). Measured (random draw per swing, timers, rank, corpse,
/// change of owner).
/// Changed: new state fields (building map, projectiles, hit zone counter), behavior unchanged.
/// Changed: new state fields groups/walk (FORMAT_VERSION 8), behavior unchanged.
/// Changed: new state fields (event bits, last track, meeting point), behavior unchanged.
/// Changed: new state fields (upgrades, counter-upgrades, temporary upgrades).
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
const COMBAT_GOLDEN: u64 = 0x5fd2_22ad_f52b_680f;

/// What a load point meets in the middle of combat.
#[derive(Default)]
struct Seen {
    /// Attack status 1 (approach) with a target set.
    approach: bool,
    /// Open timer `0x712` (hit) or `0x912` (end of swing) in the melee module.
    mid_swing: bool,
    /// Dying status 1 (callbacks/pending entries outstanding).
    dying_1: bool,
    /// Dying status 2 (sinking).
    dying_2: bool,
    /// Combat points of a first half of a double tick (phase change still open).
    half_points: bool,
}

impl Seen {
    fn note(&mut self, s: &SimState) {
        if s.phase == 1 && s.combat.attack_points.iter().any(|&p| p != 0) {
            self.half_points = true;
        }
        for o in s.objects() {
            for m in o.modules.iter().flatten() {
                match &m.kind {
                    ModuleKind::Attack(a) if a.status == 1 && !a.target.is_none() => {
                        self.approach = true;
                    }
                    ModuleKind::FightNear(_)
                        if m.base
                            .events
                            .iter()
                            .chain(&m.base.timers_ws)
                            .any(|e| matches!(e.msg.code, 0x712 | 0x912)) =>
                    {
                        self.mid_swing = true;
                    }
                    ModuleKind::Dying(d) if d.status == 1 => self.dying_1 = true,
                    ModuleKind::Dying(d) if d.status == 2 => self.dying_2 = true,
                    _ => {}
                }
            }
        }
    }
}

#[test]
fn combat_checksums_survive_saving_and_loading_mid_swing_and_mid_dying() {
    let (rules, state) = combat_scene();
    let setup = MatchSetup::default_for(&rules);
    let mut reference = state.clone();
    let mut sums = Vec::new();
    for _ in 0..COMBAT_STEPS {
        step(&mut reference, &rules, &[]);
        sums.push(checksum(&reference));
    }
    // Load points (measured on the scene): 40 approach, 121 and 300 in the middle of a swing, 483
    // dying status 1 (first death at 482) in the first half of a double tick with half combat points,
    // 600 and 900 corpses lie, 1100 dying status 2 (sinking from 1088).
    let mut seen = Seen::default();
    let mut corpses_seen = false;
    for at in [40, 121, 300, 483, 600, 900, 1100] {
        let mut s = state.clone();
        for _ in 0..at {
            step(&mut s, &rules, &[]);
        }
        corpses_seen |= s.objects().any(|o| o.status & status::DIED != 0);
        seen.note(&s);
        let save = SaveGame {
            start: start_package(&rules, &setup),
            state: s,
            commands: Vec::new(),
        };
        let mut loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap().state;
        for (i, want) in sums.iter().enumerate().skip(at) {
            step(&mut loaded, &rules, &[]);
            assert_eq!(checksum(&loaded), *want, "loaded at {at}, step {i}");
        }
    }
    assert!(
        corpses_seen,
        "at least one load point in the middle of dying (adjust load points)"
    );
    assert!(
        seen.approach,
        "load point in the approach (attack status 1)"
    );
    assert!(
        seen.mid_swing,
        "load point in the middle of a swing (0x712/0x912)"
    );
    assert!(seen.dying_1, "load point in dying status 1");
    assert!(seen.dying_2, "load point in dying status 2");
    assert!(seen.half_points, "load point with half combat points");
}

/// Ranged scene: archer against warrior; rifleman with a house in the line of fire (line of
/// sight, cell search); SPA cannon against two warriors who fight back (splash, melee at the
/// cannon). Shooters in stance 3 ("OFFENSIVE"), the targets of the first two in stance 0.
fn ranged_scene() -> (Rules, SimState) {
    let rules = rules();
    let mut state = state(&rules);
    {
        let mut sim = Sim::new(&mut state, &rules);
        for (g, slot, tile, mode) in [
            (ARCHER, 0, (10, 10), 3),
            (WARRIOR, 1, (16, 10), 0),
            (ENG_INFANTRY, 0, (10, 20), 3),
            (WARRIOR, 1, (16, 20), 0),
            (SPA_CANNON, 0, (10, 30), 3),
            (WARRIOR, 1, (17, 30), 2),
            (WARRIOR, 1, (17, 31), 2),
        ] {
            let id = spawn(&mut sim, g, slot, tile);
            crate::messaging::send(
                &mut sim,
                id,
                &crate::module::quarter::attack_mode_msg(mode, false),
            );
        }
        spawn(&mut sim, crate::command::testkit::HOUSE, 2, (13, 20));
    }
    state.events.clear();
    (rules, state)
}

const RANGED_STEPS: usize = 1600;

#[test]
fn the_ranged_checksum_sequence_is_fixed_in_debug_and_release() {
    let (rules, mut state) = ranged_scene();
    let mut h = Fnv64::default();
    let (mut deaths, mut arrows, mut balls, mut gun_sounds) = (0, 0, 0, 0);
    for _ in 0..RANGED_STEPS {
        for e in step(&mut state, &rules, &[]) {
            match e {
                SimEvent::OwnerChanged { new, .. } if new.slot() == 8 => deaths += 1,
                SimEvent::ProjectileLaunched {
                    kind: crate::combat::projectile::ProjectileKind::Arrow,
                    ..
                } => arrows += 1,
                SimEvent::ProjectileLaunched { .. } => balls += 1,
                SimEvent::Sound3d {
                    action: 1,
                    flags: 0x23,
                    ..
                } => gun_sounds += 1,
                _ => {}
            }
        }
        h.write(&checksum(&state).to_le_bytes());
    }
    assert!(deaths >= 2, "deaths in the scene: {deaths}");
    assert!(
        arrows >= 3 && balls >= 2 && gun_sounds >= 2,
        "arrows {arrows}, balls {balls}, rifle {gun_sounds}"
    );
    assert_eq!(h.0, RANGED_GOLDEN, "checksum sequence {h:016x?}", h = h.0);
}

/// Checksum over the checksums of `the_ranged_checksum_sequence_is_fixed_in_debug_and_release`.
/// Measured (arrow, rifle with magazine, cannon with splash, line of sight).
/// Changed: new state fields groups/walk (FORMAT_VERSION 8), behavior unchanged.
/// Changed: new state fields (event bits, last track, meeting point), behavior unchanged.
/// Changed: new state fields (upgrades, counter-upgrades, temporary upgrades).
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
const RANGED_GOLDEN: u64 = 0x8ef4_f595_958c_ab80;

/// What a load point meets in the ranged scene.
#[derive(Default)]
struct SeenRanged {
    arrow_in_flight: bool,
    ball_in_flight: bool,
    gun_reloading: bool,
    cannon_delay: bool,
}

impl SeenRanged {
    fn note(&mut self, s: &SimState) {
        use crate::combat::projectile::ProjectileKind;
        for p in &s.projectiles.list {
            match p.kind {
                ProjectileKind::Arrow => self.arrow_in_flight = true,
                ProjectileKind::CannonBall => self.ball_in_flight = true,
                // World effects and the throw are neither arrow nor ball.
                ProjectileKind::Dynamite
                | ProjectileKind::Explosion
                | ProjectileKind::Storm
                | ProjectileKind::Throw => {}
            }
        }
        for o in s.objects() {
            if let Some(m) = o.module(crate::module::shoot::INDEX)
                && let ModuleKind::Shoot(shoot) = &m.kind
            {
                let pending = |code: u32| {
                    m.base
                        .events
                        .iter()
                        .chain(&m.base.timers_ws)
                        .any(|e| e.msg.code == code)
                };
                // Rifle in the window between GUNDELAY and GUNRELOADED: `0xA0A` fired, `0xB0A` open.
                self.gun_reloading |= pending(0xB0A) && !pending(0xA0A);
                // Cannon in an open CANNONDELAY: `0x70A` is waiting, it must not shoot.
                self.cannon_delay |= pending(0x70A) && !shoot.can_shoot;
            }
        }
    }
}

#[test]
fn ranged_checksums_survive_saving_and_loading_mid_flight_and_mid_reload() {
    let (rules, state) = ranged_scene();
    let setup = MatchSetup::default_for(&rules);
    let mut reference = state.clone();
    let mut sums = Vec::new();
    for _ in 0..RANGED_STEPS {
        step(&mut reference, &rules, &[]);
        sums.push(checksum(&reference));
    }
    let mut seen = SeenRanged::default();
    // Load points (measured on the scene): 30 before the first shot; 52 arrow (steps 49-69) and
    // ball (39-57) fly, `CANNONDELAY` (0x70A) of the cannon is open (until 131); 60 only the arrow;
    // 120 rifle after `GUNDELAY` (89), before `GUNRELOADED` (until ~218); 200 shortly before `GUNRELOADED`; 400
    // arrow in flight (385-405) with an empty rifle pending entry; 800 cannon in the waiting window of the second shot.
    for at in [30, 52, 60, 120, 200, 400, 800] {
        let mut s = state.clone();
        for _ in 0..at {
            step(&mut s, &rules, &[]);
        }
        seen.note(&s);
        let save = SaveGame {
            start: start_package(&rules, &setup),
            state: s,
            commands: Vec::new(),
        };
        let mut loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap().state;
        for (i, want) in sums.iter().enumerate().skip(at) {
            step(&mut loaded, &rules, &[]);
            assert_eq!(checksum(&loaded), *want, "loaded at {at}, step {i}");
        }
    }
    assert!(seen.arrow_in_flight, "load point with a flying arrow");
    assert!(seen.ball_in_flight, "load point with a flying ball");
    assert!(seen.gun_reloading, "load point with an open GUNRELOADED");
    assert!(seen.cannon_delay, "load point with an open CANNONDELAY");
}
