//! Camera control without winit types (testable): original camera as in `GameEngine_UpdateCamera`
//! (`0x0043C7C0`) and a free debug camera.

use glam::{Mat4, Vec2, Vec3};
use opennml_world::camera::{GameCamera, game_projection};
use opennml_world::terrain::Terrain;

use crate::game::options::Controls;

/// Interval of the stepped keys (zoom, pitch). The original applies them **per frame**; at today's frame
/// rates that would be far too fast. Assumption: 50 steps/s (~ frame rate of 2003).
pub const STEP_MS: f32 = 20.0;
/// Duration of the smooth transition for mouse wheel zoom: the same move of 250 game ms as Home (`PUSH 0xFA` at
/// `0x0043D0F2` -> `FUN_00433890` -> `0x004335F0`, linear in the eye point with a fixed target).
pub const WHEEL_MS: f32 = crate::game::script_camera::PLAYER_MOVE_MS as f32;
/// Distance to the screen edge from which scrolling starts (pixels).
pub const EDGE_PIXELS: f64 = 3.0;

/// Where the camera keys of the map view come from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CameraKeys {
    /// As in the original (`opennml-game`): the `KD_` keys from `keymap.ini` via the table of held keys
    /// (`GameEngine_UpdateCamera` `0x0043C7C0`): `KD_CAM_MOVE_*` scroll, `KD_CAM_ZOOM_IN/OUT` zoom,
    /// `KD_CAM_YROT_*` rotate only without `lock_camera` (`LockCamera 0`, `0x0043CDCB`); no tilting by key
    /// (`KD_CAM_XROT_*` are missing from the file, O-305); `KD_CHECK_LBUTTON` held stops the camera.
    Original { lock_camera: bool },
    /// Keys of the viewer (default): arrows, Page Down/Up, Home/End tilt, Insert/Delete rotate.
    #[default]
    Viewer,
}

/// Inputs of the original camera in a frame.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CameraInput {
    /// Keys: x = forward (+) / back (-), y = right (+) / left (-).
    pub keys: Vec2,
    /// Screen edge, same axes.
    pub edge: Vec2,
    /// -1 closer, +1 farther.
    pub zoom: i8,
    /// +1 steeper, -1 flatter.
    pub pitch: i8,
    /// +1 / -1 rotation about the vertical axis.
    pub yaw: i8,
    /// `KD_CHECK_LBUTTON` held (`0x0043C7EA`...`0x0043C809`, jump to `0x0043D108`): no scrolling, zooming,
    /// tilting or rotating, and the accelerations `+0x340…+0x354` stand still (no decay). A running transition of
    /// the mouse wheel (in the original in the move list) keeps running.
    pub halt: bool,
    /// `KD_CAM_FREEHAND` (middle button) held: mouse movement of this frame in whole pixels (position - position of
    /// the previous frame, `GE+0x252C/+0x2530`); `None` if the button is not held. The block
    /// `0x0043CF1D`...`0x0043D095` uses it to rotate and tilt at `LockCamera 0` and to move at `LockCamera 1`
    /// ([`CameraController`]).
    pub freehand: Option<Vec2>,
}

/// Factor of the free mouse rotation per pixel (`0x00618EC8` = pi/300 rad), times `MouseSpeed` of the camera.
pub const FREEHAND_RAD_PER_PIXEL: f32 = std::f32::consts::PI / 300.0;
/// Factor of the free movement per pixel (`0x00615718` = 0.5) for `Camera_Move`.
pub const FREEHAND_MOVE_PER_PIXEL: f32 = 0.5;

/// Direction of edge scrolling for a mouse pointer in a window of `size`.
pub fn edge_direction(cursor: (f64, f64), size: (u32, u32)) -> Vec2 {
    let (x, y) = cursor;
    // A minimized window has size 0x0 under Windows; there is no edge there.
    if size.0 == 0 || size.1 == 0 {
        return Vec2::ZERO;
    }
    let (w, h) = (size.0 as f64, size.1 as f64);
    let mut direction = Vec2::ZERO;
    if y <= EDGE_PIXELS {
        direction.x += 1.0;
    }
    if y >= h - 1.0 - EDGE_PIXELS {
        direction.x -= 1.0;
    }
    if x <= EDGE_PIXELS {
        direction.y -= 1.0;
    }
    if x >= w - 1.0 - EDGE_PIXELS {
        direction.y += 1.0;
    }
    direction
}

struct WheelZoom {
    from: f32,
    to: f32,
    elapsed: f32,
}

/// Control of the original camera.
#[derive(Default)]
pub struct CameraController {
    /// Acceleration per axis (forward, right), `±0.1` at most.
    accel: Vec2,
    step_time: f32,
    wheel: Option<WheelZoom>,
}

/// Moves `value` toward `target` by at most `step`.
fn approach(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

impl CameraController {
    /// One frame of `dt_ms` milliseconds.
    pub fn update(
        &mut self,
        camera: &mut GameCamera,
        terrain: &Terrain,
        input: CameraInput,
        controls: &Controls,
        dt_ms: f32,
    ) {
        if !input.halt {
            self.keys_and_edge(camera, terrain, input, controls, dt_ms);
        }
        if let Some(wheel) = &mut self.wheel {
            wheel.elapsed += dt_ms;
            let t = (wheel.elapsed / WHEEL_MS).min(1.0);
            camera.set_distance(wheel.from + (wheel.to - wheel.from) * t);
            if t >= 1.0 {
                self.wheel = None;
            }
        }
    }

    /// One frame while paused (O-317): keys and edge as in [`CameraController::update`]
    /// (`GameEngine_UpdateCamera` also runs with `Engine+0x24E4`), whereas a running transition of the mouse wheel
    /// stands still - in the original it lies in the move list, which runs only without a pause
    /// (`FUN_00444C10`/`FUN_00432B90` in `GameEngine_Update` `0x00445650`), and continues after the pause.
    pub fn update_paused(
        &mut self,
        camera: &mut GameCamera,
        terrain: &Terrain,
        input: CameraInput,
        controls: &Controls,
        dt_ms: f32,
    ) {
        if !input.halt {
            self.keys_and_edge(camera, terrain, input, controls, dt_ms);
        }
    }

    /// Keys and screen edge of one frame (the block `0x0043C81C`...`0x0043CF1D`).
    fn keys_and_edge(
        &mut self,
        camera: &mut GameCamera,
        terrain: &Terrain,
        input: CameraInput,
        controls: &Controls,
        dt_ms: f32,
    ) {
        // Scrolling: acceleration rises by dt * 0.0003 up to +/-0.1 and decays the same way;
        // value = s * a * dt with s = (scroll + 1) * 0.1.
        let edge = if controls.enable_mouse_scroll {
            input.edge
        } else {
            Vec2::ZERO
        };
        let (direction, setting) = if input.keys != Vec2::ZERO {
            (input.keys, controls.keyboard_scroll)
        } else {
            (edge, controls.mouse_scroll)
        };
        let target = direction.clamp(Vec2::splat(-1.0), Vec2::ONE) * 0.1;
        let step = dt_ms * 0.0003;
        self.accel = Vec2::new(
            approach(self.accel.x, target.x, step),
            approach(self.accel.y, target.y, step),
        );
        if self.accel != Vec2::ZERO {
            let speed = (setting + 1) as f32 * 0.1;
            camera.scroll(
                speed * self.accel.x * dt_ms,
                speed * self.accel.y * dt_ms,
                terrain,
            );
        }
        // Zoom and pitch keys in fixed steps.
        self.step_time += dt_ms;
        while self.step_time >= STEP_MS {
            self.step_time -= STEP_MS;
            if input.zoom != 0 {
                camera.zoom_step(input.zoom < 0);
            }
            if input.pitch != 0 {
                camera.pitch(0.0314 * input.pitch as f32);
            }
        }
        // Rotating only without LockCamera: +/-dt * 0.001 rad per frame.
        if !controls.lock_camera && input.yaw != 0 {
            camera.yaw(input.yaw as f32 * dt_ms * 0.001);
        }
        // `KD_CAM_FREEHAND` after the keys (`0x0043CF1D`...`0x0043D025`, re/renderer, section "Movement in the game"),
        // delta = position - position of the previous frame: without LockCamera `Camera_Yaw(−Δx · π/300 · MouseSpeed)`,
        // then `Camera_Pitch(Δy · π/300 · MouseSpeed)`; with LockCamera `Camera_Move(−Δx · 0.5, −Δy · 0.5)` = to the
        // right dx * 0.5, forward -dy * 0.5 (mouse up pushes forward).
        if let Some(delta) = input.freehand {
            if controls.lock_camera {
                camera.scroll(
                    -delta.y * FREEHAND_MOVE_PER_PIXEL,
                    delta.x * FREEHAND_MOVE_PER_PIXEL,
                    terrain,
                );
            } else {
                // Order of computation as in `FILD`, `FMUL [0x00618EC8]`, `FMUL [Camera+0x74]`.
                let speed = camera.setup.mouse_speed;
                camera.yaw(-delta.x * FREEHAND_RAD_PER_PIXEL * speed);
                camera.pitch(delta.y * FREEHAND_RAD_PER_PIXEL * speed);
            }
        }
    }

    /// Aborts a running transition of the mouse wheel (Home, [`crate::game_session::GameSession::set_camera_home`]):
    /// in the original both wait one after the other in the same move list, so the home move ends in the home
    /// position; two simultaneous transitions would not hit it in the reimplementation.
    pub fn stop_wheel(&mut self) {
        self.wheel = None;
    }

    /// Mouse wheel: smooth transition to the new distance (`d · 0.76` or `d · 1.36`).
    pub fn wheel(&mut self, camera: &GameCamera, closer: bool, controls: &Controls) {
        if controls.enable_zoom {
            self.wheel = Some(WheelZoom {
                from: camera.distance(),
                to: camera.wheel_zoom_target(closer),
                elapsed: 0.0,
            });
        }
    }
}

/// Free debug camera (F2): WASD, Q/E, look with the right mouse button held.
#[derive(Debug, Clone, PartialEq)]
pub struct FreeCamera {
    pub position: Vec3,
    /// Rotation about the vertical axis (0 = looking toward +z).
    pub yaw: f32,
    /// Pitch (negative = downward).
    pub pitch: f32,
}

/// Inputs of the free camera.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct FreeInput {
    /// x = forward, y = right, z = up.
    pub movement: Vec3,
    /// Mouse movement in pixels (only with the right button held).
    pub look: Vec2,
    pub fast: bool,
}

impl FreeCamera {
    /// Takes over the eye and view direction of the game camera.
    pub fn from_game(camera: &GameCamera) -> Self {
        let dir = (camera.target - camera.eye).normalize_or_zero();
        Self {
            position: camera.eye,
            yaw: dir.x.atan2(dir.z),
            pitch: dir.y.asin(),
        }
    }

    pub fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        )
    }

    pub fn update(&mut self, input: FreeInput, dt_ms: f32) {
        self.yaw += input.look.x * 0.004;
        self.pitch = (self.pitch - input.look.y * 0.004).clamp(-1.55, 1.55);
        let forward = self.direction();
        let right = Vec3::Y.cross(forward).normalize_or_zero();
        let speed = if input.fast { 20.0 } else { 4.0 } * dt_ms;
        self.position +=
            (forward * input.movement.x + right * input.movement.y + Vec3::Y * input.movement.z)
                * speed;
    }

    pub fn view(&self) -> Mat4 {
        opennml_world::camera::look_at_lh(self.position, self.position + self.direction(), Vec3::Y)
    }

    /// Projection like the game, but with a fixed near distance (10) and a wide field of view (1 rad).
    pub fn projection(&self, aspect_factor: f32) -> Mat4 {
        game_projection(1.0, aspect_factor, 160.0)
    }
}

/// Test camera over flat terrain, also for the viewer's tests.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit {
    use super::*;
    use opennml_formats::prj::Project;
    use opennml_formats::testutil::flat_heightmap;
    use opennml_formats::tlm::TextureMap;
    use opennml_formats::wld::TerrainParams;
    use opennml_world::camera::CameraSetup;

    /// Camera over a flat 64 x 64 terrain (also for the tests in
    /// `apps/opennml-viewer/src/app.rs`).
    pub fn setup() -> (GameCamera, Terrain) {
        let params = TerrainParams {
            dim_x: 64,
            dim_z: 64,
            chunk_size: 16,
            ..TerrainParams::default()
        };
        let map = TextureMap {
            dim_x: 64,
            dim_z: 64,
            guids: vec![1; 64 * 64],
        };
        let terrain = Terrain::new(
            &params,
            &flat_heightmap(64, 64, 128.0, 0.0),
            &map,
            &Project::default(),
        )
        .unwrap();
        let setup = CameraSetup {
            min_zoom: 11000.0,
            min_pitch_deg: 20.0,
            max_pitch_deg: 80.0,
            ..CameraSetup::default()
        };
        let mut camera = GameCamera::new(setup, &terrain);
        camera.move_to(4096.0, 4096.0, &terrain);
        (camera, terrain)
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;

    #[test]
    fn edge_scrolling_uses_three_pixels() {
        assert_eq!(
            edge_direction((400.0, 0.0), (800, 600)),
            Vec2::new(1.0, 0.0)
        );
        assert_eq!(
            edge_direction((799.0, 597.0), (800, 600)),
            Vec2::new(-1.0, 1.0)
        );
        assert_eq!(edge_direction((400.0, 300.0), (800, 600)), Vec2::ZERO);
        assert_eq!(edge_direction((4.0, 300.0), (800, 600)), Vec2::ZERO);
    }

    #[test]
    fn a_zero_sized_window_has_no_edge() {
        assert_eq!(edge_direction((600.0, 400.0), (0, 0)), Vec2::ZERO);
        assert_eq!(edge_direction((0.0, 0.0), (0, 600)), Vec2::ZERO);
        assert_eq!(edge_direction((799.0, 0.0), (800, 0)), Vec2::ZERO);
    }

    #[test]
    fn scrolling_accelerates_and_stops_smoothly() {
        let (mut camera, terrain) = setup();
        let mut controller = CameraController::default();
        let controls = Controls::default();
        let start = camera.target;
        let input = CameraInput {
            keys: Vec2::new(1.0, 0.0),
            ..Default::default()
        };
        controller.update(&mut camera, &terrain, input, &controls, 100.0);
        let first = (camera.target - start).length();
        assert!((controller.accel.x - 0.03).abs() < 1e-6);
        for _ in 0..10 {
            controller.update(&mut camera, &terrain, input, &controls, 100.0);
        }
        assert!(
            (controller.accel.x - 0.1).abs() < 1e-6,
            "full acceleration after 333 ms"
        );
        assert!((camera.target - start).length() > first * 10.0);
        controller.update(
            &mut camera,
            &terrain,
            CameraInput::default(),
            &controls,
            100.0,
        );
        assert!(
            (controller.accel.x - 0.07).abs() < 1e-6,
            "decay like buildup"
        );
    }

    #[test]
    fn zoom_and_pitch_keys_step_every_20_ms() {
        let (mut camera, terrain) = setup();
        let mut controller = CameraController::default();
        let d = camera.distance();
        let input = CameraInput {
            zoom: -1,
            ..Default::default()
        };
        controller.update(&mut camera, &terrain, input, &Controls::default(), 45.0);
        assert!((camera.distance() - d * 0.988 * 0.988).abs() < 1.0);
        let pitch = camera.pitch_deg();
        let input = CameraInput {
            pitch: 1,
            ..Default::default()
        };
        controller.update(&mut camera, &terrain, input, &Controls::default(), 15.0); // 5 + 15 ms
        assert!((camera.pitch_deg() - pitch - 1.8).abs() < 0.05);
    }

    #[test]
    fn yaw_needs_unlocked_camera_and_wheel_zooms_smoothly() {
        let (mut camera, terrain) = setup();
        let mut controller = CameraController::default();
        let eye = camera.eye;
        let input = CameraInput {
            yaw: 1,
            ..Default::default()
        };
        controller.update(&mut camera, &terrain, input, &Controls::default(), 16.0);
        assert_eq!(camera.eye, eye);
        let unlocked = Controls {
            lock_camera: false,
            ..Controls::default()
        };
        controller.update(&mut camera, &terrain, input, &unlocked, 16.0);
        assert_ne!(camera.eye, eye);
        let target = camera.wheel_zoom_target(false);
        controller.wheel(&camera, false, &unlocked);
        controller.update(
            &mut camera,
            &terrain,
            CameraInput::default(),
            &unlocked,
            125.0,
        );
        assert!(camera.distance() < target);
        controller.update(
            &mut camera,
            &terrain,
            CameraInput::default(),
            &unlocked,
            125.0,
        );
        assert!((camera.distance() - target).abs() < 1.0);
    }

    /// Quirk O-331: `KD_CAM_FREEHAND` rotates at `LockCamera 0` by -dx * pi/300 * MouseSpeed (with the factor 1.5 of
    /// `Camera_Yaw`) and tilts by dy * pi/300 * MouseSpeed (steeper downward); at `LockCamera 1` it moves by
    /// delta * 0.5 (mouse right moves right, up moves forward), distance and pitch stay. Also while paused, not with
    /// the left button held.
    // QUIRK O-331: kept
    #[test]
    fn freehand_turns_and_tilts_unlocked_and_moves_locked() {
        let (mut camera, terrain) = setup();
        camera.setup.mouse_speed = 0.8;
        let unlocked = Controls {
            lock_camera: false,
            ..Controls::default()
        };
        let freehand = |x: f32, y: f32| CameraInput {
            freehand: Some(Vec2::new(x, y)),
            ..Default::default()
        };
        let mut controller = CameraController::default();
        let angle = |c: &GameCamera| {
            let o = c.eye - c.target;
            o.z.atan2(o.x)
        };
        let (yaw0, pitch0, d0) = (angle(&camera), camera.pitch_deg(), camera.distance());
        controller.update(&mut camera, &terrain, freehand(-10.0, 0.0), &unlocked, 16.0);
        let turned = angle(&camera) - yaw0;
        assert!(
            (turned - 10.0 * FREEHAND_RAD_PER_PIXEL * 0.8 * 1.5).abs() < 1e-4,
            "{turned}"
        );
        assert!((camera.pitch_deg() - pitch0).abs() < 1e-3 && (camera.distance() - d0).abs() < 1.0);
        controller.update_paused(&mut camera, &terrain, freehand(0.0, 5.0), &unlocked, 16.0);
        let tilted = camera.pitch_deg() - pitch0;
        assert!(
            (tilted - (5.0 * FREEHAND_RAD_PER_PIXEL * 0.8).to_degrees()).abs() < 1e-2,
            "{tilted}"
        );
        // Beyond MaxPitch (80 degrees) the eye stays put (`Camera_IsValidEyePosition`).
        let eye = camera.eye;
        controller.update(&mut camera, &terrain, freehand(0.0, 100.0), &unlocked, 16.0);
        assert_eq!(camera.eye, eye);

        let (mut camera, terrain) = setup();
        let locked = Controls::default();
        assert!(locked.lock_camera);
        let (start, eye_offset) = (camera.target, camera.eye - camera.target);
        controller.update(&mut camera, &terrain, freehand(20.0, -10.0), &locked, 16.0);
        let moved = camera.target - start;
        let step = eye_offset.length() * 0.0015;
        let on_screen = camera.view().transform_vector3(moved);
        assert!(on_screen.x > 0.0, "to the right: {on_screen:?}");
        let forward = -Vec3::new(eye_offset.x, 0.0, eye_offset.z).normalize();
        assert!(
            (moved.dot(forward) - 10.0 * 0.5 * step).abs() < 0.5,
            "forward"
        );
        assert!((moved.length() - (20.0f32.hypot(10.0) * 0.5 * step)).abs() < 0.5);
        assert!((camera.eye - camera.target - eye_offset).length() < 1e-2);

        // A held left button also stops the free mouse.
        let before = camera.clone();
        let held = CameraInput {
            halt: true,
            ..freehand(50.0, 50.0)
        };
        controller.update(&mut camera, &terrain, held, &locked, 16.0);
        assert_eq!(camera, before);
    }

    #[test]
    fn huge_frame_time_is_capped_to_a_100_ms_step() {
        use crate::clock::{MAX_FRAME_MS, frame_time_ms};
        use std::time::Duration;
        assert_eq!(frame_time_ms(Duration::from_millis(16)), 16.0);
        assert_eq!(frame_time_ms(Duration::from_secs(60)), MAX_FRAME_MS);
        let controls = Controls::default();
        let input = CameraInput {
            edge: Vec2::new(1.0, 0.0),
            ..Default::default()
        };
        let (mut capped, terrain) = setup();
        let (mut reference, _) = setup();
        let start = capped.target;
        CameraController::default().update(
            &mut capped,
            &terrain,
            input,
            &controls,
            frame_time_ms(Duration::from_secs(60)),
        );
        CameraController::default().update(&mut reference, &terrain, input, &controls, 100.0);
        assert_eq!(capped.target, reference.target);
        assert!((capped.target - start).length() < 100.0);
    }

    #[test]
    fn free_camera_starts_like_the_game_camera() {
        let (camera, _) = setup();
        let free = FreeCamera::from_game(&camera);
        let dir = (camera.target - camera.eye).normalize();
        assert!((free.direction() - dir).length() < 1e-4);
        let mut moved = free.clone();
        moved.update(
            FreeInput {
                movement: Vec3::X,
                ..Default::default()
            },
            10.0,
        );
        assert!(((moved.position - free.position).normalize() - dir).length() < 1e-4);
    }
}
