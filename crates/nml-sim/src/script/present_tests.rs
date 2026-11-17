//! Tests of the presentation state and the new mission fields.

use super::present::MAX_INFO_FRAMES;
use super::testing;
use super::tests::{rules_for, rules_with_areas, script, setup, spawn};
use super::*;
use crate::id::ObjectId;
use crate::rules::Rules;
use crate::setup::new_match;
use crate::sim::Sim;
use crate::state::{SimEvent, SimState};
use crate::tick::step;

#[test]
fn new_mission_fields_start_empty() {
    let rules = rules_for(&script(&[(&[], &["n12;a"])]));
    for enabled in [true, false] {
        let state = new_match(&rules, &setup(enabled)).expect("match");
        let mission = &state.script.mission;
        assert!(mission.selection.is_empty());
        assert_eq!(mission.last_button, 0);
        assert!(!mission.ui_locked);
        assert!(!mission.cinemascope);
        assert!(mission.profile_vars.is_empty());
        assert_eq!(mission.presentation, PresentationMemory::default());
        assert_eq!(mission.presentation.weather, 0);
        assert_eq!(mission.presentation.portraits, [None; 4]);
        assert!(mission.presentation.info_frames.is_empty());
    }
}

#[test]
fn profile_vars_come_from_the_setup() {
    let rules = rules_for(&script(&[(&[], &["n12;a"])]));
    let mut on = setup(true);
    on.script
        .profile_vars
        .insert("lagerhausexplosion".to_owned(), 1);
    let state = new_match(&rules, &on).expect("match");
    assert_eq!(
        state.script.mission.profile_vars.get("lagerhausexplosion"),
        Some(&1)
    );
    assert_eq!(state.script.mission.profile_vars.len(), 1);
    // Without a script the default state stays (start_match then does nothing).
    let mut off = setup(false);
    off.script
        .profile_vars
        .insert("lagerhausexplosion".to_owned(), 1);
    let state = new_match(&rules, &off).expect("match");
    assert_eq!(state.script, ScriptState::default());
}

#[test]
fn present_event_round_trips_through_serde() {
    let all = vec![
        Presentation::CameraCenter {
            tile_x: -3,
            tile_z: 40,
        },
        Presentation::CameraToObject {
            id: ObjectId(0x801),
        },
        Presentation::CameraCut {
            camera: "kamera1".into(),
        },
        Presentation::CameraMove {
            camera: "kamera2".into(),
            ms: 2000,
        },
        Presentation::CameraBackup,
        Presentation::CameraRestoreBackup,
        Presentation::CameraMoveToBackup { ms: 300 },
        Presentation::CameraToGame { ms: 0 },
        Presentation::CameraBind { id: None },
        Presentation::CameraBind {
            id: Some(ObjectId(0x802)),
        },
        Presentation::CameraRotate {
            degrees: -90,
            ms: 500,
        },
        Presentation::Fade {
            into: true,
            ms: 1000,
        },
        Presentation::Cinemascope { on: true },
        Presentation::UiLock { on: false },
        Presentation::Portrait {
            slot: 1,
            portrait: 7,
            frame: 2,
        },
        Presentation::PortraitHide { slot: 3 },
        Presentation::Highlight { id: None },
        Presentation::Speech {
            text: 120,
            slot: 4,
            ms: 3000,
        },
        Presentation::SpeechEnd { text: 120, slot: 0 },
        Presentation::Tip {
            text: 9,
            slot: 1,
            ms: 4000,
        },
        Presentation::TipEnd { text: 9, slot: 1 },
        Presentation::InfoFrame {
            text: 5,
            x: 10,
            y: 20,
            style: 2,
        },
        Presentation::InfoSpeechFrame {
            text: 5,
            slot: 4,
            portrait: 1,
            frame: 0,
        },
        Presentation::InfoFrameClose { text: 5 },
        Presentation::ScreenText {
            text: 77,
            seconds: 6,
        },
        Presentation::MinimapPing {
            player: 1,
            tile_x: 5,
            tile_z: 6,
        },
        Presentation::Music { id: 3 },
        Presentation::AttackMusic { id: 4 },
        Presentation::AttackThreshold { points: 250 },
        Presentation::Sound2d { id: 12 },
        Presentation::Light {
            preset: "nacht".into(),
        },
        Presentation::LightRestoreLevel,
        Presentation::Weather { kind: 2 },
        Presentation::DistanceFog { on: true },
        Presentation::FogValues {
            color: 0x00ff_8040,
            start: 100,
            end: 900,
            ms: 1500,
            swap_red_blue: true,
        },
        Presentation::FogShowAll { on: false },
        Presentation::QuickSave,
        Presentation::ProfileVar {
            name: "lagerhausexplosion".into(),
            value: 1,
        },
    ];
    for p in all {
        let event = SimEvent::Present(p);
        let bytes = postcard::to_stdvec(&event).unwrap();
        assert_eq!(postcard::from_bytes::<SimEvent>(&bytes).unwrap(), event);
    }
}

#[test]
fn emit_pushes_a_present_event() {
    let rules = rules_for(&script(&[(&[], &["n12;a"])]));
    let mut state = new_match(&rules, &setup(true)).unwrap();
    {
        let mut sim = Sim::new(&mut state, &rules);
        present::emit(&mut sim, Presentation::QuickSave);
    }
    assert_eq!(
        state.events,
        vec![SimEvent::Present(Presentation::QuickSave)]
    );
}

#[test]
fn presentation_memory_survives_saving_and_loading() {
    use crate::save::testkit;
    let rules = rules_for(&script(&[(&[], &["n12;a"])]));
    let mut state = new_match(&rules, &setup(true)).unwrap();
    let mission = &mut state.script.mission;
    mission.ui_locked = true;
    mission.cinemascope = true;
    mission.last_button = -4;
    mission.selection = vec![ObjectId(0x800), ObjectId(0x80F)];
    mission.profile_vars.insert("a".into(), 5);
    let m = &mut mission.presentation;
    m.light = Some("nacht".into());
    m.weather = 2;
    m.music = Some(3);
    m.attack_threshold = Some(100);
    m.fog = Some(FogMemory {
        color: 1,
        start: 2,
        end: 3,
        swap_red_blue: true,
    });
    m.faded_out = true;
    m.portraits[2] = Some(PortraitMemory {
        portrait: 4,
        frame: 1,
    });
    m.info_frames.insert(
        8,
        InfoFrameMemory::Window {
            x: 1,
            y: 2,
            style: 1,
        },
    );
    m.info_frames.insert(
        9,
        InfoFrameMemory::Speech {
            slot: 4,
            portrait: 1,
            frame: 0,
        },
    );
    m.highlight = Some(ObjectId(0x800));
    let loaded = testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.script.mission, state.script.mission);
    assert_eq!(
        crate::save::checksum(&loaded),
        crate::save::checksum(&state)
    );
}

// ---------------------------------------------------------------------------------------------
// Camera and light events
// ---------------------------------------------------------------------------------------------

/// Rules with areas and the speech lengths `(text ID, OGG ms, units)`.
fn camera_rules(level: &str, areas: &[(&str, [i32; 4])], speech: &[(i32, u32, u32)]) -> Rules {
    let mut rules = rules_with_areas(level, areas);
    rules.scripts = testing::with_speech(rules.scripts, speech);
    rules
}

/// `n` steps; the `Present` events in order.
fn run_present(state: &mut SimState, rules: &Rules, n: usize) -> Vec<Presentation> {
    let mut all = Vec::new();
    for _ in 0..n {
        for e in step(state, rules, &[]) {
            match e {
                SimEvent::Present(p) => all.push(p),
                // A handled presentation number is never "no effect yet", even if it ends silently
                // (unknown area, missing object).
                SimEvent::Script(ScriptTrace::Pending { action, .. }) => {
                    panic!("action {action} reports Pending")
                }
                _ => {}
            }
        }
    }
    all
}

fn started(rules: &Rules) -> SimState {
    new_match(rules, &setup(true)).expect("match with script")
}

/// Creates an ID list under `name` (`"a;b"`), as the original keeps it for several objects;
/// `names::register` alone overwrites (E-145).
fn name_list(sim: &mut Sim, name: &str, ids: &[ObjectId]) {
    let list: Vec<String> = ids.iter().map(|id| id.as_i32().to_string()).collect();
    sim.state.names.set(name, list.join(";"));
}

/// One action per block; the events are in block order (the container runs the blocks from back to front
/// like the original, `run::tick_container`).
fn present_of(actions: &[&str]) -> Vec<Presentation> {
    let blocks: Vec<(&[&str], &[&str])> = actions
        .iter()
        .map(|a| (&[][..], std::slice::from_ref(a)))
        .collect();
    let rules = camera_rules(
        &script(&blocks),
        &[("feld", [3, 4, 6, 9])],
        &[(300, 4000, 20)],
    );
    let mut state = started(&rules);
    let mut events = run_present(&mut state, &rules, 3);
    events.reverse();
    events
}

#[test]
fn camera_actions_emit_their_events() {
    let got = present_of(&[
        "n141;1;kam1",
        "n142;1;kam2;2500",
        "n143",
        "n144",
        "n172;750",
        "n176",
        "n212;-90;500",
        "n216;1200",
        "n220;1;kam3;50;300",
        "n221;90;50;300",
        "n222;50;300",
        "n223;50;300",
    ]);
    assert_eq!(
        got,
        vec![
            Presentation::CameraCut {
                camera: "kam1".into()
            },
            Presentation::CameraMove {
                camera: "kam2".into(),
                ms: 2500
            },
            Presentation::CameraBackup,
            Presentation::CameraRestoreBackup,
            Presentation::CameraToGame { ms: 750 },
            Presentation::CameraBind { id: None },
            Presentation::CameraRotate {
                degrees: -90,
                ms: 500
            },
            Presentation::CameraMoveToBackup { ms: 1200 },
            // Speech length 4,000 ms, 50%.
            Presentation::CameraMove {
                camera: "kam3".into(),
                ms: 2000
            },
            Presentation::CameraRotate {
                degrees: 90,
                ms: 2000
            },
            Presentation::CameraMoveToBackup { ms: 2000 },
            Presentation::CameraToGame { ms: 2000 },
        ]
    );
}

#[test]
fn camera_actions_with_timer_percent_use_the_timer() {
    // One block: the timer `t` (3,000 ms) is ready for the following percent forms.
    let rules = camera_rules(
        &script(&[(
            &[],
            &[
                "n25;t;3000",
                "n240;1;kam;10;t",
                "n241;45;10;t",
                "n242;10;t",
                "n243;10;t",
            ],
        )]),
        &[],
        &[],
    );
    let mut state = started(&rules);
    // 240 and 241 wait (time wait only with a `w` marker; here without), so a few steps suffice.
    let got = run_present(&mut state, &rules, 3);
    assert_eq!(
        got,
        vec![
            Presentation::CameraMove {
                camera: "kam".into(),
                ms: 300
            },
            Presentation::CameraRotate {
                degrees: 45,
                ms: 300
            },
            Presentation::CameraMoveToBackup { ms: 300 },
            Presentation::CameraToGame { ms: 300 },
        ]
    );
}

#[test]
fn camera_center_2_uses_the_c_division_and_ignores_the_player() {
    // Area (3, 4)-(6, 9): ((3+6)/2, (4+9)/2) = (4, 6); the player (7) is not read.
    let got = present_of(&["n2;7;area feld", "n2;1;area feld"]);
    let expect = Presentation::CameraCenter {
        tile_x: 4,
        tile_z: 6,
    };
    assert_eq!(got, vec![expect.clone(), expect]);
    // Unknown area: no event.
    assert!(present_of(&["n2;1;area nirgends"]).is_empty());
}

#[test]
fn camera_to_object_170_takes_the_first_id_and_missing_or_unknown_sends_nothing() {
    // First ID of the name; if this object is missing, nothing happens (no fallback to the second).
    let rules = camera_rules(
        &script(&[
            (&[], &["n170;held"]),
            (&[], &["n170;fehlt"]),
            (&[], &["n170;lebt"]),
        ]),
        &[],
        &[],
    );
    let mut state = started(&rules);
    let alive;
    {
        let mut sim = Sim::new(&mut state, &rules);
        let first = spawn(&mut sim, None, (2, 2));
        let second = spawn(&mut sim, None, (3, 3));
        name_list(&mut sim, "held", &[first, second]);
        alive = spawn(&mut sim, None, (4, 4));
        let other = spawn(&mut sim, None, (5, 5));
        name_list(&mut sim, "lebt", &[alive, other]);
        crate::world::destroy_object(&mut sim, first);
    }
    let got = run_present(&mut state, &rules, 2);
    // Only "lebt" (first ID present); "held" (first ID dead) and "fehlt" report nothing.
    assert_eq!(got, vec![Presentation::CameraToObject { id: alive }]);
}

#[test]
fn camera_bind_175_with_unknown_name_unbinds() {
    let rules = camera_rules(
        &script(&[(&[], &["n175;held"]), (&[], &["n175;fehlt"])]),
        &[],
        &[],
    );
    let mut state = started(&rules);
    let id;
    {
        let mut sim = Sim::new(&mut state, &rules);
        id = spawn(&mut sim, Some("held"), (2, 2));
    }
    let got = run_present(&mut state, &rules, 2);
    // The blocks run from back to front.
    assert_eq!(
        got,
        vec![
            Presentation::CameraBind { id: None },
            Presentation::CameraBind { id: Some(id) },
        ]
    );
}

#[test]
fn camera_bind_175_takes_the_first_id_even_when_destroyed() {
    let rules = camera_rules(&script(&[(&[], &["n175;held"])]), &[], &[]);
    let mut state = started(&rules);
    let first;
    {
        let mut sim = Sim::new(&mut state, &rules);
        first = spawn(&mut sim, None, (2, 2));
        let second = spawn(&mut sim, None, (3, 3));
        name_list(&mut sim, "held", &[first, second]);
        crate::world::destroy_object(&mut sim, first);
    }
    // 175 reads only the ID (the host target follows the object, if it is missing the binding is released): first ID.
    let got = run_present(&mut state, &rules, 2);
    assert_eq!(got, vec![Presentation::CameraBind { id: Some(first) }]);
}

#[test]
fn actions_177_178_unbind_like_176() {
    let got = present_of(&["n176", "n177;egal", "n178;egal"]);
    assert_eq!(got, vec![Presentation::CameraBind { id: None }; 3]);
}

#[test]
fn light_restore_381_sets_the_memory() {
    let rules = camera_rules(&script(&[(&[], &["n381"])]), &[], &[]);
    let mut state = started(&rules);
    state.script.mission.presentation.light = Some("nacht".into());
    assert!(!state.script.mission.presentation.object_light_restored);
    let got = run_present(&mut state, &rules, 2);
    assert_eq!(got, vec![Presentation::LightRestoreLevel]);
    let memory = &state.script.mission.presentation;
    assert!(memory.object_light_restored);
    // The terrain light (171) stays.
    assert_eq!(memory.light.as_deref(), Some("nacht"));
}

/// Same state with and without camera actions (the camera changes no state).
#[test]
fn camera_events_leave_the_checksum() {
    let run_with = |camera: bool| {
        let mut actions = vec!["n25;t;5"];
        if camera {
            actions.extend([
                "n2;1;area feld",
                "n141;1;kam",
                "n142;1;kam;100",
                "n143",
                "n144",
                "n170;niemand",
                "n172;100",
                "n175;niemand",
                "n176",
                "n212;10;100",
                "n216;100",
            ]);
        }
        let rules = camera_rules(&script(&[(&[], &actions)]), &[("feld", [3, 4, 6, 9])], &[]);
        let mut state = started(&rules);
        let events = run_present(&mut state, &rules, 4);
        // The source text is part of the container; equalize it for the comparison.
        state.script.level.as_mut().unwrap().source.checksum = 0;
        (crate::save::checksum(&state), events.len())
    };
    let (with, with_events) = run_with(true);
    let (without, without_events) = run_with(false);
    assert!(with_events > 0, "the camera actions report events");
    assert_eq!(without_events, 0);
    assert_eq!(with, without);
}

/// Unknown names, cameras and areas in the presentation actions: no panic, no event for anything
/// unknown, the savegame loads.
#[test]
fn present_actions_survive_unknown_inputs() {
    let rules = camera_rules(
        &script(&[(
            &[],
            &[
                "n2;1;area nirgends",
                "n141;1;gibtsnicht",
                "n142;1;gibtsnicht;0",
                "n142;1;gibtsnicht;-5",
                "n170;unbekannt",
                "n172;0",
                "n175;unbekannt",
                "n177;unbekannt",
                "n212;0;0",
                "n212;360;-1",
                "n216;-3",
                "n220;1;kam;50;99999",
                "n222;50;99999",
            ],
        )]),
        &[],
        &[],
    );
    let mut state = started(&rules);
    let got = run_present(&mut state, &rules, 4);
    // No 2 and no 170 for anything unknown.
    assert!(!got.iter().any(|p| matches!(
        p,
        Presentation::CameraCenter { .. } | Presentation::CameraToObject { .. }
    )));
    // Unknown camera: the name goes to the host unchanged (it takes the origin).
    assert!(got.contains(&Presentation::CameraCut {
        camera: "gibtsnicht".into()
    }));
    assert!(got.contains(&Presentation::CameraBind { id: None }));
    // Times <= 0 report their event anyway (the host lets the move lapse or does not rotate).
    for expected in [
        Presentation::CameraMove {
            camera: "gibtsnicht".into(),
            ms: 0,
        },
        Presentation::CameraMove {
            camera: "gibtsnicht".into(),
            ms: -5,
        },
        Presentation::CameraToGame { ms: 0 },
        Presentation::CameraRotate { degrees: 0, ms: 0 },
        Presentation::CameraRotate {
            degrees: 360,
            ms: -1,
        },
        Presentation::CameraMoveToBackup { ms: -3 },
    ] {
        assert!(got.contains(&expected), "{expected:?} missing in {got:?}");
    }
    // The savegame loads.
    let loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(
        crate::save::checksum(&loaded),
        crate::save::checksum(&state)
    );

    // Scene part: unknown areas, names, slots and flags.
    let (mut state, rules, got) = scene_run(
        &[
            "n7;Player 1;area nirgends",
            "n149;1;2;5",
            "n149;1;2;-1",
            "n156;9",
            "n156;-3",
            "n152;unbekannt",
            "n155;unbekannt",
            "n169;12345",
            "n168;1;0;0;7",
            "n168;1;0;0;-7",
            "n183;1;2",
            "n183;1;-1",
            "n19",
            "n148",
            "n148",
            "n147",
            "n147",
            "n5",
            "n213;1;1;0;0",
            "n213;2;99;0;0",
        ],
        4,
    );
    // No ping for an unknown area, no marker for an unknown name, no tip on slot 2/-1.
    assert!(
        !got.iter()
            .any(|p| matches!(p, Presentation::MinimapPing { .. }))
    );
    assert!(
        !got.iter()
            .any(|p| matches!(p, Presentation::Highlight { id: Some(_) }))
    );
    assert!(!got.iter().any(|p| matches!(p, Presentation::Tip { .. })));
    // Slots outside report their event, the state ignores them.
    assert!(got.contains(&Presentation::Portrait {
        slot: 5,
        portrait: 1,
        frame: 2
    }));
    assert_eq!(memory(&state).portraits, [None; 4]);
    // A flag outside 1/2 gives style 0.
    assert!(got.contains(&Presentation::InfoFrame {
        text: 1,
        x: 0,
        y: 0,
        style: 0
    }));
    // Direct calls with values that the load check in the script text rejects (speech slot < 0, 213 pos < -4).
    {
        let mut sim = Sim::new(&mut state, &rules);
        super::speech::play_line(&mut sim, 5, -1);
        super::speech::play_line(&mut sim, 5, i32::MIN);
        super::speech::info_window(&mut sim, 6, -5, 0, 0);
        super::speech::info_window(&mut sim, 7, i32::MIN, 9, 9);
        super::speech::info_window(&mut sim, 8, i32::MAX, -1, -1);
    }
    // Runs without a panic; the savegame loads.
    run_present(&mut state, &rules, 400);
    let loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(
        crate::save::checksum(&loaded),
        crate::save::checksum(&state)
    );

    // Sound, light, weather and fog part: presets without a counterpart and numbers at the edge
    // stay raw script numbers; the host ignores anything unknown.
    let (state, rules, got) = scene_run(
        &[
            "n171;gibtsnicht",
            "n171;;x",
            "n209;-1",
            "n209;2147483647",
            "n210;-9",
            "n9;0",
            "n9;-1",
            "n370;2147483647",
            "n371;-5",
            "n507;-1",
            "n501",
            "n505;-2147483648;2147483647;-1",
            "n506;-1;0;0;0",
            "n506;2147483647;-2147483648;-2147483648;-2147483648",
            "n382;nirgends",
            "n383;nirgends",
            "n383;Ärger",
        ],
        4,
    );
    // The unknown light goes to the host unchanged (it logs and changes nothing); the empty name has no
    // effect.
    assert!(got.contains(&Presentation::Light {
        preset: "gibtsnicht".into()
    }));
    assert!(!got.contains(&Presentation::Light {
        preset: String::new()
    }));
    assert_eq!(memory(&state).light.as_deref(), Some("gibtsnicht"));
    assert_eq!(memory(&state).weather, 0);
    assert_eq!(memory(&state).music, Some(-1));
    assert!(got.contains(&Presentation::ProfileVar {
        name: "nirgends".into(),
        value: 0
    }));
    // The savegame loads (names lowercase, light name not empty).
    let loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.script.mission, state.script.mission);
    assert_eq!(
        crate::save::checksum(&loaded),
        crate::save::checksum(&state)
    );
}

// ---------------------------------------------------------------------------------------------
// Scene (fade, cinemascope, lock, portraits, lines, tips, info window, marker, ping)
// ---------------------------------------------------------------------------------------------

/// Speech lengths of the scene tests: text 300 speaks 4,000 ms, 301 1,000 ms, 302 200 ms, 303 400 ms;
/// without OGG the units count (text 310: 20 units, `tpc` 55 -> tip 1,650 ms).
const SCENE_SPEECH: [(i32, u32, u32); 5] = [
    (300, 4000, 20),
    (301, 1000, 20),
    (302, 200, 20),
    (303, 400, 20),
    (310, 0, 20),
];

fn scene_rules(level: &str) -> Rules {
    camera_rules(level, &[("feld", [3, 4, 6, 9])], &SCENE_SPEECH)
}

/// All events of the next `n` steps.
fn run_events(state: &mut SimState, rules: &Rules, n: usize) -> Vec<SimEvent> {
    let mut all = Vec::new();
    for _ in 0..n {
        let events = step(state, rules, &[]);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SimEvent::Script(ScriptTrace::Pending { .. }))),
            "{events:?}"
        );
        all.extend(events);
    }
    all
}

/// One block with the actions in order; `n` steps, the `Present` events.
fn scene_run(actions: &[&str], n: usize) -> (SimState, Rules, Vec<Presentation>) {
    let rules = scene_rules(&script(&[(&[], actions)]));
    let mut state = started(&rules);
    let got = run_present(&mut state, &rules, n);
    (state, rules, got)
}

fn memory(state: &SimState) -> &PresentationMemory {
    &state.script.mission.presentation
}

#[test]
fn ui_lock_18_19_sets_state_and_event() {
    let rules = scene_rules(&script(&[(&[], &["n18", "n19", "n18"])]));
    let mut state = started(&rules);
    state.script.mission.selection = vec![ObjectId(0x800), ObjectId(0x801)];
    let got = run_present(&mut state, &rules, 1);
    assert_eq!(
        got,
        vec![
            Presentation::UiLock { on: true },
            Presentation::UiLock { on: false },
            Presentation::UiLock { on: true },
        ]
    );
    assert!(state.script.mission.ui_locked);
    // Without cinemascope 18 clears the mirror (the command bar is visible).
    assert!(state.script.mission.selection.is_empty());
    // 19 unlocks.
    let (state, _, got) = scene_run(&["n18", "n19"], 1);
    assert_eq!(
        got,
        vec![
            Presentation::UiLock { on: true },
            Presentation::UiLock { on: false }
        ]
    );
    assert!(!state.script.mission.ui_locked);
    // 19 leaves the mirror alone.
    let rules = scene_rules(&script(&[(&[], &["n19"])]));
    let mut state = started(&rules);
    state.script.mission.selection = vec![ObjectId(0x800)];
    run_present(&mut state, &rules, 1);
    assert_eq!(state.script.mission.selection, vec![ObjectId(0x800)]);
}

#[test]
fn ui_lock_inside_cinemascope_keeps_the_selection() {
    // In cinemascope the command bar is off: 18 clears nothing. 147, wait of the portrait (600 ms, twelve steps)
    // and 18: during the wait the selection arrives via a host command.
    let rules = scene_rules(&script(&[(&[], &["n147", "w149;1;2;0", "n18"])]));
    let mut state = started(&rules);
    run_present(&mut state, &rules, 1);
    assert!(state.script.mission.cinemascope);
    state.script.mission.selection = vec![ObjectId(0x800), ObjectId(0x801)];
    let got = run_present(&mut state, &rules, 20);
    assert_eq!(got.last(), Some(&Presentation::UiLock { on: true }));
    assert!(state.script.mission.ui_locked);
    assert_eq!(
        state.script.mission.selection,
        vec![ObjectId(0x800), ObjectId(0x801)]
    );
}

#[test]
fn cinemascope_clears_selection_portraits_and_highlight() {
    for (action, on) in [("n147", true), ("n148", false)] {
        let rules = scene_rules(&script(&[(&[], &[action])]));
        let mut state = started(&rules);
        state.script.mission.selection = vec![ObjectId(0x800)];
        let m = &mut state.script.mission.presentation;
        m.portraits[0] = Some(PortraitMemory {
            portrait: 1,
            frame: 2,
        });
        m.portraits[3] = Some(PortraitMemory {
            portrait: 3,
            frame: 4,
        });
        m.highlight = Some(ObjectId(0x800));
        m.fog_show_all = Some(!on);
        let got = run_present(&mut state, &rules, 1);
        assert_eq!(got, vec![Presentation::Cinemascope { on }], "{action}");
        assert_eq!(state.script.mission.cinemascope, on);
        assert!(state.script.mission.selection.is_empty());
        let m = memory(&state);
        assert_eq!(m.portraits, [None; 4]);
        assert_eq!(m.highlight, None);
        // 147 shows the whole map (m_FogShowMode 7), 148 resets the fog.
        assert_eq!(m.fog_show_all, Some(on));
    }
    // 148 without 147 is allowed (the speed is up to the host, E-238): off stays off.
    let (state, _, got) = scene_run(&["n148", "n148"], 1);
    assert_eq!(got.len(), 2);
    assert!(!state.script.mission.cinemascope);
    // Double 147 reports twice.
    let (state, _, got) = scene_run(&["n147", "n147"], 1);
    assert_eq!(got, vec![Presentation::Cinemascope { on: true }; 2]);
    assert!(state.script.mission.cinemascope);
}

#[test]
fn fade_145_146_sets_faded_out() {
    let (state, _, got) = scene_run(&["n146;1200"], 1);
    assert_eq!(
        got,
        vec![Presentation::Fade {
            into: false,
            ms: 1200
        }]
    );
    assert!(memory(&state).faded_out);
    let (state, _, got) = scene_run(&["n146;1200", "n145;800"], 1);
    assert_eq!(
        got,
        vec![
            Presentation::Fade {
                into: false,
                ms: 1200
            },
            Presentation::Fade {
                into: true,
                ms: 800
            }
        ]
    );
    assert!(!memory(&state).faded_out);
    // Time 0 and negative: event anyway.
    let (state, _, got) = scene_run(&["n146;0", "n146;-5"], 1);
    assert_eq!(got.len(), 2);
    assert!(memory(&state).faded_out);
}

#[test]
fn portrait_149_ignores_slots_outside_0_to_3_but_emits() {
    // Order of the parameters `n149;portrait;frame;slot`: three distinguishable numbers.
    let (state, _, got) = scene_run(&["n149;11;22;3"], 1);
    assert_eq!(
        got,
        vec![Presentation::Portrait {
            slot: 3,
            portrait: 11,
            frame: 22
        }]
    );
    assert_eq!(
        memory(&state).portraits,
        [
            None,
            None,
            None,
            Some(PortraitMemory {
                portrait: 11,
                frame: 22
            })
        ]
    );
    // Slot 4, 5 and -1: event yes, state no.
    let (state, _, got) = scene_run(&["n149;1;2;4", "n149;3;4;5", "n149;5;6;-1"], 1);
    assert_eq!(got.len(), 3);
    assert_eq!(memory(&state).portraits, [None; 4]);
    // 156 hides, here too only slots 0 to 3 in the state.
    let (state, _, got) = scene_run(&["n149;1;2;0", "n149;3;4;1", "n156;0", "n156;7"], 1);
    assert_eq!(
        got[2..],
        [
            Presentation::PortraitHide { slot: 0 },
            Presentation::PortraitHide { slot: 7 }
        ]
    );
    assert_eq!(
        memory(&state).portraits,
        [
            None,
            Some(PortraitMemory {
                portrait: 3,
                frame: 4
            }),
            None,
            None
        ]
    );
    // A new portrait in the same slot replaces the old one.
    let (state, _, _) = scene_run(&["n149;1;2;2", "n149;8;9;2"], 1);
    assert_eq!(
        memory(&state).portraits[2],
        Some(PortraitMemory {
            portrait: 8,
            frame: 9
        })
    );
}

#[test]
fn highlight_152_takes_the_last_present_id_and_155_clears() {
    let rules = scene_rules(&script(&[(
        &[],
        &["n152;redner", "n152;fehlt", "n155;egal"],
    )]));
    let mut state = started(&rules);
    let (a, b);
    {
        let mut sim = Sim::new(&mut state, &rules);
        a = spawn(&mut sim, None, (2, 2));
        b = spawn(&mut sim, None, (3, 3));
        let c = spawn(&mut sim, None, (4, 4));
        name_list(&mut sim, "redner", &[a, b, c]);
        // The last ID (c) is destroyed: the last PRESENT ID (b) wins.
        crate::world::destroy_object(&mut sim, c);
    }
    // The mirror stays untouched (E-228: 152 selects nothing).
    state.script.mission.selection = vec![a];
    let got = run_present(&mut state, &rules, 1);
    // `n152;fehlt` (unknown name) reports nothing; 155 always reports.
    assert_eq!(
        got,
        vec![
            Presentation::Highlight { id: Some(b) },
            Presentation::Highlight { id: None },
        ]
    );
    assert_eq!(memory(&state).highlight, None);
    assert_eq!(state.script.mission.selection, vec![a]);

    // Without 155 the marker stays; an unknown name does not change it.
    let rules = scene_rules(&script(&[(&[], &["n152;redner", "n152;fehlt"])]));
    let mut state = started(&rules);
    let b;
    {
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, None, (2, 2));
        b = spawn(&mut sim, None, (3, 3));
        name_list(&mut sim, "redner", &[a, b]);
    }
    state.script.mission.selection = vec![b];
    let got = run_present(&mut state, &rules, 1);
    assert_eq!(got, vec![Presentation::Highlight { id: Some(b) }]);
    assert_eq!(memory(&state).highlight, Some(b));
    assert_eq!(state.script.mission.selection, vec![b]);
}

#[test]
fn speech_150_emits_speech_and_no_end_for_the_interrupted_line() {
    // E-240: the new line 301 interrupts 300; there is no end for 300.
    let (mut state, rules, got) = scene_run(&["n150;300;1", "n150;301;0"], 1);
    assert_eq!(
        got,
        vec![
            Presentation::Speech {
                text: 300,
                slot: 1,
                ms: 4000
            },
            Presentation::Speech {
                text: 301,
                slot: 0,
                ms: 1000
            },
        ]
    );
    // The end of the second line (1,000 ms = 20 steps, the first counts) comes on its own.
    assert!(run_present(&mut state, &rules, 18).is_empty());
    assert_eq!(
        run_present(&mut state, &rules, 1),
        vec![Presentation::SpeechEnd { text: 301, slot: 0 }]
    );
    assert!(run_present(&mut state, &rules, 100).is_empty());
}

#[test]
fn speech_end_after_the_length_and_213_closes_its_window_at_the_end() {
    // Line 302 (200 ms) on slot 1: exactly in the fourth step.
    let rules = scene_rules(&script(&[(&[], &["n150;302;1"])]));
    let mut state = started(&rules);
    assert_eq!(run_present(&mut state, &rules, 3).len(), 1);
    assert_eq!(
        run_present(&mut state, &rules, 1),
        vec![Presentation::SpeechEnd { text: 302, slot: 1 }]
    );

    // 213 at pos 0 (slot 4): raw pos in the state, closing at the speech end.
    let rules = scene_rules(&script(&[(&[], &["n213;302;0;5;6"])]));
    let mut state = started(&rules);
    let got = run_present(&mut state, &rules, 1);
    assert_eq!(
        got,
        vec![
            Presentation::InfoFrameClose { text: 302 },
            Presentation::Speech {
                text: 302,
                slot: 4,
                ms: 200
            },
            Presentation::InfoSpeechFrame {
                text: 302,
                slot: 0,
                portrait: 5,
                frame: 6
            },
        ]
    );
    assert_eq!(
        memory(&state).info_frames.get(&302),
        Some(&InfoFrameMemory::Speech {
            slot: 0,
            portrait: 5,
            frame: 6
        })
    );
    assert_eq!(
        run_present(&mut state, &rules, 3),
        vec![Presentation::InfoFrameClose { text: 302 }]
    );
    assert!(memory(&state).info_frames.is_empty());

    // Pos -1 (slot 3, a text slot): the window does NOT close at the speech end, there is SpeechEnd.
    let rules = scene_rules(&script(&[(&[], &["n213;302;-1;5;6"])]));
    let mut state = started(&rules);
    let got = run_present(&mut state, &rules, 1);
    assert_eq!(
        got[1],
        Presentation::Speech {
            text: 302,
            slot: 3,
            ms: 200
        }
    );
    assert_eq!(
        run_present(&mut state, &rules, 3),
        vec![Presentation::SpeechEnd { text: 302, slot: 3 }]
    );
    assert!(memory(&state).info_frames.contains_key(&302));

    // 150 on slot 4 (not a text slot) closes the window of the text ID at the end, also a 168 one.
    let rules = scene_rules(&script(&[(&[], &["n168;302;10;20;1", "n150;302;4"])]));
    let mut state = started(&rules);
    run_present(&mut state, &rules, 1);
    assert!(memory(&state).info_frames.contains_key(&302));
    assert_eq!(
        run_present(&mut state, &rules, 3),
        vec![Presentation::InfoFrameClose { text: 302 }]
    );
    assert!(memory(&state).info_frames.is_empty());
}

#[test]
fn info_window_213_closes_the_windows_of_its_own_and_the_running_text() {
    // If line 303 runs with a 168 window 303 and 213 arrives with text 301: first the windows of 301 and 303
    // close, then the line and the new window (`FUN_00443A20`).
    let rules = scene_rules(&script(&[(
        &[],
        &[
            "n168;301;1;2;0",
            "n168;303;3;4;2",
            "n150;303;1",
            "n213;301;1;7;8",
        ],
    )]));
    let mut state = started(&rules);
    let got = run_present(&mut state, &rules, 1);
    assert_eq!(
        got[3..],
        [
            Presentation::InfoFrameClose { text: 301 },
            Presentation::InfoFrameClose { text: 303 },
            Presentation::Speech {
                text: 301,
                slot: 5,
                ms: 1000
            },
            Presentation::InfoSpeechFrame {
                text: 301,
                slot: 1,
                portrait: 7,
                frame: 8
            },
        ]
    );
    let frames = &memory(&state).info_frames;
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames.get(&301),
        Some(&InfoFrameMemory::Speech {
            slot: 1,
            portrait: 7,
            frame: 8
        })
    );
}

#[test]
fn tip_183_slot_2_does_nothing() {
    // E-241: only slot 0 and 1; otherwise no event, no history, no state.
    let (state, rules, got) = scene_run(&["n183;310;2", "n183;310;-1", "n183;310;5"], 40);
    assert!(got.is_empty(), "{got:?}");
    assert!(state.script.mission.info_history.is_empty());
    let fresh = started(&rules);
    assert_eq!(state.script.mission.speech, fresh.script.mission.speech);
    // Slots 0 and 1 report Tip (1,650 ms) and TipEnd in the 33rd step.
    let (mut state, rules, got) = scene_run(&["n183;310;0", "n183;310;1"], 1);
    assert_eq!(
        got,
        vec![
            Presentation::Tip {
                text: 310,
                slot: 0,
                ms: 1650
            },
            Presentation::Tip {
                text: 310,
                slot: 1,
                ms: 1650
            },
        ]
    );
    assert!(run_present(&mut state, &rules, 31).is_empty());
    assert_eq!(
        run_present(&mut state, &rules, 1),
        vec![
            Presentation::TipEnd { text: 310, slot: 0 },
            Presentation::TipEnd { text: 310, slot: 1 },
        ]
    );
    // Expired stays expired.
    assert!(run_present(&mut state, &rules, 10).is_empty());
}

#[test]
fn second_tip_in_a_slot_never_reports_the_first() {
    // E-242: the second tip in slot 0 overwrites time and text; the first never reports "ended". Slot 1
    // stays untouched.
    let rules = scene_rules(&script(&[(&[], &["n183;310;0", "n183;302;1"])]));
    let mut state = started(&rules);
    run_present(&mut state, &rules, 1);
    {
        let mut sim = Sim::new(&mut state, &rules);
        super::speech::show_tip(&mut sim, 303, 0);
    }
    let all = run_present(&mut state, &rules, 60);
    let ends: Vec<_> = all
        .iter()
        .filter(|p| matches!(p, Presentation::TipEnd { .. }))
        .cloned()
        .collect();
    assert_eq!(
        ends.iter()
            .filter(|p| matches!(p, Presentation::TipEnd { slot: 0, .. }))
            .count(),
        1,
        "{ends:?}"
    );
    assert!(ends.contains(&Presentation::TipEnd { text: 303, slot: 0 }));
    assert!(!ends.contains(&Presentation::TipEnd { text: 310, slot: 0 }));
    assert!(ends.contains(&Presentation::TipEnd { text: 302, slot: 1 }));
}

#[test]
fn info_frames_168_169_one_per_text_id() {
    // E-249: one per text ID. 168 replaces the same ID, other windows stay, 169 closes exactly this ID.
    // The flag becomes style 0/1/2.
    let (state, _, got) = scene_run(
        &[
            "n168;5;10;20;0",
            "n168;6;30;40;1",
            "n168;7;50;60;2",
            "n168;8;70;80;9",
            "n168;5;11;21;2",
        ],
        1,
    );
    assert_eq!(
        got[3],
        Presentation::InfoFrame {
            text: 8,
            x: 70,
            y: 80,
            style: 0
        }
    );
    let frames = &memory(&state).info_frames;
    assert_eq!(frames.len(), 4);
    assert_eq!(
        frames.get(&5),
        Some(&InfoFrameMemory::Window {
            x: 11,
            y: 21,
            style: 2
        })
    );
    assert_eq!(
        frames.get(&6),
        Some(&InfoFrameMemory::Window {
            x: 30,
            y: 40,
            style: 1
        })
    );
    // 169 closes exactly the ID; 169 without an open window reports anyway and does not change the state.
    let (state, _, got) = scene_run(&["n168;5;1;2;0", "n168;6;3;4;0", "n169;5", "n169;99"], 1);
    assert_eq!(
        got[2..],
        [
            Presentation::InfoFrameClose { text: 5 },
            Presentation::InfoFrameClose { text: 99 }
        ]
    );
    let keys: Vec<i32> = memory(&state).info_frames.keys().copied().collect();
    assert_eq!(keys, vec![6]);
}

#[test]
fn info_frame_table_full_reports_limit_and_still_emits() {
    // With a full table the new window is not taken in (the savegame stays loadable), `Limit` is reported and
    // the event is sent anyway; an existing key can still be replaced.
    let rules = scene_rules(&script(&[(&[], &["n168;1;5;6;0", "n168;1000;7;8;1"])]));
    let mut state = started(&rules);
    for id in 1000..1000 + MAX_INFO_FRAMES as i32 {
        state.script.mission.presentation.info_frames.insert(
            id,
            InfoFrameMemory::Window {
                x: 0,
                y: 0,
                style: 0,
            },
        );
    }
    let events = run_events(&mut state, &rules, 1);
    let infos = events
        .iter()
        .filter(|e| matches!(e, SimEvent::Present(Presentation::InfoFrame { .. })))
        .count();
    assert_eq!(infos, 2);
    assert!(events.iter().any(|e| matches!(
        e,
        SimEvent::Script(ScriptTrace::Limit { what }) if what.contains("info window")
    )));
    let frames = &memory(&state).info_frames;
    assert_eq!(frames.len(), MAX_INFO_FRAMES);
    assert!(!frames.contains_key(&1));
    assert_eq!(
        frames.get(&1000),
        Some(&InfoFrameMemory::Window {
            x: 7,
            y: 8,
            style: 1
        })
    );
    // The state loads.
    let loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.script.mission, state.script.mission);
}

#[test]
fn ping_7_uses_the_area_center_and_unknown_area_sends_nothing() {
    // Area (3, 4)-(6, 9): center (4, 6); the player goes along unchanged (no filter).
    let (_, _, got) = scene_run(
        &[
            "n7;Player 2;area feld",
            "n7;Player 9;area feld",
            "n7;Player 20;area feld",
            "n7;Player -1;area feld",
        ],
        1,
    );
    assert_eq!(
        got,
        vec![
            Presentation::MinimapPing {
                player: 2,
                tile_x: 4,
                tile_z: 6
            },
            Presentation::MinimapPing {
                player: 9,
                tile_x: 4,
                tile_z: 6
            },
            Presentation::MinimapPing {
                player: 20,
                tile_x: 4,
                tile_z: 6
            },
            Presentation::MinimapPing {
                player: -1,
                tile_x: 4,
                tile_z: 6
            },
        ]
    );
    let (_, _, got) = scene_run(&["n7;Player 1;area nirgends"], 1);
    assert!(got.is_empty());
}

#[test]
fn screen_text_5_emits_seconds() {
    // `n5;text;(unread);seconds`; missing values count as 0.
    let (_, _, got) = scene_run(&["n5;77;99;6", "n5;12", "n5"], 1);
    assert_eq!(
        got,
        vec![
            Presentation::ScreenText {
                text: 77,
                seconds: 6
            },
            Presentation::ScreenText {
                text: 12,
                seconds: 0
            },
            Presentation::ScreenText {
                text: 0,
                seconds: 0
            },
        ]
    );
}

#[test]
fn scene_actions_leave_the_mirror_alone_except_where_documented() {
    // 5, 7, 145, 146, 149, 156, 168, 169, 150, 183, 213, 152, 155: the selection mirror stays.
    let rules = scene_rules(&script(&[(
        &[],
        &[
            "n5;1;2;3",
            "n7;Player 1;area feld",
            "n145;100",
            "n146;100",
            "n149;1;2;0",
            "n156;0",
            "n168;1;2;3;0",
            "n169;1",
            "n150;302;1",
            "n183;310;1",
            "n213;302;0;1;2",
            "n152;nirgends",
            "n155;nirgends",
        ],
    )]));
    let mut state = started(&rules);
    state.script.mission.selection = vec![ObjectId(0x800), ObjectId(0x801)];
    run_present(&mut state, &rules, 10);
    assert_eq!(
        state.script.mission.selection,
        vec![ObjectId(0x800), ObjectId(0x801)]
    );
}

#[test]
fn save_load_restores_the_presentation_memory() {
    // Loading in the middle of the cutscene. 18, 147, 149, 168, 152, 213, 146; saving after the block,
    // loading: same state and same checksum sequence over 200 steps.
    let rules = scene_rules(&script(&[(
        &[],
        &[
            "n18",
            "n147",
            "n149;11;22;2",
            "n168;5;10;20;2",
            "n152;redner",
            "n213;300;0;3;4",
            "n501",
            "n506;2000;5192239;-4000;4750",
            "n9;100",
            "n370;115",
            "n371;1000",
            "n171;nacht",
            "n209;2",
            "n146;1500",
        ],
    )]));
    let mut state = started(&rules);
    let redner;
    {
        let mut sim = Sim::new(&mut state, &rules);
        redner = spawn(&mut sim, Some("redner"), (3, 3));
    }
    state.script.mission.selection = vec![redner];
    run_present(&mut state, &rules, 2);
    let m = &state.script.mission;
    assert!(m.ui_locked && m.cinemascope);
    assert!(m.selection.is_empty());
    let p = &m.presentation;
    assert_eq!(p.highlight, Some(redner));
    assert!(p.faded_out);
    assert_eq!(p.fog_show_all, Some(true));
    assert_eq!(
        p.portraits[2],
        Some(PortraitMemory {
            portrait: 11,
            frame: 22
        })
    );
    assert_eq!(p.info_frames.len(), 2);
    assert!(state.script.mission.speech.line.is_some());
    // Fog, music, light and weather are in the state.
    assert_eq!(p.distance_fog, Some(true));
    assert_eq!(
        p.fog,
        Some(FogMemory {
            color: 5192239,
            start: -4000,
            end: 4750,
            swap_red_blue: true,
        })
    );
    assert_eq!(
        (p.music, p.attack_music, p.attack_threshold),
        (Some(100), Some(115), Some(1000))
    );
    assert_eq!((p.light.as_deref(), p.weather), (Some("nacht"), 2));

    let mut loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.script.mission, state.script.mission);
    assert_eq!(
        crate::save::checksum(&loaded),
        crate::save::checksum(&state)
    );
    // The same sequence over 200 steps, including the speech end (the window of 300 closes).
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for _ in 0..200 {
        let ea = step(&mut state, &rules, &[]);
        let eb = step(&mut loaded, &rules, &[]);
        assert_eq!(ea, eb);
        a.push(crate::save::checksum(&state));
        b.push(crate::save::checksum(&loaded));
    }
    assert_eq!(a, b);
    assert!(state.script.mission.speech.line.is_none());
    assert!(
        !memory(&state).info_frames.contains_key(&300),
        "the 213 window closed at the speech end"
    );
    assert!(memory(&state).info_frames.contains_key(&5));
}

#[test]
fn silent_presentation_actions_are_handled_not_pending() {
    // 2 and 7 with an unknown area, 152 and 170 without a present object end silently but count as handled
    // (`Dispatched::Done`): no `Pending` trace, no event.
    let rules = scene_rules(&script(&[(
        &[],
        &[
            "n2;1;area nirgends",
            "n7;Player 1;area nirgends",
            "n152;niemand",
            "n170;niemand",
        ],
    )]));
    let mut state = started(&rules);
    let events = run_events(&mut state, &rules, 2);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            SimEvent::Present(_) | SimEvent::Script(ScriptTrace::Pending { .. })
        )),
        "{events:?}"
    );
    // The block ran through completely and was deleted.
    assert!(state.script.level.as_ref().unwrap().blocks.is_empty());
}

// ---------------------------------------------------------------------------------------------
// Sound, light, weather, fog, quicksave, profile variables
// ---------------------------------------------------------------------------------------------

/// E-251: The D case of 370 (`0x004256A0`) acts only with exactly one value (`msg[1] == 1`). Layer E
/// always writes `{370, 1, Z0}` for 370 (`0x0056C526`), so from a script exactly one value always arrives -
/// even with further parameters; a message with a different word count is done without effect. 371
/// does not check the word count.
#[test]
fn attack_music_370_needs_exactly_one_value() {
    use super::actions::{Dispatched, MsgValue, ScriptMsg, dispatch};
    // From the script: additional parameters change nothing, layer E sends one value.
    let (state, _, got) = scene_run(&["n370;115;7"], 2);
    assert_eq!(got, vec![Presentation::AttackMusic { id: 115 }]);
    assert_eq!(memory(&state).attack_music, Some(115));

    let rules = scene_rules(&script(&[(&[], &["n12;a"])]));
    let mut state = started(&rules);
    let mut c = state.script.level.take().expect("level container");
    let events_before = state.events.len();
    {
        let mut sim = Sim::new(&mut state, &rules);
        for values in [
            vec![],
            vec![MsgValue::Int(115), MsgValue::Int(7)],
            vec![MsgValue::Int(115), MsgValue::Int(7), MsgValue::Int(1)],
        ] {
            let mut msg = ScriptMsg {
                number: 370,
                values,
            };
            assert_eq!(dispatch(&mut sim, &mut c, &mut msg), Dispatched::Done);
        }
        // Counter-check: exactly one value acts; 371 also acts with two values.
        let mut one = ScriptMsg {
            number: 370,
            values: vec![MsgValue::Int(116)],
        };
        assert_eq!(dispatch(&mut sim, &mut c, &mut one), Dispatched::Done);
        let mut threshold = ScriptMsg {
            number: 371,
            values: vec![MsgValue::Int(900), MsgValue::Int(7)],
        };
        assert_eq!(dispatch(&mut sim, &mut c, &mut threshold), Dispatched::Done);
    }
    let presents: Vec<_> = state.events[events_before..]
        .iter()
        .filter_map(|e| match e {
            SimEvent::Present(p) => Some(p.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        presents,
        vec![
            Presentation::AttackMusic { id: 116 },
            Presentation::AttackThreshold { points: 900 },
        ]
    );
    assert_eq!(memory(&state).attack_music, Some(116));
    assert_eq!(memory(&state).attack_threshold, Some(900));
}

#[test]
fn sound_actions_emit_and_remember() {
    let (state, _, got) = scene_run(&["n9;100", "n370;115", "n371;1000", "n507;4711"], 2);
    assert_eq!(
        got,
        vec![
            Presentation::Music { id: 100 },
            Presentation::AttackMusic { id: 115 },
            Presentation::AttackThreshold { points: 1000 },
            Presentation::Sound2d { id: 4711 },
        ]
    );
    let m = memory(&state);
    assert_eq!(m.music, Some(100));
    assert_eq!(m.attack_music, Some(115));
    assert_eq!(m.attack_threshold, Some(1000));
    // 507 is a single sound without state: otherwise the state stays untouched.
    assert_eq!(
        *m,
        PresentationMemory {
            music: Some(100),
            attack_music: Some(115),
            attack_threshold: Some(1000),
            ..PresentationMemory::default()
        }
    );
    // A later 9 overwrites the state; 507 alone changes no state.
    let (state, _, got) = scene_run(&["n9;100", "n507;1", "n9;119"], 2);
    assert_eq!(got.len(), 3);
    assert_eq!(memory(&state).music, Some(119));
    assert_eq!(memory(&state).attack_music, None);
}

#[test]
fn light_171_sets_the_preset_and_clears_restored() {
    // 171 takes the (lowercased) name, remembers it and clears "object light restored" of 381.
    let (state, _, got) = scene_run(&["n381", "n171;Nacht", "n171;Tag"], 2);
    assert_eq!(
        got,
        vec![
            Presentation::LightRestoreLevel,
            Presentation::Light {
                preset: "nacht".into()
            },
            Presentation::Light {
                preset: "tag".into()
            },
        ]
    );
    let m = memory(&state);
    assert_eq!(m.light.as_deref(), Some("tag"));
    assert!(!m.object_light_restored);
    // 381 after 171 sets "restored", the light stays.
    let (state, _, _) = scene_run(&["n171;Nacht", "n381"], 2);
    assert!(memory(&state).object_light_restored);
    assert_eq!(memory(&state).light.as_deref(), Some("nacht"));
    // An empty name: no state, no event, the state stays loadable.
    let (state, rules, got) = scene_run(&["n171;;x"], 2);
    assert!(got.is_empty(), "{got:?}");
    assert_eq!(memory(&state).light, None);
    let loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.script.mission, state.script.mission);
}

#[test]
fn weather_209_210() {
    let (state, _, got) = scene_run(&["n209;3"], 2);
    assert_eq!(got, vec![Presentation::Weather { kind: 3 }]);
    assert_eq!(memory(&state).weather, 3);
    // 210 ends the weather and does not read the parameter.
    let (state, _, got) = scene_run(&["n209;3", "n210;7"], 2);
    assert_eq!(
        got,
        vec![
            Presentation::Weather { kind: 3 },
            Presentation::Weather { kind: 0 }
        ]
    );
    assert_eq!(memory(&state).weather, 0);
}

#[test]
fn fog_505_is_immediate_and_506_swaps_red_and_blue() {
    // 505 `color; start; end`: immediately, color unchanged (E-237).
    let (state, _, got) = scene_run(&["n505;5192239;-4000;4750"], 2);
    assert_eq!(
        got,
        vec![Presentation::FogValues {
            color: 5192239,
            start: -4000,
            end: 4750,
            ms: 0,
            swap_red_blue: false,
        }]
    );
    assert_eq!(
        memory(&state).fog,
        Some(FogMemory {
            color: 5192239,
            start: -4000,
            end: 4750,
            swap_red_blue: false,
        })
    );
    // 506 `ms; color; start; end`: crossfade (ms > 0) swaps red and blue; the state carries the target
    // values.
    let (state, _, got) = scene_run(&["n506;2000;5192239;100;9000"], 2);
    assert_eq!(
        got,
        vec![Presentation::FogValues {
            color: 5192239,
            start: 100,
            end: 9000,
            ms: 2000,
            swap_red_blue: true,
        }]
    );
    assert_eq!(
        memory(&state).fog,
        Some(FogMemory {
            color: 5192239,
            start: 100,
            end: 9000,
            swap_red_blue: true,
        })
    );
    // 506 with ms 0 (or less) takes the color unchanged, without a swap.
    for ms in ["0", "-5"] {
        let line = format!("n506;{ms};5192239;100;9000");
        let (state, _, got) = scene_run(&[line.as_str()], 2);
        assert_eq!(got.len(), 1);
        assert!(matches!(
            got[0],
            Presentation::FogValues {
                swap_red_blue: false,
                ..
            }
        ));
        assert!(!memory(&state).fog.unwrap().swap_red_blue);
    }
    // A later 505 replaces the state (and clears the swap).
    let (state, _, _) = scene_run(&["n506;2000;1;2;3", "n505;4;5;6"], 2);
    assert_eq!(
        memory(&state).fog,
        Some(FogMemory {
            color: 4,
            start: 5,
            end: 6,
            swap_red_blue: false,
        })
    );
    // 501/502: distance fog on/off.
    let (state, _, got) = scene_run(&["n501", "n502", "n501"], 2);
    assert_eq!(
        got,
        vec![
            Presentation::DistanceFog { on: true },
            Presentation::DistanceFog { on: false },
            Presentation::DistanceFog { on: true },
        ]
    );
    assert_eq!(memory(&state).distance_fog, Some(true));
}

#[test]
fn fog_show_503_504() {
    // 503 = off, 504 = on; the state `fog_show_all` shares the channel with 147/148.
    let (state, _, got) = scene_run(&["n504"], 2);
    assert_eq!(got, vec![Presentation::FogShowAll { on: true }]);
    assert_eq!(memory(&state).fog_show_all, Some(true));
    let (state, _, got) = scene_run(&["n504", "n503"], 2);
    assert_eq!(
        got,
        vec![
            Presentation::FogShowAll { on: true },
            Presentation::FogShowAll { on: false }
        ]
    );
    assert_eq!(memory(&state).fog_show_all, Some(false));
}

#[test]
fn quick_save_508_only_in_the_campaign() {
    let rules = scene_rules(&script(&[(&[], &["n508"])]));
    // Campaign (`mode == -1`): event, no state.
    let mut state = started(&rules);
    assert_eq!(state.mode, -1);
    let before = state.script.mission.clone();
    let got = run_present(&mut state, &rules, 2);
    assert_eq!(got, vec![Presentation::QuickSave]);
    assert_eq!(state.script.mission, before);
    // Skirmish and multiplayer: nothing.
    for mode in [0, 1, 3] {
        let mut s = setup(true);
        s.mode = mode;
        let mut state = new_match(&rules, &s).expect("match");
        let got = run_present(&mut state, &rules, 2);
        assert!(got.is_empty(), "mode {mode}: {got:?}");
        assert!(state.script.level.as_ref().unwrap().blocks.is_empty());
    }
}

#[test]
fn profile_382_writes_mirror_and_event_383_reads_it() {
    let rules = scene_rules(&script(&[(
        &[],
        &[
            // 383: value from the mirror (start from `ScriptSetup`), missing -> 0; `Var[p0]` set afterwards.
            "n383;Lager",
            "n383;fehlt",
            // 382: value from `Var[p0]` into the mirror, with an event.
            "n360;wert;9",
            "n382;wert",
            // 382 on a never-set variable writes 0 (E-181), 383 reads it back.
            "n382;nie",
            "n383;wert",
        ],
    )]));
    let mut s = setup(true);
    s.script.profile_vars.insert("lager".to_owned(), 7);
    s.script.profile_vars.insert("unberuehrt".to_owned(), 3);
    let mut state = new_match(&rules, &s).expect("match");
    let got = run_present(&mut state, &rules, 2);
    assert_eq!(
        got,
        vec![
            Presentation::ProfileVar {
                name: "wert".into(),
                value: 9
            },
            Presentation::ProfileVar {
                name: "nie".into(),
                value: 0
            },
        ]
    );
    let vars = &state.script.level.as_ref().unwrap().variables;
    assert_eq!(vars.get("lager"), Some(&7));
    assert_eq!(vars.get("fehlt"), Some(&0));
    assert_eq!(vars.get("wert"), Some(&9));
    assert_eq!(vars.get("nie"), Some(&0));
    let profile = &state.script.mission.profile_vars;
    assert_eq!(profile.get("wert"), Some(&9));
    assert_eq!(profile.get("nie"), Some(&0));
    assert_eq!(profile.get("lager"), Some(&7));
    assert_eq!(profile.get("unberuehrt"), Some(&3));
    // 383 does not write the mirror.
    assert_eq!(profile.get("fehlt"), None);
    // The state loads.
    let loaded = crate::save::testkit::save_and_load(&state, &rules);
    assert_eq!(loaded.script.mission, state.script.mission);
}

#[test]
fn profile_383_overwrites_the_variable_with_the_mirror() {
    // An already set variable is overwritten too (`Var[p0] = msg[2]`).
    let rules = scene_rules(&script(&[(&[], &["n360;a;5", "n383;a"])]));
    let mut s = setup(true);
    s.script.profile_vars.insert("a".to_owned(), 2);
    let mut state = new_match(&rules, &s).expect("match");
    run_present(&mut state, &rules, 2);
    assert_eq!(state.script.level.as_ref().unwrap().variables["a"], 2);
    // Without an entry in the mirror it becomes 0.
    let mut state = started(&rules);
    run_present(&mut state, &rules, 2);
    assert_eq!(state.script.level.as_ref().unwrap().variables["a"], 0);
}
