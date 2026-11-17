use super::*;
use crate::command::CommandKind;
use crate::id::PLAYER_SLOTS;
use crate::module::ModuleKind;
use crate::module::building::MASK_SIDE;
use crate::setup::{MatchSetup, start_package};
use crate::world::{ObjectDesc, create_object};

mod crash;

const TYPES: &str = "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\nGUID 4201\nNAME Fahne\nVALUEID 100 6 1 0\nPROPID 5\nVALUEID 60002 6 1 256\n";

/// `part_checksums` has one entry per field of `SimState` (without `events`), and changing one field changes exactly
/// its entry. The destructuring without `..` in the source ensures completeness.
#[test]
fn part_checksums_cover_every_field() {
    let state = SimState::new(0, 0);
    let parts = part_checksums(&state);
    assert_eq!(parts.len(), 51);
    let mut names: Vec<_> = parts.iter().map(|p| p.0).collect();
    assert_eq!((names[0], names[names.len() - 1]), ("tick", "network"));
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 51, "names unique");
    let mut changed = state.clone();
    changed.daytime = 2;
    let diff: Vec<_> = part_checksums(&changed)
        .iter()
        .zip(&parts)
        .filter(|(a, b)| a.1 != b.1)
        .map(|(a, _)| a.0)
        .collect();
    assert_eq!(diff, ["daytime"]);
    assert_ne!(checksum(&changed), checksum(&state));
    let mut net = state.clone();
    net.network = true;
    let diff: Vec<_> = part_checksums(&net)
        .iter()
        .zip(&parts)
        .filter(|(a, b)| a.1 != b.1)
        .map(|(a, _)| a.0)
        .collect();
    assert_eq!(diff, ["network"]);
}

fn scene() -> (Rules, SimState) {
    let rules = Rules::synthetic(TYPES, 16);
    let mut state = new_match(
        &rules,
        &MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: crate::ki::KiSetup::default(),
            script: crate::script::ScriptSetup::default(),
            ..MatchSetup::default()
        },
    )
    .unwrap();
    let mut sim = crate::sim::Sim::new(&mut state, &rules);
    for i in 0..6 {
        let guid = if i % 2 == 0 { 1100 } else { 4201 };
        create_object(
            &mut sim,
            &mut ObjectDesc::new(guid, i % 3, [64.0 + 128.0 * i as f32, 0.0, 300.0]),
        );
    }
    state.events.clear();
    (rules, state)
}

/// World with a train (locomotive, tender, wagon, connected) and a cart (front and rear part, connected);
/// IDs `[locomotive, tender, wagon, front part, rear part]`.
fn train_world() -> (Rules, SimState, [crate::id::ObjectId; 5]) {
    use crate::module::multipart::{head_mut, part_mut};
    use crate::module::train::testkit as tk;
    use crate::module::train::train_mut;
    use crate::sim::Sim;
    let rules = tk::rules();
    let mut state = tk::state(&rules);
    let ids = {
        let mut sim = Sim::new(&mut state, &rules);
        let ids = [
            tk::spawn(&mut sim, tk::LOCO, 0, (10, 10)),
            tk::spawn(&mut sim, tk::TENDER, 0, (10, 12)),
            tk::spawn(&mut sim, tk::WAGON, 0, (10, 14)),
            tk::spawn(&mut sim, tk::CART, 0, (20, 10)),
            tk::spawn(&mut sim, tk::CART_TAIL, 0, (20, 12)),
        ];
        let [loco, tender, wagon, cart, tail] = ids;
        train_mut(&mut sim, loco).unwrap().slaves = vec![tender, wagon];
        for slave in [tender, wagon] {
            let t = train_mut(&mut sim, slave).unwrap();
            t.is_slave = true;
            t.master = loco;
        }
        head_mut(&mut sim, cart).unwrap().linked = tail;
        part_mut(&mut sim, tail).unwrap().prev = cart;
        ids
    };
    (rules, state, ids)
}

/// Train state of the object in the state (for corruptions).
fn train_of(s: &mut SimState, id: crate::id::ObjectId) -> &mut crate::module::train::Train {
    match &mut s
        .object_mut(id, false)
        .unwrap()
        .module_mut(crate::module::train::INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::Train(t) => t,
        _ => panic!("no train module"),
    }
}

/// Three finished houses as a wall group (master `a`, members `b`, `c`) as after command `0x18`.
fn wall_save() -> (SaveGame, [crate::id::ObjectId; 3]) {
    let (_, save, ids) = wall_world();
    (save, ids)
}

/// Like [`wall_save`], plus the rules (32 x 32 tiles).
fn wall_world() -> (Rules, SaveGame, [crate::id::ObjectId; 3]) {
    const WALL_TYPES: &str =
        "GUID 1000\nNAME Haus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n";
    let rules = Rules::synthetic(WALL_TYPES, 32);
    let setup = MatchSetup::default_for(&rules);
    let mut state = new_match(&rules, &setup).unwrap();
    let mut sim = crate::sim::Sim::new(&mut state, &rules);
    let mut desc = |x: f32| {
        let mut d = ObjectDesc::new(1000, 0, [x, 0.0, 1280.0]);
        d.flags |= crate::world::desc_flags::PLACED;
        create_object(&mut sim, &mut d).unwrap()
    };
    let ids = [desc(640.0), desc(1280.0), desc(1920.0)];
    let list = crate::message::Message::new(
        crate::module::building::msg::SET_SLAVE_LIST,
        &[2, ids[1].as_i32(), ids[2].as_i32()],
    );
    crate::messaging::send(&mut sim, ids[0], &list);
    state.events.clear();
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state,
        commands: Vec::new(),
    };
    (rules, save, ids)
}

fn building_of(
    save: &mut SaveGame,
    id: crate::id::ObjectId,
) -> &mut crate::module::building::Building {
    match &mut save.state.object_mut(id, false).unwrap().modules[3]
        .as_mut()
        .unwrap()
        .kind
    {
        ModuleKind::Building(b) => b,
        _ => panic!("building module expected"),
    }
}

#[test]
fn the_repair_message_flag_is_not_saved() {
    // `+0x74` is not saved by `PropertyBuilding_SerializeAndRestore` (re/buildings 1): after
    // loading, the message "Repair too expensive" may appear again.
    let (mut save, [a, _, _]) = wall_save();
    building_of(&mut save, a).repair_msg_shown = true;
    let before = checksum(&save.state);
    let mut loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
    assert!(!building_of(&mut loaded, a).repair_msg_shown);
    assert_eq!(checksum(&loaded.state), before, "not part of the checksum");
}

#[test]
fn saves_must_match_the_map_of_the_loaded_rules() {
    let (rules, state) = scene();
    let setup = MatchSetup {
        mode: -1,
        seed: 0,
        lobby: Vec::new(),
        resource_setting: 0,
        living_space: 2,
        map_explored: false,
        ki: crate::ki::KiSetup::default(),
        script: crate::script::ScriptSetup::default(),
        ..MatchSetup::default()
    };
    // Start package of the loaded rules, but grid of a map twice as large: structurally valid, does not match the
    // map.
    let bigger = Rules::synthetic(TYPES, 32);
    let big = new_match(&bigger, &setup).unwrap();
    let mismatch = SaveGame {
        start: start_package(&rules, &setup),
        state: big,
        commands: Vec::new(),
    };
    let loaded = SaveGame::from_bytes(&mismatch.to_bytes()).unwrap();
    let err = loaded.check(&rules).unwrap_err().to_string();
    assert!(err.contains("32 x 32") && err.contains("16 x 16"), "{err}");
    let matching = SaveGame {
        start: start_package(&rules, &setup),
        state,
        commands: Vec::new(),
    };
    SaveGame::from_bytes(&matching.to_bytes())
        .unwrap()
        .check(&rules)
        .unwrap();
}

#[test]
fn checksums_change_with_the_state_and_match_the_serialisation() {
    let (rules, mut state) = scene();
    let a = checksum(&state);
    let mut h = Fnv64::default();
    h.write(&postcard::to_stdvec(&state).unwrap());
    assert_eq!(a, h.0);
    step(&mut state, &rules, &[]);
    assert_ne!(checksum(&state), a);
    // Events do not count.
    let b = checksum(&state);
    state.events.push(crate::state::SimEvent::Destroyed {
        id: crate::id::ObjectId(1),
    });
    state.events.push(crate::state::SimEvent::UnitResponse {
        unit: crate::id::ObjectId(1),
        action: crate::command::RESPONSE_EXECUTE,
        owner: 0,
        guid: 1100,
    });
    assert_eq!(checksum(&state), b);
}

#[test]
fn the_checksum_sequence_is_fixed_in_debug_and_release() {
    // Fixed sequence: the same in `cargo test` and `cargo test --release`.
    let (rules, mut state) = scene();
    let mut sequence = Vec::new();
    for i in 0..40u64 {
        let commands = if i == 7 {
            vec![Command {
                player: 0,
                kind: CommandKind::Diplomacy {
                    p: 0,
                    q: 1,
                    mode: 0,
                },
            }]
        } else {
            Vec::new()
        };
        step(&mut state, &rules, &commands);
        sequence.push(checksum(&state));
    }
    let mut h = Fnv64::default();
    for c in &sequence {
        h.write(&c.to_le_bytes());
    }
    assert_eq!(h.0, GOLDEN, "checksum sequence {h:016x?}", h = h.0);
}

/// Checksum over the 40 checksums of `the_checksum_sequence_is_fixed_in_debug_and_release`.
/// Changed: walk combat fields and behavior default Defensive (home follows, tether sight*1024); positions, status
/// and animation of the objects at the end the same as before.
/// Changed: new state fields (building map, projectiles, hit zone counter), behavior unchanged.
/// Changed: new state fields groups/walk (FORMAT_VERSION 8), behavior unchanged.
/// Changed: new state fields (event bits, last track, meeting point), behavior unchanged.
/// Changed: new state fields (upgrades, counter-upgrades, temporary upgrades).
/// Changed: time-of-day upgrades at level start.
/// Changed: new state fields (gate guard at the building, area queries), behavior unchanged (counter-check with
/// `serde(skip)`).
/// Changed: world field `headhunters` (serialization, behavior the same).
/// Changed: state field `ki` (AI state, serialization, behavior the same; counter-check with `serde(skip)`).
/// Changed: state fields `ai_marks`, `attack_reports` and `campaign_difficulty` (serialization, behavior the same;
/// counter-check with `serde(skip)`).
/// Changed: state fields `script`, `names` and evocation counter (serialization, behavior the same; counter-check
/// with `serde(skip)`).
/// Changed: event bits 1, 2, 3, 5, 7, kill counter, message sequence `0x2100`/`0x611` and new world fields
/// (counter-check: without setter and with `serde(skip)` the values of the preceding step).
/// Changed: statistics log and timer list (serialization, behavior the same; counter-check with `serde(skip)`).
/// Changed: serialization of `accounting` and `countdowns` added to the value of the preceding step (counter-check:
/// with `serde(skip)` on both, the values of the preceding step).
/// Changed: serialization of new fields (presentation state, mirror); behavior the same (counter-check with
/// `serde(skip)`).
/// Changed: list of idle workers (serialization of `SimState::idle_workers`, FORMAT_VERSION 26); behavior the same
/// (counter-check with `serde(skip)` in debug and release).
/// Changed: flag leader and time limit in the state (serialization of `SimState::flag_leader` and `time_limit`,
/// FORMAT_VERSION 27); behavior the same (counter-check with `serde(skip)`: all values unchanged).
/// Changed: new field `SimState::network` (serialization, FORMAT_VERSION 29; behavior the same; counter-check with
/// `serde(skip)`: all values unchanged).
const GOLDEN: u64 = 0x92ea_d64c_ff00_fcd5;

/// Economy scene: deposits (gold, soil) and a chapel with self-healing (600 HP per minute) in addition to the
/// template `command::testkit`.
const ECONOMY_EXTRA_TYPES: &str = concat!(
    "GUID 1008\nNAME SPA_Kapelle\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n",
    "PROPID 40\nVALUEID 410001 6 1 600\n",
    "GUID 3500\nNAME Goldvorkommen\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 1280\n",
    "GUID 3503\nNAME Boden\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 1536\n",
);
const CHAPEL: u32 = 1008;

/// IDs of the economy scene.
struct EconomyScene {
    hq: crate::id::ObjectId,
    market: crate::id::ObjectId,
    store: crate::id::ObjectId,
    mine: crate::id::ObjectId,
    farm: crate::id::ObjectId,
    chapel: crate::id::ObjectId,
    tree: crate::id::ObjectId,
    /// Second tree for the construction workers after the build.
    tree2: crate::id::ObjectId,
    /// Construction workers (2), miner, farmer, lumberjack, quartered guest.
    workers: [crate::id::ObjectId; 6],
}

/// Rules and start state of the economy scene (64 x 64 tiles, slot 0): HQ (20, 20) with quarters, trading post
/// (20, 26), warehouse (30, 14) for selling, gold mine (12, 20) with 500 gold, finca (20, 12) with 400 food, damaged
/// chapel (34, 30, 500 of 1000 HP), trees (34, 22) and (36, 16) and six colonists; account 1000/1000/1000, living
/// space 10.
fn economy_scene() -> (Rules, SimState, EconomyScene) {
    use crate::command::testkit as kit;
    let base = kit::rules();
    let types = format!("{}{ECONOMY_EXTRA_TYPES}", kit::TYPES);
    let mut rules = Rules::synthetic(&types, 64);
    rules.defaults = base.defaults;
    rules.building_shapes = base.building_shapes;
    rules.model_boxes = base.model_boxes;
    rules.toolbar = base.toolbar;
    rules.targets = base.targets;
    rules
        .defaults
        .insert(CHAPEL, [(13, 20), (14, 1000)].into_iter().collect());
    let hq_shape = rules.building_shapes[&kit::HQ].clone();
    rules.building_shapes.insert(CHAPEL, hq_shape);
    let mut state = kit::state(&rules);
    let scene = {
        let mut sim = crate::sim::Sim::new(&mut state, &rules);
        let deposit = |sim: &mut crate::sim::Sim, guid: u32, cfg: u32, amount, tile| {
            sim.state.object_config = [(cfg, amount)].into_iter().collect();
            kit::spawn(sim, guid, 10, tile);
            sim.state.object_config.clear();
        };
        deposit(&mut sim, 3500, 26, 500, (12, 20));
        deposit(&mut sim, 3503, 24, 400, (20, 12));
        let hq = kit::spawn(&mut sim, kit::HQ, 0, (20, 20));
        let market = kit::spawn(&mut sim, kit::MARKET, 0, (20, 26));
        let store = kit::spawn(&mut sim, kit::STORE, 0, (30, 14));
        let mine = kit::spawn(&mut sim, kit::MINE, 0, (12, 20));
        let farm = kit::spawn(&mut sim, kit::FARM, 0, (20, 12));
        let chapel = kit::spawn(&mut sim, CHAPEL, 0, (34, 30));
        sim.object_mut(chapel)
            .unwrap()
            .attributes
            .set_current_value(14, 500, 0);
        let tree = kit::spawn(&mut sim, kit::TREE, 9, (34, 22));
        let tree2 = kit::spawn(&mut sim, kit::TREE, 9, (36, 16));
        let workers = [(26, 24), (28, 24), (14, 24), (22, 8), (30, 22), (24, 24)]
            .map(|t| kit::spawn(&mut sim, kit::COLONIST, 0, t));
        kit::account(&mut sim, 0, [1000, 1000, 1000, 0, 10]);
        EconomyScene {
            hq,
            market,
            store,
            mine,
            farm,
            chapel,
            tree,
            tree2,
            workers,
        }
    };
    state.events.clear();
    (rules, state, scene)
}

/// Commands of the economy scene before the tick `step`: immediately house (26, 16) with two workers, gold, harvest
/// and wood; tick 2 colonist, buy and sell wood, rally point; tick 4 quarter; tick 6 sell warehouse; tick 300 unquarter
/// all; tick 700 the construction workers to the second tree (at the first one both places are occupied as soon as the
/// first lumberjack returns: a third would find none).
fn economy_commands(step: u64, s: &EconomyScene) -> Vec<Command> {
    use crate::command::testkit as kit;
    let w = s.workers;
    let kinds = match step {
        0 => vec![
            kit::place(kit::HOUSE, (26, 16), &[w[0], w[1]]),
            CommandKind::MineGold {
                mine: s.mine,
                units: vec![w[2]],
            },
            CommandKind::Harvest {
                farm: s.farm,
                units: vec![w[3]],
            },
            CommandKind::ChopWood {
                tree: s.tree,
                units: vec![w[4]],
            },
        ],
        2 => vec![
            CommandKind::EnqueueProduction {
                building: s.hq,
                guid: kit::COLONIST,
            },
            CommandKind::EnqueueProduction {
                building: s.market,
                guid: 490,
            },
            CommandKind::EnqueueProduction {
                building: s.market,
                guid: 492,
            },
            CommandKind::SetRallyPoint {
                x: (24 << 12) + 0x800,
                z: (30 << 12) + 0x800,
                buildings: vec![s.hq],
            },
        ],
        4 => vec![CommandKind::Quarter {
            container: s.hq,
            units: vec![w[5]],
        }],
        6 => vec![CommandKind::SellBuildings {
            buildings: vec![s.store],
        }],
        300 => vec![CommandKind::UnquarterAll { container: s.hq }],
        700 => vec![CommandKind::ChopWood {
            tree: s.tree2,
            units: vec![w[0], w[1]],
        }],
        _ => Vec::new(),
    };
    kinds
        .into_iter()
        .map(|kind| Command { player: 0, kind })
        .collect()
}

/// Ticks of the economy scene in [`the_economy_checksum_sequence_is_fixed_in_debug_and_release`].
const ECONOMY_STEPS: u64 = 1600;

#[test]
fn the_economy_checksum_sequence_is_fixed_in_debug_and_release() {
    // Fixed sequence over building, gathering (gold, harvest, wood), training, market, selling, quarters and
    // self-healing: the same in `cargo test` and `cargo test --release`.
    use crate::economy::resource::{FOOD, GOLD, WOOD};
    let (rules, mut state, s) = economy_scene();
    let colonists = |state: &SimState| {
        state
            .objects()
            .filter(|o| o.guid == crate::command::testkit::COLONIST)
            .count()
    };
    let chapel_hp = |state: &SimState| {
        state
            .object(s.chapel, false)
            .unwrap()
            .attributes
            .current_value(14, 0)
    };
    let (units0, hp0) = (colonists(&state), chapel_hp(&state));
    let mut quartered = false;
    let mut responses = Vec::new();
    let mut h = Fnv64::default();
    for i in 0..ECONOMY_STEPS {
        let events = step(&mut state, &rules, &economy_commands(i, &s));
        responses.extend(events.iter().filter_map(|e| match *e {
            crate::state::SimEvent::UnitResponse { unit, action, .. } => Some((i, unit, action)),
            _ => None,
        }));
        h.write(&checksum(&state).to_le_bytes());
        quartered |= state
            .object(s.workers[5], false)
            .is_some_and(|o| o.status & crate::object::status::QUARTERED != 0);
    }
    // The scene really runs through all parts.
    let houses: Vec<_> = state
        .objects()
        .filter(|o| o.guid == crate::command::testkit::HOUSE)
        .map(|o| o.id)
        .collect();
    let sim = crate::sim::Sim::new(&mut state, &rules);
    assert!(crate::command::validate::is_finished(&sim, houses[0]));
    assert!(sim.object(s.store).is_none(), "warehouse sold");
    let unit = sim.object(s.workers[5]).unwrap();
    assert!(quartered && unit.status & crate::object::status::QUARTERED == 0);
    assert_eq!(colonists(&state), units0 + 1);
    assert!(chapel_hp(&state) > hp0);
    assert_ne!(state.economy.wood_price, crate::economy::WOOD_PRICE_START);
    // Account after 80 s of game time: gold 1000 - 25 (100 wood bought) + 100 (wood sold) + 3 x
    // 20 mined; wood 1000 - 150 (house) + 100 - 398 (sale: 100 gold at price 0.251) +
    // 4 x 20 felled; food 1000 - 60 (colonist) + 2 x 20 harvested; wood price after purchase and
    // sale 1013.
    assert_eq!(
        [GOLD, WOOD, FOOD].map(|i| state.resource(0, i)),
        [1135, 632, 980]
    );
    assert_eq!(state.economy.wood_price, 1013);
    // The scene acknowledges its direct commands (events, not in the checksum): construction with
    // two workers, gold, harvest, wood, quarters, wood with two workers; the cyclic
    // "next tree" ones from the queue are silent.
    let w = s.workers;
    assert_eq!(
        responses,
        [
            (0, w[0], 12),
            (0, w[1], 12),
            (0, w[2], 12),
            (0, w[3], 12),
            (0, w[4], 12),
            (4, w[5], 12),
            (700, w[0], 12),
            (700, w[1], 12),
        ]
    );
    assert_eq!(h.0, ECONOMY_GOLDEN, "checksum sequence {h:016x?}", h = h.0);
}

/// Checksum over the checksums of the economy scene (per tick, [`ECONOMY_STEPS`] ticks).
/// Changed: walk combat fields and behavior default Defensive (home follows, tether sight*1024); positions, status
/// and animation of the objects at the end the same as before; quartering (`0x2100` with abort from
/// `enter_container`) now takes effect, but the unit was already standing (original: `GameObject_EnterContainer`
/// `0x004712A0`, re/garrison-and-commands).
/// Changed: new state fields (building map, projectiles, hit zone counter), behavior unchanged.
/// Changed: building stamps of the line-of-sight map in the state.
/// Changed: new state fields groups/walk (FORMAT_VERSION 8), behavior unchanged.
/// Changed: new state fields (event bits, last track, meeting point), behavior unchanged.
/// Changed: new state fields (upgrades, counter-upgrades, temporary upgrades).
/// Changed: new state fields (gate guard at the building, area queries), behavior unchanged (counter-check with
/// `serde(skip)`).
/// Changed: world field `headhunters` (serialization, behavior the same).
/// Changed: state field `ki` (AI state, serialization, behavior the same; counter-check with `serde(skip)`).
/// Changed: state fields `ai_marks`, `attack_reports` and `campaign_difficulty` (serialization, behavior the same;
/// counter-check with `serde(skip)`).
/// Changed: state fields `script`, `names` and evocation counter (serialization, behavior the same; counter-check
/// with `serde(skip)`).
/// Changed: event bits 1, 2, 3, 5, 7, kill counter, message sequence `0x2100`/`0x611` and new world fields
/// (counter-check: without setter and with `serde(skip)` the values of the preceding step).
/// Changed: statistics log and timer list (serialization, behavior the same; counter-check with `serde(skip)`).
/// Changed: serialization of `accounting` and `countdowns` added to the value of the preceding step (counter-check:
/// with `serde(skip)` on both, the values of the preceding step).
/// Changed: serialization of new fields (presentation state, mirror); behavior the same (counter-check with
/// `serde(skip)`).
/// Changed: list of idle workers (serialization of `SimState::idle_workers`, FORMAT_VERSION 26); behavior the same
/// (counter-check with `serde(skip)` in debug and release).
/// Changed: flag leader and time limit in the state (serialization of `SimState::flag_leader` and `time_limit`,
/// FORMAT_VERSION 27); behavior the same (counter-check with `serde(skip)`: all values unchanged).
/// Changed: new field `SimState::network` (serialization, FORMAT_VERSION 29; behavior the same; counter-check with
/// `serde(skip)`: all values unchanged).
const ECONOMY_GOLDEN: u64 = 0x6ab8_7d95_add6_1886;

#[test]
fn economy_save_games_load_at_every_tick_and_continue_identically() {
    // Partial run of the economy scene: save, load and continue at every tick.
    let (rules, state, s) = economy_scene();
    let setup = MatchSetup::default_for(&rules);
    let start = start_package(&rules, &setup);
    let mut a = state.clone();
    let mut b = state;
    for i in 0..400 {
        let bytes = SaveGame {
            start: start.clone(),
            state: b.clone(),
            commands: Vec::new(),
        }
        .to_bytes();
        b = SaveGame::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("tick {i}: {e}"))
            .state;
        let commands = economy_commands(i, &s);
        step(&mut a, &rules, &commands);
        step(&mut b, &rules, &commands);
        assert_eq!(checksum(&a), checksum(&b), "tick {i}");
    }
}

#[test]
fn hunting_checksums_survive_saving_and_loading_mid_gutting_and_mid_roam() {
    // Hunter while gutting (gutting timer, leak, load, carcass with rotting timer) and a roaming goat (walk,
    // animal module) in the same scene; from the first meat on, for 60 steps save, load and continue at every
    // step.
    use crate::combat::testkit as ck;
    use crate::command::testkit::STORE;
    use crate::message::Message;
    use crate::messaging::send;
    use crate::module::ModuleKind;
    use crate::sim::Sim;
    let rules = ck::rules();
    let mut state = ck::state(&rules);
    let (hunter, prey, roamer) = {
        let mut sim = Sim::new(&mut state, &rules);
        let prey = ck::spawn(&mut sim, ck::GOAT, 8, (10, 10));
        let hunter = ck::spawn(&mut sim, ck::HUNTER, 0, (10, 10));
        ck::spawn(&mut sim, STORE, 0, (16, 10));
        let roamer = ck::spawn(&mut sim, ck::GOAT, 8, (40, 40));
        sim.object_mut(prey)
            .unwrap()
            .attributes
            .set_current_value(14, 0, 0);
        (hunter, prey, roamer)
    };
    // Until the carcass is ready (render bits 0x42), then have it gutted.
    for _ in 0..40 {
        step(&mut state, &rules, &[]);
        if state.object(prey, false).unwrap().render_bits == 0x42 {
            break;
        }
    }
    assert_eq!(state.object(prey, false).unwrap().render_bits, 0x42);
    {
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(
            send(&mut sim, hunter, &Message::new(0x216, &[prey.as_i32()])),
            1
        );
    }
    // The goat is already roaming: trigger the roaming `0x714` until a roaming order is running (a draw that is
    // too short reports success immediately, re/animals 6.2).
    let roaming = |s: &SimState| match &s.object(roamer, false).unwrap().modules[0x14]
        .as_ref()
        .unwrap()
        .kind
    {
        ModuleKind::Animal(x) => x.roaming,
        _ => false,
    };
    for _ in 0..10 {
        if roaming(&state) {
            break;
        }
        let mut sim = Sim::new(&mut state, &rules);
        send(&mut sim, roamer, &Message::new(0x714, &[]));
    }
    assert!(
        roaming(&state),
        "roaming order is running at the first save"
    );
    let load = |s: &SimState| s.object(hunter, false).unwrap().load_amount;
    let meat = |s: &SimState| {
        s.object(prey, false)
            .unwrap()
            .attributes
            .current_value(24, 0)
    };
    for _ in 0..200 {
        step(&mut state, &rules, &[]);
        if load(&state) > 0 {
            break;
        }
    }
    assert!(load(&state) > 0, "gutting has begun");
    let setup = MatchSetup::default_for(&rules);
    let start = start_package(&rules, &setup);
    let (load0, meat0) = (load(&state), meat(&state));
    let pos = |s: &SimState| {
        let o = s.object(roamer, false).unwrap();
        (o.grid.x.0, o.grid.z.0)
    };
    let pos0 = pos(&state);
    let mut a = state.clone();
    let mut b = state;
    let mut roamed = std::collections::BTreeSet::new();
    for i in 0..60 {
        let bytes = SaveGame {
            start: start.clone(),
            state: b.clone(),
            commands: Vec::new(),
        }
        .to_bytes();
        b = SaveGame::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("step {i}: {e}"))
            .state;
        step(&mut a, &rules, &[]);
        step(&mut b, &rules, &[]);
        assert_eq!(checksum(&a), checksum(&b), "step {i}");
        roamed.insert(pos(&a));
    }
    // Evidence: gutting really happened at saving and the goat roamed.
    assert!(load(&a) > load0, "load {load0} -> {}", load(&a));
    assert!(meat(&a) < meat0, "meat {meat0} -> {}", meat(&a));
    assert!(roamed.iter().any(|p| *p != pos0), "goat roams: {roamed:?}");
}

#[test]
fn fog_cycles_survive_saving_and_loading_at_every_step() {
    // Two units (slots 0 and 1) and 38 trees: cycle of two stamp ticks, an empty tick and state -2;
    // the player switches in the middle of the tick.
    fog_cycle_save_scene(&[(0, (10, 10)), (1, (40, 10))], 38);
    // Three units on slot 0 with budget 2: after the first tick the cursor stands behind
    // the second unit; a cursor lost on loading would give `processed` 4 instead of -1.
    fog_cycle_save_scene(&[(0, (10, 10)), (0, (12, 10)), (0, (14, 10))], 37);
}

/// Scene of `units` (slot, tile) and `trees` trees (40 objects, budget 2): 12 ticks, at every tick save, load and
/// continue right away.
fn fog_cycle_save_scene(units: &[(i32, (i32, i32))], trees: i32) {
    use crate::command::testkit as kit;
    let rules = kit::rules();
    let mut state = kit::fogged_state(&rules);
    {
        let mut sim = crate::sim::Sim::new(&mut state, &rules);
        for &(slot, tile) in units {
            kit::spawn(&mut sim, kit::COLONIST, slot, tile);
        }
        for i in 0..trees {
            kit::spawn(&mut sim, kit::TREE, 9, (i, 50));
        }
    }
    assert_eq!(state.object_count(), 40);
    let setup = MatchSetup::default_for(&rules);
    let mut reference = state.clone();
    let mut sums = Vec::new();
    for _ in 0..12 {
        step(&mut reference, &rules, &[]);
        sums.push(checksum(&reference));
    }
    for at in 0..12 {
        let mut s = state.clone();
        for _ in 0..at {
            step(&mut s, &rules, &[]);
        }
        let save = SaveGame {
            start: start_package(&rules, &setup),
            state: s,
            commands: Vec::new(),
        };
        let mut s = SaveGame::from_bytes(&save.to_bytes()).unwrap().state;
        for (i, sum) in sums.iter().enumerate().skip(at) {
            step(&mut s, &rules, &[]);
            assert_eq!(checksum(&s), *sum, "loaded after tick {at}, tick {i}");
        }
    }
}

/// Cost of the checksum per tick with fog grids on 512 x 512 tiles. Measurement, not a check value:
/// `cargo test -p nml-sim --release checksum_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn checksum_cost_on_a_512_map() {
    let rules = Rules::synthetic("", 512);
    let mut state = SimState::new(0, 0);
    state.init_map(&rules);
    let start = std::time::Instant::now();
    let mut h = 0u64;
    for _ in 0..20 {
        h ^= checksum(&state);
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / 20.0;
    println!("Checksum 512 x 512: {ms:.2} ms per tick ({h:x})");
}

#[test]
fn save_games_restore_the_state_and_reject_other_versions_and_rules() {
    let (rules, mut state) = scene();
    for _ in 0..11 {
        step(&mut state, &rules, &[]);
    }
    let setup = MatchSetup {
        mode: -1,
        seed: 0,
        lobby: Vec::new(),
        resource_setting: 0,
        living_space: 2,
        map_explored: false,
        ki: crate::ki::KiSetup::default(),
        script: crate::script::ScriptSetup::default(),
        ..MatchSetup::default()
    };
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: state.clone(),
        commands: Vec::new(),
    };
    let bytes = save.to_bytes();
    let loaded = SaveGame::from_bytes(&bytes).unwrap();
    loaded.check(&rules).unwrap();
    assert_eq!(checksum(&loaded.state), checksum(&state));
    // Continuing yields the same sequence.
    let mut a = state;
    let mut b = loaded.state;
    for _ in 0..30 {
        step(&mut a, &rules, &[]);
        step(&mut b, &rules, &[]);
        assert_eq!(checksum(&a), checksum(&b));
    }
    let mut wrong = bytes.clone();
    wrong[8] = 99;
    assert!(
        SaveGame::from_bytes(&wrong)
            .unwrap_err()
            .to_string()
            .contains("format version 99")
    );
    assert!(SaveGame::from_bytes(b"NMLREPL\0\x01\0\0\0").is_err());
    let mut other = rules.clone();
    other.checksum = 7;
    assert!(
        save.check(&other)
            .unwrap_err()
            .to_string()
            .contains("rules differ")
    );
}

#[test]
fn network_flag_must_match_the_start_package() {
    // The network flag of the state follows the start package (`new_match` sets it from `MatchSetup::network`); a
    // mismatch counts as corrupted.
    let (rules, state) = scene();
    for network in [false, true] {
        let setup = MatchSetup {
            network,
            ..MatchSetup::default()
        };
        let mut save = SaveGame {
            start: start_package(&rules, &setup),
            state: state.clone(),
            commands: Vec::new(),
        };
        save.state.network = network;
        save.check(&rules).expect("flag as in the start package");
        save.state.network = !network;
        let err = format!("{:#}", save.check(&rules).unwrap_err());
        assert_eq!(
            err,
            format!(
                "savegame corrupted (network flag {}, in the start package {network})",
                !network
            )
        );
    }
}

#[test]
fn replays_repeat_the_commands_at_their_steps() {
    let rules = Rules::synthetic(TYPES, 16);
    let setup = MatchSetup {
        mode: -1,
        seed: 0,
        lobby: Vec::new(),
        resource_setting: 0,
        living_space: 2,
        map_explored: false,
        ki: crate::ki::KiSetup::default(),
        script: crate::script::ScriptSetup::default(),
        ..MatchSetup::default()
    };
    let diplomacy = |mode| Command {
        player: 0,
        kind: CommandKind::Diplomacy { p: 0, q: 1, mode },
    };
    let replay = Replay {
        start: start_package(&rules, &setup),
        commands: vec![
            RecordedCommand {
                step: 3,
                command: diplomacy(0),
            },
            RecordedCommand {
                step: 9,
                command: diplomacy(1),
            },
        ],
    };
    let replay = Replay::from_bytes(&replay.to_bytes()).unwrap();
    let at = |steps| replay.play(&rules, steps).unwrap();
    assert!(!at(3).diplomacy.is_friend(0, 1));
    assert!(at(4).diplomacy.is_friend(0, 1));
    assert!(at(10).diplomacy.is_enemy(0, 1));
    assert_eq!(at(10).steps, 10);
}

fn first(save: &mut SaveGame) -> &mut crate::object::GameObject {
    save.state
        .tables
        .iter_mut()
        .flat_map(|t| t.objects.iter_mut().flatten())
        .next()
        .unwrap()
}

/// Colonist with walk (as in `module/walk/tests.rs`).
const WALK_TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\nVALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\nPROPID 2\nVALUEID 30001 1 1 0\n";

/// Two units walk head-on toward each other; state after 70 ticks (planner, records and walk states are in use).
/// Plus their IDs.
fn walking_save() -> (Rules, SaveGame, [crate::id::ObjectId; 2]) {
    let mut rules = Rules::synthetic(WALK_TYPES, 32);
    rules
        .defaults
        .insert(1100, [(16, 183)].into_iter().collect());
    let setup = MatchSetup {
        mode: -1,
        seed: 0,
        lobby: Vec::new(),
        resource_setting: 0,
        living_space: 2,
        map_explored: false,
        ki: crate::ki::KiSetup::default(),
        script: crate::script::ScriptSetup::default(),
        ..MatchSetup::default()
    };
    let mut state = new_match(&rules, &setup).unwrap();
    let world = |t: i32| t as f32 * 128.0 + 64.0;
    let ids = {
        let mut sim = crate::sim::Sim::new(&mut state, &rules);
        [(4, 6), (14, 6)].map(|(x, z)| {
            create_object(
                &mut sim,
                &mut ObjectDesc::new(1100, 0, [world(x), 0.0, world(z)]),
            )
            .unwrap()
        })
    };
    let tile = |t: i32| (t << 12) + 0x800;
    let mv = |unit, x| Command {
        player: 0,
        kind: CommandKind::Move {
            units: vec![unit],
            x: tile(x),
            z: tile(6),
            target: -1,
            formation: 0,
            direction: crate::motion::NO_DIRECTION,
            spread: crate::command::NO_SPREAD,
            flags: 0,
        },
    };
    step(&mut state, &rules, &[mv(ids[0], 16), mv(ids[1], 2)]);
    for _ in 0..70 {
        step(&mut state, &rules, &[]);
    }
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state,
        commands: Vec::new(),
    };
    (rules, save, ids)
}

fn walk_of(save: &mut SaveGame, id: crate::id::ObjectId) -> &mut crate::module::walk::Walk {
    save.state
        .object_mut(id, false)
        .unwrap()
        .modules
        .iter_mut()
        .flatten()
        .find_map(|m| match &mut m.kind {
            ModuleKind::Walk(w) => Some(&mut **w),
            _ => None,
        })
        .expect("walk module")
}

#[test]
fn running_save_games_load_at_every_tick_and_continue_identically() {
    // The new checks must not reject a valid running state.
    let (rules, save, _) = walking_save();
    let mut a = save.state.clone();
    let mut b = save.state;
    for i in 0..200 {
        let bytes = SaveGame {
            start: save.start.clone(),
            state: b.clone(),
            commands: Vec::new(),
        }
        .to_bytes();
        b = SaveGame::from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("tick {i}: {e}"))
            .state;
        step(&mut a, &rules, &[]);
        step(&mut b, &rules, &[]);
        assert_eq!(checksum(&a), checksum(&b), "tick {i}");
    }
}

/// List range of the AI savegame tests: kind 2, type 0 (range key 20), two lists.
const KI_LIST: u32 = 0x0200_0000;

/// Catalog of the AI savegame tests: module 1 calls module 2 (`CALLM`), module 2 pauses 1000 ms (`0x8E`), module 3
/// loops with two lists.
fn ki_catalog() -> crate::ki::catalog::KiCatalog {
    use crate::ki::testkit::{catalog, int, proto, word};
    use crate::ki::vm::op;
    catalog(vec![
        proto(
            1,
            &[(0, 4)],
            &[],
            &[word(op::CALLM, 2), word(op::PUSHA, 1), word(op::JMP, 1)],
        ),
        proto(
            2,
            &[(0, 4)],
            &[(int(0), 1000)],
            &[
                word(op::LOAD, int(0)),
                word(op::CALLB, 0x8E),
                word(op::PUSHA, 7),
            ],
        ),
        proto(
            3,
            &[(0, 4), (KI_LIST, 2)],
            &[],
            &[
                word(op::PUSHA, 5),
                word(op::STORE3, 0x0FFF_FFFF),
                word(op::JMP, 0),
            ],
        ),
    ])
}

/// Scene with AI: engine on slot 2 with thread 0 (module 1, in the `CALLM` with the remaining pause of the child) and
/// thread 1 (module 3, lists with content), plus engine memory with content.
fn ki_scene() -> (Rules, SimState) {
    use crate::ki::engine::{Engine, KiCtx};
    use crate::ki::vm::status;
    let (mut rules, mut state) = scene();
    rules.ki = ki_catalog();
    let mut world = crate::ki::testkit::TestWorld::default();
    let mut engine = Engine::new(2);
    let mut cx = KiCtx::new(&rules.ki, &mut state.ki.rand, &mut world);
    assert_eq!(engine.spawn(&mut cx, 1, &[]), Some(0));
    assert_eq!(engine.spawn(&mut cx, 3, &[4]), Some(1));
    engine.tick(&mut cx);
    engine.tick(&mut cx);
    let t0 = engine.threads.get(0).expect("thread 0");
    assert_eq!(t0.status, status::RUNNING | status::CHILD);
    let child = t0.child.as_deref().expect("child of thread 0");
    assert!(
        child.pause_ms > 0 && child.pause_ms < 1000,
        "remaining pause"
    );
    let t1 = engine.threads.get_mut(1).expect("thread 1");
    let lists = &mut t1.vars.ranges.get_mut(&20).expect("list range").lists;
    lists[0].insert(3, 4);
    lists[0].insert(19, -1);
    lists[1].insert(17, 9);
    let m = &mut engine.mem;
    m.semaphores.insert(5, 1);
    m.values.insert(1, 2);
    m.goto.insert(7, 8);
    m.tables.entry(2).or_default().insert(3, 4);
    m.attackers[3].push(42);
    m.units[1].push(10, 20, 30);
    m.building_queue.push(1100);
    m.repairer_counts.push(2);
    state.ki.engines[2] = Some(engine);
    (rules, state)
}

/// `MfcMap` with exactly these chains (buckets, per chain head first), also with wrong buckets or duplicate keys:
/// via the serialization, like a corrupted savegame.
fn raw_map<V: Serialize + for<'de> Deserialize<'de>>(
    buckets: Vec<Vec<(i32, V)>>,
) -> crate::mfc::MfcMap<V> {
    postcard::from_bytes(&postcard::to_stdvec(&buckets).unwrap()).unwrap()
}

fn ki_engine(s: &mut SimState) -> &mut crate::ki::engine::Engine {
    s.ki.engines[2].as_mut().expect("engine 2")
}

fn ki_thread(s: &mut SimState, id: i32) -> &mut crate::ki::vm::Thread {
    ki_engine(s).threads.get_mut(id).expect("thread")
}

// QUIRK E-10: deviation
#[test]
fn ki_state_survives_saving_and_loading() {
    let (rules, state) = ki_scene();
    let bytes = testkit::save_bytes(&state, &rules);
    let loaded = SaveGame::from_bytes(&bytes).expect("load");
    loaded.check(&rules).expect("matches the catalog");
    assert_eq!(loaded.state.ki, state.ki);
    assert_eq!(loaded.state, state);
    assert_eq!(checksum(&loaded.state), checksum(&state));
    assert_eq!(checksum_of(&loaded.state.ki), checksum_of(&state.ki));
    // The AI state is part of the checksum.
    let mut other = state.clone();
    ki_engine(&mut other).mem.values.insert(1, 3);
    assert_ne!(checksum(&other), checksum(&state));
}

/// Scene with AI world state: marks on two objects, reports on slots 1 and 2 (one for a nonexistent victim, E-60),
/// difficulty 2.
fn ki_world_scene() -> (Rules, SimState) {
    use crate::id::ObjectId;
    use crate::state::ReportTile;
    let (rules, mut state) = scene();
    let of = |state: &SimState, slot: u8| -> Vec<ObjectId> {
        state
            .objects()
            .map(|o| o.id)
            .filter(|id| id.slot() == slot)
            .collect()
    };
    let (s1, s2) = (of(&state, 1), of(&state, 2));
    assert!(s1.len() >= 2 && s2.len() >= 2);
    state.object_mut(s1[0], false).unwrap().ai_marks = 0b0000_0010;
    state.object_mut(s2[1], false).unwrap().ai_marks = 0b1000_0100;
    for (i, &id) in s1.iter().enumerate() {
        let x = i as i32;
        state.attack_reports[1].insert(id.as_i32(), ReportTile { x, z: 7 });
    }
    for &id in &s2 {
        state.attack_reports[2].insert(id.as_i32(), ReportTile { x: 3, z: 4 });
    }
    let stale = ObjectId::new(500, 2, 3);
    assert!(state.object(stale, false).is_none());
    state.attack_reports[2].insert(stale.as_i32(), ReportTile { x: 15, z: 15 });
    state.campaign_difficulty = 2;
    (rules, state)
}

// QUIRK E-57: kept
// QUIRK E-41, E-59: deviation
#[test]
fn ki_world_state_survives_saving_and_loading() {
    let (rules, state) = ki_world_scene();
    let loaded = SaveGame::from_bytes(&testkit::save_bytes(&state, &rules)).expect("load");
    loaded.check(&rules).expect("rules check");
    assert_eq!(loaded.state, state);
    for p in 0..PLAYER_SLOTS {
        let keys = |s: &SimState| -> Vec<(i32, crate::state::ReportTile)> {
            s.attack_reports[p].iter().map(|(k, v)| (k, *v)).collect()
        };
        assert_eq!(keys(&loaded.state), keys(&state), "slot {p}");
    }
    assert_eq!(loaded.state.attack_reports[2].len(), 3);
    assert_eq!(checksum(&loaded.state), checksum(&state));
    // All three fields are part of the checksum.
    type Change = Box<dyn Fn(&mut SimState)>;
    let changes: Vec<Change> = vec![
        Box::new(|s| {
            let id = s.objects().next().unwrap().id;
            s.object_mut(id, false).unwrap().ai_marks ^= 1;
        }),
        Box::new(|s| {
            let k = s.attack_reports[1].first_key().unwrap();
            s.attack_reports[1].get_mut(k).unwrap().x += 1;
        }),
        Box::new(|s| s.campaign_difficulty = 0),
    ];
    for change in changes {
        let mut other = state.clone();
        change(&mut other);
        assert_ne!(checksum(&other), checksum(&state));
    }
}

// --- Script state and object names in the savegame ---

/// Level script of the load checks: block 0 `n12` + `n183` (tip 9 on slot 0), block 1 `w40;1000` + `n12`, block 2
/// `n39;intro` (dialog `intro` with `w150;7;0`).
const SCRIPT_LEVEL: &str = concat!(
    "{\r\n\t[BESCHREIBUNG=a]\r\n\t[AKTION=n12;schalter:Text]\r\n\t[AKTION=n183;9;0:Text]\r\n}\r\n",
    "{\r\n\t[BESCHREIBUNG=b]\r\n\t[AKTION=w40;1000:Text]\r\n\t[AKTION=n12;b:Text]\r\n}\r\n",
    "{\r\n\t[BESCHREIBUNG=c]\r\n\t[AKTION=n39;intro:Text]\r\n}\r\n",
);
const SCRIPT_DIALOG: &str = "{\r\n\t[BESCHREIBUNG=d]\r\n\t[AKTION=w150;7;0:Text]\r\n}\r\n";

fn script_setup(enabled: bool) -> MatchSetup {
    MatchSetup {
        script: crate::script::ScriptSetup {
            enabled,
            time_per_char: 55,
            ..Default::default()
        },
        ..testkit::setup()
    }
}

/// Match with script after one step: block 2 has started the dialog and waits (action 39), the dialog waits for the
/// end of line 7.
fn script_scene() -> (Rules, SimState) {
    let mut rules = Rules::synthetic(TYPES, 16);
    rules.scripts =
        crate::script::testing::rules_with(SCRIPT_LEVEL, &[("intro", SCRIPT_DIALOG)], &[]);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    let mut state = new_match(&rules, &script_setup(true)).unwrap();
    let _ = step(&mut state, &rules, &[]);
    state.events.clear();
    let level = state.script.level.as_ref().expect("level container");
    assert!(level.blocks[2].running, "action 39 waits");
    assert!(state.script.dialog.is_some(), "dialog running");
    assert!(state.script.mission.speech.line.is_some(), "line running");
    (rules, state)
}

/// Savegame bytes with the script on or off in the start package.
fn script_save(state: &SimState, rules: &Rules, enabled: bool) -> Vec<u8> {
    SaveGame {
        start: start_package(rules, &script_setup(enabled)),
        state: state.clone(),
        commands: Vec::new(),
    }
    .to_bytes()
}

/// Loading and rules check of a savegame with the script on.
fn script_load_and_check(state: &SimState, rules: &Rules) -> anyhow::Result<()> {
    SaveGame::from_bytes(&script_save(state, rules, true))?.check(rules)
}

fn level_of(s: &mut SimState) -> &mut crate::script::Container {
    s.script.level.as_mut().expect("level container")
}

#[test]
fn script_state_round_trips() {
    let (rules, mut state) = script_scene();
    {
        let level = level_of(&mut state);
        level.switches.insert("tor offen".into(), 1);
        level.random_switches.insert("zufall".into(), 3);
        level.timers.insert("uhr".into(), 750);
        level.variables.insert("zähler".into(), -4);
        level.counter = 12_000;
    }
    let speech = &mut state.script.mission.speech;
    speech.tip_ms = [300, 0];
    speech.tip_text = [11, 12];
    speech.events = vec![5, 7, 5];
    state.script.mission.info_history = vec![11, 12];
    crate::names::register(&mut state.names, "Bote", crate::id::ObjectId::from_i32(3));
    state.names.set("gruppe", "3;-4".into());

    let loaded = testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.script, state.script);
    assert_eq!(loaded.names, state.names);
    assert_eq!(loaded, state);
    script_load_and_check(&state, &rules).unwrap();
    // Continue as without saving: dialog end, level blocks, tip, random.
    let (mut a, mut b) = (state, loaded);
    for i in 0..40 {
        let _ = step(&mut a, &rules, &[]);
        let _ = step(&mut b, &rules, &[]);
        assert_eq!(checksum(&a), checksum(&b), "step {i}");
    }
}

// QUIRK E-170: deviation
#[test]
fn names_survive_save_load_in_bucket_order() {
    use crate::id::ObjectId;
    use crate::names::{ObjectNames, reverse};
    let (rules, mut state) = scene();
    // Four names in the same bucket (2) and one ID as the first in two values: the reverse search finds the front
    // entry of the chain. The original reverses the chain on loading (E-170); the reimplementation stores it
    // unchanged.
    for k in ["unit9", "unit10", "unit21", "unit32"] {
        assert_eq!(ObjectNames::bucket_of(k), 2, "{k}");
    }
    for (i, k) in ["unit9", "unit10", "unit21", "unit32"].iter().enumerate() {
        state.names.set(k, format!("{}", 40 + i));
    }
    state.names.set("unit9", "5".into());
    state.names.set("unit21", "5;6".into());
    let order = |s: &SimState| -> Vec<(String, String)> {
        s.names
            .iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    };
    assert_eq!(
        order(&state)
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>(),
        ["unit32", "unit21", "unit10", "unit9"]
    );
    assert_eq!(
        reverse(&state.names, ObjectId::from_i32(5)).as_deref(),
        Some("unit21")
    );
    let loaded = testkit::save_and_load(&state, &rules);
    assert_eq!(order(&loaded), order(&state));
    assert_eq!(
        reverse(&loaded.names, ObjectId::from_i32(5)).as_deref(),
        Some("unit21")
    );
    assert_eq!(checksum(&loaded), checksum(&state));
    assert_eq!(loaded, state);
}

/// Script match (`script_scene`) with one object each on slots 1 and 2 for the world fields; returns the two
/// IDs.
fn world_fields_scene() -> (Rules, SimState, [crate::id::ObjectId; 2]) {
    let (rules, mut state) = script_scene();
    let ids = {
        let mut sim = crate::sim::Sim::new(&mut state, &rules);
        [1u8, 2].map(|slot| {
            create_object(
                &mut sim,
                &mut ObjectDesc::new(1100, i32::from(slot), [64.0 * f32::from(slot), 0.0, 300.0]),
            )
            .expect("object")
        })
    };
    state.events.clear();
    assert_eq!((ids[0].slot(), ids[1].slot()), (1, 2));
    (rules, state, ids)
}

/// All new world fields and mission fields filled with reachable values.
fn fill_world_fields(state: &mut SimState, ids: [crate::id::ObjectId; 2]) {
    use crate::fog::AreaView;
    use crate::script::Quest;
    state.object_event_bits.insert(ids[0], 0x3FF);
    state.kill_counts[2].insert(1100, 3);
    state.kill_counts[0].insert(4201, 0);
    state.button_locks.insert(1100, 1);
    state.button_locks.insert(4201, 0);
    state.slot_running[1] = false;
    state.inactive_lists[1] = vec![ids[0]];
    state.fog.player = 1;
    state.fog.inactive_pos = 1;
    state.fog.area_views.push(AreaView {
        name: "tal".into(),
        rect: [-3, 4, 40, 50],
        player: 7,
        clear: true,
    });
    let mission = &mut state.script.mission;
    // The status values of the actions: 0 empty, 1 new, 3 set (27/182), 6 and 10 done (184...187).
    for (i, q) in [0, 1, 3, 6, 10].iter().enumerate() {
        mission.main_quests[i] = Quest {
            status: *q,
            text: 100 + i as i32,
        };
        mission.sub_quests[9 - i] = Quest {
            status: *q,
            text: -(i as i32),
        };
    }
    mission.countdown_container = 1;
    mission.countdown_s = -7;
    mission.task_status.insert(5, 1);
    mission.task_status.insert(-2, 0);
    state
        .script
        .area_overrides
        .insert("tal".into(), [9, 8, -7, 6]);
}

#[test]
fn new_world_fields_round_trip() {
    let (rules, mut state, ids) = world_fields_scene();
    fill_world_fields(&mut state, ids);
    check_state(&state).expect("reachable values");
    let loaded = testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.kill_counts, state.kill_counts);
    assert_eq!(loaded.button_locks, state.button_locks);
    assert_eq!(loaded.slot_running, state.slot_running);
    assert_eq!(loaded.inactive_lists, state.inactive_lists);
    assert_eq!(loaded.fog.area_views, state.fog.area_views);
    assert_eq!(loaded.fog.inactive_pos, 1);
    assert_eq!(loaded.script, state.script);
    assert_eq!(loaded, state);
    assert_eq!(checksum(&loaded), checksum(&state));
    script_load_and_check(&state, &rules).unwrap();
}

#[test]
fn world_fields_default_without_save() {
    // Defaults (`SimState::new`): all slots running, nothing counted, nothing locked.
    let state = SimState::new(-1, 0);
    assert_eq!(state.slot_running, [true; 11]);
    assert!(
        state
            .kill_counts
            .iter()
            .all(std::collections::BTreeMap::is_empty)
    );
    assert!(state.button_locks.is_empty());
    assert!(state.inactive_lists.iter().all(Vec::is_empty));
    assert!(state.fog.area_views.is_empty());
    assert_eq!(state.fog.inactive_pos, 0);
    // A loaded state without hidden slots keeps the defaults (the fields are in the savegame).
    let (rules, scene_state) = scene();
    let loaded = testkit::save_and_load(&scene_state, &rules);
    assert_eq!(loaded.slot_running, [true; 11]);
    assert!(
        loaded
            .kill_counts
            .iter()
            .all(std::collections::BTreeMap::is_empty)
    );
}

/// Script with a negative wait time (`w40;-5`) and negative text IDs (150, 183, 213) passes `catalog::check`; every
/// intermediate state loads, and saving/loading does not change the sequence.
#[test]
fn negative_wait_times_and_text_ids_survive_saving_and_loading() {
    const LEVEL: &str = concat!(
        "{\r\n\t[BESCHREIBUNG=a]\r\n\t[AKTION=w40;-5:Text]\r\n\t[AKTION=n12;x:Text]\r\n}\r\n",
        "{\r\n\t[BESCHREIBUNG=b]\r\n\t[AKTION=n150;-1;0:Text]\r\n\t[AKTION=n183;-1;0:Text]\r\n",
        "\t[AKTION=n213;-2;0;0;0:Text]\r\n}\r\n",
    );
    let mut rules = Rules::synthetic(TYPES, 16);
    rules.scripts = crate::script::testing::rules_with(LEVEL, &[], &[]);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    let mut state = new_match(&rules, &script_setup(true)).unwrap();
    let _ = step(&mut state, &rules, &[]);
    // After the first step: `w40` waits with -5, line -2 (213) has displaced -1 (150).
    let level = state.script.level.as_ref().unwrap();
    assert_eq!(level.blocks[0].index, 0);
    assert!(level.blocks[0].running);
    assert_eq!(level.blocks[0].timeout, -5);
    let speech = &state.script.mission.speech;
    assert_eq!(speech.line.map(|l| l.text), Some(-2));
    assert_eq!(speech.tip_text[0], -1);
    assert_eq!(speech.events, [-1]);
    assert_eq!(state.script.mission.info_history, [-1, -2]);
    for i in 0..20 {
        script_load_and_check(&state, &rules).unwrap_or_else(|e| panic!("step {i}: {e:#}"));
        let mut loaded = SaveGame::from_bytes(&script_save(&state, &rules, true))
            .unwrap()
            .state;
        assert_eq!(loaded, state, "step {i}");
        let _ = step(&mut state, &rules, &[]);
        let _ = step(&mut loaded, &rules, &[]);
        assert_eq!(checksum(&loaded), checksum(&state), "step {i}");
    }
    // `w40` is continued after the following step, the block deleted.
    assert!(state.script.level.as_ref().unwrap().blocks.is_empty());
}

// --- Random map: savegame ---

/// Synthetic rules like [`scene`] as a random map with the description `spec` (game mode = index in `GTYPE`) and the
/// generator checksum `digest` in the rules checksum.
fn random_scene(
    spec: crate::random_map::RandomMapSpec,
    digest: u64,
) -> (Rules, MatchSetup, SimState) {
    use crate::random_map::RandomMapRules;
    let mut rules = Rules::synthetic(TYPES, 16);
    rules.level_path = format!("randommap:{}", spec.seed_text);
    rules.checksum ^= digest;
    let setup = MatchSetup {
        mode: spec.game_type as i32,
        ..MatchSetup::default()
    };
    rules.random_map = Some(RandomMapRules {
        spec,
        slots: [false; 8],
        island: false,
        crt_start: 5,
        digest,
    });
    let state = new_match(&rules, &setup).unwrap();
    (rules, setup, state)
}

fn random_spec(seed: &str, game_type: usize) -> crate::random_map::RandomMapSpec {
    crate::random_map::RandomMapSpec {
        seed_text: seed.into(),
        size: 1,
        players: 4,
        terrain: 2,
        landscape: 1,
        daytime: 0,
        game_type,
    }
}

fn random_save(rules: &Rules, setup: &MatchSetup, state: &SimState) -> SaveGame {
    SaveGame {
        start: start_package(rules, setup),
        state: state.clone(),
        commands: Vec::new(),
    }
}

/// The seed text is kept as the map description in the start package and comes back character for character, also
/// umlauts, spaces and special characters; `level` is `randommap:<seed text>`.
#[test]
fn seed_text_round_trips_through_the_save() {
    for seed in ["ZKA", "Ä ö ü ß – 41", "  two  blank  ", "x:y/z\\.lvl", ""] {
        let spec = random_spec(seed, 1);
        let (rules, setup, state) = random_scene(spec.clone(), 0x1234);
        let save = random_save(&rules, &setup, &state);
        assert_eq!(save.start.random_map.as_ref(), Some(&spec));
        assert_eq!(save.start.level, format!("randommap:{seed}"));
        let back = SaveGame::from_bytes(&save.to_bytes()).unwrap();
        assert_eq!(back.start.random_map.as_ref().unwrap().seed_text, seed);
        assert_eq!(back, save);
        back.check(&rules).unwrap();
    }
}

/// Other map description (seed, size, map type ...) or other rules checksum with the same description (other game
/// data or mods change the generated map) -> message, no loading.
// QUIRK D-348: deviation
#[test]
fn changed_map_data_rejects_the_save() {
    let spec = random_spec("ZKA", 0);
    let (rules, setup, state) = random_scene(spec.clone(), 0x1234);
    let save = random_save(&rules, &setup, &state);
    save.check(&rules).unwrap();
    // Description changed in the savegame (same seed text, so the same map path).
    for edit in [
        &(|s: &mut crate::random_map::RandomMapSpec| s.size = 2)
            as &dyn Fn(&mut crate::random_map::RandomMapSpec),
        &|s| s.players = 2,
        &|s| s.terrain = 3,
        &|s| s.landscape = 0,
        &|s| s.daytime = 1,
    ] {
        let mut changed = save.clone();
        edit(changed.start.random_map.as_mut().unwrap());
        let back = SaveGame::from_bytes(&changed.to_bytes()).unwrap();
        let err = format!("{:#}", back.check(&rules).unwrap_err());
        assert!(err.contains("savegame: map description differs"), "{err}");
    }
    // A fixed map with the same path, but without a description: rejected likewise.
    let mut fixed = save.clone();
    fixed.start.random_map = None;
    let err = format!("{:#}", fixed.check(&rules).unwrap_err());
    assert!(err.contains("savegame: map description differs"), "{err}");
    // Same description, other generator checksum (other game data): checksum differs.
    let (other, _, _) = random_scene(spec, 0x5678);
    let err = format!("{:#}", save.check(&other).unwrap_err());
    assert!(
        err.contains("savegame: map differs (other game data or mods)"),
        "{err}"
    );
    // Other seed: other map path.
    let (zkb, _, _) = random_scene(random_spec("ZKB", 0), 0x1234);
    assert!(save.check(&zkb).is_err());
}

/// The game mode of the match must be the one of the description from which the map was generated (deviating from
/// D-348); a savegame with another game mode in the start package is rejected.
// QUIRK D-348: deviation
#[test]
fn game_type_mismatch_is_rejected() {
    let spec = random_spec("KOTH", 3);
    let (rules, setup, state) = random_scene(spec, 0x99);
    let save = random_save(&rules, &setup, &state);
    save.check(&rules).unwrap();
    for mode in [-1, 0, 1, 2, 4] {
        let mut changed = save.clone();
        changed.start.setup.mode = mode;
        let err = format!("{:#}", changed.check(&rules).unwrap_err());
        assert!(err.contains("game mode"), "mode {mode}: {err}");
    }
}

/// A fixed map carries no map description; saving and loading stay as before (start package the same, check passed,
/// continuing bit-identical).
#[test]
fn fixed_map_save_round_trips_with_random_map_none() {
    let (rules, mut state) = scene();
    assert!(rules.random_map.is_none());
    let setup = testkit::setup();
    let save = random_save(&rules, &setup, &state);
    assert_eq!(save.start.random_map, None);
    assert_eq!(save.start.level, rules.level_path);
    let back = SaveGame::from_bytes(&save.to_bytes()).unwrap();
    assert_eq!(back, save);
    back.check(&rules).unwrap();
    let mut loaded = back.state;
    for _ in 0..5 {
        let _ = step(&mut state, &rules, &[]);
        let _ = step(&mut loaded, &rules, &[]);
        assert_eq!(checksum(&loaded), checksum(&state));
    }
}
