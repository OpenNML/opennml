//! Camera of the scripts in the host (deviation from the original): the simulation reports the camera actions as
//! events ([`Presentation`]`::Camera*`), the host keeps the backup, move list, rotation, binding and the
//! deferred cut and uses them to move the game camera each frame. The state is host state: it is in the
//! savegame of `nml-game` (`HostPart::script_camera`), not in the simulation state; the viewer starts after
//! `sim_load` with a free camera.
//!
//! Reference in the original (re/script-actions):
//! `GameEngine_UpdateCameraFollow` (`0x00432590`), per frame with the game time of the frame
//! (`dt * (speed + 10) / 20`), not in pause. Floating point (`sin`/`cos`, moves) only here in the host,
//! never in the simulation.

use std::collections::VecDeque;

use glam::Vec3;
use nml_formats::config::lower_german;
use nml_formats::wld::NamedCamera;
use nml_sim::id::ObjectId;
use nml_sim::script::present::Presentation;
use nml_world::camera::GameCamera;
use nml_world::terrain::Terrain;
use serde::{Deserialize, Serialize};

use crate::game::sim_host::SimHost;

/// Maximum number of waiting moves. The original appends without limit (`m_CameraMoveList`, at most one is taken
/// per frame); the reimplementation discards beyond that (with a warning) so that the list does not grow without
/// limit.
pub const MAX_MOVES: usize = 65_536;

/// Duration of the player's soft moves in game ms: Home (`/setcamera`, `PUSH 0xFA` at `0x00420265` ->
/// `FUN_00433FC0`) and mouse wheel (`PUSH 0xFA` at `0x0043D0F2` -> `FUN_00433890`).
pub const PLAYER_MOVE_MS: i32 = 250;

/// A waiting move (element of `m_CameraMoveList` `GE+0x2574`): target eye, target point and duration in game ms.
/// Roll and FOV of the entry never take effect (E-235) and are missing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MoveLeg {
    pub from: [f32; 3],
    pub at: [f32; 3],
    pub ms: i32,
    /// Move of the player (Home, [`ScriptCamera::push_home`]) instead of the scripts. The original does not know the
    /// difference (one list); the reimplementation needs it because only script moves lock the player camera
    /// ([`ScriptCamera::running`]).
    pub player: bool,
}

/// Running rotation 212 (`m_CamRotCounter` `GE+0x25D4`, `m_CamRotWinkelSpeed` `GE+0x25D8`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraRotation {
    /// Remaining time in game ms; the rotation runs as long as it is > 0.
    pub remaining_ms: f32,
    /// Radians per game ms around the vertical axis through the camera target.
    pub rad_per_ms: f32,
}

/// Camera state of the scripts (host state, in the savegame of `nml-game`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScriptCamera {
    /// `m_BackupFrom`/`m_BackupAt` (`GE+0x5D0`/`+0x5DC`) from 143: eye and target. `None` before the first 143; 144
    /// and 216 then take the origin (initial value of the constructor).
    pub backup: Option<([f32; 3], [f32; 3])>,
    /// Waiting moves (142, 172, 216 and their percent forms), chained, never replaced.
    pub moves: VecDeque<MoveLeg>,
    /// `m_CameraTime` (`GE+0x2554`): remaining time of the foremost, already taken move in game ms; <= 0 if none
    /// is running (also negative after an expired move).
    pub rest_ms: f32,
    /// `m_FromSpeed` (`GE+0x2534`): movement of the eye per game ms.
    pub eye_speed: [f32; 3],
    /// `m_AtSpeed` (`GE+0x2540`): movement of the target per game ms (without binding).
    pub target_speed: [f32; 3],
    /// The running move (`rest_ms > 0`) is one of the player ([`MoveLeg::player`]).
    pub player_leg: bool,
    /// Running rotation 212.
    pub rotation: Option<CameraRotation>,
    /// `m_CameraHID` (`GE+0x5C8`): the target follows this object (175); `None` = no binding.
    pub bind: Option<ObjectId>,
    /// `m_NextFrom`/`m_NextAt` with `m_IsNextCamSet` (141): cut in the next frame.
    pub pending_cut: Option<([f32; 3], [f32; 3])>,
}

impl ScriptCamera {
    /// Is the script camera running (cut pending, move running or waiting, rotation)? Then the player does not move
    /// the camera (keys, edge, wheel). A binding 175 alone does not count: as in the original
    /// the player then moves the eye, the target follows the object again in the next frame - so the player is not
    /// stuck either if a level never releases the binding. Moves of the player (Home, [`MoveLeg::player`]) do not
    /// count: the original never locks the keys because of the move list (`GameEngine_UpdateCamera` does not check
    /// `+0x2554`).
    pub fn running(&self) -> bool {
        self.pending_cut.is_some()
            || (self.rest_ms > 0.0 && !self.player_leg)
            || self.moves.iter().any(|m| !m.player)
            || self.rotation.is_some()
    }

    /// Is a move running or waiting, including one of the player? Then the original does not read the mouse wheel
    /// (`0x0043D0B2`: `+0x2554 > 0`); the reimplementation discards the wheel then.
    pub fn moving(&self) -> bool {
        self.rest_ms > 0.0 || !self.moves.is_empty()
    }

    /// Move to the home position (`FUN_00433FC0`): target at the call, eye = target + `CAMERA_OFFSET`
    /// (`Engine+0x634`; `+0x640` applies only in overview mode `+0x2700`, which the reimplementation does not have),
    /// appended to the move list. Script action 172 and Home (`/setcamera`, `player`) share it.
    fn push_to_game(&mut self, camera: &GameCamera, ms: i32, player: bool) {
        let at = camera.target;
        self.push(MoveLeg {
            from: (at + camera.setup.offset).to_array(),
            at: at.to_array(),
            ms,
            player,
        });
    }

    /// Home (`/setcamera`, `FUN_00420230` -> `FUN_00433FC0(250)`): soft move of [`PLAYER_MOVE_MS`] game ms to
    /// the home position, linear in the eye - target offset, target and FOV stay. It queues behind running
    /// script moves, stands still during the pause and does not lock the player camera ([`Self::running`]).
    pub fn push_home(&mut self, camera: &GameCamera) {
        self.push_to_game(camera, PLAYER_MOVE_MS, true);
    }

    /// A presentation event (events other than camera events are ignored): 2/170/144 set the camera
    /// immediately, 141 remembers the cut for the next frame, 142/172/216 append moves, 143 remembers the camera,
    /// 175-178 bind or release, 212 rotates. `cameras` are the `[CAMERA]` entries of the `.wld`; 143, 172 and 216
    /// read the camera **at the event** (E-233).
    pub fn apply(
        &mut self,
        event: &Presentation,
        camera: &mut GameCamera,
        cameras: &[NamedCamera],
        terrain: &Terrain,
        sim: &SimHost,
    ) {
        let cell = sim.rules.map.cell;
        match event {
            // 2 (`FUN_00435CC0`): tile * tile size (`FILD SIZE`, `FIMUL`), then `GameEngine_MoveCameraTo`.
            Presentation::CameraCenter { tile_x, tile_z } => {
                let x = (f64::from(cell[0]) * f64::from(*tile_x)) as f32;
                let z = (f64::from(cell[2]) * f64::from(*tile_z)) as f32;
                camera.move_to(x, z, terrain);
            }
            // 170 (`FUN_00439940`): simulation position `Fixed_ToFloat(+0x19C) * SIZE_X` of the object, once; if it
            // is missing, nothing.
            Presentation::CameraToObject { id } => {
                if let Some(object) = sim.state.object(*id, false) {
                    let x = object.grid.x.to_f32() * cell[0] as f32;
                    let z = object.grid.z.to_f32() * cell[2] as f32;
                    camera.move_to(x, z, terrain);
                }
            }
            // 141 (`FUN_0042EB50`): deferred to the next frame (E-232).
            Presentation::CameraCut { camera: name } => {
                self.pending_cut = Some(lookup(cameras, name));
            }
            // 142 (`FUN_00433910` -> `AddTail`): position of the named camera.
            Presentation::CameraMove { camera: name, ms } => {
                let (from, at) = lookup(cameras, name);
                self.push(MoveLeg {
                    from,
                    at,
                    ms: *ms,
                    player: false,
                });
            }
            // 143 (`FUN_0042DB70`): eye and target, without FOV.
            Presentation::CameraBackup => {
                self.backup = Some((camera.eye.to_array(), camera.target.to_array()));
            }
            // 144 (`FUN_00430B20` -> `FUN_00430220`): immediately; list, binding, rotation stay.
            Presentation::CameraRestoreBackup => {
                let (from, at) = self.backup.unwrap_or_default();
                set_pose(camera, from.into(), at.into(), terrain);
            }
            // 216 (`FUN_004310E0`): move to the backup with the values at the call.
            Presentation::CameraMoveToBackup { ms } => {
                let (from, at) = self.backup.unwrap_or_default();
                self.push(MoveLeg {
                    from,
                    at,
                    ms: *ms,
                    player: false,
                });
            }
            // 172 (`FUN_00433FC0`): target at the call, eye = target + `CAMERA_OFFSET`; resets nothing.
            Presentation::CameraToGame { ms } => self.push_to_game(camera, *ms, false),
            // 175-178 (`FUN_0042DDA0`): `m_CameraHID` = ID or 0.
            Presentation::CameraBind { id } => self.bind = *id,
            // 212 (`FUN_0042DEE0`): counter = ms, rate `(FILD degrees * pi_f) FIDIV (ms * 180)` (`IMUL` 32-bit) in
            // `f32`; replaces a running rotation, `ms <= 0` does not rotate. A divisor 0 (overflow of `ms * 180`)
            // would give +-inf/NaN, the reimplementation takes the rate 0.
            Presentation::CameraRotate { degrees, ms } => {
                self.rotation = (*ms > 0).then(|| {
                    let divisor = ms.wrapping_mul(180);
                    let rate = if divisor == 0 {
                        0.0
                    } else {
                        (f64::from(*degrees) * f64::from(std::f32::consts::PI) / f64::from(divisor))
                            as f32
                    };
                    CameraRotation {
                        remaining_ms: *ms as f32,
                        rad_per_ms: rate,
                    }
                });
            }
            _ => {}
        }
    }

    /// Append a move (`AddTail`), at most [`MAX_MOVES`].
    fn push(&mut self, leg: MoveLeg) {
        if self.moves.len() >= MAX_MOVES {
            log::warn!(
                "Script camera: more than {MAX_MOVES} waiting moves, the new one is dropped"
            );
            return;
        }
        self.moves.push_back(leg);
    }

    /// One frame with `dt_game_ms` of game time (`GameEngine_UpdateCameraFollow` `0x00432590`, steps as in
    /// the original):
    /// 1. pending cut: set it, otherwise nothing in this frame (its time expires for move, rotation, binding);
    /// 2. `T` = camera target, `O` = eye - target;
    /// 3. binding: `T` = presentation position of the object; if it is missing, the binding is released;
    /// 4. rotation of `O` by the full frame time (overrotation in the last frame);
    /// 5. running move: remainder > frame time -> remainder -= frame time; otherwise travel the remainder, the
    ///    leftover time goes on;
    /// 6. no running move: take the next one (only one per frame), velocities from `T + O` and `T`;
    ///    a move that fits into the leftover time expires without effect;
    /// 7. move by the leftover time (the target only without binding);
    /// 8. eye = old target + `O`, target `T` (x/z clamped to the map, y stays, D-254; `FUN_00430220`).
    pub fn update(
        &mut self,
        dt_game_ms: f32,
        camera: &mut GameCamera,
        terrain: &Terrain,
        sim: &SimHost,
    ) {
        let dt = dt_game_ms;
        if let Some((from, at)) = self.pending_cut.take() {
            set_pose(camera, from.into(), at.into(), terrain);
            return;
        }
        let old_target = camera.target;
        let mut target = camera.target;
        let mut offset = camera.eye - camera.target;
        let mut changed = false;
        if let Some(id) = self.bind {
            match sim.state.object(id, false) {
                Some(object) => {
                    target = sim.interpolated(object);
                    changed = true;
                }
                None => self.bind = None,
            }
        }
        if let Some(rotation) = self.rotation.as_mut() {
            // `Matrix_RotationY` with a row vector: x' = x*cos - z*sin, z' = x*sin + z*cos.
            let (sin, cos) = (dt * rotation.rad_per_ms).sin_cos();
            offset = Vec3::new(
                offset.x * cos - offset.z * sin,
                offset.y,
                offset.x * sin + offset.z * cos,
            );
            rotation.remaining_ms -= dt;
            if rotation.remaining_ms <= 0.0 {
                self.rotation = None;
            }
            changed = true;
        }
        let bound = self.bind.is_some();
        let mut rest_dt = dt;
        if self.rest_ms > dt {
            self.rest_ms -= dt;
        } else if self.rest_ms > 0.0 {
            let r = self.rest_ms;
            offset += Vec3::from(self.eye_speed) * r;
            if !bound {
                target += Vec3::from(self.target_speed) * r;
            }
            rest_dt = dt - r;
            self.rest_ms = 0.0;
            changed = true;
        }
        if self.rest_ms <= 0.0
            && let Some(leg) = self.moves.pop_front()
        {
            let r = leg.ms as f32;
            // If the move fits into the leftover time, the velocities stay unused (in the original +-inf/NaN
            // at 0 ms); the reimplementation sets them to 0 so that the savegame stays finite.
            if r > rest_dt {
                self.eye_speed = ((Vec3::from(leg.from) - (target + offset)) / r).to_array();
                self.target_speed = ((Vec3::from(leg.at) - target) / r).to_array();
            } else {
                self.eye_speed = [0.0; 3];
                self.target_speed = [0.0; 3];
            }
            self.rest_ms = r - rest_dt;
            self.player_leg = leg.player;
        }
        if self.rest_ms > 0.0 {
            offset += Vec3::from(self.eye_speed) * rest_dt;
            if !bound {
                target += Vec3::from(self.target_speed) * rest_dt;
            }
            changed = true;
        }
        if changed {
            set_pose(camera, old_target + offset, target, terrain);
        }
    }
}

/// Position of the camera `name` (`FUN_00434910`): the `[CAMERA]` name is lowercased as when loading the
/// `.wld` (`CString_MakeLowerGerman` at `0x0044280F`); for equal names the last one applies (the table `GE+0x618`
/// is a `CMap`). Unknown -> eye and target at the origin, without a message in the original (E-232).
fn lookup(cameras: &[NamedCamera], name: &str) -> ([f32; 3], [f32; 3]) {
    let name = lower_german(name);
    match cameras.iter().rev().find(|c| lower_german(&c.name) == name) {
        Some(c) => (c.from, c.at),
        None => {
            log::warn!("Script camera: camera \"{name}\" unknown, origin");
            ([0.0; 3], [0.0; 3])
        }
    }
}

/// `FUN_00430220`: target via `Camera_SetTarget` (x/z clamped to the map, y stays, E-235/D-254), eye unchecked.
fn set_pose(camera: &mut GameCamera, eye: Vec3, target: Vec3, terrain: &Terrain) {
    camera.target = GameCamera::clamp_target(target, terrain);
    camera.eye = eye;
}

#[cfg(test)]
mod tests;
