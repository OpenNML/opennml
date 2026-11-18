//! Tests of the control channel for scripts (protocol 28): `sim_setup` with `script_enabled`/`time_per_char` and
//! `script_state`. They need the settings of the viewer and therefore live here instead of next to
//! `SimHost::control_script_state` in `opennml-client`.

use opennml_client::game::sim_host::SimHost;
use opennml_sim::id::ObjectId;
use opennml_sim::script::LEVEL_CONTAINER_ID;

use crate::testworld_viewer::{
    script_host, script_host_with_areas, script_host_with_dialogs, sim_setup,
};

/// State of the simulation and the host for "the query changes nothing".
fn sums(host: &SimHost) -> (u64, u64) {
    (host.checksum(), opennml_sim::save::checksum(&host.state))
}

#[test]
fn sim_setup_switches_scripts_on() {
    use opennml_client::game::testworld::{abl, script_rules};
    use opennml_sim::script::ScriptSetup;
    let rules = script_rules(&abl(&[(&[], &["n12;on"])]));
    let setup = opennml_sim::setup::MatchSetup::default_for(&rules);
    let mut host = SimHost::new(
        rules,
        setup,
        opennml_client::game::options::DEFAULT_GAME_SPEED,
    )
    .unwrap();
    let mut config = crate::config::Config::default();
    assert!(!config.script_enabled, "viewer default: off");
    assert_eq!(config.time_per_char, 55);
    // Restart with the default: no container.
    let args = crate::app::SimSetupArgs {
        seed: Some(3),
        ..Default::default()
    };
    sim_setup(&mut host, &mut config, args).unwrap();
    assert_eq!(host.setup.script, ScriptSetup::default());
    assert!(host.state.script.level.is_none());
    // Switch on with its own speaking time.
    let args = crate::app::SimSetupArgs {
        script_enabled: Some(true),
        time_per_char: Some(80),
        ..Default::default()
    };
    sim_setup(&mut host, &mut config, args).unwrap();
    assert_eq!(
        host.setup.script,
        ScriptSetup {
            enabled: true,
            time_per_char: 80,
            ..Default::default()
        }
    );
    assert!(config.script_enabled);
    assert_eq!(config.time_per_char, 80);
    assert_eq!(host.state.script.time_per_char, 80);
    assert!(host.state.script.level.is_some());
    host.step_once();
    let level = host.state.script.level.as_ref().unwrap();
    assert!(
        level.switches.contains_key("on"),
        "the level script is running"
    );
    // Invalid speaking time: error message, no restart.
    let before = sums(&host);
    let error = sim_setup(
        &mut host,
        &mut config,
        crate::app::SimSetupArgs {
            time_per_char: Some(0),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.contains("time_per_char = 0"), "{error}");
    assert_eq!(sums(&host), before, "no restart");
    assert_eq!(config.time_per_char, 80);
    // Switch off: no container again; the speaking time stays in the settings.
    let args = crate::app::SimSetupArgs {
        script_enabled: Some(false),
        ..Default::default()
    };
    sim_setup(&mut host, &mut config, args).unwrap();
    assert!(host.state.script.level.is_none());
    assert_eq!(
        host.setup.script,
        ScriptSetup {
            enabled: false,
            time_per_char: 80,
            ..Default::default()
        }
    );
    // A script that the load check rejects: switching on fails with the message, the match stays without a
    // script.
    let mut broken = opennml_client::game::testworld::rules();
    broken.scripts =
        opennml_sim::script::testing::rules_with(&abl(&[(&[], &["n39;absent"])]), &[], &[]);
    assert!(!broken.scripts.errors.is_empty());
    let setup = opennml_sim::setup::MatchSetup::default_for(&broken);
    let mut host = SimHost::new(
        broken,
        setup,
        opennml_client::game::options::DEFAULT_GAME_SPEED,
    )
    .unwrap();
    let mut config = crate::config::Config::default();
    let error = sim_setup(
        &mut host,
        &mut config,
        crate::app::SimSetupArgs {
            script_enabled: Some(true),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.contains("absent"), "{error}");
    assert!(!config.script_enabled);
    assert!(!host.setup.script.enabled);
}

#[test]
fn script_state_reads_without_changing_the_checksum() {
    use opennml_client::game::testworld::abl;
    use opennml_control::ScriptBlockRef;
    use opennml_sim::script::testing::LEVEL_PATH;
    let level = abl(&[
        // Block 0: waits for the text event 5; 35 with an unknown name is true, 15 has no case.
        (&["|19;5", "&35;x", "|15"], &["n12;text"]),
        // Block 1: switch, timer, then wait 1000 ms.
        (&[], &["n12;on", "n25;clock;3000", "w40;1000", "n11;20"]),
        // Block 2: repeatable, random switch.
        (&[], &["n10", "n33;throw;10"]),
    ]);
    let (mut host, _) = script_host(&level);
    for _ in 0..3 {
        host.step_once();
    }
    host.state.script.mission.speech.events.push(5);
    opennml_sim::names::register(&mut host.state.names, "Hero", ObjectId(65537));
    opennml_sim::names::register(&mut host.state.names, "wagon", ObjectId(65538));
    let before = sums(&host);
    let pending = host.pending_commands().len();
    let state = host.state.clone();

    let all = host
        .control_script_state(
            Some(ScriptBlockRef {
                container: LEVEL_CONTAINER_ID,
                index: 0,
            }),
            None,
            None,
        )
        .unwrap();
    assert!(all.enabled && !all.dialog_running);
    assert_eq!(all.containers.len(), 1);
    let c = &all.containers[0];
    assert_eq!(
        (c.id, c.path.as_str(), c.counter),
        (LEVEL_CONTAINER_ID, LEVEL_PATH, -1)
    );
    assert_eq!(c.switches, [("on".to_string(), 1)]);
    // Timer from the following tick: 3000 - 2 * 50.
    assert_eq!(c.timers, [("clock".to_string(), 2900)]);
    assert!(c.variables.is_empty());
    assert_eq!(c.random_switches.len(), 1);
    assert_eq!(c.random_switches[0].0, "throw");
    let blocks: Vec<_> = c
        .blocks
        .iter()
        .map(|b| (b.index, b.name.as_str(), b.state.as_str(), b.cur_action))
        .collect();
    assert_eq!(
        blocks,
        [
            (0, "block0]", "bereit", 0),
            (1, "block1]", "wartet", 2),
            (2, "block2]", "wiederholbar", 0)
        ]
    );
    assert_eq!(c.blocks[0].wait, None);
    assert_eq!(
        c.blocks[1].wait.as_deref(),
        Some("Action 2 (w40;1000): 900 ms remaining")
    );
    let conditions: Vec<_> = all
        .conditions
        .as_ref()
        .unwrap()
        .iter()
        .map(|k| (k.number, k.or, k.params.join(";"), k.value))
        .collect();
    assert_eq!(
        conditions,
        [
            (19, true, "5".to_string(), Some(true)),
            (35, false, "x".to_string(), Some(true)),
            (15, true, String::new(), Some(false)),
        ]
    );
    assert_eq!(all.speech.events, [5]);
    assert_eq!(all.speech.tips.len(), 2);
    assert!(all.speech.line.is_none());
    let mut names = all.names.clone();
    names.sort();
    assert_eq!(
        names,
        [
            ("hero".to_string(), "65537".to_string()),
            ("wagon".to_string(), "65538".to_string())
        ]
    );
    // Block log (default 50 steps): all three steps, messages as JSON.
    let steps: Vec<u64> = all.log.iter().map(|e| e.step).collect();
    assert_eq!((steps.first(), steps.last()), (Some(&1), Some(&3)));
    assert!(all.log.contains(&opennml_control::ScriptLogEntry {
        step: 1,
        trace: serde_json::json!({"Suspended": {"container": LEVEL_CONTAINER_ID, "block": 1, "action": 2}}),
    }));
    assert!(all.log.contains(&opennml_control::ScriptLogEntry {
        step: 3,
        trace: serde_json::json!({"Fired": {"container": LEVEL_CONTAINER_ID, "block": 2}}),
    }));
    // Names by substring (case-insensitive), the log of the last steps.
    let some = host
        .control_script_state(None, Some("HER"), Some(1))
        .unwrap();
    assert!(some.conditions.is_none());
    assert_eq!(some.names, [("hero".to_string(), "65537".to_string())]);
    assert!(!some.log.is_empty() && some.log.iter().all(|e| e.step == 3));
    let none = host
        .control_script_state(None, Some("xyz"), Some(0))
        .unwrap();
    assert!(none.names.is_empty() && none.log.is_empty());
    // Errors: unknown container, block beyond the end, log too long.
    let block = |container, index| Some(ScriptBlockRef { container, index });
    let error = host
        .control_script_state(block(3, 0), None, None)
        .unwrap_err();
    assert!(error.contains("container 3"), "{error}");
    let error = host
        .control_script_state(block(-1, 0), None, None)
        .unwrap_err();
    assert!(error.contains("container -1"), "{error}");
    let error = host
        .control_script_state(block(LEVEL_CONTAINER_ID, 9), None, None)
        .unwrap_err();
    assert!(error.contains("block 9"), "{error}");
    let error = host
        .control_script_state(None, None, Some(1001))
        .unwrap_err();
    assert!(error.contains("log = 1001"), "{error}");
    // None of this changes the simulation (19 consumes the event only on the copy).
    assert_eq!(sums(&host), before);
    assert_eq!(host.state, state);
    assert_eq!(host.pending_commands().len(), pending);
    // The log is host state: a restart clears it; without a script everything is empty.
    host.restart(host.setup.clone()).unwrap();
    assert!(
        host.control_script_state(None, None, None)
            .unwrap()
            .log
            .is_empty()
    );
    let mut off = host.setup.clone();
    off.script.enabled = false;
    host.restart(off).unwrap();
    host.step_once();
    let empty = host.control_script_state(None, None, None).unwrap();
    assert!(!empty.enabled && empty.containers.is_empty() && empty.log.is_empty());
}

#[test]
fn script_log_keeps_the_last_thousand_steps() {
    use opennml_client::game::testworld::abl;
    // A repeatable block fires in every step.
    let (mut host, _) = script_host(&abl(&[(&[], &["n10"])]));
    for _ in 0..1005 {
        host.step_once();
    }
    let all = host
        .control_script_state(None, None, Some(opennml_control::MAX_SCRIPT_LOG_STEPS))
        .unwrap();
    let steps: Vec<u64> = all.log.iter().map(|e| e.step).collect();
    assert_eq!(steps.len(), 1000);
    assert_eq!((steps[0], steps[999]), (6, 1005));
    assert_eq!(
        all.log[999].trace,
        serde_json::json!({"Fired": {"container": LEVEL_CONTAINER_ID, "block": 0}})
    );
}

#[test]
fn script_events_keep_the_last_thousand_steps_and_at_most_4096() {
    use opennml_client::game::sim_host::{PRESENT_LOG_EVENTS, PresentLog};
    use opennml_client::game::testworld::abl;
    use opennml_sim::script::Presentation;
    use opennml_sim::state::SimEvent;
    // A repeatable block reports cinemascope in every step.
    let (mut host, _) = script_host(&abl(&[(&[], &["n10", "n147"])]));
    for _ in 0..1005 {
        host.step_once();
    }
    let all = host.control_script_events(Some(1000)).unwrap();
    let steps: Vec<u64> = all.events.iter().map(|e| e.step).collect();
    assert_eq!(steps.len(), 1000);
    assert_eq!((steps[0], steps[999]), (6, 1005));
    // At most 4,096 events, the oldest discarded first.
    let mut log = PresentLog::default();
    let many: Vec<SimEvent> = (0..5000)
        .map(|_| SimEvent::Present(Presentation::QuickSave))
        .collect();
    log.record(1, &many);
    assert_eq!(log.entries.len(), PRESENT_LOG_EVENTS);
}

#[test]
fn script_state_reports_quests_and_countdown() {
    use opennml_client::game::testworld::abl;
    use opennml_control::ScriptQuestInfo;
    // Main objective 1 set (text 105), main objective 2 failed, sub-objective 2 set (text 40), task 2, countdown
    // 90 s shown in the level container.
    let level = abl(&[(
        &[],
        &[
            "n27;1;105",
            "n187;2",
            "n182;2;40",
            "n198;1",
            "n200;1;2",
            "n11;90",
            "n15",
        ],
    )]);
    let (mut host, _) = script_host(&level);
    // Before the first tick: ten empty objectives per list, no display.
    let empty = host.control_script_state(None, None, None).unwrap();
    let quest = |status, text| ScriptQuestInfo { status, text };
    assert_eq!(empty.main_quests, [quest(0, 0); 10]);
    assert_eq!(empty.sub_quests, [quest(0, 0); 10]);
    assert_eq!((empty.countdown_container, empty.countdown_s), (0, 0));
    assert!(empty.task_status.is_empty() && empty.area_overrides.is_empty());

    host.step_once();
    let state = host.control_script_state(None, None, None).unwrap();
    assert_eq!(state.main_quests.len(), 10);
    assert_eq!(state.main_quests[0], quest(3, 105));
    assert_eq!(state.main_quests[1], quest(10, 0));
    assert_eq!(state.main_quests[2], quest(0, 0));
    // 198 sets the first sub-objective to status 1; 182 set the second to 3.
    assert_eq!(state.sub_quests.len(), 10);
    assert_eq!(state.sub_quests[0], quest(1, 0));
    assert_eq!(state.sub_quests[1], quest(3, 40));
    assert_eq!(state.task_status, [(2, 1)]);
    // The level container has ID 1; the display follows the counter after the tick (89,950 ms -> 89 s).
    assert_eq!(state.countdown_container, LEVEL_CONTAINER_ID);
    assert_eq!(state.containers[0].id, 1);
    assert_eq!(state.containers[0].counter, 89_950);
    assert_eq!(state.countdown_s, 89);
}

#[test]
fn script_state_reports_changed_areas_and_leaves_the_checksum() {
    use opennml_client::game::testworld::abl;
    use opennml_control::ScriptBlockRef;
    // Condition 6 (area seen) swaps the swapped corners in the stored area. `script_state` evaluates on a copy:
    // the real state (and the checksum) stay, `area_overrides` does not change until the tick itself.
    let level = abl(&[(&["&6;0;area meadow"], &["n12;seen"])]);
    let (mut host, _) = script_host_with_areas(&level, &[("meadow", [9, 8, 2, 1])]);
    let before = sums(&host);
    let state = host.state.clone();
    let block = Some(ScriptBlockRef {
        container: LEVEL_CONTAINER_ID,
        index: 0,
    });
    let result = host.control_script_state(block, None, None).unwrap();
    assert_eq!(result.conditions.as_ref().map(Vec::len), Some(1));
    assert!(result.area_overrides.is_empty(), "copy, not the state");
    assert_eq!(sums(&host), before);
    assert_eq!(host.state, state);
    // In the tick the area is normalized and then appears in the response.
    host.step_once();
    let result = host.control_script_state(None, None, None).unwrap();
    assert_eq!(
        result.area_overrides,
        [("meadow".to_string(), [2, 1, 9, 8])]
    );
}

// --- Script commands (protocol 32) -----------------------------------------------------------------

/// A foreign unit on slot 3 at tile (x, 10).
fn foreign_unit(host: &mut SimHost, x: i32) -> ObjectId {
    opennml_client::game::testworld::spawn(
        host,
        opennml_client::game::testworld::COLONIST,
        3,
        (x, 10),
    )
}

/// Name `name` onto the objects `ids` (list like `script::names`, `;`-separated).
fn name_objects(host: &mut SimHost, name: &str, ids: &[ObjectId]) {
    let list: Vec<String> = ids.iter().map(|id| id.as_i32().to_string()).collect();
    host.state.names.set(name, list.join(";"));
}

/// Remove a unit (its ID becomes stale).
fn remove_object(host: &mut SimHost, id: ObjectId) {
    let rules = host.rules.clone();
    let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
    opennml_sim::world::destroy_object(&mut sim, id);
}

#[test]
fn script_command_skip_dialog_is_logged_and_replays() {
    use opennml_client::game::testworld::abl;
    use opennml_control::ScriptAction;
    use opennml_sim::command::CommandKind;
    use opennml_sim::save::{Replay, checksum};
    // The level script starts dialog `d` (action 39), which waits 1000 ms; then it sets "after".
    let level = abl(&[(&[], &["n39;d", "n12;after"])]);
    let dialog = abl(&[(&[], &["w40;1000", "n12;x"])]);
    let (mut host, _) = script_host_with_dialogs(&level, &[("d", &dialog)]);

    // Without a running dialog: error, nothing queued, nothing changed.
    let before = sums(&host);
    let error = host
        .control_script_command(ScriptAction::SkipDialog)
        .unwrap_err();
    assert!(error.contains("no dialog running"), "{error}");
    assert!(host.pending_commands().is_empty());
    assert_eq!(sums(&host), before);

    for _ in 0..3 {
        host.step_once();
    }
    assert!(opennml_sim::script::dialog_running(&host.state));
    let reply = host
        .control_script_command(ScriptAction::SkipDialog)
        .unwrap();
    assert_eq!(reply, serde_json::json!({"step": 3}));
    // The command takes effect at the next tick (the dialog is still running) and then appears in the command log.
    assert!(opennml_sim::script::dialog_running(&host.state));
    assert_eq!(host.pending_commands().len(), 1);
    assert!(host.log.is_empty());
    host.step_once();
    assert!(
        !opennml_sim::script::dialog_running(&host.state),
        "dialog ends in the next tick"
    );
    assert_eq!(host.log.len(), 1);
    assert_eq!(host.log[0].step, 3);
    assert_eq!(host.log[0].command.kind, CommandKind::ScriptSkipDialog);
    assert_eq!(host.log[0].command.player, host.active_player);
    assert!(
        host.script_log
            .entries
            .iter()
            .any(|(step, t)| *step == 4 && *t == opennml_sim::script::ScriptTrace::DialogEnded),
        "block log reports the dialog end"
    );
    for _ in 0..5 {
        host.step_once();
    }
    let level_state = host.state.script.level.as_ref().unwrap();
    assert!(
        level_state.switches.contains_key("after"),
        "level script continues"
    );

    // Replay from the start: the same checksum sequence in every step of the history.
    let replay = Replay::from_bytes(&host.replay().unwrap().to_bytes()).unwrap();
    assert_eq!(replay.commands.len(), 1);
    assert!(host.hashes.len() >= 9);
    for &(steps, hash) in &host.hashes {
        let state = replay.play(&host.rules, steps).unwrap();
        assert_eq!(checksum(&state), hash, "step {steps}");
    }
    let mut other = SimHost::new(
        host.rules.clone(),
        host.setup.clone(),
        opennml_client::game::options::DEFAULT_GAME_SPEED,
    )
    .unwrap();
    other
        .load_replay(replay.clone(), Some(host.state.steps))
        .unwrap();
    assert_eq!(other.checksum(), host.checksum());
    assert!(!opennml_sim::script::dialog_running(&other.state));
    // Counter-check: without the command the dialog keeps running and the checksum differs.
    let without = Replay {
        commands: Vec::new(),
        ..replay
    };
    let state = without.play(&host.rules, host.state.steps).unwrap();
    assert_ne!(checksum(&state), host.checksum());
    assert!(opennml_sim::script::dialog_running(&state));
}

#[test]
fn script_state_log_reports_commands() {
    use opennml_client::game::testworld::abl;
    // Action 191 (halt) on two units: the first ID is stale (rejected), the second lives.
    let level = abl(&[(&[], &["n191;enemy", "n12;done"])]);
    let (mut host, _) = script_host(&level);
    let live = foreign_unit(&mut host, 10);
    let gone = foreign_unit(&mut host, 12);
    remove_object(&mut host, gone);
    name_objects(&mut host, "enemy", &[gone, live]);
    host.step_once();
    let state = host.control_script_state(None, None, None).unwrap();
    let commands: Vec<&opennml_control::ScriptLogEntry> = state
        .log
        .iter()
        .filter(|e| e.trace.get("Command").is_some())
        .collect();
    assert_eq!(commands.len(), 2, "{:?}", state.log);
    let first = &commands[0].trace["Command"];
    let second = &commands[1].trace["Command"];
    for entry in [first, second] {
        assert_eq!(entry["container"], LEVEL_CONTAINER_ID);
        assert_eq!(entry["block"], 0);
        assert_eq!(entry["action"], 191);
        assert_eq!(entry["code"], 0x3C);
    }
    assert!(first["outcome"]["Rejected"].is_string(), "{first}");
    assert_eq!(second["outcome"], "Accepted");
    assert_eq!(commands[0].step, 1);
    // The counters of the window: one rejection with a reason, one accepted command.
    assert_eq!(
        state.commands.actions,
        [opennml_control::ScriptActionCount {
            action: 191,
            accepted: 1,
            partial: 0,
            rejected: 1
        }]
    );
    assert_eq!(state.commands.rejections.len(), 1);
    let rejection = &state.commands.rejections[0];
    assert_eq!(
        (
            rejection.step,
            rejection.container,
            rejection.block,
            rejection.action,
            rejection.code
        ),
        (1, LEVEL_CONTAINER_ID, 0, 191, 0x3C)
    );
    assert_eq!(first["outcome"]["Rejected"], rejection.reason.as_str());
    assert!(state.results.is_empty(), "{:?}", state.results);
    // A restart clears counters and log (host state like the block log).
    host.restart(host.setup.clone()).unwrap();
    let state = host.control_script_state(None, None, None).unwrap();
    assert!(state.commands.actions.is_empty() && state.commands.rejections.is_empty());
}

#[test]
fn queries_do_not_change_the_checksum() {
    use opennml_client::game::testworld::abl;
    use opennml_control::ScriptBlockRef;
    // Script commands (accepted, rejected) and a game result in the state, then all queries.
    let level = abl(&[(&[], &["n191;enemy", "n13;0", "n12;done"])]);
    let (mut host, _) = script_host(&level);
    let live = foreign_unit(&mut host, 10);
    let gone = foreign_unit(&mut host, 12);
    remove_object(&mut host, gone);
    name_objects(&mut host, "enemy", &[gone, live]);
    // The test world has no playing slots (result 3 = unoccupied): let slots 0 and 1 play so that `n13;0` decides
    // (0 wins, 1 is not a mutual friend and loses).
    host.state.players[0].result = 0;
    host.state.players[1].result = 0;
    for _ in 0..3 {
        host.step_once();
    }
    let before = sums(&host);
    let state = host.state.clone();
    let pending = host.pending_commands().len();
    let log = host.log.len();

    let block = Some(ScriptBlockRef {
        container: LEVEL_CONTAINER_ID,
        index: 0,
    });
    let first = host
        .control_script_state(block, Some("enemy"), Some(1000))
        .unwrap();
    let second = host.control_script_state(None, None, None).unwrap();
    assert_eq!(
        first.commands, second.commands,
        "counters do not depend on the log window"
    );
    assert_eq!(first.results, second.results);
    assert_eq!(first.results, [(0, 2), (1, 1)], "n13 has decided");
    let _ = host.query(None, None, None, 10_000);
    let _ = host.query(Some(3), None, Some(&[live.0, gone.0]), 10);
    let _ = host.hash_result();
    // A failed intervention likewise changes nothing.
    assert!(
        host.control_script_command(opennml_control::ScriptAction::SkipDialog)
            .is_err()
    );
    assert_eq!(sums(&host), before);
    assert_eq!(host.state, state);
    assert_eq!(host.pending_commands().len(), pending);
    assert_eq!(host.log.len(), log);
}

#[test]
fn script_events_round_trip_and_leave_the_checksum() {
    use opennml_client::game::sim_control::parse_sim_command;
    use opennml_client::game::testworld::abl;
    use opennml_sim::script::Presentation;
    // Block 0 runs in the first tick: profile variable, weather, music, cinemascope, lock.
    let level = abl(&[(
        &[],
        &[
            "n360;value;9",
            "n382;value",
            "n209;1",
            "n9;5",
            "n147",
            "n18",
        ],
    )]);
    let (mut host, _) = script_host(&level);
    for _ in 0..3 {
        host.step_once();
    }
    // Host commands: selection mirror and last button (after 147, which clears the mirror).
    let unit = foreign_unit(&mut host, 10);
    for command in [
        serde_json::json!({"player": 0, "kind": {"ScriptSelection": {"ids": [unit.0]}}}),
        serde_json::json!({"player": 0, "kind": {"ScriptButton": {"word": 511}}}),
    ] {
        host.control_submit(parse_sim_command(command).unwrap());
    }
    host.step_once();
    assert_eq!(host.state.steps, 4);

    let before = sums(&host);
    let state = host.state.clone();
    let pending = host.pending_commands().len();
    let log = host.log.len();

    let result = host.control_script_events(None).unwrap();
    // The events with step, in reporting order; everything happened in the first step.
    let events: Vec<(u64, Presentation)> = result
        .events
        .iter()
        .map(|e| (e.step, serde_json::from_value(e.event.clone()).unwrap()))
        .collect();
    assert_eq!(
        events,
        [
            (
                1,
                Presentation::ProfileVar {
                    name: "value".into(),
                    value: 9
                }
            ),
            (1, Presentation::Weather { kind: 1 }),
            (1, Presentation::Music { id: 5 }),
            (1, Presentation::Cinemascope { on: true }),
            (1, Presentation::UiLock { on: true }),
        ]
    );
    // State, UI and profile variables; the camera is set by the viewer (here `null`).
    assert_eq!(result.presentation["weather"], 1);
    assert_eq!(result.presentation["music"], 5);
    assert_eq!(
        result.ui,
        opennml_control::ScriptUiInfo {
            ui_locked: true,
            cinemascope: true,
            selection: vec![unit.0],
            last_button: 511,
        }
    );
    assert_eq!(result.profile_vars, [("value".to_string(), 9)]);
    assert_eq!(result.camera, serde_json::Value::Null);
    // `script_state` carries the same UI (`ui`).
    assert_eq!(
        host.control_script_state(None, None, Some(0)).unwrap().ui,
        result.ui
    );
    // Window of the steps: 0 steps no events, one step only the last (none here, tick 4 was quiet).
    assert!(
        host.control_script_events(Some(0))
            .unwrap()
            .events
            .is_empty()
    );
    assert!(
        host.control_script_events(Some(1))
            .unwrap()
            .events
            .is_empty()
    );
    assert_eq!(host.control_script_events(Some(4)).unwrap().events.len(), 5);
    let error = host.control_script_events(Some(1001)).unwrap_err();
    assert!(error.contains("steps = 1001"), "{error}");
    // The response goes through JSON and back.
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(
        serde_json::from_value::<opennml_control::ScriptEventsResult>(value).unwrap(),
        result
    );
    // None of this changes the simulation or the host.
    assert_eq!(sums(&host), before);
    assert_eq!(host.state, state);
    assert_eq!(host.pending_commands().len(), pending);
    assert_eq!(host.log.len(), log);
    // The log of the events is host state: a restart clears it.
    host.restart(host.setup.clone()).unwrap();
    assert!(host.control_script_events(None).unwrap().events.is_empty());
}

#[test]
fn sim_command_accepts_ui_commands() {
    use opennml_client::game::sim_control::{command_name, parse_sim_command};
    use opennml_client::game::testworld::abl;
    use opennml_sim::command::CommandKind;
    // JSON form (protocol 33): `ScriptSelection { ids }` and `ScriptButton { word }` go through `sim_command`.
    let selection = parse_sim_command(serde_json::json!(
        {"player": 0, "kind": {"ScriptSelection": {"ids": [2048, 2049]}}}
    ))
    .unwrap();
    assert_eq!(
        selection.kind,
        CommandKind::ScriptSelection {
            ids: vec![ObjectId(2048), ObjectId(2049)]
        }
    );
    assert_eq!(command_name(&selection.kind), "ScriptSelection");
    let button = parse_sim_command(serde_json::json!(
        {"player": 0, "kind": {"ScriptButton": {"word": 511}}}
    ))
    .unwrap();
    assert_eq!(button.kind, CommandKind::ScriptButton { word: 511 });
    assert_eq!(command_name(&button.kind), "ScriptButton");
    for command in [selection, button] {
        let back = parse_sim_command(serde_json::to_value(&command).unwrap()).unwrap();
        assert_eq!(back, command);
    }
    // Without a word and with a wrong type: message "command invalid".
    let error = parse_sim_command(serde_json::json!(
        {"player": 0, "kind": {"ScriptButton": {}}}
    ))
    .unwrap_err();
    assert!(error.starts_with("command invalid"), "{error}");

    // Effect at the next tick: mirror and last button (conditions 35 and 38 read them).
    let level = abl(&[(&["&38;0;511"], &["n12;button"])]);
    let (mut host, _) = script_host(&level);
    let unit = foreign_unit(&mut host, 10);
    let reply = host.control_submit(
        parse_sim_command(serde_json::json!(
            {"player": 0, "kind": {"ScriptSelection": {"ids": [unit.0]}}}
        ))
        .unwrap(),
    );
    assert_eq!(reply, serde_json::json!({"step": 0}));
    host.control_submit(
        parse_sim_command(serde_json::json!(
            {"player": 0, "kind": {"ScriptButton": {"word": 511}}}
        ))
        .unwrap(),
    );
    assert!(
        host.state.script.mission.selection.is_empty(),
        "not until the tick"
    );
    host.step_once();
    host.step_once();
    let ui = host.control_script_state(None, None, Some(0)).unwrap().ui;
    assert_eq!((ui.selection, ui.last_button), (vec![unit.0], 511));
    assert!(
        host.state
            .script
            .level
            .as_ref()
            .unwrap()
            .switches
            .contains_key("button"),
        "condition 38 saw the button"
    );
    // Both are in the command log (replay).
    let names: Vec<String> = host
        .log
        .iter()
        .map(|r| command_name(&r.command.kind))
        .collect();
    assert_eq!(names, ["ScriptSelection", "ScriptButton"]);
    // More than 30 IDs are rejected by the simulation; the mirror stays.
    let many: Vec<u32> = (0..31).map(|i| 4096 + i).collect();
    host.control_submit(
        parse_sim_command(serde_json::json!(
            {"player": 0, "kind": {"ScriptSelection": {"ids": many}}}
        ))
        .unwrap(),
    );
    host.step_once();
    assert_eq!(host.state.script.mission.selection, [unit]);
}
