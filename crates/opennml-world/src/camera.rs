//! Game camera of the original (`Camera_*` in `NML.exe`, re/renderer camera section):
//! settings from `data/gifcon/camerasetup.ini` and `camera.ini`, movements with the
//! constants of the game, view and projection matrix in the original coordinate system
//! (left-handed, Direct3D depth 0..1). wgpu uses the same normalization as Direct3D
//! (x right, y up, z 0..1), so the matrices apply unchanged.

use glam::{Mat4, Vec2, Vec3, Vec4};
use opennml_formats::FormatError;
use opennml_formats::text::{decode_cp1252, parse_blocks};
use opennml_formats::wld::FogParams;
use opennml_rda::Vfs;

use crate::level::{LevelScene, ObjectClass};
use crate::terrain::Terrain;

/// `CAMERA_OFFSET` of the shipped data (`data/gifcon/camera.ini`); default if the file is missing.
pub const DEFAULT_CAMERA_OFFSET: Vec3 = Vec3::new(-10026.259, 11488.535, -10381.374);

/// Camera settings (`Camera_Construct` `0x0040B530`, built-in defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct CameraSetup {
    pub min_zoom: f32,
    pub max_zoom: f32,
    pub min_pitch_deg: f32,
    pub max_pitch_deg: f32,
    /// `MouseSpeed * 0.001`.
    pub mouse_speed: f32,
    /// `ZoomSpeed * 0.001`.
    pub zoom_speed: f32,
    /// Field of view in radians (`Fov`, default 0.1).
    pub fov: f32,
    /// `AspectRatio` (default 1.0; factor for `m00`).
    pub aspect_ratio: f32,
    /// Fog start without `[FOG]`: `BackClip - Fogstart` (`Camera+0x7C`, `0x0040BB94`).
    pub default_fog_start: f32,
    /// `CAMERA_OFFSET`: eye relative to the target at level start.
    pub offset: Vec3,
}

impl Default for CameraSetup {
    fn default() -> Self {
        Self {
            min_zoom: 1000.0,
            max_zoom: 30000.0,
            min_pitch_deg: 0.0,
            max_pitch_deg: 90.0,
            mouse_speed: 1.0,
            zoom_speed: 1.0,
            fov: 0.1,
            aspect_ratio: 1.0,
            default_fog_start: -100.0,
            offset: DEFAULT_CAMERA_OFFSET,
        }
    }
}

impl CameraSetup {
    /// Reads `camerasetup.ini` (keys case-insensitive, `#`/`//` lines are comments)
    /// and optionally `camera.ini` (`CAMERA_OFFSET x y z`).
    pub fn parse(setup: &str, camera_ini: Option<&str>) -> Result<Self, FormatError> {
        let mut result = Self::default();
        let (mut back_clip, mut fog_start) = (-100.0f32, 0.0f32);
        for node in parse_blocks(setup)? {
            let value = || node.parse_arg::<f32>(0);
            match node.name().to_ascii_lowercase().as_str() {
                "minzoomdistance" => result.min_zoom = value()?,
                "maxzoomdistance" => result.max_zoom = value()?,
                "minpitch" => result.min_pitch_deg = value()?,
                "maxpitch" => result.max_pitch_deg = value()?,
                "mousespeed" => result.mouse_speed = value()? * 0.001,
                "zoomspeed" => result.zoom_speed = value()? * 0.001,
                "fov" => result.fov = value()?,
                "aspectratio" => result.aspect_ratio = value()?,
                "backclip" => back_clip = value()?,
                "fogstart" => fog_start = value()?,
                _ => {} // FrontClip, FogColor: no effect on the camera
            }
        }
        result.default_fog_start = back_clip - fog_start;
        if let Some(text) = camera_ini {
            for node in parse_blocks(text)? {
                if node.name().eq_ignore_ascii_case("CAMERA_OFFSET") {
                    result.offset = Vec3::from(node.parse_args::<f32, 3>()?);
                }
            }
        }
        Ok(result)
    }

    /// Loads both files from the VFS; missing or broken files yield the defaults.
    pub fn load(vfs: &Vfs) -> Self {
        let read = |path: &str| vfs.read(path).ok().map(|b| decode_cp1252(&b));
        let Some(setup) = read("data/gifcon/camerasetup.ini") else {
            log::warn!(
                "data/gifcon/camerasetup.ini is missing, using the built-in camera defaults"
            );
            return Self::default();
        };
        let camera_ini = read("data/gifcon/camera.ini");
        Self::parse(&setup, camera_ini.as_deref()).unwrap_or_else(|e| {
            log::warn!("Camera settings not readable ({e}), using the built-in defaults");
            Self::default()
        })
    }
}

/// Aspect ratio of the projection. The original computes `m00 = AspectRatio * cot(fov/2)` with
/// `AspectRatio` = 1 and thereby stretches the image by width/height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AspectMode {
    /// Undistorted: `a = height / width`.
    #[default]
    Correct,
    /// Image impression of the original on a 4:3 screen: `a = 4/3 * height / width`.
    Original43,
    /// Literally like the code: `a = AspectRatio` (1.0).
    Stretch,
}

/// Factor `a` of the projection for an image `width x height`; `stretch_ratio` is `AspectRatio`
/// from `camerasetup.ini` (mode [`AspectMode::Stretch`]). Width and height are limited to at least 1
/// so that a tiny window does not divide by 0.
pub fn aspect_factor(mode: AspectMode, width: u32, height: u32, stretch_ratio: f32) -> f32 {
    let inverse = height.max(1) as f32 / width.max(1) as f32;
    match mode {
        AspectMode::Correct => inverse,
        AspectMode::Original43 => 4.0 / 3.0 * inverse,
        AspectMode::Stretch => stretch_ratio,
    }
}

/// Projection like `Matrix_PerspectiveFovLH` (`0x00575A60`) with `n = d/16`, `f = -d`:
/// `m00 = a*c`, `m11 = c`, `m22 = Q = 16/17`, `m23 = 1`, `m32 = -Q*n` (row-vector notation);
/// there is no far clipping plane.
pub fn game_projection(fov: f32, aspect_factor: f32, distance: f32) -> Mat4 {
    let c = 1.0 / (fov / 2.0).tan();
    let near = distance / 16.0;
    let far = -distance;
    let q = far / (far - near);
    Mat4::from_cols(
        Vec4::new(aspect_factor * c, 0.0, 0.0, 0.0),
        Vec4::new(0.0, c, 0.0, 0.0),
        Vec4::new(0.0, 0.0, q, 1.0),
        Vec4::new(0.0, 0.0, -q * near, 0.0),
    )
}

/// Game camera: eye, target, up (`Camera+0xA0`, `+0x94`, `+0xAC`).
#[derive(Debug, Clone, PartialEq)]
pub struct GameCamera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub setup: CameraSetup,
}

impl GameCamera {
    /// Initial pose of `GameEngine_ResetCamera` (`0x0043FF60`): target map center (y = 0),
    /// eye = target + `CAMERA_OFFSET`.
    pub fn new(setup: CameraSetup, terrain: &Terrain) -> Self {
        let target = Vec3::new(
            terrain.params.width() / 2.0,
            0.0,
            terrain.params.depth() / 2.0,
        );
        Self {
            eye: target + setup.offset,
            target,
            up: Vec3::Y,
            setup,
        }
    }

    /// Distance between eye and target.
    pub fn distance(&self) -> f32 {
        (self.eye - self.target).length()
    }

    /// Pitch in degrees like `Camera_IsValidEyePosition` (`atan2(dy, horizontal distance)`).
    pub fn pitch_deg(&self) -> f32 {
        pitch_deg(self.eye - self.target)
    }

    /// `Camera_IsValidEyePosition` (`0x004EBCE0`): distance in `[MinZoom, MaxZoom]`, integer
    /// pitch in `[MinPitch, MaxPitch]`.
    pub fn is_valid_eye(&self, eye: Vec3) -> bool {
        let offset = eye - self.target;
        let d = offset.length();
        let pitch = pitch_deg(offset).trunc();
        (self.setup.min_zoom..=self.setup.max_zoom).contains(&d)
            && (self.setup.min_pitch_deg..=self.setup.max_pitch_deg).contains(&pitch)
    }

    /// `Camera_SetTarget` (`0x004EB7E0`): x/z clamped to the map, y stays. The branch
    /// "y = terrain height + 80, at least `WATER_LEVEL`" is never reached by the game: the terrain of the
    /// camera (`Camera+0x70`) is only set by `Camera_Construct`, to 0 (D-254).
    pub fn clamp_target(target: Vec3, terrain: &Terrain) -> Vec3 {
        let x = target.x.clamp(0.0, terrain.params.width());
        let z = target.z.clamp(0.0, terrain.params.depth());
        Vec3::new(x, target.y, z)
    }

    /// `GameEngine_MoveCameraTo` (`0x004322A0`) with offset: eye = `(x, 0, z) + CAMERA_OFFSET`,
    /// target `(x, 0, z)` (x/z clamped to the map, y stays 0).
    pub fn move_to(&mut self, x: f32, z: f32, terrain: &Terrain) {
        self.eye = Vec3::new(x, 0.0, z) + self.setup.offset;
        self.target = Self::clamp_target(Vec3::new(x, 0.0, z), terrain);
    }

    /// `GameEngine_MoveCameraTo` (`0x004322A0`) with flag 0: target `(x, 0, z)` (x/z clamped to the map), eye =
    /// `(x, 0, z)` + the current eye - target offset (distance, yaw and pitch stay). Camera cycle of the
    /// control groups (`0x0047BE00`) and `selectnextguid` (`0x0048B4A0`); the reimplementation has no
    /// overview mode (`GE+0x2700`).
    pub fn move_to_keeping_offset(&mut self, x: f32, z: f32, terrain: &Terrain) {
        let offset = self.eye - self.target;
        self.eye = Vec3::new(x, 0.0, z) + offset;
        self.target = Self::clamp_target(Vec3::new(x, 0.0, z), terrain);
    }

    /// `Camera_Move` (`0x004EB5B0`): move the target, distance and pitch stay. `forward`
    /// moves away from the eye, `right` to the right in the image; step = value * distance * 0.0015.
    pub fn scroll(&mut self, forward: f32, right: f32, terrain: &Terrain) {
        let offset = self.eye - self.target;
        let horizontal = Vec3::new(offset.x, 0.0, offset.z).normalize_or_zero();
        let side = horizontal.cross(self.up).normalize_or_zero();
        let step = offset.length() * 0.0015;
        let moved = self.target - horizontal * forward * step + side * right * step;
        self.target = Self::clamp_target(moved, terrain);
        self.eye = self.target + offset;
    }

    /// Sets the eye if the pose is valid; otherwise everything stays (like the game).
    fn try_eye(&mut self, eye: Vec3) -> bool {
        let valid = self.is_valid_eye(eye);
        if valid {
            self.eye = eye;
        }
        valid
    }

    /// `Camera_ZoomStep` (`0x004EBD90`): -1.2 % (closer) or +1.5 % of the distance per step.
    pub fn zoom_step(&mut self, closer: bool) -> bool {
        let factor = if closer { 1.0 - 0.012 } else { 1.015 };
        self.try_eye(self.target + (self.eye - self.target) * factor)
    }

    /// Target distance of the mouse wheel (`Camera_ComputeWheelZoomTarget` `0x004EBB40`): `d * 0.76`
    /// (closer) or `d * 1.36`, limited to `[MinZoom + 10, MaxZoom - 10]`.
    pub fn wheel_zoom_target(&self, closer: bool) -> f32 {
        let d = self.distance() * if closer { 0.76 } else { 1.36 };
        d.clamp(self.setup.min_zoom + 10.0, self.setup.max_zoom - 10.0)
    }

    /// Sets the distance with the same viewing direction (for the smooth transition of the mouse wheel).
    pub fn set_distance(&mut self, distance: f32) -> bool {
        let direction = (self.eye - self.target).normalize_or_zero();
        self.try_eye(self.target + direction * distance)
    }

    /// `Camera_Pitch` (`0x004EBF00`): rotate the eye about the horizontal transverse axis through the target
    /// (positive = steeper).
    pub fn pitch(&mut self, radians: f32) -> bool {
        let offset = self.eye - self.target;
        let horizontal = Vec3::new(offset.x, 0.0, offset.z).normalize_or_zero();
        let axis = horizontal.cross(self.up).normalize_or_zero();
        let rotated = glam::Quat::from_axis_angle(axis, radians) * offset;
        self.try_eye(self.target + rotated)
    }

    /// `Camera_Yaw` (`0x004EB4D0`): rotate the viewing direction about the vertical axis, angle * 1.5, with the game's
    /// rotation matrix (`Matrix_RotationY` `0x005755A0`, transposed D3DX matrix, [`crate::math::game_rotation_y`]): a
    /// positive angle rotates the eye - target offset in top view (+x right, +z up) counterclockwise (+x -> +z)
    /// (O-331).
    pub fn yaw(&mut self, radians: f32) -> bool {
        let rotated = crate::math::game_rotation_y(radians * 1.5) * (self.eye - self.target);
        self.try_eye(self.target + rotated)
    }

    /// View matrix like `Matrix_LookAtLH` (`0x00576D70`, roll angle 0).
    pub fn view(&self) -> Mat4 {
        look_at_lh(self.eye, self.target, self.up)
    }

    /// Factor `a` of the projection for an image `width x height` ([`aspect_factor`] with
    /// `AspectRatio` of this camera).
    pub fn aspect_factor(&self, mode: AspectMode, width: u32, height: u32) -> f32 {
        aspect_factor(mode, width, height, self.setup.aspect_ratio)
    }

    /// Projection for an image `width x height` (`game_projection`).
    pub fn projection(&self, mode: AspectMode, width: u32, height: u32) -> Mat4 {
        game_projection(
            self.setup.fov,
            self.aspect_factor(mode, width, height),
            self.distance(),
        )
    }

    /// Fog range like `Camera_Apply` (`0x004EBA5B`...): `Start = d + FOG_START`,
    /// `End = Start + FOG_END`. Without `FOG_START`, `BackClip - Fogstart` applies; without `FOG_END`
    /// (not preset by the game) 1,000.
    pub fn fog_range(&self, fog: Option<&FogParams>) -> (f32, f32) {
        let start = fog
            .and_then(|f| f.start)
            .unwrap_or(self.setup.default_fog_start);
        let length = fog.and_then(|f| f.end).unwrap_or(1000.0);
        let start = self.distance() + start;
        (start, start + length)
    }

    /// View ray through an image point (`ndc` in -1..1, y up): origin and direction
    /// (normalized); `projection` is the matrix from [`GameCamera::projection`].
    pub fn ray(&self, projection: Mat4, ndc: Vec2) -> (Vec3, Vec3) {
        ray_from_ndc(projection * self.view(), ndc)
    }
}

/// View ray through an image point for an arbitrary view-projection matrix (`view_proj` =
/// projection * view; `ndc` in -1..1, y up): origin on the near plane (depth 0) and
/// normalized direction. Also for the free camera of the map viewer.
pub fn ray_from_ndc(view_proj: Mat4, ndc: Vec2) -> (Vec3, Vec3) {
    let inverse = view_proj.inverse();
    let near = inverse.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
    let far = inverse.project_point3(Vec3::new(ndc.x, ndc.y, 0.5));
    (near, (far - near).normalize_or_zero())
}

/// `D3DXMatrixLookAtLH`: `z = normalize(target - eye)`, `x = normalize(up x z)`, `y = z x x`;
/// as a glam matrix (columns = rows of the Direct3D matrix, transposed).
pub fn look_at_lh(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let z = (target - eye).normalize_or_zero();
    let x = up.cross(z).normalize_or_zero();
    let y = z.cross(x);
    Mat4::from_cols(
        Vec4::new(x.x, y.x, z.x, 0.0),
        Vec4::new(x.y, y.y, z.y, 0.0),
        Vec4::new(x.z, y.z, z.z, 0.0),
        Vec4::new(-x.dot(eye), -y.dot(eye), -z.dot(eye), 1.0),
    )
}

/// Distance along the ray (`direction` normalized) to the first intersection with a sphere;
/// `None` without an intersection or if the sphere lies entirely behind the origin.
pub fn ray_sphere(origin: Vec3, direction: Vec3, center: Vec3, radius: f32) -> Option<f32> {
    let oc = origin - center;
    let b = oc.dot(direction);
    let c = oc.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    [-b - root, -b + root].into_iter().find(|&t| t >= 0.0)
}

fn pitch_deg(offset: Vec3) -> f32 {
    let horizontal = (offset.x * offset.x + offset.z * offset.z).sqrt();
    offset.y.atan2(horizontal).to_degrees()
}

/// Start point of the camera of a fixed map: [`start_focus_of`] for the first human player of the `.lvl`
/// (`PLAYER_TYPE` 0, with its `PLAYER_COLOR`), without one the map center. A random map has no players in the
/// `.lvl`; there the game host calls [`start_focus_of`] with the local player (re/random-map Z.10).
pub fn start_focus(scene: &LevelScene) -> Vec2 {
    match scene.level.players.iter().find(|p| p.kind == 0) {
        Some(player) => start_focus_of(scene, player.color),
        None => map_center(scene),
    }
}

/// `GameEngine_CenterCameraOnStart(player)` (`0x00439190`) for the player `player` (slot = color): area
/// `start <color>`/`start<color>`, otherwise the position of the first game object of this player (`0x00475E70`; in the
/// game host the first object of the slot in the simulation, in free play the HQ), otherwise the map center.
/// Ambient effects (`ObjectClass::Ambient`) are also in `scene.objects` and do not count.
pub fn start_focus_of(scene: &LevelScene, player: i32) -> Vec2 {
    const COLORS: [&str; 9] = [
        "yellow", "blue", "green", "red", "purple", "pink", "orange", "cyan", "white",
    ];
    if let Some(color) = usize::try_from(player).ok().and_then(|i| COLORS.get(i)) {
        let names = [format!("start {color}"), format!("start{color}")];
        if let Some(area) = scene.world.areas.iter().find(|a| names.contains(&a.name)) {
            let [x0, z0, x1, z1] = area.rect;
            return Vec2::new(
                scene.terrain.params.size_x * ((x0 + x1) / 2) as f32,
                scene.terrain.params.size_z * ((z0 + z1) / 2) as f32,
            );
        }
    }
    scene
        .objects
        .iter()
        .filter(|o| o.class == ObjectClass::Game)
        .find(|o| o.player == Some(player))
        .map_or_else(
            || map_center(scene),
            |o| Vec2::new(o.position.x, o.position.z),
        )
}

/// Map center (`GameEngine_ResetCamera` `0x0043FF60`: target `(width/2, 0, depth/2)`).
fn map_center(scene: &LevelScene) -> Vec2 {
    Vec2::new(
        scene.terrain.params.width() / 2.0,
        scene.terrain.params.depth() / 2.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::PlacedObject;
    use crate::terrain::tests::test_terrain;
    use glam::Affine3A;
    use opennml_formats::lvl::parse_level;
    use opennml_formats::wld::parse_world;
    use std::collections::BTreeMap;

    const SETUP: &str = "// Comment\r\nFrontClip 1.0\r\nBackClip 6000.0\r\nFogstart 0\r\n#Fov 0.1\r\nMinZoomDistance 11000\r\nMaxZoomDistance 30000\r\nMinPitch 20\r\nMaxPitch 80\r\nMouseSpeed 800\r\nZoomSpeed 800\r\n";

    fn camera() -> (GameCamera, Terrain) {
        let (terrain, _) = test_terrain(64, |_, _| 3);
        let setup = CameraSetup::parse(
            SETUP,
            Some("CAMERA_OFFSET -10026.258789 11488.535156 -10381.374023\r\n"),
        )
        .unwrap();
        (GameCamera::new(setup, &terrain), terrain)
    }

    #[test]
    fn setup_is_read_like_camera_construct() {
        let (camera, _) = camera();
        let s = &camera.setup;
        assert_eq!(
            (s.min_zoom, s.max_zoom, s.min_pitch_deg, s.max_pitch_deg),
            (11000.0, 30000.0, 20.0, 80.0)
        );
        assert!((s.mouse_speed - 0.8).abs() < 1e-6 && (s.zoom_speed - 0.8).abs() < 1e-6);
        assert_eq!(
            (s.fov, s.aspect_ratio, s.default_fog_start),
            (0.1, 1.0, 6000.0)
        );
        assert!((s.offset - DEFAULT_CAMERA_OFFSET).length() < 1e-3);
        assert_eq!(
            CameraSetup::parse("", None).unwrap(),
            CameraSetup {
                default_fog_start: -100.0,
                ..CameraSetup::default()
            }
        );
    }

    // QUIRK D-254: kept
    #[test]
    fn start_position_matches_the_documented_values() {
        // re/renderer: distance ~ 18,447, pitch ~ 38.5 degrees with the target at height 0.
        let (mut camera, terrain) = camera();
        camera.move_to(4096.0, 4096.0, &terrain);
        assert_eq!(camera.target, Vec3::new(4096.0, 0.0, 4096.0));
        let raw = camera.setup.offset.length();
        assert!((raw - 18447.0).abs() < 1.0, "{raw}");
        assert!((pitch_deg(camera.setup.offset) - 38.5).abs() < 0.1);
        assert!(camera.is_valid_eye(camera.eye));
    }

    // QUIRK D-254: kept
    #[test]
    fn the_target_stays_at_height_0_over_hills_and_water() {
        // `Camera+0x70` (terrain) stays 0 in the original: `Camera_SetTarget` never lifts the target to
        // terrain height + 80 (D-254). Start and scrolling keep y = 0 and the distance of
        // `CAMERA_OFFSET`, even over hills and below the water level.
        let (mut camera, mut terrain) = camera();
        for (i, v) in terrain.vertices.iter_mut().enumerate() {
            v.position.y = if i % 2 == 0 { 500.0 } else { -400.0 };
        }
        camera.move_to(4096.0, 4096.0, &terrain);
        assert_eq!(camera.target, Vec3::new(4096.0, 0.0, 4096.0));
        assert_eq!(camera.eye, camera.target + camera.setup.offset);
        camera.scroll(10.0, 5.0, &terrain);
        assert_eq!(camera.target.y, 0.0);
        assert!((camera.distance() - camera.setup.offset.length()).abs() < 1e-2);
        // `Camera_SetTarget` further clamps x/z to the map.
        camera.move_to(-500.0, 99999.0, &terrain);
        assert_eq!(camera.target, Vec3::new(0.0, 0.0, terrain.params.depth()));
    }

    /// `GameEngine_MoveCameraTo(x, z, 0)` (`0x004322A0`): the eye - target offset stays, the
    /// target is clamped to the map as with `move_to`; `move_to` still sets the default offset.
    // QUIRK O-43: kept
    #[test]
    fn move_keeping_offset_keeps_the_current_offset() {
        let (mut camera, terrain) = camera();
        camera.move_to(4096.0, 4096.0, &terrain);
        assert!(camera.yaw(0.5) && camera.zoom_step(true));
        let offset = camera.eye - camera.target;
        assert!((offset - camera.setup.offset).length() > 10.0);
        camera.move_to_keeping_offset(1000.0, 2000.0, &terrain);
        assert_eq!(camera.target, Vec3::new(1000.0, 0.0, 2000.0));
        assert_eq!(camera.eye, Vec3::new(1000.0, 0.0, 2000.0) + offset);
        camera.move_to_keeping_offset(-50.0, 300.0, &terrain);
        assert_eq!(camera.target, Vec3::new(0.0, 0.0, 300.0));
        camera.move_to(1000.0, 2000.0, &terrain);
        assert_eq!(camera.eye, camera.target + camera.setup.offset);
    }

    #[test]
    fn scrolling_keeps_distance_and_moves_away_from_the_eye() {
        let (mut camera, terrain) = camera();
        camera.move_to(4096.0, 4096.0, &terrain);
        let (d, eye_dir) = (camera.distance(), camera.eye - camera.target);
        let before = camera.target;
        camera.scroll(10.0, 0.0, &terrain);
        assert!((camera.distance() - d).abs() < 1e-2);
        let moved = camera.target - before;
        assert!(moved.dot(eye_dir) < 0.0, "forward = away from the eye");
        assert!((moved.length() - 10.0 * d * 0.0015).abs() < 1.0);

        // To the right: perpendicular to the horizontal viewing direction, to the right in the image, same
        // distance, the eye moves with the target.
        let (before, eye_before) = (camera.target, camera.eye);
        camera.scroll(0.0, 10.0, &terrain);
        assert!((camera.distance() - d).abs() < 1e-2);
        let moved = camera.target - before;
        let horizontal_eye_dir = Vec3::new(eye_dir.x, 0.0, eye_dir.z).normalize();
        assert!(moved.dot(horizontal_eye_dir).abs() < 1e-2, "{moved:?}");
        assert!((moved.length() - 10.0 * d * 0.0015).abs() < 1.0);
        let on_screen = camera.view().transform_vector3(moved);
        assert!(
            on_screen.x > 0.0 && on_screen.y.abs() < 1e-2 * on_screen.x,
            "{on_screen:?}"
        );
        assert!((camera.eye - eye_before - moved).length() < 1e-2);

        // Target stays on the map, even with huge steps in all directions.
        let (width, depth) = (terrain.params.width(), terrain.params.depth());
        for (forward, right) in [(1e6, 0.0), (-1e6, 0.0), (0.0, 1e6), (0.0, -1e6)] {
            camera.scroll(forward, right, &terrain);
            assert!(
                (0.0..=width).contains(&camera.target.x)
                    && (0.0..=depth).contains(&camera.target.z),
                "{:?}",
                camera.target
            );
            assert!((camera.distance() - d).abs() < 1e-1);
        }
    }

    #[test]
    fn zoom_and_pitch_respect_the_limits() {
        let (mut camera, terrain) = camera();
        camera.move_to(4096.0, 4096.0, &terrain);
        while camera.zoom_step(true) {}
        assert!(camera.distance() >= 11000.0 && camera.distance() < 11000.0 / 0.988);
        while camera.pitch(0.0314) {}
        assert!(
            camera.pitch_deg().trunc() <= 80.0 && camera.pitch_deg() > 75.0,
            "{}",
            camera.pitch_deg()
        );
        while camera.pitch(-0.0314) {}
        assert!(
            camera.pitch_deg().trunc() >= 20.0 && camera.pitch_deg() < 25.0,
            "{}",
            camera.pitch_deg()
        );
        assert_eq!(camera.wheel_zoom_target(true), 11010.0);
        let d = camera.distance();
        assert!(camera.yaw(0.5) && (camera.distance() - d).abs() < 1e-1);
    }

    /// `Camera_Yaw(+a)` rotates the offset like the game's `Matrix_RotationY` from +x to +z (top view
    /// counterclockwise), distance and pitch stay; `Camera_Pitch(+a)` makes it steeper (O-331).
    // QUIRK O-306: kept
    #[test]
    fn yaw_turns_like_the_game_matrix_and_pitch_positive_is_steeper() {
        let (mut camera, terrain) = camera();
        camera.move_to(4096.0, 4096.0, &terrain);
        let horizontal = camera.setup.offset.length() * 0.6;
        camera.eye = camera.target + Vec3::new(horizontal, camera.setup.offset.length() * 0.8, 0.0);
        let (d, pitch) = (camera.distance(), camera.pitch_deg());
        assert!(camera.yaw(0.1));
        let offset = camera.eye - camera.target;
        assert!(offset.z > 0.0 && offset.x > 0.0, "{offset:?}");
        assert!(
            (offset.z.atan2(offset.x) - 0.15).abs() < 1e-4,
            "angle * 1.5"
        );
        assert!((camera.distance() - d).abs() < 1e-1 && (camera.pitch_deg() - pitch).abs() < 1e-3);
        assert!(camera.pitch(0.01) && camera.pitch_deg() > pitch);
    }

    #[test]
    fn look_at_maps_target_to_the_view_axis() {
        let view = look_at_lh(Vec3::new(0.0, 100.0, -100.0), Vec3::ZERO, Vec3::Y);
        let target = view.transform_point3(Vec3::ZERO);
        assert!(
            target.x.abs() < 1e-4 && target.y.abs() < 1e-4 && target.z > 0.0,
            "{target:?}"
        );
        // Left-handed: +x of the world appears on the right of the image.
        assert!(view.transform_point3(Vec3::X).x > 0.0);
    }

    #[test]
    fn aspect_factor_without_camera_uses_the_given_stretch_ratio() {
        assert_eq!(
            aspect_factor(AspectMode::Correct, 1600, 900, 1.25),
            900.0 / 1600.0
        );
        assert_eq!(aspect_factor(AspectMode::Original43, 1600, 1200, 1.25), 1.0);
        assert_eq!(aspect_factor(AspectMode::Stretch, 1600, 900, 1.25), 1.25);
        assert_eq!(aspect_factor(AspectMode::Correct, 0, 0, 1.25), 1.0);
        let (camera, _) = camera();
        for mode in [
            AspectMode::Correct,
            AspectMode::Original43,
            AspectMode::Stretch,
        ] {
            assert_eq!(
                camera.aspect_factor(mode, 1024, 768),
                aspect_factor(mode, 1024, 768, camera.setup.aspect_ratio)
            );
        }
    }

    #[test]
    fn projection_is_the_game_matrix() {
        let p = game_projection(0.1, 1.0, 1600.0);
        // Near plane d/16 = 100 -> depth 0; far away toward 16/17.
        let near = p * Vec4::new(0.0, 0.0, 100.0, 1.0);
        assert!((near.z / near.w).abs() < 1e-6);
        let far = p * Vec4::new(0.0, 0.0, 1e9, 1.0);
        assert!((far.z / far.w - 16.0 / 17.0).abs() < 1e-4);
        let (camera, _) = camera();
        assert_eq!(
            camera.aspect_factor(AspectMode::Correct, 1600, 900),
            900.0 / 1600.0
        );
        assert_eq!(
            camera.aspect_factor(AspectMode::Original43, 1600, 1200),
            1.0
        );
        assert_eq!(camera.aspect_factor(AspectMode::Stretch, 1600, 900), 1.0);

        // Tiny window (0 x 0): no division by 0, finite and positive in every mode.
        for mode in [
            AspectMode::Correct,
            AspectMode::Original43,
            AspectMode::Stretch,
        ] {
            let a = camera.aspect_factor(mode, 0, 0);
            assert!(a.is_finite() && a > 0.0, "{mode:?}: {a}");
            let projection = camera.projection(mode, 0, 0);
            assert!(projection.to_cols_array().iter().all(|v| v.is_finite()));
        }
        assert_eq!(camera.aspect_factor(AspectMode::Correct, 0, 0), 1.0);

        // Portrait 900 x 1600: a = height / width > 1, the ratio of the scales m00 : m11
        // is height : width, so the pixels stay square.
        assert_eq!(
            camera.aspect_factor(AspectMode::Correct, 900, 1600),
            1600.0 / 900.0
        );
        assert_eq!(
            camera.aspect_factor(AspectMode::Original43, 900, 1600),
            4.0 / 3.0 * (1600.0 / 900.0)
        );
        assert_eq!(camera.aspect_factor(AspectMode::Stretch, 900, 1600), 1.0);
        let portrait = camera.projection(AspectMode::Correct, 900, 1600);
        assert!(portrait.to_cols_array().iter().all(|v| v.is_finite()));
        let (m00, m11) = (portrait.x_axis.x, portrait.y_axis.y);
        assert!(m00 > 0.0 && m11 > 0.0);
        assert!((m00 / m11 - 1600.0 / 900.0).abs() < 1e-4, "{m00} / {m11}");
    }

    #[test]
    fn ray_through_the_center_hits_the_target() {
        let (mut camera, terrain) = camera();
        camera.move_to(4096.0, 4096.0, &terrain);
        let projection = camera.projection(AspectMode::Correct, 800, 600);
        let (origin, dir) = camera.ray(projection, Vec2::ZERO);
        let to_target = (camera.target - origin).normalize();
        assert!(dir.dot(to_target) > 0.9999, "{dir:?} vs {to_target:?}");
    }

    #[test]
    fn ray_from_ndc_is_shared_with_the_game_camera() {
        let (mut camera, terrain) = camera();
        camera.move_to(4096.0, 4096.0, &terrain);
        let projection = camera.projection(AspectMode::Correct, 800, 600);
        let view_proj = projection * camera.view();
        for ndc in [Vec2::new(0.5, -0.25), Vec2::new(-0.9, 0.8)] {
            let (origin, dir) = ray_from_ndc(view_proj, ndc);
            assert_eq!((origin, dir), camera.ray(projection, ndc));
            assert!((dir.length() - 1.0).abs() < 1e-4);
            // A point on the ray maps back to the image point.
            let back = view_proj.project_point3(origin + dir * 5000.0);
            assert!(
                (back.truncate() - ndc).length() < 1e-3,
                "{back:?} vs {ndc:?}"
            );
        }
    }

    #[test]
    fn ray_sphere_finds_the_nearest_hit_in_front() {
        let hit = ray_sphere(Vec3::ZERO, Vec3::Z, Vec3::new(0.0, 0.0, 10.0), 2.0);
        assert_eq!(hit, Some(8.0));
        assert_eq!(
            ray_sphere(Vec3::ZERO, Vec3::Z, Vec3::new(0.0, 5.0, 10.0), 2.0),
            None
        );
        assert_eq!(
            ray_sphere(Vec3::ZERO, Vec3::Z, Vec3::new(0.0, 0.0, -10.0), 2.0),
            None
        );
        assert_eq!(ray_sphere(Vec3::ZERO, Vec3::Z, Vec3::ZERO, 2.0), Some(2.0));
    }

    #[test]
    fn fog_range_starts_behind_the_target() {
        let (camera, _) = camera();
        let fog = FogParams {
            color: [255; 3],
            start: Some(-8000.0),
            end: Some(10000.0),
        };
        let (start, end) = camera.fog_range(Some(&fog));
        assert!((start - (camera.distance() - 8000.0)).abs() < 1e-2);
        assert!((end - start - 10000.0).abs() < 1e-2);
        let (start, _) = camera.fog_range(None);
        assert!((start - camera.distance() - 6000.0).abs() < 1e-2);
    }

    fn placed(class: ObjectClass, x: f32, z: f32, player: Option<i32>) -> PlacedObject {
        PlacedObject {
            index: 0,
            guid: 10,
            class,
            type_name: String::new(),
            name: None,
            model: String::new(),
            player,
            parts: Vec::new(),
            no_shadow: false,
            position: Vec3::new(x, 0.0, z),
            rotation: Vec3::ZERO,
            transform: Affine3A::IDENTITY,
            sim_id: None,
        }
    }

    /// Scene with a human player on slot 1 (`blue`) and terrain 64 x 64 tiles.
    fn scene(areas: &str, objects: Vec<PlacedObject>) -> LevelScene {
        let (terrain, project) = test_terrain(64, |_, _| 3);
        let lvl = "LEVELFILE V1.08e\r\nPROJECT_FILE \"Data\\Projects\\test.prj\"\r\n[PLAYER]\r\n{\r\n\tPLAYER_NAME \"29081 RED\"\r\n\tPLAYER_PLAYER \"SPA\"\r\n\tPLAYER_TYPE 0\r\n\tPLAYER_COLOR 1\r\n}\r\n";
        let wld = format!(
            "WORLDFILE V1.08e\r\n[TERRAIN]\r\n{{\r\n\tTERRAIN_DIM_X 2\r\n\tTERRAIN_DIM_Z 2\r\n\tTERRAIN_SIZE_X 128\r\n\tTERRAIN_SIZE_Z 128\r\n\tLIGHT_POSITION 0 1 0\r\n\tLIGHT_COLOR 255 255 255\r\n\tLIGHT_INTENSITY 100\r\n\tCHUNK_SIZE 2\r\n}}\r\n{areas}"
        );
        LevelScene {
            lvl_path: "data/levels/test/test.lvl".into(),
            level: parse_level(lvl).unwrap(),
            world: parse_world(&wld).unwrap(),
            project,
            types: Default::default(),
            terrain,
            objects,
            start_points: BTreeMap::new(),
            object_light: None,
        }
    }

    #[test]
    fn start_focus_prefers_the_start_area_then_game_objects_then_the_center() {
        let area = "[AREA]\r\n{\r\n\tAREA_NAME \"Start Blue\"\r\n\tAREA_RECT 10 20 14 24\r\n}\r\n";
        let objects = vec![
            placed(ObjectClass::Ambient, 10.0, 20.0, Some(1)),
            placed(ObjectClass::Game, 300.0, 400.0, Some(2)),
            placed(ObjectClass::Game, 30.0, 40.0, Some(1)),
        ];
        assert_eq!(
            start_focus(&scene(area, objects.clone())),
            Vec2::new(128.0 * 12.0, 128.0 * 22.0)
        );
        // Without an area the first game object of the player counts; ambient effects do not.
        assert_eq!(start_focus(&scene("", objects)), Vec2::new(30.0, 40.0));
        // Only ambient effects: map center.
        let only_ambient = vec![placed(ObjectClass::Ambient, 10.0, 20.0, Some(1))];
        assert_eq!(
            start_focus(&scene("", only_ambient)),
            Vec2::new(4096.0, 4096.0)
        );
    }

    /// `start_focus_of` takes the passed player, even if the `.lvl` has no human
    /// (random map): area of its color, otherwise its first game object, otherwise the map center.
    // QUIRK D-366: kept
    #[test]
    fn start_focus_of_uses_the_given_player() {
        let area = "[AREA]\r\n{\r\n\tAREA_NAME \"start green\"\r\n\tAREA_RECT 2 4 6 8\r\n}\r\n";
        let objects = vec![
            placed(ObjectClass::Ambient, 10.0, 20.0, Some(2)),
            placed(ObjectClass::Game, 300.0, 400.0, Some(2)),
            placed(ObjectClass::Game, 500.0, 600.0, Some(2)),
            placed(ObjectClass::Game, 30.0, 40.0, Some(1)),
        ];
        let mut no_players = scene("", objects.clone());
        no_players.level.players.clear();
        assert_eq!(start_focus(&no_players), Vec2::new(4096.0, 4096.0));
        assert_eq!(start_focus_of(&no_players, 2), Vec2::new(300.0, 400.0));
        assert_eq!(start_focus_of(&no_players, 0), Vec2::new(4096.0, 4096.0));
        assert_eq!(
            start_focus_of(&scene(area, objects), 2),
            Vec2::new(128.0 * 4.0, 128.0 * 6.0)
        );
    }
}
