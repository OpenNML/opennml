//! Scene with `UPGRADE_GOLDEN` (pattern `riding_run_probed`): research in the headquarters, counter-upgrade
//! of the missionary, spell of the governor with a temporary upgrade, arming a colonist and lumbering on a
//! real level start with time-of-day upgrades; saving/loading in the middle of each run. Plus the ticked
//! scene for K-300 (dying colonist and upgrade).

use std::sync::LazyLock;

use nml_formats::config::LevelPlayer;

use super::testkit::*;
use crate::command::{Command, CommandKind};
use crate::id::ObjectId;
use crate::module::building;
use crate::module::gather::{self, status as gather_status};
use crate::module::{arming, fight_far, fight_near, spell};
use crate::object::status;
use crate::rules::Rules;
use crate::save::testkit::save_and_load;
use crate::setup::{MatchSetup, new_match};
use crate::sim::Sim;
use crate::state::SimState;
use crate::tick::step;

/// Deciduous tree (like the economy world `command::testkit`): tree module 4, 200 wood.
const TREE: u32 = 3602;
const TREE_TYPE: &str =
    "GUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\nVALUEID 50003 6 1 0\n";

/// Time of day of the level (`level_resources[40]`, re/upgrades 3.8).
const DAYTIME: i32 = 1;

/// Research (wood rate +25%), arming, "develop" (prerequisite of 3374), taxes button,
/// chastity belt, missionary upgrade, taxes (temporary).
const WOOD_RESEARCH: u32 = 2800;
const ARM_RESEARCH: u32 = 2806;
const DEVELOP: u32 = 3354;
const TAX_BUTTON_UPGRADE: u32 = 2828;
const CHASTITY: u32 = 3374;
const MISSIONARY_UPGRADE: u32 = 2840;
const TAX: u32 = 4450;
/// Ability button "Collect taxes" (kind 4, animation 2, index 1; mana 75).
const TAX_BUTTON: i32 = 543;

/// IDs of the scene.
struct Scene {
    hq: ObjectId,
    lumberjack: ObjectId,
    armed: ObjectId,
    idle: ObjectId,
    tree: ObjectId,
    missionary: ObjectId,
    governor: ObjectId,
    enemy_hq: ObjectId,
    enemy: ObjectId,
}

/// Rules of the scene: test world with deciduous tree, players 0 and 1 human (1000 gold, wood and
/// food each) in the `.lvl`, time of day [`DAYTIME`].
fn scene_rules() -> Rules {
    let mut r = rules_from(&format!("{TYPES}{TREE_TYPE}"));
    r.defaults.insert(TREE, [(25, 200)].into_iter().collect());
    // Load capacity 20 (attribute 17, like the economy world).
    r.defaults
        .get_mut(&COLONIST)
        .expect("colonist")
        .insert(17, 20);
    r.level.level_resources.insert(40, Some(DAYTIME));
    for slot in [0, 1] {
        r.level.players.push(LevelPlayer {
            slot,
            name: format!("P{slot}"),
            nation: 0,
            kind: 0,
            resources: vec![(0, 1000), (1, 1000), (2, 1000)],
        });
    }
    r
}

/// Scene via a real level start (`setup::new_match`: with time-of-day upgrades), map revealed like the
/// test world. Slot 0: headquarters, three colonists (lumberjack, later armed, idle), deciduous tree,
/// missionary, governor; slot 1: headquarters and one colonist.
fn upgrade_scene() -> (SimState, Rules, Scene) {
    let rules = scene_rules();
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).expect("level start");
    crate::fog::reveal_map(&mut state, -1);
    let scene = {
        let mut sim = Sim::new(&mut state, &rules);
        Scene {
            hq: spawn(&mut sim, HQ, 0, (16, 16)),
            lumberjack: spawn(&mut sim, COLONIST, 0, (20, 18)),
            armed: spawn(&mut sim, COLONIST, 0, (21, 18)),
            idle: spawn(&mut sim, COLONIST, 0, (22, 18)),
            tree: spawn(&mut sim, TREE, 9, (24, 16)),
            missionary: spawn(&mut sim, MISSIONARY, 0, (18, 22)),
            governor: spawn(&mut sim, GOVERNOR, 0, (20, 22)),
            enemy_hq: spawn(&mut sim, HQ, 1, (44, 44)),
            enemy: spawn(&mut sim, COLONIST, 1, (40, 44)),
        }
    };
    (state, rules, scene)
}

/// Enqueue research and chop wood.
const RESEARCH_AT: usize = 0;
/// `0x39` 2828 (taxes button free).
const TAX_UNLOCK_AT: usize = 1;
/// Spell "Collect taxes".
const CAST_AT: usize = 400;
/// Arming, after the end of 2806 (measured: done in `step` 500).
const ARM_AT: usize = 501;
/// Counter-upgrade 3374 and training at slot 1, after the end of 3354 (measured: done in
/// `step` 900).
const COUNTER_AT: usize = 901;
/// Length of the scene in `step` calls (two per object tick).
const UPGRADE_STEPS: usize = 3000;

/// Commands in the `step` call `i` (not object tick: two `step`s are one tick).
fn upgrade_orders(i: usize, s: &Scene) -> Vec<Command> {
    let cmd = |player, kind| Command { player, kind };
    let research = |guid| CommandKind::EnqueueProduction {
        building: s.hq,
        guid,
    };
    match i {
        RESEARCH_AT => vec![
            cmd(0, research(WOOD_RESEARCH)),
            cmd(0, research(ARM_RESEARCH)),
            cmd(0, research(DEVELOP)),
            cmd(
                0,
                CommandKind::ChopWood {
                    tree: s.tree,
                    units: vec![s.lumberjack],
                },
            ),
        ],
        TAX_UNLOCK_AT => vec![cmd(
            0,
            CommandKind::ApplyUpgrade {
                slot: 0,
                guid: TAX_BUTTON_UPGRADE as i32,
            },
        )],
        CAST_AT => vec![cmd(
            0,
            CommandKind::CastSpell {
                caster: s.governor,
                art: spell::ART_EXEC_TEMP,
                anim: 2,
                idx: 1,
                button: TAX_BUTTON,
            },
        )],
        ARM_AT => vec![cmd(
            0,
            CommandKind::ToggleArming {
                units: vec![s.armed],
                arm: true,
            },
        )],
        COUNTER_AT => vec![
            cmd(
                0,
                CommandKind::CounterUpgrade {
                    guid: CHASTITY,
                    source: s.missionary,
                },
            ),
            cmd(
                1,
                CommandKind::EnqueueProduction {
                    building: s.enemy_hq,
                    guid: COLONIST,
                },
            ),
        ],
        _ => vec![],
    }
}

/// State assertion at the save point: state, rules, IDs of the scene.
type UpgradeProbe = fn(&mut SimState, &Rules, &Scene);

/// Checksum per `step` call of the scene. With `save_at = Some(i)` it saves after the `step` call `i`,
/// checks against the rules (like the viewer when loading) and reloads; `at_save`
/// sees the state immediately before.
fn upgrade_run_probed(save_at: Option<usize>, at_save: UpgradeProbe) -> Vec<u64> {
    let (mut state, rules, scene) = upgrade_scene();
    let mut out = Vec::with_capacity(UPGRADE_STEPS);
    for i in 0..UPGRADE_STEPS {
        step(&mut state, &rules, &upgrade_orders(i, &scene));
        if save_at == Some(i) {
            at_save(&mut state, &rules, &scene);
            assert_loads(&state, &rules);
            state = save_and_load(&state, &rules);
        }
        out.push(crate::save::checksum(&state));
    }
    out
}

fn count(state: &SimState, slot: usize, guid: u32) -> i32 {
    state.guid_counts[slot].get(&guid).copied().unwrap_or(0)
}

/// Running order of the building as `(GUID, remaining time)`.
fn making(sim: &Sim, b: ObjectId) -> (u32, i32) {
    building::building(sim, b).map_or((0, 0), |b| (b.make_guid, b.make_time))
}

/// Temporary upgrades of the object as `(GUID, rest)`.
fn temps(sim: &Sim, id: ObjectId) -> Vec<(u32, i32)> {
    sim.object(id).map_or_else(Vec::new, |o| {
        o.temp_upgrades
            .iter()
            .map(|e| (e.guid, e.rest_ms))
            .collect()
    })
}

/// Measured `step` calls of the scene (debug = release): end of 2800, 2806 and 3354 (research of 15, 10
/// and 20 s one after the other), cast of the spell (the FIFO runs in the tick of the governor),
/// effect `0x326` (500 ms later: 4450 begins), arrival of the armed unit at the headquarters,
/// finished colonist at slot 1 and end of 4450.
const WOOD_DONE: usize = 300;
const ARM_DONE: usize = 500;
const DEVELOP_DONE: usize = 900;
const CAST_DONE: usize = 401;
const TAX_STARTS: usize = 413;
const ARMED: usize = 564;
const TRAINED: usize = 1502;
const TAX_ENDS: usize = 1611;

#[test]
fn the_upgrade_scene_shows_every_state() {
    // Counter-check before the golden: 2800 done (wood rate 75), 2806 done, colonist armed
    // (ISARMED, FightFar on), missionary counter 2840 = 1, time-of-day counters, 3374 runs and slot 1
    // trains half as fast, governor pays taxes (gold rises by 1000 raw per object tick),
    // mana drops by 75. Times are `step` calls (two per object tick).
    let (mut state, rules, s) = upgrade_scene();
    // Level start: time of day 1, for each slot 0-7 the six counters `4501 + 10k` = 1.
    assert_eq!(state.daytime, DAYTIME);
    for slot in 0..8 {
        for k in 0..6 {
            assert_eq!(count(&state, slot, 4501 + 10 * k), 1, "slot {slot}, k {k}");
        }
    }
    let mut gold_at = Vec::with_capacity(UPGRADE_STEPS);
    let mut wood_at = Vec::with_capacity(UPGRADE_STEPS);
    for i in 0..UPGRADE_STEPS {
        step(&mut state, &rules, &upgrade_orders(i, &s));
        let sim = Sim::new(&mut state, &rules);
        gold_at.push(sim.state.players[0].resources_fx[0]);
        wood_at.push(sim.state.players[0].resources_fx[1]);
        // Module 30 credits 2840 via the queue in the first phase 0 step.
        if i >= 1 {
            assert_eq!(count(sim.state, 0, MISSIONARY_UPGRADE), 1, "step {i}");
        }
        // Research: end and wood rate 60 -> 75 (+25%).
        let wood = (
            count(sim.state, 0, WOOD_RESEARCH),
            sim.attribute(s.lumberjack, 32),
        );
        let expected = if i < WOOD_DONE { (0, 60) } else { (1, 75) };
        assert_eq!(wood, expected, "step {i}");
        assert_eq!(count(sim.state, 0, ARM_RESEARCH), i32::from(i >= ARM_DONE));
        assert_eq!(count(sim.state, 0, DEVELOP), i32::from(i >= DEVELOP_DONE));
        // Arming: way to the headquarters, on arrival ISARMED and FightFar (variant 7).
        let ff = sim
            .object(s.armed)
            .unwrap()
            .has_module(fight_far::INDEX, false);
        assert_eq!(
            (arming::is_armed(&sim, s.armed), ff),
            (i >= ARMED, i >= ARMED),
            "step {i}"
        );
        // Spell: mana 100 -> 25 on cast, animation locked until `0x326`; 4450 from then on.
        let mana = sim.attribute(s.governor, spell::MANA);
        if i < CAST_DONE {
            assert_eq!(mana, 100, "step {i}");
        }
        if i == CAST_DONE {
            assert_eq!(mana, 25, "mana -75");
        }
        let locked = sim.object(s.governor).unwrap().status & status::ANIM_LOCKED != 0;
        if (CAST_DONE..TAX_STARTS).contains(&i) {
            assert!(locked && temps(&sim, s.governor).is_empty(), "step {i}");
        }
        let taxing = temps(&sim, s.governor).first().map(|t| t.0);
        let expected = (TAX_STARTS..TAX_ENDS).contains(&i).then_some(TAX);
        assert_eq!(taxing, expected, "step {i}");
        // Counter-upgrade: in the list from the command on, counter 1 at slot 1, flag on the enemy
        // colonist; slot 0 (sender) gets no counter.
        if i >= COUNTER_AT {
            let entries: Vec<_> = sim
                .state
                .counter_upgrades
                .iter()
                .map(|c| (c.guid, c.source))
                .collect();
            assert_eq!(entries, [(CHASTITY, s.missionary)], "step {i}");
            let counts = (count(sim.state, 1, CHASTITY), count(sim.state, 0, CHASTITY));
            assert_eq!(counts, (1, 0), "step {i}");
            assert_eq!(sim.object(s.enemy).unwrap().show_counter_upgrade, 1);
        } else {
            assert!(sim.state.counter_upgrades.is_empty(), "step {i}");
        }
        // Training at slot 1 with -50% (W-19): 50 instead of 100 ms per building tick.
        if i == COUNTER_AT + 3 {
            assert_eq!(making(&sim, s.enemy_hq), (COLONIST, 14_950));
        }
        if i == COUNTER_AT + 201 {
            assert_eq!(making(&sim, s.enemy_hq), (COLONIST, 10_000));
        }
        let trained = i32::from(i >= TRAINED);
        assert_eq!(count(sim.state, 1, COLONIST), 1 + trained, "step {i}");
    }
    // 300 building ticks instead of 150: from the command to the colonist 601 `step` calls.
    assert_eq!(TRAINED - COUNTER_AT, 601);
    // Taxes: 50 object ticks without other gold movement bring 50 * 1000 raw.
    assert_eq!(gold_at[800] - gold_at[700], 50_000);
    // 4450 lasts 600 object ticks (60 s, config 13).
    assert_eq!(TAX_ENDS - TAX_STARTS, 1198);
    // The lumberjack delivers his first load (20) between `step` 600 and 700.
    assert_eq!(wood_at[700] - wood_at[600], 20 << 12);
    // At the end 3374 is still running (rest 15 s).
    assert_eq!(state.counter_upgrades[0].rest_ms, 15_000);
    // The idle colonist stays unarmed in variant 6 after 2806 (melee, no ranged combat).
    let sim = Sim::new(&mut state, &rules);
    let idle = sim.object(s.idle).unwrap();
    assert!(!arming::is_armed(&sim, s.idle));
    assert!(idle.has_module(fight_near::INDEX, false) && !idle.has_module(fight_far::INDEX, false));
}

/// Changed: new (research, counter-upgrade, governor, arming).
/// Changed: new state fields (gate guard at the building, area queries), behavior unchanged
/// (counter-check with `serde(skip)`).
/// Changed: module 44 gets the influence fields (serialization, behavior the same)
/// (counter-check with `serde(skip)` on `influences`/`influenced`: `0x36fb_e6b5_2818_d360` again).
/// Changed: module 38 gets target point and corpse (serialization, behavior the same)
/// (counter-check with `serde(skip)` on `target`/`corpse`: `0x48e6_a865_5606_92d9` again).
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
const UPGRADE_GOLDEN: u64 = 0x4aa8_de9d_1cbe_79bc;

/// Continuous sequence of the scene without saving, computed once per test process and shared by the
/// save tests (runtime).
static UPGRADE_SEQUENCE: LazyLock<Vec<u64>> =
    LazyLock::new(|| upgrade_run_probed(None, |_, _, _| {}));

#[test]
fn upgrade_checksums_match_the_golden_sequence() {
    let mut h = crate::rules::Fnv64::default();
    for c in UPGRADE_SEQUENCE.iter() {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, UPGRADE_GOLDEN, "{:#018x}", h.0);
}

/// Savegames of the scene (measured): in the middle of the research of 2800, between cast and
/// `0x326`, on the way to arming, in the middle of the temporary upgrade (without counter-upgrade), on
/// the way to the store (temporary and counter-upgrade running), in the middle of the counter-upgrade (after 4450).
/// Each save point is a test of its own; each sequence equals the continuous one.
const SAVE_IN_RESEARCH: usize = 200;
const SAVE_IN_SPELL: usize = 405;
const SAVE_ARMING: usize = 530;
const SAVE_IN_TEMP: usize = 700;
const SAVE_TO_STORE: usize = 1200;
const SAVE_IN_COUNTER: usize = 2000;

/// Scene with saving/loading after `save_at` against the continuous sequence.
fn assert_survives_saving(save_at: usize, at_save: UpgradeProbe, what: &str) {
    let saved = upgrade_run_probed(Some(save_at), at_save);
    let first_diff = saved
        .iter()
        .zip(UPGRADE_SEQUENCE.iter())
        .position(|(a, b)| a != b);
    assert_eq!(first_diff, None, "saved {what}: first deviation in `step`");
}

#[test]
fn upgrade_checksums_survive_saving_in_research() {
    assert_survives_saving(
        SAVE_IN_RESEARCH,
        |state, rules, s| {
            let sim = Sim::new(state, rules);
            let (guid, rest) = making(&sim, s.hq);
            assert_eq!(guid, WOOD_RESEARCH);
            assert!(rest > 0, "research running: {rest}");
            assert_eq!(count(sim.state, 0, WOOD_RESEARCH), 0);
        },
        "in the middle of the research",
    );
}

#[test]
fn upgrade_checksums_survive_saving_in_spell() {
    assert_survives_saving(
        SAVE_IN_SPELL,
        |state, rules, s| {
            let sim = Sim::new(state, rules);
            let g = sim.object(s.governor).unwrap();
            assert_ne!(g.status & status::ANIM_LOCKED, 0, "spell animation running");
            assert!(g.temp_upgrades.is_empty(), "before 0x326");
            assert_eq!(sim.attribute(s.governor, spell::MANA), 25);
        },
        "between cast and 0x326",
    );
}

#[test]
fn upgrade_checksums_survive_saving_while_arming() {
    assert_survives_saving(
        SAVE_ARMING,
        |state, rules, s| {
            let sim = Sim::new(state, rules);
            assert!(!arming::is_armed(&sim, s.armed), "on the way to the store");
            let walk = sim.module(s.armed, 0).unwrap();
            let arrived = crate::message::Message::new(arming::msg::ARRIVED, &[]);
            assert_eq!(walk.base.success, Some(arrived));
        },
        "on the way to arming",
    );
}

#[test]
fn upgrade_checksums_survive_saving_in_temp_upgrade() {
    assert_survives_saving(
        SAVE_IN_TEMP,
        |state, rules, s| {
            let sim = Sim::new(state, rules);
            assert_eq!(temps(&sim, s.governor), [(TAX, 45_600)]);
            assert!(sim.state.counter_upgrades.is_empty());
        },
        "in the middle of the temporary upgrade",
    );
}

#[test]
fn upgrade_checksums_survive_saving_on_the_way_to_the_store() {
    assert_survives_saving(
        SAVE_TO_STORE,
        |state, rules, s| {
            let sim = Sim::new(state, rules);
            let lumber = gather::state(&sim, s.lumberjack, 8);
            assert_eq!(lumber, gather_status::TO_DELIVERY);
            assert_eq!(temps(&sim, s.governor).len(), 1);
            assert_eq!(sim.state.counter_upgrades.len(), 1);
        },
        "on the way to the store",
    );
}

#[test]
fn upgrade_checksums_survive_saving_in_counter_upgrade() {
    assert_survives_saving(
        SAVE_IN_COUNTER,
        |state, rules, s| {
            let sim = Sim::new(state, rules);
            let c = &sim.state.counter_upgrades;
            assert_eq!(
                (c.len(), c[0].guid, c[0].source),
                (1, CHASTITY, s.missionary)
            );
            assert!((1..120_000).contains(&c[0].rest_ms), "{}", c[0].rest_ms);
            assert!(temps(&sim, s.governor).is_empty());
        },
        "in the middle of the counter-upgrade",
    );
}

/// Colonists in the slot table `slot` (also dying ones).
fn colonists_of_slot(state: &SimState, slot: u8) -> Vec<ObjectId> {
    super::objects_of_slot(state, slot)
        .into_iter()
        .filter(|&id| state.object(id, false).is_some_and(|o| o.guid == COLONIST))
        .collect()
}

/// Read the savegame and check it against the rules (like the viewer when loading).
fn assert_loads(state: &SimState, rules: &Rules) {
    let bytes = crate::save::testkit::save_bytes(state, rules);
    crate::save::SaveGame::from_bytes(&bytes)
        .expect("savegame readable")
        .check(rules)
        .expect("savegame valid");
}

// QUIRK K-300: kept
#[test]
fn an_upgrade_during_dying_leaves_the_colonist_corpse_standing() {
    // K-300 ticked: a colonist (modules with the variants 6/7, also dying 16) dies (HP 0 -> `0x110` in
    // the object tick, `step` 0); in the next `step`, before the change of owner to slot 8 at the end of the
    // phase 0 step, 2800 (OBJ_UPGRADES of 1100) completes (`0x39`, the same `Player_ApplyUpgrade` as the end
    // of a research). The variant term 485 fails on the dying one at the pre-check of
    // `GameObject_MatchesFilter` (DIED without `RETURN_DIED`): neither bit 6 nor 7, all modules with
    // variants go off - also dying (16).
    // Observed: the carcass still changes to slot 8 but no longer ticks (empty tick list);
    // decay and removal stay off, it still stands after 1000 object ticks. Without the upgrade
    // it is removed after about 330 object ticks. No panic; the savegame in this state loads
    // and continues bit-identically.
    // Not observed in the original; behavior of the reimplementation, consequence of K-300 = hypothesis (the mechanism
    // is confirmed by Ghidra, K-300, re/upgrades 3.4).
    const STEPS: usize = 2000;
    const SAVE_AT: usize = 2;
    let run = |with_upgrade: bool, save: bool| {
        let rules = rules();
        let mut state = state(&rules);
        let c = spawn(&mut Sim::new(&mut state, &rules), COLONIST, 0, (10, 10));
        Sim::new(&mut state, &rules)
            .object_mut(c)
            .unwrap()
            .attributes
            .set_current_fraction(14, 0);
        let mut sums = Vec::with_capacity(STEPS);
        let mut gone = None;
        for i in 0..STEPS {
            let mut cmds = Vec::new();
            if i == 1 {
                // Died in `step` 0 (tick group 0), change of owner still queued.
                let o = state.object(c, false).expect("still on slot 0");
                assert_ne!(o.status & status::DIED, 0);
                assert_eq!(state.owner_changes, [(c, 8)]);
                if with_upgrade {
                    cmds.push(Command {
                        player: 0,
                        kind: CommandKind::ApplyUpgrade {
                            slot: 0,
                            guid: WOOD_RESEARCH as i32,
                        },
                    });
                }
            }
            step(&mut state, &rules, &cmds);
            if save && i == SAVE_AT {
                assert_loads(&state, &rules);
                state = save_and_load(&state, &rules);
            }
            sums.push(crate::save::checksum(&state));
            if gone.is_none() && i > 1 && colonists_of_slot(&state, 8).is_empty() {
                gone = Some(i);
            }
        }
        (state, rules, sums, gone)
    };
    // Counter-check without upgrade: carcass on slot 8, decay after 30 s, then removed.
    let (_, _, _, gone) = run(false, false);
    assert!(gone.is_some_and(|g| g < 700), "carcass removed: {gone:?}");
    // With an upgrade during dying: the carcass stays.
    let (mut state, rules, sums, gone) = run(true, false);
    assert_eq!(gone, None, "the carcass stays standing");
    let corpses = colonists_of_slot(&state, 8);
    assert_eq!(corpses.len(), 1);
    assert!(colonists_of_slot(&state, 0).is_empty());
    let sim = Sim::new(&mut state, &rules);
    let corpse = sim.object(corpses[0]).unwrap();
    assert_ne!(corpse.status & status::DIED, 0);
    assert!(corpse.tick_list.is_empty(), "{:?}", corpse.tick_list);
    let dying = sim.module(corpses[0], crate::module::dying::INDEX).unwrap();
    assert!(!dying.base.enabled, "dying (16) off");
    // Saving and loading directly after the variant switch on the dying one: sequence bit-identical,
    // and the final state loads as well.
    let (_, _, saved, _) = run(true, true);
    assert_eq!(
        saved, sums,
        "saved after the variant switch on the dying one"
    );
    assert_loads(&state, &rules);
}
