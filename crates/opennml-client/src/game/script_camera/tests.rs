//! Tests of the camera of the scripts (E-232 ... E-235). GPU-free over flat test terrain
//! (`input::testkit`, 64 x 64 tiles of 128, ground 0, so target height 80) with the test host `plain_testkit`
//! (cell size 128); `camera_rests_in_pause` needs the installation and a GPU (otherwise silently skipped).

use super::*;
use crate::game::input::testkit;
use crate::game::sim_host::plain_testkit;

/// Frame time of the tests in game ms.
const DT: f32 = 100.0;

/// Cameras of the `.wld` as in the shipped data (name not lowercase).
fn cameras() -> Vec<NamedCamera> {
    vec![
        NamedCamera {
            name: "Start Camera".into(),
            from: [1000.0, 3000.0, 1200.0],
            at: [2000.0, 500.0, 2400.0],
            roll: 0.3,
        },
        NamedCamera {
            name: "End".into(),
            from: [6000.0, 4000.0, 5000.0],
            at: [6500.0, 0.0, 5800.0],
            roll: 0.0,
        },
    ]
}

struct World {
    camera: GameCamera,
    terrain: Terrain,
    sim: SimHost,
    cameras: Vec<NamedCamera>,
    script: ScriptCamera,
}

impl World {
    fn new() -> Self {
        let (camera, terrain) = testkit::setup();
        Self {
            camera,
            terrain,
            sim: plain_testkit::host(),
            cameras: cameras(),
            script: ScriptCamera::default(),
        }
    }

    fn apply(&mut self, event: Presentation) {
        self.script.apply(
            &event,
            &mut self.camera,
            &self.cameras,
            &self.terrain,
            &self.sim,
        );
    }

    fn frame(&mut self, dt: f32) {
        self.script
            .update(dt, &mut self.camera, &self.terrain, &self.sim);
    }

    fn pose(&self) -> (Vec3, Vec3) {
        (self.camera.eye, self.camera.target)
    }

    /// Target point like `Camera_SetTarget` (x/z clamped, y stays, D-254).
    fn ground(&self, at: [f32; 3]) -> Vec3 {
        GameCamera::clamp_target(at.into(), &self.terrain)
    }
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a - b).abs().max_element() < 0.05
}

fn assert_close(a: Vec3, b: Vec3, what: &str) {
    assert!(close(a, b), "{what}: {a} vs {b}");
}

fn move_to(camera: &str, ms: i32) -> Presentation {
    Presentation::CameraMove {
        camera: camera.into(),
        ms,
    }
}

// QUIRK E-232, E-235: kept
#[test]
fn cut_141_applies_on_the_next_frame_and_keeps_a_running_move() {
    // E-232 (`FUN_0042EB50`, `0x00432590` step 1): 141 remembers the cut; the next frame applies only it (move,
    // rotation, binding rest, its time lapses), afterwards the move continues relatively.
    let mut w = World::new();
    // The script name comes lowercase from the parser, the `.wld` writes it capitalized (`0x0044280F` lowercases when
    // loading).
    w.apply(move_to("start camera", 1000));
    w.frame(DT);
    assert_eq!(w.script.rest_ms, 900.0);
    let speed = Vec3::from(w.script.eye_speed);
    assert!(speed != Vec3::ZERO);
    let before = w.pose();
    w.apply(Presentation::CameraCut {
        camera: "end".into(),
    });
    assert_eq!(w.pose(), before, "141 has no immediate effect");
    assert!(w.script.running());
    w.frame(DT);
    let ende = &cameras()[1];
    assert_eq!(w.camera.eye, Vec3::from(ende.from), "eye unchecked");
    assert_eq!(w.camera.target, w.ground(ende.at), "target as passed");
    assert_eq!(w.camera.target.y, 0.0, "no raise to ground + 80 (D-254)");
    assert_eq!(w.script.rest_ms, 900.0, "the move rests in this frame");
    assert!(w.script.pending_cut.is_none());
    // The move continues from the cut at the old speed (does not end at the target of the move).
    w.frame(DT);
    assert_eq!(w.script.rest_ms, 800.0);
    assert_close(w.camera.eye, Vec3::from(ende.from) + speed * DT, "relative");
    for _ in 0..8 {
        w.frame(DT);
    }
    assert!(!w.script.running());
    let start = &cameras()[0];
    assert!(
        !close(w.camera.eye, Vec3::from(start.from)),
        "the move does not end at its target"
    );
}

// QUIRK E-233: kept
#[test]
fn moves_chain_and_pass_the_rest_on() {
    // E-233: moves are appended; the rest time of a frame passes exactly to the next.
    let mut w = World::new();
    let target = w.camera.target;
    // The first move keeps the target (otherwise the rest of the target speed offsets the final eye, as in the
    // original: step 6 calculates from the already shifted target).
    w.cameras.push(NamedCamera {
        name: "Stop".into(),
        from: [3000.0, 6000.0, 3000.0],
        at: target.to_array(),
        roll: 0.0,
    });
    w.apply(move_to("stop", 150));
    w.apply(move_to("end", 300));
    assert_eq!(w.script.moves.len(), 2);
    w.frame(DT);
    assert_eq!(w.script.rest_ms, 50.0);
    assert_eq!(w.script.moves.len(), 1);
    // Frame 2: 50 ms for the first move (it ends exactly), 50 ms already for the second.
    w.frame(DT);
    assert_eq!(w.script.rest_ms, 250.0, "rest time passed on");
    assert!(w.script.moves.is_empty());
    w.frame(DT);
    w.frame(DT);
    assert!(w.script.running());
    w.frame(DT);
    assert!(!w.script.running(), "450 ms in five frames");
    let ende = &cameras()[1];
    assert_close(w.camera.eye, Vec3::from(ende.from), "eye at the target");
    assert_close(w.camera.target, w.ground(ende.at), "target at the target");
    // After that nothing moves any more.
    let pose = w.pose();
    w.frame(DT);
    assert_eq!(w.pose(), pose);
}

// QUIRK E-233: kept
#[test]
fn a_chained_move_keeps_the_target_rest_of_the_previous_one() {
    // E-233 (`0x00432590` steps 5/6): if a move ends in the middle of a frame, step 5 shifts the target by its rest,
    // step 6 calculates the eye speed of the next move from this shifted target + offset, but step 8 sets the eye to
    // the old target + offset: the final eye of the next move lies `m_AtSpeed * rest` beside its target.
    let mut w = World::new();
    w.apply(move_to("end", 150));
    w.apply(move_to("start camera", 300));
    w.frame(DT);
    let rest = w.script.rest_ms;
    let target_speed = Vec3::from(w.script.target_speed);
    assert_eq!(rest, 50.0);
    assert!(target_speed != Vec3::ZERO);
    for _ in 0..4 {
        w.frame(DT);
    }
    assert!(!w.script.running());
    let start = &cameras()[0];
    assert_close(
        w.camera.eye,
        Vec3::from(start.from) - target_speed * rest,
        "final eye offset",
    );
    assert_close(w.camera.target, w.ground(start.at), "target at the target");
}

// QUIRK E-233: kept
#[test]
fn a_move_shorter_than_a_frame_is_dropped() {
    // E-233: a move with time <= frame time (also 0 and negative) lapses without effect; only one move is taken per
    // frame, the rest of the frame lapses with it.
    let mut w = World::new();
    let start = w.pose();
    for ms in [100, 0, -5] {
        w.apply(move_to("end", ms));
    }
    w.apply(move_to("start camera", 200));
    for left in [3, 2, 1] {
        w.frame(DT);
        assert_eq!(w.pose(), start, "no jump");
        assert_eq!(w.script.moves.len(), left);
        assert!(w.script.eye_speed.iter().all(|v| v.is_finite()));
    }
    w.frame(DT);
    assert_eq!(w.script.rest_ms, 100.0, "full frame time for the next move");
    assert!(w.pose() != start);
    w.frame(DT);
    assert_close(w.camera.eye, Vec3::from(cameras()[0].from), "eye");
    assert!(!w.script.running());
}

// QUIRK E-233: kept
#[test]
fn to_game_172_freezes_the_target_at_the_event() {
    // E-233 (`FUN_00433FC0`): 172 takes the camera target at the call, not at the start of the move, and resets
    // nothing.
    let mut w = World::new();
    let target = w.camera.target;
    let id = w.sim.state.objects().next().unwrap().id;
    // The first move keeps the target (otherwise its target rest offsets the final eye of the second, see
    // `a_chained_move_keeps_the_target_rest_of_the_previous_one`).
    w.cameras.push(NamedCamera {
        name: "Away".into(),
        from: [3000.0, 6000.0, 3000.0],
        at: target.to_array(),
        roll: 0.0,
    });
    w.apply(Presentation::CameraBind { id: Some(id) });
    w.apply(move_to("away", 300));
    w.apply(Presentation::CameraToGame { ms: 300 });
    let leg = w.script.moves[1];
    assert_eq!(leg.at, target.to_array());
    assert_eq!(leg.from, (target + w.camera.setup.offset).to_array());
    assert_eq!(w.script.bind, Some(id), "172 does not release the binding");
    w.apply(Presentation::CameraBind { id: None });
    for _ in 0..7 {
        w.frame(DT);
    }
    assert!(!w.script.running());
    assert_close(
        w.camera.target,
        w.ground(target.to_array()),
        "target at the call",
    );
    assert_close(
        w.camera.eye,
        target + w.camera.setup.offset,
        "eye = target + CAMERA_OFFSET",
    );
}

// QUIRK E-233: kept
// QUIRK E-250: deviation
#[test]
fn move_to_backup_216_uses_the_backup_at_the_event() {
    // E-233 (`FUN_004310E0`, `FUN_0042DB70`, `FUN_00430B20`): 143 remembers eye and target, 216 copies them into the
    // move at the call, 144 resets at once.
    let mut w = World::new();
    // Without 143: origin (start value of the constructor, hypothesis 0).
    w.apply(Presentation::CameraMoveToBackup { ms: 200 });
    assert_eq!(
        w.script.moves[0],
        MoveLeg {
            from: [0.0; 3],
            at: [0.0; 3],
            ms: 200,
            player: false,
        }
    );
    w.script.moves.clear();
    let (eye, target) = w.pose();
    w.apply(Presentation::CameraBackup);
    assert_eq!(w.script.backup, Some((eye.to_array(), target.to_array())));
    w.apply(Presentation::CameraMoveToBackup { ms: 200 });
    // The camera moves, a new 143 does not change the move already queued.
    w.camera.move_to(1000.0, 1500.0, &w.terrain);
    w.apply(Presentation::CameraBackup);
    for _ in 0..2 {
        w.frame(DT);
    }
    assert!(!w.script.running());
    assert_close(w.camera.eye, eye, "eye of the backup at the call");
    assert_close(w.camera.target, target, "target of the backup at the call");
    // 144 applies the second backup at once.
    w.apply(Presentation::CameraRestoreBackup);
    let (b_eye, b_target) = w.script.backup.unwrap();
    assert_eq!(w.camera.eye, Vec3::from(b_eye));
    assert_eq!(w.camera.target, w.ground(b_target));
}

// QUIRK E-234: kept
#[test]
fn bind_175_moves_the_target_not_the_eye_and_ends_when_the_object_is_gone() {
    // E-234 (`FUN_0042DDA0`, `0x00432590` step 3): the target follows the object each frame, the eye stays; if the
    // object is missing, the binding is finally released.
    let mut w = World::new();
    let id = w.sim.state.objects().next().unwrap().id;
    let position = w.sim.interpolated(w.sim.state.object(id, false).unwrap());
    let eye = w.camera.eye;
    w.apply(Presentation::CameraBind { id: Some(id) });
    assert!(
        !w.script.running(),
        "a binding alone does not lock the player"
    );
    w.frame(DT);
    assert_eq!(w.camera.eye, eye, "the eye stays put");
    assert_eq!(w.camera.target, w.ground(position.to_array()));
    w.frame(DT);
    assert_eq!(w.camera.eye, eye);
    // The object disappears from the world: the binding lapses, the camera stays.
    let rules = w.sim.rules.clone();
    opennml_sim::world::destroy_object(
        &mut opennml_sim::sim::Sim::new(&mut w.sim.state, &rules),
        id,
    );
    let pose = w.pose();
    w.frame(DT);
    assert_eq!(w.script.bind, None);
    assert_eq!(w.pose(), pose);
    // 176-178 (and 175 with an unknown name) release.
    let other = w.sim.state.objects().next().unwrap().id;
    w.apply(Presentation::CameraBind { id: Some(other) });
    assert_eq!(w.script.bind, Some(other));
    w.apply(Presentation::CameraBind { id: None });
    assert_eq!(w.script.bind, None);
}

// QUIRK E-233: kept
#[test]
fn rotate_212_is_relative_in_degrees_and_may_overshoot_one_frame() {
    // E-233 (`FUN_0042DEE0`): rate (degrees * pi_f) / (ms * 180) per game ms, rotation of the offset eye - target
    // around the vertical axis through the target with x' = x*cos - z*sin, z' = x*sin + z*cos; every frame rotates the
    // full frame time, also the last (overshoot).
    let mut w = World::new();
    let (eye, target) = w.pose();
    let offset = eye - target;
    w.apply(Presentation::CameraRotate {
        degrees: 90,
        ms: 250,
    });
    let rate = w.script.rotation.unwrap().rad_per_ms;
    assert!((rate - std::f32::consts::PI / 2.0 / 250.0).abs() < 1e-9);
    for _ in 0..3 {
        w.frame(DT);
    }
    assert!(w.script.rotation.is_none(), "finished after three frames");
    // 300 ms instead of 250: 108 degrees instead of 90.
    let angle = 108f32.to_radians();
    let (s, c) = angle.sin_cos();
    let expected = Vec3::new(
        offset.x * c - offset.z * s,
        offset.y,
        offset.x * s + offset.z * c,
    );
    assert_eq!(w.camera.target, target, "the target stays");
    assert_close(w.camera.eye - target, expected, "offset rotated");
    // Relative: the same rotation once more rotates by another 108 degrees.
    let turned = w.camera.eye - target;
    w.apply(Presentation::CameraRotate {
        degrees: 90,
        ms: 250,
    });
    for _ in 0..3 {
        w.frame(DT);
    }
    let again = Vec3::new(
        turned.x * c - turned.z * s,
        turned.y,
        turned.x * s + turned.z * c,
    );
    assert_close(w.camera.eye - target, again, "relative");
    // A new 212 replaces the running rotation; ms <= 0 ends it.
    w.apply(Presentation::CameraRotate {
        degrees: 45,
        ms: 1000,
    });
    w.apply(Presentation::CameraRotate { degrees: 45, ms: 0 });
    assert!(w.script.rotation.is_none());
    w.apply(Presentation::CameraRotate {
        degrees: -30,
        ms: -10,
    });
    assert!(w.script.rotation.is_none());
}

// QUIRK E-234: kept
#[test]
fn center_2_and_170_jump_at_once() {
    // Actions 2 and 170 (re/script-actions, E-234): target (x, 0, z) at ground height, eye =
    // (x, 0, z) + CAMERA_OFFSET, at once; move, binding and rotation stay.
    let mut w = World::new();
    let id = w.sim.state.objects().next().unwrap().id;
    w.apply(move_to("end", 1000));
    w.apply(Presentation::CameraBind { id: Some(id) });
    w.apply(Presentation::CameraCenter {
        tile_x: 10,
        tile_z: 20,
    });
    let (x, z) = (1280.0, 2560.0);
    assert_eq!(w.camera.target, w.ground([x, 0.0, z]));
    assert_eq!(w.camera.eye, Vec3::new(x, 0.0, z) + w.camera.setup.offset);
    // 170: simulation position (grid position * tile size), once.
    let object = w.sim.state.object(id, false).unwrap();
    let (x, z) = (
        object.grid.x.to_f32() * 128.0,
        object.grid.z.to_f32() * 128.0,
    );
    w.apply(Presentation::CameraToObject { id });
    assert_eq!(w.camera.target, w.ground([x, 0.0, z]));
    assert_eq!(w.camera.eye, Vec3::new(x, 0.0, z) + w.camera.setup.offset);
    assert_eq!(w.script.moves.len(), 1, "the move keeps waiting");
    assert_eq!(w.script.bind, Some(id), "the binding stays");
}

// QUIRK E-232, E-235: kept
// QUIRK E-250: deviation
#[test]
fn script_camera_handles_unknown_cameras() {
    // E-232: an unknown camera cuts or moves to the origin (eye (0, 0, 0), target map corner at height 0, D-254);
    // unknown objects, a missing backup and an empty camera list without a panic.
    let mut w = World::new();
    w.cameras.clear();
    w.apply(Presentation::CameraCut {
        camera: "does not exist".into(),
    });
    w.frame(DT);
    assert_eq!(w.camera.eye, Vec3::ZERO);
    assert_eq!(w.camera.target, Vec3::ZERO);
    w.camera.move_to(4096.0, 4096.0, &w.terrain);
    w.apply(move_to("", 200));
    assert_eq!(
        w.script.moves[0],
        MoveLeg {
            from: [0.0; 3],
            at: [0.0; 3],
            ms: 200,
            player: false,
        }
    );
    for _ in 0..2 {
        w.frame(DT);
    }
    assert_close(w.camera.eye, Vec3::ZERO, "move to the origin");
    let pose = w.pose();
    // 170 with an object that does not exist: nothing; 175 with it: the binding lapses in the next frame.
    let ghost = ObjectId(0x7777);
    w.apply(Presentation::CameraToObject { id: ghost });
    assert_eq!(w.pose(), pose);
    w.apply(Presentation::CameraBind { id: Some(ghost) });
    w.frame(DT);
    assert_eq!(w.script.bind, None);
    assert_eq!(w.pose(), pose);
    // 144 without 143: origin; a range far outside the map clamps the target.
    w.apply(Presentation::CameraRestoreBackup);
    assert_eq!(w.camera.target, Vec3::ZERO);
    w.apply(Presentation::CameraCenter {
        tile_x: i32::MAX,
        tile_z: i32::MIN,
    });
    assert!(w.camera.target.is_finite());
    w.apply(Presentation::CameraRotate {
        degrees: i32::MAX,
        ms: 1 << 30,
    });
    w.frame(DT);
    assert!(w.camera.eye.is_finite());
}

#[test]
fn appending_stops_at_max_moves() {
    let mut w = World::new();
    // Appending stops at MAX_MOVES (the list does not grow without limit).
    w.script = ScriptCamera {
        moves: std::iter::repeat_n(
            MoveLeg {
                from: [0.0; 3],
                at: [0.0; 3],
                ms: 1,
                player: false,
            },
            MAX_MOVES,
        )
        .collect(),
        ..ScriptCamera::default()
    };
    w.apply(Presentation::CameraToGame { ms: 10 });
    assert_eq!(w.script.moves.len(), MAX_MOVES);
}

// QUIRK E-232: kept
#[test]
fn camera_rests_in_pause() {
    // Time basis is the game time of the frame; in the pause (`GameSession::advance` without `running`) no frame
    // runs, the camera of the scripts rests together with the open cut.
    use crate::game::options::ViewOptions;
    use crate::game_session::{GameSession, SessionGpu};
    use opennml_sim::setup::MatchSetup;
    let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let Ok((device, queue, _)) =
        opennml_render::headless_device(opennml_render::BackendChoice::Auto)
    else {
        eprintln!("no GPU, test skipped");
        return;
    };
    let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
    let gpu = SessionGpu {
        device: &device,
        queue: &queue,
        format: opennml_render::CAPTURE_FORMAT,
    };
    let mut session = GameSession::start(
        &gpu,
        &vfs,
        "data/levels/kampagne1/kamp1-2/kamp1-2.lvl",
        &mut |rules| {
            let mut setup = MatchSetup::default_for(rules);
            setup.script.enabled = false;
            Ok(setup)
        },
        10,
        &ViewOptions::default(),
        (320, 200),
        &mut |_| {},
    )
    .unwrap();
    session.advance(&gpu, &vfs, 1_000, true, None);
    let pose = (session.map.camera.eye, session.map.camera.target);
    let script = &mut session.presentation.camera;
    script.moves.push_back(MoveLeg {
        from: [1000.0, 5000.0, 1000.0],
        at: [1500.0, 0.0, 1500.0],
        ms: 2000,
        player: false,
    });
    script.rotation = Some(CameraRotation {
        remaining_ms: 500.0,
        rad_per_ms: 0.001,
    });
    script.pending_cut = Some(([2000.0, 6000.0, 2000.0], [2500.0, 0.0, 2500.0]));
    let saved = session.presentation.camera.clone();
    for now in [1_100, 1_500, 3_000] {
        session.advance(&gpu, &vfs, now, false, None);
        assert_eq!(session.presentation.camera, saved, "rests in the pause");
        assert_eq!((session.map.camera.eye, session.map.camera.target), pose);
    }
    // Continue: the first frame after the pause applies the cut, the second moves and rotates.
    session.advance(&gpu, &vfs, 3_050, true, None);
    assert_eq!(session.map.camera.eye, Vec3::new(2000.0, 6000.0, 2000.0));
    assert_eq!(session.presentation.camera.moves.len(), 1);
    session.advance(&gpu, &vfs, 3_100, true, None);
    assert!(session.presentation.camera.moves.is_empty());
    assert_eq!(session.presentation.camera.rest_ms, 1950.0);
}

// QUIRK O-447: kept
#[test]
fn held_time_turns_the_script_camera_at_once() {
    // O-447 (re/menu-level ML.15 no. 4): after the stopped menu level (`GameSession::hold`) the camera of the scripts
    // gets the whole stopped time at once in the first running frame (`GameEngine_UpdateCameraFollow` `0x00432590`
    // with the real time since the last `GameEngine_Update`); the rotation is not clamped to its rest time and
    // overshoots. The ticks stay capped at 2. An ordinary pause (`advance` without `running`, game menu) catches up on
    // nothing.
    use crate::game::options::ViewOptions;
    use crate::game_session::{GameSession, SessionGpu};
    use opennml_sim::setup::MatchSetup;
    let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let Ok((device, queue, _)) =
        opennml_render::headless_device(opennml_render::BackendChoice::Auto)
    else {
        eprintln!("no GPU, test skipped");
        return;
    };
    let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
    let gpu = SessionGpu {
        device: &device,
        queue: &queue,
        format: opennml_render::CAPTURE_FORMAT,
    };
    let rotate = |o: Vec3, angle: f32| {
        let (s, c) = angle.sin_cos();
        Vec3::new(o.x * c - o.z * s, o.y, o.x * s + o.z * c)
    };
    for hold in [true, false] {
        let mut session = GameSession::start(
            &gpu,
            &vfs,
            "data/levels/kampagne1/kamp1-2/kamp1-2.lvl",
            &mut |rules| {
                let mut setup = MatchSetup::default_for(rules);
                setup.script.enabled = false;
                Ok(setup)
            },
            10,
            &ViewOptions::default(),
            (320, 200),
            &mut |_| {},
        )
        .unwrap();
        session.advance(&gpu, &vfs, 1_000, true, None);
        // Speed 10: game time = frame time.
        session.presentation.camera.rotation = Some(CameraRotation {
            remaining_ms: 500.0,
            rad_per_ms: 0.001,
        });
        session.advance(&gpu, &vfs, 1_050, true, None);
        let target = session.map.camera.target;
        let offset = session.map.camera.eye - target;
        let saved = session.presentation.camera.clone();
        for now in [1_100, 5_000, 9_000] {
            if hold {
                session.hold(&gpu, &vfs, now, None);
            } else {
                session.advance(&gpu, &vfs, now, false, None);
            }
            assert_eq!(session.ticks, 0, "stopped without ticks");
            assert_eq!(
                session.presentation.camera, saved,
                "rests as long as stopped"
            );
            assert_eq!(session.map.camera.eye - target, offset);
        }
        let steps = session.steps();
        session.advance(&gpu, &vfs, 9_050, true, None);
        assert!(session.ticks <= 2, "ticks capped: {}", session.ticks);
        assert_eq!(session.steps() - steps, u64::from(session.ticks));
        assert_eq!(session.map.camera.target, target, "the target stays");
        let turned = session.map.camera.eye - target;
        if hold {
            // 7950 ms stopped + 50 ms frame time at once: 8 rad instead of the rest time 450 ms (overshoot).
            assert_close(turned, rotate(offset, 8.0), "pause time caught up");
            assert!(
                session.presentation.camera.rotation.is_none(),
                "rotation finished"
            );
        } else {
            assert_close(turned, rotate(offset, 0.05), "only the frame time");
            assert_eq!(
                session.presentation.camera.rotation.map(|r| r.remaining_ms),
                Some(400.0)
            );
        }
        // The stopped time counts exactly once.
        let after = session.map.camera.eye;
        session.advance(&gpu, &vfs, 9_100, true, None);
        if hold {
            assert_eq!(session.map.camera.eye, after, "nothing more to catch up");
        }
    }
}
