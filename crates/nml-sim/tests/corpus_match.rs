//! Sets up a match on all multiplayer maps and `kamp1-1` and runs it.

use nml_rda::Vfs;
use nml_rda::testutil::corpus_game_dir;
use nml_sim::rules::Rules;
use nml_sim::save::{SaveGame, checksum};
use nml_sim::setup::{MatchSetup, new_match, start_package};
use nml_sim::state::Control;
use nml_sim::tick::step;
use nml_world::level::{level_paths, load_level};

// QUIRK W-221: kept
#[test]
fn matches_start_on_all_multiplayer_maps_and_run() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut levels: Vec<String> = level_paths(&vfs)
        .into_iter()
        .filter(|p| p.contains("/multiplayer/"))
        .collect();
    levels.push("data/levels/kampagne1/kamp1-1/kamp1-1.lvl".into());
    for lvl in &levels {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        let setup = MatchSetup::default_for(&rules);
        let mut state = new_match(&rules, &setup).unwrap_or_else(|e| panic!("{lvl}: {e}"));
        // Time-of-day upgrades of the map (`level_resources[40]`, re/upgrades 3.8): per slot 0-7
        // the six counters `4500 + t + 10k` = 1.
        let t = rules.level_daytime();
        assert_eq!(state.daytime, t, "{lvl}");
        for (slot, counts) in state.guid_counts.iter().enumerate().take(8) {
            for k in 0..6 {
                let guid = (4500 + t + 10 * k) as u32;
                assert_eq!(
                    counts.get(&guid),
                    Some(&1),
                    "{lvl}: slot {slot}, GUID {guid}"
                );
            }
        }
        let humans = state
            .players
            .iter()
            .filter(|p| p.control == Control::Human)
            .count();
        if setup.mode >= 0 {
            // Each player has their HQ and four units (game type 0).
            assert_eq!(humans, setup.lobby.len(), "{lvl}");
            for (slot, p) in state.players.iter().enumerate() {
                if p.control == Control::Human {
                    assert!(state.tables[slot].count >= 5, "{lvl}: slot {slot}");
                }
            }
        }
        // Trees (9) and nature (10) from the .wld, not ticked.
        assert!(state.tables[9].count > 0, "{lvl}");
        let ticked: usize = state.tick_groups.iter().map(Vec::len).sum();
        let expected: usize = state.tables[..9].iter().map(|t| t.count).sum();
        assert_eq!(ticked, expected, "{lvl}");
        for _ in 0..200 {
            step(&mut state, &rules, &[]);
        }
        assert_eq!(state.tick, 100, "{lvl}");
        let total: usize = state.tables.iter().map(|t| t.count).sum();
        eprintln!("{lvl}: {total} objects, {ticked} ticked");
    }
}

#[test]
fn saving_and_loading_mid_match_continues_bit_identically() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[6players]-legends.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let mut a = new_match(&rules, &setup).unwrap();
    let mut b = new_match(&rules, &setup).unwrap();
    assert_eq!(checksum(&a), checksum(&b));
    for _ in 0..100 {
        step(&mut a, &rules, &[]);
    }
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: a.clone(),
        commands: Vec::new(),
    };
    let mut loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap().state;
    for _ in 0..100 {
        step(&mut b, &rules, &[]);
    }
    let started = std::time::Instant::now();
    for _ in 0..100 {
        step(&mut a, &rules, &[]);
        step(&mut b, &rules, &[]);
        step(&mut loaded, &rules, &[]);
        let h = checksum(&a);
        assert_eq!(h, checksum(&b));
        assert_eq!(h, checksum(&loaded));
    }
    eprintln!(
        "legends: 100 x (3 ticks + 3 checksums) in {:.0} ms, savegame {} bytes",
        started.elapsed().as_secs_f64() * 1000.0,
        save.to_bytes().len()
    );
}

#[test]
fn units_walk_on_a_real_map_and_saves_continue_bit_identically() {
    use nml_sim::command::{Command, CommandKind};
    use nml_sim::motion::NO_DIRECTION;
    use nml_sim::object::status;
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[6players]-legends.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let mut a = new_match(&rules, &setup).unwrap();
    // All units with a walk module of one slot walk as a group of type 0 to the map center (targets
    // spread around the centroid).
    let center = (rules.map.dim_x << 11, rules.map.dim_z << 11);
    let mut commands = Vec::new();
    for slot in 0..8u8 {
        let units: Vec<_> = a.tables[usize::from(slot)]
            .objects
            .iter()
            .flatten()
            .filter(|o| o.has_module(0, false))
            .map(|o| o.id)
            .collect();
        if !units.is_empty() {
            commands.push(Command {
                player: slot,
                kind: CommandKind::Move {
                    units,
                    x: center.0,
                    z: center.1,
                    target: -1,
                    formation: 0,
                    direction: NO_DIRECTION,
                    spread: nml_sim::command::NO_SPREAD,
                    flags: 0,
                },
            });
        }
    }
    assert!(!commands.is_empty());
    // Player units (the neutral horses in slot 8 get no command).
    let start: Vec<_> = a
        .objects()
        .filter(|o| o.has_module(0, false) && o.owner() < 8)
        .map(|o| (o.id, o.grid))
        .collect();
    step(&mut a, &rules, &commands);
    let mut b = a.clone();
    for _ in 0..300 {
        step(&mut a, &rules, &[]);
    }
    // Saved in the middle of the run (tick 301): at least one unit is still on its way, otherwise
    // the test would only check idle walk states.
    let moving = a.objects().filter(|o| o.status & status::MOVE != 0).count();
    assert!(moving > 0, "at tick 301 no unit is walking anymore");
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: a.clone(),
        commands: Vec::new(),
    };
    let mut loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap().state;
    for _ in 0..300 {
        step(&mut b, &rules, &[]);
    }
    let started = std::time::Instant::now();
    for _ in 0..300 {
        step(&mut a, &rules, &[]);
        step(&mut b, &rules, &[]);
        step(&mut loaded, &rules, &[]);
        let h = checksum(&a);
        assert_eq!(h, checksum(&b));
        assert_eq!(h, checksum(&loaded));
    }
    let moved = start
        .iter()
        .filter(|(id, g)| a.object(*id, false).is_some_and(|o| o.grid != *g))
        .count();
    let walking = a.objects().filter(|o| o.status & status::MOVE != 0).count();
    eprintln!(
        "legends: {} units, {moved} walked, {walking} still on their way; 300 x 3 ticks in {:.0} ms",
        start.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(moved, start.len());
}

#[test]
fn units_play_their_idle_animation_again_after_stopping() {
    use nml_sim::command::{Command, CommandKind};
    use nml_sim::module::ModuleKind;
    use nml_sim::motion::NO_DIRECTION;
    use nml_sim::object::status;
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let mut state = new_match(&rules, &setup).unwrap();
    let unit = state
        .objects()
        .find(|o| o.owner() == 0 && o.has_module(0, false))
        .unwrap();
    let (id, idle) = (unit.id, unit.anim.current);
    let Some(ModuleKind::Walk(w)) = unit.module(0).map(|m| &m.kind) else {
        panic!("walk expected");
    };
    let walking = w.anims;
    // Two tiles toward +x (stand-in target, in case something stands there).
    let command = Command {
        player: 0,
        kind: CommandKind::Move {
            units: vec![id],
            x: unit.grid.x.0 + (2 << 12),
            z: unit.grid.z.0,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: nml_sim::command::NO_SPREAD,
            flags: 0,
        },
    };
    step(&mut state, &rules, &[command]);
    let mut seen_walking = false;
    for _ in 0..200 {
        step(&mut state, &rules, &[]);
        let o = state.object(id, false).unwrap();
        seen_walking |= o.status & status::MOVE != 0 && o.anim.current == walking[0];
        if seen_walking && o.status & status::MOVE == 0 {
            // Idle from the frame update of the stand module in the same tick.
            assert_eq!(o.anim.current, idle);
            return;
        }
    }
    panic!("unit did not start walking or did not arrive");
}

/// Starting account per resource setting on a multiplayer map: 0 = the `.lvl`, 1-3 =
/// `resourcenconfig.ini` (gold, wood, food each 500 / 1500 / 3000) for all players; living space
/// per setting, living space and population of the starting stock remain.
#[test]
fn start_resources_follow_the_resource_setting() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[6players]-legends.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let mut setup = MatchSetup::default_for(&rules);
    let from_map = new_match(&rules, &setup).unwrap();
    let lvl: Vec<(usize, i32)> = rules.level.players[0].resources.clone();
    let slot0 = usize::from(rules.level.players[0].slot);
    for &(index, value) in &lvl {
        assert_eq!(from_map.resource(slot0, index), value);
    }
    let population = from_map.resource(slot0, 3);
    assert!(population > 0);
    for (setting, amount) in [(1, 500), (2, 1500), (3, 3000)] {
        setup.resource_setting = setting;
        setup.living_space = setting + 1;
        let state = new_match(&rules, &setup).unwrap();
        for slot in 0..8 {
            let account: Vec<i32> = (0..3).map(|i| state.resource(slot, i)).collect();
            assert_eq!(account, [amount; 3], "setting {setting}, slot {slot}");
        }
        assert_eq!(state.resource(slot0, 3), population);
        assert_eq!(
            state.economy.max_living_space,
            [75, 100, 125, 150, 175, 200][setting as usize + 1]
        );
    }
}

/// Economy on a real map through commands only (`[4players]-exile`, slot 0 = SID with town hall
/// 2500, two settlers 2600 and two female settlers 2602; account 500 gold, 1000 wood, 1000 food):
/// one settler builds the house 2501 next to the town hall and then the farm 2502 on the fertile
/// soil, the other the gold mine 2504 at the gold deposit (places with
/// `find_building_placement_near`, snapped like the build cursor with `snapped_command_pose`, and
/// `check_build`); a female settler fells wood
/// at the nearest tree, after construction settlers mine gold and the second female settler
/// harvests - all through the right-click assignment (`targets.ini`) -, plus a settler trained
/// in the town hall. (The female settlers may build too: their build menu buttons 260/261 without
/// kind open the submenu the same way.) This runs once through `toolbar.ini`, `targets.ini`, the
/// model nodes `ausgang`/`farmer01...` and the docking places of the real buildings; saved and
/// loaded in the middle of the run, it continues bit-identically.
#[test]
fn colonists_build_gather_and_train_on_a_real_map_with_commands_only() {
    use nml_sim::command::validate::{check_build, is_finished};
    use nml_sim::command::{Command, CommandKind, F32Bits};
    use nml_sim::id::ObjectId;
    use nml_sim::object::GameObject;
    use nml_sim::placement::{
        find_building_placement_near, snapped_command_pose, worker_layer_region,
    };
    use nml_sim::sim::Sim;
    use nml_sim::state::SimState;
    use nml_sim::targeting::pick_target_action;
    const HQ: u32 = 2500;
    const HOUSE: u32 = 2501;
    const FARM: u32 = 2502;
    const GOLD_MINE: u32 = 2504;
    const SETTLER: u32 = 2600;
    const GOLD_DEPOSIT: u32 = 3500;
    const SOIL: u32 = 3503;
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let mut a = new_match(&rules, &setup).unwrap();
    let find = |s: &SimState, guid: u32| {
        s.objects()
            .filter(|o| o.guid == guid && o.owner() == 0)
            .map(|o| o.id)
            .collect::<Vec<_>>()
    };
    let hq = find(&a, HQ)[0];
    let workers: Vec<ObjectId> = a
        .objects()
        .filter(|o| o.owner() == 0 && o.has_module(1, false))
        .map(|o| o.id)
        .collect();
    assert_eq!(workers.len(), 4);
    // Female settlers have the build menu through buttons without kind (like the original's UI).
    {
        let sim = Sim::new(&mut a, &rules);
        for &w in &workers {
            check_build(&sim, 0, HOUSE, &[w]).unwrap();
        }
    }
    // Right-clicks only see what the fog reveals: the cycle steps run until the first fog cycle
    // has copied the sight of the starting objects (at most 20). This happens without `step`, so
    // the measured ticks below (commands in tick 0) stay unchanged.
    let mut warmup = 0;
    while a.fog.processed != -1 {
        assert!(warmup < 20, "the first fog cycle does not end");
        nml_sim::fog::fog_step(&mut Sim::new(&mut a, &rules));
        warmup += 1;
    }
    let hq_tile = a.object(hq, false).unwrap().tile;
    // The nearest matching object to tile `t` (tile distance, then ID).
    let nearest = |s: &SimState, pred: &dyn Fn(&GameObject) -> bool, t: (i32, i32)| {
        s.objects()
            .filter(|o| pred(o))
            .min_by_key(|o| ((o.tile.0 - t.0).pow(2) + (o.tile.1 - t.1).pow(2), o.id.0))
            .map(|o| (o.id, o.world_pos))
            .unwrap()
    };
    let (_, gold_at) = nearest(&a, &|o| o.guid == GOLD_DEPOSIT, hq_tile);
    let (_, soil_at) = nearest(&a, &|o| o.guid == SOIL, hq_tile);
    let hq_at = a.object(hq, false).unwrap().world_pos;
    // Building places like the build cursor: check of the build menu, then the nearest valid place.
    let place = |s: &mut SimState, guid: u32, worker: ObjectId, from: [f32; 3]| {
        let mut sim = Sim::new(s, &rules);
        check_build(&sim, 0, guid, &[worker]).unwrap();
        let (layer, region) = worker_layer_region(&mut sim, worker);
        let (mut x, mut z) = (from[0], from[2]);
        assert!(
            find_building_placement_near(&mut sim, &mut x, &mut z, guid, 0, layer, region),
            "no place for {guid}"
        );
        eprintln!("{guid}: place ({x}, {z}) from {from:?}");
        // The search returns only x/z; the check snaps gold mine and farm completely (with the
        // height of the deposit) - this position is carried by the command.
        let (pos, rot) = snapped_command_pose(&mut sim, guid, [x, 0.0, z], 0.0, 0, Some(worker))
            .expect("snapped");
        CommandKind::PlaceBuilding {
            guid,
            rotation: F32Bits::new(rot),
            x: F32Bits::new(pos[0]),
            y: F32Bits::new(pos[1]),
            z: F32Bits::new(pos[2]),
            workers: vec![worker],
            border: None,
        }
    };
    // East of the town hall, away from gold deposit (south) and soil (north): the docking places
    // of a construction site next to another would otherwise lie in the latter's footprint, which
    // is not locked until the first build step - the worker walking there then aborts.
    let next_to_hq = [hq_at[0] + 8.0 * 128.0, 0.0, hq_at[2] - 2.0 * 128.0];
    let mut first = vec![
        place(&mut a, HOUSE, workers[0], next_to_hq),
        place(&mut a, GOLD_MINE, workers[1], gold_at),
    ];
    // Right-click of the first female settler on the nearest tree: fell wood (400).
    let lumberjack = workers[2];
    let w_tile = a.object(lumberjack, false).unwrap().tile;
    let (tree, _) = nearest(&a, &|o| o.has_module(4, false), w_tile);
    let ground = a
        .object(lumberjack, false)
        .map(|o| (o.grid.x.0, o.grid.z.0))
        .unwrap();
    {
        let mut sim = Sim::new(&mut a, &rules);
        let (msg, cmd) =
            pick_target_action(&mut sim, 0, &[lumberjack], Some(tree), ground).unwrap();
        assert_eq!(msg, 400);
        assert_eq!(
            cmd,
            CommandKind::ChopWood {
                tree,
                units: vec![lumberjack]
            }
        );
        first.push(cmd);
        // Own town hall (finished, intact): quarter (414); ground: "Go to" (448).
        let on_hq = pick_target_action(&mut sim, 0, &[lumberjack], Some(hq), ground);
        assert!(
            matches!(on_hq, Some((414, CommandKind::Quarter { .. }))),
            "{on_hq:?}"
        );
        let on_ground = pick_target_action(&mut sim, 0, &[lumberjack], None, ground);
        assert!(
            matches!(on_ground, Some((448, CommandKind::Move { .. }))),
            "{on_ground:?}"
        );
    }
    first.push(CommandKind::EnqueueProduction {
        building: hq,
        guid: SETTLER,
    });
    let wood0 = a.resource(0, 1);
    let settlers0 = find(&a, SETTLER).len();
    let cmd = |kind| Command { player: 0, kind };
    step(
        &mut a,
        &rules,
        &first.into_iter().map(cmd).collect::<Vec<_>>(),
    );
    let wood_after_sites = a.resource(0, 1);
    assert_eq!(wood_after_sites, wood0 - 110 - 180);
    let sites = [HOUSE, GOLD_MINE].map(|g| find(&a, g));
    assert!(sites.iter().all(|s| s.len() == 1), "{sites:?}");
    let [house, mine] = sites.map(|s| s[0]);
    let mut farm = None;
    // Run: after construction, gold and harvest through right-click; ticks of the events.
    let mut b: Option<SimState> = None;
    let mut done: std::collections::BTreeMap<&str, u64> = Default::default();
    while a.steps < 1600 {
        let now = a.steps;
        let mut commands = Vec::new();
        if !done.contains_key("House") && is_finished(&Sim::new(&mut a, &rules), house) {
            // After that the same settler builds the farm.
            done.insert("House", now);
            commands.push(cmd(place(&mut a, FARM, workers[0], soil_at)));
        }
        {
            let mut sim = Sim::new(&mut a, &rules);
            for (what, building, worker, msg) in [
                ("Gold mine", Some(mine), workers[1], 402),
                ("Farm", farm, workers[3], 401),
            ] {
                let Some(building) = building else { continue };
                if !done.contains_key(what) && is_finished(&sim, building) {
                    done.insert(what, now);
                    let picked = pick_target_action(&mut sim, 0, &[worker], Some(building), ground);
                    let (m, c) = picked.unwrap();
                    assert_eq!(m, msg, "{what}");
                    commands.push(cmd(c));
                }
            }
        }
        if now == 700 {
            // Save and load in the middle of the run.
            let save = SaveGame {
                start: start_package(&rules, &setup),
                state: a.clone(),
                commands: Vec::new(),
            };
            b = Some(SaveGame::from_bytes(&save.to_bytes()).unwrap().state);
        }
        step(&mut a, &rules, &commands);
        if farm.is_none() {
            farm = find(&a, FARM).first().copied();
        }
        if let Some(b) = b.as_mut() {
            step(b, &rules, &commands);
            if a.steps.is_multiple_of(50) {
                assert_eq!(checksum(&a), checksum(b), "tick {}", a.steps);
            }
        }
        for (what, hit) in [
            (
                "Wood",
                a.resource(0, 1) > wood_after_sites - if farm.is_some() { 210 } else { 0 },
            ),
            ("Gold", a.resource(0, 0) > 500),
            ("Food", a.resource(0, 2) > 1000 - 60),
            ("Settler", find(&a, SETTLER).len() > settlers0),
        ] {
            if hit && !done.contains_key(what) {
                done.insert(what, a.steps);
            }
        }
    }
    if let Some(b) = b.as_ref() {
        assert_eq!(checksum(&a), checksum(b), "tick {}", a.steps);
    }
    let account: Vec<i32> = (0..5).map(|i| a.resource(0, i)).collect();
    eprintln!("exile: {done:?}, account {account:?}");
    // Measured ticks (engine ticks of 50 ms, object tick every second one; commands in tick 0):
    // - Settler 301: paid in the first building tick, 15 s = 150 object ticks.
    // - House 398: 14 s build time = 140 build steps of a worker (speed 100 -> 10 per step),
    //   so 280 ticks, plus the way to the docking place.
    // - Gold mine 511: 21 s = 210 steps = 420 ticks plus way.
    // - Farm 880: placed in tick 398 after the house, 21 s build time plus way.
    // - Wood 638, gold 909, food 1492: the first full load (load capacity) is delivered.
    let expected: std::collections::BTreeMap<&str, u64> = [
        ("Settler", 301),
        ("House", 398),
        ("Gold mine", 511),
        ("Wood", 638),
        ("Farm", 880),
        ("Gold", 909),
        ("Food", 1492),
    ]
    .into_iter()
    .collect();
    assert_eq!(done, expected);
    // Account after 80 s: two loads of 20 each (the new settler finds work on its own at the rally
    // point via `0x77F`) - gold 500 + 40, wood 1000 - 110 - 180 - 210 + 40, food 1000 - 60 + 40;
    // population 4 + 1, living space 25 + 15.
    assert_eq!(account, [540, 540, 980, 5, 40]);
}

/// Fishing on a real map (`[2players]-tintebariver`, slot 0 = SPA with two fishing baskets 3505):
/// a finished harbor 1008 at the basket nearest to the HQ (place via
/// `find_building_placement_near` with layer and region of a colonist like the build cursor; the
/// harbor itself needs a prerequisite in the build menu that the starting equipment lacks, and
/// therefore stands finished like a start object), the fishing boat 1117 from the harbor (`0x0E`,
/// exit at the model node), then the right-click of the boat on the basket (398 -> `Fish`,
/// `0x7D`). The first load (300 food) is delivered at the harbor; the ticks are fixed.
#[test]
fn a_fishing_boat_delivers_its_first_catch_at_the_harbor() {
    use nml_sim::command::validate::is_finished;
    use nml_sim::command::{Command, CommandKind};
    use nml_sim::module::gather::{fishing, gather};
    use nml_sim::placement::{find_building_placement_near, worker_layer_region};
    use nml_sim::sim::Sim;
    use nml_sim::state::SimEvent;
    use nml_sim::targeting::pick_target_action;
    use nml_sim::world::{ObjectDesc, create_object, desc_flags};
    const HARBOR: u32 = 1008;
    const BOAT: u32 = 1117;
    const BASKET: u32 = 3505;
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[2players]-tintebariver.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let mut a = new_match(&rules, &setup).unwrap();
    let hq_tile = a
        .objects()
        .find(|o| o.owner() == 0 && o.guid == 1000)
        .map(|o| o.tile)
        .expect("HQ");
    let worker = a
        .objects()
        .find(|o| o.owner() == 0 && o.guid == 1100)
        .map(|o| o.id)
        .expect("colonist");
    let (basket, basket_at) = a
        .objects()
        .filter(|o| o.guid == BASKET)
        .min_by_key(|o| {
            (
                (o.tile.0 - hq_tile.0).pow(2) + (o.tile.1 - hq_tile.1).pow(2),
                o.id.0,
            )
        })
        .map(|o| (o.id, o.world_pos))
        .expect("fishing basket");
    let harbor = {
        let mut sim = Sim::new(&mut a, &rules);
        let (layer, region) = worker_layer_region(&mut sim, worker);
        let (mut x, mut z) = (basket_at[0], basket_at[2]);
        assert!(find_building_placement_near(
            &mut sim, &mut x, &mut z, HARBOR, 0, layer, region
        ));
        assert_eq!((x, z), (2496.0, 6080.0));
        let mut desc = ObjectDesc::new(HARBOR, 0, [x, 0.0, z]);
        desc.flags |= desc_flags::READY | desc_flags::COLOR;
        create_object(&mut sim, &mut desc).expect("harbor")
    };
    assert!(is_finished(&Sim::new(&mut a, &rules), harbor));
    let cmd = |kind| Command { player: 0, kind };
    let train = CommandKind::EnqueueProduction {
        building: harbor,
        guid: BOAT,
    };
    step(&mut a, &rules, &[cmd(train)]);
    let food0 = a.resource(0, 2);
    let mut boat = None;
    let mut seen: std::collections::BTreeMap<&str, u64> = Default::default();
    let mut credited = None;
    while a.steps < 8000 && credited.is_none() {
        let mut commands = Vec::new();
        let spawned = a
            .objects()
            .find(|o| o.guid == BOAT && o.owner() == 0)
            .map(|o| o.id);
        if boat.is_none()
            && let Some(b) = spawned
        {
            boat = Some(b);
            seen.insert("Boat", a.steps);
            // Right-click of the boat on the basket: fishing (398).
            let mut sim = Sim::new(&mut a, &rules);
            let picked = pick_target_action(&mut sim, 0, &[b], Some(basket), (0, 0));
            let want = CommandKind::Fish {
                basket,
                units: vec![b],
            };
            assert_eq!(picked, Some((398, want.clone())));
            commands.push(cmd(want));
        }
        let events = step(&mut a, &rules, &commands);
        for e in events {
            if let SimEvent::Credited {
                object,
                amount,
                kind,
            } = e
                && Some(object) == boat
            {
                credited = Some((amount, kind));
                seen.insert("Food", a.steps);
                // Delivered at the harbor: the target of the delivery is the harbor.
                let sim = Sim::new(&mut a, &rules);
                let hq = gather(&sim, object, fishing::INDEX).map(|g| g.hq_hid);
                assert_eq!(hq, Some(harbor));
            }
        }
    }
    eprintln!("tintebariver: {seen:?}");
    // Measured (engine ticks of 50 ms, object tick every second one; same in debug and release):
    // the boat appears in tick 1001 (50 s training), the first load of 300 food is credited at the
    // harbor in tick 6390 (trip to the basket, catch, return trip, delivery).
    assert_eq!(credited, Some((300, 2)));
    assert_eq!(a.resource(0, 2), food0 + 300);
    let expected: std::collections::BTreeMap<&str, u64> =
        [("Boat", 1001), ("Food", 6390)].into_iter().collect();
    assert_eq!(seen, expected);
}

/// re/riding 13.1: Among the mounts (walk `SADDLE`/`SADDLEBOAT`) only the PRI canoe 1712 has a
/// `dummy01` - in the sub-object `pri/units/kanu/krieger.rdo` of its animations, rest position
/// (0; 11.81; -24.95).
// QUIRK K-232: deviation
#[test]
fn mount_nodes_contain_only_the_pri_canoe() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let keys: Vec<u32> = rules.mount_nodes.keys().copied().collect();
    assert_eq!(keys, [1712]);
    let p = rules.mount_nodes[&1712];
    for (got, want) in p.iter().zip([0.0, 11.81, -24.95]) {
        assert!((got - want).abs() < 1e-2, "{p:?}");
    }
    assert!(
        (p[1] - 11.806_807).abs() < 1e-3 && (p[2] + 24.950_59).abs() < 1e-3,
        "{p:?}"
    );
}

/// re/ships 4.4, 9.1; re/train-multipart 3.1: cannons of the ships and coupling nodes of the
/// train vehicles come from the models (`Rules::cannon_nodes`, `Rules::train_nodes`).
// QUIRK C-215: kept
#[test]
fn ship_cannons_and_train_couplings_come_from_the_models() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    // Number of cannons per (type, animation, side); only non-empty lists occur.
    let count = |guid: u32, anim: i32, starboard: bool| {
        rules
            .cannon_nodes
            .get(&(guid, anim, starboard))
            .map_or(0, Vec::len)
    };
    // 1109 SPA_Kriegsschiff: animation 2 (110002) starboard, 3 (110003) port, 6 each.
    assert_eq!((count(1109, 2, true), count(1109, 3, false)), (6, 6));
    // 2010 ENG_Kriegsschiff: 4 each.
    assert_eq!((count(2010, 2, true), count(2010, 3, false)), (4, 4));
    // 1108 SPA_Transportschiff (variant bit 13): animation 5 starboard, 6 port; hypothesis
    // re/ships 4.4: 2 per side (nodes in the sub-object `karavelle_spa_kanonen.rdo`).
    eprintln!(
        "1108: starboard {}, port {}",
        count(1108, 5, true),
        count(1108, 6, false)
    );
    assert_eq!((count(1108, 5, true), count(1108, 6, false)), (2, 2));
    // The rest carries no cannons: exactly these six keys.
    let keys: Vec<(u32, i32, bool)> = rules.cannon_nodes.keys().copied().collect();
    assert_eq!(
        keys,
        [
            (1108, 5, true),
            (1108, 6, false),
            (1109, 2, true),
            (1109, 3, false),
            (2010, 2, true),
            (2010, 3, false)
        ]
    );
    // Train: locomotive 2613 has only `vhinten01`, tender 2618 and wagon 2617 both nodes.
    let t = |guid: u32| rules.train_nodes[&guid];
    assert!(t(2613).hitch.is_some(), "{:?}", t(2613));
    assert!(t(2618).hitch.is_some() && t(2618).coupling.is_some());
    assert!(t(2617).hitch.is_some() && t(2617).coupling.is_some());
    eprintln!("train_nodes: {:?}", rules.train_nodes);
    // `mount_nodes` stays [1712] (mount_nodes_contain_only_the_pri_canoe).
}

/// Train init on `[2players]-crossrail`: each locomotive snaps onto its track and couples tender
/// and wagon at the real nodes `vhinten01`/`vvorne01`. Afterwards both stand on track tiles
/// **behind** the locomotive (against the travel vector `(cos, sin)` of the direction) - this
/// checks the rotation direction of the deviation from the original (`Matrix_RotationY`
/// `0x005755A0`).
#[test]
fn crossrail_trains_couple_behind_their_locomotives() {
    use nml_sim::module::ModuleKind;
    use nml_sim::module::train::{INDEX, LOCO, TENDER, TRACK, WAGON};
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let lvl = level_paths(&vfs)
        .into_iter()
        .find(|p| p.contains("crossrail"))
        .expect("crossrail in the shipped data");
    let scene = load_level(&vfs, &lvl).unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let state = new_match(&rules, &setup).unwrap();
    let train = |id| match &state.object(id, false).unwrap().module(INDEX).unwrap().kind {
        ModuleKind::Train(t) => t.clone(),
        _ => panic!("not a train"),
    };
    let track_tiles: std::collections::BTreeSet<(i32, i32)> = state
        .objects()
        .filter(|o| o.guid == TRACK)
        .map(|o| o.tile)
        .collect();
    let locos: Vec<_> = state.objects().filter(|o| o.guid == LOCO).collect();
    assert_eq!(locos.len(), 2, "one locomotive per start point");
    for lok in locos {
        let t = train(lok.id);
        assert!(!t.is_slave);
        assert!(
            track_tiles.contains(&lok.tile),
            "locomotive {:?} on track",
            lok.tile
        );
        let [tender, wagon] = t.slaves[..] else {
            panic!("slaves {:?}", t.slaves)
        };
        let (cx, sz) = t.dir;
        let mut last = 0i64;
        for (id, guid) in [(tender, TENDER), (wagon, WAGON)] {
            let o = state.object(id, false).unwrap();
            assert_eq!(o.guid, guid);
            assert_eq!(train(id).master, lok.id);
            assert!(train(id).is_slave);
            // Distance along the travel direction: negative = behind the locomotive, wagon behind tender.
            let ahead = i64::from(o.grid.x.0 - lok.grid.x.0) * i64::from(cx)
                + i64::from(o.grid.z.0 - lok.grid.z.0) * i64::from(sz);
            eprintln!(
                "locomotive {:?} direction {:?}: {guid} on {:?}, ahead {}",
                lok.tile,
                t.dir,
                o.tile,
                ahead >> 24
            );
            assert!(ahead < last, "{guid} behind the locomotive or the tender");
            assert!(
                track_tiles.contains(&o.tile),
                "{guid} on track {:?}",
                o.tile
            );
            last = ahead;
        }
        // Track blocking (re/train-multipart 4.1): the init locks 12 tiles `T + k*r` (k = 2 ... -9)
        // in all nine layers; tiles outside the map are dropped.
        let (rx, rz) = t.raster_dir;
        assert_eq!(t.last_block_tile, lok.tile);
        let path = &state.path;
        for k in -9..=2 {
            let (x, z) = (lok.tile.0 + k * rx, lok.tile.1 + k * rz);
            if !path.inside(x, z) {
                continue;
            }
            for layer in 0..9 {
                assert_eq!(
                    path.region_id(layer, x, z),
                    0xFFFF,
                    "locomotive {:?} layer {layer} tile ({x}, {z}) blocked",
                    lok.tile
                );
            }
        }
    }
}

/// Both locomotives on `[2players]-crossrail` depart (re/train-multipart 3.2, 4.1, 9): on the
/// track of the shipped data the locomotives stand almost at the end (slot 0: locomotive at (78,
/// 105) in -z with two tracks ahead and buffer stop behind; slot 1: locomotive at (23, 49) in +x
/// with two tracks ahead) and could hardly travel. The test therefore lays 32 finished track
/// segments 2518 in the travel direction (direction like the end segment) before departure and
/// then measures: start-up, stop at the track end with event bit 0 within a measured number of
/// ticks, same offset of tender and wagon in every step, blocking window of 13 tiles after the
/// first step into the next tile, and a bit-identical continuation after saving and loading in
/// the middle of the trip.
#[test]
fn crossrail_trains_run_to_the_track_end() {
    use nml_sim::command::{Command, CommandKind, TrainOp};
    use nml_sim::id::ObjectId;
    use nml_sim::module::ModuleKind;
    use nml_sim::module::train::{INDEX, LOCO, TRACK};
    use nml_sim::path::BLOCKED;
    use nml_sim::sim::Sim;
    use nml_sim::state::SimState;
    use nml_sim::world::{ObjectDesc, create_object, desc_flags, object_event_bits};

    /// Additional track segments per locomotive.
    const EXTRA_TRACKS: i32 = 32;
    /// Steps until both locomotives stop (measured; `step` calls of 50 ms, same in debug and
    /// release) and step of the save check in the middle of the trip.
    const MAX_STEPS: usize = 600;
    const SAVE_AT: usize = 60;

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let lvl = level_paths(&vfs)
        .into_iter()
        .find(|p| p.contains("crossrail"))
        .expect("crossrail in the shipped data");
    let scene = load_level(&vfs, &lvl).unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let mut state = new_match(&rules, &setup).unwrap();

    let train = |state: &SimState, id: ObjectId| match &state
        .object(id, false)
        .unwrap()
        .module(INDEX)
        .unwrap()
        .kind
    {
        ModuleKind::Train(t) => t.clone(),
        _ => panic!("not a train"),
    };

    // Extend the track: further finished segments from the end segment in travel direction `raster_dir`.
    let locos: Vec<_> = state
        .objects()
        .filter(|o| o.guid == LOCO)
        .map(|o| (o.id, o.tile, train(&state, o.id).raster_dir))
        .collect();
    assert_eq!(locos.len(), 2, "one locomotive per start point");
    for &(id, tile, (rx, rz)) in &locos {
        let tracks: std::collections::BTreeMap<(i32, i32), (i32, i32)> = state
            .objects()
            .filter(|o| o.guid == TRACK && o.id.slot() == id.slot())
            .map(|o| (o.tile, (o.direction.0 >> 12, 0)))
            .collect();
        let mut end = tile;
        while tracks.contains_key(&(end.0 + rx, end.1 + rz)) {
            end = (end.0 + rx, end.1 + rz);
        }
        let ahead = ((end.0 - tile.0).abs() + (end.1 - tile.1).abs()) as usize;
        eprintln!("locomotive {tile:?} direction ({rx}, {rz}): {ahead} tracks ahead, end {end:?}");
        assert!(ahead <= 2, "hardly any track ahead (shipped data): {ahead}");
        let dir_deg = tracks[&end].0;
        let mut sim = Sim::new(&mut state, &rules);
        for k in 1..=EXTRA_TRACKS {
            let (x, z) = (end.0 + k * rx, end.1 + k * rz);
            assert!(sim.state.path.inside(x, z), "track ({x}, {z}) in the map");
            let mut desc = ObjectDesc::new(
                TRACK,
                i32::from(id.slot()),
                [x as f32 * 128.0 + 64.0, 0.0, z as f32 * 128.0 + 64.0],
            );
            desc.flags |= desc_flags::PLACED | desc_flags::ROT_Y;
            desc.rotation[1] =
                ((dir_deg + 90) << 12) as f32 * (1.0 / 4096.0) * f32::from_bits(0x3C8E_FA35);
            create_object(&mut sim, &mut desc).expect("track created");
        }
    }

    // Drive command of both locomotives: depart forward (slot = owner).
    let depart = |state: &mut SimState| {
        let cmds: Vec<Command> = locos
            .iter()
            .map(|&(id, ..)| Command {
                player: id.slot(),
                kind: CommandKind::Train {
                    train: id,
                    op: TrainOp::Forward,
                    target: None,
                },
            })
            .collect();
        step(state, &rules, &cmds);
    };
    let consist_offsets = |state: &SimState, loco: ObjectId| -> Vec<(i32, i32)> {
        let at = state.object(loco, false).unwrap().grid;
        train(state, loco)
            .slaves
            .iter()
            .map(|&s| {
                let g = state.object(s, false).unwrap().grid;
                (g.x.0 - at.x.0, g.z.0 - at.z.0)
            })
            .collect()
    };

    // Run without saving: checksums, stop steps, blocking window and offset.
    let run = |save: bool| {
        let mut st = state.clone();
        let offsets: Vec<_> = locos
            .iter()
            .map(|&(id, ..)| consist_offsets(&st, id))
            .collect();
        for o in &offsets {
            assert_eq!(o.len(), 2, "tender and wagon coupled");
        }
        let mut sums = Vec::new();
        let mut halted: Vec<Option<usize>> = vec![None; locos.len()];
        let mut first_tile: Vec<Option<(i32, i32)>> = vec![None; locos.len()];
        let mut moved: Vec<bool> = vec![false; locos.len()];
        for i in 0..MAX_STEPS {
            if i == 0 {
                depart(&mut st);
            } else {
                step(&mut st, &rules, &[]);
            }
            if save && i == SAVE_AT {
                // In the middle of the trip: both locomotives are still running.
                for &(id, ..) in &locos {
                    assert_ne!(train(&st, id).status, 0, "locomotive {id:?} still running");
                }
                let save = SaveGame {
                    start: start_package(&rules, &setup),
                    state: st.clone(),
                    commands: Vec::new(),
                };
                st = SaveGame::from_bytes(&save.to_bytes()).unwrap().state;
            }
            sums.push(checksum(&st));
            for (n, &(id, tile, (rx, rz))) in locos.iter().enumerate() {
                assert_eq!(consist_offsets(&st, id), offsets[n], "offset, step {i}");
                let now = st.object(id, false).unwrap().tile;
                if !moved[n] && now != tile {
                    // Blocking window after the first step into the next tile: 13 tiles
                    // `T - 10 ... T + 2` along the travel direction in all nine layers.
                    moved[n] = true;
                    first_tile[n] = Some(now);
                    assert_eq!(train(&st, id).last_block_tile, now);
                    for k in -10..=2 {
                        let (x, z) = (now.0 + k * rx, now.1 + k * rz);
                        for layer in 0..9 {
                            assert_eq!(
                                st.path.region_id(layer, x, z),
                                BLOCKED,
                                "locomotive {tile:?} layer {layer} tile ({x}, {z}) blocked"
                            );
                        }
                    }
                }
                if halted[n].is_none() && moved[n] && train(&st, id).status == 0 {
                    halted[n] = Some(i);
                    // Stop via the braking curve: event bit 0, A = -100, no travel.
                    let t = train(&st, id);
                    assert_eq!((t.speed, t.next_speed, t.acc_time), (0, 0, -100));
                    assert_eq!(object_event_bits(&Sim::new(&mut st, &rules), id) & 1, 1);
                }
            }
            if halted.iter().all(Option::is_some) && i > SAVE_AT {
                break;
            }
        }
        let end_tiles: Vec<_> = locos
            .iter()
            .map(|&(id, ..)| st.object(id, false).unwrap().tile)
            .collect();
        (sums, halted, first_tile, end_tiles)
    };
    let (sums, halted, first_tile, end_tiles) = run(false);
    eprintln!(
        "crossrail: stop in step {halted:?}, first tiles {first_tile:?}, stop at {end_tiles:?}"
    );
    // Measured (debug = release): stop two tiles before the last segment (K-245, as in the test
    // land: track end 49 -> stop at 47), slot 1 one step later than slot 0.
    assert_eq!(halted, [Some(154), Some(155)], "stop steps");
    assert_eq!(end_tiles, [(78, 73), (55, 49)], "stop tiles");
    // Continuation after saving and loading in the middle of the trip: bit-identical.
    let (saved_sums, saved_halted, ..) = run(true);
    assert_eq!(saved_halted, halted);
    assert_eq!(saved_sums, sums, "checksum sequence after loading");
}

/// Data of the governor (1112) and the missionary (1113) from the shipped data (re/upgrades 3.7,
/// re/temp-upgrades): module 30 with 2840, module 44 with the execution list `4452 4450 4451`,
/// module 38 with the spell lists `7 7 7` and `5 6 11`; a savegame keeps them and continues
/// bit-identically. Missionaries stand on `kamp1-3` (one) and `kamp3-2`; no map of the shipped
/// data has a governor, it is created directly.
#[test]
fn governors_and_missionaries_keep_their_data() {
    use nml_sim::id::ObjectId;
    use nml_sim::module::{ModuleKind, exec_temp, global_upgrade, spell};
    use nml_sim::sim::Sim;
    use nml_sim::state::SimState;
    use nml_sim::world::{ObjectDesc, create_object, desc_flags};

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/kampagne1/kamp1-3/kamp1-3.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let mut state = new_match(&rules, &setup).unwrap();
    let missionaries: Vec<ObjectId> = state
        .objects()
        .filter(|o| o.guid == 1113)
        .map(|o| o.id)
        .collect();
    assert_eq!(missionaries.len(), 1, "one missionary on kamp1-3");
    let m = missionaries[0];
    let slot = usize::from(m.slot());
    // The governor next to the missionary, at the same slot.
    let governor = {
        let pos = state.object(m, false).unwrap().world_pos;
        let mut sim = Sim::new(&mut state, &rules);
        let mut desc = ObjectDesc::new(1112, slot as i32, [pos[0] + 256.0, pos[1], pos[2]]);
        desc.flags |= desc_flags::PLACED;
        create_object(&mut sim, &mut desc).expect("governor created")
    };
    // The init of module 30 enqueues 2840; credited in the first phase-0 step.
    let before = state
        .guid_counts
        .get(slot)
        .and_then(|c| c.get(&2840))
        .copied();
    for _ in 0..2 {
        step(&mut state, &rules, &[]);
    }
    let after = state
        .guid_counts
        .get(slot)
        .and_then(|c| c.get(&2840))
        .copied();
    // Measured: the missionary belongs to slot 6 (players, counters only 0-7).
    assert_eq!(slot, 6);
    assert_eq!((before, after), (None, Some(1)), "2840 credited once");
    let check = |state: &SimState, what: &str| {
        let o = state.object(m, false).expect("missionary");
        match &o.module(global_upgrade::INDEX).expect("module 30").kind {
            ModuleKind::GlobalUpgrade(g) => assert_eq!(g.upgrade_guid, 2840, "{what}"),
            k => panic!("{what}: module 30 is {k:?}"),
        }
        let g = state.object(governor, false).expect("governor");
        match &g.module(exec_temp::INDEX).expect("module 44").kind {
            ModuleKind::ExecTempUpgrade(e) => {
                assert_eq!(e.exec_list, [4452, 4450, 4451], "{what}");
            }
            k => panic!("{what}: module 44 is {k:?}"),
        }
        match &g.module(spell::INDEX).expect("module 38").kind {
            ModuleKind::Spell(s) => {
                assert_eq!(
                    (&s.anims[..], &s.effects[..]),
                    (&[7, 7, 7][..], &[5, 6, 11][..])
                );
            }
            k => panic!("{what}: module 38 is {k:?}"),
        }
    };
    check(&state, "after the start");
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: state.clone(),
        commands: Vec::new(),
    };
    let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
    loaded.check(&rules).expect("savegame valid");
    let mut loaded = loaded.state;
    check(&loaded, "after loading");
    for _ in 0..100 {
        step(&mut state, &rules, &[]);
        step(&mut loaded, &rules, &[]);
        assert_eq!(checksum(&state), checksum(&loaded));
    }
}

/// Saving and loading on a multiplayer map with time of day 2 (`[2players]-ironhorsehunt`,
/// re/upgrades 3.8): counters and lists of the time-of-day upgrades remain, the checksum sequence
/// is the same.
#[test]
fn daytime_upgrades_survive_saving_on_a_multiplayer_map() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[2players]-ironhorsehunt.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    assert_eq!(rules.level_daytime(), 2);
    let setup = MatchSetup::default_for(&rules);
    let mut a = new_match(&rules, &setup).unwrap();
    let mut b = new_match(&rules, &setup).unwrap();
    let daytime: Vec<u32> = (0..6).map(|k| 4502 + 10 * k).collect();
    for slot in 0..=8 {
        assert_eq!(a.applied_upgrades[slot], daytime, "slot {slot}");
    }
    for _ in 0..100 {
        step(&mut a, &rules, &[]);
        step(&mut b, &rules, &[]);
    }
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: a.clone(),
        commands: Vec::new(),
    };
    let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
    loaded.check(&rules).expect("savegame valid");
    let mut loaded = loaded.state;
    assert_eq!(
        (
            loaded.daytime,
            &loaded.applied_upgrades,
            &loaded.guid_counts
        ),
        (a.daytime, &a.applied_upgrades, &a.guid_counts)
    );
    for _ in 0..200 {
        step(&mut a, &rules, &[]);
        step(&mut b, &rules, &[]);
        step(&mut loaded, &rules, &[]);
        let h = checksum(&a);
        assert_eq!(h, checksum(&b));
        assert_eq!(h, checksum(&loaded));
    }
}

/// Chief's tent of the Prairie Indians on `[5players]-viciouscircle` (re/tents 9.4): the start
/// object 1600 of the PRI slot (nation 1 from the `.lvl`, measured slot 2) carries the tent state
/// of the init (`Tent { travois_guid: 1717, status: 0 }`) and stands in the collision manager; a
/// savegame keeps it.
#[test]
fn viciouscircle_chieftain_tents_carry_tent_state() {
    use nml_sim::module::ModuleKind;
    use nml_sim::module::tent::{self, Tent};

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/multiplayer/[5players]-viciouscircle.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let setup = MatchSetup::default_for(&rules);
    let state = new_match(&rules, &setup).unwrap();
    let pri: Vec<usize> = (0..8).filter(|&i| state.players[i].nation == 1).collect();
    assert_eq!(pri, [2], "one PRI slot");
    let hqs: Vec<_> = state.tables[pri[0]]
        .objects
        .iter()
        .flatten()
        .filter(|o| o.guid == 1600)
        .map(|o| o.id)
        .collect();
    assert_eq!(hqs.len(), 1, "one chief's tent");
    let hq = hqs[0];
    let tent_of = |state: &nml_sim::state::SimState| {
        let o = state.object(hq, false).expect("chief's tent");
        match &o.module(tent::INDEX).expect("module 42").kind {
            ModuleKind::Tent(t) => t.clone(),
            k => panic!("module 42 is {k:?}"),
        }
    };
    let expected = Tent {
        travois_guid: 1717,
        status: tent::status::REST,
        ..Tent::default()
    };
    assert_eq!(tent_of(&state), expected);
    assert!(
        state.collision.all.iter().any(|b| b.contains(&hq)),
        "in the collision manager"
    );
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: state.clone(),
        commands: Vec::new(),
    };
    let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
    loaded.check(&rules).expect("savegame valid");
    assert_eq!(tent_of(&loaded.state), expected);
}

/// The two travois 1717 on `kamp2-6` (re/tents 9.4) carry no tent: `m_TentHID = 0`, flags 0.
/// `0x223` answers 0 and has no effect, and the command `0x2E` is rejected (the filter blocks
/// button 453).
#[test]
fn kamp2_6_travois_have_no_tent() {
    use nml_sim::command::tent::check_unpack_tent;
    use nml_sim::id::ObjectId;
    use nml_sim::message::Message;
    use nml_sim::messaging::send;
    use nml_sim::module::ModuleKind;
    use nml_sim::module::tent::transport;
    use nml_sim::sim::Sim;

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, "data/levels/kampagne2/kamp2-6/kamp2-6.lvl").unwrap();
    let rules = Rules::load(&vfs, &scene).unwrap();
    let mut state = new_match(&rules, &MatchSetup::default_for(&rules)).unwrap();
    let travois: Vec<ObjectId> = state
        .objects()
        .filter(|o| o.guid == 1717)
        .map(|o| o.id)
        .collect();
    assert_eq!(travois.len(), 2, "two travois on kamp2-6");
    for &t in &travois {
        let m = state
            .object(t, false)
            .unwrap()
            .module(transport::INDEX)
            .expect("module 35");
        match &m.kind {
            ModuleKind::TransportTent(tt) => assert_eq!(tt.tent, ObjectId::NONE),
            k => panic!("module 35 is {k:?}"),
        }
        assert_eq!(m.base.flags, 0, "Flags");
        let mut sim = Sim::new(&mut state, &rules);
        let (x, z) = {
            let g = sim.object(t).unwrap().grid;
            (g.x.0 + (2 << 12), g.z.0)
        };
        assert!(check_unpack_tent(&sim, t.slot(), t, x, z).is_err());
        let before = checksum(sim.state);
        let answer = send(&mut sim, t, &Message::new(transport::msg::ARRIVED, &[x, z]));
        assert_eq!(answer, 0);
        assert_eq!(checksum(sim.state), before, "has no effect");
    }
}

/// Trade carts of the shipped data (re/trading 1.1): on the maps with carts all have module 43 with
/// the data values 0 (tax interval 0, maximum tax 0) and tax level 50; a savegame keeps that.
/// Totals as in re/trading 1.1: 1110 x2, 1409 x1, 1713 x6, 2011 x12, 2311 none (measured: the 12
/// ox carts include 2 from `kamp3-4/oxdebug.lvl`).
#[test]
fn campaign_trade_carts_keep_their_data() {
    use std::collections::BTreeMap;

    use nml_sim::module::{ModuleKind, trading};

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let mut totals: BTreeMap<u32, usize> = BTreeMap::new();
    for lvl in [
        "data/levels/kampagne1/kamp1-3/kamp1-3.lvl",
        "data/levels/kampagne1/kamp1-4/kamp1-4.lvl",
        "data/levels/kampagne2/kamp2-10/kamp2-10.lvl",
        "data/levels/kampagne3/kamp3-4/kamp3-4.lvl",
        "data/levels/kampagne3/kamp3-4/oxdebug.lvl",
        "data/levels/kampagne3/kamp3-7/kamp3-7.lvl",
        "data/levels/tutorial0-1/kamp0-1.lvl",
    ] {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        let setup = MatchSetup::default_for(&rules);
        let state = new_match(&rules, &setup).unwrap_or_else(|e| panic!("{lvl}: {e}"));
        let carts: Vec<_> = state
            .objects()
            .filter(|o| matches!(o.guid, 1110 | 1409 | 1713 | 2011 | 2311))
            .map(|o| (o.id, o.guid))
            .collect();
        assert!(!carts.is_empty(), "{lvl}: carts");
        let trading_of = |state: &nml_sim::state::SimState, id| {
            let o = state.object(id, false).expect("cart");
            match &o.module(trading::INDEX).expect("module 43").kind {
                ModuleKind::Trading(t) => t.clone(),
                k => panic!("{lvl}: module 43 is {k:?}"),
            }
        };
        let save = SaveGame {
            start: start_package(&rules, &setup),
            state: state.clone(),
            commands: Vec::new(),
        };
        let loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
        loaded.check(&rules).expect("savegame valid");
        for &(id, guid) in &carts {
            *totals.entry(guid).or_default() += 1;
            let t = trading_of(&state, id);
            assert_eq!(
                (t.tax_level, t.tax_interval, t.max_tax),
                (50, 0, 0),
                "{lvl}: {guid}"
            );
            assert_eq!(trading_of(&loaded.state, id), t, "{lvl}: {guid} loaded");
        }
    }
    assert_eq!(
        totals,
        BTreeMap::from([(1110, 2), (1409, 1), (1713, 6), (2011, 12)])
    );
}

/// Walls of the five nations (fort cursor `FUN_00453530`: 1011, 1311, 1912, 2212, 2512); corner =
/// wall + 1, gate = wall + 2 (`corpus_rules.rs`, `wall_corner_pairs_from_the_flags_match_the_fort_cursor`).
const WALLS: [u32; 5] = [1011, 1311, 1912, 2212, 2512];

/// Wall piece or corner.
fn is_wall_piece(guid: u32) -> bool {
    WALLS.iter().any(|&w| guid == w || guid == w + 1)
}

/// Gate (1013, 1313, 1914, 2214, 2514).
fn is_gate(guid: u32) -> bool {
    WALLS.iter().any(|&w| guid == w + 2)
}

/// The campaign levels (all `.lvl` without `/multiplayer/`) whose level data carry an object with
/// `keep`, sorted. Only the level data are read (without rules).
fn campaign_levels_with(vfs: &Vfs, keep: fn(u32) -> bool) -> Vec<String> {
    let mut levels: Vec<String> = level_paths(vfs)
        .into_iter()
        .filter(|p| !p.contains("/multiplayer/"))
        .filter(|p| {
            load_level(vfs, p).is_ok_and(|scene| scene.objects.iter().any(|o| keep(o.guid)))
        })
        .collect();
    levels.sort();
    levels
}

/// Campaign levels with gates (measured; 23 campaign levels).
const GATE_LEVELS: [&str; 23] = [
    "data/levels/kampagne1/kamp1-2/kamp1-2.lvl",
    "data/levels/kampagne1/kamp1-3/kamp1-3.lvl",
    "data/levels/kampagne1/kamp1-4/kamp1-4.lvl",
    "data/levels/kampagne1/kamp1-5/kamp1-5.lvl",
    "data/levels/kampagne1/kamp1-6/kamp1-6.lvl",
    "data/levels/kampagne2/kamp2-1/kamp2-1.lvl",
    "data/levels/kampagne2/kamp2-10/kamp2-10.lvl",
    "data/levels/kampagne2/kamp2-2/kamp2-2.lvl",
    "data/levels/kampagne2/kamp2-4/kamp2-4.lvl",
    "data/levels/kampagne2/kamp2-5/kamp2-5.lvl",
    "data/levels/kampagne3/kamp3-1/kamp3-1.lvl",
    "data/levels/kampagne3/kamp3-10/kamp3-10.lvl",
    "data/levels/kampagne3/kamp3-12/kamp3-12.lvl",
    "data/levels/kampagne3/kamp3-2/kamp3-2.lvl",
    "data/levels/kampagne3/kamp3-3/kamp3-3.lvl",
    "data/levels/kampagne3/kamp3-4/kamp3-4.lvl",
    "data/levels/kampagne3/kamp3-5/kamp3-5.lvl",
    "data/levels/kampagne3/kamp3-6/kamp3-6.lvl",
    "data/levels/kampagne3/kamp3-7/kamp3-7.lvl",
    "data/levels/kampagne3/kamp3-8/kamp3-8.lvl",
    "data/levels/kampagne3/kamp3-9/kamp3-9.lvl",
    "data/levels/tutorial0-1/kamp0-1.lvl",
    "data/levels/tutorial0-2/kamp0-2.lvl",
];

/// Campaign levels with wall pieces, corners or gates (measured): the gate levels and `kamp2-9`,
/// `kamp3-14` (walls only).
const WALL_LEVELS: [&str; 25] = [
    "data/levels/kampagne1/kamp1-2/kamp1-2.lvl",
    "data/levels/kampagne1/kamp1-3/kamp1-3.lvl",
    "data/levels/kampagne1/kamp1-4/kamp1-4.lvl",
    "data/levels/kampagne1/kamp1-5/kamp1-5.lvl",
    "data/levels/kampagne1/kamp1-6/kamp1-6.lvl",
    "data/levels/kampagne2/kamp2-1/kamp2-1.lvl",
    "data/levels/kampagne2/kamp2-10/kamp2-10.lvl",
    "data/levels/kampagne2/kamp2-2/kamp2-2.lvl",
    "data/levels/kampagne2/kamp2-4/kamp2-4.lvl",
    "data/levels/kampagne2/kamp2-5/kamp2-5.lvl",
    "data/levels/kampagne2/kamp2-9/kamp2-9.lvl",
    "data/levels/kampagne3/kamp3-1/kamp3-1.lvl",
    "data/levels/kampagne3/kamp3-10/kamp3-10.lvl",
    "data/levels/kampagne3/kamp3-12/kamp3-12.lvl",
    "data/levels/kampagne3/kamp3-14/kamp3-14.lvl",
    "data/levels/kampagne3/kamp3-2/kamp3-2.lvl",
    "data/levels/kampagne3/kamp3-3/kamp3-3.lvl",
    "data/levels/kampagne3/kamp3-4/kamp3-4.lvl",
    "data/levels/kampagne3/kamp3-5/kamp3-5.lvl",
    "data/levels/kampagne3/kamp3-6/kamp3-6.lvl",
    "data/levels/kampagne3/kamp3-7/kamp3-7.lvl",
    "data/levels/kampagne3/kamp3-8/kamp3-8.lvl",
    "data/levels/kampagne3/kamp3-9/kamp3-9.lvl",
    "data/levels/tutorial0-1/kamp0-1.lvl",
    "data/levels/tutorial0-2/kamp0-2.lvl",
];

/// Building module of a loaded object (flag word and state).
fn building_module(
    state: &nml_sim::state::SimState,
    id: nml_sim::id::ObjectId,
) -> (
    &nml_sim::module::Module,
    &nml_sim::module::building::Building,
) {
    use nml_sim::module::{ModuleKind, building};
    let m = state
        .object(id, false)
        .and_then(|o| o.module(building::INDEX))
        .expect("building module");
    match &m.kind {
        ModuleKind::Building(b) => (m, b),
        k => panic!("module 3 is {k:?}"),
    }
}

/// Gates of the campaign (re/arming-gates 9.C): on every level with gates each gate stands
/// finished and open (`TOROPEN`, no `TORCLOSE`), none of its two door tiles is blocked in the
/// land layers 0-3 of the path grid, and the gate init on loading finds nothing: no gate hangs on
/// a master, and the number of wall pieces and corners equals the level data. The gates stand
/// everywhere in wall gaps like on kamp1-2; measured, no level deviates, so there is no exception
/// list. 103 gates in 23 levels.
#[test]
fn campaign_gates_start_open_and_keep_their_walls() {
    use nml_sim::id::ObjectId;
    use nml_sim::module::building::{flags, gate::door_tiles_of};
    use nml_sim::path::grid::COST_FREE;

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    assert_eq!(campaign_levels_with(&vfs, is_gate), GATE_LEVELS);
    let mut total_gates = 0;
    for lvl in GATE_LEVELS {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        let state = new_match(&rules, &MatchSetup::default_for(&rules))
            .unwrap_or_else(|e| panic!("{lvl}: {e}"));
        let pieces_in_level = scene
            .objects
            .iter()
            .filter(|o| is_wall_piece(o.guid))
            .count();
        let pieces = state
            .objects()
            .filter(|o| is_wall_piece(o.guid) && !state.destroy_queue.contains(&o.id))
            .count();
        assert_eq!(pieces, pieces_in_level, "{lvl}: wall pieces and corners");
        assert!(state.destroy_queue.is_empty(), "{lvl}: nothing queued");
        let gates: Vec<ObjectId> = state
            .objects()
            .filter(|o| is_gate(o.guid))
            .map(|o| o.id)
            .collect();
        let gates_in_level = scene.objects.iter().filter(|o| is_gate(o.guid)).count();
        assert_eq!(gates.len(), gates_in_level, "{lvl}: gates");
        total_gates += gates.len();
        for g in gates {
            let (m, b) = building_module(&state, g);
            assert!(
                m.base.has_flags(flags::CONSTRUCTED_FINISHED),
                "{lvl}: {g:?} finished"
            );
            assert!(m.base.has_flags(flags::TOROPEN), "{lvl}: {g:?} open");
            assert!(
                !m.base.has_flags(flags::TORCLOSE),
                "{lvl}: {g:?} not closed"
            );
            assert_eq!(b.master, ObjectId(0), "{lvl}: {g:?} without master");
            let doors = door_tiles_of(b);
            assert_eq!(doors.len(), 2, "{lvl}: {g:?} door tiles");
            for (x, z) in doors {
                let free = (0..4).all(|l| state.path.cell_cost(x, z, l) == COST_FREE);
                assert!(free, "{lvl}: {g:?} door tile ({x}, {z}) blocked");
            }
        }
    }
    assert_eq!(total_gates, 103);
}

/// No enemy watch on loading (re/arming-gates 3.D1): on no campaign level with walls does a wall
/// piece, a corner or a gate have members or a master (way a, `0x1B03` to parts created finished,
/// does not occur in the level data), so there is no area query and no key.
#[test]
fn campaign_walls_have_no_watch() {
    use nml_sim::id::ObjectId;

    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let with_walls = |g| is_wall_piece(g) || is_gate(g);
    assert_eq!(campaign_levels_with(&vfs, with_walls), WALL_LEVELS);
    for lvl in WALL_LEVELS {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        let state = new_match(&rules, &MatchSetup::default_for(&rules))
            .unwrap_or_else(|e| panic!("{lvl}: {e}"));
        let parts: Vec<ObjectId> = state
            .objects()
            .filter(|o| with_walls(o.guid))
            .map(|o| o.id)
            .collect();
        assert!(!parts.is_empty(), "{lvl}");
        for p in parts {
            let (_, b) = building_module(&state, p);
            assert!(b.slaves.is_empty(), "{lvl}: {p:?} with members");
            assert_eq!(b.master, ObjectId(0), "{lvl}: {p:?} with master");
            assert_eq!(b.check_area_id, -1, "{lvl}: {p:?} guarded");
        }
        assert!(state.area_queries.queries.is_empty(), "{lvl}: queries");
        assert_eq!(state.area_queries.counter, 0, "{lvl}: key assigned");
    }
}
