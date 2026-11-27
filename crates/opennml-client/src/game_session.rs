//! Frame loop of a match without egui: loading in phases, camera, simulation, selection, right-click, voices, target
//! highlight, ground markers and drawing the world. The viewer and `opennml-game` share it; the viewer first checks its
//! egui windows and modes ([`HostModes`]) and draws dialogs, overlay and egui on top.
//!
//! Time: The core works with frame times in fractions of milliseconds ([`GameSession::advance_by`], like the viewer's
//! virtual clock); the wrapper [`GameSession::advance`] turns them into frame times of the program clock in whole
//! milliseconds. If the match is not running (game menu open), nothing happens and the time base is only brought up to
//! date: no catching up when play resumes, like the halt branch of `GameEngine_Update` (`0x00445650`, re/ingame-menus
//! 0.1). [`SessionClock`] carries the GPU-free part of it.
//!
//! Device, archives and sound system belong to the host; the session gets them on each call.

use std::collections::HashSet;

use anyhow::Context;
use glam::{Vec2, Vec3};
use opennml_audio::engine::AudioEngine;
use opennml_audio::volume::Category;
use opennml_control::{
    RandomMapState, UiHudData, UiInputData, UiMinimapData, UiPortraitSlot, UiPresentationData,
    UiTextSlot,
};
use opennml_rda::Vfs;
use opennml_render::ViewParams;
use opennml_sim::id::{ObjectId, PLAYER_SLOTS};
use opennml_sim::rules::Rules;
use opennml_sim::save::RecordedCommand;
use opennml_sim::script::InfoFrameMemory;
use opennml_sim::script::present::Presentation;
use opennml_sim::setup::MatchSetup;
use opennml_sim::state::SimEvent;
use opennml_ui::assets::Texts;
use opennml_ui::coords::{Screen, VIRTUAL_H, VIRTUAL_W, game_rect};
use opennml_ui::dialog::UiContext;
use opennml_ui::draw::DrawCmd;
use opennml_ui::hud::countdown::{TIMER_POS, countdown_cmds, countdown_text, timer_cmds};
use opennml_ui::hud::info_frame::{InfoFrameKind, InfoFrameParams, InfoFrames};
use opennml_ui::hud::scene_fade::SceneFade;
use opennml_ui::hud::screen_text::{self, ScreenTexts};
use opennml_ui::hud::toolbar::minimap_buttons::{
    MSG_DIPLOMACY_BUTTON, MSG_DIPLOMACY_KEY, MSG_GAME_MENU, MSG_IDLE_ALL, MSG_IDLE_ONE, MSG_SIGNAL,
    mode_of,
};
use opennml_ui::hud::toolbar::window::MSG_MINIMAP_FIRST;
use opennml_ui::text::GameTags;
use opennml_world::camera::GameCamera;
use opennml_world::terrain::Terrain;
use winit::event::MouseScrollDelta;
use winit::keyboard::KeyCode;

use crate::game::chat;
use crate::game::combat_calc;
use crate::game::control_groups::{
    ControlGroups, GroupCx, GroupOutcome, GroupsSave, GuidTap, NextGuidIndex, SelectionClusters,
};
use crate::game::diplomacy::Masks;
use crate::game::economy::{PlacementPreview, queue_keys};
use crate::game::formation_order::FormationOrder;
use crate::game::hotkeys::{HotkeyCommand, Hotkeys, KeyContext};
use crate::game::hud::{
    Hud, SPEED_LOWERED_MESSAGE, SPEED_MESSAGE_FLAGS, SPEED_RAISED_MESSAGE, SimGameText,
};
use crate::game::input::{
    CameraController, CameraInput, CameraKeys, FreeCamera, FreeInput, edge_direction,
};
use crate::game::input_gate::{InputGate, sim_gate};
use crate::game::map::{MapLoad, MapSource, MapView};
use crate::game::markers::{
    TargetHighlight, ground_marker_objects, on_target_action, retain_highlight,
};
use crate::game::net_driver::{NetDriver, NetFrame, SessionStep, SpeedChange};
use crate::game::notices::{EVENT_2D_FLAGS, SoundKind, SoundRequest, sound_kind};
use crate::game::options::{Controls, ViewOptions};
use crate::game::pause::PauseState;
use crate::game::pointer::{PointerChoice, PointerState};
use crate::game::presentation::{HostRequest, PresentCtx, ScriptPresentation, game_ms};
use crate::game::rally_flag::rally_flag;
use crate::game::selection::{ClickClocks, MouseSide, RightClickTarget, Selection};
use crate::game::sim_host::SimHost;
use crate::game::speech_audio::SpeechAudio;
use crate::game::toolbar_host::{
    ActiveTargetDecal, DECAL_OBJECT_BUTTONS, TARGET_DECAL_BUTTONS, TargetDecal, ToolbarEffects,
    ToolbarHost, hover_target, portrait_model, surface_point,
};
use crate::game::unit_bar::{FormationDragTracker, UnitBar};
use crate::game::voices::Voices;
use crate::game_save::{CameraSave, GameSaveFile, HostPart};
use crate::keys::{InputEvent, MouseButton};
use crate::menu_host::MenuAudio;
use crate::ui_layer::screen_of;
use opennml_ui::hud::message_bar::NO_TILE;

/// Upper limit of the speed via key (`FUN_0042F050`: `v > 18` gives 19); loading and resuming allow any speed 0 ... 255
/// (O-153).
const KEY_MAX_GAME_SPEED: i32 = 19;

/// New speed after `changegamespeed(d)` (`FUN_0042F050`): `v = speed + d`, `v < 0` gives 0, `v > 18` gives 19; `None`
/// if it does not change (then neither message nor setting).
pub fn key_game_speed(current: u8, d: i32) -> Option<u8> {
    let v = i32::from(current)
        .saturating_add(d)
        .clamp(0, KEY_MAX_GAME_SPEED);
    u8::try_from(v).ok().filter(|&v| v != current)
}

/// Device, queue and target format (the GPU arguments of [`MapView::load`]).
#[derive(Clone, Copy)]
pub struct SessionGpu<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub format: wgpu::TextureFormat,
}

impl<'a> SessionGpu<'a> {
    /// Device, queue and surface format of the window framework.
    pub fn of(gpu: &'a opennml_render::Gpu) -> Self {
        Self {
            device: &gpu.device,
            queue: &gpu.queue,
            format: gpu.surface_format(),
        }
    }
}

/// Loading phases (a deviation from the original): the reimplementation loads in fixed steps and reports the progress
/// in percent after each one, the original redraws for each percent of the `.wld` read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LoadPhase {
    /// Before loading.
    Start = 0,
    /// Level read.
    LevelRead = 40,
    /// Simulation set up.
    SimReady = 70,
    /// Renderer ready.
    RenderReady = 100,
}

impl LoadPhase {
    /// Progress in percent.
    pub fn percent(self) -> u8 {
        self as u8
    }
}

/// What a frame reports to the host.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// A sound played by the simulation (message, object, combat); the viewer uses it to keep its state snapshot up to
    /// date.
    Sound(SoundRequest),
}

/// GPU-free core of the time base: frame time from the program clock in whole milliseconds and the rendering clock.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SessionClock {
    last_now: Option<u64>,
    /// Rendering clock in milliseconds (animation, messages, target highlight), from 0 at the start of the session; it
    /// only runs with the match ([`SessionClock::frame_ms`] with `running`), so it stands still in the game menu.
    /// Messages get their timestamp from this clock and expire against it, not against the program clock. Writers: in
    /// `opennml-game` only [`GameSession::advance`]; the viewer sets it before each call to its virtual clock
    /// (`sync_session`).
    pub now_ms: f64,
    /// Program clock in milliseconds (`GetTickCount`): the time of the last [`SessionClock::frame_ms`], also while the
    /// match is stopped (game menu). The bar's target point expires against it (300 ms real time, re/game-ui N.5.8,
    /// O-237). The viewer sets it to its virtual clock (`sync_session`).
    pub program_ms: f64,
    /// Frame time that passed during pause (O-317): the rendering clock stands still there, the water keeps running
    /// against [`SessionClock::water_ms`] (`Terrain_AdvanceWaterAnimation` in `GameEngine_Update` `0x00445650` also
    /// with `Engine+0x24E4`). Only [`SessionClock::pause_frame`] writes it; the viewer has no pause, for it this stays
    /// 0.
    pub pause_ms: f64,
    /// Game time of the pause that has not yet yielded an engine tick ([`SessionClock::pause_frame`]).
    pause_carry_ms: f64,
    /// Frame time of the stopped frames of the menu level ([`SessionClock::hold_frame`]) that the scripts' camera
    /// catches up in the next running frame ([`SessionClock::take_held_ms`], O-447).
    held_ms: f64,
}

impl SessionClock {
    /// Frame time since the last call (the first frame: 0); the rendering clock advances by it. If the match is not
    /// running, `None`: the time base is still set to `now` (`GameEngine_Update` in the halt branch), the rendering
    /// clock stands still, and resuming catches nothing up.
    pub fn frame_ms(&mut self, now: u64, running: bool) -> Option<f32> {
        self.program_ms = now as f64;
        self.pause_carry_ms = 0.0;
        let last = self.last_now.replace(now);
        if !running {
            return None;
        }
        let dt_ms = last.map_or(0, |last| now.saturating_sub(last)) as f32;
        self.now_ms += f64::from(dt_ms);
        Some(dt_ms)
    }

    /// A stopped frame of the menu level (O-447): like [`SessionClock::frame_ms`] without `running`, but the frame time
    /// is added to the time that the scripts' camera catches up. In the original `GameEngine_Update` does not run at
    /// all then (`GameApp_Tick` `0x004CA770` with bit `0x1` cleared), its time base `+0x2610` stays put: the first
    /// `Update` afterwards gives `GameEngine_UpdateCameraFollow` `0x00432590` the whole pause time, and it caps the
    /// ticks at 2. The reimplementation nevertheless keeps the simulation's time base up to date (no catching up) and
    /// only holds the camera time.
    pub fn hold_frame(&mut self, now: u64) {
        let dt_ms = self.last_now.map_or(0, |last| now.saturating_sub(last));
        self.frame_ms(now, false);
        self.held_ms += dt_ms as f64;
    }

    /// Takes the accumulated time of the stopped frames ([`SessionClock::hold_frame`]) and clears it.
    pub fn take_held_ms(&mut self) -> f32 {
        std::mem::take(&mut self.held_ms) as f32
    }

    /// A frame during pause (`Engine+0x24E4`) at program clock `now` with speed `game_speed`: program clock and time
    /// base as in [`SessionClock::frame_ms`], the rendering clock stands still (animation, messages), the water's pause
    /// time ([`SessionClock::pause_ms`]) advances by the frame time. Returns the frame time and the engine ticks of
    /// this frame. `GameEngine_Update` counts the ticks during pause too (`GameEngine_GameTick` runs, only without
    /// world tick; afterwards dialog script and ESC flag, O-152): game time `dt * (v + 10) / 20` in ticks of 50 ms, at
    /// most 2 per frame, the remainder then discarded as in [`SimHost::advance`]. The simulation's game-time remainder
    /// (interpolation) stays untouched.
    pub fn pause_frame(&mut self, now: u64, game_speed: u8) -> (f32, u32) {
        self.program_ms = now as f64;
        let last = self.last_now.replace(now);
        let dt_ms = last.map_or(0, |last| now.saturating_sub(last)) as f32;
        self.pause_ms += f64::from(dt_ms);
        self.pause_carry_ms += f64::from(dt_ms) * SimHost::game_time_factor(game_speed);
        let step = f64::from(opennml_sim::tick::STEP_MS);
        let mut ticks = 0;
        while self.pause_carry_ms >= step {
            if ticks == crate::game::sim_host::MAX_STEPS_PER_FRAME {
                self.pause_carry_ms = 0.0;
                break;
            }
            self.pause_carry_ms -= step;
            ticks += 1;
        }
        (dt_ms, ticks)
    }

    /// Clock of the water animation: the rendering clock plus the pause time (25 ms steps, `water_phases`).
    pub fn water_ms(&self) -> f64 {
        self.now_ms + self.pause_ms
    }

    /// A frame of the simulation alone (without map, e.g. `FakeSession`): number of engine ticks and the events.
    pub fn advance(&mut self, sim: &mut SimHost, now: u64, running: bool) -> (u32, Vec<SimEvent>) {
        let Some(dt_ms) = self.frame_ms(now, running) else {
            return (0, Vec::new());
        };
        let before = sim.state.steps;
        let events = sim.advance(f64::from(dt_ms));
        (sim.state.steps.wrapping_sub(before) as u32, events)
    }
}

/// Pointer and held keys of the map view. [`GameSession::input`] maintains them from the inputs; the viewer sets them
/// before each call from its own state (two input sources, dialog layer).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pointer {
    pub cursor: Option<(f64, f64)>,
    pub keys: HashSet<KeyCode>,
    /// Mouse movement with the right button held since the last frame (look of the free camera).
    pub look: Vec2,
    pub left: bool,
    pub right: bool,
}

impl Pointer {
    /// Shift held.
    pub fn shift(&self) -> bool {
        self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight)
    }

    /// Ctrl held.
    pub fn ctrl(&self) -> bool {
        self.keys.contains(&KeyCode::ControlLeft) || self.keys.contains(&KeyCode::ControlRight)
    }
}

/// Host modes that take clicks on the map (in the viewer: building placement, unpack cursor, targeting mode, headhunter
/// menu, attack mode, formation of the unit bar). The command bar sees every mouse event first: an open cursor of the
/// bar takes the clicks in the world and thereby replaces the modes; in `opennml-game` there are no others ([`NoModes`]
/// remains only as the empty remainder).
pub trait HostModes {
    /// Left button pressed on the map: `true` if a mode takes the click (placing on release instead of selecting).
    fn left_press(&mut self) -> bool {
        false
    }

    /// Left button released (the double-click is counted): `true` if the mode has placed the click.
    fn left_release(&mut self, _session: &mut GameSession) -> bool {
        false
    }

    /// Right-click at the press point: `true` if it only ends a mode.
    fn cancel_right_click(&mut self) -> bool {
        false
    }

    /// Attack mode (once): `true` if this right-click attacks.
    fn take_attack(&mut self) -> bool {
        false
    }

    /// Formation and dragging of the commands; the default is the formation `toolbar` chosen in the command bar of the
    /// session ([`ToolbarHost::bar`]). The viewer takes that of its unit bar.
    fn formation(&self, drag: Option<(i32, i32)>, toolbar: &UnitBar) -> FormationOrder {
        FormationOrder::new(toolbar, drag)
    }
}

/// No host modes.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoModes;

impl HostModes for NoModes {}

/// Plays a sound of the simulation ([`SoundRequest`]): message sound as 2D sound (flags `0x12`, noises,
/// `MessageSystem_PlayMessageSound` `0x00492740`); object sounds as 3D sound at the position: an action of the type's
/// sound database (flags per `action_flags`) or a sound GUID of the events (flags `0x32`, without the volume factor of
/// `SoundManager_PlayEvent3D`). If the database does not know the number, it stays silent. Combat sounds (`Object3d`)
/// bring their flags from the caller and are played without a fog check (re/combat-host 3, C-116).
pub fn play_sim_sound(audio: &mut AudioEngine, vfs: &Vfs, request: SoundRequest) {
    match request {
        SoundRequest::Message(sound) => {
            audio
                .sound
                .play_sound_guid_2d(vfs, sound, 0x12, Category::Sfx, false);
        }
        // `SoundManager_PlayEvent2D(name, 0x14, 1, 0)` (`0x00552FB0`, re/headhunter-robbing 7, D-238): headhunter
        // sounds for everyone.
        SoundRequest::Event2d { name } => {
            audio
                .sound
                .play_event_2d(vfs, &name, EVENT_2D_FLAGS, Category::Sfx, false);
        }
        // `SoundManager_PlayEvent3D(name, flags, position, 1)` (`0x00553420`): wading noise `SFX_WATERWALK<n>` of the
        // units (re/world-display W.3.4), without a fog check (D-273).
        SoundRequest::Event3d {
            name,
            flags,
            position,
        } => {
            audio
                .sound
                .play_event_3d(vfs, &name, flags, position, Category::Sfx);
        }
        SoundRequest::Object3d {
            action,
            flags,
            position,
            guid,
            ..
        } => {
            audio.sound.play_object_3d(
                vfs,
                action,
                flags,
                position,
                guid as i32,
                -1,
                Category::Sfx,
            );
        }
        SoundRequest::Object {
            guid,
            sound,
            position,
        } => match sound_kind(&audio.sound.db.action_names, sound) {
            SoundKind::Action => {
                let (flags, category) = opennml_audio::engine::action_flags(sound, false);
                audio
                    .sound
                    .play_object_3d(vfs, sound, flags, position, guid as i32, -1, category);
            }
            SoundKind::Guid => {
                if let Some(entry) = audio.sound.db.sound(sound).cloned() {
                    audio
                        .sound
                        .play_file_3d(vfs, &entry.path, 0x32, position, Category::Sfx);
                }
            }
        },
    }
}

/// Message 9/10 for each speed change of the turns, to the message bar (`GameEngine_CmdSetGameSpeed` `0x0042EF30`, flag
/// `0x03010000`, no location; E-294), like [`GameSession::change_game_speed`] in single player.
fn post_speed_messages(hud: &mut Hud, changes: &[SpeedChange], out: &mut Vec<SoundRequest>) {
    for change in changes {
        if let Some(id) = change.message() {
            hud.post(id, NO_TILE, SPEED_MESSAGE_FLAGS, out);
        }
    }
}

/// Direction of the mouse wheel: `Some(true)` upward (closer), `Some(false)` downward; purely horizontal scrolling (`y
/// == 0`) is ignored.
pub fn wheel_up(delta: MouseScrollDelta) -> Option<bool> {
    let y = match delta {
        MouseScrollDelta::LineDelta(_, y) => y,
        MouseScrollDelta::PixelDelta(p) => p.y as f32,
    };
    (y != 0.0).then_some(y > 0.0)
}

/// Point of the right-click from press and release point: the original chooses the action on press and triggers it on
/// release (`0x00453790`, `0x00457C50`), with no threshold for the mouse travel. Only the free camera (viewer) rotates
/// with the right button: there it is no click from 3 pixels on.
pub fn right_click_point(
    start: (f64, f64),
    end: (f64, f64),
    free_camera: bool,
) -> Option<(f64, f64)> {
    let turned = (start.0 - end.0).abs() >= 3.0 || (start.1 - end.1).abs() >= 3.0;
    (!(free_camera && turned)).then_some(start)
}

/// Target point of a target button or right-click (re/game-ui N.5.8): object targets 529/533 at the object's position
/// with height, otherwise the ground point with `y = 0` (terrain decal).
fn decal_for(button: i32, object_pos: Option<[f32; 3]>, ground: Vec3) -> TargetDecal {
    let pos = match object_pos {
        Some(p) if DECAL_OBJECT_BUTTONS.contains(&button) => p,
        _ => [ground.x, 0.0, ground.z],
    };
    TargetDecal { button, pos }
}

/// Inputs of a frame for both cameras: keys, screen edge and look; with `locked` (not [`InputGate::player_camera`]:
/// input lock or running camera of the scripts) the game camera stands still (`GameEngine_UpdateCamera` `0x0043C7DC`:
/// keyboard and mouse camera off), the viewer's free camera does not. The camera keys come per `mode`
/// ([`camera_input`]): for the viewer from `pointer.keys`, for the original from the table `held_vk` of the host with
/// the `KD_` keys of `hotkeys`. If an input field has the focus (`typing`, chat window open), the original's camera
/// keys have no effect (no scrolling, zooming, rotating; O-393); the screen edge and `KD_CHECK_LBUTTON` remain.
pub fn frame_input(
    pointer: &Pointer,
    held_vk: &[u8],
    screen: (u32, u32),
    locked: bool,
    typing: bool,
    mode: &CameraKeys,
    hotkeys: &Hotkeys,
) -> FrameInput {
    let edge = pointer
        .cursor
        .map_or(Vec2::ZERO, |c| edge_direction(c, screen));
    FrameInput {
        camera: if locked {
            CameraInput::default()
        } else {
            let mut camera = camera_input(&pointer.keys, held_vk, edge, mode, hotkeys);
            if typing && matches!(mode, CameraKeys::Original { .. }) {
                camera.keys = Vec2::ZERO;
                camera.zoom = 0;
                camera.pitch = 0;
                camera.yaw = 0;
            }
            camera
        },
        free: free_input(&pointer.keys, pointer.look),
    }
}

/// Inputs of the map view for one frame:
///
/// - [`CameraKeys::Viewer`]: held keys `keys` - arrows scroll, Page Down/Up zoom (down = farther, opposite to the
///   original), Home/End tilt, Insert/Delete rotate; `held_vk` and `hotkeys` stay unused.
/// - [`CameraKeys::Original`] (`GameEngine_UpdateCamera` `0x0043C7C0`, query `FUN_004143A0` on the table `held_vk`):
///   `KD_CHECK_LBUTTON` held -> nothing, not even the edge, and [`CameraInput::halt`] (`0x0043C7EA`); otherwise
///   `KD_CAM_MOVE_*` scroll, `KD_CAM_YROT_CLOCKWISE`/`_ANTICLOCKWISE` rotate `Camera_Yaw(-/+dt * 0.001)` only without
///   `lock_camera` (`0x0043CDAD`...`0x0043CE29`), `KD_CAM_XROT_DOWN`/`_UP` tilt `-/+0.0314` (`0x0043CE44`...; the file
///   does not have them, so no tilting, O-305), `KD_CAM_ZOOM_IN`/`_OUT` zoom `Camera_ZoomStep(-/+...)`, i.e. closer or
///   farther (`0x0043CEB2`/`0x0043CEEE`). The code does not query `KD_SPEEDUP_GAME` (TAB) (O-305). `KD_CAM_FREEHAND`
///   (middle button) held -> [`CameraInput::freehand`] with movement 0; [`freehand_step`] sets the movement (O-331).
///   With the chat open it remains (a mouse button, no typing).
pub fn camera_input(
    keys: &HashSet<KeyCode>,
    held_vk: &[u8],
    edge: Vec2,
    mode: &CameraKeys,
    hotkeys: &Hotkeys,
) -> CameraInput {
    match *mode {
        CameraKeys::Viewer => {
            let axis = |plus: KeyCode, minus: KeyCode| {
                keys.contains(&plus) as i8 - keys.contains(&minus) as i8
            };
            CameraInput {
                keys: Vec2::new(
                    axis(KeyCode::ArrowUp, KeyCode::ArrowDown) as f32,
                    axis(KeyCode::ArrowRight, KeyCode::ArrowLeft) as f32,
                ),
                edge,
                zoom: axis(KeyCode::PageDown, KeyCode::PageUp),
                pitch: axis(KeyCode::Home, KeyCode::End),
                yaw: axis(KeyCode::Insert, KeyCode::Delete),
                halt: false,
                freehand: None,
            }
        }
        CameraKeys::Original { lock_camera } => {
            let down = |name: &str| hotkeys.keydef_down(name, held_vk);
            if down("KD_CHECK_LBUTTON") {
                return CameraInput {
                    halt: true,
                    ..CameraInput::default()
                };
            }
            let axis = |plus: &str, minus: &str| down(plus) as i8 - down(minus) as i8;
            CameraInput {
                keys: Vec2::new(
                    axis("KD_CAM_MOVE_FORWARD", "KD_CAM_MOVE_BACKWARD") as f32,
                    axis("KD_CAM_MOVE_RIGHT", "KD_CAM_MOVE_LEFT") as f32,
                ),
                edge,
                zoom: axis("KD_CAM_ZOOM_OUT", "KD_CAM_ZOOM_IN"),
                pitch: axis("KD_CAM_XROT_UP", "KD_CAM_XROT_DOWN"),
                yaw: if lock_camera {
                    0
                } else {
                    axis("KD_CAM_YROT_ANTICLOCKWISE", "KD_CAM_YROT_CLOCKWISE")
                },
                halt: false,
                // The session sets the movement ([`freehand_step`]).
                freehand: down("KD_CAM_FREEHAND").then_some(Vec2::ZERO),
            }
        }
    }
}

/// Inputs of the free camera (WASD, Q/E, Shift).
pub fn free_input(keys: &HashSet<KeyCode>, look: Vec2) -> FreeInput {
    let axis = |plus: KeyCode, minus: KeyCode| {
        (keys.contains(&plus) as i8 - keys.contains(&minus) as i8) as f32
    };
    FreeInput {
        movement: Vec3::new(
            axis(KeyCode::KeyW, KeyCode::KeyS),
            axis(KeyCode::KeyD, KeyCode::KeyA),
            axis(KeyCode::KeyE, KeyCode::KeyQ),
        ),
        look,
        fast: keys.contains(&KeyCode::ShiftLeft) || keys.contains(&KeyCode::ShiftRight),
    }
}

/// `KD_CAM_FREEHAND` of a frame (`0x0043CF1D`...`0x0043D095`, O-331): If the camera block runs (`runs`: no lock, no
/// held left button, camera of the original) and the key is held, it sets the movement `position - anchor` in whole
/// pixels into [`CameraInput::freehand`], resets `cursor` to `anchor` and returns this position for `SetCursorPos`;
/// `anchor` stays. Without the key it moves `anchor` to the position (`GE+0x252C/+0x2530` at the end of the block). If
/// the block does not run, `anchor` stays put (O-434). Without a position (pointer outside the window) nothing happens.
pub fn freehand_step(
    camera: &mut CameraInput,
    cursor: &mut Option<(f64, f64)>,
    anchor: &mut Option<(f64, f64)>,
    runs: bool,
) -> Option<(f64, f64)> {
    let held = camera.freehand.take().is_some();
    let at = cursor.filter(|_| runs)?;
    if !held {
        *anchor = Some(at);
        return None;
    }
    let from = anchor.unwrap_or(at);
    camera.freehand = Some(Vec2::new(
        (at.0.floor() - from.0.floor()) as f32,
        (at.1.floor() - from.1.floor()) as f32,
    ));
    *cursor = Some(from);
    *anchor = Some(from);
    Some(from)
}

/// Inputs of a frame for both cameras of the map view.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameInput {
    pub camera: CameraInput,
    pub free: FreeInput,
}

/// Camera of the map view for a frame of `dt_ms` milliseconds (without GPU, testable): free camera with WASD and look,
/// otherwise original camera with keys, screen edge and mouse-wheel transition.
pub fn advance_camera(
    controller: &mut CameraController,
    camera: &mut GameCamera,
    free: Option<&mut FreeCamera>,
    terrain: &Terrain,
    input: FrameInput,
    controls: &Controls,
    dt_ms: f32,
) {
    match free {
        Some(free) => free.update(input.free, dt_ms),
        None => controller.update(camera, terrain, input.camera, controls, dt_ms),
    }
}

/// Objects of the ground markers of a frame ([`ground_marker_objects`]); the target highlight expires beforehand or
/// ends with its object (`0x0047FC70`). Without a simulation there are none.
pub fn frame_markers(
    map: &MapView,
    selection: &[ObjectId],
    highlight: &mut Option<TargetHighlight>,
    script: Option<ObjectId>,
    hovered: Option<ObjectId>,
    now_ms: f32,
) -> Vec<ObjectId> {
    let Some(sim) = map.sim.as_ref() else {
        *highlight = None;
        return Vec::new();
    };
    retain_highlight(highlight, &sim.state, now_ms);
    // The scripts' marker (152) is in the same field `GameWorld+0x318`, only without expiry; a newer target of the
    // player has replaced it ([`replace_script_highlight`]). For the rings only the object counts.
    let script = script
        .filter(|_| highlight.is_none())
        .map(|id| TargetHighlight {
            id,
            start_ms: now_ms,
        });
    ground_marker_objects(
        &sim.state,
        selection,
        highlight.as_ref().or(script.as_ref()),
        hovered,
        |o| sim.fog_selectable(o),
    )
}

/// A target action of the player (right-click, `sim_action`) has set the target highlight anew (`after != before`): it
/// replaces the scripts' marker 152, since both are in the field `GameWorld+0x318` (E-228).
fn replace_script_highlight(
    before: Option<TargetHighlight>,
    after: Option<TargetHighlight>,
    script: &mut Option<ObjectId>,
) {
    if after.is_some() && after != before {
        *script = None;
    }
}

/// Distributes the presentation events of a frame to the speech output; `sim` supplies the line that the simulation
/// still speaks after the frame (decides on an `InfoFrameClose`, [`SpeechAudio::flush`]). As a separate function with
/// `&mut dyn MenuAudio` so the wiring can be checked without a real `AudioEngine`.
fn speech_step(
    speech: &mut SpeechAudio,
    sim: Option<&SimHost>,
    events: &[SimEvent],
    audio: &mut dyn MenuAudio,
    vfs: &Vfs,
) {
    let running = sim
        .and_then(|s| s.state.script.mission.speech.line)
        .map(|line| line.text);
    speech.on_events(events, running, audio, vfs);
}

/// Scene fade and target highlight of a presentation event in the host: 145/146 set the fade (`FUN_004B96A0`,
/// re/campaign-ui D6.6); 152/155 and 147/148 write the field `GameWorld+0x318` (`FUN_00475F70`, E-228), which also
/// carries the highlight of a right-click, and thereby also replace or clear it (the scripts' marker itself is held by
/// [`ScriptPresentation::apply_host`]).
fn scene_event(
    fade: &mut SceneFade,
    target_highlight: &mut Option<TargetHighlight>,
    event: &Presentation,
) {
    match *event {
        Presentation::Fade { into, ms } => fade.set(ms, into),
        Presentation::Highlight { .. } | Presentation::Cinemascope { .. } => {
            *target_highlight = None
        }
        _ => {}
    }
}

/// Info windows of a presentation event in the host (re/campaign-ui D6.5): 168 (`InfoFrame`, `0x004434E0`) opens a
/// window, 213 (`InfoSpeechFrame`, `0x00443A20`) a speech window, both at the head and in place of an open window of
/// the same text ID ([`InfoFrames::open`]); `InfoFrameClose` (169, pre-closing of 168/213, end of speech from slot 4;
/// `FUN_004401C0`) closes exactly this ID. The simulation keeps the same table (`presentation.info_frames`, E-249).
fn info_frame_event(frames: &mut InfoFrames, event: &Presentation) {
    let (text, kind) = match *event {
        Presentation::InfoFrame { text, x, y, style } => {
            (text, InfoFrameKind::Window { x, y, style })
        }
        Presentation::InfoSpeechFrame {
            text,
            slot,
            portrait,
            frame,
        } => (
            text,
            InfoFrameKind::Speech {
                slot,
                portrait,
                frame,
            },
        ),
        Presentation::InfoFrameClose { text } => return frames.close(text),
        _ => return,
    };
    frames.open(text, InfoFrameParams::of(&kind, text));
}

/// Screen text of a presentation event in the host (re/campaign-ui D6.12): `n5` (`ScreenText{text, seconds}`,
/// `FUN_00492C50`) queues a text of kind 2 in white, for `seconds * 1000` ms of game time ([`ScreenTexts::push`]).
fn screen_text_event(texts: &mut ScreenTexts, event: &Presentation) {
    if let Presentation::ScreenText { text, seconds } = *event {
        texts.push(
            screen_text::kind::MIDDLE,
            text,
            0x00FF_FFFF,
            seconds.wrapping_mul(1000),
        );
    }
}

/// Kind of a remembered info window from the presentation state ([`InfoFrameMemory`], loading).
fn info_frame_kind(memory: &InfoFrameMemory) -> InfoFrameKind {
    match *memory {
        InfoFrameMemory::Window { x, y, style } => InfoFrameKind::Window { x, y, style },
        InfoFrameMemory::Speech {
            slot,
            portrait,
            frame,
        } => InfoFrameKind::Speech {
            slot,
            portrait,
            frame,
        },
    }
}

/// A running match: map view with simulation and the state of the frame loop.
pub struct GameSession {
    /// Map view; it contains the simulation ([`GameSession::sim`]).
    pub map: MapView,
    /// Camera control of the original (keys, edge, mouse-wheel transition).
    pub controller: CameraController,
    /// Selected simulation objects.
    pub selection: Selection,
    /// Selection sound and command acknowledgment (lock, counter, history; also without sound).
    pub voices: Voices,
    /// Times of the last release per mouse button on the map (double-click).
    pub clicks: ClickClocks,
    /// Pixel at the press of the left mouse button on the map (selection box).
    pub select_start: Option<(f64, f64)>,
    /// Pointer position at the press of the right mouse button (click action instead of rotating).
    pub right_start: Option<(f64, f64)>,
    /// Formation dragging from the terrain point at the press of the right mouse button (only with the game camera;
    /// mouse-motion handler `0x00453790`, re/groups-formations 12.6).
    pub right_drag: Option<FormationDragTracker>,
    /// Target highlight of the last right-click or `sim_action` on an object (rendering only).
    pub target_highlight: Option<TargetHighlight>,
    /// Objects with a ground marker in the last frame (state snapshot).
    pub ground_markers: Vec<ObjectId>,
    /// Pointer and held keys. Writers: in `opennml-game` only [`GameSession::input`]; the viewer sets them before each call
    /// from its own state (`sync_session`: two input sources, dialog layer).
    pub pointer: Pointer,
    /// Rendering options (aspect ratio, fog, animation ...), taken over at start. The viewer sets aspect ratio, fog and
    /// animation before each call (`sync_session`, keys F3 to F5); `opennml-game` updates `unit_response` (with
    /// `voices.unit_response`) each frame from the settings (`LiveSession::apply_settings`).
    pub view: ViewOptions,
    /// Camera control and double-click time (`Engine.ini`), the default at start. `opennml-game` sets them at start and
    /// each frame from the settings (`LiveSession::apply_settings`, like the original from `g_pEngineConfig`); the
    /// viewer before each call (`sync_session`).
    pub controls: Controls,
    /// Size of the image (surface): aspect ratio and visibility for weather and effects in [`GameSession::advance_by`].
    /// Writers: `start`/`resume_from` (start size), [`GameSession::resize`] and [`GameSession::render`] (the size of
    /// the target); the viewer before each call (`sync_session`, surface size).
    pub size: (u32, u32),
    /// Size of the area in which the pointer lies (ray tests, edge scrolling). Writers: `start`/`resume_from` (start
    /// size), [`GameSession::resize`], [`GameSession::input`]; the viewer before each call (`sync_session`, window
    /// pixels, which can differ from the surface).
    pub screen: (u32, u32),
    /// Time base of the wrapper [`GameSession::advance`] and rendering clock ([`SessionClock::now_ms`]).
    pub clock: SessionClock,
    /// Game UI: window tree, message bar in game time, action stack, status bar. Runs with the match in
    /// [`GameSession::advance_by`]; loading and restart clear bar and stack.
    pub hud: Hud,
    /// The game UI is drawn ([`GameSession::hud_cmds`]): always in `opennml-game`, in the viewer per the switch `--hud` or
    /// the command `hud` (`sync_session`). Without it [`GameSession::hud_cmds`] draws nothing; events and tick of the
    /// bar still run.
    pub hud_enabled: bool,
    /// Object under the pointer (picker `FUN_0047A210(mouse, 0)`, [`MapView::pick_target`]): computed once per frame in
    /// [`GameSession::advance_by`] after the simulation step and used by [`GameSession::render`] for the ground markers
    /// (one value per frame). `None` without a pointer.
    pub hovered: Option<ObjectId>,
    /// Events to the command bar (`{8}`, `{0x10, n}`, hero and headhunter row, `{0x29}`); built each frame in
    /// [`GameSession::advance_by`], `frame_events` hands them to the bar. Plus the state of the bar commands (chosen
    /// formation).
    pub toolbar_host: ToolbarHost,
    /// Last pressed button of the bar (`GameEngine+0x4E0`, script condition 38): `BUTTONMSG`, for hero, multi-selection
    /// and occupant buttons the HID. Host state, not in the savegame; `None` at session start, restart and load
    /// (deviation from the original, O-269: the original has no initial value). Only [`GameSession::button_pressed`]
    /// writes it.
    pub last_button: Option<i32>,
    /// The match ran in the last [`GameSession::advance`] (no game menu, no saving, no loading). Only then does the
    /// command bar take input; the viewer calls `advance_by` directly and leaves it on.
    pub running: bool,
    /// Building preview of the open bar cursor (building, tent, wall pieces) from the last frame step; empty without a
    /// cursor. It takes precedence over a preview of the viewer ([`GameSession::mark_ground`]).
    pub toolbar_previews: Vec<PlacementPreview>,
    /// Target-point decal of the bar (O-237): button, position, end in program time.
    pub target_decal: Option<ActiveTargetDecal>,
    /// Key layout for `[KEYID]` in the tooltip: default `defaultkeys.ini`, `opennml-game` each frame from the settings
    /// (`LiveSession::apply_settings`).
    pub key_map: Vec<(i32, String)>,
    /// Previews or decals of the bar are in the renderer (clear once when they go away).
    pub(crate) previews_shown: bool,
    pub(crate) decals_shown: bool,
    /// Image width for which the hero and headhunter row is built.
    pub(crate) toolbar_width: Option<i32>,
    /// Script presentation: light, weather, music, fog, cinemascope (bars off, speed), lock, requests to the flow;
    /// [`GameSession::advance_by`] applies the events, loading and restart restore it from the presentation state.
    pub presentation: ScriptPresentation,
    /// Game type of the match (`GE+0x108`, `Engine+0x108`): -1 campaign or direct start (default, also in the viewer),
    /// 0 free play, -2 menu level. `GameFlow` sets it each frame (`SessionView::set_match_info`), loading takes it from
    /// the host part (`HostPart::game_type`). Only in the campaign does the cinemascope show bars (`0x004309BE`,
    /// O-350).
    pub game_type: i32,
    /// Game menu, diplomacy or quest menu open (menu state `Engine+0x2CCC != 0`); `GameFlow` sets it each frame
    /// (`SessionView::set_match_info`). For [`GameSession::input_gate`].
    pub menu_open: bool,
    /// Source of the camera keys ([`camera_input`]): default [`CameraKeys::Viewer`]; `opennml-game` sets
    /// [`CameraKeys::Original`] each frame with `LockCamera` from the settings (`LiveSession::apply_settings`).
    pub camera_keys: CameraKeys,
    /// Table of the host's held keys (`0x0066FA60+0x9C`, `MenuHost::held_vk`, including mouse buttons) for the camera's
    /// `KD_` queries; `opennml-game` sets it each frame. The viewer leaves it empty (it does not query `KD_`).
    pub held_vk: Vec<u8>,
    /// An input field has the keyboard focus (chat window open; `MenuApp::text_focus`, `opennml-game` sets it each frame
    /// together with [`Self::held_vk`]): the original's camera keys then have no effect (O-393).
    pub text_focus: bool,
    /// Position of the pointer in the previous camera frame (`GE+0x252C/+0x2530`, O-331): reference of the free mouse
    /// (`KD_CAM_FREEHAND`). As in the original it is only updated when the camera block runs (not under lock and with
    /// the left button held, O-434). Not in the savegame (input state of the host).
    freehand_anchor: Option<(f64, f64)>,
    /// The pointer is to return to this window position (`SetCursorPos` at `0x0043D083`): `KD_CAM_FREEHAND` holds it in
    /// place. `opennml-game` takes it after each frame ([`GameSession::take_cursor_warp`]).
    cursor_warp: Option<(f64, f64)>,
    /// The pointer is the 3D pointer in the original (`HardwareCursor 0`, `Cursor3D+0x18 == 0`): only then does the
    /// build cursor tint it (O-394); `opennml-game` sets it each frame from the settings. Default false (viewer).
    pub software_pointer: bool,
    /// Key data of the installation (`keymap.ini` for the camera, [`Hotkeys::load`] at construction).
    pub hotkeys: Hotkeys,
    /// Control groups per player slot (`Player+0x23C`, `+0x354`): selecting and saving happens in those of the active
    /// slot. Host state without command and without effect on the checksum; in the host part of the savegame
    /// (`HostPart::control_groups`, without the last group). Sync with the selection: [`GameSession::sync_groups`].
    pub groups: [ControlGroups; PLAYER_SLOTS],
    /// Spatial clusters of the selection for the camera cycle (`GameWorld+0x390`); rebuilt on every change of the
    /// selection (deviation from the original, O-315). Not in the savegame.
    pub clusters: SelectionClusters,
    /// Last press of `selectnextguid` with double-tap deadline (`DAT_00671368`/`DAT_00671364`, program clock). Volatile
    /// as in the original: not in the savegame, 0/0 after loading.
    pub next_guid_tap: GuidTap,
    /// Pause with two bits (`Engine+0x24E4`/`+0x24E5`): `togglepause` ([`GameSession::toggle_pause`]), loss of focus
    /// ([`GameSession::focus_lost`]), giving up (`GameFlow`). Local: no command, not in the savegame; a new or loaded
    /// session begins without pause. With [`PauseState::sim`] [`GameSession::advance`] only does the pause step
    /// (O-317).
    pub pause: PauseState,
    /// Engine ticks of the last [`GameSession::advance`]: with the match running the simulation steps, in pause the
    /// ticks without world tick ([`SessionClock::pause_frame`]), otherwise 0. The flow uses it to open the game menu
    /// after ESC (O-152), also in pause.
    pub ticks: u32,
    /// Mouse pointer of the match ([`crate::game::pointer`]): ID and variant, set on movements in the frame step
    /// ([`GameSession::pointer_step`]), when a right-click is sent, and in the game menu. Not in the savegame; a new or
    /// loaded session begins with 7/0.
    pub mouse_pointer: PointerState,
    /// Scene fade of the scripts 145/146 (re/campaign-ui D6.6): set in [`GameSession::advance_by`], ticked in
    /// [`GameSession::tick_presentation`], drawn via [`GameSession::fade_cmds`]. Session start, restart and loading
    /// reset it (`FUN_004BAA10`, O-376); a state after 146 and before 145 is black after loading, without a fade
    /// (deviation from the original). Host state, not in the savegame (`PresentationMemory::faded_out` carries it).
    pub scene_fade: SceneFade,
    /// Running dialog line and tip sound (re/campaign-ui D6.7/D6.8): [`GameSession::advance_by`] distributes the events
    /// `Speech`, `SpeechEnd`, `InfoFrameClose` and `Tip` to it. Host state, not in the savegame: loading and restart
    /// forget the line without a new sound (deviation from the original), [`GameSession::stop_speech`] fades it out
    /// beforehand.
    pub speech: SpeechAudio,
    /// Open info windows 168/213 on the desktop (re/campaign-ui D6.5), head first: [`GameSession::advance_by`] and
    /// [`GameSession::present_by_hand`] open and close them from the presentation events, drawn via
    /// [`GameSession::desktop_cmds`]. Empty at session start; loading and restart rebuild them from
    /// `presentation.info_frames` in text-ID order (deviation from the original, E-249). Host state, the table in the
    /// state carries it. The windows measure themselves on first drawing and hold texture IDs of the UI layer
    /// (`InfoFrames`): the hosts draw them every frame with the same layer as long as the session exists.
    pub info_frames: InfoFrames,
    /// Index of the idle workers per slot (`m_WaitingWorkerIndex`): button or key 576 ([`GameSession::select_idle`])
    /// increments it without a command, so it lives with the host (deviation from the original); in the host part
    /// of the savegame (`HostPart::idle_worker_index`). New match 0.
    pub idle_worker_index: [i32; PLAYER_SLOTS],
    /// Chat and tribute masks of all slots: start [`Masks::initial`], loading from the host part (`HostPart::masks`).
    /// The signal and the chat read them, diplomacy sets them ([`crate::game::diplomacy::apply`]).
    pub masks: Masks,
    /// Requests to the flow since the last [`GameSession::take_ui_requests`].
    ui_requests: Vec<UiRequest>,
    /// Display names of the slots for the chat lines (`Player+0xC`), set by the flow each frame before the steps
    /// (`game_flow::player_display_name`, needs the texts); `None` without a flow (viewer): then the raw name.
    pub player_names: Option<[String; PLAYER_SLOTS]>,
    /// State of the match's random map for the control command `random_map` (from [`MapLoad::random_state`]); `None`
    /// for fixed maps and for the viewer's random map ([`MapLoad::from_scene`]).
    random_map: Option<RandomMapState>,
    /// Generated map of the match's random map for the savegame (from [`MapLoad::random_map`]; a restart builds it
    /// anew); `None` for fixed maps and for the viewer's random map.
    generated_map: Option<Box<opennml_randommap::RandomMap>>,
    /// Turn driver of the network match (set by `GameFlow` at network start); `None` in single player and in the
    /// viewer. In the network it computes the ticks ([`GameSession::advance_by`]), and pause, speed and loss of focus
    /// go through it ([`GameSession::toggle_pause`], [`GameSession::change_game_speed`], [`GameSession::focus_lost`]).
    pub net: Option<NetDriver>,
    /// Control entries, dropouts and failure of the network match for the flow ([`GameSession::take_net_frame`]).
    net_frame: NetFrame,
    /// Offset of the network time to the program clock (`opennml-game`: the minimized phases, `NetClock`); the driver
    /// gets `now + offset`, the same time as from the pumps of the host. 0 in the viewer.
    net_time_offset_ms: u64,
}

/// Request of the match to the flow (`GameFlow`), which picks it up right after a key command or input
/// ([`GameSession::take_ui_requests`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiRequest {
    /// Button 579 "Menu (ESC)" (`0x0044C040` -> `FUN_0042E150`): open the game menu, only in menu state 0.
    GameMenu,
    /// Key 572 (`SHIFT D`) and button 580 (`0x0044C040` -> `FUN_0042E060(1)`): toggle the diplomacy dialog; nothing
    /// under the input lock (`MenuApp::toggle_diplomacy`).
    ToggleDiplomacy,
}

impl GameSession {
    /// Loads the level `level` and starts the match: `progress` receives the phases 0, 40, 70, 100; `setup` builds the
    /// setup from the just loaded rules (`to_match_setup` in `opennml-game`, the settings in the viewer); `size` is the
    /// image size at start (like a first [`GameSession::resize`]).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        gpu: &SessionGpu,
        vfs: &Vfs,
        level: &str,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        view: &ViewOptions,
        size: (u32, u32),
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<Self> {
        let source = MapSource::Level(level.to_string());
        Self::start_source(gpu, vfs, &source, setup, game_speed, view, size, progress)
    }

    /// Like [`GameSession::start`] with the map from `source`: a fixed map from its `.lvl` or a random map from its
    /// description ([`MapLoad::open`]).
    #[allow(clippy::too_many_arguments)]
    pub fn start_source(
        gpu: &SessionGpu,
        vfs: &Vfs,
        source: &MapSource,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        view: &ViewOptions,
        size: (u32, u32),
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<Self> {
        progress(LoadPhase::Start);
        let mut load = MapLoad::open(vfs, source)?;
        progress(LoadPhase::LevelRead);
        load.start_sim(vfs, &view.language_tag, game_speed, |rules| setup(rules))?;
        load.place_objects(vfs);
        progress(LoadPhase::SimReady);
        Self::from_load(gpu, vfs, load, view, size, progress)
    }

    /// Session around an already loaded level `load` (last step of [`GameSession::start`]): GPU data
    /// ([`MapView::build`]), then phase 100 to `progress`. The viewer's random map comes in this way without a
    /// simulation ([`MapLoad::from_scene`]); `size` as for [`GameSession::start`].
    pub fn from_load(
        gpu: &SessionGpu,
        vfs: &Vfs,
        mut load: MapLoad,
        view: &ViewOptions,
        size: (u32, u32),
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<Self> {
        let random_map = load.random_state.take();
        let generated_map = load.random_map.take();
        let map = MapView::build(gpu.device, gpu.queue, gpu.format, vfs, load, view)?;
        progress(LoadPhase::RenderReady);
        let mut session = Self::new(vfs, map, view, size);
        session.random_map = random_map;
        session.generated_map = generated_map;
        Ok(session)
    }

    /// Resumes a saved match: map of the savegame ([`MapSource::of_save`]: a random map from the generated map in the
    /// file, without the generator), speed from the host part, then the state ([`GameSaveFile::prepare`] against the
    /// loaded rules, otherwise an error) and the camera (eye, target, view angle; free camera off); `size` as for
    /// [`GameSession::start`].
    pub fn resume_from(
        gpu: &SessionGpu,
        vfs: &Vfs,
        mut save: GameSaveFile,
        view: &ViewOptions,
        size: (u32, u32),
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<Self> {
        progress(LoadPhase::Start);
        let mut load = MapLoad::open(vfs, &MapSource::of_save(&save))?;
        progress(LoadPhase::LevelRead);
        let setup = save.sim.start.setup.clone();
        load.start_sim(vfs, &view.language_tag, save.host.game_speed, |_| Ok(setup))?;
        let sim = load
            .sim
            .as_mut()
            .context("savegame: the map has no loadable rules")?;
        save.prepare(&sim.rules)?;
        let GameSaveFile {
            sim: state, host, ..
        } = save;
        sim.load(state, host.log.clone());
        load.place_objects(vfs);
        progress(LoadPhase::SimReady);
        let random_map = load.random_state.take();
        let generated_map = load.random_map.take();
        let map = MapView::build(gpu.device, gpu.queue, gpu.format, vfs, load, view)?;
        let mut session = Self::new(vfs, map, view, size);
        session.random_map = random_map;
        session.generated_map = generated_map;
        // The game type before rebuilding the presentation: the cinemascope bars exist only in the campaign
        // (`GameFlow::started` sets it once more afterwards).
        session.set_game_type(host.game_type);
        // As after `sim_load` of the viewer: trajectories, fires and effects from the loaded state. The message bar is
        // not in the savegame (re/game-ui N.7.4): empty.
        session.map.reset_sim_view(gpu.device, gpu.queue, vfs);
        session.hud.clear();
        host.camera.apply(&mut session.map.camera);
        session.map.free = None;
        // Presentation from the presentation state and the remembered speed of the host part (deviation from the
        // original); the music follows in the first frame with sound.
        session.restore_presentation(gpu.queue, vfs, host.cinemascope_speed);
        // Scripts' camera with backup, moves, rotation, binding and open cut (deviation from the original); it keeps
        // running with its remaining values (`FUN_00444C60`).
        session.presentation.camera = host.script_camera;
        // Control groups of all slots (unchecked on loading; stale and foreign IDs remain, O-311); the last group is
        // -1, the selection empty (as in the original, O-312).
        session.groups = std::array::from_fn(|slot| ControlGroups {
            next_guid_index: host.next_guid_index.get(slot).cloned().unwrap_or_default(),
            ..host
                .control_groups
                .get(slot)
                .map_or_else(ControlGroups::default, ControlGroups::from_save)
        });
        // Index of the idle workers and masks (unchecked on loading).
        session.idle_worker_index = host.idle_worker_index;
        session.masks = host.masks;
        // Fog switch `m_FogShowMode` (unchecked on loading) after `restore_presentation`: the original has a single
        // variable that it saves with the world; the saved value takes precedence over the script state
        // (`presentation.fog_show_all`), e.g. after `rainbowring`.
        if let Some(sim) = session.sim_mut() {
            sim.set_fog_show_mode(host.fog_show_mode);
        }
        progress(LoadPhase::RenderReady);
        Ok(session)
    }

    /// Session around the loaded map; the game UI is built here from the switches and the start size ([`Hud::load`]).
    fn new(vfs: &Vfs, map: MapView, view: &ViewOptions, size: (u32, u32)) -> Self {
        let mut hud = Hud::load(vfs, &map.notices, &view.hud, screen_of(size));
        // Minimap from the level (`Minimap_Construct` `0x004E4B00` at loading): also without `IsMap`, since its state
        // (pings) keeps running without a window; it is only drawn with nodes.
        hud.minimap = Some(crate::game::minimap::Minimap::new(
            crate::game::minimap::MinimapTerrain::of_level(&map.scene),
        ));
        Self {
            map,
            controller: CameraController::default(),
            selection: Selection::default(),
            voices: Voices::new(view.unit_response),
            clicks: ClickClocks::default(),
            select_start: None,
            right_start: None,
            right_drag: None,
            target_highlight: None,
            ground_markers: Vec::new(),
            pointer: Pointer::default(),
            view: view.clone(),
            controls: Controls::default(),
            size,
            screen: size,
            clock: SessionClock::default(),
            hud,
            hud_enabled: true,
            hovered: None,
            toolbar_host: ToolbarHost::new(),
            last_button: None,
            running: true,
            toolbar_previews: Vec::new(),
            target_decal: None,
            key_map: opennml_ui::screens::load_default_keys(vfs),
            previews_shown: false,
            decals_shown: false,
            toolbar_width: None,
            presentation: ScriptPresentation::new(view),
            game_type: -1,
            menu_open: false,
            camera_keys: CameraKeys::Viewer,
            held_vk: Vec::new(),
            text_focus: false,
            freehand_anchor: None,
            cursor_warp: None,
            software_pointer: false,
            hotkeys: Hotkeys::load(vfs),
            groups: Default::default(),
            clusters: SelectionClusters::default(),
            next_guid_tap: GuidTap::default(),
            pause: PauseState::default(),
            ticks: 0,
            mouse_pointer: PointerState::default(),
            scene_fade: SceneFade::default(),
            speech: SpeechAudio::default(),
            info_frames: InfoFrames::new(),
            idle_worker_index: [0; PLAYER_SLOTS],
            masks: Masks::initial(),
            ui_requests: Vec::new(),
            player_names: None,
            random_map: None,
            generated_map: None,
            net: None,
            net_frame: NetFrame::default(),
            net_time_offset_ms: 0,
        }
    }

    /// Sets the offset of the network time to the program clock (`NetClock::offset_ms`): the turn driver gets
    /// `now + offset_ms` in [`GameSession::advance`], like the pumps of the host while the window is minimized (O-465).
    pub fn set_net_time_offset(&mut self, offset_ms: u64) {
        self.net_time_offset_ms = offset_ms;
    }

    /// Sets the game type of the match (`Engine+0x108`, [`GameSession::game_type`]) and tells the game UI: the corner
    /// buttons of the minimap have the diplomacy button 580 only for game type >= 0 ([`Hud::set_game_type`]).
    pub fn set_game_type(&mut self, game_type: i32) {
        self.game_type = game_type;
        self.hud.set_game_type(game_type);
    }

    /// Requests to the flow since the last call ([`UiRequest`]); empty afterwards.
    pub fn take_ui_requests(&mut self) -> Vec<UiRequest> {
        std::mem::take(&mut self.ui_requests)
    }

    /// Campaign or direct start (`GE+0x108 == -1`): cinemascope with bars (O-350).
    pub fn campaign(&self) -> bool {
        self.game_type == -1
    }

    /// A button of the bar was pressed (`0x0042ED10` as the first step of the click path `FUN_00458130`, for each
    /// `ToolbarOut::ButtonPressed` in output order; the headhunter button reports twice, O-269): `value` becomes
    /// [`GameSession::last_button`] (host, report `ui_state`) and goes as host command `ScriptButton` to the
    /// simulation's mirror (`GE+0x4E0`, condition 38; O-246, E-229). [`SimHost::press_button`] only sends the command
    /// while a script with 35-38 is running, otherwise the command log stays free. The bar is the only sender in the
    /// host; `ScriptButton` via the control channel (`sim_command`) remains possible alongside.
    pub fn button_pressed(&mut self, value: i32) {
        self.last_button = Some(value);
        if let Some(sim) = self.sim_mut() {
            sim.press_button(value);
        }
    }

    /// The just started match becomes the replay of the recording `commands` (`Game_FinishLevelSetup` branch
    /// `GameApp+0x19F3`): simulation in replay mode ([`SimHost::begin_playback`]), fog off (`m_FogShowMode`
    /// [`crate::game::fog_view::FOG_OFF`], `FUN_00476860(0)`), selection empty and locked (`+0x2420`, `FUN_00479710`)
    /// and the command bar hidden as in the cinemascope (`{0x11, 1}`, `0x004C77E4` -> `FUN_004529F0`, E-344). A
    /// cinemascope of the level script can turn bar and fog on again afterwards (as in the original).
    pub fn begin_playback(&mut self, commands: Vec<RecordedCommand>) {
        if let Some(sim) = self.sim_mut() {
            sim.begin_playback(commands);
            sim.set_fog_shown(false);
            sim.request_minimap_rebuild();
        }
        self.selection.clear();
        self.selection.locked = true;
        self.toolbar_host
            .host_event(opennml_ui::hud::HudEvent::with_args(
                opennml_ui::hud::HudEventKind::ToolbarOnOff,
                &[1],
            ));
    }

    /// Restores the script presentation from the simulation's presentation state ([`ScriptPresentation::restore`],
    /// without sound: the music follows in the next frame with a sound system), scene fade and info windows, then the
    /// game UI ([`Self::restore_scene`]); `tempo_memory` is the remembered speed of the host part (viewer: `None`).
    /// Without a simulation only the info windows are emptied.
    fn restore_presentation(&mut self, queue: &wgpu::Queue, vfs: &Vfs, tempo_memory: Option<u8>) {
        // The info windows of the old match go with it (also windows only in the host, e.g. from `ui_present`).
        self.info_frames = InfoFrames::new();
        let Some(mission) = self.sim().map(|s| s.state.script.mission.clone()) else {
            return;
        };
        let mut ctx = PresentCtx {
            map: &mut self.map,
            selection: &mut self.selection,
            queue,
            audio: None,
            vfs,
        };
        self.presentation.restore(&mission, tempo_memory, &mut ctx);
        // Scene fade: loading and restart reset it (`FUN_004BAA10`, O-376); after 146 and before 145 the image stays
        // black, without a new fade (deviation from the original, continuation of E-239).
        self.scene_fade.reset();
        if mission.presentation.faded_out {
            self.scene_fade.set_black();
        }
        // Info windows from the table in key order (text ID), each at the head: the table does not know the order of
        // opening, so the largest ID ends up at the bottom (deviation from the original, E-249). Measuring happens on
        // first drawing after loading, with the counters of the loaded state.
        for (&text, memory) in &mission.presentation.info_frames {
            self.info_frames
                .open(text, InfoFrameParams::of(&info_frame_kind(memory), text));
        }
        // The bar is fresh after loading (visible); a loaded cinemascope hides it again (`{0x11, 1}` as
        // `FUN_00430920`). The lock needs nothing: fresh means without cursor and without selection.
        if self.presentation.cinemascope {
            self.toolbar_host
                .host_event(opennml_ui::hud::HudEvent::with_args(
                    opennml_ui::hud::HudEventKind::ToolbarOnOff,
                    &[1],
                ));
        }
        self.restore_scene(&mission);
    }

    /// Game UI after loading from the state (deviation from the original; the area is empty after [`Hud::clear`]):
    /// - status bar `{0x1C, !cinemascope}`, messages hidden in the cinemascope (O-217);
    /// - bars `{0x1B, H/6}` only in the campaign's cinemascope (O-350), otherwise `{0x1B, 0}`;
    /// - then immediately full and without fade: the portraits from `presentation.portraits`, the running line
    ///   (`speech.line` with slot < 4) and the tips (`tip_ms[i] > 0` -> slot `i + 2`);
    /// - in the cinemascope or under the lock without corner buttons of the minimap.
    fn restore_scene(&mut self, mission: &opennml_sim::script::MissionState) {
        use opennml_sim::script::present::Presentation;
        use opennml_ui::hud::{HudEvent, HudEventKind};
        let cinemascope = self.presentation.cinemascope;
        self.hud.messages.cinemascope = cinemascope;
        let bar = if cinemascope && self.campaign() {
            self.hud.screen().height / 6
        } else {
            0
        };
        let mut events = vec![
            HudEvent::with_args(HudEventKind::StatusBarOnOff, &[i32::from(!cinemascope)]),
            HudEvent::with_args(HudEventKind::Cinemascope, &[bar]),
        ];
        let speech = &mission.speech;
        let mut shown = Vec::new();
        for (slot, p) in (0..).zip(&mission.presentation.portraits) {
            if let Some(p) = p {
                shown.push(Presentation::Portrait {
                    slot,
                    portrait: p.portrait,
                    frame: p.frame,
                });
            }
        }
        if let Some(line) = speech.line {
            shown.push(Presentation::Speech {
                text: line.text,
                slot: line.slot,
                ms: line.remaining_ms,
            });
        }
        for (slot, (&ms, &text)) in (0..).zip(speech.tip_ms.iter().zip(&speech.tip_text)) {
            if ms > 0 {
                shown.push(Presentation::Tip { text, slot, ms });
            }
        }
        let (campaign, height) = (self.campaign(), self.hud.screen().height);
        for p in &shown {
            events.extend(crate::game::toolbar_host::presentation_events(
                p, campaign, height,
            ));
        }
        for ev in events {
            self.hud.route(ev, &mut self.toolbar_host, true);
        }
        // The corner buttons of the minimap: `Hud::clear` builds them like the constructor; in the cinemascope (`{0x1D,
        // 0}`) and under the lock (`{0x19, 1}`) the original would have removed them.
        if cinemascope || mission.ui_locked {
            self.hud.minimap_buttons.remove();
        }
    }

    /// Sound for the level start (viewer and `opennml-game`): voices reset, fresh music management with the nation music of
    /// the local player; with a level script without start music (the scripts set it), otherwise the music actions of
    /// the start blocks or the nation music in multiplayer.
    pub fn start_level_audio(&mut self, audio: &mut AudioEngine, vfs: &Vfs) {
        self.voices.reset();
        // A line of the match before (restart in the same session) fades out.
        self.stop_speech(audio);
        self.presentation.start_audio(&self.map, audio, vfs);
    }

    /// The session is replaced or ends (loading, restart, end of the match): the running dialog line fades out within
    /// 1000 ms instead of sounding over the new level ([`SpeechAudio::stop`]).
    pub fn stop_speech(&mut self, audio: &mut AudioEngine) {
        self.speech.stop(audio);
    }

    /// Sound of the map for a frame: rain according to the map's weather (`SFX_REGEN`), combat points of the active
    /// player for the combat music (0 without a simulation; in pause the value stays, C-161).
    pub fn update_level_audio(&self, audio: &mut AudioEngine, vfs: &Vfs) {
        let rain = self.map.weather.as_ref().map(|w| w.kind())
            == Some(opennml_world::environment::WeatherKind::Rain);
        audio.set_rain(vfs, rain);
        audio.set_attack_points(self.sim().map_or(0, SimHost::active_attack_points));
    }

    /// Requests of the scripts to the flow since the last call (quicksave, profile variables).
    pub fn take_host_requests(&mut self) -> Vec<HostRequest> {
        self.presentation.take_requests()
    }

    /// Input lock of the scripts (18/19, `MissionState::ui_locked`, E-246).
    pub fn ui_locked(&self) -> bool {
        self.sim().is_some_and(|s| s.state.script.mission.ui_locked)
    }

    /// What the input may do in this frame (re/campaign-ui D6.10): lock and cinemascope from `MissionState`, the
    /// running dialog from the simulation ([`sim_gate`]), the scripts' camera
    /// ([`crate::game::script_camera::ScriptCamera::running`]; the player's camera moves with Home do not count), game
    /// type ([`Self::campaign`], menu level at -2) and game menu ([`Self::menu_open`]) from the host.
    pub fn input_gate(&self) -> InputGate {
        InputGate {
            script_camera: self.presentation.camera.running(),
            campaign: self.campaign(),
            menu_open: self.menu_open,
            menu_level: self.game_type == opennml_sim::setup::MENU_LEVEL_MODE,
            ..self.sim().map_or_else(InputGate::default, sim_gate)
        }
    }

    /// Keyboard context for `keys.ini` ([`KeyContext`], re/game-ui D5.3): type and selection GUID from the layouts of
    /// the command bar (`Toolbar::key_context`, O-303), the nation of the local player + 1 from the simulation (the
    /// original sets it when setting the local player, `0x00430140`). Without a bar type 1 and selection 0, without a
    /// simulation nation 0.
    pub fn key_context(&self) -> KeyContext {
        let nation = self.sim().map_or(0, |sim| {
            sim.state
                .players
                .get(usize::from(sim.active_player))
                .map_or(0, |p| p.nation + 1)
        });
        self.hud.toolbar.as_ref().map_or(
            KeyContext {
                nation,
                ..KeyContext::default()
            },
            |t| t.key_context(nation),
        )
    }

    /// A key command from `bindings2.ini` or `keys.ini` (`GameFlow::key`) at program clock `now`; `true` if the match
    /// takes it (always: the original's console takes every known command). The bar's outputs are carried out by the
    /// session with `vfs` and `audio` like a click.
    ///
    /// - `button(n)`/`xbutton(n)` (O-309): bar shortcut ([`GameSession::toolbar_shortcut`]). `xbutton(572)` (diplomacy,
    ///   `SHIFT D`) and 570-577, 579/580 reach the minimap dispatcher via the bar ([`GameSession::minimap_button`]).
    /// - `quicksave`/`quickload` are executed by `GameFlow` and not passed on here.
    /// - `/setcamera` (Home): [`GameSession::set_camera_home`].
    /// - `storegroup`, `selectgroup`, `addtogroup`: [`GameSession::run_group`].
    /// - `selectnextguid`: [`GameSession::select_next_guid`] at program clock `now`.
    /// - `togglepause`: [`GameSession::toggle_pause`].
    /// - `changegamespeed(d)`: [`GameSession::change_game_speed_played`]. `GameFlow` handles the key itself
    ///   ([`SessionView::change_game_speed`]) and writes the setting; this path only changes the speed.
    /// - The rest are only resolved; their effect comes later (chat and quest menu). Until then they are taken without
    ///   effect, with a log line.
    ///
    /// Afterwards the session syncs the groups with the selection ([`GameSession::sync_groups`]).
    pub fn run_hotkey(
        &mut self,
        cmd: &HotkeyCommand,
        now: u64,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) -> bool {
        let taken = self.run_hotkey_inner(cmd, now, vfs, audio);
        self.sync_groups();
        taken
    }

    fn run_hotkey_inner(
        &mut self,
        cmd: &HotkeyCommand,
        now: u64,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) -> bool {
        use HotkeyCommand as C;
        let task = match *cmd {
            C::Button(n) => {
                self.toolbar_shortcut(n, true, vfs, audio);
                return true;
            }
            C::XButton(n) => {
                self.toolbar_shortcut(n, false, vfs, audio);
                return true;
            }
            C::SetCamera => {
                self.set_camera_home();
                return true;
            }
            C::StoreGroup(_) | C::SelectGroup(_) | C::AddToGroup(_) => {
                self.run_group(cmd, vfs, audio);
                return true;
            }
            C::SelectNextGuid(guid) => {
                self.select_next_guid(guid, now, vfs, audio);
                return true;
            }
            C::TogglePause => {
                self.toggle_pause();
                return true;
            }
            // `showchatwindow` opens a menu window: the flow does that with the `MenuApp`.
            C::QuickSave | C::QuickLoad | C::ShowChatWindow(_) => "GameFlow",
            C::ChangeGameSpeed(d) => {
                // The flow handles the key itself ([`SessionView::change_game_speed`], writes the setting); this path
                // only changes the speed.
                self.change_game_speed_played(d, vfs, audio);
                return true;
            }
            C::ShowQuestMenu(_) => "quest menu",
        };
        log::debug!("Key command {cmd:?} at clock {now}: no effect yet ({task})");
        true
    }

    /// `KD_ADD_TO_GROUP` held (`FUN_004143A0` with the table `held_vk`; LSHIFT in `keymap.ini`).
    pub fn add_to_group_held(&self) -> bool {
        self.hotkeys.keydef_down("KD_ADD_TO_GROUP", &self.held_vk)
    }

    /// Syncs the control groups of the active slot with the selection ([`GroupCx::sync`]): the selection log (clearing
    /// -> last group -1, adding with `KD_ADD_TO_GROUP` held -> shift path `FUN_004FB180`, O-310) and the clusters anew
    /// on every change (deviation from the original). After every input, every key command and every frame; without a
    /// simulation the log lapses.
    pub fn sync_groups(&mut self) {
        let add_held = self.add_to_group_held();
        let Some(sim) = self.map.sim.as_ref() else {
            self.selection.log.clear();
            return;
        };
        let Some(groups) = self.groups.get_mut(usize::from(sim.active_player)) else {
            self.selection.log.clear();
            return;
        };
        GroupCx {
            groups,
            clusters: &mut self.clusters,
            selection: &mut self.selection,
            sim,
            add_held,
        }
        .sync();
    }

    /// A group command on the groups of the active slot (`storegroup` `0x004FB1C0`, `selectgroup` `0x004FAE90`,
    /// `addtogroup` `0x004FB0E0`; [`GroupCx::run`]): a new selection reports `{0x10, 2}` with the selection sound
    /// ([`GameSession::selection_changed`]), the camera cycle moves the game camera with the current offset
    /// (`GameEngine_MoveCameraTo(x, z, 0)`, [`GameCamera::move_to_keeping_offset`]). Local: no command, no recording,
    /// no effect on the checksum.
    pub fn run_group(&mut self, cmd: &HotkeyCommand, vfs: &Vfs, audio: Option<&mut AudioEngine>) {
        let add_held = self.add_to_group_held();
        let Some(sim) = self.map.sim.as_ref() else {
            return;
        };
        let Some(groups) = self.groups.get_mut(usize::from(sim.active_player)) else {
            return;
        };
        let outcome = GroupCx {
            groups,
            clusters: &mut self.clusters,
            selection: &mut self.selection,
            sim,
            add_held,
        }
        .run(cmd);
        match outcome {
            GroupOutcome::Nothing => {}
            GroupOutcome::Selected => self.selection_changed(vfs, audio),
            GroupOutcome::Camera(at) => {
                self.map
                    .camera
                    .move_to_keeping_offset(at.x, at.y, &self.map.scene.terrain);
            }
        }
    }

    /// Control groups of all 8 slots for the host part of the savegame (`HostPart::control_groups`).
    pub fn control_groups_save(&self) -> Vec<GroupsSave> {
        self.groups.iter().map(ControlGroups::to_save).collect()
    }

    /// `selectnextguid` indices of all 8 slots for the host part of the savegame (`HostPart::next_guid_index`).
    pub fn next_guid_index_save(&self) -> Vec<NextGuidIndex> {
        self.groups
            .iter()
            .map(|g| g.next_guid_index.clone())
            .collect()
    }

    /// `selectnextguid(guid)` (`GameWorld_SelectNextGuid` `0x0048B4A0`, O-314; [`GroupCx::select_next_guid`]) in the
    /// active slot at program clock `now_ms` (instead of `GetTickCount`; virtual in check runs): a double-tap within
    /// 300 ms or an already chosen hit only moves the game camera with the current offset (`GameEngine_MoveCameraTo(x,
    /// z, 0)`, [`GameCamera::move_to_keeping_offset`]); otherwise a new selection with `{0x10, 2}` and selection sound
    /// ([`GameSession::selection_changed`]), even if `Selection` rejects the hit. Local: no command, no recording, no
    /// effect on the checksum. Nothing without a simulation.
    pub fn select_next_guid(
        &mut self,
        guid: i32,
        now_ms: u64,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) {
        let add_held = self.add_to_group_held();
        let Some(sim) = self.map.sim.as_ref() else {
            return;
        };
        let Some(groups) = self.groups.get_mut(usize::from(sim.active_player)) else {
            return;
        };
        let outcome = GroupCx {
            groups,
            clusters: &mut self.clusters,
            selection: &mut self.selection,
            sim,
            add_held,
        }
        .select_next_guid(guid, now_ms, &mut self.next_guid_tap);
        match outcome {
            GroupOutcome::Nothing => {}
            GroupOutcome::Selected => self.selection_changed(vfs, audio),
            GroupOutcome::Camera(at) => {
                self.map
                    .camera
                    .move_to_keeping_offset(at.x, at.y, &self.map.scene.terrain);
            }
        }
    }

    /// Home key (`/setcamera`, `FUN_00420230` -> `FUN_00433FC0(250)`): smooth move to the home position
    /// ([`crate::game::script_camera::ScriptCamera::push_home`]): 250 game ms, linear in the offset, target stays;
    /// behind running script moves, stands still in pause, does not lock the player camera. A running mouse-wheel
    /// transition ends (in the original the home move would be behind it in the list, the result is the same).
    pub fn set_camera_home(&mut self) {
        self.controller.stop_wheel();
        self.presentation.camera.push_home(&self.map.camera);
    }

    /// `KD_CHECK_LBUTTON` held (only with [`CameraKeys::Original`]): keys, edge, rotating, zoom and mouse wheel stand
    /// still (`0x0043C7EA`).
    pub fn left_button_holds_camera(&self) -> bool {
        matches!(self.camera_keys, CameraKeys::Original { .. })
            && self.hotkeys.keydef_down("KD_CHECK_LBUTTON", &self.held_vk)
    }

    /// Camera inputs of a frame ([`frame_input`]) including `KD_CAM_FREEHAND` ([`freehand_step`], O-331): the camera
    /// block runs without lock and scripts' camera ([`InputGate::player_camera`]), without a held left button and only
    /// with the original's camera, not with the free camera. Without `running` (only in the network with the game menu
    /// open) the player camera stands still.
    fn camera_frame_input(&mut self) -> FrameInput {
        let locked = !self.input_gate().player_camera() || !self.running;
        let mut input = frame_input(
            &self.pointer,
            &self.held_vk,
            self.screen,
            locked,
            self.text_focus,
            &self.camera_keys,
            &self.hotkeys,
        );
        let runs = !locked
            && !input.camera.halt
            && matches!(self.camera_keys, CameraKeys::Original { .. })
            && self.map.free.is_none();
        self.cursor_warp = freehand_step(
            &mut input.camera,
            &mut self.pointer.cursor,
            &mut self.freehand_anchor,
            runs,
        );
        input
    }

    /// Window position to which the pointer is to return (`KD_CAM_FREEHAND` holds it in place, `SetCursorPos` at
    /// `0x0043D083`), once per frame; `opennml-game` uses it to set the system's mouse pointer (O-331).
    pub fn take_cursor_warp(&mut self) -> Option<(f64, f64)> {
        self.cursor_warp.take()
    }

    /// Remembered speed of the cinemascope for the host part of the savegame (`HostPart::cinemascope_speed`).
    pub fn cinemascope_speed(&self) -> Option<u8> {
        self.presentation.tempo_memory
    }

    /// Savegame with the host part `host` (camera from [`GameSession::camera`]; map description, game mode, name and
    /// speed are set by the host) and the generated map of a random map.
    pub fn save(&self, host: HostPart) -> anyhow::Result<GameSaveFile> {
        let sim = self.sim().context("saving: the map has no simulation")?;
        Ok(GameSaveFile::new(
            sim.save(),
            host,
            self.generated_map.clone(),
        ))
    }

    /// Pose of the game camera for the savegame.
    pub fn camera(&self) -> CameraSave {
        CameraSave::of(&self.map.camera)
    }

    pub fn sim(&self) -> Option<&SimHost> {
        self.map.sim.as_ref()
    }

    pub fn sim_mut(&mut self) -> Option<&mut SimHost> {
        self.map.sim.as_mut()
    }

    /// Path of the `.lvl`; for a random map `randommap:<seed>`.
    pub fn level(&self) -> &str {
        &self.map.scene.lvl_path
    }

    /// State of the match's random map (control command `random_map`); `None` for fixed maps.
    pub fn random_map_state(&self) -> Option<&RandomMapState> {
        self.random_map.as_ref()
    }

    /// Generated map of the match's random map (into the savegame); `None` for fixed maps.
    pub fn random_map(&self) -> Option<&opennml_randommap::RandomMap> {
        self.generated_map.as_deref()
    }

    /// Engine ticks of the simulation (0 without a simulation).
    pub fn steps(&self) -> u64 {
        self.sim().map_or(0, |sim| sim.state.steps)
    }

    /// Checksum of the last tick (0 without a simulation).
    pub fn checksum(&self) -> u64 {
        self.sim().map_or(0, SimHost::checksum)
    }

    /// New window size (also 0 when minimizing).
    pub fn resize(&mut self, w: u32, h: u32) {
        self.size = (w, h);
        self.screen = (w, h);
        self.hud.resize(screen_of((w, h)));
    }

    /// A frame at program clock `now` (whole milliseconds): frame time since the last call, then
    /// [`GameSession::advance_by`]. Without `running` (game menu, saving, loading) only the time base is brought up to
    /// date; in pause ([`PauseState::sim`]) only the pause step ([`GameSession::advance_paused`]). In every case, also
    /// in pause and game menu, afterwards the UI fades by the program time since the last frame
    /// ([`Self::tick_presentation`]; the first frame and a backward jump of the clock count 0). Afterwards
    /// [`GameSession::ticks`] and [`SimHost::halted`] of this frame.
    ///
    /// In the network ([`Self::net`]) clock and ticks run with the turns independently of `running` (O-143): an open
    /// game menu does not stop the others; `running` only locks input and camera. The pause bits follow the core
    /// ([`NetDriver::pause_state`]); in the network pause the driver keeps pumping (lifting the pause, keep-alive), the
    /// pause step is computed.
    pub fn advance(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        now: u64,
        running: bool,
        audio: Option<&mut AudioEngine>,
    ) -> Vec<SessionEvent> {
        self.advance_frame(gpu, vfs, now, running, false, audio)
    }

    /// A frame of the stopped menu level: [`GameSession::advance`] without `running`, but the frame time accumulates
    /// for the scripts' camera ([`SessionClock::hold_frame`], O-447); the next running frame gives it to the camera all
    /// at once ([`GameSession::advance_by`]).
    pub fn hold(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        now: u64,
        audio: Option<&mut AudioEngine>,
    ) -> Vec<SessionEvent> {
        self.advance_frame(gpu, vfs, now, false, true, audio)
    }

    /// [`GameSession::advance`]; with `hold` (only without `running`) [`GameSession::hold`].
    fn advance_frame(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        now: u64,
        running: bool,
        hold: bool,
        mut audio: Option<&mut AudioEngine>,
    ) -> Vec<SessionEvent> {
        self.running = running;
        if let Some(net) = &self.net {
            self.pause = net.pause_state();
        }
        self.sync_halted();
        self.ticks = 0;
        if !running {
            // The game menu (or saving/loading) has the mouse: menus set 7/0 (`MenuFrame` `0x00460599`), after closing
            // that stays until the next movement over the world.
            self.mouse_pointer.menu();
        }
        let (first, before) = (self.clock.last_now.is_none(), self.clock.program_ms);
        let run = running || self.net.is_some();
        let played = if run && self.pause.sim {
            let speed = self.sim().map_or(10, |s| s.game_speed);
            let (dt_ms, ticks) = self.clock.pause_frame(now, speed);
            self.ticks = ticks;
            let net_now = now.saturating_add(self.net_time_offset_ms);
            self.net_halt_step(gpu, vfs, net_now, audio.as_deref_mut());
            self.advance_paused(vfs, dt_ms, audio);
            Vec::new()
        } else {
            let frame = if hold && !run {
                self.clock.hold_frame(now);
                None
            } else {
                self.clock.frame_ms(now, run)
            };
            match frame {
                Some(dt_ms) => {
                    let steps_before = self.steps();
                    let played = self.advance_by(gpu, vfs, dt_ms, true, audio);
                    self.ticks = u32::try_from(self.steps() - steps_before).unwrap_or(u32::MAX);
                    played
                }
                None => Vec::new(),
            }
        };
        if let Some(net) = &self.net {
            self.pause = net.pause_state();
            self.sync_halted();
        }
        // The fades keep running in real time, also in pause and in the game menu; line, tip and screen-text time stand
        // still there with the game time.
        let program_dt = if first {
            0.0
        } else {
            (self.clock.program_ms - before).max(0.0) as f32
        };
        self.tick_presentation(program_dt);
        played
    }

    /// A frame in pause (`Engine+0x24E4`, O-317; `GameEngine_Update` `0x00445650` and `GameEngine_UpdateCamera`
    /// `0x0043C7C0` with the bit set) with `dt_ms` milliseconds of frame time:
    ///
    /// - **runs:** player camera with keys and edge (`GameEngine_UpdateCamera` always; lock and script camera as
    ///   usual), listener at the camera (`SoundManager_SetListenerFromCamera`), music (`MusicManager_Update`; here the
    ///   sound of the map and the music after loading), the object under the mouse and the frame step of the command
    ///   bar (event 6 comes once per frame from the UI, `0x004BB16D`), the sync of the control groups; the water via
    ///   [`SessionClock::water_ms`].
    /// - **stands still:** world, AI, level scripts, simulation including the selection mirror (`sync_selection`),
    ///   message bar (`FUN_00494A00` only without pause), text messages, the scripts' camera moves and the mouse-wheel
    ///   transition (the move list `FUN_00444C10`/`FUN_00432B90` runs only without pause), fog, weather and effects,
    ///   animation (rendering clock, `GameWorld_BuildRenderListsAndFrameUpdate` with `dt` 0).
    pub fn advance_paused(&mut self, vfs: &Vfs, dt_ms: f32, mut audio: Option<&mut AudioEngine>) {
        if let Some(audio) = audio.as_deref_mut() {
            self.presentation.resume_audio(&self.map, audio, vfs);
        }
        let input = self.camera_frame_input();
        self.pointer.look = Vec2::ZERO;
        let map = &mut self.map;
        match map.free.as_mut() {
            Some(free) => free.update(input.free, dt_ms),
            None => self.controller.update_paused(
                &mut map.camera,
                &map.scene.terrain,
                input.camera,
                &self.controls,
                dt_ms,
            ),
        }
        if let Some(audio) = audio.as_deref_mut() {
            let target = map.camera.target;
            let height = map.scene.terrain.ground_height(target.x, target.z);
            audio.sound.set_listener(target, height, map.camera.eye);
            self.update_level_audio(audio, vfs);
        }
        self.hovered = self.pointer.cursor.and_then(|c| {
            let params = self.pick_params();
            self.map.pick_target(&params, c, self.screen)
        });
        if let Some(sim) = self.map.sim.as_mut() {
            let hover = hover_target(sim, self.hovered);
            // Without a tick only the events of the commands executed immediately in pause (`SimHost::execute_now`):
            // the original reports e.g. a cancel in the queue synchronously (`0x00507960` -> `NotifyUiIfSelected(0)`),
            // so the button of a canceled UPGRADE returns even in pause (O-395, re/game-ui N.14.13).
            //
            // After resuming, the first tick delivers the same events once more (until then they are in
            // `state.events`). A canceled order then gives a second `{0x10, 0}`, only for a building that is selected
            // in both frames (`rebuild_kind`). That stays without sound and without a visible change: with a single
            // selection the second rebuild is silent and yields the same buttons (`0x00457DB0`; menu, submenu and
            // cursor remain, D-234). The fallback to the full rebuild applies only if several objects are selected in
            // the meantime (`shown_menu = 0`; a construction site has no queue). It then plays no selection sound,
            // which applies only to a single selection (`0x00457A4D`), and closes at most an open cursor. The hero row
            // rebuilds the same way twice after a creation or destruction by an immediate command.
            let events = sim.take_immediate_events();
            self.toolbar_host
                .frame(sim, &self.selection.ids, &events, hover);
        }
        self.toolbar_step(vfs, audio);
        self.pointer_step();
        self.sync_groups();
        self.sync_minimap();
    }

    /// Fog of the minimap to the host's state ([`crate::game::minimap::Minimap::sync_host`]: empty ticks and immediate
    /// rebuilds since the last frame); once per frame after the step and before drawing. If the map is drawn in this
    /// frame ([`Self::minimap_drawn`]), the ping clock also runs with the session's game time (`steps * 50 ms`):
    /// without drawing its last time stays, the next drawn frame gets the whole span (O-289).
    pub fn sync_minimap(&mut self) {
        let drawn = self.minimap_drawn();
        if let (Some(m), Some(sim)) = (self.hud.minimap.as_mut(), self.map.sim.as_ref()) {
            m.sync_host(sim);
            if drawn {
                // Wrapping: a loaded savegame may carry any tick.
                let now_ms = sim
                    .state
                    .steps
                    .wrapping_mul(u64::from(opennml_sim::tick::STEP_MS));
                self.hud.pings.advance(now_ms);
            }
        }
    }

    /// Does the UI draw the minimap in this frame? UI on with simulation, node with `IsMap` and visible
    /// ([`Hud::minimap_shown`]) and no cinemascope ([`InputGate::minimap_visible`]); the same condition as
    /// [`Self::hud_cmds_into`] with [`Self::minimap_frame`].
    fn minimap_drawn(&self) -> bool {
        self.hud_enabled
            && self.map.sim.is_some()
            && self.hud.minimap.is_some()
            && self.hud.minimap_shown()
            && self.input_gate().minimap_visible()
    }

    /// A minimap button or minimap key ([`ToolbarIntent::Minimap`]; dispatcher `0x0044C040`, cases from `0x0E`).
    /// `double`: double-click of a corner button (event `0x27`, evaluated only for 576/577, `0x0044CAC5...0x0044CAD8`);
    /// `vfs`/`audio` for the selection sounds. Everything local: no command, no effect on the checksum.
    /// - 570 "Center on action" (`0x0047F1E0`): only with a non-empty action stack, take out the most recent location
    ///   and move the game camera there with the current offset (`GameEngine_MoveCameraTo(SIZE_X*x, SIZE_Z*z, 0)`,
    ///   [`GameCamera::move_to_keeping_offset`]; target at y = 0 as in D-254, without height + 80). Every press uses up
    ///   one location (O-219).
    /// - 573/574/575 (cases `0xE`/`0xF`/`0x10`, `0x0044CB1C`): map mode 0/1/2 (`FUN_004785D0`, `GameWorld+0x22D4`),
    ///   then `0x186AB` -> `{0x22}`: the corner buttons anew ([`Hud::rebuild_minimap_buttons`]). Nothing without a
    ///   minimap.
    /// - 576/577: [`GameSession::select_idle`] with `double` and "all" at 577.
    /// - 579: game menu (`FUN_0042E150`) via the flow ([`UiRequest::GameMenu`]; it checks menu state 0).
    /// - 571: signal ([`GameSession::send_signal`]).
    /// - 572 (`SHIFT D`) and 580: toggle the diplomacy dialog via the flow ([`UiRequest::ToggleDiplomacy`]): 572
    ///   without a game type check (re/game-ui D5.7, hypothesis: also in the campaign), 580 only for game type >= 0
    ///   (the button does not exist otherwise; a key `xbutton(580)` therefore has no effect in the campaign). The flow
    ///   checks the lock. 578/581 do not come here at all (no branch in the dispatcher).
    ///
    /// [`ToolbarIntent::Minimap`]: opennml_ui::hud::toolbar::ToolbarIntent::Minimap
    /// [`GameCamera::move_to_keeping_offset`]: opennml_world::camera::GameCamera::move_to_keeping_offset
    pub fn minimap_button(
        &mut self,
        msg: i32,
        double: bool,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) -> ToolbarEffects {
        let mut fx = ToolbarEffects::default();
        match msg {
            MSG_MINIMAP_FIRST => {
                let Some(cell) = self.sim().map(|s| s.rules.map.cell) else {
                    return fx;
                };
                if let Some((x, z)) = self.hud.actions.pop() {
                    let (x, z) = ((cell[0] * x) as f32, (cell[2] * z) as f32);
                    self.map
                        .camera
                        .move_to_keeping_offset(x, z, &self.map.scene.terrain);
                }
            }
            m if mode_of(m).is_some() => {
                if let (Some(mode), Some(minimap)) = (mode_of(m), self.hud.minimap.as_mut()) {
                    minimap.mode = mode;
                    self.hud.rebuild_minimap_buttons();
                }
            }
            MSG_IDLE_ONE | MSG_IDLE_ALL => {
                fx.selection_changed = self.select_idle(double, msg == MSG_IDLE_ALL, vfs, audio);
            }
            MSG_GAME_MENU => self.ui_requests.push(UiRequest::GameMenu),
            MSG_SIGNAL => self.send_signal(),
            MSG_DIPLOMACY_KEY => self.ui_requests.push(UiRequest::ToggleDiplomacy),
            MSG_DIPLOMACY_BUTTON => {
                if self.game_type >= 0 {
                    self.ui_requests.push(UiRequest::ToggleDiplomacy);
                } else {
                    log::debug!(
                        "Minimap: button 580 does not exist for game type {}",
                        self.game_type
                    );
                }
            }
            _ => log::debug!("Minimap: button {msg} has no branch in the dispatcher"),
        }
        fx
    }

    /// Sends a signal (571 / `SHIFT F`, sender `0x0044CB6E...0x0044CBE7`): nothing without a local player. Otherwise
    /// the tile of the camera target (`kx = (int)(target.x / SIZE_X)` in `f32`, truncated toward 0, likewise `kz`) and
    /// the chat mask of the local slot ([`Masks::chat`]) as command `{0x5C, slot, mask, kx, kz}` via the normal command
    /// path ([`SimHost::submit`], immediate in pause). Sound, ping and stack come with the event ([`Hud::handle`]); in
    /// pause therefore only with the first tick afterwards (deviation from the original).
    fn send_signal(&mut self) {
        let Some(sim) = self.map.sim.as_mut() else {
            return;
        };
        let player = sim.active_player;
        let cell = sim.rules.map.cell;
        let target = self.map.camera.target;
        let (x, z) = (
            (target.x / cell[0] as f32) as i32,
            (target.z / cell[2] as f32) as i32,
        );
        let mask = self.masks.chat[usize::from(player)];
        sim.submit(opennml_sim::command::Command {
            player,
            kind: opennml_sim::command::CommandKind::Signal {
                player: i32::from(player),
                mask,
                x,
                z,
            },
        });
    }

    /// Selects idle workers (`0x0048B610(slot, double, all)` for the local slot; O-293), without a command:
    /// 1. Clear the selection (`0x0047A130`) and `{0x10, 2}` ([`GameSession::selection_changed`]), also without
    ///    workers.
    /// 2. Empty list ([`opennml_sim::idle_workers::IdleWorkers::list`]): end.
    /// 3. `all` (577): each ID via the group path `0x004891B0` ([`Selection::select_list`]: fog, at most 30, with more
    ///    than one object only units), then `{0x10, 2}` once more; neither camera nor stack nor index.
    /// 4. Otherwise (576): index >= count -> 0; take the entry at the index, index + 1
    ///    ([`GameSession::idle_worker_index`]). If the object is missing (`GameWorld_GetObjectById(id, 0)`), end; the
    ///    index is already incremented. Otherwise clear, `0x00488F60` ([`Selection::select_only`], can reject) and
    ///    `{0x10, 2}`; with `double` the camera to the world position (`GameEngine_MoveCameraTo(x, z, 0)`, offset
    ///    stays), otherwise the tile (`GameObject_GetTilePos`) to the front of the action stack (`0x0047F1A0`).
    ///
    /// A double-click on 576 arrives as 9, 9, `0x27`: three workers in a row, only the third with the camera (O-293).
    /// Returns whether the selection has changed (always true with a simulation, the first step already reports it).
    pub fn select_idle(
        &mut self,
        double: bool,
        all: bool,
        vfs: &Vfs,
        mut audio: Option<&mut AudioEngine>,
    ) -> bool {
        let Some(sim) = self.map.sim.as_ref() else {
            return false;
        };
        let player = sim.active_player;
        let list = sim.state.idle_workers.list(player).to_vec();
        self.selection.clear();
        self.selection_changed(vfs, audio.as_deref_mut());
        if list.is_empty() {
            return true;
        }
        let Some(sim) = self.map.sim.as_ref() else {
            return true;
        };
        if all {
            self.selection
                .select_list(&sim.state, &sim.rules, |o| sim.fog_selectable(o), &list);
            self.selection_changed(vfs, audio);
            return true;
        }
        let Some(index) = self.idle_worker_index.get_mut(usize::from(player)) else {
            return true;
        };
        if usize::try_from(*index).map_or(true, |i| i >= list.len()) {
            *index = 0;
        }
        let id = list[*index as usize];
        *index += 1;
        let Some((pos, tile)) = sim.state.object(id, false).map(|o| (o.world_pos, o.tile)) else {
            return true;
        };
        self.selection
            .select_only(&sim.state, &sim.rules, |o| sim.fog_selectable(o), id);
        self.selection_changed(vfs, audio);
        if double {
            self.map
                .camera
                .move_to_keeping_offset(pos[0], pos[2], &self.map.scene.terrain);
        } else {
            self.hud.actions.push(tile);
        }
        true
    }

    /// Effect of a mouse event of the minimap ([`crate::game::minimap::Minimap::mouse`]):
    /// - Camera (`0x00449D20` -> `0x004323F0`): target `(SIZE_X*mx, 0, SIZE_Z*mz)`, the eye keeps the offset
    ///   ([`GameCamera::move_to_keeping_offset`]; y stays 0 as in D-254, no terrain height + 80). Local, without a
    ///   command.
    /// - Movement (`0x00449DD0` -> `MoveCommand_Construct(n, x, z, -1, 0, Fixed(-200), -1, double)`): only if the first
    ///   selected object belongs to the local player ([`SimHost::commands_selection`]), then a pure walk command for
    ///   the whole selection: formation 0, no direction, target radius -1, without spread, without Shift/Ctrl wrapper
    ///   and without target-point decal (O-40), via [`SimHost::submit`] (immediate in pause). Otherwise nothing
    ///   happens; the minimap took the event anyway.
    ///
    /// [`GameCamera::move_to_keeping_offset`]: opennml_world::camera::GameCamera::move_to_keeping_offset
    pub(crate) fn minimap_click(&mut self, click: crate::game::minimap::MinimapClick) {
        use crate::game::minimap::MinimapClick;
        let Some(sim) = self.map.sim.as_mut() else {
            return;
        };
        match click {
            MinimapClick::Camera { tile: (mx, mz) } => {
                let cell = sim.rules.map.cell;
                let (x, z) = (cell[0] as f32 * mx, cell[2] as f32 * mz);
                self.map
                    .camera
                    .move_to_keeping_offset(x, z, &self.map.scene.terrain);
            }
            MinimapClick::Move { x, z, run } => {
                if !sim.commands_selection(&self.selection.ids) {
                    return;
                }
                let player = sim.active_player;
                sim.submit(opennml_sim::command::Command {
                    player,
                    kind: opennml_sim::command::CommandKind::Move {
                        units: self.selection.ids.clone(),
                        x,
                        z,
                        target: -1,
                        formation: 0,
                        direction: opennml_sim::motion::NO_DIRECTION,
                        spread: -1,
                        flags: i32::from(run),
                    },
                });
            }
        }
    }

    /// [`SimHost::halted`] from the session's state: no ticks without `running` or in pause; in the network the host
    /// only stands still if the driver stands after an execution tick ([`NetDriver::halted`]; the game menu does not
    /// halt).
    fn sync_halted(&mut self) {
        let halted = match &self.net {
            Some(net) => net.halted(),
            None => !self.running || self.pause.sim,
        };
        if let Some(sim) = self.sim_mut() {
            sim.halted = halted;
        }
    }

    /// `togglepause` (`0x0042D780`, bindings `CONTROL P` and `PAUSE`; [`PauseState::toggle`], O-316) with the session's
    /// game type; afterwards [`SimHost::halted`] applies immediately so that commands already take the path of the
    /// pause until the next frame. When switching on, [`SimHost::flush_pending`] executes the still waiting commands
    /// immediately in order (O-318): otherwise they would run only after the commands of the pause and be missing from
    /// a savegame made in the pause. `true` if the pause changed.
    ///
    /// In the network ([`NetDriver::toggle_pause`]): a pause request or lifting one's own pause (O-316); the bits
    /// follow the core only with the next frame.
    pub fn toggle_pause(&mut self) -> bool {
        if let Some(net) = self.net.as_mut() {
            return net.toggle_pause();
        }
        let changed = self.pause.toggle(self.game_type);
        self.sync_halted();
        if changed
            && self.pause.sim
            && let Some(sim) = self.sim_mut()
        {
            sim.flush_pending();
        }
        changed
    }

    /// `changegamespeed(d)` (`GameEngine_ChangeGameSpeed` `0x0042F050`, keys `+`/`-`): `v = speed + d`, `v < 0` gives
    /// 0, `v > 18` gives 19. `v` only applies on a change (`SimHost::game_speed`), and message 9 (faster) or 10
    /// (slower) goes to the message bar (flag `0x03010000`, no location); the return value `Some(v)` lets the flow
    /// write the setting `GameSpeed`. At the limit neither message nor return value. No check for pause, cinemascope or
    /// game menu. With an empty queue the message appears immediately, also in pause and with the game menu open; there
    /// only its expiry halts (the bar's clock runs with the game tick), and queued messages wait until the match
    /// continues.
    ///
    /// Deviation from the original: the original sends command `0x6E` (`GameEngine_CmdSetGameSpeed` `0x0042EF30`, which
    /// triggers the message); here the host holds the speed, a simulation command is omitted. The cinemascope's
    /// remembered speed (`cinemascope_speed`) stays untouched (O-320). Sounds of the message go to `out`.
    ///
    /// In the network ([`NetDriver::change_game_speed`]): only the network host requests the speed and gets `Some(v)`
    /// for the setting; speed and message come only after the execution tick ([`SpeedChange::message`]). Clients:
    /// nothing (E-265, E-294).
    pub fn change_game_speed(&mut self, d: i32, out: &mut Vec<SoundRequest>) -> Option<u8> {
        if let Some(net) = self.net.as_mut() {
            let current = self.map.sim.as_ref()?.game_speed;
            return net.change_game_speed(current, d);
        }
        let sim = self.map.sim.as_mut()?;
        let current = sim.game_speed;
        let v = key_game_speed(current, d)?;
        sim.game_speed = v;
        let id = if v > current {
            SPEED_RAISED_MESSAGE
        } else {
            SPEED_LOWERED_MESSAGE
        };
        self.hud.post(id, NO_TILE, SPEED_MESSAGE_FLAGS, out);
        Some(v)
    }

    /// [`GameSession::change_game_speed`] including the sound of the message (the flow calls it via `SessionView`).
    pub fn change_game_speed_played(
        &mut self,
        d: i32,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) -> Option<u8> {
        let mut sounds = Vec::new();
        let changed = self.change_game_speed(d, &mut sounds);
        if let Some(audio) = audio {
            for sound in sounds {
                play_sim_sound(audio, vfs, sound);
            }
        }
        changed
    }

    /// The window has lost focus (`GameApp_DispatchEvent` `0x33`, `0x004C9A4B`; O-154): with a running match and
    /// without a visible pause it toggles `togglepause`; returning does not lift it. `opennml-game` calls it only without a
    /// control channel (deviation from the original). `true` if the pause was toggled. In the network only a pause
    /// request ([`NetDriver::focus_lost`], O-154).
    pub fn focus_lost(&mut self) -> bool {
        if let Some(net) = self.net.as_mut() {
            return net.focus_lost();
        }
        if self.pause.shown {
            return false;
        }
        self.toggle_pause()
    }

    /// Control entries, dropouts and failure of the network match since the last call, for the flow
    /// (`SessionView::take_net_frame`); empty afterwards.
    pub fn take_net_frame(&mut self) -> NetFrame {
        std::mem::take(&mut self.net_frame)
    }

    /// Network pause: the driver keeps pumping (lifting, keep-alive, a state of the sync arriving late), without game
    /// time; speed changes and adoption as in [`Self::advance_by`].
    fn net_halt_step(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        now: u64,
        audio: Option<&mut AudioEngine>,
    ) {
        let (Some(net), Some(sim)) = (self.net.as_mut(), self.map.sim.as_mut()) else {
            return;
        };
        let step = net.advance(sim, 0.0, now).into_session(&mut self.net_frame);
        let mut sounds = Vec::new();
        post_speed_messages(&mut self.hud, &step.speed, &mut sounds);
        let mut audio = audio;
        if let Some(audio) = audio.as_deref_mut() {
            for sound in sounds {
                play_sim_sound(audio, vfs, sound);
            }
        }
        if step.adopted {
            self.rebuild_after_adopt(gpu, vfs, audio);
        }
    }

    /// After adopting the network host's state during a sync ("as when loading"): the presentation from the new state
    /// as after loading (`resume_from`, [`Self::reset_view`]): selection, voices, markers, trajectories, fires and
    /// effects, message bar, command bar, dialog line (fades out), script presentation including info windows and scene
    /// fade. What the game host holds and a savegame carries in the host part stays: camera, scripts' camera,
    /// remembered speed, fog switch, control groups, index of the idle workers, masks.
    fn rebuild_after_adopt(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) {
        let tempo = self.cinemascope_speed();
        let script_camera = self.presentation.camera.clone();
        let fog = self.sim().map(|s| s.fog.show_mode);
        if let Some(audio) = audio {
            self.stop_speech(audio);
        }
        self.clear_sim_view(gpu, vfs, tempo);
        self.presentation.camera = script_camera;
        if let (Some(mode), Some(sim)) = (fog, self.sim_mut()) {
            sim.set_fog_show_mode(mode);
        }
        log::info!("Network: presentation rebuilt after the sync");
    }

    /// Fades of the UI by `dt_ms` of program time: the original computes them while drawing with `Timer_GetTimeMs`
    /// (`0x0044851D...0x00448535`), so also in pause and game menu; the reimplementation computes them per frame after
    /// the step and then draws read-only. Here the cinemascope area (`CinemascopeView::tick`: portraits only with bars,
    /// O-354) and the scene fade (`FUN_004BC130`, real time `0x004BFF09...0x004BFF1D`). `opennml-game` calls it from
    /// [`Self::advance`], the viewer with its frame time independent of the run; [`Self::advance_by`] does not call it.
    pub fn tick_presentation(&mut self, dt_ms: f32) {
        self.hud.cinemascope.tick(dt_ms);
        self.scene_fade.tick(dt_ms);
    }

    /// Distributes a presentation event of the simulation or by hand (loop of [`Self::advance_by`]): the script
    /// presentation ([`ScriptPresentation::apply`]), scene fade and target highlight ([`scene_event`]), the info
    /// windows ([`info_frame_event`]), the screen text ([`screen_text_event`]), then the UI messages of these actions
    /// to cinemascope area, status bar and command bar ([`toolbar_host::presentation_events`], [`Hud::present`]). The
    /// caller does the speech ([`speech_step`]) once for all events. It appends played sounds (script ping
    /// `SFX_SIGNAL`) to `played`, as [`Self::advance_by`] reports the simulation's sounds.
    fn apply_presentation_event(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        mut audio: Option<&mut AudioEngine>,
        event: &Presentation,
        played: &mut Vec<SessionEvent>,
    ) {
        // Game type and image height for the UI messages of the script presentation (bars only in the campaign, O-350).
        let (campaign, height) = (self.campaign(), self.hud.screen().height);
        // `{0x19, 1}` to the root (`0x00449ADB`): a running selection box ends without a selection (`+0x48 = 0`) and
        // does not come back even with `{0x19, 0}` (O-384).
        if matches!(event, Presentation::UiLock { on: true }) {
            self.select_start = None;
        }
        self.presentation.apply(
            event,
            &mut PresentCtx {
                map: &mut self.map,
                selection: &mut self.selection,
                queue: gpu.queue,
                audio: audio.as_deref_mut(),
                vfs,
            },
        );
        // Script action 7 (`FUN_0043EBE0`): `SFX_SIGNAL` 2D, then the ping with 10 repetitions.
        if let Presentation::MinimapPing {
            player,
            tile_x,
            tile_z,
        } = *event
        {
            let sound = self.hud.script_ping(player, (tile_x, tile_z));
            if let Some(audio) = audio {
                // Reported to the viewer like the session's other sounds (state snapshot of the sound report).
                played.push(SessionEvent::Sound(sound.clone()));
                play_sim_sound(audio, vfs, sound);
            }
        }
        scene_event(&mut self.scene_fade, &mut self.target_highlight, event);
        info_frame_event(&mut self.info_frames, event);
        screen_text_event(&mut self.hud.screen_texts, event);
        self.hud.present(
            crate::game::toolbar_host::presentation_events(event, campaign, height),
            &mut self.toolbar_host,
        );
    }

    /// Triggers a presentation event by hand (control command `ui_present`): like an event of the simulation through
    /// [`Self::apply_presentation_event`], plus the speech ([`speech_step`], only with a sound system) and `{8}` to the
    /// bar if the event clears the selection (147/148, 18). No `SimHost` command and no entry in the command log: the
    /// checksum stays. It does change the host state (speed, fog mode, selection).
    ///
    /// `QuickSave` and `ProfileVar` are requests to the flow ([`HostRequest`]), not presentation: error, nothing
    /// happens.
    pub fn present_by_hand(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        mut audio: Option<&mut AudioEngine>,
        event: &Presentation,
    ) -> Result<(), String> {
        let unsupported = match event {
            Presentation::QuickSave => Some("QuickSave"),
            Presentation::ProfileVar { .. } => Some("ProfileVar"),
            _ => None,
        };
        if let Some(name) = unsupported {
            return Err(format!(
                "ui_present does not know {name} (presentation only)"
            ));
        }
        let selection_before = self.selection.ids.clone();
        // The manual call reports no sounds (return value `()`); the script ping sound is played but not reported.
        self.apply_presentation_event(gpu, vfs, audio.as_deref_mut(), event, &mut Vec::new());
        if self.selection.ids != selection_before {
            self.toolbar_host.selection_changed();
        }
        if let Some(audio) = audio {
            let events = [SimEvent::Present(event.clone())];
            speech_step(&mut self.speech, self.map.sim.as_ref(), &events, audio, vfs);
        }
        Ok(())
    }

    /// [`Self::present_by_hand`] from the JSON form of [`Presentation`] (`ui_present`); an unknown form reports serde's
    /// error.
    pub fn present_json(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
        event: &serde_json::Value,
    ) -> Result<(), String> {
        let event: Presentation =
            serde_json::from_value(event.clone()).map_err(|e| format!("ui_present: {e}"))?;
        self.present_by_hand(gpu, vfs, audio, &event)
    }

    /// Presentation of the campaign UI for `ui_state.game.presentation`; read-only. `pointer_visible` is the real
    /// pointer state of the menu host (`!MenuApp::pointer_hidden`), not the lock.
    /// - `bars`: bars at top and bottom in screen pixels, clipped as when drawing (`game_rect(0, 0, 1280, bar)` or `(0,
    ///   1024 - bar, 1280, 1024)`, O-351); `[0, 0]` without bars.
    /// - `portraits`, `texts`: the slots with state bit 1 (`CinemascopeView::slots`). Portraits also report without
    ///   bars (the state), they are only drawn with bars (O-354).
    /// - `fade_alpha`: alpha of the scene fade ([`SceneFade::alpha`]).
    /// - `clear_color`: [`Self::clear_color`] per channel `(c * 255)` rounded.
    /// - `info_frames`: [`InfoFrames::rects`] (measured windows from head to end); `countdown`: `m:ss` with a
    ///   registered container ([`countdown_text`]); `timers`: the remaining times of the timer list as `m:ss` in ID
    ///   order (`ms / 1000`, C division); `screen_text`: text ID of the first screen text of kind 2.
    pub fn presentation_state(&self, pointer_visible: bool) -> UiPresentationData {
        let screen = self.hud.screen();
        let bar = self.hud.cinemascope.bar;
        let bars = if bar == 0 {
            [0, 0]
        } else {
            let top = game_rect(screen, [0, 0, VIRTUAL_W, bar]);
            let bottom = game_rect(screen, [0, VIRTUAL_H - bar, VIRTUAL_W, VIRTUAL_H]);
            [top[3] - top[1], bottom[3] - bottom[1]]
        };
        let (portraits, texts) = self.hud.cinemascope.slots();
        UiPresentationData {
            bars,
            portraits: portraits
                .into_iter()
                .map(|(slot, portrait, frame, alpha)| UiPortraitSlot {
                    slot: slot as u8,
                    portrait,
                    frame,
                    alpha,
                })
                .collect(),
            texts: texts
                .into_iter()
                .map(|(slot, text, alpha)| UiTextSlot {
                    slot: slot as u8,
                    text,
                    alpha,
                })
                .collect(),
            fade_alpha: self.scene_fade.alpha(),
            pointer_visible,
            clear_color: self.clear_color().map(|c| (c * 255.0).round() as u8),
            info_frames: self.info_frames.rects(),
            countdown: self
                .sim()
                .map(|s| &s.state.script.mission)
                .filter(|m| m.countdown_container != 0)
                .map(|m| countdown_text(m.countdown_s)),
            timers: self
                .sim()
                .map(|s| {
                    s.state
                        .countdowns
                        .values()
                        .map(|&ms| countdown_text(ms / 1000))
                        .collect()
                })
                .unwrap_or_default(),
            screen_text: self
                .hud
                .screen_texts
                .first_of(screen_text::kind::MIDDLE)
                .map(|t| t.text),
        }
    }

    /// A frame of `dt_ms` milliseconds at the clock [`SessionClock::now_ms`]: camera (with the held keys, the screen
    /// edge and the look; under the input lock the game camera stands still), simulation, rendering of its events, the
    /// script presentation ([`ScriptPresentation::apply`] in event order, then [`ScriptPresentation::update`] in game
    /// time), selection changes, messages, sounds and voices, weather, effects, surf and the sound of the map
    /// ([`GameSession::update_level_audio`]). Without `running` nothing happens (halt branch).
    pub fn advance_by(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        dt_ms: f32,
        running: bool,
        mut audio: Option<&mut AudioEngine>,
    ) -> Vec<SessionEvent> {
        let mut played = Vec::new();
        if !running {
            return played;
        }
        let clock_ms = self.clock.now_ms as f32;
        // Music after loading without sound (`restore` in `resume_from`/`reset_view`) in the first frame with a sound
        // system.
        if let Some(audio) = audio.as_deref_mut() {
            self.presentation.resume_audio(&self.map, audio, vfs);
        }
        // The player camera (keys, edge) stands still under the lock and while the scripts' camera runs
        // ([`InputGate::player_camera`]; a binding 175 alone does not lock); the control keeps running with empty input
        // so that the acceleration dies down.
        let input = self.camera_frame_input();
        self.pointer.look = Vec2::ZERO;
        // Game time of this frame with the speed before the step (the fog crossfade 506). The scripts' camera also gets
        // the time of the stopped frames of the menu level (O-447, `GameEngine_UpdateCameraFollow` `0x00432590` with
        // the real time since the last `Update`, without an upper limit).
        let speed = self.sim().map_or(10, |s| s.game_speed);
        let dt_game = game_ms(dt_ms, speed);
        let camera_dt_game = game_ms(dt_ms + self.clock.take_held_ms(), speed);
        advance_camera(
            &mut self.controller,
            &mut self.map.camera,
            self.map.free.as_mut(),
            &self.map.scene.terrain,
            input,
            &self.controls,
            dt_ms,
        );
        // Weather and effects with the camera of this step: only visible tiles move, effects outside freeze.
        // Game time of the simulation from the frame time (`GameSpeed`).
        // Beforehand the host reports its selection to the mirror of conditions 35-37 (only on change and only if the
        // script uses them).
        let selection = &self.selection.ids;
        let now = (self.clock.program_ms as u64).saturating_add(self.net_time_offset_ms);
        let (net, pending) = (&mut self.net, &mut self.net_frame);
        let step = self
            .map
            .sim
            .as_mut()
            .map_or_else(SessionStep::default, |sim| {
                sim.sync_selection(selection);
                match net.as_mut() {
                    // In the network the turn driver computes the ticks: contributions, bundle, halt, sync.
                    Some(net) => net
                        .advance(sim, f64::from(dt_ms), now)
                        .into_session(pending),
                    None => SessionStep {
                        events: sim.advance(f64::from(dt_ms)),
                        ..SessionStep::default()
                    },
                }
            });
        let SessionStep {
            events,
            speed: speed_changes,
            adopted,
        } = step;
        self.map
            .apply_sim_events(gpu.device, gpu.queue, vfs, &events);
        if adopted {
            self.rebuild_after_adopt(gpu, vfs, audio.as_deref_mut());
        }
        // State before the step for the command acknowledgments (silent in the cinemascope, `0x0047BBC0`).
        let cinemascope_before = self.presentation.cinemascope;
        // Selection before the step: also clearing by cinemascope and lock reports `{8}` to the bar.
        let selection_before = self.selection.ids.clone();
        // Script presentation in event order, then the fog crossfade in game time.
        for event in &events {
            if let SimEvent::Present(p) = event {
                self.apply_presentation_event(gpu, vfs, audio.as_deref_mut(), p, &mut played);
            }
        }
        self.presentation.update(
            dt_game,
            camera_dt_game,
            &mut PresentCtx {
                map: &mut self.map,
                selection: &mut self.selection,
                queue: gpu.queue,
                audio: audio.as_deref_mut(),
                vfs,
            },
        );
        let map = &mut self.map;
        // Speech of the dialog lines and tip sound (`Dialog_PlaySpeech`, `FUN_00443450`, `FUN_0043E250`), once per
        // event; nothing without a sound system.
        if let Some(audio) = audio.as_deref_mut() {
            speech_step(&mut self.speech, map.sim.as_ref(), &events, audio, vfs);
        }
        // Messages are invisible and silent in the cinemascope (`Engine+0x104`), already for the messages of this frame
        // (O-217).
        self.hud.messages.cinemascope = self.presentation.cinemascope;
        // Fire sounds of the buildings (action 102) take the way of the combat sounds.
        let fire_sounds = map.take_fire_sounds();
        // Wading noises of the units (`SoundManager_PlayEvent3D`, re/world-display W.3.4).
        let movement_sounds = map.take_movement_sounds();
        // Game time of the bar before its tick of this frame: in the original the floating numbers of the step arise in
        // the game tick and then run this `dt` with `FUN_00494A00`.
        let bar_ms = self.hud.bar_ms;
        if let Some(sim) = &map.sim {
            // Selection changes of the step: mount with a new rider (re/riding 4.7; merging triggers no sound), end of
            // a tent crossfade tent <-> travois (re/tents 3.3/3.5), a locomotive leaving the map
            // (`SimEvent::Deselected`), vanished objects.
            let before = selection_before;
            let reselected = self.selection.apply_sim_step(
                &events,
                &sim.state,
                &sim.rules,
                |o| sim.fog_selectable(o),
                sim.active_player,
            );
            // Every selection change of the step reports `{8}` to the bar (quartering, death, handover, deselecting);
            // the selection sound is played by the branch below. The sequence of IDs is compared: even a mere
            // reordering without new or removed objects reports `{8}` and rebuilds fully (the first object determines
            // the menu).
            if reselected || self.selection.ids != before {
                self.toolbar_host.selection_changed();
            }
            // Credits and damage numbers (at the bar's game time), text messages (at the rendering clock) and sounds of
            // the simulation, then the message bar: events, then its tick in game time (`GameEngine_UpdateCamera`
            // `0x0043D19C` after `GameEngine_Update`; O-213).
            let mut sounds = map.notices.handle(&events, sim, self.clock.now_ms, bar_ms);
            self.hud.handle(&events, sim.active_player, &mut sounds);
            // Speed changes of the turns on every machine (E-294).
            post_speed_messages(&mut self.hud, &speed_changes, &mut sounds);
            // Chat lines `0x5D` (`FUN_0042E980` when executing, before the bar's tick `FUN_00494A00`).
            let names = &self.player_names;
            chat::show(
                &events,
                sim.active_player,
                self.size.1 as i32,
                |slot| match names {
                    Some(n) => n[usize::from(slot) % PLAYER_SLOTS].clone(),
                    None => sim.state.players[usize::from(slot) % PLAYER_SLOTS]
                        .name
                        .clone(),
                },
                |slot| {
                    opennml_world::markers::marker_argb(
                        sim.state.players[usize::from(slot) % PLAYER_SLOTS].color,
                    )
                },
                &mut self.hud.screen_texts,
            );
            self.hud.tick(dt_ms, sim.game_speed, &mut sounds);
            sounds.extend(fire_sounds);
            sounds.extend(movement_sounds);
            // Command acknowledgments (on command execution at the start of the tick), then the selection sound of the
            // new selection (`FUN_00457980` -> `0x0044BB60`) when the game shrinks the selection (event 8 from
            // `GameWorld_DeselectObject` `0x004801B0`) or a tent handover has swapped it: packing up -> voice of the
            // travois, pitching -> noise of the tent (type lock) or voice of the unit left alone (`0x0052DD20`,
            // `0x0052DD60`, `{0x10, 0}` -> `FUN_00457DB0`). There is no separate deselect sound; at most one selection
            // sound per step.
            let mut voices = self
                .voices
                .responses(&events, sim.active_player, cinemascope_before);
            if reselected {
                voices.extend(self.voices.selection(
                    &sim.state,
                    &self.selection.ids,
                    sim.active_player,
                ));
            }
            if let Some(audio) = audio.as_deref_mut() {
                let target = map.camera.target;
                let height = map.scene.terrain.ground_height(target.x, target.z);
                audio.sound.set_listener(target, height, map.camera.eye);
                for sound in sounds {
                    played.push(SessionEvent::Sound(sound.clone()));
                    play_sim_sound(audio, vfs, sound);
                }
                for voice in voices {
                    self.voices.play(audio, vfs, voice);
                }
            }
        }
        // Digits of the dynamite charges behind the messages, at the same clock (`0x004F07A0`).
        map.charge_digits(vfs, bar_ms);
        let params = map.view_params(self.size, self.view.aspect.into(), false, clock_ms);
        let view_proj = params.projection * params.view;
        map.advance_weather(view_proj, dt_ms);
        let poses = self.view.animate.then_some(clock_ms);
        map.advance_effects(vfs, view_proj, dt_ms, poses);
        // Surf in the game tick; distance to the listener of this step (camera above).
        if let Some(audio) = audio.as_deref_mut() {
            let target = map.camera.target;
            let height = map.scene.terrain.ground_height(target.x, target.z);
            audio.sound.set_listener(target, height, map.camera.eye);
            for surf in map.effects.take_surf_sounds() {
                audio.play_surf(vfs, surf.guid, surf.position);
            }
            self.update_level_audio(audio, vfs);
        }
        // Object under the pointer after the step (`FUN_0047A490` from `GameEngine_RenderWorld`) and the events of this
        // frame to the command bar.
        self.hovered = self.pointer.cursor.and_then(|c| {
            let params = self.pick_params();
            self.map.pick_target(&params, c, self.screen)
        });
        if let Some(sim) = self.map.sim.as_ref() {
            let hover = hover_target(sim, self.hovered);
            self.toolbar_host
                .frame(sim, &self.selection.ids, &events, hover);
        }
        // Frame step of the command bar: events, frame event 6, verdict of the cursor.
        self.toolbar_step(vfs, audio);
        // Mouse pointer after the verdict of the bar cursor.
        self.pointer_step();
        // Selection changes of this frame (game, scripts, bar) to the control groups.
        self.sync_groups();
        // Fog rows of the minimap from the empty ticks of this frame, before drawing.
        self.sync_minimap();
        played
    }

    /// Input at program clock `now` without host modes ([`GameSession::input_with`]); `screen` is the area of the
    /// pointer.
    pub fn input(
        &mut self,
        ev: &InputEvent,
        screen: Screen,
        now: u64,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) {
        let screen = (screen.width.max(0) as u32, screen.height.max(0) as u32);
        self.input_with(ev, screen, now as f64, vfs, audio, &mut NoModes);
    }

    /// Input on the map at clock `now_ms` (double-click): held keys, pointer (look, formation dragging), left button
    /// (selection; with a host mode its click), right button (right-click command per `targets.ini`, formation
    /// dragging, double-click), mouse wheel (zoom of the game camera).
    ///
    /// Afterwards the session syncs the control groups with the selection ([`GameSession::sync_groups`]).
    pub fn input_with(
        &mut self,
        ev: &InputEvent,
        screen: (u32, u32),
        now_ms: f64,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
        modes: &mut dyn HostModes,
    ) {
        self.input_map(ev, screen, now_ms, vfs, audio, modes);
        self.sync_groups();
    }

    fn input_map(
        &mut self,
        ev: &InputEvent,
        screen: (u32, u32),
        now_ms: f64,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
        modes: &mut dyn HostModes,
    ) {
        self.screen = screen;
        let mut audio = audio;
        // Every release counts for the double-click (`FUN_004145B0`), on the right with its own clock (timestamp
        // `+0x54`); the result applies to the windows (`0x13`/`0x12` after 2/4) and the world.
        let double = match *ev {
            InputEvent::Button {
                button,
                pressed: false,
            } => {
                let side = match button {
                    MouseButton::Left => MouseSide::Left,
                    MouseButton::Right => MouseSide::Right,
                    // The middle double-click (`0x2C`, clock `+0x50`) reaches no window (O-331).
                    MouseButton::Middle => return,
                };
                self.clicks
                    .release(side, now_ms, self.controls.double_click_ms)
            }
            _ => false,
        };
        // Every movement re-chooses the pointer (event 1, evaluated in the frame step: `GameSession::pointer_step`),
        // whether or not a window takes it.
        if matches!(ev, InputEvent::CursorMoved { .. }) {
            self.mouse_pointer.moved();
        }
        // The mouse first goes to the window tree of the game UI (O-222); only what no window takes goes to the world
        // and the host's modes. The input lock (`GE+0x600`) does not keep the bar away: the windows see the mouse
        // before the root (`FUN_004B8E70` -> `FUN_004528F0` without checking the lock), only root (`+0x60`,
        // `0x00449B00`) and minimap (`+0x4C`) are locked; the bar itself gets `{0x19, 1}` (cursor closed, rebuild
        // without selection). O-385: with a running selection box slot 3 of the root takes the release (raw kind 2)
        // before all windows (`0x004528F0`: slot 3, children, slot 2; `0x00449AEF` -> `0x004498A0`), also over bar or
        // minimap. The double-click `0x13` then takes the usual path: first the windows, then slot 2 of the root, there
        // only without lock (`0x00449B45`).
        if let InputEvent::Button {
            button: MouseButton::Left,
            pressed: false,
        } = *ev
            && self.select_start.is_some()
        {
            self.pointer.left = false;
            if !modes.left_release(self) && self.finish_rect(vfs, audio.as_deref_mut()) && double {
                let taken = self.toolbar_double_left(ev, vfs, audio.as_deref_mut());
                if !taken && self.input_gate().world_click() {
                    self.double_click_at_pointer(vfs, audio);
                }
            }
            return;
        }
        if self.toolbar_input(ev, double, vfs, audio.as_deref_mut()) {
            return;
        }
        // The world after the input gate ([`InputGate`], re/campaign-ui D6.10): left click, box, double-click and the
        // host's modes only with `world_click` (lock: root `+0x60`, `0x00449B00`), the wheel only with `player_camera`;
        // the right-click command also works under the lock (O-233). A locked press only remembers the button, a
        // started click of the same button lapses.
        let gate = self.input_gate();
        let blocked = match *ev {
            InputEvent::Button {
                button: MouseButton::Left,
                ..
            } => !gate.world_click(),
            InputEvent::Button {
                button: MouseButton::Right,
                ..
            } => !gate.right_click_command(),
            InputEvent::Wheel { .. } => !gate.player_camera(),
            _ => false,
        };
        if blocked {
            if let InputEvent::Button { button, pressed } = *ev {
                match button {
                    MouseButton::Left => {
                        self.pointer.left = pressed;
                        self.select_start = None;
                    }
                    MouseButton::Right => {
                        self.pointer.right = pressed;
                        self.right_start = None;
                        self.right_drag = None;
                    }
                    MouseButton::Middle => {}
                }
            }
            return;
        }
        match *ev {
            InputEvent::Key { code, pressed, .. } => {
                if pressed {
                    self.pointer.keys.insert(code);
                } else {
                    self.pointer.keys.remove(&code);
                }
            }
            InputEvent::CursorMoved { x, y } => {
                if let Some((old_x, old_y)) = self.pointer.cursor
                    && self.pointer.right
                {
                    self.pointer.look += Vec2::new((x - old_x) as f32, (y - old_y) as f32);
                }
                self.pointer.cursor = Some((x, y));
                // Formation dragging: every mouse movement with the button held (`0x00453790`).
                if self.pointer.right
                    && self.right_drag.is_some()
                    && let Some(p) = self.ground_at((x, y))
                    && let Some(drag) = self.right_drag.as_mut()
                {
                    drag.moved(p);
                }
            }
            InputEvent::CursorLeft => self.pointer.cursor = None,
            InputEvent::Button {
                button: MouseButton::Right,
                pressed,
            } => {
                self.pointer.right = pressed;
                if pressed {
                    // The right press searches the pointer like a movement (`0x00453790` at event 5).
                    self.mouse_pointer.right_down();
                    self.right_start = self.pointer.cursor;
                    // Start of formation dragging; with the free camera the button rotates.
                    let free_camera = self.map.free.is_some();
                    self.right_drag = self
                        .pointer
                        .cursor
                        .filter(|_| !free_camera)
                        .and_then(|c| self.ground_at(c))
                        .map(FormationDragTracker::new);
                } else {
                    self.finish_right_click(double, modes);
                }
            }
            InputEvent::Button {
                button: MouseButton::Left,
                pressed,
            } => {
                self.pointer.left = pressed;
                if pressed {
                    // In placement mode, with the unpack cursor and in targeting mode the click places instead of
                    // selecting (no selection change).
                    if !modes.left_press() {
                        self.select_start = self.pointer.cursor;
                    }
                } else {
                    if !modes.left_release(self) {
                        self.finish_selection(double, vfs, audio);
                    }
                }
            }
            // The middle button only acts via the table of held keys (`KD_CAM_FREEHAND`, [`Self::held_vk`]); its raw
            // events `0x2A`...`0x2C` are not evaluated by any slot of the game UI (O-331).
            InputEvent::Button {
                button: MouseButton::Middle,
                ..
            } => {}
            InputEvent::Wheel { up } => {
                // Lock and the scripts' camera are kept out by `InputGate::player_camera` above. The wheel also does
                // not zoom while any move is running or waiting (also Home; `0x0043D0B2`) and while `KD_CHECK_LBUTTON`
                // is held (`0x0043C7EA`). The original remembers the wheel then and acts once later (memory bit
                // `+0x10C`, `FUN_00413E80`); the reimplementation discards it (deviation, re/renderer, section
                // "Movement in the game"). In pause the original reads the wheel and discards it: the transition would
                // go into the move list (`FUN_00433890`), which the pause skips (`FUN_0042D840`, `0x0043D0E9`). A Home
                // move waiting there therefore changes nothing.
                if self.map.free.is_none()
                    && !self.pause.sim
                    && !self.presentation.camera.moving()
                    && !self.left_button_holds_camera()
                {
                    self.controller.wheel(&self.map.camera, up, &self.controls);
                }
            }
        }
    }

    /// View of the game camera for ray tests in the pointer area (without fog).
    pub fn pick_params(&self) -> ViewParams {
        self.map.view_params(
            self.screen,
            self.view.aspect.into(),
            false,
            self.clock.now_ms as f32,
        )
    }

    /// Terrain point under the pixel `point` (game camera view of the current frame).
    pub fn ground_at(&self, point: (f64, f64)) -> Option<Vec3> {
        self.map
            .ground_point(&self.pick_params(), point, self.screen)
    }

    /// Left mouse button released: under 2 pixels a click (`click_target`), otherwise a box over the pixels of the own
    /// objects (`0x004498A0`); Shift adds. If it was the second release within `DoubleClickTimeout` (`double`), the
    /// double-click at the pointer follows as in the original (event `0x13` after event 2; `0x00449450` ->
    /// `0x0048A0C0`).
    pub(crate) fn finish_selection(
        &mut self,
        double: bool,
        vfs: &Vfs,
        mut audio: Option<&mut AudioEngine>,
    ) {
        if self.finish_rect(vfs, audio.as_deref_mut()) && double {
            self.double_click_at_pointer(vfs, audio);
        }
    }

    /// Click or box of the release (`0x004498A0`, first part of [`GameSession::finish_selection`]): `false` without a
    /// started box, without a pointer or without a simulation.
    fn finish_rect(&mut self, vfs: &Vfs, audio: Option<&mut AudioEngine>) -> bool {
        let (Some(start), Some(end)) = (self.select_start.take(), self.pointer.cursor) else {
            return false;
        };
        let add = self.pointer.shift();
        let size = self.screen;
        let params = self.pick_params();
        let map = &self.map;
        let Some(sim) = &map.sim else {
            return false;
        };
        let (x0, x1) = (start.0.min(end.0) as f32, start.0.max(end.0) as f32);
        let (y0, y1) = (start.1.min(end.1) as f32, start.1.max(end.1) as f32);
        // Fog check `0x00467D70` when adding (`0x00488F60`).
        let fog = |o: &opennml_sim::object::GameObject| sim.fog_selectable(o);
        if x1 - x0 < 2.0 || y1 - y0 < 2.0 {
            let target = map.pick_target(&params, end, size);
            self.selection
                .click(&sim.state, &sim.rules, fog, sim.active_player, target, add);
        } else {
            let points = map.sim_screen_points(&params, size);
            let inside = |o: &opennml_sim::object::GameObject| {
                points
                    .get(&o.id)
                    .is_some_and(|p| x0 < p[0] && p[0] < x1 && y0 < p[1] && p[1] < y1)
            };
            self.selection
                .rect(&sim.state, &sim.rules, fog, sim.active_player, inside, add);
        }
        // Every release on the map reports "selection changed" (UI event 8), even without a change.
        self.selection_changed(vfs, audio);
        true
    }

    /// Left double-click at the pointer (event `0x13` to slot 2 of the root, `0x00449450` -> `0x0048A0C0`): the object
    /// under the pointer (a carrier becomes its occupant), plus all visible own units of the same kind; event 8 only
    /// with a target. Target and visibility list do not depend on the selection, so the sequence after the box changes
    /// nothing.
    fn double_click_at_pointer(&mut self, vfs: &Vfs, audio: Option<&mut AudioEngine>) {
        let Some(end) = self.pointer.cursor else {
            return;
        };
        let add = self.pointer.shift();
        let size = self.screen;
        let params = self.pick_params();
        let map = &self.map;
        let Some((target, visible)) = map
            .pick_target(&params, end, size)
            .map(|target| (target, map.visible_sim_objects(&params, size)))
        else {
            return;
        };
        let Some(sim) = self.map.sim.as_ref() else {
            return;
        };
        let fog = |o: &opennml_sim::object::GameObject| sim.fog_selectable(o);
        self.selection.double_click(
            &sim.state,
            &sim.rules,
            fog,
            sim.active_player,
            Some(target),
            add,
            &visible,
        );
        self.selection_changed(vfs, audio);
    }

    /// Selection changed (`FUN_00457980`): `{8}` to the command bar ([`ToolbarHost::selection_changed`], without the
    /// bar's sound) and the selection sound or voice as in `Selection_PlaySelectSound` ([`Voices::selection`]): one
    /// `{8}`, one selection sound.
    pub fn selection_changed(&mut self, vfs: &Vfs, audio: Option<&mut AudioEngine>) {
        self.toolbar_host.selection_changed();
        let Some(sim) = self.map.sim.as_ref() else {
            return;
        };
        let Some(request) =
            self.voices
                .selection(&sim.state, &self.selection.ids, sim.active_player)
        else {
            return;
        };
        if let Some(audio) = audio {
            self.voices.play(audio, vfs, request);
        }
    }

    /// Right mouse button released: a click at the press point ([`right_click_point`]; only with the free camera is a
    /// mouse travel of 3 pixels or more rotating). If the click ends a host mode ([`HostModes::cancel_right_click`]),
    /// nothing else happens; in attack mode ([`HostModes::take_attack`]) the attack once
    /// ([`SimHost::issue_attack_action`]); otherwise the action per `targets.ini` for the right-click target (ranks of
    /// `0x00480E80`, also trees, finally active fog memories) or the terrain point ([`SimHost::issue_target_action`]);
    /// Shift queues, Ctrl cycles (only the actions that the original wraps); Ctrl sets modifier 4 when attacking (411).
    /// Nothing without a matching action. A right double-click (`double`, event `0x12` -> `0x00457C50`) issues the same
    /// action a second time with modifier 1 ("Go to" and attacking run, D-227); only in the normal branch, not in
    /// attack mode (statically open in the original).
    fn finish_right_click(&mut self, double: bool, modes: &mut dyn HostModes) {
        let right_drag = self.right_drag.take();
        let (Some(start), Some(end)) = (self.right_start.take(), self.pointer.cursor) else {
            return;
        };
        let free_camera = self.map.free.is_some();
        let Some(point) = right_click_point(start, end, free_camera) else {
            return;
        };
        if modes.cancel_right_click() {
            return;
        }
        let shift = self.pointer.shift();
        let ctrl = self.pointer.ctrl();
        let size = self.screen;
        let now = self.clock.now_ms as f32;
        let params = self.pick_params();
        let map = &mut self.map;
        let Some(ground) = map.ground_point(&params, point, size) else {
            return;
        };
        let target = map.pick_right_click_target(&params, point, size, &self.selection.ids);
        // Formation dragging (game camera only): ground points in world coordinates from the press point to the last
        // mouse movement beyond 16 world units, measured in the mouse movements; on release nothing is recomputed, as
        // in the original (`0x00457C50`). Facing and spread only for "Go to" 448 (re/groups-formations 12.6).
        let drag = right_drag.filter(|_| !free_camera).and_then(|t| t.drag());
        let order = modes.formation(drag, &self.toolbar_host.bar);
        if let Some(sim) = map.sim.as_mut() {
            // Attack mode (key X): once instead of the action per `targets.ini`.
            if modes.take_attack() {
                let target_id = target.map(RightClickTarget::id);
                let kinds = sim.issue_attack_action(
                    &self.selection.ids,
                    target_id,
                    ground,
                    queue_keys(shift, ctrl),
                    order,
                );
                if kinds.is_empty() && !self.selection.ids.is_empty() {
                    log::debug!("Attack mode: no action");
                }
                let issued_any = !kinds.is_empty();
                let attack = kinds
                    .into_iter()
                    .find(|k| matches!(k, opennml_sim::command::CommandKind::Attack { .. }))
                    .map(|k| (opennml_sim::targeting::button::ATTACK, k));
                let before = self.target_highlight;
                on_target_action(&mut self.target_highlight, attack.as_ref(), target_id, now);
                replace_script_highlight(
                    before,
                    self.target_highlight,
                    &mut self.presentation.highlight,
                );
                if issued_any {
                    let pos =
                        target_id.and_then(|id| sim.state.object(id, false).map(|o| o.world_pos));
                    let decal = decal_for(opennml_sim::targeting::button::ATTACK, pos, ground);
                    self.start_target_decal(decal);
                }
                return;
            }
            // `0x00457C50`: with a found row and an own (or empty) selection, sending sets pointer 7 until the next
            // movement (`0x00457D8F`); asked before sending because a command takes effect immediately in pause.
            if sim.right_click_sends(&self.selection.ids, target) {
                self.mouse_pointer.right_click_sent();
            }
            let issued = sim.issue_right_click(
                &self.selection.ids,
                target,
                ground,
                queue_keys(shift, ctrl),
                ctrl,
                order,
                double,
            );
            if issued.is_empty() && !self.selection.ids.is_empty() {
                log::debug!("Right-click: no action");
            }
            // Target highlight like `Input_OnTargetButtonUp` (`0x00457C50`, there before sending; the order has no
            // effect here, the highlight is purely rendering); on a double-click per output.
            let before = self.target_highlight;
            for action in &issued {
                on_target_action(
                    &mut self.target_highlight,
                    Some(action),
                    target.map(RightClickTarget::id),
                    now,
                );
            }
            replace_script_highlight(
                before,
                self.target_highlight,
                &mut self.presentation.highlight,
            );
            // Target point (O-237): `Input_IssueTargetAction` sends `0xFA` also for the world's right-click
            // (`0x00457C50`), for the buttons 427, 448, 530, 529, 533 (re/game-ui N.5.8); the last output wins.
            let object_pos = target
                .map(RightClickTarget::id)
                .and_then(|id| sim.state.object(id, false).map(|o| o.world_pos));
            let decal = issued
                .iter()
                .rev()
                .find(|(button, _)| TARGET_DECAL_BUTTONS.contains(button))
                .map(|&(button, _)| decal_for(button, object_pos, ground));
            if let Some(decal) = decal {
                self.start_target_decal(decal);
            }
        }
    }

    /// After loading or restarting the simulation: selection, voices, target highlight and ground markers empty, map
    /// from the new state.
    pub fn reset_view(&mut self, gpu: &SessionGpu, vfs: &Vfs) {
        // Map mode 0 as after loading in the original (`GameWorld+0x22D4` is not in the savegame); `Hud::clear` in
        // [`Self::clear_sim_view`] then rebuilds the corner buttons for the mode.
        if let Some(minimap) = self.hud.minimap.as_mut() {
            minimap.mode = 0;
        }
        // Script presentation from the new state (viewer: without remembered speed in the savegame).
        self.clear_sim_view(gpu, vfs, None);
        // A savegame of the simulation (viewer) has no groups and no `selectnextguid` indices: empty as for a new
        // level, the double-tap deadline likewise.
        self.groups = Default::default();
        self.next_guid_tap = GuidTap::default();
        // Likewise without index of the idle workers and masks (host part of `opennml-game`): start values of a new match.
        self.idle_worker_index = [0; PLAYER_SLOTS];
        self.masks = Masks::initial();
    }

    /// Common part of [`Self::reset_view`] and [`Self::rebuild_after_adopt`]: presentation from a new simulation state.
    /// Selection, voices, target and ground markers empty, map from the state (trajectories, fires, effects), message
    /// bar and command bar fresh, dialog line forgotten without sound (the caller or the host takes care of its
    /// fade-out with [`Self::stop_speech`]: viewer in `after_sim_reset`, `opennml-game` when replacing the session), then
    /// the script presentation with the remembered speed `tempo` ([`Self::restore_presentation`], after resetting the
    /// bar so that a loaded cinemascope hides it again); afterwards clusters and selection log empty.
    fn clear_sim_view(&mut self, gpu: &SessionGpu, vfs: &Vfs, tempo: Option<u8>) {
        self.selection.clear();
        self.voices.reset();
        self.target_highlight = None;
        self.ground_markers.clear();
        self.map.reset_sim_view(gpu.device, gpu.queue, vfs);
        self.hud.clear();
        self.hovered = None;
        self.toolbar_host.reset();
        self.last_button = None;
        self.toolbar_previews.clear();
        self.target_decal = None;
        self.toolbar_width = None;
        self.speech.forget();
        self.restore_presentation(gpu.queue, vfs, tempo);
        // After the script presentation (it can clear the selection): clusters and selection log anew.
        self.clusters = SelectionClusters::default();
        self.selection.log.clear();
    }

    /// Poses of a frame at the clock [`SessionClock::now_ms`] and the renderer's data (weather, shadows, effects) for
    /// the view of size `size`, before [`MapView::render_world`].
    pub fn prepare(&mut self, gpu: &SessionGpu, vfs: &Vfs, size: (u32, u32)) -> ViewParams {
        let clock_ms = self.clock.now_ms as f32;
        let map = &mut self.map;
        map.update(gpu.device, gpu.queue, vfs, clock_ms, self.view.animate);
        let mut params = map.view_params(size, self.view.aspect.into(), self.view.fog, clock_ms);
        // Clear color the same for opennml-game and viewer (E-245, O-374).
        params.clear = Some(self.clear_color());
        let map = &mut self.map;
        map.prepare_weather(gpu.device, gpu.queue, &params);
        map.prepare_shadows(gpu.device, gpu.queue, &params);
        let poses = self.view.animate.then_some(clock_ms);
        map.prepare_effects(gpu.device, gpu.queue, vfs, &params, poses);
        // Rally flag with a single selection of an own 447 building (re/world-display W.1, D-257): for viewer and
        // `opennml-game` at this one place.
        let flag = map.sim.as_ref().and_then(|sim| {
            rally_flag(
                &sim.state,
                &sim.rules,
                &self.selection.ids,
                sim.active_player,
            )
        });
        map.set_rally_flag(gpu.device, gpu.queue, vfs, flag.as_ref());
        // Drawing now reads the time only for the water; it also runs in pause (O-317, [`SessionClock::water_ms`]).
        // Without pause that is the same clock.
        ViewParams {
            now_ms: self.clock.water_ms() as f32,
            ..params
        }
    }

    /// Object under the mouse for hover ring and hover health bar: none in the cinemascope (`Engine+0x104`)
    /// (`GameWorld_RenderSelectionMarkers` `0x0047A490` checks the flag before both).
    pub fn hover_shown(&self, hovered: Option<ObjectId>) -> Option<ObjectId> {
        hovered.filter(|_| !self.presentation.cinemascope)
    }

    /// Ground markers of a frame (step 8): selection, target highlight, object under the mouse (`hovered`, the click
    /// target, not the right-click target: no ring on trees; none in the cinemascope, [`GameSession::hover_shown`]).
    pub fn mark_ground(&mut self, gpu: &SessionGpu, vfs: &Vfs, hovered: Option<ObjectId>) {
        let hovered = self.hover_shown(hovered);
        // The scripts' marker ends with its object (`GameWorld_DestroyObject` `0x00481BB0` clears `+0x318`).
        let script = self.presentation.highlight.filter(|&id| {
            self.sim()
                .is_some_and(|s| s.state.object(id, false).is_some())
        });
        self.presentation.highlight = script;
        self.ground_markers = frame_markers(
            &self.map,
            &self.selection.ids,
            &mut self.target_highlight,
            script,
            hovered,
            self.clock.now_ms as f32,
        );
        self.map
            .set_ground_markers(gpu.device, gpu.queue, vfs, &self.ground_markers);
        // Building preview of the bar cursor (before a preview of the viewer), target point and defense ring.
        self.toolbar_world(gpu, vfs);
    }

    /// Draws the world (terrain, objects, effects, fog, ground markers) into `target` of size `size`; with size 0
    /// (minimized) nothing. `size` is the size of the target and from now on also applies to
    /// [`GameSession::advance_by`] ([`GameSession::size`], one source) and to the game UI's window tree. The game UI
    /// comes via [`GameSession::hud_cmds`] (or in the viewer without `--hud` as lines via
    /// [`GameSession::message_lines`]); overlay and menu are drawn on top by the host.
    pub fn render(
        &mut self,
        gpu: &SessionGpu,
        vfs: &Vfs,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        self.size = size;
        self.hud.resize(screen_of(size));
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let params = self.prepare(gpu, vfs, size);
        // The same value as for the bar ([`GameSession::hovered`] from `advance_by`).
        self.mark_ground(gpu, vfs, self.hovered);
        self.map
            .render_world(gpu.device, gpu.queue, encoder, target, size, &params);
    }

    /// Readable message lines with color: the heads of the message bar per kind in file order (game tags resolved with
    /// `texts`, [`opennml_ui::hud::message_bar::MessageBar::lines`]), then the standing text messages at the rendering
    /// clock. For the viewer overlay without `--hud`; drawing goes via [`GameSession::hud_cmds`].
    pub fn message_lines(&self, texts: &Texts) -> Vec<(String, u32)> {
        let mut lines = match self.sim() {
            Some(sim) => self.hud.messages.lines(&SimGameText::new(sim, texts)),
            None => Vec::new(),
        };
        lines.extend(self.map.notices.text_message_lines(self.clock.now_ms));
        lines
    }

    /// Draw commands of the game UI of a frame (after the world, before menu and pointer): messages of the world pass,
    /// then the game windows ([`Hud::draw`]: status bar, command bar, cinemascope area, tooltip; re/campaign-ui D6.4);
    /// finally the headhunters' text messages ([`crate::game::notices::Notices::draw_text_messages`], provisionally at
    /// the old place, above the status bar so that they stay readable). In the cinemascope the bars hide themselves
    /// (`{0x11, 1}`, `{0x1C, 0}`) and so do the messages (O-217). Game values and names via `texts`. Nothing without a
    /// simulation or with [`GameSession::hud_enabled`] off. Read-only (no checksum changes).
    pub fn hud_cmds(&self, ctx: &mut UiContext<'_>, texts: &Texts) -> Vec<DrawCmd> {
        let mut out = Vec::new();
        self.hud_cmds_into(ctx, texts, false, None, &mut out);
        out
    }

    /// [`GameSession::hud_cmds`] to `out`; with `pause_text` the PAUSE lettering lies between the messages of the world
    /// pass and the game windows (`GameEngine_RenderWorld`: messages `0x00438F2A`, PAUSE at the end `0x004390F9`; the
    /// UI then draws the windows including the cinemascope area), also without the game UI. With `drag` the selection
    /// box follows ([`GameSession::selection_rect`]): the root draws itself before its children (`0x004529A0`), so
    /// above the world pass and below all game windows.
    fn hud_cmds_into(
        &self,
        ctx: &mut UiContext<'_>,
        texts: &Texts,
        pause_text: bool,
        drag: Option<([i32; 2], [i32; 2])>,
        out: &mut Vec<DrawCmd>,
    ) {
        let draw_pause = |ctx: &mut UiContext<'_>, out: &mut Vec<DrawCmd>| {
            if pause_text {
                let text = texts.display(opennml_ui::hud::pause_text::PAUSE_TEXT);
                opennml_ui::hud::pause_text::draw(ctx, &text, out);
            }
            if let Some((start, end)) = drag {
                opennml_ui::hud::selection_rect::draw(start, end, out);
            }
        };
        // In the cinemascope bars and messages hide themselves (`{0x11,1}`, `{0x1D,0}`, `{0x1C,0}`, re/campaign-ui 4.2,
        // O-217); the cinemascope area draws in the game windows (re/campaign-ui D6.4).
        let Some(sim) = self.sim().filter(|_| self.hud_enabled) else {
            draw_pause(ctx, out);
            return;
        };
        let game = SimGameText::new(sim, texts);
        let values = Hud::status_values(sim, texts);
        // What the bar needs for drawing: portrait and counter-upgrades, the game values of the tooltip with selection
        // and formation, the key layout for `[KEYID]`. The snapshot for the buttons is empty: their state comes from
        // the frame step, drawing does not read it.
        let model = opennml_ui::hud::toolbar::ToolbarModel::new(&sim.rules.toolbar);
        let portrait = self
            .hud
            .toolbar
            .as_ref()
            .map(|t| portrait_model(sim, t))
            .unwrap_or_default();
        let (tip, keys) = self.bar_texts(sim, texts);
        let bar = crate::game::hud::BarFrame {
            model: &model,
            portrait: &portrait,
            game: &tip,
            keys: &keys,
        };
        let minimap = self.minimap_frame(sim, ctx);
        self.hud.draw_world_pass(ctx, &game, out);
        draw_pause(ctx, out);
        self.hud
            .draw_windows(ctx, &game, &values, Some(&bar), minimap.as_ref(), out);
        self.map
            .notices
            .draw_text_messages(self.clock.now_ms, ctx, out);
    }

    /// Data of the minimap for a frame of size `ctx.screen`, only if it is drawn: node with `IsMap` and visible
    /// ([`Hud::minimap_shown`]) and no cinemascope ([`InputGate::minimap_visible`]; the original hides it there with
    /// `{0x1D, 0}`, re/game-ui D5.1). Position from the game camera, points in the map's mode, camera frame via the
    /// rays of this frame's game view ([`MapView::pick_ray`]) and the front fog image in the image collection of `ctx`.
    /// Read-only.
    fn minimap_frame(
        &self,
        sim: &SimHost,
        ctx: &mut UiContext<'_>,
    ) -> Option<crate::game::minimap::MinimapFrame> {
        use crate::game::minimap::draw;
        let m = self.hud.minimap.as_ref()?;
        if !self.minimap_drawn() {
            return None;
        }
        let size = draw::screen_size(ctx.screen);
        let terrain = m.terrain();
        let layout = draw::layout(size, &self.map.camera, (terrain.dim_x, terrain.dim_z));
        let params = self.map.view_params(
            size,
            self.view.aspect.into(),
            false,
            self.clock.now_ms as f32,
        );
        let camera = draw::camera_frame(&layout, size, sim.rules.map.cell, |p| {
            let ray = self.map.pick_ray(&params, p, size);
            (ray.origin, ray.direction)
        });
        Some(crate::game::minimap::MinimapFrame {
            layout,
            dots: draw::dots(m.mode, sim),
            pings: self.hud.pings.dots(terrain.dim_x, sim.rules.map.cell),
            camera,
            texture: m.texture(ctx.assets),
        })
    }

    /// Combat display of a frame as draw commands (calculation [`combat_calc::frame_overlay`], commands
    /// [`combat_calc::overlay_cmds`]), then the screen texts, the floating numbers
    /// ([`crate::game::notices::Notices::draw_floats`]) and the script countdown (re/campaign-ui D6.12 and D6.9, only
    /// with a registered container): rank and counter-upgrade icons, the health bars and group digits
    /// ([`combat_calc::group_icons`]) of the selection and of the object under the mouse ([`GameSession::hovered`], the
    /// same value as for the ground rings; none in the cinemascope, [`GameSession::hover_shown`], `0x0047A490` checks
    /// `Engine+0x104` before hover ring and hover bar). The top edges of the hulls come from the last
    /// [`GameSession::render`] (call after it, then they belong to this frame), the camera from the game view at the
    /// size of `ctx.screen`. Independent of [`GameSession::hud_enabled`]: bars and icons belong to the world pass in
    /// the original (`GameEngine_RenderWorld` `0x00438C60`), not to the UI. Nothing without a simulation. Read-only (no
    /// checksum changes).
    pub fn combat_cmds(&self, ctx: &mut UiContext<'_>) -> Vec<DrawCmd> {
        let mut out = Vec::new();
        let Some(sim) = self.sim() else {
            return out;
        };
        let size = (
            u32::try_from(ctx.screen.width).unwrap_or(0),
            u32::try_from(ctx.screen.height).unwrap_or(0),
        );
        if size.0 == 0 || size.1 == 0 {
            return out;
        }
        let params = self.map.view_params(
            size,
            self.view.aspect.into(),
            false,
            self.clock.now_ms as f32,
        );
        let camera_right = params.view.inverse().x_axis.truncate().normalize_or_zero();
        let hovered = self.hover_shown(self.hovered);
        let tops = self.map.sim_top_points();
        let view_proj = params.projection * params.view;
        let mut overlay = combat_calc::frame_overlay(
            sim,
            &self.selection.ids,
            hovered,
            &tops,
            view_proj,
            camera_right,
            size,
        );
        // Group digit (O-313) with the groups of the active slot; only `opennml-game`, the viewer draws none.
        if let Some(groups) = self.groups.get(usize::from(sim.active_player)) {
            let targets = combat_calc::overlay_targets(sim, &self.selection.ids, hovered);
            overlay.group_icons = combat_calc::group_icons(
                sim,
                &targets,
                groups,
                &tops,
                view_proj,
                camera_right,
                size,
            );
        }
        combat_calc::overlay_cmds(&overlay, ctx.assets, ctx.vfs, &mut out);
        // Screen texts (`FUN_00493B80` first part, `0x00438F2A`: before the messages, also in the cinemascope), then
        // the countdown (`FUN_00435EE0`, `0x00438F74`, after the weather): both in the world pass, so below status bar,
        // bars and info windows (O-367). The reimplementation draws the message heads only in
        // [`GameSession::hud_cmds`]; in the original they lie below the countdown (re/campaign-ui D6.4; the "PAUSE"
        // lettering is placed by [`GameSession::over_world_cmds`] after the messages before the game windows). In the
        // original the floating numbers are entries of the same list (`FUN_004926D0` branch `+0x54 & 1` ->
        // `FUN_004931A0`); the reimplementation keeps them in `Notices` and draws them right after the screen texts
        // (re/world-display W.5).
        let game = SimGameText::new(sim, ctx.texts);
        self.hud
            .screen_texts
            .draw(ctx, &GameTags::new(&game, 0), &mut out);
        self.map
            .notices
            .draw_floats(view_proj, self.hud.bar_ms, ctx, &mut out);
        let mission = &sim.state.script.mission;
        if mission.countdown_container != 0 {
            countdown_cmds(mission.countdown_s, ctx, &mut out);
        }
        // Part 2 of the same function (`0x00435FC0...0x004360B5`): every timer of the list with kind bit 1 as `m:ss` at
        // its position in the 1280 x 1024 grid, after the script countdown. In the reimplementation all entries have
        // kind 1 and position `{0, 0x28}` (top left): the leader timer of game types 2/3 and the time limit of game
        // type 4.
        for &ms in sim.state.countdowns.values() {
            timer_cmds(ms / 1000, TIMER_POS, ctx, &mut out);
        }
        out
    }

    /// Drawing path over the world (after [`GameSession::render`]) in one place, for window and recording of `opennml-game`
    /// (order re/campaign-ui D6.4): first the combat display with screen texts and countdown
    /// ([`GameSession::combat_cmds`], part of the world pass in the original), above it the game UI
    /// ([`GameSession::hud_cmds`]: messages, game windows with bar and cinemascope area, tooltip, text messages;
    /// re/game-ui 1.2), above it the scene fade ([`GameSession::fade_cmds`]) and last the info windows as first
    /// children of the desktop ([`GameSession::desktop_cmds`]); game menu, dialogs and pointer follow at the host. With
    /// a visible pause ([`PauseState::shown`]) the PAUSE lettering ([`opennml_ui::hud::pause_text::draw`], text 15788) lies
    /// after the messages and before the game windows, also in the cinemascope and with the game menu open, so below
    /// cinemascope area, scene fade and info windows. The viewer takes [`GameSession::hud_cmds`] (without PAUSE),
    /// [`GameSession::fade_cmds`] and [`GameSession::desktop_cmds`] and draws the combat display itself with egui;
    /// screen texts and countdown are missing there (a known viewer gap).
    ///
    /// With a running selection box ([`GameSession::selection_rect`]) it lies after PAUSE and before the game windows
    /// (root `0x00449890`, re/game-ui, section "Selection box addendum"); the viewer draws its own with egui.
    pub fn over_world_cmds(&self, ctx: &mut UiContext<'_>, texts: &Texts) -> Vec<DrawCmd> {
        let mut out = self.combat_cmds(ctx);
        let drag = self.selection_rect(ctx.screen);
        self.hud_cmds_into(ctx, texts, self.pause.shown, drag, &mut out);
        out.extend(self.fade_cmds(ctx.screen));
        out.extend(self.desktop_cmds(ctx, texts));
        out
    }

    /// Corners of the running selection box in UI pixels of a frame of size `screen`: the press point
    /// ([`GameSession::select_start`], in the original `+0x40/+0x44` from event 3, `0x00449410`) and the pointer
    /// position of this frame (`Cursor3D` `0x004CCB00`), both converted like the bar's input. The root draws from the
    /// press on without a minimum distance (`0x00449890` only checks `+0x48`); the 2 px only decide on release between
    /// click and box ([`GameSession::finish_selection`]). Pause and cinemascope do not stop it, the lock ends it
    /// (`{0x19, 1}` clears `+0x48`, [`GameSession::apply_presentation_event`]). None without a pointer.
    pub fn selection_rect(&self, screen: Screen) -> Option<([i32; 2], [i32; 2])> {
        let (start, end) = self.select_start.zip(self.pointer.cursor)?;
        let size = (
            u32::try_from(screen.width).unwrap_or(0),
            u32::try_from(screen.height).unwrap_or(0),
        );
        Some((
            surface_point(start, size, self.screen),
            surface_point(end, size, self.screen),
        ))
    }

    /// The info windows of a frame (re/campaign-ui D6.4 step 4, desktop `0x0045A420`): they hang at the head of the
    /// desktop's children, so they lie above world, game windows and scene fade and below game menu, dialogs and
    /// pointer (O-360), from head to end ([`InfoFrames::draw`]); portraits and frames from the data of the cinemascope
    /// area, game values of the counters via `texts`. Like [`GameSession::hud_cmds`] nothing without a simulation or
    /// with [`GameSession::hud_enabled`] off; otherwise in every frame, also in pause, game menu and cinemascope (the
    /// original draws the desktop without a check).
    ///
    /// Drawing measures a newly opened window (once, then fixed, O-358). Because the hosts draw here every frame with a
    /// match, the counter state of the frame in which the event came applies (exceptions in re/campaign-ui D6.5,
    /// section "Implementation in the host": minimized window, `step` with several steps, viewer without `--hud`).
    /// Otherwise read-only.
    pub fn desktop_cmds(&self, ctx: &mut UiContext<'_>, texts: &Texts) -> Vec<DrawCmd> {
        let mut out = Vec::new();
        let Some(sim) = self.sim().filter(|_| self.hud_enabled) else {
            return out;
        };
        let game = SimGameText::new(sim, texts);
        let tags = GameTags::new(&game, 0);
        self.info_frames
            .draw(&self.hud.cinemascope_data, ctx, &tags, &mut out);
        out
    }

    /// The scene fade of a frame of size `screen` (`FUN_004BC130`, re/campaign-ui D6.4 step 3): a black full screen
    /// above world and game windows, below the desktop. Like [`GameSession::hud_cmds`] nothing without a simulation or
    /// with [`GameSession::hud_enabled`] off. Read-only; ticked in [`GameSession::tick_presentation`].
    pub fn fade_cmds(&self, screen: Screen) -> Vec<DrawCmd> {
        let mut out = Vec::new();
        if self.hud_enabled && self.sim().is_some() {
            self.scene_fade.draw(screen, &mut out);
        }
        out
    }

    /// Clear color of the world (`GameApp_Tick` `0x004CA8EC...0x004CA946`, E-245, O-374): in the cinemascope, in every
    /// game type, the fog color with fog (`m_EnableFog`, here the effective switch [`MapView::fog_enabled`]; color as
    /// in the world fog, `Camera+0x84`), otherwise `BackColor` of the `Engine.ini` ([`ViewOptions::back_color`]);
    /// outside the cinemascope black, also with fog.
    pub fn clear_color(&self) -> [f32; 3] {
        if !self.presentation.cinemascope {
            return [0.0; 3];
        }
        if self.map.fog_enabled(self.view.fog) {
            return self.map.fog_color();
        }
        let c = self.view.back_color;
        [c >> 16, c >> 8, c].map(|b| (b & 0xFF) as f32 / 255.0)
    }

    /// Sets the target highlight from outside (viewer `sim_action`, [`crate::game::markers::control_action_marked`]): a
    /// new target replaces the scripts' marker 152 (the same field `GameWorld+0x318`, E-228).
    pub fn set_target_highlight(&mut self, highlight: Option<TargetHighlight>) {
        replace_script_highlight(
            self.target_highlight,
            highlight,
            &mut self.presentation.highlight,
        );
        self.target_highlight = highlight;
    }

    /// Game values of the tooltip (with selection and formation) and key layout for `[KEYID]`: the same inputs for
    /// drawing ([`GameSession::hud_cmds`]) and the report ([`GameSession::hud_state`]), so that the text of
    /// `ui_state.game.hud.toolbar.tooltip` is the drawn one.
    fn bar_texts<'a>(
        &'a self,
        sim: &'a SimHost,
        texts: &'a Texts,
    ) -> (SimGameText<'a>, opennml_ui::text::TextContext) {
        let game = SimGameText::new(sim, texts)
            .with_selection(&self.selection.ids)
            .with_formation(self.toolbar_host.bar.formation());
        (
            game,
            opennml_ui::screens::text_context(&self.key_map, texts, None),
        )
    }

    /// Report of the input for `ui_state.game.input`: pause bits, speed of the simulation, the control groups and the
    /// last group of the active slot, key context and the names of the effective locks ([`InputGate::names`]). What the
    /// host holds it passes in: the pointer it draws (`MenuHost::pointer_choice`, set from `GameFlow::pointer_choice`),
    /// its visibility and the setting `GameSpeed`. Read-only.
    pub fn input_data(
        &self,
        pointer: PointerChoice,
        pointer_visible: bool,
        config_speed: i32,
    ) -> UiInputData {
        let sim = self.sim();
        let groups = sim.and_then(|s| self.groups.get(usize::from(s.active_player)));
        let ctx = self.key_context();
        UiInputData {
            paused: self.pause.sim,
            shown_paused: self.pause.shown,
            game_speed: sim.map_or(0, |s| s.game_speed),
            config_speed,
            groups: groups.map_or_else(Vec::new, |g| {
                g.lists
                    .iter()
                    .map(|list| list.iter().map(|id| id.0).collect())
                    .collect()
            }),
            last_group: groups.map_or(-1, |g| g.last),
            pointer: pointer.id,
            pointer_variant: pointer.variant,
            pointer_visible,
            key_context: [ctx.kind, ctx.nation, ctx.selection],
            gate: self
                .input_gate()
                .names()
                .into_iter()
                .map(String::from)
                .collect(),
        }
    }

    /// Report of the game UI for `ui_state.game.hud` ([`Hud::state`]); `None` without a simulation. Independent of
    /// [`GameSession::hud_enabled`] (the bar also runs undrawn).
    pub fn hud_state(&self, texts: &Texts) -> Option<UiHudData> {
        let sim = self.sim()?;
        let values = Hud::status_values(sim, texts);
        let mut state = self.hud.state(&SimGameText::new(sim, texts), &values);
        // The command bar with the snapshots of drawing (`hud_cmds`).
        if let Some(toolbar) = &self.hud.toolbar {
            let (tip, keys) = self.bar_texts(sim, texts);
            state.toolbar = self.hud.toolbar_report(
                &portrait_model(sim, toolbar),
                &tip,
                &keys,
                texts,
                self.last_button,
            );
        }
        Some(state)
    }

    /// Report of the minimap for `ui_state.game.minimap`; `None` without a simulation or without a minimap (node
    /// `IsMap`, host data). Read-only.
    /// - `visible`: node and switch `+0x4D` ([`Hud::minimap_shown`]) and no cinemascope
    ///   ([`InputGate::minimap_visible`]), the condition of drawing without the UI switch;
    /// - `mode`, `center`, `theta`: map mode and position of the last frame ([`Hud::minimap_center`],
    ///   [`Minimap::last_layout`]; before the first frame the center from the image size and theta = 0);
    /// - `pings`: tile and remaining repeats in list order ([`MinimapPings::list`]);
    /// - `idle_workers`: length of the list of the active slot (`SimState::idle_workers`, buttons 576/577);
    /// - `fog_swaps`: number of buffer swaps of the fog image ([`MinimapFog::swaps`]).
    ///
    /// [`Minimap::last_layout`]: crate::game::minimap::Minimap::last_layout
    /// [`MinimapPings::list`]: crate::game::minimap::MinimapPings::list
    /// [`MinimapFog::swaps`]: crate::game::minimap::MinimapFog::swaps
    pub fn minimap_state(&self) -> Option<UiMinimapData> {
        let sim = self.sim()?;
        let map = self.hud.minimap.as_ref()?;
        let center = self.hud.minimap_center()?;
        Some(UiMinimapData {
            visible: self.hud.minimap_shown() && self.input_gate().minimap_visible(),
            mode: map.mode,
            center,
            theta: map.last_layout().map_or(0.0, |l| l.theta),
            pings: self
                .hud
                .pings
                .list()
                .iter()
                .map(|p| (p.tile.0, p.tile.1, p.repeats))
                .collect(),
            idle_workers: sim.state.idle_workers.list(sim.active_player).len(),
            fog_swaps: map.fog().swaps(),
        })
    }

    /// Rectangles `[x0, y0, x1, y1]` (right and bottom exclusive) in pixels of `screen` in which this frame's minimap
    /// leaves depth 0 (re/pointer-3d MZ.7, O-457): the rotated terrain quad in the state block `Map` (`FUN_004E3B70`
    /// `0x004E3EB6`), row by row ([`crate::game::minimap::draw::depth_rects`]). For `PointerScene::depth_zero` of the
    /// 3D pointer, which stays visible there only in the cleared square around the mouse. The same condition and
    /// position as drawing ([`Self::minimap_frame`]); if the UI does not draw the minimap (hidden, cinemascope, without
    /// `IsMap`, UI off), empty, even if its rectangle keeps taking clicks (O-291, O-368). Read-only.
    pub fn minimap_depth_rects(&self, screen: Screen) -> Vec<[i32; 4]> {
        use crate::game::minimap::draw;
        let Some(m) = self.hud.minimap.as_ref().filter(|_| self.minimap_drawn()) else {
            return Vec::new();
        };
        let size = draw::screen_size(screen);
        let terrain = m.terrain();
        let layout = draw::layout(size, &self.map.camera, (terrain.dim_x, terrain.dim_z));
        draw::depth_rects(&layout.corners(), size)
    }

    /// The active chat lines for `ui_state.game.chat_lines`: screen texts of kind 10 with own text in list order as
    /// "name: text"; waiting lines not yet ([`ScreenTexts::chat_lines`]).
    pub fn chat_lines(&self) -> Vec<String> {
        self.hud
            .screen_texts
            .chat_lines()
            .filter_map(|line| line.own_text.clone())
            .collect()
    }

    /// Center of a command-bar element for `ui_click` on `GAMEUI` in **window pixels** (the rectangles of the last
    /// frame are in UI pixels; the inverse of `surface_point`, to the pixel center so that rounding down hits the same
    /// pixel). Errors: [`Hud::element_center`].
    pub fn gameui_center(&self, element: &str) -> Result<(f64, f64), String> {
        let [x, y] = self.hud.element_center(element)?;
        let scale = |v: i32, screen: u32, size: u32| {
            if size == 0 {
                f64::from(v)
            } else {
                (f64::from(v) + 0.5) * f64::from(screen) / f64::from(size)
            }
        };
        Ok((
            scale(x, self.screen.0, self.size.0),
            scale(y, self.screen.1, self.size.1),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Clock;
    use crate::game::script_camera::ScriptCamera;
    use crate::game::sim_host::plain_testkit;
    use opennml_sim::command::CommandKind;
    use opennml_ui::draw::Blend;

    /// Right-click: with the game camera the viewer discarded a right-click with 3 pixels of mouse travel as rotating;
    /// the original always triggers the action chosen on press when releasing (`0x00453790`, `0x00457C50`).
    #[test]
    fn a_right_click_with_the_game_camera_survives_mouse_jitter() {
        let start = (800.0, 450.0);
        // Game camera: every mouse travel is a click at the press point.
        assert_eq!(right_click_point(start, (803.0, 450.0), false), Some(start));
        assert_eq!(right_click_point(start, (900.0, 520.0), false), Some(start));
        // Free camera: from 3 pixels on the right button rotates the camera.
        assert_eq!(right_click_point(start, (802.0, 448.0), true), Some(start));
        assert_eq!(right_click_point(start, (803.0, 450.0), true), None);
        assert_eq!(right_click_point(start, (800.0, 447.0), true), None);
    }

    #[test]
    fn horizontal_wheel_scrolls_are_ignored() {
        use winit::dpi::PhysicalPosition;
        assert_eq!(wheel_up(MouseScrollDelta::LineDelta(0.0, 1.0)), Some(true));
        assert_eq!(
            wheel_up(MouseScrollDelta::LineDelta(0.0, -1.0)),
            Some(false)
        );
        assert_eq!(wheel_up(MouseScrollDelta::LineDelta(2.0, 0.0)), None);
        assert_eq!(
            wheel_up(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                5.0, 0.0
            ))),
            None
        );
        assert_eq!(
            wheel_up(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                0.0, -3.0
            ))),
            Some(false)
        );
    }

    /// The player camera stands still under the lock and during a move, rotation or open cut of the scripts; with a
    /// binding alone the keys move the eye (the target then follows the object).
    #[test]
    fn a_binding_alone_leaves_the_player_camera_free() {
        use crate::game::script_camera::MoveLeg;
        use opennml_sim::script::present::Presentation;
        let host = plain_testkit::host();
        let id = host.state.objects().next().unwrap().id;
        let mut pointer = Pointer::default();
        pointer.keys.insert(KeyCode::ArrowUp);
        let screen = (320, 200);
        // A frame like `advance_by`: player camera, then the script camera.
        let gate = |script: &ScriptCamera, ui_locked: bool| InputGate {
            ui_locked,
            script_camera: script.running(),
            ..InputGate::default()
        };
        let frame = |script: &mut ScriptCamera, ui_locked: bool| {
            let (mut camera, terrain) = crate::game::input::testkit::setup();
            let mut controller = CameraController::default();
            for _ in 0..10 {
                let input = frame_input(
                    &pointer,
                    &[],
                    screen,
                    !gate(script, ui_locked).player_camera(),
                    false,
                    &CameraKeys::Viewer,
                    &Hotkeys::default(),
                );
                advance_camera(
                    &mut controller,
                    &mut camera,
                    None,
                    &terrain,
                    input,
                    &Controls::default(),
                    50.0,
                );
                script.update(50.0, &mut camera, &terrain, &host);
            }
            camera
        };
        let (start, terrain) = crate::game::input::testkit::setup();
        // Bound: the arrow key moves the eye, the target stays at the object.
        let mut bound = ScriptCamera::default();
        bound.apply(
            &Presentation::CameraBind { id: Some(id) },
            &mut start.clone(),
            &[],
            &terrain,
            &host,
        );
        assert!(!bound.running(), "a binding alone does not lock");
        assert!(gate(&bound, false).player_camera());
        let mut still = bound.clone();
        let moved = frame(&mut bound, false);
        let object = host.interpolated(host.state.object(id, false).unwrap());
        assert_eq!(moved.target, GameCamera::clamp_target(object, &terrain));
        assert!(moved.eye != start.eye, "scrolling moves the eye");
        // Counter-check: under the lock the eye stays put.
        let locked = frame(&mut still, true);
        assert_eq!(locked.eye, start.eye);
        // A running move locks the player camera.
        let mut moving = ScriptCamera::default();
        moving.moves.push_back(MoveLeg {
            from: start.eye.to_array(),
            at: start.target.to_array(),
            ms: 100_000,
            player: false,
        });
        assert!(moving.running(), "a move locks the player camera");
        assert!(!gate(&moving, false).player_camera());
        let held = frame(&mut moving, false);
        assert!(
            (held.eye - start.eye).length() < 0.01,
            "no player movement during the move: {} against {}",
            held.eye,
            start.eye
        );
        // Without script camera and without lock the camera moves.
        let mut idle = ScriptCamera::default();
        assert!(frame(&mut idle, false).target != start.target);
    }

    /// The camera lines of the installation's `keymap.ini` (the same names and keys; re/game-ui D5.3), readable without
    /// an installation. The corpus test [`installed_keymap_matches_the_fixture`] keeps them equal.
    fn camera_keymap() -> Hotkeys {
        use opennml_ui::keys::parse_chord;
        let lines = [
            ("KD_SPEEDUP_GAME", "TAB"),
            ("KD_CAM_MOVE_FORWARD", "UP"),
            ("KD_CAM_MOVE_BACKWARD", "DOWN"),
            ("KD_CAM_MOVE_LEFT", "LEFT"),
            ("KD_CAM_MOVE_RIGHT", "RIGHT"),
            ("KD_CAM_YROT_CLOCKWISE", "END"),
            ("KD_CAM_YROT_ANTICLOCKWISE", "DELETE"),
            ("KD_CAM_ZOOM_IN", "NEXT"),
            ("KD_CAM_ZOOM_OUT", "PRIOR"),
            ("KD_CAM_FREEHAND", "MBUTTON"),
            ("KD_CHECK_LBUTTON", "LBUTTON"),
        ];
        Hotkeys {
            keydefs: lines
                .iter()
                .map(|(name, chord)| (name.to_string(), parse_chord(chord)))
                .collect(),
            ..Hotkeys::default()
        }
    }

    /// Camera input of the table `held` (VK codes) per the `KD_` keys.
    fn original_input(held: &[u8], lock_camera: bool, hotkeys: &Hotkeys) -> CameraInput {
        camera_input(
            &HashSet::new(),
            held,
            Vec2::new(1.0, 0.0),
            &CameraKeys::Original { lock_camera },
            hotkeys,
        )
    }

    /// With the chat window open (input field with focus) the camera in the original does not scroll, zoom or rotate
    /// although the arrow keys are held; the screen edge keeps working, and after closing the same key scrolls again.
    // QUIRK O-393: kept
    #[test]
    fn camera_keys_are_dead_while_a_text_field_has_focus() {
        let hotkeys = camera_keymap();
        let pointer = Pointer {
            cursor: Some((0.0, 100.0)),
            ..Pointer::default()
        };
        // LEFT, PRIOR, END held: scrolling, zoom, rotating (LockCamera 0).
        let held = [0x25, 0x21, 0x23];
        let mode = CameraKeys::Original { lock_camera: false };
        let frame = |typing: bool| {
            frame_input(&pointer, &held, (1024, 768), false, typing, &mode, &hotkeys).camera
        };
        let typing = frame(true);
        assert_eq!(typing.keys, Vec2::ZERO, "no scrolling");
        assert_eq!((typing.zoom, typing.pitch, typing.yaw), (0, 0, 0));
        assert!(!typing.halt);
        assert!(typing.edge != Vec2::ZERO, "the edge keeps working");
        let free = frame(false);
        assert_eq!(free.keys, Vec2::new(0.0, -1.0), "after closing");
        assert_eq!(free.zoom, 1);
        assert_eq!(free.yaw, -1);
        assert_eq!(typing.edge, free.edge);
    }

    /// The installation has the same `KD_` lines for the camera as [`camera_keymap`] (corpus).
    // QUIRK O-305: kept
    #[test]
    fn installed_keymap_matches_the_fixture() {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let installed = Hotkeys::load(&Vfs::open_game_dir(&dir).unwrap());
        for (name, chord) in &camera_keymap().keydefs {
            assert_eq!(installed.keydef(name), chord.as_slice(), "{name}");
        }
        for dead in ["KD_CAM_XROT_UP", "KD_CAM_XROT_DOWN"] {
            assert!(installed.keydef(dead).is_empty(), "{dead}");
        }
    }

    /// O-306 (`0x0043CDAD`...`0x0043CE29`): End/Delete rotate only with `LockCamera 0`: End (`KD_CAM_YROT_CLOCKWISE`)
    /// with `Camera_Yaw(-dt * 0.001)`, Delete with `+`; with `LockCamera 1` (installation) nothing.
    // QUIRK O-306: kept
    #[test]
    fn end_and_delete_rotate_only_without_lock_camera() {
        const END: u8 = 0x23;
        const DELETE: u8 = 0x2E;
        let hotkeys = camera_keymap();
        assert_eq!(original_input(&[END], true, &hotkeys).yaw, 0);
        assert_eq!(original_input(&[DELETE], true, &hotkeys).yaw, 0);
        assert_eq!(original_input(&[END], false, &hotkeys).yaw, -1);
        assert_eq!(original_input(&[DELETE], false, &hotkeys).yaw, 1);
        assert_eq!(original_input(&[END, DELETE], false, &hotkeys).yaw, 0);
        // Via the control: with `LockCamera 1` the camera stands still, with 0 End turns the eye around the target.
        let turn = |lock_camera: bool| {
            let (mut camera, terrain) = crate::game::input::testkit::setup();
            let controls = Controls {
                lock_camera,
                ..Controls::default()
            };
            let mut controller = CameraController::default();
            let input = camera_input(
                &HashSet::new(),
                &[END],
                Vec2::ZERO,
                &CameraKeys::Original { lock_camera },
                &hotkeys,
            );
            let before = camera.clone();
            for _ in 0..10 {
                controller.update(&mut camera, &terrain, input, &controls, 16.0);
            }
            (before, camera)
        };
        let (before, locked) = turn(true);
        assert_eq!((locked.eye, locked.target), (before.eye, before.target));
        let (before, turned) = turn(false);
        assert_eq!(turned.target, before.target, "rotating around the target");
        assert!((turned.eye - before.eye).length() > 1.0, "End rotates");
        assert!((turned.distance() - before.distance()).abs() < 1.0);
    }

    /// O-305: `keymap.ini` has no `KD_CAM_XROT_*`, so neither Home nor End nor any other key tilts in `opennml-game`;
    /// arrows scroll, Page Down (`KD_CAM_ZOOM_IN` = `NEXT`) zooms closer, Page Up farther.
    // QUIRK O-305: kept
    #[test]
    fn no_tilt_keys_in_nml_game() {
        let hotkeys = camera_keymap();
        for lock_camera in [true, false] {
            for vk in 0..=255u8 {
                assert_eq!(
                    original_input(&[vk], lock_camera, &hotkeys).pitch,
                    0,
                    "VK {vk:#04X}"
                );
            }
        }
        let input = original_input(&[0x24], true, &hotkeys);
        assert_eq!(
            input,
            CameraInput {
                edge: Vec2::new(1.0, 0.0),
                ..CameraInput::default()
            },
            "Home does not move the camera (it is /setcamera)"
        );
        assert_eq!(original_input(&[0x23], true, &hotkeys), input, "End");
        // Arrows, Page Down/Up (`0x0043CEB2`/`0x0043CEEE`).
        let input = original_input(&[0x26, 0x25], true, &hotkeys);
        assert_eq!(input.keys, Vec2::new(1.0, -1.0));
        assert_eq!(
            original_input(&[0x28, 0x27], true, &hotkeys).keys,
            Vec2::new(-1.0, 1.0)
        );
        assert_eq!(original_input(&[0x22], true, &hotkeys).zoom, -1, "closer");
        assert_eq!(original_input(&[0x21], true, &hotkeys).zoom, 1, "farther");
        // The viewer's physical keys (`pointer.keys`) do not count in the original.
        let keys: HashSet<KeyCode> = [KeyCode::Home, KeyCode::ArrowUp].into();
        let input = camera_input(
            &keys,
            &[],
            Vec2::ZERO,
            &CameraKeys::Original { lock_camera: false },
            &hotkeys,
        );
        assert_eq!(input, CameraInput::default());
    }

    /// O-305: `KD_SPEEDUP_GAME` (TAB) is in `keymap.ini`, the code does not query it: TAB moves nothing.
    // QUIRK O-305: kept
    #[test]
    fn tab_does_nothing() {
        let hotkeys = camera_keymap();
        assert_eq!(hotkeys.keydef("KD_SPEEDUP_GAME"), [0x09]);
        for lock_camera in [true, false] {
            let input = camera_input(
                &[KeyCode::Tab].into(),
                &[0x09],
                Vec2::ZERO,
                &CameraKeys::Original { lock_camera },
                &hotkeys,
            );
            assert_eq!(input, CameraInput::default());
        }
    }

    /// `KD_CHECK_LBUTTON` (`0x0043C7EA`): a held left mouse button halts keys, edge, rotating and zoom; the
    /// acceleration stays put (no gliding out) and continues after release.
    // QUIRK O-329: kept
    #[test]
    fn held_left_button_stops_the_camera() {
        const LBUTTON: u8 = 0x01;
        let hotkeys = camera_keymap();
        let all = [0x26, 0x25, 0x23, 0x22, LBUTTON];
        let input = original_input(&all, false, &hotkeys);
        assert_eq!(
            input,
            CameraInput {
                halt: true,
                ..CameraInput::default()
            },
            "nothing, not even the edge"
        );
        // The mouse button anywhere in the table, also pressed first.
        assert!(original_input(&[LBUTTON, 0x26], true, &hotkeys).halt);
        assert!(!original_input(&[0x26], true, &hotkeys).halt);
        // The viewer does not know the halt (it queries no `KD_`).
        let viewer = camera_input(
            &[KeyCode::ArrowUp].into(),
            &[LBUTTON],
            Vec2::ZERO,
            &CameraKeys::Viewer,
            &hotkeys,
        );
        assert!(!viewer.halt && viewer.keys.x == 1.0);
        // Control: arrow up accelerates, the left button halts at once, after release (without arrow) the camera glides
        // out with the acceleration that stood still.
        let (mut camera, terrain) = crate::game::input::testkit::setup();
        let controls = Controls::default();
        let mut controller = CameraController::default();
        let step = |controller: &mut CameraController, camera: &mut GameCamera, held: &[u8]| {
            let input = original_input(held, true, &hotkeys);
            let input = CameraInput {
                edge: Vec2::ZERO,
                ..input
            };
            let before = camera.target;
            controller.update(camera, &terrain, input, &controls, 16.0);
            (camera.target - before).length()
        };
        for _ in 0..30 {
            step(&mut controller, &mut camera, &[0x26]);
        }
        let full = step(&mut controller, &mut camera, &[0x26]);
        assert!(full > 0.0);
        for _ in 0..30 {
            assert_eq!(step(&mut controller, &mut camera, &[0x26, LBUTTON]), 0.0);
        }
        let glide = step(&mut controller, &mut camera, &[]);
        assert!(
            glide > full * 0.9,
            "the acceleration stood still: {glide} against {full}"
        );
    }

    /// O-331: `KD_CAM_FREEHAND` (`MBUTTON`, VK 4) held -> [`CameraInput::freehand`] ([`freehand_step`] sets the
    /// movement), also with `LockCamera 1` and with the chat open; not with the left button held, not under the lock,
    /// never in the viewer.
    // QUIRK O-331: kept
    #[test]
    fn middle_button_marks_the_freehand_camera() {
        const MBUTTON: u8 = 0x04;
        let hotkeys = camera_keymap();
        for lock_camera in [true, false] {
            assert_eq!(
                original_input(&[MBUTTON], lock_camera, &hotkeys).freehand,
                Some(Vec2::ZERO)
            );
            assert_eq!(original_input(&[], lock_camera, &hotkeys).freehand, None);
        }
        assert_eq!(
            original_input(&[0x01, MBUTTON], false, &hotkeys).freehand,
            None
        );
        let pointer = Pointer {
            cursor: Some((500.0, 400.0)),
            ..Pointer::default()
        };
        let mode = CameraKeys::Original { lock_camera: false };
        let frame = |locked: bool, typing: bool| {
            frame_input(
                &pointer,
                &[MBUTTON],
                (1024, 768),
                locked,
                typing,
                &mode,
                &hotkeys,
            )
            .camera
        };
        assert_eq!(frame(false, true).freehand, Some(Vec2::ZERO), "chat open");
        assert_eq!(frame(true, false).freehand, None, "lock");
        let viewer = camera_input(
            &HashSet::new(),
            &[MBUTTON],
            Vec2::ZERO,
            &CameraKeys::Viewer,
            &hotkeys,
        );
        assert_eq!(viewer.freehand, None);
    }

    /// O-331, O-434 (`0x0043CF1D`...`0x0043D095`): with the middle button held the movement is position minus position
    /// of the previous frame in whole pixels; the pointer jumps back to this position (`SetCursorPos`), which stays the
    /// reference. Without the key the reference follows; if the camera block does not run (lock, left button), it stays
    /// put, and the first frame afterwards brings the whole movement at once. Nothing without a pointer position.
    // QUIRK O-331, O-434: kept
    #[test]
    fn freehand_step_measures_against_the_previous_frame_and_holds_the_pointer() {
        let held = || CameraInput {
            freehand: Some(Vec2::ZERO),
            ..CameraInput::default()
        };
        let mut anchor = None;
        // Without the key: the reference follows the pointer.
        let mut cursor = Some((300.4, 200.0));
        let mut input = CameraInput::default();
        assert_eq!(
            freehand_step(&mut input, &mut cursor, &mut anchor, true),
            None
        );
        assert_eq!((input.freehand, anchor), (None, Some((300.4, 200.0))));
        // Pressed: movement against the reference, whole pixels, pointer back.
        cursor = Some((310.9, 195.0));
        let mut input = held();
        let warp = freehand_step(&mut input, &mut cursor, &mut anchor, true);
        assert_eq!(input.freehand, Some(Vec2::new(10.0, -5.0)));
        assert_eq!(
            (warp, cursor, anchor),
            (
                Some((300.4, 200.0)),
                Some((300.4, 200.0)),
                Some((300.4, 200.0))
            )
        );
        // Block does not run (left button, lock): no movement, reference stays, the pointer stays where it is.
        cursor = Some((400.0, 260.0));
        let mut input = held();
        assert_eq!(
            freehand_step(&mut input, &mut cursor, &mut anchor, false),
            None
        );
        assert_eq!(
            (input.freehand, cursor, anchor),
            (None, Some((400.0, 260.0)), Some((300.4, 200.0)))
        );
        // Afterwards with the middle button held: the whole movement since the lock at once (O-434).
        let mut input = held();
        freehand_step(&mut input, &mut cursor, &mut anchor, true);
        assert_eq!(input.freehand, Some(Vec2::new(100.0, 60.0)));
        // Pointer outside the window: nothing, reference stays.
        let mut outside = None;
        let mut input = held();
        assert_eq!(
            freehand_step(&mut input, &mut outside, &mut anchor, true),
            None
        );
        assert_eq!((input.freehand, anchor), (None, Some((300.4, 200.0))));
        // First frame without reference: movement 0.
        let (mut anchor, mut cursor) = (None, Some((5.0, 6.0)));
        let mut input = held();
        assert_eq!(
            freehand_step(&mut input, &mut cursor, &mut anchor, true),
            Some((5.0, 6.0))
        );
        assert_eq!(input.freehand, Some(Vec2::ZERO));
    }

    /// O-331 in the session: with `LockCamera 0` the held middle button rotates the camera around the target, the
    /// pointer jumps back and `opennml-game` gets the position for `SetCursorPos`; with `LockCamera 1` it moves. With the
    /// left button held everything stands still, afterwards the movement since the press acts at once (O-434).
    // QUIRK O-331, O-434: kept
    #[test]
    fn middle_button_turns_or_moves_the_camera_in_the_session() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.resize(1024, 768);
        session.hotkeys = camera_keymap();
        session.camera_keys = CameraKeys::Original { lock_camera: false };
        session.controls.lock_camera = false;
        session.held_vk = vec![0x04];
        session.pointer.cursor = Some((500.0, 400.0));
        let mut now = 1_000;
        let mut frame = |session: &mut GameSession| {
            now += 16;
            session.advance(&gpu, &vfs, now, true, None);
        };
        frame(&mut session);
        assert_eq!(session.take_cursor_warp(), Some((500.0, 400.0)));
        assert_eq!(session.take_cursor_warp(), None, "once per frame");
        let before = session.map.camera.clone();
        session.pointer.cursor = Some((520.0, 400.0));
        frame(&mut session);
        let camera = &session.map.camera;
        assert_eq!(camera.target, before.target, "rotating around the target");
        assert!((camera.eye - before.eye).length() > 100.0);
        assert!((camera.distance() - before.distance()).abs() < 1.0);
        assert_eq!(
            session.pointer.cursor,
            Some((500.0, 400.0)),
            "the pointer stays"
        );
        assert_eq!(session.take_cursor_warp(), Some((500.0, 400.0)));
        // LockCamera 1: mouse up moves forward, distance and direction stay.
        session.camera_keys = CameraKeys::Original { lock_camera: true };
        session.controls.lock_camera = true;
        let before = session.map.camera.clone();
        session.pointer.cursor = Some((500.0, 380.0));
        frame(&mut session);
        let camera = session.map.camera.clone();
        assert!((camera.target - before.target).length() > 100.0);
        assert!((camera.eye - camera.target - (before.eye - before.target)).length() < 1e-1);
        // Left button added: nothing, reference stays; afterwards the movement since the press at once.
        session.held_vk = vec![0x01, 0x04];
        session.pointer.cursor = Some((500.0, 300.0));
        frame(&mut session);
        assert_eq!(session.map.camera, camera);
        assert_eq!(session.take_cursor_warp(), None);
        session.held_vk = vec![0x04];
        frame(&mut session);
        let jump = (session.map.camera.target - camera.target).length();
        assert!(
            jump > 4.0 * (camera.target - before.target).length(),
            "{jump}"
        );
        assert_eq!(session.pointer.cursor, Some((500.0, 400.0)));
    }

    /// The viewer keeps its keys (Home/End tilt, Insert/Delete rotate, Page Down farther); `CameraKeys::Viewer` is the
    /// session's default.
    // QUIRK O-306: kept
    #[test]
    fn viewer_camera_keys_stay() {
        assert_eq!(CameraKeys::default(), CameraKeys::Viewer);
        let hotkeys = camera_keymap();
        let input = |keys: &[KeyCode]| {
            camera_input(
                &keys.iter().copied().collect(),
                &[0x01, 0x23],
                Vec2::ZERO,
                &CameraKeys::Viewer,
                &hotkeys,
            )
        };
        assert_eq!(input(&[KeyCode::Home]).pitch, 1);
        assert_eq!(input(&[KeyCode::End]).pitch, -1);
        assert_eq!(input(&[KeyCode::Insert]).yaw, 1);
        assert_eq!(input(&[KeyCode::Delete]).yaw, -1);
        assert_eq!(input(&[KeyCode::PageDown]).zoom, 1);
        assert_eq!(input(&[KeyCode::PageUp]).zoom, -1);
        assert_eq!(input(&[KeyCode::ArrowUp]).keys, Vec2::new(1.0, 0.0));
        assert_eq!(input(&[]), CameraInput::default(), "table ignored");
        // A session begins with the viewer's keys; only `opennml-game` switches (`LiveSession::apply_settings`).
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        assert_eq!(session.camera_keys, CameraKeys::Viewer);
        assert_eq!(session.hotkeys.keydef("KD_CHECK_LBUTTON"), [0x01]);
    }

    /// Home (`FUN_00420230` -> `FUN_00433FC0(250)` -> `FUN_00433890`) moves the camera back to the home position
    /// (`CAMERA_OFFSET`) in 250 game ms, linear in the offset; the target stays, the first frame is already under way
    /// but not at the destination. The move queues behind a script move, does not lock the player camera, stands still
    /// in pause, and the mouse wheel does not wait (it is discarded).
    // QUIRK O-306, O-329: kept
    // QUIRK O-330: deviation
    #[test]
    fn home_moves_the_camera_back_softly() {
        use crate::game::script_camera::MoveLeg;
        let host = plain_testkit::host();
        let (mut camera, terrain) = crate::game::input::testkit::setup();
        let home_offset = camera.eye - camera.target;
        // Rotated, closer, scrolled.
        assert!(camera.yaw(0.4));
        assert!(camera.zoom_step(true) && camera.zoom_step(true));
        camera.scroll(30.0, -20.0, &terrain);
        let target = camera.target;
        let moved = camera.eye;
        assert!((moved - target - home_offset).length() > 100.0);
        let mut script = ScriptCamera::default();
        script.push_home(&camera);
        assert!(!script.running(), "Home does not lock the player camera");
        assert!(script.moving(), "but the wheel waits");
        script.update(16.0, &mut camera, &terrain, &host);
        assert_eq!(camera.target, target, "target stays");
        let home = target + home_offset;
        assert!(camera.eye != moved && (camera.eye - home).length() > 1.0);
        // Linear in the offset: after 16 of 250 ms exactly 16/250 of the way.
        let expected = moved + (home - moved) * (16.0 / 250.0);
        assert!(
            (camera.eye - expected).length() < 0.5,
            "{} against {expected}",
            camera.eye
        );
        assert!(!script.running());
        for _ in 0..14 {
            script.update(16.0, &mut camera, &terrain, &host);
        }
        assert!(script.moving(), "240 ms: still under way");
        script.update(16.0, &mut camera, &terrain, &host);
        assert!(!script.moving());
        assert_eq!(camera.target, target);
        assert!(
            (camera.eye - home).length() < 0.5,
            "{} against {home}",
            camera.eye
        );
        // Behind a script move: first it (locks), then Home (does not lock).
        let mut script = ScriptCamera::default();
        let (mut camera, _) = crate::game::input::testkit::setup();
        script.moves.push_back(MoveLeg {
            from: (camera.eye + Vec3::new(500.0, 0.0, 0.0)).to_array(),
            at: camera.target.to_array(),
            ms: 100,
            player: false,
        });
        // Home remembers target and eye on pressing (`FUN_00433FC0`), not when taken from the list.
        let pressed = (camera.target, camera.target + camera.setup.offset);
        script.push_home(&camera);
        assert!(script.running(), "the script move locks");
        for _ in 0..7 {
            script.update(16.0, &mut camera, &terrain, &host);
        }
        assert!(!script.running() && script.moving(), "now Home runs");
        assert!(
            (camera.eye - pressed.1).length() > 100.0,
            "the script move ran"
        );
        for _ in 0..20 {
            script.update(16.0, &mut camera, &terrain, &host);
        }
        assert!(
            (camera.target - pressed.0).length() < 0.5,
            "{}",
            camera.target
        );
        assert!((camera.eye - pressed.1).length() < 0.5, "{}", camera.eye);

        // Whole session: binding `HOME` -> `/setcamera` via `run_hotkey`, frames of 16 ms.
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.camera_keys = CameraKeys::Original { lock_camera: true };
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let offset = session.map.camera.setup.offset;
        assert!(session.map.camera.yaw(0.5));
        assert!(session.map.camera.zoom_step(false));
        let (eye, target) = (session.map.camera.eye, session.map.camera.target);
        assert!(session.run_hotkey(&HotkeyCommand::SetCamera, 1_000, &vfs, None));
        assert!(session.input_gate().player_camera());
        // In pause (no frame) the move stands still.
        session.advance_by(&gpu, &vfs, 16.0, false, None);
        assert_eq!(session.map.camera.eye, eye);
        // The wheel is discarded during the move.
        let screen = session.size;
        session.input_with(
            &InputEvent::Wheel { up: true },
            screen,
            1_000.0,
            &vfs,
            None,
            &mut NoModes,
        );
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let first = session.map.camera.eye;
        assert_eq!(session.map.camera.target, target);
        assert!(first != eye && (first - (target + offset)).length() > 1.0);
        for _ in 0..30 {
            session.advance_by(&gpu, &vfs, 16.0, true, None);
        }
        assert_eq!(session.map.camera.target, target, "target stays");
        let home = target + offset;
        assert!(
            (session.map.camera.eye - home).length() < 1.0,
            "{} against {home}",
            session.map.camera.eye
        );
        // Held left button: the wheel does not act afterwards either; without it the zoom works again.
        session.held_vk = vec![0x01];
        assert!(session.left_button_holds_camera());
        session.input_with(
            &InputEvent::Wheel { up: true },
            screen,
            2_000.0,
            &vfs,
            None,
            &mut NoModes,
        );
        for _ in 0..30 {
            session.advance_by(&gpu, &vfs, 16.0, true, None);
        }
        assert!((session.map.camera.eye - home).length() < 1.0, "no wheel");
        session.held_vk.clear();
        session.input_with(
            &InputEvent::Wheel { up: true },
            screen,
            3_000.0,
            &vfs,
            None,
            &mut NoModes,
        );
        for _ in 0..30 {
            session.advance_by(&gpu, &vfs, 16.0, true, None);
        }
        assert!(
            session.map.camera.distance() < offset.length() - 100.0,
            "wheel zooms"
        );
    }

    #[test]
    fn keys_map_to_camera_input() {
        let keys: HashSet<KeyCode> = [
            KeyCode::ArrowUp,
            KeyCode::ArrowLeft,
            KeyCode::PageUp,
            KeyCode::Home,
        ]
        .into();
        let input = camera_input(
            &keys,
            &[],
            Vec2::new(0.0, 1.0),
            &CameraKeys::Viewer,
            &Hotkeys::default(),
        );
        assert_eq!(input.keys, Vec2::new(1.0, -1.0));
        assert_eq!((input.zoom, input.pitch, input.yaw), (-1, 1, 0));
        assert_eq!(input.edge, Vec2::new(0.0, 1.0));
        let free = free_input(
            &[KeyCode::KeyW, KeyCode::KeyQ, KeyCode::ShiftLeft].into(),
            Vec2::ONE,
        );
        assert_eq!(free.movement, Vec3::new(1.0, 0.0, -1.0));
        assert!(free.fast);
    }

    /// Camera and clock after steps of 16 ms in the groups `chunks` (held: arrow up and Home; with `free` the free
    /// camera with W and look).
    fn run_steps(chunks: &[u32], free: bool) -> (Vec3, Vec3, Option<FreeCamera>, f64) {
        let (mut camera, terrain) = crate::game::input::testkit::setup();
        let mut free_camera = free.then(|| FreeCamera::from_game(&camera));
        let keys: HashSet<KeyCode> = [KeyCode::ArrowUp, KeyCode::Home, KeyCode::KeyW].into();
        let input = FrameInput {
            camera: camera_input(
                &keys,
                &[],
                Vec2::ZERO,
                &CameraKeys::Viewer,
                &Hotkeys::default(),
            ),
            free: free_input(&keys, Vec2::new(3.0, 0.0)),
        };
        let mut controller = CameraController::default();
        let mut clock = Clock::new();
        clock.pause();
        for &frames in chunks {
            for _ in 0..frames {
                clock.advance(16.0);
                advance_camera(
                    &mut controller,
                    &mut camera,
                    free_camera.as_mut(),
                    &terrain,
                    input,
                    &Controls::default(),
                    16.0,
                );
            }
        }
        (camera.eye, camera.target, free_camera, clock.now_ms())
    }

    #[test]
    fn same_steps_give_the_same_camera_however_they_are_split() {
        let whole = run_steps(&[30], false);
        assert_eq!(whole, run_steps(&[10, 20], false));
        assert_eq!(whole, run_steps(&[1, 28, 1], false));
        assert_eq!(whole.3, 480.0);
        let still = run_steps(&[], false);
        assert_ne!(whole.1, still.1, "target moved");
        assert_ne!(whole.0 - whole.1, still.0 - still.1, "tilt changed");
        let free = run_steps(&[30], true);
        assert_eq!(free, run_steps(&[10, 20], true));
        assert_eq!(free.1, still.1, "free camera leaves the game camera alone");
        assert_ne!(free.2, run_steps(&[], true).2);
    }

    /// Stopped match (game menu open): 5 s without a step and without catching up; afterwards 50 ms give exactly one
    /// tick (speed 10: game time = real time, 50 ms per tick).
    #[test]
    fn stopped_session_does_not_step_or_catch_up() {
        let mut sim = plain_testkit::host();
        sim.game_speed = 10;
        let mut clock = SessionClock::default();
        let start = 1_000;
        assert_eq!(clock.advance(&mut sim, start, true), (0, Vec::new()));
        let steps = sim.state.steps;
        for now in (start..=start + 5_000).step_by(16) {
            assert_eq!(clock.advance(&mut sim, now, false).0, 0);
        }
        assert_eq!(clock.advance(&mut sim, start + 5_000, false).0, 0);
        assert_eq!(sim.state.steps, steps, "no step while halted");
        assert_eq!(clock.advance(&mut sim, start + 5_050, true).0, 1);
        assert_eq!(
            sim.state.steps,
            steps + 1,
            "exactly one step, no catching up"
        );
    }

    /// The message bar runs only with the match (O-213): it appears, stands as long as you like with the game menu open
    /// (the program clock keeps running, `frame_ms` yields nothing, so no tick) and expires in game time when play
    /// resumes.
    // QUIRK O-213: kept
    #[test]
    fn bar_stands_while_the_session_is_stopped() {
        use crate::game::hud::{Hud, HudOptions};
        let mut sim = plain_testkit::host();
        sim.active_player = 0;
        sim.game_speed = 10;
        let messages = opennml_formats::messages::parse_messages(
            "MSGTAG ECONOMY
COLOR E000FF00
/MSGTAG
MSGID 0
MSGTYPE ECONOMY
TEXT 17000
/MSGID
",
        );
        // 30 characters: 1650 ms display + 1500 ms fade-out (speed 10 = 1:1).
        let texts = [(17000, "WIR HABEN NICHT GENUG NAHRUNG!".to_string())].into();
        let mut hud = Hud::new(
            &messages,
            &texts,
            &HudOptions::default(),
            opennml_ui::coords::Screen {
                width: 1024,
                height: 768,
            },
        );
        let mut clock = SessionClock::default();
        // A frame like `GameSession::advance`: without `running` neither events nor tick.
        let frame = |hud: &mut Hud, clock: &mut SessionClock, now: u64, running: bool| {
            if let Some(dt) = clock.frame_ms(now, running) {
                hud.tick(dt, sim.game_speed, &mut Vec::new());
            }
        };
        let start = 1_000_000;
        frame(&mut hud, &mut clock, start, true);
        let message = SimEvent::PlayerMessage {
            to: Some(0),
            id: 0,
            tile: (0, 0),
            flags: 0x0301_0000,
        };
        hud.handle(&[message], 0, &mut Vec::new());
        frame(&mut hud, &mut clock, start + 1_000, true);
        let shown = hud.messages.queue(0)[0].clone();
        assert_eq!(shown.display_ms, 650);
        // Game menu open: 60 s of program clock, the message stands.
        for now in (start + 2_000..=start + 61_000).step_by(1_000) {
            frame(&mut hud, &mut clock, now, false);
        }
        assert_eq!(hud.messages.queue(0)[0], shown);
        // Resuming: 650 ms display, then 1500 ms fade-out; just before that it still stands.
        frame(&mut hud, &mut clock, start + 61_650, true);
        frame(&mut hud, &mut clock, start + 63_149, true);
        assert_eq!(hud.messages.queue(0)[0].fade_ms, 1);
        frame(&mut hud, &mut clock, start + 63_150, true);
        assert!(hud.messages.queue(0).is_empty());
    }

    /// Loading and restart clear message bar and action stack (not in the savegame, re/game-ui N.7.4).
    #[test]
    fn loading_clears_the_bar() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        let fill = |session: &mut GameSession| {
            let active = session.sim().unwrap().active_player;
            // Message 4 (field empty) carries `PING`: with a location it goes on the action stack; behind it a queued
            // one.
            let events = [4, 0].map(|id| SimEvent::PlayerMessage {
                to: Some(active),
                id,
                tile: (3, 4),
                flags: 0x0301_0000,
            });
            session.hud.handle(&events, active, &mut Vec::new());
            assert!(session.hud.messages.heads().next().is_some());
            assert_eq!(session.hud.messages.queue(0).len(), 2);
            assert_eq!(session.hud.actions.tiles().len(), 1);
        };
        fill(&mut session);
        // Restart of the view (viewer `sim_load`): empty.
        session.reset_view(&gpu, &vfs);
        assert!(session.hud.messages.heads().next().is_none());
        assert!(session.hud.actions.tiles().is_empty());
        // Saving with a filled bar and resuming: empty.
        fill(&mut session);
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: 0,
            local_name: "Tester".into(),
            game_speed: 10,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: crate::game::control_groups::empty_save(),
            next_guid_index: crate::game::control_groups::empty_next_guid_index(),
            idle_worker_index: [0; 8],
            masks: crate::game::diplomacy::Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (320, 200),
            &mut |_| {},
        )
        .unwrap();
        assert!(resumed.hud.messages.heads().next().is_none());
        assert!(resumed.hud.actions.tiles().is_empty());
    }

    /// The bar runs via [`GameSession::advance`] only with the match (O-213): without `running` the head stands, with
    /// `running` it expires by the frame's game time (speed 10: 500 ms frame time = 500 ms game time).
    // QUIRK O-213: kept
    #[test]
    fn session_advance_ticks_the_bar_only_while_running() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        assert_eq!(session.sim().unwrap().game_speed, 10);
        let active = session.sim().unwrap().active_player;
        let event = SimEvent::PlayerMessage {
            to: Some(active),
            id: 0,
            tile: (-1, -1),
            flags: 0x0301_0000,
        };
        session.hud.handle(&[event], active, &mut Vec::new());
        let head = |s: &GameSession| {
            let (_, m) = s.hud.messages.heads().find(|(_, m)| m.id == 0).unwrap();
            (m.display_ms, m.fade_ms)
        };
        let shown = head(&session);
        assert!(shown.0 > 500, "{shown:?}");
        session.advance(&gpu, &vfs, 1_000, true, None);
        assert_eq!(head(&session), shown, "first frame: frame time 0");
        for now in [2_000, 30_000, 60_000] {
            session.advance(&gpu, &vfs, now, false, None);
        }
        assert_eq!(head(&session), shown, "game menu open: the bar stands");
        session.advance(&gpu, &vfs, 60_500, true, None);
        assert_eq!(head(&session), (shown.0 - 500, shown.1));
    }

    /// Last button (deviation from the original, O-269): `None` at session start, after a button its value, `None`
    /// again after restarting the view (`reset_view`) and after loading; not in the savegame.
    // QUIRK O-269: deviation
    #[test]
    fn last_button_starts_empty_and_resets_on_load() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        assert_eq!(session.last_button, None);
        session.button_pressed(408);
        assert_eq!(session.last_button, Some(408));
        session.reset_view(&gpu, &vfs);
        assert_eq!(session.last_button, None);
        session.button_pressed(511);
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: 0,
            local_name: "Tester".into(),
            game_speed: 10,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: crate::game::control_groups::empty_save(),
            next_guid_index: crate::game::control_groups::empty_next_guid_index(),
            idle_worker_index: [0; 8],
            masks: crate::game::diplomacy::Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (320, 200),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(resumed.last_button, None);
    }

    /// O-246/O-269: every click path reports the button first; for selection, hero and occupant buttons the value is
    /// the HID, the headhunter button reports twice with its own HQ. [`GameSession::toolbar_outputs`] passes each
    /// report in sequence to [`GameSession::button_pressed`].
    // QUIRK O-246: kept
    // QUIRK O-269: deviation
    #[test]
    fn button_pressed_records_hid_for_selection_hero_and_occupant_buttons() {
        use opennml_ui::hud::toolbar::button::{ButtonEvent, KIND_HERO, KIND_SELECTION};
        use opennml_ui::hud::toolbar::data::ToolbarData;
        use opennml_ui::hud::toolbar::set::{KIND_HEADHUNTER, KIND_OCCUPANT};
        use opennml_ui::hud::toolbar::{Toolbar, ToolbarEvent, ToolbarOut};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        let screen = Screen {
            width: 1024,
            height: 768,
        };
        let mut data = ToolbarData::from_texts("", "");
        data.normal = (0..14)
            .map(|i| [245 + 53 * (i % 7), if i < 7 { 17 } else { 71 }])
            .collect();
        let mut toolbar = Toolbar::new(data, screen);
        let (hero, occupant, unit, hq) =
            (0x10_0011_u32, 0x10_0012_u32, 0x10_0013_u32, 0x10_0014_u32);
        let mut press = |session: &mut GameSession, msg: i32, kind: u32, object: Option<u32>| {
            let mut model = crate::game::toolbar_host::toolbar_model(
                session.sim().unwrap(),
                &session.selection.ids,
                false,
            );
            model.heroes = vec![(hero, 1201)];
            model.headquarters = Some(hq);
            let mut outs = Vec::new();
            toolbar.press(
                ToolbarEvent::Click {
                    msg,
                    kind,
                    object,
                    ev: ButtonEvent::Left,
                },
                &model,
                &mut outs,
            );
            drop(model);
            let values: Vec<i32> = outs
                .iter()
                .filter_map(|o| match o {
                    ToolbarOut::ButtonPressed(v) => Some(*v),
                    _ => None,
                })
                .collect();
            let set = toolbar.set.clone();
            session.toolbar_outputs(outs, &set, &vfs, None);
            values
        };
        assert_eq!(
            press(&mut session, 7, KIND_SELECTION, Some(unit)),
            [unit as i32]
        );
        assert_eq!(session.last_button, Some(unit as i32));
        assert_eq!(press(&mut session, 7, KIND_HERO, Some(hero)), [hero as i32]);
        assert_eq!(session.last_button, Some(hero as i32));
        assert_eq!(
            press(&mut session, 7, KIND_OCCUPANT, Some(occupant)),
            [occupant as i32]
        );
        assert_eq!(session.last_button, Some(occupant as i32));
        assert_eq!(
            press(&mut session, 4100, KIND_HEADHUNTER, None),
            [4100, 4100]
        );
        assert_eq!(session.last_button, Some(4100));
        assert_eq!(
            press(
                &mut session,
                408,
                opennml_formats::gifcon::button_kind::DO,
                None
            ),
            [408]
        );
        assert_eq!(session.last_button, Some(408));
    }

    // ---- Command bar in the match ----------------------------------------------------------------------------------

    /// Host modes that only count how often the world saw a left click.
    #[derive(Default)]
    struct CountingModes {
        presses: usize,
        releases: usize,
    }

    impl HostModes for CountingModes {
        fn left_press(&mut self) -> bool {
            self.presses += 1;
            false
        }

        fn left_release(&mut self, _session: &mut GameSession) -> bool {
            self.releases += 1;
            false
        }
    }

    /// Match in 1024 x 768 with the first own object with a bar menu selected; two frames so that the bar builds its
    /// buttons (`{8}` -> full rebuild, then frame event 6).
    fn toolbar_session(gpu: &SessionGpu, vfs: &Vfs) -> GameSession {
        let mut session = start_two_players(gpu, vfs, &mut Vec::new()).unwrap();
        select_menu_object(&mut session, gpu, vfs);
        session
    }

    /// [`toolbar_session`] on an already started match: 1024 x 768, first own object with a bar menu selected, two
    /// frames of 16 ms.
    fn select_menu_object(session: &mut GameSession, gpu: &SessionGpu, vfs: &Vfs) {
        session.resize(1024, 768);
        let sim = session.sim().unwrap();
        let local = usize::from(sim.active_player);
        let first = sim.state.tables[local]
            .objects
            .iter()
            .flatten()
            .find(|o| {
                sim.rules
                    .toolbar
                    .menus
                    .get(&(o.guid as i32))
                    .is_some_and(|m| !m.is_empty())
            })
            .map(|o| o.id)
            .expect("own object with a bar menu");
        session.selection.ids = vec![first];
        session.selection_changed(vfs, None);
        session.advance_by(gpu, vfs, 16.0, true, None);
        session.advance_by(gpu, vfs, 16.0, true, None);
    }

    fn move_to(session: &mut GameSession, vfs: &Vfs, modes: &mut CountingModes, at: [i32; 2]) {
        let ev = InputEvent::CursorMoved {
            x: f64::from(at[0]),
            y: f64::from(at[1]),
        };
        let screen = (session.size.0, session.size.1);
        session.input_with(&ev, screen, 0.0, vfs, None, modes);
    }

    /// Left click at `at` at clock `now_ms` (movement, press, release).
    fn left_click(
        session: &mut GameSession,
        vfs: &Vfs,
        modes: &mut CountingModes,
        at: [i32; 2],
        now_ms: f64,
    ) {
        move_to(session, vfs, modes, at);
        let screen = (session.size.0, session.size.1);
        for pressed in [true, false] {
            let ev = InputEvent::Button {
                button: MouseButton::Left,
                pressed,
            };
            session.input_with(&ev, screen, now_ms, vfs, None, modes);
        }
    }

    /// Center of the first clickable bar button and its `BUTTONMSG`.
    fn clickable_button(session: &GameSession) -> ([i32; 2], i32) {
        let toolbar = session.hud.toolbar.as_ref().unwrap();
        let b = toolbar
            .buttons()
            .find(|b| b.clickable)
            .expect("clickable button");
        let [l, t, r, bo] = b.spec.rect;
        ([(l + r) / 2, (t + bo) / 2], b.spec.msg)
    }

    /// Free bar area at the top left of the bar (no button).
    fn free_bar_point(session: &GameSession) -> [i32; 2] {
        let [l, t, ..] = session.hud.toolbar.as_ref().unwrap().rect();
        [l + 5, t + 5]
    }

    fn gpu_of<'a>(device: &'a wgpu::Device, queue: &'a wgpu::Queue) -> SessionGpu<'a> {
        SessionGpu {
            device,
            queue,
            format: opennml_render::CAPTURE_FORMAT,
        }
    }

    /// O-222: a click in the bar rectangle belongs to the bar, the world does not see it; a click on a button takes the
    /// click path (first the last button).
    // QUIRK O-222: kept
    #[test]
    fn hud_takes_bar_clicks_before_the_world() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let mut modes = CountingModes::default();
        let selection = session.selection.ids.clone();
        let free = free_bar_point(&session);
        left_click(&mut session, &vfs, &mut modes, free, 1_000.0);
        assert_eq!((modes.presses, modes.releases), (0, 0), "free bar area");
        assert_eq!(session.selection.ids, selection, "selection stays");
        assert_eq!(session.last_button, None);
        // Button: first the mouse over it (the initial lock falls), one frame (clickable), then the click.
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let (at, msg) = clickable_button(&session);
        move_to(&mut session, &vfs, &mut modes, at);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        left_click(&mut session, &vfs, &mut modes, at, 5_000.0);
        assert_eq!((modes.presses, modes.releases), (0, 0), "button");
        assert_eq!(session.last_button, Some(msg));
    }

    /// Clicks outside the windows go to the world. In the 16 px overlap of minimap and bar at 800 x 600 the tree order
    /// applies: outside the diamond the minimap does not take the press, so the bar does; the minimap takes the release
    /// in the rectangle. The world sees neither.
    // QUIRK O-222: kept
    #[test]
    fn world_gets_clicks_outside_the_windows() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let mut modes = CountingModes::default();
        left_click(&mut session, &vfs, &mut modes, [500, 300], 1_000.0);
        assert_eq!((modes.presses, modes.releases), (1, 1));
        session.resize(800, 600);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let minimap = session
            .hud
            .windows
            .find(opennml_ui::hud::tree::WindowKind::Minimap)
            .map(|id| session.hud.windows.rect(id))
            .unwrap();
        let bar = session.hud.toolbar.as_ref().unwrap().rect();
        let overlap = [minimap[0].max(bar[0]), minimap[1].max(bar[1])];
        assert!(
            overlap[0] < minimap[2].min(bar[2]) && overlap[1] < minimap[3].min(bar[3]),
            "overlap at 800 x 600: {minimap:?} {bar:?}"
        );
        left_click(&mut session, &vfs, &mut modes, overlap, 3_000.0);
        assert_eq!(
            (modes.presses, modes.releases),
            (1, 1),
            "overlap -> bar and minimap, not the world"
        );
    }

    /// Game menu open (the match is not running): the session passes nothing to the bar, it stands still (pointer
    /// position, object under the mouse, buttons); `GameFlow` then lets only movements through, a click does not reach
    /// the bar either. After resuming it takes clicks again.
    #[test]
    fn toolbar_ignores_clicks_while_the_game_menu_is_open() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let mut modes = CountingModes::default();
        let selection = session.selection.ids.clone();
        let (at, _) = clickable_button(&session);
        session.advance(&gpu, &vfs, 1_000, true, None);
        session.advance(&gpu, &vfs, 1_016, false, None);
        let toolbar = session.hud.toolbar.clone().unwrap();
        move_to(&mut session, &vfs, &mut modes, at);
        session.advance(&gpu, &vfs, 1_032, false, None);
        left_click(&mut session, &vfs, &mut modes, at, 1_100.0);
        assert_eq!(session.last_button, None, "no click to the bar");
        let now = session.hud.toolbar.as_ref().unwrap();
        assert_eq!(now.mouse_pos, toolbar.mouse_pos, "pointer position stays");
        assert_eq!(now.hover, toolbar.hover, "object under the mouse stays");
        // Resuming: selection as before, mouse over the button, one frame, click.
        session.selection.ids = selection;
        session.selection_changed(&vfs, None);
        session.advance(&gpu, &vfs, 1_048, true, None);
        session.advance(&gpu, &vfs, 1_064, true, None);
        let (at, msg) = clickable_button(&session);
        move_to(&mut session, &vfs, &mut modes, at);
        session.advance(&gpu, &vfs, 1_080, true, None);
        left_click(&mut session, &vfs, &mut modes, at, 9_000.0);
        assert_eq!(session.last_button, Some(msg));
    }

    /// O-309: `button(n)` and `xbutton(n)` act only with a visible bar (event 9). `button` searches the row in the
    /// normal slots and checks the lock including `REQUIRED_GUID_NATION` (`Toolbar_IsButtonLocked`, which the drawn bar
    /// leaves out); `xbutton` sends `{n, DO}` without button and lock like a left click: `xbutton(408)` (F12) thus
    /// gives the command Die to the selection.
    // QUIRK O-309: kept
    #[test]
    fn button_and_xbutton_only_with_a_visible_toolbar() {
        use crate::game::hotkeys::HotkeyCommand as C;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let die = |s: &GameSession| {
            s.sim()
                .unwrap()
                .pending_commands()
                .iter()
                .filter(|c| matches!(c.kind, CommandKind::Die { .. }))
                .count()
        };
        let row = session
            .hud
            .toolbar
            .as_ref()
            .unwrap()
            .set
            .slots
            .iter()
            .flatten()
            .next()
            .cloned();
        let row = row.expect("a row in the shown set");
        // Hidden (`SPACE` 570, `Q` as `button`, `F12` 408): nothing.
        session.hud.toolbar.as_mut().unwrap().shown = false;
        for cmd in [C::XButton(570), C::Button(row.msg), C::XButton(408)] {
            assert!(session.run_hotkey(&cmd, 0, &vfs, None), "{cmd:?} taken");
        }
        assert_eq!(session.last_button, None, "hidden: no button");
        assert_eq!(die(&session), 0);
        session.hud.toolbar.as_mut().unwrap().shown = true;
        // `button(n)` without a row in the set: nothing.
        session.run_hotkey(&C::Button(99_999), 0, &vfs, None);
        assert_eq!(session.last_button, None, "no button 99999");
        // Locked via `REQUIRED_GUID_NATION` (nation part of the original): nothing; without the lock the click path.
        let set_nation = |s: &mut GameSession, nation: i32| {
            let t = s.hud.toolbar.as_mut().unwrap();
            let r = t
                .set
                .slots
                .iter_mut()
                .flatten()
                .find(|r| r.msg == row.msg)
                .unwrap();
            r.flags = opennml_formats::gifcon::button_kind::DO;
            r.required = [0; 5];
            r.required_nation = [nation, 0, 0, 0, 0];
        };
        set_nation(&mut session, 5);
        session.run_hotkey(&C::Button(row.msg), 0, &vfs, None);
        assert_eq!(session.last_button, None, "locked");
        set_nation(&mut session, 0);
        session.run_hotkey(&C::Button(row.msg), 0, &vfs, None);
        assert_eq!(
            session.last_button,
            Some(row.msg),
            "free: like a left click"
        );
        // `xbutton(408)` without a button in the set: command Die to the selection.
        let toolbar = session.hud.toolbar.as_ref().unwrap();
        assert!(!toolbar.set.slots.iter().flatten().any(|r| r.msg == 408));
        session.run_hotkey(&C::XButton(408), 0, &vfs, None);
        assert_eq!(session.last_button, Some(408));
        assert_eq!(die(&session), 1, "command Die like a left click");
    }

    /// O-219: key and button 570 take the newest location from the action stack and move the game camera with the
    /// current offset to `(SIZE_X*x, 0, SIZE_Z*z)` (`0x0047F1E0`); every press uses up one location, the empty stack
    /// does nothing. Local: no command, no effect on the checksum; the pings stay.
    // QUIRK O-219: kept
    #[test]
    fn button_570_pops_the_newest_location() {
        use crate::game::hotkeys::HotkeyCommand as C;
        use opennml_ui::hud::toolbar::{ButtonSet, ToolbarIntent, ToolbarOut};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let pending = session.sim().unwrap().pending_commands().len();
        let checksum = session.checksum();
        let cell = session.sim().unwrap().rules.map.cell;
        let offset = session.map.camera.eye - session.map.camera.target;
        session.hud.ping((10, 12), 0xFFFF_0000, 5);
        session.hud.ping((30, 40), 0xFF00_FF00, 5);
        let at = |x: i32, z: i32| Vec3::new((cell[0] * x) as f32, 0.0, (cell[2] * z) as f32);
        let check = |session: &GameSession, target: Vec3, what: &str| {
            let cam = &session.map.camera;
            assert_eq!(cam.target, target, "{what}: target");
            assert!(
                (cam.eye - cam.target - offset).length() < 0.01,
                "{what}: offset stays"
            );
        };
        // Key (`xbutton(570)`, space bar): the newest location.
        assert!(session.run_hotkey(&C::XButton(570), 0, &vfs, None));
        assert_eq!(session.last_button, Some(570));
        check(&session, at(30, 40), "key");
        let stack: Vec<_> = session.hud.actions.tiles().iter().copied().collect();
        assert_eq!(stack, [(10, 12)]);
        // Button (double-click of kind `0x13000000`): the next location, a double-click changes nothing for 570.
        let outs = vec![
            ToolbarOut::ButtonPressed(570),
            ToolbarOut::Intent(ToolbarIntent::Minimap {
                msg: 570,
                double: true,
            }),
        ];
        session.toolbar_outputs(outs, &ButtonSet::default(), &vfs, None);
        check(&session, at(10, 12), "button");
        assert!(session.hud.actions.tiles().is_empty());
        // Empty stack: nothing.
        assert!(session.run_hotkey(&C::XButton(570), 0, &vfs, None));
        check(&session, at(10, 12), "empty");
        assert_eq!(session.hud.pings.list().len(), 2, "the pings stay");
        assert_eq!(session.sim().unwrap().pending_commands().len(), pending);
        assert_eq!(session.checksum(), checksum);
    }

    /// `FUN_0042E980`: window 2 sends `0x5D` to the friends of the local slot including itself (per the map's
    /// diplomacy; without allies mask `0x01`); with the tick "Anna: hello" stands as a chat line (kind 10) in the
    /// player color at (20, H - 200) and lives 8000 ms of game time. A line whose mask does not carry the local slot
    /// does not appear.
    // QUIRK O-325: kept
    #[test]
    fn chat_line_name_text_8_s() {
        use opennml_ui::hud::screen_text::kind;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let local = session.sim().unwrap().active_player;
        let mut names: [String; PLAYER_SLOTS] = Default::default();
        names[usize::from(local)] = "Anna".into();
        session.player_names = Some(names);
        let masks = session.masks;
        let sim = session.map.sim.as_mut().unwrap();
        let command = chat::command(2, local, "hello", sim, &masks).expect("command");
        // The friends per the map's diplomacy, the sender included.
        let friends = (0..8)
            .filter(|&q| sim.state.diplomacy.is_friend(i32::from(local), q))
            .fold(0u32, |m, q| m | 1 << q);
        assert_ne!(friends & 1 << local, 0);
        assert_eq!(
            command.kind,
            opennml_sim::command::CommandKind::Chat {
                player: i32::from(local),
                mask: friends,
                text: "hello".encode_utf16().collect(),
            }
        );
        sim.submit(command);
        // A line to another slot: not for us.
        let mut other = chat::command(1, local, "not here", sim, &masks).unwrap();
        if let opennml_sim::command::CommandKind::Chat { mask, .. } = &mut other.kind {
            *mask = !(1 << local);
        }
        sim.submit(other);
        let color = sim.state.players[usize::from(local)].color;
        assert!(session.hud.screen_texts.entries().is_empty());
        session.advance_by(&gpu, &vfs, 100.0, true, None);
        let lines: Vec<_> = session.hud.screen_texts.chat_lines().cloned().collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        let line = &lines[0];
        assert_eq!(line.own_text.as_deref(), Some("Anna: hello"));
        assert_eq!(line.kind, kind::CHAT);
        assert_eq!(line.ms, 8000);
        assert_eq!(line.color, opennml_world::markers::marker_argb(color));
        let start = line.motion.as_ref().unwrap().start;
        assert_eq!(start, [20.0, session.size.1 as f32 - 200.0]);
        // Lives until 8000 ms of the bar's game time, then gone.
        let mut frames = 0;
        while session.hud.screen_texts.chat_lines().count() == 1 {
            let elapsed = session
                .hud
                .screen_texts
                .chat_lines()
                .next()
                .unwrap()
                .elapsed;
            assert!(elapsed < 8000, "{elapsed}");
            session.advance_by(&gpu, &vfs, 100.0, true, None);
            frames += 1;
            assert!(frames < 200, "line stays");
        }
    }

    /// O-321: key and button 571 send with the camera target `(9000, y, 13000)` the command `{0x5C, slot, mask, 70,
    /// 101}` (tile = target / tile size, truncated, mask = chat mask of the local slot) via the normal command path;
    /// before the tick neither checksum nor ping changes. With the tick the ping at (70, 101) with one repetition in
    /// the slot's color and the location on the stack arrive. A second signal at the same tile only deposits the
    /// location once more.
    // QUIRK O-321: kept
    #[test]
    fn signal_m9() {
        use crate::game::hotkeys::HotkeyCommand as C;
        use opennml_sim::command::CommandKind;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let local = session.sim().unwrap().active_player;
        let color = opennml_sim::setup::player_color(
            &opennml_sim::sim::Sim::new(
                &mut session.sim().unwrap().state.clone(),
                &session.sim().unwrap().rules,
            ),
            i32::from(local),
        );
        let pending = session.sim().unwrap().pending_commands().len();
        let checksum = session.checksum();
        session.map.camera.target = Vec3::new(9000.0, 0.0, 13000.0);
        assert!(session.run_hotkey(&C::XButton(571), 0, &vfs, None));
        let sent = &session.sim().unwrap().pending_commands()[pending..];
        assert_eq!(sent.len(), 1, "one command");
        assert_eq!(sent[0].player, local);
        assert_eq!(sent[0].kind.type_code(), 0x5000_005C);
        assert_eq!(
            sent[0].kind,
            CommandKind::Signal {
                player: i32::from(local),
                mask: session.masks.chat[usize::from(local)],
                x: 70,
                z: 101,
            }
        );
        assert_eq!(session.masks.chat[usize::from(local)], 1 << local);
        assert!(
            session.hud.pings.list().is_empty(),
            "ping only with the tick"
        );
        assert_eq!(session.checksum(), checksum, "pending command: no state");
        // Button: the same command, a second one at the same tile.
        session.minimap_button(571, false, &vfs, None);
        assert_eq!(session.sim().unwrap().pending_commands().len(), pending + 2);
        for _ in 0..3 {
            session.advance_by(&gpu, &vfs, 100.0, true, None);
        }
        let pings: Vec<_> = session
            .hud
            .pings
            .list()
            .iter()
            .map(|p| (p.tile, p.color, p.repeats))
            .collect();
        assert_eq!(
            pings,
            [((70, 101), opennml_world::markers::marker_argb(color), 1)]
        );
        let stack: Vec<_> = session.hud.actions.tiles().iter().copied().collect();
        assert_eq!(&stack[..2], [(70, 101), (70, 101)]);
    }

    /// In pause the command runs immediately (`execute_now`, in the log), but the event comes only with the first tick
    /// after resuming (deviation from the original); before that no ping and no stack entry.
    // QUIRK O-321: kept
    #[test]
    fn signal_in_pause_shows_up_after_resuming() {
        use crate::game::hotkeys::HotkeyCommand as C;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        session.map.camera.target = Vec3::new(9000.0, 0.0, 13000.0);
        assert!(session.toggle_pause());
        let pending = session.sim().unwrap().pending_commands().len();
        assert!(session.run_hotkey(&C::XButton(571), 0, &vfs, None));
        assert_eq!(
            session.sim().unwrap().pending_commands().len(),
            pending,
            "executed immediately"
        );
        assert!(session.hud.pings.list().is_empty());
        assert!(session.toggle_pause());
        for _ in 0..3 {
            session.advance_by(&gpu, &vfs, 100.0, true, None);
        }
        assert_eq!(session.hud.pings.list().len(), 1);
        assert_eq!(session.hud.pings.list()[0].tile, (70, 101));
    }

    /// 578 (quicksave) and 581 (load) are not known to the dispatcher (re/game-ui N.5.4): they reach the bar (last
    /// button), but no minimap branch: no command, no camera move, the action stack stays.
    // QUIRK O-219, O-292: kept
    #[test]
    fn xbuttons_578_and_581_have_no_branch() {
        use crate::game::hotkeys::HotkeyCommand as C;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let pending = session.sim().unwrap().pending_commands().len();
        session.hud.ping((10, 12), 0xFFFF_0000, 5);
        let camera = (session.map.camera.eye, session.map.camera.target);
        for n in [578, 581] {
            assert!(session.run_hotkey(&C::XButton(n), 0, &vfs, None));
            assert_eq!(session.last_button, Some(n), "{n} reaches the bar");
            assert_eq!(
                (session.map.camera.eye, session.map.camera.target),
                camera,
                "{n}: no camera move"
            );
            assert_eq!(session.hud.actions.tiles().len(), 1, "{n}: stack stays");
            assert_eq!(
                session.sim().unwrap().pending_commands().len(),
                pending,
                "{n}: no command"
            );
        }
    }

    // ---- Corner buttons and idle workers ----------------------------------------------------------------------------

    /// Three own workers as the list of the idle workers of the local slot (from the level start's list), index 0; plus
    /// the local slot. Beforehand one second of game time so that the fog stands (only what is visible gets selected).
    fn idle_three(
        session: &mut GameSession,
        gpu: &SessionGpu,
        vfs: &Vfs,
    ) -> (usize, [ObjectId; 3]) {
        for _ in 0..10 {
            session.advance_by(gpu, vfs, 100.0, true, None);
        }
        let sim = session.map.sim.as_mut().unwrap();
        let local = usize::from(sim.active_player);
        let list = sim.state.idle_workers.list(sim.active_player).to_vec();
        assert!(
            list.len() >= 3,
            "three idle workers at level start: {list:?}"
        );
        let three = [list[0], list[1], list[2]];
        sim.state.idle_workers.lists[local] = three.to_vec();
        session.idle_worker_index[local] = 0;
        (local, three)
    }

    /// Number of `{0x10, 2}` messages (selection changed, `selection_changed`) to the bar since the last frame.
    fn selection_events(session: &GameSession) -> usize {
        session
            .toolbar_host
            .pending()
            .iter()
            .filter(|e| e.kind == opennml_ui::hud::HudEventKind::SelectionChanged)
            .count()
    }

    /// Tile of an object (`GameObject_GetTilePos`).
    fn tile_of(session: &GameSession, id: ObjectId) -> (i32, i32) {
        session.sim().unwrap().state.object(id, false).unwrap().tile
    }

    /// O-293: 576 selects [A, B, C] in turn from index 0 (A, B, C, A), every press first clears and reports `{0x10, 2}`
    /// twice, the tile of the selected one goes to the front of the action stack, the camera stays. Key
    /// (`xbutton(576)`) and button take the same path. A missing object still raises the index and leaves the selection
    /// empty (one message); an empty list clears the selection (one message). No command, no effect on the checksum.
    // QUIRK O-293: kept
    #[test]
    fn idle_576_cycles_m8() {
        use crate::game::hotkeys::HotkeyCommand as C;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let (local, [a, b, c]) = idle_three(&mut session, &gpu, &vfs);
        let checksum = session.checksum();
        let pending = session.sim().unwrap().pending_commands().len();
        let camera = (session.map.camera.eye, session.map.camera.target);
        session.hud.actions.clear();
        for (i, expected) in [a, b, c, a].into_iter().enumerate() {
            let before = selection_events(&session);
            if i == 1 {
                assert!(session.run_hotkey(&C::XButton(576), 0, &vfs, None));
            } else {
                session.minimap_button(576, false, &vfs, None);
            }
            assert_eq!(session.selection.ids, [expected], "press {i}");
            assert_eq!(session.idle_worker_index[local], [1, 2, 3, 1][i]);
            assert_eq!(
                selection_events(&session) - before,
                2,
                "press {i}: clear, select"
            );
            assert_eq!(session.hud.actions.tiles()[0], tile_of(&session, expected));
        }
        assert_eq!(session.hud.actions.tiles().len(), 4);
        assert_eq!(
            (session.map.camera.eye, session.map.camera.target),
            camera,
            "no camera without a double-click"
        );
        // Missing object: index + 1, selection empty, only the first message.
        let stale = ObjectId::new(a.index() as u16, a.slot(), (a.generation() + 1) & 0x7FF);
        session.map.sim.as_mut().unwrap().state.idle_workers.lists[local] = vec![stale, b];
        session.idle_worker_index[local] = 0;
        let before = selection_events(&session);
        session.minimap_button(576, false, &vfs, None);
        assert!(session.selection.ids.is_empty());
        assert_eq!(session.idle_worker_index[local], 1);
        assert_eq!(selection_events(&session) - before, 1);
        session.minimap_button(576, false, &vfs, None);
        assert_eq!(session.selection.ids, [b]);
        // Empty list: the selection is cleared anyway, the index stays.
        session.map.sim.as_mut().unwrap().state.idle_workers.lists[local].clear();
        session.minimap_button(576, false, &vfs, None);
        assert!(session.selection.ids.is_empty());
        assert_eq!(session.idle_worker_index[local], 2);
        assert_eq!(session.sim().unwrap().pending_commands().len(), pending);
        session.map.sim.as_mut().unwrap().state.idle_workers.lists[local] = vec![a, b, c];
        assert_eq!(session.checksum(), checksum, "no effect on the checksum");
    }

    /// O-293: a double-click on button 576 (mouse through the tree: 9, 9, `0x27`) selects three workers in a row; the
    /// first two end up on the action stack, only the third gets the camera (world position, offset stays). Click sound
    /// three times, `ButtonPressed` three times.
    // QUIRK O-293: kept
    #[test]
    fn idle_576_double_click_three_then_camera() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let (local, [a, b, c]) = idle_three(&mut session, &gpu, &vfs);
        session.hud.actions.clear();
        let offset = session.map.camera.eye - session.map.camera.target;
        let button = session.hud.minimap_buttons.buttons[1].spec.rect;
        assert_eq!(session.hud.minimap_buttons.buttons[1].spec.msg, 576);
        let at = [(button[0] + button[2]) / 2, (button[1] + button[3]) / 2];
        let mut modes = CountingModes::default();
        left_click(&mut session, &vfs, &mut modes, at, 1000.0);
        left_click(&mut session, &vfs, &mut modes, at, 1000.0);
        assert_eq!(
            (modes.presses, modes.releases),
            (0, 0),
            "the world sees nothing"
        );
        assert_eq!(session.selection.ids, [c]);
        assert_eq!(session.idle_worker_index[local], 3);
        let stack: Vec<_> = session.hud.actions.tiles().iter().copied().collect();
        assert_eq!(stack, [tile_of(&session, b), tile_of(&session, a)]);
        let pos = session
            .sim()
            .unwrap()
            .state
            .object(c, false)
            .unwrap()
            .world_pos;
        let camera = &session.map.camera;
        assert!((camera.target.x - pos[0]).abs() < 1e-2 && (camera.target.z - pos[2]).abs() < 1e-2);
        assert!((camera.eye - camera.target - offset).length() < 1e-1);
        assert_eq!(session.last_button, Some(576));
    }

    /// 577: all idle workers via the group path (with more than one object only units), two messages `{0x10, 2}`;
    /// neither camera nor stack nor index (O-293).
    // QUIRK O-293: kept
    #[test]
    fn idle_577_selects_all_without_camera() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let (local, three) = idle_three(&mut session, &gpu, &vfs);
        session.idle_worker_index[local] = 1;
        session.hud.actions.clear();
        let camera = (session.map.camera.eye, session.map.camera.target);
        let before = selection_events(&session);
        session.minimap_button(577, true, &vfs, None);
        assert_eq!(session.selection.ids, three);
        assert_eq!(selection_events(&session) - before, 2);
        assert_eq!(session.idle_worker_index[local], 1);
        assert!(session.hud.actions.tiles().is_empty());
        assert_eq!(
            (session.map.camera.eye, session.map.camera.target),
            camera,
            "no camera with a double-click either"
        );
    }

    /// O-292: corner buttons only with `IsMap`, 580 only for game type >= 0 (not in the campaign); a click on the mode
    /// button advances the mode and rebuilds the buttons; without a visible bar the click is taken but has no effect;
    /// 579 asks the flow for the game menu. The keys 573 ... 577 also work without `IsMap`.
    // QUIRK O-292, O-388: kept
    #[test]
    fn buttons_only_with_ismap_and_580_only_outside_campaign() {
        use crate::game::hotkeys::HotkeyCommand as C;
        use opennml_ui::hud::{HudEvent, HudEventKind};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let msgs = |s: &GameSession| -> Vec<i32> {
            s.hud
                .minimap_buttons
                .buttons
                .iter()
                .map(|b| b.spec.msg)
                .collect()
        };
        assert_eq!(session.game_type, -1);
        assert_eq!(
            msgs(&session),
            [574, 576, 571, 577, 579],
            "campaign: without 580"
        );
        session.set_game_type(0);
        assert_eq!(msgs(&session), [574, 576, 571, 577, 579, 580]);
        // Mode button by click: mode 1, the button now shows 575.
        let mut modes = CountingModes::default();
        let rect = session.hud.minimap_buttons.buttons[0].spec.rect;
        let mode_at = [(rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2];
        left_click(&mut session, &vfs, &mut modes, mode_at, 5000.0);
        assert_eq!(session.hud.minimap.as_ref().unwrap().mode, 1);
        assert_eq!(msgs(&session)[0], 575);
        assert_eq!(session.last_button, Some(574));
        // Key and right-click (event `0x26`, dispatcher for every kind of click) act likewise.
        assert!(session.run_hotkey(&C::XButton(575), 0, &vfs, None));
        assert_eq!(session.hud.minimap.as_ref().unwrap().mode, 2);
        assert_eq!(msgs(&session)[0], 573);
        let (sim, toolbar) = (
            session.map.sim.as_ref().unwrap(),
            session.hud.toolbar.as_mut().unwrap(),
        );
        let model = crate::game::toolbar_host::toolbar_model(sim, &[], false);
        let mut outs = Vec::new();
        let right = HudEvent::with_args(HudEventKind::ButtonRight, &[573, 0x1300_0000]);
        toolbar.event(&right, &model, &mut outs);
        session.apply_toolbar_outputs(outs, &vfs, None);
        assert_eq!(session.hud.minimap.as_ref().unwrap().mode, 0);
        // 579 by click (top right, outside the map): a request to the flow.
        let rect = session.hud.minimap_buttons.buttons[4].spec.rect;
        left_click(
            &mut session,
            &vfs,
            &mut modes,
            [rect[0] + 5, rect[1] + 5],
            7000.0,
        );
        assert_eq!(session.take_ui_requests(), [UiRequest::GameMenu]);
        assert!(session.take_ui_requests().is_empty());
        // Without a visible bar (`{0x11, 1}`): taken, no effect (O-292 hypothesis).
        session.hud.toolbar.as_mut().unwrap().shown = false;
        session.hud.sync_toolbar_visibility();
        left_click(&mut session, &vfs, &mut modes, mode_at, 9000.0);
        assert_eq!(
            session.hud.minimap.as_ref().unwrap().mode,
            0,
            "no effect without the bar"
        );
        assert_eq!((modes.presses, modes.releases), (0, 0), "taken");
        session.hud.toolbar.as_mut().unwrap().shown = true;
        session.hud.sync_toolbar_visibility();
        // Lock and cinemascope take the buttons away and rebuild them (re/game-ui D5.1).
        for (kind, off, on) in [
            (HudEventKind::Lock, 1, 0),
            (HudEventKind::MinimapOnOff, 0, 1),
        ] {
            let mut host = std::mem::take(&mut session.toolbar_host);
            session
                .hud
                .route(HudEvent::with_args(kind, &[off]), &mut host, false);
            assert!(msgs(&session).is_empty(), "{kind:?} {off}");
            session
                .hud
                .route(HudEvent::with_args(kind, &[on]), &mut host, false);
            assert_eq!(msgs(&session).len(), 6, "{kind:?} {on}");
            session.toolbar_host = host;
        }
        // Without `IsMap`: no buttons; key 576 still selects.
        let (_, [a, ..]) = idle_three(&mut session, &gpu, &vfs);
        without_ismap(&mut session, &vfs);
        assert!(msgs(&session).is_empty());
        session.set_game_type(1);
        assert!(msgs(&session).is_empty());
        assert!(session.run_hotkey(&C::XButton(576), 0, &vfs, None));
        assert_eq!(session.selection.ids, [a]);
    }

    /// Key 572 (`SHIFT D` = `xbutton(572)`) asks the flow for diplomacy, also in the campaign; button 580 only for game
    /// type >= 0. No command.
    // QUIRK O-323: kept
    #[test]
    fn diplomacy_key_and_button_ask_the_flow() {
        use crate::game::hotkeys::HotkeyCommand as C;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let checksum = session.sim().unwrap().checksum();
        assert_eq!(session.game_type, -1);
        assert!(session.run_hotkey(&C::XButton(572), 0, &vfs, None));
        assert_eq!(session.take_ui_requests(), [UiRequest::ToggleDiplomacy]);
        session.minimap_button(580, false, &vfs, None);
        assert!(
            session.take_ui_requests().is_empty(),
            "580 not in the campaign"
        );
        session.set_game_type(0);
        session.minimap_button(580, false, &vfs, None);
        assert_eq!(session.take_ui_requests(), [UiRequest::ToggleDiplomacy]);
        let sim = session.sim().unwrap();
        assert_eq!(sim.checksum(), checksum);
        assert!(sim.pending_commands().is_empty());
    }

    /// Index of the idle workers and masks go into the host part and come back with `resume_from`; the next press on
    /// 576 continues there.
    // QUIRK O-293: kept
    #[test]
    fn idle_index_survives_save_and_load() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let (local, list) = idle_three(&mut session, &gpu, &vfs);
        session.minimap_button(576, false, &vfs, None);
        assert_eq!(session.selection.ids, [list[0]]);
        session.idle_worker_index[7] = 4;
        session.masks.chat[local] = 0b11;
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: 0,
            local_name: "Tester".into(),
            game_speed: 10,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: session.control_groups_save(),
            next_guid_index: session.next_guid_index_save(),
            idle_worker_index: session.idle_worker_index,
            masks: session.masks,
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let mut resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (1024, 768),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(resumed.idle_worker_index, session.idle_worker_index);
        assert_eq!(resumed.idle_worker_index[local], 1);
        assert_eq!(resumed.masks, session.masks);
        assert_eq!(resumed.game_type, 0);
        assert_eq!(
            resumed.hud.minimap_buttons.buttons.len(),
            6,
            "580 after loading"
        );
        resumed.minimap_button(576, false, &vfs, None);
        assert_eq!(resumed.selection.ids, [list[1]]);
        // A new match begins with index 0 and the start masks.
        let fresh = toolbar_session(&gpu, &vfs);
        assert_eq!(fresh.idle_worker_index, [0; 8]);
        assert_eq!(fresh.masks, crate::game::diplomacy::Masks::initial());
    }

    /// The fog switch `m_FogShowMode` is in the savegame as in the original (`HostPart::fog_show_mode`); `rainbowring`
    /// survives saving and resuming. It applies after rebuilding the presentation: the script state "fog on"
    /// (`fog_show_all = Some(false)`) does not reset it on resuming, because the original has a single variable.
    #[test]
    fn fog_show_mode_survives_save_and_load() {
        use crate::game::fog_view::FOG_OFF;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = tutorial_session(&gpu, &vfs);
        let sim = session.sim_mut().unwrap();
        let local = sim.active_player;
        assert_eq!(sim.fog.show_mode, 0);
        sim.submit(opennml_sim::command::Command {
            player: local,
            kind: opennml_sim::command::CommandKind::Chat {
                player: i32::from(local),
                mask: 1 << local,
                text: "rainbowring".encode_utf16().collect(),
            },
        });
        sim.step_once();
        assert_eq!(sim.fog.show_mode, FOG_OFF, "rainbowring");
        // Script state "fog on" after the tick (the tick could set it via the script); the tutorial has a script,
        // without a script loading rejected a changed script state.
        sim.state.script.mission.presentation.fog_show_all = Some(false);
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: -1,
            local_name: "Tester".into(),
            game_speed: 10,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: session.control_groups_save(),
            next_guid_index: session.next_guid_index_save(),
            idle_worker_index: session.idle_worker_index,
            masks: session.masks,
            lock_teams: false,
            fog_show_mode: session.sim().unwrap().fog.show_mode,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        assert_eq!(file.host.fog_show_mode, FOG_OFF);
        let resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (1024, 768),
            &mut |_| {},
        )
        .unwrap();
        let sim = resumed.sim().unwrap();
        assert_eq!(sim.fog.show_mode, FOG_OFF, "still off after resuming");
        assert!(!sim.section(&[]).fog_shown);
    }

    /// `reset_view` (viewer after `sim_load`/`sim_setup`) starts like a new match: index of the idle workers 0, start
    /// masks, map mode 0 (not in the savegame) and the corner buttons rebuilt accordingly (mode button 574 again).
    #[test]
    fn reset_view_starts_idle_index_masks_and_minimap_mode_afresh() {
        use crate::game::hotkeys::HotkeyCommand as C;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let first_msg = |s: &GameSession| s.hud.minimap_buttons.buttons[0].spec.msg;
        assert_eq!(first_msg(&session), 574);
        assert!(session.run_hotkey(&C::XButton(574), 0, &vfs, None));
        assert_eq!(session.hud.minimap.as_ref().unwrap().mode, 1);
        assert_eq!(first_msg(&session), 575);
        let local = usize::from(session.sim().unwrap().active_player);
        session.idle_worker_index[local] = 2;
        session.idle_worker_index[7] = 5;
        session.masks.chat[local] = 0b11;
        session.masks.tribute[3] = 0xFF;
        session.reset_view(&gpu, &vfs);
        assert_eq!(session.idle_worker_index, [0; 8]);
        assert_eq!(session.masks, crate::game::diplomacy::Masks::initial());
        assert_eq!(session.hud.minimap.as_ref().unwrap().mode, 0);
        assert_eq!(first_msg(&session), 574, "corner buttons for mode 0");
    }

    // ---- Minimap input ----------------------------------------------------------------------------------------------

    /// Draws the UI once (like a frame of `opennml-game`): the minimap remembers the position that clicks calculate with.
    fn draw_hud_once(session: &GameSession, vfs: &Vfs) -> crate::game::minimap::MinimapLayout {
        let mut layer = crate::ui_layer::UiLayer::new(vfs, "DEU", 1024, None).unwrap();
        let mut ctx = layer.context(vfs, screen_of((1024, 768)));
        let texts = ctx.texts;
        session.hud_cmds(&mut ctx, texts);
        session.hud.minimap.as_ref().unwrap().last_layout().unwrap()
    }

    /// [`toolbar_session`] with the minimap drawn once (start camera, theta = -0.768) and its layout.
    fn minimap_session(
        gpu: &SessionGpu,
        vfs: &Vfs,
    ) -> (GameSession, crate::game::minimap::MinimapLayout) {
        let session = toolbar_session(gpu, vfs);
        let layout = draw_hud_once(&session, vfs);
        (session, layout)
    }

    /// Whole pixel on the drawn minimap at the fraction (fx, fz) of the map.
    fn map_point(layout: &crate::game::minimap::MinimapLayout, fx: f32, fz: f32) -> [i32; 2] {
        let [x, y] = layout.tile_to_screen(fx * layout.dim[0] as f32, fz * layout.dim[1] as f32);
        [x.round() as i32, y.round() as i32]
    }

    /// Presses or releases a mouse button at clock `now_ms` at the last pointer position.
    fn mouse_button(
        session: &mut GameSession,
        vfs: &Vfs,
        modes: &mut CountingModes,
        button: MouseButton,
        pressed: bool,
        now_ms: f64,
    ) {
        let screen = (session.size.0, session.size.1);
        let ev = InputEvent::Button { button, pressed };
        session.input_with(&ev, screen, now_ms, vfs, None, modes);
    }

    /// `IsMap 0`: the tree without minimap (status bar reread, the other switches on).
    fn without_ismap(session: &mut GameSession, vfs: &Vfs) {
        use crate::game::hud::HudOptions;
        let options = HudOptions {
            switches: opennml_ui::hud::tree::HudSwitches {
                map: false,
                ..opennml_ui::hud::tree::HudSwitches::default()
            },
            ..HudOptions::default()
        };
        session.hud.load_status_bar(vfs, &options);
        assert!(
            session
                .hud
                .windows
                .find(opennml_ui::hud::tree::WindowKind::Minimap)
                .is_none()
        );
    }

    /// Walk commands of the queue as `(x, z, flags, units)`.
    fn pending_moves(session: &GameSession) -> Vec<(i32, i32, i32, Vec<ObjectId>)> {
        session
            .sim()
            .unwrap()
            .pending_commands()
            .iter()
            .filter_map(|c| match &c.kind {
                CommandKind::Move {
                    units, x, z, flags, ..
                } => Some((*x, *z, *flags, units.clone())),
                _ => None,
            })
            .collect()
    }

    /// O-285: left press in the diamond sets the camera to `(SIZE_X*mx, 0, SIZE_Z*mz)` with the old offset; dragging
    /// with the button held and releasing no longer change it. The world does not see the click, there is no command.
    // QUIRK O-285: kept
    #[test]
    fn drag_does_not_move_the_camera() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut session, layout) = minimap_session(&gpu, &vfs);
        let mut modes = CountingModes::default();
        let checksum = session.checksum();
        let pending = session.sim().unwrap().pending_commands().len();
        let cell = session.sim().unwrap().rules.map.cell;
        let offset = session.map.camera.eye - session.map.camera.target;
        let at = map_point(&layout, 0.3, 0.25);
        let (mx, mz) = layout.screen_to_tile([at[0] as f32, at[1] as f32]).unwrap();
        move_to(&mut session, &vfs, &mut modes, at);
        mouse_button(
            &mut session,
            &vfs,
            &mut modes,
            MouseButton::Left,
            true,
            1_000.0,
        );
        let jumped = (session.map.camera.eye, session.map.camera.target);
        let want = Vec3::new(cell[0] as f32 * mx, 0.0, cell[2] as f32 * mz);
        assert!(
            (jumped.1 - want).length() < 0.01,
            "target {:?} instead of {want:?}",
            jumped.1
        );
        assert!(
            (jumped.0 - jumped.1 - offset).length() < 0.01,
            "offset stays"
        );
        // Dragging over the map and beyond, then releasing: the camera stands still.
        for to in [map_point(&layout, 0.7, 0.8), [500, 300]] {
            move_to(&mut session, &vfs, &mut modes, to);
            assert_eq!(
                (session.map.camera.eye, session.map.camera.target),
                jumped,
                "dragging to {to:?}"
            );
        }
        mouse_button(
            &mut session,
            &vfs,
            &mut modes,
            MouseButton::Left,
            false,
            1_100.0,
        );
        assert_eq!((session.map.camera.eye, session.map.camera.target), jumped);
        assert_eq!(modes.presses, 0, "the world did not see the press");
        assert_eq!(session.sim().unwrap().pending_commands().len(), pending);
        assert_eq!(
            session.checksum(),
            checksum,
            "local, no effect on the checksum"
        );
    }

    /// O-40: right-click in the diamond gives the same command as `sim_command` with `Move` (fixed point from the
    /// floating-point tile, formation 0, no direction, target radius -1, spread -1, not running): the same queue, the
    /// same checksum after the next steps.
    // QUIRK O-40: kept
    #[test]
    fn right_click_sends_move_like_sim_command() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut clicked, layout) = minimap_session(&gpu, &vfs);
        let mut by_hand = toolbar_session(&gpu, &vfs);
        assert_eq!(clicked.checksum(), by_hand.checksum());
        let at = map_point(&layout, 0.55, 0.4);
        let (mx, mz) = layout.screen_to_tile([at[0] as f32, at[1] as f32]).unwrap();
        let (x, z) = ((mx * 4096.0) as i32, (mz * 4096.0) as i32);
        let mut modes = CountingModes::default();
        right_click(&mut clicked, &vfs, &mut modes, at, 1_000.0);
        let player = by_hand.sim().unwrap().active_player;
        let units = serde_json::to_value(&by_hand.selection.ids).unwrap();
        let command = crate::game::sim_control::parse_sim_command(serde_json::json!({
            "player": player,
            "kind": {"Move": {
                "units": units, "x": x, "z": z, "target": -1, "formation": 0,
                "direction": opennml_sim::motion::NO_DIRECTION, "spread": -1, "flags": 0
            }}
        }))
        .unwrap();
        by_hand.sim_mut().unwrap().control_submit(command);
        let queued = |s: &GameSession| s.sim().unwrap().pending_commands().to_vec();
        assert_eq!(queued(&clicked).len(), 1, "one command");
        assert_eq!(queued(&clicked), queued(&by_hand));
        assert_eq!(modes.presses, 0);
        for _ in 0..20 {
            clicked.advance_by(&gpu, &vfs, 50.0, true, None);
            by_hand.advance_by(&gpu, &vfs, 50.0, true, None);
        }
        assert_eq!(clicked.checksum(), by_hand.checksum(), "same checksum");
    }

    /// With an open build cursor the minimap takes the left click (camera jump, no building) and the right click (move
    /// command to the selection) before the world; the build cursor stays open (O-40, O-285).
    // QUIRK O-40, O-285: kept
    #[test]
    fn minimap_takes_both_buttons_before_an_open_build_cursor() {
        use opennml_ui::hud::toolbar::{BuildCursor, Cursor};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut session, layout) = minimap_session(&gpu, &vfs);
        let guid = session
            .sim()
            .unwrap()
            .state
            .object(session.selection.ids[0], false)
            .unwrap()
            .guid;
        session.hud.toolbar.as_mut().unwrap().cursor = Some(Cursor::Build(BuildCursor::new(guid)));
        let open = |s: &GameSession| {
            matches!(
                s.hud.toolbar.as_ref().unwrap().cursor,
                Some(Cursor::Build(_))
            )
        };
        let at = map_point(&layout, 0.3, 0.6);
        let mut modes = CountingModes::default();
        let before = session.map.camera.target;
        left_click(&mut session, &vfs, &mut modes, at, 1_000.0);
        assert!(session.map.camera.target != before, "camera jump");
        assert!(open(&session), "build cursor stays open");
        assert!(session.sim().unwrap().pending_commands().is_empty());
        right_click(&mut session, &vfs, &mut modes, at, 2_000.0);
        assert!(open(&session), "build cursor stays open");
        assert_eq!(pending_moves(&session).len(), 1, "one move command");
        assert_eq!(
            session.sim().unwrap().pending_commands().len(),
            1,
            "no building"
        );
        assert_eq!(modes.presses, 0);
    }

    /// O-394: with the software pointer the build cursor tints the pointer by the verdict of its preview, over the
    /// world as over the minimap, via the real position from `toolbar_step` (ground behind the pointer,
    /// [`GameSession::ground_at`]); over the minimap the pointer is an arrow (7). With the hardware pointer the variant
    /// stays 0.
    // QUIRK O-394: kept
    #[test]
    fn build_pointer_tint_follows_the_preview_verdict_over_the_minimap() {
        use crate::game::pointer::{VARIANT_GREEN, VARIANT_NORMAL, VARIANT_RED};
        use opennml_ui::hud::toolbar::{BuildCursor, Cursor};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut session, layout) = minimap_session(&gpu, &vfs);
        let guid = session
            .sim()
            .unwrap()
            .state
            .object(session.selection.ids[0], false)
            .unwrap()
            .guid;
        session.hud.toolbar.as_mut().unwrap().cursor = Some(Cursor::Build(BuildCursor::new(guid)));
        session.software_pointer = true;
        let mut modes = CountingModes::default();
        let tint = |valid: bool| if valid { VARIANT_GREEN } else { VARIANT_RED };
        let over_map = map_point(&layout, 0.5, 0.5);
        let mut seen = Vec::new();
        for (name, at) in [("world", [300, 300]), ("minimap", over_map)] {
            move_to(&mut session, &vfs, &mut modes, at);
            for _ in 0..2 {
                session.advance_by(&gpu, &vfs, 16.0, true, None);
            }
            assert_eq!(session.toolbar_previews.len(), 1, "{name}: one preview");
            let verdict = session.toolbar_previews[0].valid;
            let choice = session.mouse_pointer.choice;
            assert_eq!(choice.variant, tint(verdict), "{name}: tint = verdict");
            if name == "minimap" {
                assert_eq!(choice.id, opennml_formats::gifcon::POINTER_NORMAL, "arrow");
            }
            seen.push(verdict);
        }
        // Hardware pointer (`HardwareCursor 1`): no tint, also over the minimap.
        session.software_pointer = false;
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        assert_eq!(session.mouse_pointer.choice.variant, VARIANT_NORMAL);
        // The verdicts of this map: green over the world, red on the ground behind the minimap (O-394, re/game-ui
        // D5.1).
        assert_eq!(seen, [true, false], "verdicts world/minimap");
    }

    /// O-40: a right double-click in the diamond delivers 4, 4, `0x12` (`ClickClocks::release`) and thereby three
    /// commands to the same target; only the third runs.
    // QUIRK O-40: kept
    #[test]
    fn right_double_click_gives_three_commands_third_runs() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut session, layout) = minimap_session(&gpu, &vfs);
        let at = map_point(&layout, 0.15, 0.7);
        let (mx, mz) = layout.screen_to_tile([at[0] as f32, at[1] as f32]).unwrap();
        let (x, z) = ((mx * 4096.0) as i32, (mz * 4096.0) as i32);
        let mut modes = CountingModes::default();
        right_click(&mut session, &vfs, &mut modes, at, 1_000.0);
        right_click(&mut session, &vfs, &mut modes, at, 1_100.0);
        let ids = session.selection.ids.clone();
        assert_eq!(
            pending_moves(&session),
            [
                (x, z, 0, ids.clone()),
                (x, z, 0, ids.clone()),
                (x, z, 1, ids)
            ]
        );
        assert_eq!(session.sim().unwrap().pending_commands().len(), 3);
    }

    /// O-40: if the first selected object does not belong to the local player, the minimap sends nothing but still
    /// takes the right-click (the world does not see the press); also not on a double-click.
    // QUIRK O-40: kept
    #[test]
    fn foreign_first_object_sends_nothing() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut session, layout) = minimap_session(&gpu, &vfs);
        let sim = session.sim().unwrap();
        let local = usize::from(sim.active_player);
        let foreign = (0..8)
            .filter(|&p| p != local)
            .find_map(|p| sim.state.tables[p].objects.iter().flatten().next())
            .map(|o| o.id)
            .expect("foreign object");
        let own = session.selection.ids[0];
        session.selection.ids = vec![foreign, own];
        let pending = session.sim().unwrap().pending_commands().len();
        let at = map_point(&layout, 0.5, 0.5);
        let mut modes = CountingModes::default();
        move_to(&mut session, &vfs, &mut modes, at);
        mouse_button(
            &mut session,
            &vfs,
            &mut modes,
            MouseButton::Right,
            true,
            1_000.0,
        );
        assert!(
            session.pointer.right && session.right_start.is_none(),
            "press taken"
        );
        mouse_button(
            &mut session,
            &vfs,
            &mut modes,
            MouseButton::Right,
            false,
            1_000.0,
        );
        right_click(&mut session, &vfs, &mut modes, at, 1_100.0);
        assert_eq!(session.sim().unwrap().pending_commands().len(), pending);
        // Counter-check: with the own object in front the command goes out.
        session.selection.ids = vec![own, foreign];
        right_click(&mut session, &vfs, &mut modes, at, 5_000.0);
        assert_eq!(pending_moves(&session).len(), 1);
    }

    /// O-290: under the input lock with `IsMap` the minimap takes every left press and release, every right press and
    /// right double-click of the whole screen: neither the bar button nor the world (also not the right-click command,
    /// O-233) gets a click, and the minimap itself has no effect. Without `IsMap` the normal path applies: the bar
    /// takes clicks, the right-click command works.
    // QUIRK O-290: kept
    #[test]
    fn locked_minimap_swallows_all_clicks_m11() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut session, layout) = minimap_session(&gpu, &vfs);
        session.sim_mut().unwrap().state.script.mission.ui_locked = true;
        assert!(session.input_gate().minimap_swallows_clicks());
        let camera = (session.map.camera.eye, session.map.camera.target);
        let mut modes = CountingModes::default();
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let (button, msg) = clickable_button(&session);
        move_to(&mut session, &vfs, &mut modes, button);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        left_click(&mut session, &vfs, &mut modes, button, 1_000.0);
        assert_eq!(session.last_button, None, "bar button");
        left_click(&mut session, &vfs, &mut modes, [500, 300], 2_000.0);
        right_click(&mut session, &vfs, &mut modes, [500, 300], 3_000.0);
        let map = map_point(&layout, 0.5, 0.5);
        left_click(&mut session, &vfs, &mut modes, map, 4_000.0);
        right_click(&mut session, &vfs, &mut modes, map, 5_000.0);
        right_click(&mut session, &vfs, &mut modes, map, 5_100.0);
        assert_eq!((modes.presses, modes.releases), (0, 0), "world");
        assert!(pending_moves(&session).is_empty(), "no command");
        assert_eq!(
            (session.map.camera.eye, session.map.camera.target),
            camera,
            "no camera jump"
        );
        // Without `IsMap`: the bar takes the button, the right-click in the world gives a command.
        without_ismap(&mut session, &vfs);
        move_to(&mut session, &vfs, &mut modes, button);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        left_click(&mut session, &vfs, &mut modes, button, 8_000.0);
        assert_eq!(
            session.last_button,
            Some(msg),
            "without IsMap the bar takes it"
        );
        let before = session.sim().unwrap().pending_commands().len();
        right_click(&mut session, &vfs, &mut modes, [500, 300], 9_000.0);
        assert!(
            session.sim().unwrap().pending_commands().len() > before,
            "right-click command without IsMap"
        );
    }

    /// O-291: the hidden minimap (`{0x1D, 0}`, `+0x4D = 0`) keeps taking clicks with the last drawn position: left
    /// press in the invisible diamond sets the camera, the right-click sends the selection.
    // QUIRK O-291: kept
    #[test]
    fn hidden_minimap_still_takes_clicks() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut session, layout) = minimap_session(&gpu, &vfs);
        session.hud.minimap.as_mut().unwrap().set_visible(false);
        assert!(!session.hud.minimap_shown());
        let camera = session.map.camera.target;
        let mut modes = CountingModes::default();
        let at = map_point(&layout, 0.8, 0.15);
        left_click(&mut session, &vfs, &mut modes, at, 1_000.0);
        assert_ne!(session.map.camera.target, camera, "camera jump");
        assert_eq!(modes.presses, 0);
        right_click(&mut session, &vfs, &mut modes, at, 2_000.0);
        assert_eq!(pending_moves(&session).len(), 1, "move command");
    }

    /// O-457: the drawn minimap reports the rows of its diamond (terrain in the state block `Map`, depth 0) for the
    /// pointer: 128 x 128 px rotated by theta, inside the window including the overhang, the center covered, the window
    /// corner not. Hidden or in the cinemascope it does not draw: then no rectangles, even if its rectangle keeps
    /// taking clicks (O-291, O-368).
    // QUIRK O-457: kept
    #[test]
    fn minimap_depth_rects_follow_the_minimap() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let (mut session, layout) = minimap_session(&gpu, &vfs);
        let screen = screen_of((1024, 768));
        let rects = session.minimap_depth_rects(screen);
        assert!(!rects.is_empty(), "visible minimap without rectangles");
        let covers = |p: [i32; 2]| {
            rects
                .iter()
                .any(|r| (r[0]..r[2]).contains(&p[0]) && (r[1]..r[3]).contains(&p[1]))
        };
        assert!(covers([935, 679]), "center of the diamond");
        assert!(covers(map_point(&layout, 0.8, 0.15)), "point of the map");
        assert!(!covers([1024 - 176, 768 - 176]), "window corner");
        let area: i32 = rects.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
        assert!((area - 128 * 128).abs() < 400, "area {area}");
        for r in &rects {
            assert!(r[0] < r[2] && r[1] < r[3], "empty rectangle {r:?}");
            assert!(
                r[0] >= 1024 - 176 - 4 && r[1] >= 768 - 176 - 4 && r[2] <= 1024 && r[3] <= 768,
                "outside the window: {r:?}"
            );
        }
        let before = session.checksum();
        assert_eq!(session.minimap_depth_rects(screen), rects, "read-only");
        assert_eq!(session.checksum(), before);

        session.hud.minimap.as_mut().unwrap().set_visible(false);
        assert!(session.minimap_depth_rects(screen).is_empty(), "hidden");
        session.hud.minimap.as_mut().unwrap().set_visible(true);
        session.sim_mut().unwrap().state.script.mission.cinemascope = true;
        session
            .present_by_hand(&gpu, &vfs, None, &Presentation::Cinemascope { on: true })
            .unwrap();
        session.hud.minimap.as_mut().unwrap().set_visible(true);
        assert!(
            session.minimap_depth_rects(screen).is_empty(),
            "cinemascope"
        );
    }

    /// O-289: the script ping (action 7) creates location and ping with 10 repetitions; its clock runs only in frames
    /// with a drawn minimap. In the cinemascope it stands still, the first drawn frame afterwards brings all the game
    /// time at once and uses up one repetition without a ring.
    // QUIRK O-289: kept
    // QUIRK E-244: deviation
    #[test]
    fn minimap_pings_tick_only_in_drawn_frames() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.resize(1024, 768);
        let mut now = 0;
        let mut frame = |session: &mut GameSession| {
            now += 100;
            session.advance(&gpu, &vfs, now, true, None);
        };
        frame(&mut session);
        frame(&mut session);
        let ping = Presentation::MinimapPing {
            player: 1,
            tile_x: 3,
            tile_z: 4,
        };
        session.present_by_hand(&gpu, &vfs, None, &ping).unwrap();
        let first = session.hud.pings.list()[0];
        assert_eq!(
            (first.tile, first.color, first.repeats, first.t_ms),
            ((3, 4), opennml_world::markers::marker_argb(1), 10, 0)
        );
        assert_eq!(session.hud.actions.tiles().front(), Some(&(3, 4)));
        frame(&mut session);
        frame(&mut session);
        let t = session.hud.pings.list()[0].t_ms;
        assert!(t > 0 && t <= 1000, "the clock runs: {t}");
        // Cinemascope (like script action 147): 2 s game time, the ping stands still.
        session.sim_mut().unwrap().state.script.mission.cinemascope = true;
        session
            .present_by_hand(&gpu, &vfs, None, &Presentation::Cinemascope { on: true })
            .unwrap();
        let steps = session.sim().unwrap().state.steps;
        for _ in 0..30 {
            frame(&mut session);
        }
        assert!(
            session.sim().unwrap().state.steps >= steps + 22,
            "game time runs"
        );
        assert_eq!(session.hud.pings.list()[0].t_ms, t, "frozen");
        session.sim_mut().unwrap().state.script.mission.cinemascope = false;
        session
            .present_by_hand(&gpu, &vfs, None, &Presentation::Cinemascope { on: false })
            .unwrap();
        frame(&mut session);
        let p = session.hud.pings.list()[0];
        assert_eq!(
            (p.t_ms, p.repeats, p.drawn),
            (0, 9, false),
            "first frame afterwards: one repetition used up"
        );
        frame(&mut session);
        assert!(session.hud.pings.list()[0].drawn);
    }

    /// Loading and restart: bar without buttons and without cursor, no tooltip, no areas, no target point, no last
    /// button; the next frame builds the rows from the state.
    #[test]
    fn loading_resets_the_toolbar() {
        use opennml_ui::hud::toolbar::{Cursor, DefenseCursor};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let fill = |session: &mut GameSession| {
            let toolbar = session.hud.toolbar.as_mut().unwrap();
            assert!(toolbar.buttons().next().is_some(), "buttons built");
            toolbar.cursor = Some(Cursor::Defense(DefenseCursor::default()));
            session
                .hud
                .tooltip
                .set(opennml_ui::hud::toolbar::button::TooltipRequest {
                    guid: 20,
                    kind: 0x1700_0000,
                    value: 1.0,
                    required: [0; 5],
                });
            session.button_pressed(408);
            session.start_target_decal(crate::game::toolbar_host::TargetDecal {
                button: 448,
                pos: [100.0, 0.0, 100.0],
            });
        };
        let assert_reset = |session: &GameSession| {
            let toolbar = session.hud.toolbar.as_ref().unwrap();
            assert!(toolbar.buttons().next().is_none(), "no buttons");
            assert!(toolbar.cursor.is_none(), "no cursor");
            assert!(toolbar.shown);
            assert_eq!(toolbar.shown_menu(), 0);
            assert!(session.hud.tooltip.request.is_none());
            assert!(session.hud.areas.borrow().is_empty());
            assert_eq!(session.last_button, None);
            assert!(session.target_decal.is_none());
            assert!(session.toolbar_previews.is_empty());
        };
        fill(&mut session);
        session.reset_view(&gpu, &vfs);
        assert_reset(&session);
        // Saving and resuming.
        let mut fresh = toolbar_session(&gpu, &vfs);
        fill(&mut fresh);
        let host = HostPart {
            camera: fresh.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: 0,
            local_name: "Tester".into(),
            game_speed: 10,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: crate::game::control_groups::empty_save(),
            next_guid_index: crate::game::control_groups::empty_next_guid_index(),
            idle_worker_index: [0; 8],
            masks: crate::game::diplomacy::Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&fresh.save(host).unwrap().to_bytes()).unwrap();
        let resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (1024, 768),
            &mut |_| {},
        )
        .unwrap();
        assert_reset(&resumed);
    }

    /// O-237: the target point stands 300 ms of **program time**, also with the game menu open, and shrinks from 33 to
    /// 1 (`rest * 0.0033333334 * 32 + 1`); ground targets on the terrain, object targets flat.
    // QUIRK O-237: kept
    #[test]
    fn target_decal_lasts_300_ms_of_program_time() {
        use crate::game::toolbar_host::{TargetDecal, target_decal_size};
        assert_eq!(target_decal_size(1_300.0, 1_000.0), Some(33.0));
        assert_eq!(
            target_decal_size(1_300.0, 1_150.0),
            Some(150.0 * 0.003_333_333_4 * 32.0 + 1.0)
        );
        assert_eq!(target_decal_size(1_300.0, 1_300.0), None);
        // The program clock keeps running while halted, the rendering clock does not.
        let mut clock = SessionClock::default();
        clock.frame_ms(1_000, true);
        clock.frame_ms(1_200, false);
        assert_eq!((clock.program_ms, clock.now_ms), (1_200.0, 0.0));

        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        session.advance(&gpu, &vfs, 10_000, true, None);
        session.start_target_decal(TargetDecal {
            button: 448,
            pos: [1_000.0, 0.0, 1_000.0],
        });
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.map.world_decal_count(), 1, "target point drawn");
        // Game menu open: 299 ms later it still stands, at 300 ms it is gone.
        session.advance(&gpu, &vfs, 10_299, false, None);
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.map.world_decal_count(), 1);
        session.advance(&gpu, &vfs, 10_300, false, None);
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.map.world_decal_count(), 0);
        assert!(session.target_decal.is_none());
        // Without a drawn game UI (viewer without `--hud`) no target point.
        session.hud_enabled = false;
        session.start_target_decal(TargetDecal {
            button: 448,
            pos: [1_000.0, 0.0, 1_000.0],
        });
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.map.world_decal_count(), 0);
    }

    /// re/game-ui N.5.8: ground targets lie on the terrain with `y = 0`, only the object targets 529/533 at the
    /// object's position with height (flat, `0x0054BFA0`).
    // QUIRK O-237: kept
    #[test]
    fn decal_lies_on_the_ground_or_at_the_object() {
        let ground = Vec3::new(700.0, 55.0, 900.0);
        let object = Some([10.0, 120.0, 20.0]);
        assert_eq!(decal_for(448, object, ground).pos, [700.0, 0.0, 900.0]);
        assert_eq!(decal_for(427, None, ground).pos, [700.0, 0.0, 900.0]);
        assert_eq!(decal_for(533, object, ground).pos, [10.0, 120.0, 20.0]);
        assert_eq!(decal_for(529, None, ground).pos, [700.0, 0.0, 900.0]);
    }

    /// Without the game UI (viewer without `--hud`) the bar sees no mouse; every click, also in the bar rectangle, goes
    /// to the host's modes as before.
    // QUIRK O-222: kept
    #[test]
    fn viewer_without_hud_keeps_viewer_modes() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        session.hud_enabled = false;
        let mut modes = CountingModes::default();
        let (at, _) = clickable_button(&session);
        let pos = session.hud.toolbar.as_ref().unwrap().mouse_pos;
        left_click(&mut session, &vfs, &mut modes, at, 1_000.0);
        let free = free_bar_point(&session);
        left_click(&mut session, &vfs, &mut modes, free, 2_000.0);
        assert_eq!((modes.presses, modes.releases), (2, 2));
        assert_eq!(session.last_button, None);
        assert_eq!(session.hud.toolbar.as_ref().unwrap().mouse_pos, pos);
        // The bar's frame step rests too (no sounds, no preview).
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        assert!(session.toolbar_previews.is_empty());
    }

    /// Event `0x11` (bar off/on): the node in the window tree follows `Toolbar::shown`; the hidden bar takes no clicks,
    /// they go to the world.
    #[test]
    fn toolbar_visibility_follows_event_0x11() {
        use opennml_ui::hud::{HudEvent, HudEventKind};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let id = session.hud.toolbar_window().unwrap();
        let model = opennml_ui::hud::toolbar::ToolbarModel::new(
            &session.map.sim.as_ref().unwrap().rules.toolbar,
        );
        let mut out = Vec::new();
        let hide = HudEvent::with_args(HudEventKind::ToolbarOnOff, &[1]);
        session
            .hud
            .toolbar
            .as_mut()
            .unwrap()
            .event(&hide, &model, &mut out);
        drop(model);
        session.hud.sync_toolbar_visibility();
        assert!(!session.hud.windows.visible(id));
        let mut modes = CountingModes::default();
        let free = free_bar_point(&session);
        left_click(&mut session, &vfs, &mut modes, free, 1_000.0);
        assert_eq!((modes.presses, modes.releases), (1, 1), "hidden -> world");
        let show = HudEvent::with_args(HudEventKind::ToolbarOnOff, &[0]);
        let model = opennml_ui::hud::toolbar::ToolbarModel::new(
            &session.map.sim.as_ref().unwrap().rules.toolbar,
        );
        session
            .hud
            .toolbar
            .as_mut()
            .unwrap()
            .event(&show, &model, &mut out);
        drop(model);
        session.hud.sync_toolbar_visibility();
        assert!(session.hud.windows.visible(id));
    }

    /// The build cursor gets in the frame step the verdict for exactly its pose after the step, also when the pose
    /// changes; the preview shows the same pose and the same verdict.
    #[test]
    fn build_cursor_verdict_and_preview_follow_the_pose() {
        use opennml_ui::hud::toolbar::{BuildCursor, Cursor};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let guid = session
            .sim()
            .unwrap()
            .state
            .object(session.selection.ids[0], false)
            .unwrap()
            .guid;
        session.hud.toolbar.as_mut().unwrap().cursor = Some(Cursor::Build(BuildCursor::new(guid)));
        let mut modes = CountingModes::default();
        for x in [300, 340, 380] {
            move_to(&mut session, &vfs, &mut modes, [x, 300]);
            session.advance_by(&gpu, &vfs, 16.0, true, None);
            let Some(Cursor::Build(b)) = session.hud.toolbar.as_ref().unwrap().cursor.clone()
            else {
                panic!("build cursor open");
            };
            let ids = session.selection.ids.clone();
            let expected = session
                .sim_mut()
                .unwrap()
                .placement_pose_valid(b.guid, b.pos, b.rotation, &ids);
            assert_eq!(b.valid, expected, "verdict for the pose after the step");
            assert_eq!(session.toolbar_previews.len(), 1);
            let p = &session.toolbar_previews[0];
            assert_eq!(
                (p.guid, p.pos, p.rot, p.valid),
                (b.guid, b.pos, b.rotation, b.valid)
            );
        }
        // If the cursor closes, the preview goes away.
        session.hud.toolbar.as_mut().unwrap().cursor = None;
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        assert!(session.toolbar_previews.is_empty());
    }

    /// Checksums of a session: host and savegame.
    fn sums(session: &GameSession) -> (u64, u64) {
        let sim = session.sim().unwrap();
        (sim.checksum(), opennml_sim::save::checksum(&sim.state))
    }

    /// One frame in both twins: mouse to `at`, then 16 ms; afterwards the checksums must be equal.
    fn twin_frame(
        gpu: &SessionGpu,
        vfs: &Vfs,
        twins: &mut [GameSession; 2],
        at: [i32; 2],
        what: &str,
    ) {
        for s in twins.iter_mut() {
            move_to(s, vfs, &mut CountingModes::default(), at);
            s.advance_by(gpu, vfs, 16.0, true, None);
        }
        assert_eq!(sums(&twins[0]), sums(&twins[1]), "{what} at {at:?}");
    }

    /// The bar's queries borrow the simulation mutably (`active_target_buttons` via the MY filters, `cursor_frame`,
    /// `placement_pose_valid`, `wall_cursor_parts_valid`, `button_target_valid`), per input and per frame. They must
    /// change nothing: match A with game UI, twin B without (there `toolbar_step` and `toolbar_input` do not run), both
    /// with the same steps and mouse movements. Selection worker, group and HQ with occupant (via
    /// `quarter::accept_unit` in both), each with open build, wall, target and defense cursor; after every frame the
    /// same checksum and the same savegame.
    #[test]
    fn toolbar_queries_leave_the_checksum() {
        use opennml_sim::command::headhunter::NATION_HQ;
        use opennml_ui::hud::toolbar::window::WALL_GUIDS;
        use opennml_ui::hud::toolbar::{
            BuildCursor, Cursor, DefenseCursor, FortCursor, FortPhase, TargetCursor,
        };
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let start = |hud: bool| {
            let mut s = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
            s.resize(1024, 768);
            s.hud_enabled = hud;
            s
        };
        let mut twins = [start(true), start(false)];
        assert_eq!(sums(&twins[0]), sums(&twins[1]), "start");
        twin_frame(&gpu, &vfs, &mut twins, [512, 300], "first frame");

        // Selection and cursor types from the match.
        let sim = twins[0].sim().unwrap();
        let rules = &sim.rules;
        let local = sim.active_player;
        let nation = sim.state.players[usize::from(local)].nation;
        let hq_guid = NATION_HQ[usize::try_from(nation).unwrap()];
        let objects: Vec<&opennml_sim::object::GameObject> = sim.state.tables[usize::from(local)]
            .objects
            .iter()
            .flatten()
            .collect();
        let hq = objects
            .iter()
            .find(|o| o.guid == hq_guid)
            .map(|o| o.id)
            .expect("own headquarters");
        let units: Vec<(ObjectId, u32)> = objects
            .iter()
            .filter(|o| {
                o.has_module(0, false)
                    && rules
                        .toolbar
                        .menus
                        .get(&(o.guid as i32))
                        .is_some_and(|m| !m.is_empty())
            })
            .map(|o| (o.id, o.guid))
            .collect();
        assert!(units.len() >= 2, "at least two own units: {units:?}");
        let (worker, worker_guid) = units[0];
        let menu = opennml_sim::command::validate::menu_buttons(rules, worker_guid);
        let build = |wall: bool| {
            menu.iter()
                .find(|b| {
                    b.kind() == opennml_formats::gifcon::button_kind::BUILD
                        && WALL_GUIDS.contains(&(b.msg as u32)) == wall
                })
                .map(|b| b.msg as u32)
        };
        let building = build(false).expect("build button of the worker");
        let wall = build(true).unwrap_or(0x3F3);
        let occupant = units.last().unwrap().0;
        let group: Vec<ObjectId> = units.iter().map(|u| u.0).collect();

        // Mouse points: world, the selected objects on screen (target under the pointer), the bar.
        let points = |s: &GameSession, ids: &[ObjectId]| {
            let on_screen = s.map.sim_screen_points(&s.pick_params(), s.screen);
            let mut out = vec![[300, 300], [520, 260], [700, 420]];
            for id in ids.iter().chain([&hq]) {
                if let Some(p) = on_screen.get(id) {
                    let (w, h) = (s.screen.0 as f32, s.screen.1 as f32);
                    out.push([
                        p[0].clamp(0.0, w - 1.0) as i32,
                        p[1].clamp(0.0, h - 1.0) as i32,
                    ]);
                }
            }
            let [l, t, r, b] = s.hud.toolbar.as_ref().unwrap().rect();
            out.push([(l + r) / 2, (t + b) / 2]);
            out
        };

        let phase = |twins: &mut [GameSession; 2], name: &str, ids: Vec<ObjectId>| {
            for s in twins.iter_mut() {
                s.selection.ids = ids.clone();
                s.selection_changed(&vfs, None);
            }
            twin_frame(&gpu, &vfs, twins, [512, 300], name);
            twin_frame(&gpu, &vfs, twins, [512, 310], name);
            let target = {
                let sim = twins[0].sim_mut().unwrap();
                crate::game::toolbar_host::active_target_buttons(sim, &ids)
                    .into_iter()
                    .next()
                    .unwrap_or(448)
            };
            let cursors = [
                ("build", Cursor::Build(BuildCursor::new(building))),
                ("wall", Cursor::Fort(FortCursor::new(wall))),
                ("target", Cursor::Target(TargetCursor::new(target, 0))),
                ("defense", Cursor::Defense(DefenseCursor::default())),
            ];
            let mut previews = [0usize; 2];
            for (kind, cursor) in cursors {
                let what = format!("{name}, {kind} cursor");
                twins[0].hud.toolbar.as_mut().unwrap().cursor = Some(cursor);
                let pts = points(&twins[0], &ids);
                for (i, &at) in pts.iter().enumerate() {
                    twin_frame(&gpu, &vfs, twins, at, &what);
                    // Pull up the wall after the first point: many parts per frame.
                    if i == 0
                        && let Some(Cursor::Fort(f)) =
                            &mut twins[0].hud.toolbar.as_mut().unwrap().cursor
                    {
                        f.phase = FortPhase::Size;
                    }
                    match kind {
                        "build" => previews[0] = previews[0].max(twins[0].toolbar_previews.len()),
                        "wall" => previews[1] = previews[1].max(twins[0].toolbar_previews.len()),
                        _ => {}
                    }
                }
                assert!(
                    twins[0].hud.toolbar.as_ref().unwrap().cursor.is_some(),
                    "{what}: cursor stayed open"
                );
                twins[0].hud.toolbar.as_mut().unwrap().cursor = None;
                twin_frame(&gpu, &vfs, twins, [512, 300], &what);
            }
            assert!(previews[0] >= 1, "{name}: build preview came up");
            assert!(previews[1] > 1, "{name}: wall ring with several parts");
        };
        phase(&mut twins, "worker", vec![worker]);
        phase(&mut twins, "group", group);
        // An occupant via the simulation's path, the same in both twins.
        for s in twins.iter_mut() {
            let sim = s.sim_mut().unwrap();
            let rules = sim.rules.clone();
            let accepted = opennml_sim::module::quarter::accept_unit(
                &mut opennml_sim::sim::Sim::new(&mut sim.state, &rules),
                hq,
                occupant,
            );
            assert_eq!(accepted, 0, "occupant accepted");
        }
        assert_eq!(sums(&twins[0]), sums(&twins[1]), "after quartering");
        phase(&mut twins, "HQ with occupant", vec![hq]);
        assert_ne!(sums(&twins[0]).0, 0);
    }

    /// If the viewer switches off the game UI with an open build cursor (`hud {on:false}`), the bar's build preview
    /// goes away; the frame step then rests and must not leave an old preview standing.
    #[test]
    fn hud_off_drops_the_toolbar_preview() {
        use opennml_ui::hud::toolbar::{BuildCursor, Cursor};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let guid = session
            .sim()
            .unwrap()
            .state
            .object(session.selection.ids[0], false)
            .unwrap()
            .guid;
        session.hud.toolbar.as_mut().unwrap().cursor = Some(Cursor::Build(BuildCursor::new(guid)));
        move_to(
            &mut session,
            &vfs,
            &mut CountingModes::default(),
            [400, 300],
        );
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.toolbar_previews.len(), 1, "preview with UI");
        assert!(session.previews_shown);
        // UI off: without another frame (viewer stopped) and after one frame no preview anymore.
        session.hud_enabled = false;
        session.mark_ground(&gpu, &vfs, None);
        assert!(session.toolbar_previews.is_empty(), "without frame");
        assert!(!session.previews_shown, "renderer cleared");
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        session.mark_ground(&gpu, &vfs, None);
        assert!(session.toolbar_previews.is_empty(), "after a frame");
        assert!(!session.previews_shown);
    }

    // ---- Bar with script mirror, input lock and cinemascope --------------------------------------------------------

    /// Tutorial 0-1 with script (uses the conditions 35-38).
    fn tutorial_session(gpu: &SessionGpu, vfs: &Vfs) -> GameSession {
        GameSession::start(
            gpu,
            vfs,
            "data/levels/tutorial0-1/kamp0-1.lvl",
            &mut |rules: &Rules| {
                let mut setup = crate::game::sim_host::testkit::default_setup(rules)?;
                setup.script.enabled = true;
                Ok(setup)
            },
            10,
            &ViewOptions::default(),
            (320, 200),
            &mut |_| {},
        )
        .unwrap()
    }

    /// O-246, E-229: a click on a button of the command bar in a match whose script uses the conditions 35-38 goes via
    /// [`GameSession::button_pressed`] as host command `ScriptButton` with the `BUTTONMSG` to the simulation; the next
    /// tick sets the mirror's last button (`GE+0x4E0`, condition 38). Without such a script the bar sends nothing
    /// ([`SimHost::press_button`], counter-check on the multiplayer map).
    // QUIRK O-246: kept
    // QUIRK E-229: deviation
    #[test]
    fn toolbar_click_reaches_the_script_mirror_as_script_button() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = tutorial_session(&gpu, &vfs);
        assert!(session.sim().unwrap().rules.scripts.uses_ui_conditions);
        // Before the first tick (the intro turns cinemascope and lock on there): selection, bar, click.
        select_menu_object(&mut session, &gpu, &vfs);
        let mut modes = CountingModes::default();
        let (at, msg) = clickable_button(&session);
        move_to(&mut session, &vfs, &mut modes, at);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        left_click(&mut session, &vfs, &mut modes, at, 5_000.0);
        assert_eq!(session.last_button, Some(msg));
        let button = CommandKind::ScriptButton { word: msg };
        let pending = |s: &GameSession| {
            s.sim()
                .unwrap()
                .pending_commands()
                .iter()
                .filter(|c| c.kind == button)
                .count()
        };
        assert_eq!(pending(&session), 1, "exactly one ScriptButton per report");
        // The next tick executes it.
        let steps = session.steps();
        while session.steps() == steps {
            session.advance_by(&gpu, &vfs, 16.0, true, None);
        }
        let sim = session.sim().unwrap();
        assert_eq!(
            sim.state.script.mission.last_button, msg,
            "condition 38 sees the button"
        );
        assert_eq!(
            sim.log.iter().filter(|c| c.command.kind == button).count(),
            1
        );
        assert_eq!(pending(&session), 0);

        // Counter-check without script: the same click path, no command.
        let mut plain = toolbar_session(&gpu, &vfs);
        plain.advance_by(&gpu, &vfs, 16.0, true, None);
        let (at, msg) = clickable_button(&plain);
        move_to(&mut plain, &vfs, &mut modes, at);
        plain.advance_by(&gpu, &vfs, 16.0, true, None);
        left_click(&mut plain, &vfs, &mut modes, at, 5_000.0);
        assert_eq!(plain.last_button, Some(msg));
        assert!(
            plain
                .sim()
                .unwrap()
                .pending_commands()
                .iter()
                .all(|c| !matches!(c.kind, CommandKind::ScriptButton { .. }))
        );
    }

    /// The input lock (`GE+0x600`) keeps the world out (root `+0x60`, `0x00449B00`), not the bar: the windows see the
    /// mouse before the root (`FUN_004B8E70` -> `FUN_004528F0`, without checking the lock). The lock 18 sends the bar
    /// `{0x19, 1}` (`FUN_0042D920`): the open cursor closes. Applies without `IsMap`: with a minimap it intercepts the
    /// clicks before the bar under the lock (O-290, test `locked_minimap_swallows_all_clicks_m11`).
    // QUIRK E-246, O-290: kept
    #[test]
    fn ui_lock_blocks_the_world_but_not_the_toolbar() {
        use opennml_sim::script::present::Presentation;
        use opennml_ui::hud::toolbar::{Cursor, DefenseCursor};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        without_ismap(&mut session, &vfs);
        session.sim_mut().unwrap().state.script.mission.ui_locked = true;
        assert!(session.ui_locked());
        let mut modes = CountingModes::default();
        left_click(&mut session, &vfs, &mut modes, [500, 300], 1_000.0);
        assert_eq!((modes.presses, modes.releases), (0, 0), "world locked");
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let (at, msg) = clickable_button(&session);
        move_to(&mut session, &vfs, &mut modes, at);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        left_click(&mut session, &vfs, &mut modes, at, 5_000.0);
        assert_eq!(session.last_button, Some(msg), "the bar takes clicks");
        // `{0x19, 1}` closes the cursor.
        session.hud.toolbar.as_mut().unwrap().cursor =
            Some(Cursor::Defense(DefenseCursor::default()));
        let lock = Presentation::UiLock { on: true };
        let height = session.hud.screen().height;
        for e in crate::game::toolbar_host::presentation_events(&lock, session.campaign(), height) {
            session.toolbar_host.host_event(e);
        }
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        assert!(session.hud.toolbar.as_ref().unwrap().cursor.is_none());
    }

    // ---- Input via `InputGate` -------------------------------------------------------------------------------------

    /// Right-click at `at` at clock `now_ms` (movement, press, release).
    fn right_click(
        session: &mut GameSession,
        vfs: &Vfs,
        modes: &mut CountingModes,
        at: [i32; 2],
        now_ms: f64,
    ) {
        move_to(session, vfs, modes, at);
        let screen = (session.size.0, session.size.1);
        for pressed in [true, false] {
            let ev = InputEvent::Button {
                button: MouseButton::Right,
                pressed,
            };
            session.input_with(&ev, screen, now_ms, vfs, None, modes);
        }
    }

    /// O-233: nobody checks the right-click command against the lock in the original (the bar's flag `+0xCC` is never
    /// set, `0x00458510` case 4); with a selection it works as without the lock. Applies without `IsMap`: with a
    /// minimap it takes every right press under the lock (O-290, test `locked_minimap_swallows_all_clicks_m11`).
    // QUIRK E-246, O-233, O-290: kept
    #[test]
    fn right_click_command_works_under_the_lock() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut free = toolbar_session(&gpu, &vfs);
        let mut locked = toolbar_session(&gpu, &vfs);
        without_ismap(&mut free, &vfs);
        without_ismap(&mut locked, &vfs);
        locked.sim_mut().unwrap().state.script.mission.ui_locked = true;
        let gate = locked.input_gate();
        assert!(gate.ui_locked && gate.right_click_command());
        assert!(!free.selection.ids.is_empty());
        assert_eq!(locked.selection.ids, free.selection.ids);
        let mut modes = CountingModes::default();
        for session in [&mut free, &mut locked] {
            right_click(session, &vfs, &mut modes, [500, 300], 1_000.0);
        }
        let pending = |s: &GameSession| s.sim().unwrap().pending_commands().to_vec();
        assert!(!pending(&free).is_empty(), "right-click gives a command");
        assert_eq!(pending(&locked), pending(&free), "also under the lock");
    }

    /// re/campaign-ui D6.10 (`0x00449B00`, `GameEngine_UpdateCamera` `0x0043C7DC`): under the lock the world takes no
    /// left click (selection, box, the host's modes) and the wheel does not zoom ([`InputGate::world_click`],
    /// [`InputGate::player_camera`]); without the lock both.
    // QUIRK E-246: kept
    #[test]
    fn left_click_and_wheel_stay_blocked_under_the_lock() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut free = toolbar_session(&gpu, &vfs);
        let mut locked = toolbar_session(&gpu, &vfs);
        locked.sim_mut().unwrap().state.script.mission.ui_locked = true;
        let gate = locked.input_gate();
        assert!(!gate.world_click() && !gate.player_camera());
        let selection = locked.selection.ids.clone();
        let distance = locked.map.camera.distance();
        let mut counts = Vec::new();
        for session in [&mut free, &mut locked] {
            let mut modes = CountingModes::default();
            left_click(session, &vfs, &mut modes, [500, 300], 1_000.0);
            counts.push((modes.presses, modes.releases));
            let screen = (session.size.0, session.size.1);
            session.input_with(
                &InputEvent::Wheel { up: true },
                screen,
                2_000.0,
                &vfs,
                None,
                &mut modes,
            );
            for _ in 0..20 {
                session.advance_by(&gpu, &vfs, 16.0, true, None);
            }
        }
        assert_eq!(counts, [(1, 1), (0, 0)], "left click only without the lock");
        assert_eq!(locked.selection.ids, selection, "selection stays");
        assert!(locked.select_start.is_none(), "no started box");
        assert_eq!(locked.map.camera.distance(), distance, "no zoom");
        assert!(
            free.map.camera.distance() < distance,
            "wheel zooms without the lock"
        );
    }

    /// Under the lock ([`InputGate::world_click`]) neither left click, double-click, selection box nor wheel reach the
    /// world; without the lock the wheel zooms.
    #[test]
    fn lock_blocks_double_click_frame_and_wheel() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let screen = (session.size.0, session.size.1);
        let wheel = |s: &mut GameSession, modes: &mut CountingModes| {
            s.input_with(
                &InputEvent::Wheel { up: true },
                screen,
                0.0,
                &vfs,
                None,
                modes,
            );
        };
        let distance_after_frames = |s: &mut GameSession| {
            for _ in 0..20 {
                s.advance_by(&gpu, &vfs, 16.0, true, None);
            }
            s.map.camera.distance()
        };
        session.sim_mut().unwrap().state.script.mission.ui_locked = true;
        assert!(!session.input_gate().world_click());
        let mut modes = CountingModes::default();
        let selected = session.selection.ids.clone();
        left_click(&mut session, &vfs, &mut modes, [500, 300], 1_000.0);
        left_click(&mut session, &vfs, &mut modes, [500, 300], 1_050.0);
        assert_eq!(
            (modes.presses, modes.releases),
            (0, 0),
            "no click, no double-click"
        );
        // Box: press, drag, release does not start a selection box.
        let press = InputEvent::Button {
            button: MouseButton::Left,
            pressed: true,
        };
        session.input_with(&press, screen, 2_000.0, &vfs, None, &mut modes);
        assert_eq!(session.select_start, None, "no selection box");
        move_to(&mut session, &vfs, &mut modes, [600, 400]);
        let release = InputEvent::Button {
            button: MouseButton::Left,
            pressed: false,
        };
        session.input_with(&release, screen, 2_100.0, &vfs, None, &mut modes);
        assert_eq!(session.selection.ids, selected, "selection unchanged");
        move_to(&mut session, &vfs, &mut modes, [500, 300]);
        let before = distance_after_frames(&mut session);
        wheel(&mut session, &mut modes);
        assert_eq!(distance_after_frames(&mut session), before, "wheel locked");
        // Counter-check without the lock: the wheel zooms.
        session.sim_mut().unwrap().state.script.mission.ui_locked = false;
        wheel(&mut session, &mut modes);
        assert_ne!(distance_after_frames(&mut session), before, "wheel works");
    }

    /// Cinemascope 147 sends `{0x11, 1}` to the bar (`FUN_00430920`): it is hidden and takes no clicks; after
    /// restarting the view and after loading in the cinemascope it stays hidden. The `{8}` afterwards is not taken by
    /// the hidden bar (as in the original, `FUN_0047A130` clears the selection without its own `{8}`). Without `IsMap`:
    /// the intro also locks the input; with a minimap it would take the press (O-290).
    // QUIRK E-238, O-290: kept
    #[test]
    fn cinemascope_hides_the_toolbar_also_after_loading() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = tutorial_session(&gpu, &vfs);
        select_menu_object(&mut session, &gpu, &vfs);
        without_ismap(&mut session, &vfs);
        let id = session.hud.toolbar_window().unwrap();
        assert!(session.hud.windows.visible(id));
        let mut frames = 0;
        while !session.presentation.cinemascope {
            session.advance_by(&gpu, &vfs, 50.0, true, None);
            frames += 1;
            assert!(frames < 200, "cinemascope arrives");
        }
        // The identifier per session: the loaded tree has the minimap again (`IsMap` of the default).
        let hidden = |s: &GameSession| {
            let toolbar = s.hud.toolbar.as_ref().unwrap();
            !toolbar.shown && !s.hud.windows.visible(s.hud.toolbar_window().unwrap())
        };
        assert!(session.selection.ids.is_empty());
        assert!(hidden(&session), "hidden in the cinemascope frame");
        let free = free_bar_point(&session);
        move_to(&mut session, &vfs, &mut CountingModes::default(), free);
        let press = InputEvent::Button {
            button: MouseButton::Left,
            pressed: true,
        };
        assert!(
            !session.toolbar_input(&press, false, &vfs, None),
            "no clicks"
        );
        // Restart of the view and loading: fresh bar, hidden again after one frame.
        session.reset_view(&gpu, &vfs);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        assert!(hidden(&session), "after reset_view");
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: -1,
            local_name: String::new(),
            game_speed: session.sim().unwrap().game_speed,
            cinemascope_speed: session.cinemascope_speed(),
            script_camera: session.presentation.camera.clone(),
            control_groups: crate::game::control_groups::empty_save(),
            next_guid_index: crate::game::control_groups::empty_next_guid_index(),
            idle_worker_index: [0; 8],
            masks: crate::game::diplomacy::Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let mut resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (1024, 768),
            &mut |_| {},
        )
        .unwrap();
        assert!(resumed.presentation.cinemascope);
        resumed.advance_by(&gpu, &vfs, 16.0, true, None);
        assert!(hidden(&resumed), "after loading");
    }

    /// `advance_by` reports the host's selection to the simulation's mirror before the step, but only for scripts with
    /// conditions 35-38 (tutorial 0-1) and only on change; on a map without them the command log stays free of it.
    // QUIRK E-230: deviation
    #[test]
    fn session_advance_syncs_the_selection_only_for_ui_scripts() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let start = |level: &str, script: bool| {
            GameSession::start(
                &gpu,
                &vfs,
                level,
                &mut |rules: &Rules| {
                    let mut setup = crate::game::sim_host::testkit::default_setup(rules)?;
                    setup.script.enabled = script;
                    Ok(setup)
                },
                10,
                &ViewOptions::default(),
                (320, 200),
                &mut |_| {},
            )
            .unwrap()
        };
        let selections = |s: &GameSession| {
            s.sim()
                .unwrap()
                .pending_commands()
                .iter()
                .filter(|c| matches!(c.kind, CommandKind::ScriptSelection { .. }))
                .count()
        };
        let mut tutorial = start("data/levels/tutorial0-1/kamp0-1.lvl", true);
        assert!(tutorial.sim().unwrap().rules.scripts.uses_ui_conditions);
        let ids: Vec<ObjectId> = tutorial
            .sim()
            .unwrap()
            .state
            .objects()
            .take(2)
            .map(|o| o.id)
            .collect();
        tutorial.selection.ids = ids.clone();
        assert_eq!(tutorial.hover_shown(Some(ids[0])), Some(ids[0]));
        tutorial.advance(&gpu, &vfs, 1_000, true, None);
        // Frame time 0 in the first frame: no tick, the command waits.
        assert_eq!(selections(&tutorial), 1);
        // Further frames with the same selection send nothing more; the next tick executes the command (the mission's
        // intro turns the cinemascope on in the same tick and empties the mirror again, the host clears its selection
        // with it).
        tutorial.advance(&gpu, &vfs, 1_000, true, None);
        assert_eq!(selections(&tutorial), 1);
        tutorial.advance(&gpu, &vfs, 1_200, true, None);
        assert_eq!(selections(&tutorial), 0);
        assert!(tutorial.presentation.cinemascope);
        assert_eq!(
            tutorial.hover_shown(Some(ids[0])),
            None,
            "in the cinemascope neither hover ring nor hover bar (`0x0047A490`)"
        );
        assert!(
            tutorial.selection.ids.is_empty(),
            "147 also clears the host's selection"
        );
        tutorial.advance(&gpu, &vfs, 1_300, true, None);
        assert_eq!(
            selections(&tutorial),
            0,
            "the empty selection matches the mirror"
        );
        let logged = tutorial
            .sim()
            .unwrap()
            .log
            .iter()
            .filter(|c| matches!(&c.command.kind, CommandKind::ScriptSelection { ids: sent } if *sent == ids))
            .count();
        assert_eq!(logged, 1);

        // A map without script: never a command.
        let mut plain = start(TWO_PLAYERS, false);
        let ids: Vec<ObjectId> = plain
            .sim()
            .unwrap()
            .state
            .objects()
            .take(2)
            .map(|o| o.id)
            .collect();
        plain.selection.ids = ids;
        for now in [1_000, 1_100, 1_300] {
            plain.advance(&gpu, &vfs, now, true, None);
        }
        assert!(plain.sim().unwrap().log.iter().all(|c| !matches!(
            c.command.kind,
            CommandKind::ScriptSelection { .. } | CommandKind::ScriptButton { .. }
        )));
    }

    // ---- Cinemascope area in the host ----------------------------------------------------------------------------

    /// A presentation event without script as in [`GameSession::advance_by`]: the host part
    /// ([`ScriptPresentation::apply_host`]: speed, clearing the selection), scene fade and target highlight
    /// ([`scene_event`]), the info windows ([`info_frame_event`]), the UI messages ([`Hud::present`]), then `{8}` for a
    /// changed selection.
    fn present(session: &mut GameSession, event: &opennml_sim::script::present::Presentation) {
        let before = session.selection.ids.clone();
        let sim = session.map.sim.as_mut().unwrap();
        session
            .presentation
            .apply_host(event, sim, &mut session.selection);
        scene_event(
            &mut session.scene_fade,
            &mut session.target_highlight,
            event,
        );
        info_frame_event(&mut session.info_frames, event);
        screen_text_event(&mut session.hud.screen_texts, event);
        let (campaign, height) = (session.campaign(), session.hud.screen().height);
        session.hud.present(
            crate::game::toolbar_host::presentation_events(event, campaign, height),
            &mut session.toolbar_host,
        );
        if session.selection.ids != before {
            session.toolbar_host.selection_changed();
        }
    }

    /// O-350 (`FUN_00430920`, `0x004309BE`): the tutorial's cinemascope sets speed 10 and turns the command bar and the
    /// status bar off in every game type; bars (`BARSIZE`) only for game type -1.
    // QUIRK O-350: kept
    #[test]
    fn cinemascope_bars_only_in_the_campaign() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        for game_type in [-1, 0] {
            let mut session = GameSession::start(
                &gpu,
                &vfs,
                "data/levels/tutorial0-1/kamp0-1.lvl",
                &mut |rules: &Rules| {
                    let mut setup = crate::game::sim_host::testkit::default_setup(rules)?;
                    setup.script.enabled = true;
                    Ok(setup)
                },
                20,
                &ViewOptions::default(),
                (1024, 768),
                &mut |_| {},
            )
            .unwrap();
            session.game_type = game_type;
            assert_eq!(session.campaign(), game_type == -1);
            let bar_size = session.hud.cinemascope_data.ini.bar_size;
            assert!(bar_size > 0, "BARSIZE of the installation");
            let mut frames = 0;
            while !session.presentation.cinemascope {
                session.advance_by(&gpu, &vfs, 50.0, true, None);
                frames += 1;
                assert!(frames < 200, "cinemascope arrives");
            }
            assert_eq!(session.sim().unwrap().game_speed, 10, "speed 10");
            assert!(!session.hud.toolbar.as_ref().unwrap().shown, "bar off");
            assert!(
                !session.hud.status.as_ref().unwrap().shown,
                "status bar off"
            );
            let expected = if game_type == -1 { bar_size } else { 0 };
            assert_eq!(
                session.hud.cinemascope.bar, expected,
                "game type {game_type}"
            );
        }
    }

    /// O-217: in the cinemascope messages expire invisibly (and silently); the host sets the flag each frame from the
    /// scripts' cinemascope, already for the messages of the frame in which it begins.
    // QUIRK O-217: kept
    #[test]
    fn messages_are_hidden_in_cinemascope() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = tutorial_session(&gpu, &vfs);
        let texts = Texts::load(
            &Vfs::from_archives(Vec::new()),
            &opennml_formats::localisation::Localisation::default(),
        );
        let active = session.sim().unwrap().active_player;
        let message = SimEvent::PlayerMessage {
            to: Some(active),
            id: 0,
            tile: (-1, -1),
            flags: 0x0301_0000,
        };
        session
            .hud
            .handle(std::slice::from_ref(&message), active, &mut Vec::new());
        assert!(!session.hud.messages.cinemascope);
        assert!(
            !session.message_lines(&texts).is_empty(),
            "before the cinemascope"
        );
        let mut frames = 0;
        while !session.presentation.cinemascope {
            session.advance_by(&gpu, &vfs, 50.0, true, None);
            frames += 1;
            assert!(frames < 200, "cinemascope arrives");
        }
        assert!(session.hud.messages.cinemascope);
        session.hud.handle(&[message], active, &mut Vec::new());
        assert!(
            session.hud.messages.heads().next().is_some(),
            "the message runs"
        );
        assert!(session.message_lines(&texts).is_empty(), "but invisible");
    }

    /// O-354: the portrait fade runs on the program clock, also with the game menu open (without a step); `advance_by`
    /// does not compute it, a backward jump of the clock counts 0.
    // QUIRK O-354: kept
    #[test]
    fn portrait_fade_runs_while_the_game_menu_is_open() {
        use opennml_sim::script::present::Presentation;
        use opennml_ui::hud::cinemascope::state;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.resize(1024, 768);
        present(&mut session, &Presentation::Cinemascope { on: true });
        present(
            &mut session,
            &Presentation::Portrait {
                slot: 0,
                portrait: 1,
                frame: 1,
            },
        );
        let fade = |s: &GameSession| s.hud.cinemascope.portraits[0].fade;
        session.advance(&gpu, &vfs, 1_000, false, None);
        let steps = session.steps();
        assert_eq!(fade(&session).alpha_u8(), 0, "first frame: 0 ms");
        session.advance(&gpu, &vfs, 1_250, false, None);
        assert_eq!(fade(&session).alpha_u8(), 127, "250 * 0.51");
        session.advance(&gpu, &vfs, 1_500, false, None);
        assert_eq!(
            (fade(&session).alpha_u8(), fade(&session).state),
            (255, state::VISIBLE)
        );
        assert_eq!(session.steps(), steps, "game menu open: no step");
        // `advance_by` (viewer, tests) does not compute the fades; a backward jump of the clock counts 0.
        present(&mut session, &Presentation::PortraitHide { slot: 0 });
        session.advance_by(&gpu, &vfs, 250.0, true, None);
        assert_eq!(fade(&session).alpha_u8(), 255);
        session.advance(&gpu, &vfs, 1_400, false, None);
        assert_eq!(fade(&session).alpha_u8(), 255, "backward jump");
        session.advance(&gpu, &vfs, 1_650, true, None);
        assert_eq!(fade(&session).alpha_u8(), 127, "also with a running match");
        session.advance(&gpu, &vfs, 1_900, false, None);
        assert_eq!(fade(&session).state, 0, "faded out");
    }

    /// After loading, bars (campaign only), portraits, the running line and the tips stand there at once in full,
    /// without fade; the status bar and the messages stay hidden in the cinemascope.
    // QUIRK O-217, O-355, O-356: kept
    // QUIRK E-239: deviation
    #[test]
    fn loading_restores_bars_portraits_line_and_tips_without_fade() {
        use opennml_sim::script::present::PortraitMemory;
        use opennml_sim::script::speech::SpeechLine;
        use opennml_ui::hud::cinemascope::Fade;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        // With a level script as in the game; saved before the first tick.
        let mut session = tutorial_session(&gpu, &vfs);
        let mission = &mut session.sim_mut().unwrap().state.script.mission;
        mission.cinemascope = true;
        mission.presentation.portraits[1] = Some(PortraitMemory {
            portrait: 7,
            frame: 3,
        });
        mission.speech.line = Some(SpeechLine {
            text: 500,
            remaining_ms: 2_000,
            slot: 0,
        });
        mission.speech.tip_ms = [0, 1_500];
        mission.speech.tip_text = [0, 600];
        let resume = |game_type: i32| {
            let host = HostPart {
                camera: session.camera(),
                level_info: opennml_ui::draft::LevelInfoData::default(),
                game_type,
                local_name: String::new(),
                game_speed: 10,
                cinemascope_speed: Some(14),
                script_camera: Default::default(),
                control_groups: crate::game::control_groups::empty_save(),
                next_guid_index: crate::game::control_groups::empty_next_guid_index(),
                idle_worker_index: [0; 8],
                masks: crate::game::diplomacy::Masks::initial(),
                lock_teams: false,
                fog_show_mode: 0,
                net: None,
                log: None,
                save_replay: false,
            };
            let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
            GameSession::resume_from(
                &gpu,
                &vfs,
                file,
                &ViewOptions::default(),
                (1024, 768),
                &mut |_| {},
            )
            .unwrap()
        };
        let resumed = resume(-1);
        let view = &resumed.hud.cinemascope;
        assert_eq!(view.bar, resumed.hud.cinemascope_data.ini.bar_size);
        assert!(view.bar > 0);
        let p = view.portraits[1];
        assert_eq!((p.portrait, p.frame, p.fade), (7, 3, Fade::shown()));
        assert_eq!(view.portraits[0].fade.state, 0);
        assert_eq!(
            (view.texts[0].text, view.texts[0].fade),
            (500, Fade::shown())
        );
        assert_eq!(
            (view.texts[3].text, view.texts[3].fade),
            (600, Fade::shown())
        );
        assert_eq!((view.texts[1].fade.state, view.texts[2].fade.state), (0, 0));
        assert!(!resumed.hud.status.as_ref().unwrap().shown);
        assert!(resumed.hud.messages.cinemascope);
        // Free play: the same slots, but no bars (the portraits then do not draw, O-354).
        let free = resume(0);
        assert_eq!(free.hud.cinemascope.bar, 0);
        assert_eq!(free.hud.cinemascope.portraits[1].fade, Fade::shown());
        assert_eq!(free.hud.cinemascope.texts[3].fade, Fade::shown());
        // Without cinemascope in the state: status bar and messages visible, no bars (restart of the view).
        let mut plain = resume(-1);
        plain.sim_mut().unwrap().state.script.mission.cinemascope = false;
        plain.reset_view(&gpu, &vfs);
        assert_eq!(plain.hud.cinemascope.bar, 0);
        assert!(plain.hud.status.as_ref().unwrap().shown);
        assert!(!plain.hud.messages.cinemascope);
        assert_eq!(plain.hud.cinemascope.texts[0].fade, Fade::shown());
    }

    /// O-234: the cinemascope's `{8}` comes after `{0x11, 1}` and is lost; hidden, the bar does not rebuild even with a
    /// changed signature (here a button lock). After the cinemascope the buttons from before stand.
    // QUIRK O-234: kept
    #[test]
    fn toolbar_keeps_the_old_buttons_after_cinemascope() {
        use opennml_sim::script::present::Presentation;
        use opennml_ui::hud::HudEventKind;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let buttons = |s: &GameSession| -> Vec<i32> {
            s.hud
                .toolbar
                .as_ref()
                .unwrap()
                .buttons()
                .map(|b| b.spec.msg)
                .collect()
        };
        let before = buttons(&session);
        assert!(!before.is_empty());
        present(&mut session, &Presentation::Cinemascope { on: true });
        assert!(session.selection.ids.is_empty(), "147 clears the selection");
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        assert!(!session.hud.toolbar.as_ref().unwrap().shown);
        assert_eq!(buttons(&session), before, "{{8}} discarded");
        // In the cinemascope the signature changes (button lock 207): no `{0x10, n}`.
        session
            .sim_mut()
            .unwrap()
            .state
            .button_locks
            .insert(0x7FFF, 1);
        for _ in 0..2 {
            session.advance_by(&gpu, &vfs, 16.0, true, None);
            assert!(
                session
                    .toolbar_host
                    .frame_events
                    .iter()
                    .all(|e| e.kind != HudEventKind::Rebuild)
            );
        }
        present(&mut session, &Presentation::Cinemascope { on: false });
        for _ in 0..2 {
            session.advance_by(&gpu, &vfs, 16.0, true, None);
        }
        assert!(session.hud.toolbar.as_ref().unwrap().shown);
        assert_eq!(buttons(&session), before, "old buttons");
    }

    /// Small multiplayer map with two slots.
    const TWO_PLAYERS: &str = "data/levels/multiplayer/[2players]-tintebariver.lvl";

    /// Original installation and graphics card (silently skipped without either).
    fn corpus_gpu() -> Option<(Vfs, wgpu::Device, wgpu::Queue)> {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return None;
        };
        let Ok((device, queue, _)) =
            opennml_render::headless_device(opennml_render::BackendChoice::Auto)
        else {
            eprintln!("no GPU, test skipped");
            return None;
        };
        Some((
            opennml_rda::Vfs::open_game_dir(&dir).unwrap(),
            device,
            queue,
        ))
    }

    fn start_two_players(
        gpu: &SessionGpu,
        vfs: &Vfs,
        phases: &mut Vec<u8>,
    ) -> anyhow::Result<GameSession> {
        GameSession::start(
            gpu,
            vfs,
            TWO_PLAYERS,
            &mut crate::game::sim_host::testkit::default_setup,
            10,
            &ViewOptions::default(),
            (320, 200),
            &mut |phase| phases.push(phase.percent()),
        )
    }

    #[test]
    fn session_starts_a_multiplayer_map() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let mut phases = Vec::new();
        let mut session = start_two_players(&gpu, &vfs, &mut phases).unwrap();
        assert_eq!(phases, [0, 40, 70, 100]);
        assert_eq!(session.level(), TWO_PLAYERS);
        assert_eq!(session.sim().unwrap().setup.lobby.len(), 2);
        // The same match directly via `SimHost`.
        let scene = opennml_world::level::load_level(&vfs, TWO_PLAYERS).unwrap();
        let mut direct = crate::game::sim_host::start_sim(
            &vfs,
            &scene,
            "DEU",
            10,
            crate::game::sim_host::testkit::default_setup,
        )
        .unwrap()
        .unwrap();
        assert_eq!((session.size, session.screen), ((320, 200), (320, 200)));
        for frame in 1..=20u64 {
            session.advance(&gpu, &vfs, frame * 50, true, None);
        }
        // The first frame has frame time 0: 19 ticks of 50 ms each, then one more.
        assert_eq!(session.steps(), 19);
        session.advance_by(&gpu, &vfs, 50.0, true, None);
        for _ in 0..20 {
            direct.step_once();
        }
        assert_eq!(session.steps(), 20);
        assert_eq!(session.checksum(), direct.checksum());
        assert_ne!(session.checksum(), 0);
    }

    /// Rally flag in viewer and `opennml-game` (D-257, D-258, D-261): with a single selection of the own HQ
    /// [`GameSession::prepare`] sets exactly one world model; the cloth waves with the effect world's clock, so it
    /// stands still in pause and game menu (20 frames of the same animation time) and keeps running afterwards. Without
    /// a selection no flag.
    // QUIRK D-257, D-261: kept
    // QUIRK D-258: deviation
    #[test]
    fn flag_cloth_stands_still_in_pause_and_game_menu() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        let sim = session.sim().unwrap();
        let hq = sim
            .state
            .objects()
            .find(|o| {
                o.owner() == sim.active_player
                    && crate::game::rally_flag::has_flag(&sim.rules, o.guid)
            })
            .map(|o| o.id)
            .expect("own building with a flag");
        let mut now = 0;
        let mut frame = |session: &mut GameSession, running: bool| {
            now += 50;
            session.advance(&gpu, &vfs, now, running, None);
            session.prepare(&gpu, &vfs, (320, 200));
            assert!(session.map.renderer.world_model_count() <= 1);
            session
                .map
                .rally_flag_draw()
                .and_then(|d| d.animation)
                .map(|(_, ms)| ms)
        };
        assert_eq!(
            frame(&mut session, true),
            None,
            "no flag without a selection"
        );
        session.selection.ids = vec![hq];
        let t0 = frame(&mut session, true).expect("flag at the HQ");
        assert_eq!(session.map.rally_flag().map(|f| f.building), Some(hq));
        assert_eq!(
            session.map.renderer.world_model_count(),
            1,
            "once per frame"
        );
        let t1 = frame(&mut session, true).unwrap();
        assert!(t1 > t0, "the cloth waves: {t0} -> {t1}");
        assert!(session.toggle_pause());
        let paused = frame(&mut session, true).unwrap();
        for _ in 0..20 {
            assert_eq!(frame(&mut session, true), Some(paused), "pause");
        }
        assert!(session.toggle_pause());
        let menu = frame(&mut session, false).unwrap();
        for _ in 0..20 {
            assert_eq!(frame(&mut session, false), Some(menu), "game menu");
        }
        frame(&mut session, true);
        assert!(
            frame(&mut session, true).unwrap() > menu,
            "keeps running afterwards"
        );
        session.selection.ids.clear();
        assert_eq!(frame(&mut session, true), None);
        assert_eq!(session.map.renderer.world_model_count(), 0);
    }

    /// Saving and resuming: tick, checksum and camera as when saving; a savegame with a foreign rules checksum is
    /// rejected on resuming.
    #[test]
    fn resumed_session_continues_the_saved_match() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        assert_eq!((session.size, session.screen), ((320, 200), (320, 200)));
        for _ in 0..10 {
            session.advance_by(&gpu, &vfs, 50.0, true, None);
        }
        let terrain = &session.map.scene.terrain;
        session.map.camera.scroll(40.0, -25.0, terrain);
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: 0,
            local_name: "Tester".into(),
            game_speed: 12,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: crate::game::control_groups::empty_save(),
            next_guid_index: crate::game::control_groups::empty_next_guid_index(),
            idle_worker_index: [0; 8],
            masks: crate::game::diplomacy::Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let mut phases = Vec::new();
        let resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file.clone(),
            &ViewOptions::default(),
            (320, 200),
            &mut |phase| phases.push(phase.percent()),
        )
        .unwrap();
        assert_eq!(phases, [0, 40, 70, 100]);
        assert_eq!(resumed.level(), TWO_PLAYERS);
        assert_eq!(
            (resumed.steps(), resumed.checksum()),
            (session.steps(), session.checksum())
        );
        assert_eq!(resumed.camera(), session.camera());
        assert_eq!(resumed.sim().unwrap().game_speed, 12);
        let mut broken = file;
        broken.sim.start.rules.logic ^= 1;
        let error = GameSession::resume_from(
            &gpu,
            &vfs,
            broken,
            &ViewOptions::default(),
            (320, 200),
            &mut |_| {},
        )
        .err()
        .expect("rejected");
        assert!(format!("{error:#}").contains("rules differ"), "{error:#}");
    }

    /// Group commands via `run_hotkey` on the groups of the active slot in the whole session; the first press selects,
    /// the second moves the camera with the current offset to the mean; clearing the selection (also in the frame)
    /// resets the last group; the shift path works via the table of held keys; none of it changes checksum or command
    /// log. Saving and loading brings back the groups of all slots, with an empty selection and without a last group.
    // QUIRK O-43, O-310: kept
    // QUIRK O-312: deviation
    #[test]
    fn session_runs_control_groups_and_restores_them() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        // A few ticks: the fog of the active slot is built (fog check when selecting).
        for _ in 0..10 {
            session.advance_by(&gpu, &vfs, 50.0, true, None);
        }
        let (local, units) = {
            let sim = session.sim().unwrap();
            let local = sim.active_player;
            let units: Vec<_> = sim.state.tables[usize::from(local)]
                .objects
                .iter()
                .flatten()
                .filter(|o| o.has_module(0, true) && sim.fog_selectable(o))
                .map(|o| o.id)
                .take(3)
                .collect();
            (usize::from(local), units)
        };
        assert_eq!(units.len(), 3, "three own units");
        let before = (session.checksum(), session.sim().unwrap().log.len());
        let (a, b, c) = (units[0], units[1], units[2]);
        session.selection.clear();
        {
            let sim = session.map.sim.as_ref().unwrap();
            session.selection.select_list(
                &sim.state,
                &sim.rules,
                |o| sim.fog_selectable(o),
                &[a, b],
            );
        }
        session.sync_groups();
        assert!(session.run_hotkey(&HotkeyCommand::StoreGroup(1), 0, &vfs, None));
        assert_eq!(session.groups[local].lists[1], [a, b]);
        assert_eq!(session.groups[local].last, 1);
        // Second press: camera to the mean of the first cluster, offset stays, selection stays.
        assert!(session.map.camera.yaw(0.4));
        let offset = session.map.camera.eye - session.map.camera.target;
        let mean = {
            let state = &session.sim().unwrap().state;
            let first = &session.clusters.clusters()[0];
            assert_eq!(first[0], a);
            let sum: Vec3 = first
                .iter()
                .map(|&id| Vec3::from(state.object(id, false).unwrap().world_pos))
                .sum();
            sum / first.len() as f32
        };
        assert!(session.run_hotkey(&HotkeyCommand::SelectGroup(1), 0, &vfs, None));
        let camera = &session.map.camera;
        assert!((camera.target.x - mean.x).abs() < 1e-2 && (camera.target.z - mean.z).abs() < 1e-2);
        assert!((camera.eye - camera.target - offset).length() < 1e-1);
        assert_eq!(session.selection.ids, [a, b]);
        // Clearing in the frame (viewer, script): the next 1 selects anew, without camera.
        session.selection.clear();
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        assert_eq!(session.groups[local].last, -1);
        let eye = session.map.camera.eye;
        session.run_hotkey(&HotkeyCommand::SelectGroup(1), 0, &vfs, None);
        assert_eq!(session.selection.ids, [a, b]);
        assert_eq!(session.map.camera.eye, eye, "first press without camera");
        // Shift path (O-310): `KD_ADD_TO_GROUP` (LSHIFT) held, C is added and goes into G1.
        session.held_vk = vec![0xA0];
        {
            let sim = session.map.sim.as_ref().unwrap();
            session
                .selection
                .select_list(&sim.state, &sim.rules, |o| sim.fog_selectable(o), &[c]);
        }
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        session.held_vk.clear();
        assert_eq!(session.groups[local].lists[1], [a, b, c]);
        session.run_hotkey(&HotkeyCommand::AddToGroup(2), 0, &vfs, None);
        assert_eq!(
            (
                session.groups[local].lists[1].len(),
                session.groups[local].last
            ),
            (0, -1)
        );
        session.run_hotkey(&HotkeyCommand::StoreGroup(4), 0, &vfs, None);
        assert_eq!(
            (session.checksum(), session.sim().unwrap().log.len()),
            before,
            "no effect on the checksum, no command"
        );
        // Saving and loading: all slots, selection empty, last group -1.
        session.groups[3].lists[0] = vec![ObjectId::new(7, 3, 1)];
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: 0,
            local_name: "Tester".into(),
            game_speed: 10,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: session.control_groups_save(),
            next_guid_index: session.next_guid_index_save(),
            idle_worker_index: session.idle_worker_index,
            masks: session.masks,
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let mut resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (320, 200),
            &mut |_| {},
        )
        .unwrap();
        for slot in 0..PLAYER_SLOTS {
            assert_eq!(
                resumed.groups[slot].lists, session.groups[slot].lists,
                "slot {slot}"
            );
            assert_eq!(resumed.groups[slot].last, -1);
        }
        // `storegroup(4)` took the three from G2 (`0x004FB0A0`).
        assert!(resumed.groups[local].lists[2].is_empty());
        assert_eq!(resumed.groups[local].lists[4], [a, b, c]);
        assert!(resumed.selection.ids.is_empty());
        // The savegame carries the fog grids (`Fog` in `SimState`): the group is selectable right after loading.
        let eye = resumed.map.camera.eye;
        resumed.run_hotkey(&HotkeyCommand::SelectGroup(4), 0, &vfs, None);
        assert_eq!(resumed.selection.ids, [a, b, c]);
        assert_eq!(resumed.map.camera.eye, eye);
    }

    /// `selectnextguid` via `run_hotkey` at program clock selects without a camera, the double-tap within 300 ms moves
    /// the game camera with the current offset to the selected object; no effect on the checksum, no command. The index
    /// is in the host part and continues after loading.
    // QUIRK O-314: kept
    // QUIRK O-333: deviation
    #[test]
    fn session_selects_next_guid_and_restores_the_index() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        for _ in 0..10 {
            session.advance_by(&gpu, &vfs, 50.0, true, None);
        }
        // Two own units of the same GUID in table order.
        let (local, guid, first, second) = {
            let sim = session.sim().unwrap();
            let local = sim.active_player;
            let units: Vec<_> = sim.state.tables[usize::from(local)]
                .objects
                .iter()
                .flatten()
                .filter(|o| o.has_module(0, true) && sim.fog_selectable(o))
                .collect();
            let pair = units
                .iter()
                .find_map(|a| {
                    units
                        .iter()
                        .find(|b| b.guid == a.guid && b.id != a.id)
                        .map(|b| (a.guid as i32, a.id, b.id))
                })
                .expect("two own units of the same GUID");
            (usize::from(local), pair.0, pair.1, pair.2)
        };
        let before = (session.checksum(), session.sim().unwrap().log.len());
        let cmd = HotkeyCommand::SelectNextGuid(guid);
        let eye = session.map.camera.eye;
        assert!(session.run_hotkey(&cmd, 1_000, &vfs, None));
        assert_eq!(session.selection.ids, [first]);
        assert_eq!(session.map.camera.eye, eye, "first press without camera");
        // Double-tap: camera to the selected object, offset stays, selection stays.
        assert!(session.map.camera.yaw(0.4));
        let offset = session.map.camera.eye - session.map.camera.target;
        assert!(session.run_hotkey(&cmd, 1_200, &vfs, None));
        let at = Vec3::from(
            session
                .sim()
                .unwrap()
                .state
                .object(first, false)
                .unwrap()
                .world_pos,
        );
        let camera = &session.map.camera;
        assert!((camera.target.x - at.x).abs() < 1e-2 && (camera.target.z - at.z).abs() < 1e-2);
        assert!((camera.eye - camera.target - offset).length() < 1e-1);
        assert_eq!(session.selection.ids, [first]);
        assert_eq!(
            (session.checksum(), session.sim().unwrap().log.len()),
            before,
            "no effect on the checksum, no command"
        );
        let index = session.groups[local].next_guid_index[&guid];
        assert_eq!(index, first.index() as u32 + 1);
        // Saving and loading: the index of all slots comes back, the next press selects the second unit.
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: 0,
            local_name: "Tester".into(),
            game_speed: 10,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: session.control_groups_save(),
            next_guid_index: session.next_guid_index_save(),
            idle_worker_index: session.idle_worker_index,
            masks: session.masks,
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let mut resumed = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (320, 200),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(resumed.groups[local].next_guid_index[&guid], index);
        assert!(resumed.run_hotkey(&cmd, 1_300, &vfs, None));
        assert_eq!(resumed.selection.ids, [second]);
    }

    /// `opennml-game` saves in the middle of a camera move of the scripts with binding (tutorial 0-1 in the cinemascope);
    /// `resume_from` restores game camera and script camera, and both matches keep running alike (camera and checksum).
    /// The savegame's selection mirror is not empty, the host's selection after loading is: the first step reports the
    /// empty selection (`ScriptSelection []`).
    // QUIRK E-239: deviation
    #[test]
    fn resumed_session_restores_the_script_camera_and_reports_the_empty_selection() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format: opennml_render::CAPTURE_FORMAT,
        };
        let view = ViewOptions::default();
        let mut a = GameSession::start(
            &gpu,
            &vfs,
            "data/levels/tutorial0-1/kamp0-1.lvl",
            &mut |rules: &Rules| {
                let mut setup = crate::game::sim_host::testkit::default_setup(rules)?;
                setup.script.enabled = true;
                Ok(setup)
            },
            10,
            &view,
            (320, 200),
            &mut |_| {},
        )
        .unwrap();
        // The intro turns the cinemascope on (and clears mirror and selection).
        let mut now = 1_000;
        while !a.presentation.cinemascope {
            a.advance(&gpu, &vfs, now, true, None);
            now += 50;
            assert!(now < 10_000, "cinemascope arrives");
        }
        // In the cinemascope the player selects; the next step fills the mirror.
        let ids: Vec<ObjectId> = a
            .sim()
            .unwrap()
            .state
            .objects()
            .take(2)
            .map(|o| o.id)
            .collect();
        a.selection.ids = ids.clone();
        for _ in 0..2 {
            a.advance(&gpu, &vfs, now, true, None);
            now += 50;
        }
        assert!(!a.sim().unwrap().state.script.mission.selection.is_empty());
        // A move with binding runs.
        let events = [
            Presentation::CameraBind { id: Some(ids[0]) },
            Presentation::CameraToGame { ms: 3_000 },
            Presentation::CameraRotate {
                degrees: 60,
                ms: 2_000,
            },
        ];
        {
            let map = &mut a.map;
            let sim = map.sim.as_ref().unwrap();
            for event in &events {
                a.presentation.camera.apply(
                    event,
                    &mut map.camera,
                    &map.scene.world.cameras,
                    &map.scene.terrain,
                    sim,
                );
            }
        }
        for _ in 0..3 {
            a.advance(&gpu, &vfs, now, true, None);
            now += 50;
        }
        assert!(a.presentation.camera.rest_ms > 0.0, "move runs");
        assert_eq!(a.presentation.camera.bind, Some(ids[0]));
        // Saving like `GameFlow::save_game`; clear the host's selection so that both matches report the same in the
        // first step (the savegame's mirror still holds it).
        a.selection.ids.clear();
        let host = HostPart {
            camera: a.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: -1,
            local_name: String::new(),
            game_speed: a.sim().unwrap().game_speed,
            cinemascope_speed: a.cinemascope_speed(),
            script_camera: a.presentation.camera.clone(),
            control_groups: crate::game::control_groups::empty_save(),
            next_guid_index: crate::game::control_groups::empty_next_guid_index(),
            idle_worker_index: [0; 8],
            masks: crate::game::diplomacy::Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&a.save(host).unwrap().to_bytes()).unwrap();
        let mut b =
            GameSession::resume_from(&gpu, &vfs, file, &view, (320, 200), &mut |_| {}).unwrap();
        assert_eq!(b.presentation.camera, a.presentation.camera);
        assert_eq!(b.camera(), a.camera());
        assert!(b.selection.ids.is_empty());
        // In the cinemascope the minimap's corner buttons are absent, as in the running match.
        assert!(!a.hud.minimap_buttons.is_built());
        assert!(!b.hud.minimap_buttons.is_built());
        assert!(!b.sim().unwrap().state.script.mission.selection.is_empty());
        // The same frame times for both (the clock of `b` starts anew).
        for frame in 0..40 {
            a.advance_by(&gpu, &vfs, 50.0, true, None);
            b.advance_by(&gpu, &vfs, 50.0, true, None);
            if frame == 0 {
                let reported = b.sim().unwrap().log.iter().any(|c| {
                    matches!(&c.command.kind, CommandKind::ScriptSelection { ids } if ids.is_empty())
                });
                assert!(reported, "the first step reports the empty selection");
                assert!(b.sim().unwrap().state.script.mission.selection.is_empty());
            }
            assert_eq!(b.checksum(), a.checksum(), "frame {frame}");
            assert_eq!(
                (b.map.camera.eye, b.map.camera.target),
                (a.map.camera.eye, a.map.camera.target),
                "frame {frame}"
            );
        }
        assert_eq!(b.presentation.camera, a.presentation.camera);
    }

    #[test]
    fn game_session_survives_resize_and_zero_size() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let format = opennml_render::CAPTURE_FORMAT;
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format,
        };
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        // Minimized: frames keep running, nothing is drawn (the target stays untouched).
        session.resize(0, 0);
        session.advance(&gpu, &vfs, 0, true, None);
        session.advance(&gpu, &vfs, 50, true, None);
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = dummy.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        session.render(&gpu, &vfs, &mut encoder, &view, (0, 0));
        queue.submit([encoder.finish()]);
        // Large again: a full frame.
        session.resize(1600, 900);
        session.advance(&gpu, &vfs, 100, true, None);
        let image = opennml_render::render_to_image(
            &device,
            &queue,
            (1600, 900),
            format,
            |encoder, target| {
                session.render(&gpu, &vfs, encoder, target, (1600, 900));
            },
        )
        .unwrap();
        assert_eq!(image.dimensions(), (1600, 900));
        assert!(image.pixels().any(|p| p.0[3] != 0), "the world is drawn");
    }

    /// The game UI draws the minimap (frame, terrain with fog, border, dots of the own start objects, four lines of the
    /// camera frame) without effect on the checksum. In the cinemascope it is absent, even if the map itself would be
    /// visible (the host's lock, `InputGate::minimap_visible`), afterwards it is back.
    // QUIRK O-39: kept
    #[test]
    fn the_session_draws_the_minimap_except_in_cinemascope() {
        use crate::game::minimap::draw::{BORDER_IMAGE, FOG_TEXTURE, FRAME_IMAGE};
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.resize(1024, 768);
        session.advance(&gpu, &vfs, 0, true, None);
        session.advance(&gpu, &vfs, 50, true, None);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let paths = |session: &GameSession, layer: &mut crate::ui_layer::UiLayer| {
            let mut ctx = layer.context(&vfs, screen_of((1024, 768)));
            let texts = ctx.texts;
            let cmds = session.hud_cmds(&mut ctx, texts);
            cmds.iter()
                .map(|c| match c {
                    DrawCmd::Quad { texture, .. } | DrawCmd::Quad4 { texture, .. } => {
                        texture.map(|t| ctx.assets.path(t).to_string())
                    }
                })
                .collect::<Vec<_>>()
        };
        let norm = opennml_rda::normalize_path;
        let before = session.checksum();
        let drawn = paths(&session, &mut layer);
        assert_eq!(session.checksum(), before, "drawing changes nothing");
        let at = |p: &str| {
            drawn
                .iter()
                .position(|d| d.as_deref() == Some(norm(p).as_str()))
        };
        let (frame, fog, border) = (at(FRAME_IMAGE), at(FOG_TEXTURE), at(BORDER_IMAGE));
        assert!(
            frame < fog && fog < border && frame.is_some(),
            "frame, terrain, border: {frame:?} {fog:?} {border:?}"
        );
        let after_border = &drawn[border.unwrap() + 1..];
        let untextured = after_border.iter().take_while(|d| d.is_none()).count();
        assert!(
            untextured > 4,
            "dots of the start objects and four lines: {untextured}"
        );
        let layout = session.hud.minimap.as_ref().unwrap().last_layout().unwrap();
        assert_eq!(layout.center, [935.0, 679.0]);

        // Like script action 147: the flag in the mission state (source of the lock) and the host's presentation.
        session.sim_mut().unwrap().state.script.mission.cinemascope = true;
        session
            .present_by_hand(&gpu, &vfs, None, &Presentation::Cinemascope { on: true })
            .unwrap();
        assert!(
            !session.hud.minimap.as_ref().unwrap().visible(),
            "{{0x1D, 0}}"
        );
        session.hud.minimap.as_mut().unwrap().set_visible(true);
        let hidden = paths(&session, &mut layer);
        assert!(
            !hidden.iter().flatten().any(|p| *p == norm(FRAME_IMAGE)),
            "no minimap in the cinemascope"
        );
        session.sim_mut().unwrap().state.script.mission.cinemascope = false;
        session
            .present_by_hand(&gpu, &vfs, None, &Presentation::Cinemascope { on: false })
            .unwrap();
        assert!(
            session.hud.minimap.as_ref().unwrap().visible(),
            "{{0x1D, 1}}"
        );
        let back = paths(&session, &mut layer);
        assert!(back.iter().flatten().any(|p| *p == norm(FRAME_IMAGE)));
    }

    /// The draw path of `opennml-game` ([`GameSession::over_world_cmds`]) puts the combat display
    /// ([`GameSession::combat_cmds`]: health bars, rank and counter-upgrade icons) below the whole game UI (messages,
    /// bar). A selected wounded unit shows the bar with its fraction; quartered objects and objects invisible to the
    /// player show none. Read-only.
    #[test]
    fn the_game_path_draws_health_bars_under_messages_and_toolbar() {
        use opennml_sim::object::status;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let format = opennml_render::CAPTURE_FORMAT;
        let gpu = SessionGpu {
            device: &device,
            queue: &queue,
            format,
        };
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        let size = (1024, 768);
        session.resize(size.0, size.1);
        session.advance(&gpu, &vfs, 0, true, None);
        session.advance(&gpu, &vfs, 50, true, None);
        let sim = session.sim().unwrap();
        let local = sim.active_player;
        let unit = sim
            .state
            .objects()
            .find(|o| {
                o.owner() == local
                    && o.has_module(0, false)
                    && o.attributes.is_present(14)
                    && o.status & (status::QUARTERED | status::DIED) == 0
            })
            .map(|o| o.id)
            .expect("own unit with hit points");
        let hidden = sim
            .state
            .objects()
            .find(|o| o.owner() != local && o.attributes.is_present(14) && !sim.fog_selectable(o))
            .map(|o| o.id)
            .expect("foreign object in the fog");
        session
            .sim_mut()
            .unwrap()
            .state
            .object_mut(unit, false)
            .unwrap()
            .attributes
            .set_current_fraction(14, 25_000);
        session.selection.ids = vec![unit];
        session.hovered = None;
        // One frame: the top edges of the hulls come from the frame (as in `opennml-game` after `render`).
        opennml_render::render_to_image(&device, &queue, size, format, |encoder, target| {
            session.render(&gpu, &vfs, encoder, target, size);
        })
        .unwrap();
        let before = opennml_sim::save::checksum(&session.sim().unwrap().state);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = layer.context(&vfs, screen_of(size));
        let texts = ctx.texts;
        let all = session.over_world_cmds(&mut ctx, texts);
        let world = session.combat_cmds(&mut ctx);
        let hud = session.hud_cmds(&mut ctx, texts);
        assert_eq!(
            opennml_sim::save::checksum(&session.sim().unwrap().state),
            before,
            "presentation changes nothing"
        );
        assert!(!hud.is_empty(), "bar and status bar are there");
        assert_eq!(
            all,
            [world.clone(), hud].concat(),
            "combat display below the UI"
        );
        let bar_texture = ctx.assets.find(crate::game::combat_calc::BAR_TEXTURE);
        assert!(bar_texture.is_some(), "balken.bmp loaded");
        let bars = |cmds: &[DrawCmd]| -> Vec<[i32; 4]> {
            cmds.iter()
                .filter_map(|c| match *c {
                    DrawCmd::Quad { texture, dst, .. } if texture == bar_texture => Some(dst),
                    _ => None,
                })
                .collect()
        };
        let quads = bars(&world);
        assert_eq!(quads.len(), 2, "background and fill: {world:?}");
        let (back, fill) = (quads[0], quads[1]);
        let width = |r: [i32; 4]| (r[2] - r[0]) as f32;
        assert!(width(back) > 8.0, "{back:?}");
        assert_eq!((fill[0], fill[1], fill[3]), (back[0], back[1], back[3]));
        assert!(
            (width(fill) - 0.25 * width(back)).abs() <= 1.5,
            "fill 25%: {fill:?} in {back:?}"
        );
        // Quartered: no bar.
        session
            .sim_mut()
            .unwrap()
            .state
            .object_mut(unit, false)
            .unwrap()
            .status |= status::QUARTERED;
        assert!(bars(&session.combat_cmds(&mut ctx)).is_empty());
        // Invisible to the player (fog), under the mouse: no bar.
        session.selection.ids.clear();
        session.hovered = Some(hidden);
        assert!(bars(&session.combat_cmds(&mut ctx)).is_empty());
    }

    /// First own object of the active slot with a walk module.
    fn own_walker(session: &GameSession) -> ObjectId {
        let sim = session.sim().unwrap();
        let local = sim.active_player;
        sim.state
            .objects()
            .find(|o| o.owner() == local && o.has_module(0, false))
            .expect("own unit with a walk module")
            .id
    }

    /// O-317: in pause world, message bar and rendering clock stand still; the player camera (keys) and the water run,
    /// the engine counts ticks without world tick (ESC, O-152). The wheel has no effect in pause and not later either
    /// (`0x0043D0E9`). With the game menu open, in contrast, the camera stands still too.
    // QUIRK O-317, O-335: kept
    #[test]
    fn pause_stops_world_and_messages_but_not_the_camera() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.resize(1024, 768);
        let active = session.sim().unwrap().active_player;
        let event = SimEvent::PlayerMessage {
            to: Some(active),
            id: 0,
            tile: (-1, -1),
            flags: 0x0301_0000,
        };
        session.hud.handle(&[event], active, &mut Vec::new());
        let head = |s: &GameSession| {
            let (_, m) = s.hud.messages.heads().find(|(_, m)| m.id == 0).unwrap();
            (m.display_ms, m.fade_ms)
        };
        session.advance(&gpu, &vfs, 1_000, true, None);
        session.advance(&gpu, &vfs, 1_100, true, None);
        assert_eq!(session.ticks, 2, "100 ms at speed 10");
        assert!(!session.sim().unwrap().halted);
        assert!(session.toggle_pause());
        assert_eq!(
            session.pause,
            PauseState {
                sim: true,
                shown: true
            }
        );
        assert!(
            session.sim().unwrap().halted,
            "commands immediate from now on"
        );
        let steps = session.steps();
        let sum = sums(&session);
        let shown = head(&session);
        let clock = session.clock.now_ms;
        let water = session.clock.water_ms();
        let target = session.map.camera.target;
        session.pointer.keys.insert(KeyCode::ArrowUp);
        let mut ticks = 0;
        for i in 1..=62 {
            session.advance(&gpu, &vfs, 1_100 + 16 * i, true, None);
            ticks += session.ticks;
        }
        assert_eq!(session.steps(), steps, "no world tick");
        assert_eq!(sums(&session), sum, "state unchanged");
        assert_eq!(head(&session), shown, "the message bar stands");
        assert_eq!(session.clock.now_ms, clock, "rendering clock stands");
        assert_eq!(session.clock.water_ms(), water + 992.0, "the water runs");
        assert_eq!(ticks, 19, "992 ms of game time: ticks without world tick");
        assert_ne!(session.map.camera.target, target, "the keys scroll");
        // Wheel in pause: read and discarded, no effect after the pause either.
        session.pointer.keys.clear();
        let distance = session.map.camera.distance();
        let wheel = InputEvent::Wheel { up: true };
        let screen = Screen {
            width: 1024,
            height: 768,
        };
        session.input(&wheel, screen, 2_100, &vfs, None);
        for i in 1..=20 {
            session.advance(&gpu, &vfs, 2_092 + 16 * i, true, None);
        }
        assert_eq!(session.map.camera.distance(), distance);
        assert!(session.toggle_pause());
        assert!(!session.sim().unwrap().halted);
        for i in 1..=20 {
            session.advance(&gpu, &vfs, 2_412 + 16 * i, true, None);
        }
        assert!(session.steps() > steps, "the world runs again");
        assert!(head(&session).0 < shown.0, "the message bar runs again");
        assert_eq!(session.map.camera.distance(), distance, "no late wheel");
        session.input(&wheel, screen, 2_800, &vfs, None);
        for i in 1..=20 {
            session.advance(&gpu, &vfs, 2_732 + 16 * i, true, None);
        }
        assert_ne!(
            session.map.camera.distance(),
            distance,
            "without pause the wheel zooms"
        );
        // Game menu open (with pause): everything stands, also camera and water; no ticks.
        assert!(session.toggle_pause());
        session.pointer.keys.insert(KeyCode::ArrowUp);
        let target = session.map.camera.target;
        let water = session.clock.water_ms();
        for i in 1..=20 {
            session.advance(&gpu, &vfs, 3_052 + 16 * i, false, None);
            assert_eq!(session.ticks, 0);
        }
        assert_eq!(session.map.camera.target, target);
        assert_eq!(session.clock.water_ms(), water);
        assert!(session.sim().unwrap().halted);
    }

    /// O-231: in the visible pause a build button has no effect (no build cursor), but the click counts as a button
    /// (`ButtonPressed` first); without pause the same button opens the cursor.
    // QUIRK O-231: kept
    #[test]
    fn build_does_nothing_in_the_visible_pause() {
        use opennml_formats::gifcon::button_kind as kinds;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.resize(1024, 768);
        let sim = session.sim().unwrap();
        let local = sim.active_player;
        let own: Vec<ObjectId> = sim
            .state
            .objects()
            .filter(|o| o.owner() == local)
            .map(|o| o.id)
            .collect();
        let build_button = |s: &GameSession| {
            s.hud.toolbar.as_ref().unwrap().buttons().find_map(|b| {
                let kind = b.spec.flags & kinds::KIND_MASK;
                (b.clickable && (kind == kinds::BUILD || kind == kinds::ONLYONCE)).then(|| {
                    let [l, t, r, bo] = b.spec.rect;
                    ([(l + r) / 2, (t + bo) / 2], b.spec.msg)
                })
            })
        };
        // A button that opens a submenu with build buttons (worker: buildings).
        let build_menu = |s: &GameSession| {
            let menus = &s.sim().unwrap().rules.toolbar.menus;
            s.hud.toolbar.as_ref().unwrap().buttons().find_map(|b| {
                let opens = menus
                    .get(&b.spec.msg)
                    .is_some_and(|m| m.iter().any(|r| r.flags & kinds::KIND_MASK == kinds::BUILD));
                (b.clickable && opens).then(|| {
                    let [l, t, r, bo] = b.spec.rect;
                    [(l + r) / 2, (t + bo) / 2]
                })
            })
        };
        let mut modes = CountingModes::default();
        let mut found = None;
        for id in own {
            session.selection.ids = vec![id];
            session.selection_changed(&vfs, None);
            session.advance_by(&gpu, &vfs, 16.0, true, None);
            session.advance_by(&gpu, &vfs, 16.0, true, None);
            if let Some(at) = build_menu(&session) {
                left_click(&mut session, &vfs, &mut modes, at, 0.0);
                session.advance_by(&gpu, &vfs, 16.0, true, None);
                session.advance_by(&gpu, &vfs, 16.0, true, None);
            }
            if let Some(b) = build_button(&session) {
                found = Some(b);
                break;
            }
        }
        let (at, msg) = found.expect("own object with a clickable build button");
        let presses = modes.presses;
        assert!(session.toggle_pause());
        left_click(&mut session, &vfs, &mut modes, at, 2_000.0);
        let toolbar = session.hud.toolbar.as_ref().unwrap();
        assert!(toolbar.cursor.is_none(), "no build cursor in pause");
        assert_eq!(session.last_button, Some(msg), "the button reports anyway");
        assert_eq!(modes.presses, presses, "the click belongs to the bar");
        // In the pause step the bar keeps running (frame event 6), the button stays.
        session.advance(&gpu, &vfs, 1_000, true, None);
        session.advance(&gpu, &vfs, 1_016, true, None);
        assert_eq!(build_button(&session).map(|b| b.1), Some(msg));
        assert!(session.toggle_pause());
        left_click(&mut session, &vfs, &mut modes, at, 4_000.0);
        assert!(
            session.hud.toolbar.as_ref().unwrap().cursor.is_some(),
            "without pause the build button opens the cursor"
        );
    }

    /// O-318: a move command in pause takes effect in the state at once (it does not wait for a tick), but the unit
    /// stands still until resuming; the replay of the log gives the same checksum as the match.
    // QUIRK O-318: kept
    #[test]
    fn move_command_in_pause_executes_at_once_and_replays_alike() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.advance(&gpu, &vfs, 1_000, true, None);
        session.advance(&gpu, &vfs, 1_100, true, None);
        let unit = own_walker(&session);
        assert!(session.toggle_pause());
        let sim = session.sim_mut().unwrap();
        let pos = sim.state.object(unit, false).unwrap().world_pos;
        let steps = sim.state.steps;
        let before = opennml_sim::save::checksum(&sim.state);
        let target = Vec3::new(pos[0] + 400.0, pos[1], pos[2] + 400.0);
        assert!(sim.move_units(&[unit], target, None, FormationOrder::default()));
        assert!(
            sim.pending_commands().is_empty(),
            "not at the tick boundary"
        );
        let last = sim.log.last().unwrap();
        assert_eq!(last.step, steps);
        assert!(matches!(last.command.kind, CommandKind::Move { .. }));
        let after = opennml_sim::save::checksum(&sim.state);
        assert_ne!(
            after, before,
            "the command takes effect in the state at once"
        );
        assert_eq!(sim.checksum(), after, "checksum brought up to date");
        for i in 1..=60 {
            session.advance(&gpu, &vfs, 1_100 + 16 * i, true, None);
        }
        let sim = session.sim().unwrap();
        assert_eq!(sim.state.steps, steps);
        assert_eq!(
            sim.state.object(unit, false).unwrap().world_pos,
            pos,
            "the unit stands until resuming"
        );
        assert!(session.toggle_pause());
        for i in 1..=60 {
            session.advance(&gpu, &vfs, 2_060 + 16 * i, true, None);
        }
        let sim = session.sim().unwrap();
        assert!(sim.state.steps > steps + 10);
        assert_ne!(
            sim.state.object(unit, false).unwrap().world_pos,
            pos,
            "it walks"
        );
        let replay =
            opennml_sim::save::Replay::from_bytes(&sim.replay().unwrap().to_bytes()).unwrap();
        let played = replay.play(&sim.rules, sim.state.steps).unwrap();
        assert_eq!(
            opennml_sim::save::checksum(&played),
            opennml_sim::save::checksum(&sim.state),
            "replay equal"
        );
    }

    /// O-318: a command A that is still waiting at the tick boundary when the pause is switched on runs at once, before
    /// the command B given in the pause, in the log as in the state; a savegame of the pause contains A, and the replay
    /// reaches the same checksum. Likewise via loss of focus.
    // QUIRK O-318: kept
    #[test]
    fn pending_command_runs_before_commands_of_the_pause() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.advance(&gpu, &vfs, 1_000, true, None);
        session.advance(&gpu, &vfs, 1_100, true, None);
        let p = i32::from(session.sim().unwrap().active_player);
        let q = (p + 1) % 2;
        let diplomacy = |mode| opennml_sim::command::Command {
            player: p as u8,
            kind: CommandKind::Diplomacy { p, q, mode },
        };
        let (a, b) = (diplomacy(0), diplomacy(1));
        session.sim_mut().unwrap().submit(a.clone());
        assert_eq!(
            session.sim().unwrap().pending_commands(),
            std::slice::from_ref(&a)
        );
        assert!(session.toggle_pause());
        let sim = session.sim().unwrap();
        assert!(
            sim.pending_commands().is_empty(),
            "A does not wait for resuming"
        );
        assert!(
            sim.state.diplomacy.is_friend(p, q),
            "A takes effect at once"
        );
        let saved = opennml_sim::save::SaveGame::from_bytes(&sim.save().to_bytes()).unwrap();
        assert!(
            saved.state.diplomacy.is_friend(p, q),
            "the savegame of the pause contains A"
        );
        let steps = sim.state.steps;
        session.sim_mut().unwrap().submit(b.clone());
        let sim = session.sim().unwrap();
        assert!(
            sim.state.diplomacy.is_enemy(p, q) && !sim.state.diplomacy.is_friend(p, q),
            "B takes effect after A"
        );
        let tail: Vec<_> = sim.log[sim.log.len() - 2..]
            .iter()
            .map(|c| (c.step, c.command.clone()))
            .collect();
        assert_eq!(tail, [(steps, a.clone()), (steps, b)]);
        assert!(session.toggle_pause());
        for i in 1..=30 {
            session.advance(&gpu, &vfs, 1_100 + 16 * i, true, None);
        }
        let sim = session.sim().unwrap();
        assert!(sim.state.steps > steps);
        assert!(sim.state.diplomacy.is_enemy(p, q), "A does not run again");
        let replay =
            opennml_sim::save::Replay::from_bytes(&sim.replay().unwrap().to_bytes()).unwrap();
        let played = replay.play(&sim.rules, sim.state.steps).unwrap();
        assert_eq!(
            opennml_sim::save::checksum(&played),
            opennml_sim::save::checksum(&sim.state),
            "replay equal"
        );
        // Loss of focus takes the same path.
        session.sim_mut().unwrap().submit(a.clone());
        assert!(session.focus_lost());
        let sim = session.sim().unwrap();
        assert!(sim.pending_commands().is_empty());
        assert!(sim.state.diplomacy.is_friend(p, q));
        assert_eq!(sim.log.last().unwrap().command, a);
    }

    /// O-154: loss of focus switches the pause on if it is not visible; a further loss of focus changes nothing, the
    /// player has to resume the match themselves.
    // QUIRK O-154: kept
    #[test]
    fn focus_loss_pauses_and_stays() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.advance(&gpu, &vfs, 1_000, true, None);
        assert!(session.focus_lost());
        let paused = PauseState {
            sim: true,
            shown: true,
        };
        assert_eq!(session.pause, paused);
        assert!(session.sim().unwrap().halted);
        assert!(!session.focus_lost(), "already visibly paused: nothing");
        assert_eq!(session.pause, paused);
        let steps = session.steps();
        for i in 1..=30 {
            session.advance(&gpu, &vfs, 1_000 + 16 * i, true, None);
        }
        assert_eq!(session.steps(), steps, "stays stopped");
        assert!(session.toggle_pause(), "Ctrl+P resumes");
        assert_eq!(session.pause, PauseState::default());
        // Only the visible bit (network echo): no toggling.
        session.pause.shown = true;
        assert!(!session.focus_lost());
        // Menu level (game type -2): `togglepause` has no effect.
        session.pause = PauseState::default();
        session.game_type = -2;
        assert!(!session.focus_lost());
        assert_eq!(session.pause, PauseState::default());
    }

    /// `ui_state.game.input` reports pause bits, speed, groups and last group of the active slot, the pointer and the
    /// host's setting, the keyboard context and the locks, without changing the checksum.
    #[test]
    fn ui_state_reports_pause_speed_groups_pointer_and_context() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.advance(&gpu, &vfs, 1_000, true, None);
        let checksum = session.sim().unwrap().checksum();
        let normal = session.input_data(PointerChoice::NORMAL, true, 10);
        assert!(!normal.paused && !normal.shown_paused);
        assert_eq!(normal.game_speed, 10);
        assert_eq!(normal.config_speed, 10);
        assert_eq!(normal.groups, vec![Vec::<u32>::new(); 10]);
        assert_eq!(normal.last_group, -1);
        assert_eq!((normal.pointer, normal.pointer_variant), (7, 0));
        assert!(normal.pointer_visible);
        let ctx = session.key_context();
        assert_eq!(normal.key_context, [ctx.kind, ctx.nation, ctx.selection]);
        assert!(normal.key_context[1] >= 1, "nation + 1");
        // Direct start: game type -1 like the campaign (`GameSession::game_type`), otherwise no lock.
        assert_eq!(normal.gate, ["campaign"]);

        // Pause, speed +1, group 2 of the active slot, game menu open, pointer 9 green and off.
        assert!(session.toggle_pause());
        assert_eq!(session.change_game_speed(1, &mut Vec::new()), Some(11));
        let local = usize::from(session.sim().unwrap().active_player);
        let ids = [
            ObjectId::new(1, local as u8, 3),
            ObjectId::new(1, local as u8, 4),
        ];
        session.groups[local].lists[2] = ids.to_vec();
        session.groups[local].last = 2;
        session.groups[(local + 1) % PLAYER_SLOTS].lists[5] = vec![ObjectId::new(9, 0, 9)];
        session.game_type = 3;
        session.menu_open = true;
        let input = session.input_data(
            PointerChoice {
                id: 9,
                variant: 2,
                ..PointerChoice::NORMAL
            },
            false,
            11,
        );
        assert!(input.paused && input.shown_paused);
        assert_eq!((input.game_speed, input.config_speed), (11, 11));
        assert_eq!(input.groups.len(), 10);
        assert_eq!(input.groups[2], [ids[0].0, ids[1].0]);
        assert!(
            input.groups[5].is_empty(),
            "only the groups of the active slot"
        );
        assert_eq!(input.last_group, 2);
        assert_eq!((input.pointer, input.pointer_variant), (9, 2));
        assert!(!input.pointer_visible);
        assert_eq!(input.gate, ["menu_open"]);
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["groups"][2], serde_json::json!([ids[0].0, ids[1].0]));
        assert_eq!(json["gate"], serde_json::json!(["menu_open"]));
        assert_eq!(session.sim().unwrap().checksum(), checksum, "read-only");
    }

    /// re/game-ui N.4: the PAUSE lettering (text 15788) lies, with a visible pause, between the messages and the game
    /// windows, also in the cinemascope; without pause `over_world_cmds` draws exactly as before (combat display, UI,
    /// scene fade, info windows).
    #[test]
    fn pause_text_lies_between_messages_and_windows() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let size = (1024, 768);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = layer.context(&vfs, screen_of(size));
        let texts = ctx.texts;
        let plain = session.over_world_cmds(&mut ctx, texts);
        let world = session.combat_cmds(&mut ctx);
        assert_eq!(
            plain,
            [
                world.clone(),
                session.hud_cmds(&mut ctx, texts),
                session.fade_cmds(ctx.screen),
                session.desktop_cmds(&mut ctx, texts)
            ]
            .concat()
        );
        assert!(session.toggle_pause());
        let paused = session.over_world_cmds(&mut ctx, texts);
        let label = texts.display(opennml_ui::hud::pause_text::PAUSE_TEXT);
        let mut text = Vec::new();
        opennml_ui::hud::pause_text::draw(&mut ctx, &label, &mut text);
        assert_eq!(text.len(), label.encode_utf16().count());
        let mut messages = Vec::new();
        let sim = session.sim().unwrap();
        session
            .hud
            .draw_world_pass(&ctx, &SimGameText::new(sim, texts), &mut messages);
        let start = world.len() + messages.len();
        assert_eq!(paused[start..start + text.len()], text[..]);
        assert_eq!(paused.len(), plain.len() + text.len());
        // In the cinemascope the lettering stays at the same place; the UI keeps drawing because the cinemascope area
        // belongs to the game windows (re/campaign-ui D6.4; bars and messages hide themselves).
        session.presentation.cinemascope = true;
        let cinema = session.over_world_cmds(&mut ctx, texts);
        let world = session.combat_cmds(&mut ctx);
        let mut messages = Vec::new();
        let sim = session.sim().unwrap();
        session
            .hud
            .draw_world_pass(&ctx, &SimGameText::new(sim, texts), &mut messages);
        let start = world.len() + messages.len();
        assert_eq!(cinema[start..start + text.len()], text[..]);
    }

    // ---- Selection box: drawing of the root -------------------------------------------------------------------------

    /// Presses (`true`) or releases the left button at the current pointer position.
    fn left_button(session: &mut GameSession, vfs: &Vfs, modes: &mut CountingModes, pressed: bool) {
        let ev = InputEvent::Button {
            button: MouseButton::Left,
            pressed,
        };
        let screen = (session.size.0, session.size.1);
        session.input_with(&ev, screen, 0.0, vfs, None, modes);
    }

    /// O-384: the box appears from the press on (without extent it draws nothing) between press point and pointer,
    /// after the messages of the world pass and after PAUSE, before the game windows; also in the cinemascope. The
    /// viewer path [`GameSession::hud_cmds`] does not have it. Releasing ends it; the checksum stays.
    // QUIRK O-384: kept
    #[test]
    fn selection_rect_lies_between_pause_and_windows() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let checksum = session.sim().unwrap().checksum();
        let size = (1024, 768);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = layer.context(&vfs, screen_of(size));
        let texts = ctx.texts;
        let mut modes = CountingModes::default();
        move_to(&mut session, &vfs, &mut modes, [300, 200]);
        let plain = session.over_world_cmds(&mut ctx, texts);
        left_button(&mut session, &vfs, &mut modes, true);
        assert_eq!(
            session.selection_rect(ctx.screen),
            Some(([300, 200], [300, 200])),
            "from the press on"
        );
        assert_eq!(
            session.over_world_cmds(&mut ctx, texts),
            plain,
            "nothing without extent"
        );
        move_to(&mut session, &vfs, &mut modes, [420, 260]);
        let mut rect = Vec::new();
        opennml_ui::hud::selection_rect::draw([300, 200], [420, 260], &mut rect);
        assert_eq!(rect.len(), 5, "area and four lines");
        let start = |session: &GameSession, ctx: &mut UiContext<'_>| {
            let world = session.combat_cmds(ctx);
            let mut messages = Vec::new();
            let sim = session.sim().unwrap();
            session
                .hud
                .draw_world_pass(ctx, &SimGameText::new(sim, texts), &mut messages);
            world.len() + messages.len()
        };
        let dragged = session.over_world_cmds(&mut ctx, texts);
        let at = start(&session, &mut ctx);
        assert_eq!(dragged[at..at + rect.len()], rect[..]);
        assert_eq!(dragged.len(), plain.len() + rect.len());
        assert!(
            !session
                .hud_cmds(&mut ctx, texts)
                .windows(rect.len())
                .any(|w| w == rect),
            "the viewer draws its own box"
        );
        // Pause: first PAUSE, then the box.
        assert!(session.toggle_pause());
        let paused = session.over_world_cmds(&mut ctx, texts);
        let label = texts.display(opennml_ui::hud::pause_text::PAUSE_TEXT);
        let mut text = Vec::new();
        opennml_ui::hud::pause_text::draw(&mut ctx, &label, &mut text);
        let at = start(&session, &mut ctx);
        assert_eq!(paused[at..at + text.len()], text[..]);
        assert_eq!(
            paused[at + text.len()..at + text.len() + rect.len()],
            rect[..]
        );
        assert!(session.toggle_pause());
        // Cinemascope (without lock): the box stays, the cinemascope area lies above it as a game window.
        session.presentation.cinemascope = true;
        let cinema = session.over_world_cmds(&mut ctx, texts);
        let at = start(&session, &mut ctx);
        assert_eq!(cinema[at..at + rect.len()], rect[..]);
        session.presentation.cinemascope = false;
        left_button(&mut session, &vfs, &mut modes, false);
        assert_eq!(session.selection_rect(ctx.screen), None, "released");
        assert_eq!(session.over_world_cmds(&mut ctx, texts).len(), plain.len());
        assert_eq!(session.sim().unwrap().checksum(), checksum, "read-only");
    }

    /// O-384: `{0x19, 1}` ends a running box without a selection (`0x00449ADB`), `{0x19, 0}` does not bring it back;
    /// releasing afterwards selects nothing.
    // QUIRK O-384: kept
    #[test]
    fn lock_ends_a_running_selection_rect() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let screen = screen_of((1024, 768));
        let mut modes = CountingModes::default();
        move_to(&mut session, &vfs, &mut modes, [0, 30]);
        left_button(&mut session, &vfs, &mut modes, true);
        move_to(&mut session, &vfs, &mut modes, [1023, 590]);
        assert!(session.selection_rect(screen).is_some());
        for on in [true, false] {
            session
                .present_by_hand(&gpu, &vfs, None, &Presentation::UiLock { on })
                .unwrap();
            assert_eq!(session.selection_rect(screen), None, "lock {on}");
        }
        left_button(&mut session, &vfs, &mut modes, false);
        assert!(session.selection.ids.is_empty(), "no box evaluated");
    }

    /// O-385: a box begun in the world, released over the command bar, is evaluated (slot 3 of the root takes raw kind
    /// 2 before the windows, `0x004528F0`, `0x00449AEF`); the bar does not see the release. The box without Shift
    /// clears the selection first; in this test match shortly after the start nothing in the box is visible enough to
    /// select (fog check `0x00467D70`), so the selection is empty afterwards. If the bar takes the release
    /// (beforehand), the box lapses and the bar object stays selected.
    // QUIRK O-385: kept
    #[test]
    fn releasing_a_rect_over_the_toolbar_selects() {
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let before = session.selection.ids.clone();
        assert_eq!(before.len(), 1, "bar object selected");
        let [l, t, r, b] = session.hud.toolbar.as_ref().unwrap().rect();
        let end = [r - 40, b - 60];
        assert!(
            l <= end[0] && end[0] < r && t <= end[1] && end[1] < b,
            "end in the bar"
        );
        let mut modes = CountingModes::default();
        move_to(&mut session, &vfs, &mut modes, [0, 30]);
        left_button(&mut session, &vfs, &mut modes, true);
        move_to(&mut session, &vfs, &mut modes, end);
        left_button(&mut session, &vfs, &mut modes, false);
        assert_eq!(modes.releases, 1, "the release belongs to the world");
        assert_eq!(session.select_start, None);
        assert_eq!(session.last_button, None, "no bar button");
        assert_eq!(session.selection.ids, [], "box evaluated");
    }

    // ---- Scene fade, clear color, target highlight ---------------------------------------------------------------

    /// re/campaign-ui D6.4: the scene fade lies above world (combat display) and game UI, as the last command of
    /// [`GameSession::over_world_cmds`]; the desktop (menu, dialogs) and the pointer follow at the host afterwards. It
    /// runs on the program clock also with the game menu open. Without a drawn UI (viewer without `--hud`) none.
    // QUIRK O-362: kept
    #[test]
    fn fade_draws_after_the_hud_and_before_the_desktop() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        let size = (1024, 768);
        session.resize(size.0, size.1);
        session.advance(&gpu, &vfs, 1_000, true, None);
        present(
            &mut session,
            &Presentation::Fade {
                into: true,
                ms: 5_000,
            },
        );
        // Game menu open: no step, but the fade runs (real time `0x004BFF09`).
        session.advance(&gpu, &vfs, 2_000, false, None);
        assert_eq!(session.scene_fade.alpha(), 204, "1000 ms of 5000");
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = layer.context(&vfs, screen_of(size));
        let texts = ctx.texts;
        let all = session.over_world_cmds(&mut ctx, texts);
        let world = session.combat_cmds(&mut ctx);
        let hud = session.hud_cmds(&mut ctx, texts);
        let fade = session.fade_cmds(ctx.screen);
        assert!(!hud.is_empty(), "bar and status bar are there");
        assert_eq!(
            fade,
            [DrawCmd::Quad {
                texture: None,
                dst: [0, 0, 1024, 768],
                uv: [0.0; 4],
                color: 0xCC00_0000,
                filter: opennml_ui::draw::Filter::Point,
                blend: Blend::Alpha,
            }]
        );
        assert_eq!(all, [world, hud, fade].concat(), "fade last");
        session.hud_enabled = false;
        assert!(session.fade_cmds(ctx.screen).is_empty());
    }

    /// In pause (O-317) the scene fade keeps running in real time as in the game menu (`0x004BFF09`); world, game time
    /// and rendering clock stand still.
    #[test]
    fn pause_keeps_the_fades_running() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.resize(1024, 768);
        session.advance(&gpu, &vfs, 1_000, true, None);
        present(
            &mut session,
            &Presentation::Fade {
                into: true,
                ms: 5_000,
            },
        );
        assert!(session.toggle_pause());
        let (steps, clock) = (session.steps(), session.clock.now_ms);
        session.advance(&gpu, &vfs, 2_000, true, None);
        assert_eq!(session.scene_fade.alpha(), 204, "1000 ms of 5000");
        assert_eq!(session.steps(), steps, "no world tick");
        assert_eq!(session.clock.now_ms, clock, "rendering clock stands");
    }

    /// O-376: session start, restart and loading reset the scene fade (`FUN_004BAA10`: state 0, value 255); a running
    /// fade aborts, a 146 without a flag in the state becomes clear.
    // QUIRK O-376: deviation
    #[test]
    fn level_start_and_restart_reset_the_fade() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        assert_eq!(session.scene_fade, SceneFade::default(), "level start");
        let screen = Screen {
            width: 320,
            height: 200,
        };
        assert!(session.fade_cmds(screen).is_empty());
        // Running 145: restart aborts it.
        present(
            &mut session,
            &Presentation::Fade {
                into: true,
                ms: 5_000,
            },
        );
        session.tick_presentation(1_000.0);
        assert_eq!(session.scene_fade.alpha(), 204);
        session.reset_view(&gpu, &vfs);
        assert_eq!(session.scene_fade, SceneFade::default(), "restart");
        assert!(session.fade_cmds(screen).is_empty());
        // 146 only in the host (the state does not have `faded_out`): clear after the restart.
        present(&mut session, &Presentation::Fade { into: false, ms: 0 });
        session.tick_presentation(16.0);
        assert_eq!(session.scene_fade.alpha(), 255);
        session.reset_view(&gpu, &vfs);
        assert_eq!(session.scene_fade.alpha(), 0);
        assert!(session.fade_cmds(screen).is_empty());
    }

    /// O-376: a savegame after 146 and before 145 (`PresentationMemory::faded_out`) shows black at once after loading,
    /// without a fade, and stays black; without the flag the image is clear.
    // QUIRK O-376: deviation
    #[test]
    fn loading_a_faded_out_save_shows_black_without_fade() {
        use opennml_ui::hud::scene_fade::state;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = tutorial_session(&gpu, &vfs);
        let resume = |session: &GameSession| {
            let host = HostPart {
                camera: session.camera(),
                level_info: opennml_ui::draft::LevelInfoData::default(),
                game_type: -1,
                local_name: String::new(),
                game_speed: 10,
                cinemascope_speed: None,
                script_camera: Default::default(),
                control_groups: crate::game::control_groups::empty_save(),
                next_guid_index: crate::game::control_groups::empty_next_guid_index(),
                idle_worker_index: [0; 8],
                masks: crate::game::diplomacy::Masks::initial(),
                lock_teams: false,
                fog_show_mode: 0,
                net: None,
                log: None,
                save_replay: false,
            };
            let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
            GameSession::resume_from(
                &gpu,
                &vfs,
                file,
                &ViewOptions::default(),
                (1024, 768),
                &mut |_| {},
            )
            .unwrap()
        };
        let clear = resume(&session);
        assert_eq!(
            clear.scene_fade,
            SceneFade::default(),
            "clear without the flag"
        );
        session
            .sim_mut()
            .unwrap()
            .state
            .script
            .mission
            .presentation
            .faded_out = true;
        let mut black = resume(&session);
        assert_eq!(
            (black.scene_fade.state, black.scene_fade.alpha()),
            (state::OUT, 255),
            "black at once"
        );
        let screen = Screen {
            width: 1024,
            height: 768,
        };
        let cmds = black.fade_cmds(screen);
        assert!(
            matches!(
                cmds[..],
                [DrawCmd::Quad {
                    color: 0xFF00_0000,
                    dst: [0, 0, 1024, 768],
                    ..
                }]
            ),
            "{cmds:?}"
        );
        black.advance(&gpu, &vfs, 1_000, false, None);
        black.advance(&gpu, &vfs, 3_000, false, None);
        assert_eq!(black.scene_fade.alpha(), 255, "stays black");
    }

    /// E-245, O-374 (`GameApp_Tick` `0x004CA8EC...0x004CA946`): in the cinemascope, in every game type, the world
    /// clears with the fog color when fog is on (`m_EnableFog`: 501/502 before the view's switch), otherwise with
    /// `BackColor`; without cinemascope black, also with fog. [`GameSession::prepare`] passes the color to the
    /// renderer.
    // QUIRK E-245, O-374: kept
    #[test]
    fn clear_color_in_cinemascope_uses_fog_or_back_color() {
        use crate::game::presentation::FogValues;
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let apply = |session: &mut GameSession, event: &Presentation| {
            let mut ctx = PresentCtx {
                map: &mut session.map,
                selection: &mut session.selection,
                queue: &queue,
                audio: None,
                vfs: &vfs,
            };
            session.presentation.apply(event, &mut ctx);
        };
        let back = [0x33, 0x66, 0x99].map(|c| c as f32 / 255.0);
        let fog = [0x10, 0x20, 0x30].map(|c| c as f32 / 255.0);
        for game_type in [-1, 0] {
            let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
            session.game_type = game_type;
            session.view.back_color = 0x0033_6699;
            session.map.script_fog.values = Some(FogValues {
                color: 0xFF10_2030,
                start: 100.0,
                end: 900.0,
            });
            assert_eq!(session.map.fog_color(), fog, "505 before the `.wld`");
            // Without cinemascope black, also with fog.
            session.view.fog = true;
            assert_eq!(session.clear_color(), [0.0; 3], "game type {game_type}");
            apply(&mut session, &Presentation::Cinemascope { on: true });
            assert!(session.presentation.cinemascope);
            // F3 without 501: fog color.
            assert_eq!(session.clear_color(), fog, "F3, game type {game_type}");
            // No fog: `BackColor`.
            session.view.fog = false;
            assert_eq!(session.clear_color(), back, "game type {game_type}");
            // 501 with the view switched off: fog color.
            apply(&mut session, &Presentation::DistanceFog { on: true });
            assert_eq!(session.clear_color(), fog, "501, game type {game_type}");
            // 502 with the view switched on: `BackColor`.
            session.view.fog = true;
            apply(&mut session, &Presentation::DistanceFog { on: false });
            assert_eq!(session.clear_color(), back, "502, game type {game_type}");
            let params = session.prepare(&gpu, &vfs, (320, 200));
            assert_eq!(params.clear, Some(back), "prepare");
            assert!(params.fog.is_none());
            // Cinemascope off: black again.
            apply(&mut session, &Presentation::DistanceFog { on: true });
            apply(&mut session, &Presentation::Cinemascope { on: false });
            assert_eq!(session.clear_color(), [0.0; 3]);
            let params = session.prepare(&gpu, &vfs, (320, 200));
            assert_eq!(params.clear, Some([0.0; 3]));
            assert_eq!(params.fog.map(|f| f.color), Some(fog), "world fog stays");
        }
    }

    /// E-228, re/campaign-ui D6.13: the target highlight 152 gets the selection ring (the same ground marker as
    /// selection and right-click target), without expiry; a later right-click target replaces it (the same field
    /// `GameWorld+0x318`), and conversely 152 replaces a right-click target; 155 and the end of the object clear it.
    // QUIRK E-228: kept
    #[test]
    fn highlight_draws_a_selection_ring() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        session.advance(&gpu, &vfs, 1_000, true, None);
        let sim = session.sim().unwrap();
        let local = sim.active_player;
        let own: Vec<ObjectId> = sim
            .state
            .objects()
            .filter(|o| o.owner() == local && o.has_module(0, false))
            .map(|o| o.id)
            .collect();
        let (unit, other) = (own[0], own[1]);
        assert!(session.selection.ids.is_empty());
        present(&mut session, &Presentation::Highlight { id: Some(unit) });
        assert_eq!(session.presentation.highlight, Some(unit));
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.ground_markers, [unit], "ring at the target");
        // Without expiry (a right-click target ends after 300 ms).
        session.advance(&gpu, &vfs, 2_000, true, None);
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.ground_markers, [unit], "after 1000 ms");
        // A right-click target replaces it and expires; afterwards it does not come back.
        let now = session.clock.now_ms as f32;
        session.set_target_highlight(Some(TargetHighlight {
            id: other,
            start_ms: now,
        }));
        assert_eq!(session.presentation.highlight, None);
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.ground_markers, [other]);
        session.advance(&gpu, &vfs, 2_400, true, None);
        session.mark_ground(&gpu, &vfs, None);
        assert!(session.ground_markers.is_empty(), "expired");
        // 152 replaces a right-click target.
        session.set_target_highlight(Some(TargetHighlight {
            id: other,
            start_ms: session.clock.now_ms as f32,
        }));
        present(&mut session, &Presentation::Highlight { id: Some(unit) });
        assert_eq!(session.target_highlight, None);
        session.mark_ground(&gpu, &vfs, None);
        assert_eq!(session.ground_markers, [unit]);
        // 155 clears.
        present(&mut session, &Presentation::Highlight { id: None });
        session.mark_ground(&gpu, &vfs, None);
        assert!(session.ground_markers.is_empty(), "155");
        // End of the object (`GameWorld_DestroyObject` clears the field).
        present(&mut session, &Presentation::Highlight { id: Some(unit) });
        opennml_sim::world::destroy_object(&mut session.sim_mut().unwrap().sim_context(), unit);
        session.mark_ground(&gpu, &vfs, None);
        assert!(session.ground_markers.is_empty());
        assert_eq!(session.presentation.highlight, None);
    }

    // ---- Speech output at session level ------------------------------------------------------------------------

    /// The wiring of `advance_by` (`speech_step`): every event acts once; an `InfoFrameClose` decides on the
    /// simulation's line (`SpeechLine`): if it still runs (`n169`), the sound stays, if it is over, it fades out.
    // QUIRK O-363: kept
    #[test]
    fn speech_events_reach_the_audio_once() {
        use crate::testutil::FakeAudio;
        use opennml_sim::script::present::Presentation;
        use opennml_sim::script::speech::SpeechLine;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        let mut audio = FakeAudio::default();
        let line = SpeechLine {
            text: 302,
            remaining_ms: 1000,
            slot: 4,
        };
        session.sim_mut().unwrap().state.script.mission.speech.line = Some(line);
        let start = [
            SimEvent::Present(Presentation::Speech {
                text: 302,
                slot: 4,
                ms: 1000,
            }),
            SimEvent::Present(Presentation::Tip {
                text: 5,
                slot: 0,
                ms: 100,
            }),
        ];
        speech_step(
            &mut session.speech,
            session.map.sim.as_ref(),
            &start,
            &mut audio,
            &vfs,
        );
        assert_eq!(
            audio.calls,
            [
                "speech data/sprache/302.ogg",
                "event SFX_TEXT 0x14 sfx false"
            ]
        );
        // `n169` on the window: the simulation still speaks the line, the sound keeps running.
        let close = [SimEvent::Present(Presentation::InfoFrameClose {
            text: 302,
        })];
        speech_step(
            &mut session.speech,
            session.map.sim.as_ref(),
            &close,
            &mut audio,
            &vfs,
        );
        assert_eq!(audio.calls.len(), 2);
        assert_eq!(session.speech.current_text(), Some(302));
        // End of the line in the simulation.
        session.sim_mut().unwrap().state.script.mission.speech.line = None;
        speech_step(
            &mut session.speech,
            session.map.sim.as_ref(),
            &close,
            &mut audio,
            &vfs,
        );
        assert_eq!(audio.calls.last().unwrap(), "fade-out 42 1000");
        assert_eq!(audio.calls.len(), 3);
    }

    /// O-363 (pause and game menu do not stop the channel): without `running` the session produces no events and does
    /// not advance, the speech output stays untouched (the line keeps running, no fading out). The sound side
    /// (`AudioEngine` keeps running in pause) cannot be checked without a device (manual check).
    // QUIRK O-363: kept
    #[test]
    fn paused_session_leaves_the_speech_line_alone() {
        use crate::testutil::FakeAudio;
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = start_two_players(&gpu, &vfs, &mut Vec::new()).unwrap();
        let mut audio = FakeAudio::default();
        let start = [SimEvent::Present(Presentation::Speech {
            text: 120,
            slot: 0,
            ms: 3000,
        })];
        speech_step(
            &mut session.speech,
            session.map.sim.as_ref(),
            &start,
            &mut audio,
            &vfs,
        );
        let steps = session.steps();
        session.menu_open = true;
        for now in [1_000, 5_000, 9_000] {
            let played = session.advance(&gpu, &vfs, now, false, None);
            assert!(played.is_empty());
        }
        assert_eq!(session.steps(), steps, "no simulation in pause and menu");
        assert_eq!(
            session.speech.current_text(),
            Some(120),
            "line keeps running"
        );
        // Structural evidence: `advance` does not get the sound fake at all (it requires the real `AudioEngine`), so
        // the session can neither fade out nor stop it in pause and menu; that the line really keeps playing needs a
        // manual listening check (O-363).
        assert!(audio.speech_playing(42));
    }

    // ---- Control command `ui_present` and `ui_state.game.presentation` -----------------------------------------

    /// Campaign match (game type -1) on two slots at 1024 x 768: bars exist only there (O-350).
    fn campaign_session(gpu: &SessionGpu, vfs: &Vfs) -> GameSession {
        let mut session = start_two_players(gpu, vfs, &mut Vec::new()).unwrap();
        session.game_type = -1;
        session.resize(1024, 768);
        session
    }

    /// `ui_present` is presentation only: the checksum and the simulation's steps stay the same until the next step,
    /// even if the event changes the speed (147) or the fade; `QuickSave` does not arrive at all. Afterwards host
    /// consequences can produce commands: 147/18 clear the host's selection, and `sync_selection` sends a
    /// `ScriptSelection` command in the next frame for scripts with the conditions 35-37; the speed 10 changes the
    /// steps per frame. Checked here is only the state immediately after the events, on a map without UI conditions.
    #[test]
    fn ui_present_leaves_the_checksum() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.advance_by(&gpu, &vfs, 50.0, true, None);
        let (hash, steps) = (session.sim().unwrap().checksum(), session.steps());
        for event in [
            Presentation::Cinemascope { on: true },
            Presentation::Portrait {
                slot: 1,
                portrait: 1000,
                frame: 101,
            },
            Presentation::Speech {
                text: 7,
                slot: 0,
                ms: 1000,
            },
            Presentation::Fade {
                into: false,
                ms: 500,
            },
            Presentation::Cinemascope { on: false },
        ] {
            session
                .present_by_hand(&gpu, &vfs, None, &event)
                .unwrap_or_else(|e| panic!("{event:?}: {e}"));
        }
        let sim = session.sim().unwrap();
        assert_eq!((sim.checksum(), session.steps()), (hash, steps));
        assert!(session.map.sim.as_ref().unwrap().log.is_empty());
    }

    /// `QuickSave` and `ProfileVar` are requests to the flow, not presentation: error message, nothing requested. An
    /// unknown JSON form reports serde's error with the command name.
    #[test]
    fn ui_present_rejects_quicksave_in_german() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        assert_eq!(
            session.present_by_hand(&gpu, &vfs, None, &Presentation::QuickSave),
            Err("ui_present does not know QuickSave (presentation only)".to_string())
        );
        let var = Presentation::ProfileVar {
            name: "x".into(),
            value: 1,
        };
        assert_eq!(
            session.present_by_hand(&gpu, &vfs, None, &var),
            Err("ui_present does not know ProfileVar (presentation only)".to_string())
        );
        assert!(session.presentation.take_requests().is_empty());
        let unknown = session
            .present_json(&gpu, &vfs, None, &serde_json::json!({"Bogus": {}}))
            .unwrap_err();
        assert!(unknown.starts_with("ui_present: "), "{unknown}");
        let missing = session
            .present_json(
                &gpu,
                &vfs,
                None,
                &serde_json::json!({"Portrait": {"slot": 1}}),
            )
            .unwrap_err();
        assert!(missing.starts_with("ui_present: "), "{missing}");
        // The form from the documentation and a unit variant as a string go through.
        session
            .present_json(
                &gpu,
                &vfs,
                None,
                &serde_json::json!({"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}}),
            )
            .unwrap();
        session
            .present_json(&gpu, &vfs, None, &serde_json::json!("CameraBackup"))
            .unwrap();
    }

    /// `ui_state.game.presentation`: bars in screen pixels (clipped like the area), occupied portrait and text slots
    /// with alpha, fade, pointer and clear color in 0...255.
    #[test]
    fn ui_state_reports_bars_portraits_texts_fade_and_pointer() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.view.fog = false;
        session.view.back_color = 0x0033_6699;
        let quiet = session.presentation_state(true);
        assert_eq!(
            quiet,
            UiPresentationData {
                bars: [0, 0],
                portraits: Vec::new(),
                texts: Vec::new(),
                fade_alpha: 0,
                pointer_visible: true,
                clear_color: [0, 0, 0],
                info_frames: Vec::new(),
                countdown: None,
                timers: Vec::new(),
                screen_text: None,
            }
        );
        for event in [
            Presentation::Cinemascope { on: true },
            Presentation::Portrait {
                slot: 1,
                portrait: 1000,
                frame: 101,
            },
            Presentation::Speech {
                text: 7,
                slot: 0,
                ms: 1000,
            },
            Presentation::Fade {
                into: false,
                ms: 1000,
            },
        ] {
            session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
        }
        // Everything begins with alpha 0 (fade 0 ms, first frame); 500 ms of program time afterwards.
        let start = session.presentation_state(false);
        assert_eq!(start.fade_alpha, 0, "fade begins clear");
        assert!(!start.pointer_visible);
        session.tick_presentation(500.0);
        let report = session.presentation_state(false);
        // Bars: BARSIZE of the installation in virtual units (0x1B sets `BARSIZE`, not the image height/6), at H = 768
        // clipped on both sides: top `H*b/1024`, bottom `H - H*(1024 - b)/1024` (O-351); the sides can differ by one
        // pixel.
        let b = session.hud.cinemascope_data.ini.bar_size;
        assert!(b > 0 && session.hud.cinemascope.bar == b);
        assert_eq!(report.bars, [768 * b / 1024, 768 - 768 * (1024 - b) / 1024]);
        assert_eq!(
            report.portraits,
            [UiPortraitSlot {
                slot: 1,
                portrait: 1000,
                frame: 101,
                alpha: 255
            }]
        );
        assert_eq!(
            report.texts,
            [UiTextSlot {
                slot: 0,
                text: 7,
                alpha: 255
            }]
        );
        // Fade after 146 over 1000 ms after 500 ms: value 127.5 -> ftol 127 -> alpha 128.
        assert_eq!(report.fade_alpha, 128);
        assert_eq!(
            report.clear_color,
            [0x33, 0x66, 0x99],
            "BackColor in the cinemascope"
        );
        // Without bars (game type 0): no bar height, the portraits are still in the state.
        session.game_type = 0;
        session.hud.cinemascope.bar = 0;
        let flat = session.presentation_state(true);
        assert_eq!(flat.bars, [0, 0]);
        assert_eq!(flat.portraits.len(), 1);
        assert!(flat.pointer_visible);
    }

    /// `ui_present` with `Cinemascope` or `UiLock` draws bars and hides the command and status bars like the script
    /// event, but pointer and input locks follow the simulation's state ([`GameSession::input_gate`] from
    /// `MissionState`): the pointer stays on, camera, map click and `keys.ini` stay free.
    #[test]
    fn ui_present_cinemascope_leaves_pointer_and_gate_to_the_simulation() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        let free = session.input_gate();
        for event in [
            Presentation::Cinemascope { on: true },
            Presentation::UiLock { on: true },
        ] {
            session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
        }
        assert!(session.presentation.cinemascope);
        assert_eq!(
            session.hud.cinemascope.bar,
            session.hud.cinemascope_data.ini.bar_size
        );
        let gate = session.input_gate();
        assert_eq!(gate, free, "state of the simulation unchanged");
        assert!(
            gate.pointer_visible() && gate.world_click() && gate.keys_ini() && gate.player_camera()
        );
        assert!(
            session
                .presentation_state(gate.pointer_visible())
                .pointer_visible
        );
    }

    // ---- Intro `kamp1-1` with the real script -----------------------------------------------------------------------

    /// Until step `step` in 50 ms frames of program clock `now` ([`GameSession::advance`]: step, then the UI's fades).
    fn run_to_step(
        session: &mut GameSession,
        gpu: &SessionGpu,
        vfs: &Vfs,
        now: &mut u64,
        step: u64,
    ) {
        while session.steps() < step {
            *now += 50;
            session.advance(gpu, vfs, *now, true, None);
        }
    }

    /// `kamp1-1` with level script and campaign setup (game type -1, difficulty 1, `TimePerChar` 55, speed 14 like the
    /// viewer, 1024 x 768) in 50 ms frames, checked at these steps:
    /// - 2: `Fade into 5000` and `Cinemascope on`: bars `BARSIZE` (127/128 px), scene fade begins black;
    /// - 639: line 20002 on slot 1 next to portrait 1000/101 on slot 1 (from 635), 1,098: line 20008; saving and
    ///   loading at 1,100;
    /// - 1,600: `Cinemascope off`: no bars, command and status bars back; 1,622: `Tip 20100` on slot 1 -> text slot 3.
    // QUIRK E-246, O-56, O-350, O-351, O-353, O-356, O-362, O-363: kept
    #[test]
    fn kamp1_intro_shows_bars_portrait_and_line() {
        use opennml_ui::hud::cinemascope::Fade;
        use opennml_ui::hud::scene_fade::state;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = GameSession::start(
            &gpu,
            &vfs,
            "data/levels/kampagne1/kamp1-1/kamp1-1.lvl",
            &mut |_: &Rules| Ok(crate::match_request::campaign_setup(1, 55, None, &[])),
            14,
            &ViewOptions::default(),
            (1024, 768),
            &mut |_| {},
        )
        .unwrap();
        assert!(session.campaign());
        let bar_size = session.hud.cinemascope_data.ini.bar_size;
        let mut now = 0;
        // Step 2: cinemascope bars and scene fade from black, no command and status bars, pointer off.
        run_to_step(&mut session, &gpu, &vfs, &mut now, 2);
        assert!(session.presentation.cinemascope);
        assert_eq!(session.hud.cinemascope.bar, bar_size);
        assert_eq!(session.presentation_state(false).bars, [127, 128]);
        assert_eq!(session.scene_fade.state, state::INTO);
        assert!(session.scene_fade.alpha() >= 250, "begins black");
        assert!(!session.hud.toolbar.as_ref().unwrap().shown);
        assert!(!session.hud.status.as_ref().unwrap().shown);
        let gate = session.input_gate();
        assert!(gate.ui_locked && !gate.world_click() && !gate.pointer_visible());
        // After 5,000 ms of program time (100 frames) the fade is off.
        run_to_step(&mut session, &gpu, &vfs, &mut now, 110);
        assert_eq!(
            (session.scene_fade.state, session.scene_fade.alpha()),
            (state::OFF, 0)
        );
        // Step 700: portrait 1000 and line 20002 on slot 1, both fully visible.
        run_to_step(&mut session, &gpu, &vfs, &mut now, 700);
        let view = &session.hud.cinemascope;
        let p = view.portraits[1];
        assert_eq!((p.portrait, p.frame, p.fade), (1000, 101, Fade::shown()));
        assert_eq!(view.texts[1].text, 20002);
        assert_eq!(view.texts[1].fade.state, 1);
        assert!(
            [0, 2, 3]
                .iter()
                .all(|&i| view.portraits[i].fade.state == 0 && view.texts[i].fade.state == 0),
            "only slot 1 occupied"
        );
        // Step 1,100: line 20008 next to portrait 1000.
        run_to_step(&mut session, &gpu, &vfs, &mut now, 1_100);
        let view = &session.hud.cinemascope;
        assert_eq!(
            (view.texts[1].text, view.portraits[1].portrait),
            (20008, 1000)
        );
        assert!(view.texts[1].fade.visible() && view.portraits[1].fade.visible());
        // Saving and loading in the middle of line 20008 (in `opennml-game` the input lock blocks saving and F1, but the
        // host loads exactly like this): bars, portrait and line stand again at once without fade, speed 10 with the
        // remembered speed of the host part, marker and checksum equal; the next 100 steps give the same checksum.
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: -1,
            local_name: String::new(),
            game_speed: session.sim().unwrap().game_speed,
            cinemascope_speed: session.cinemascope_speed(),
            script_camera: session.presentation.camera.clone(),
            control_groups: crate::game::control_groups::empty_save(),
            next_guid_index: crate::game::control_groups::empty_next_guid_index(),
            idle_worker_index: [0; 8],
            masks: crate::game::diplomacy::Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let mut loaded = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (1024, 768),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(
            (loaded.steps(), loaded.checksum()),
            (session.steps(), session.checksum())
        );
        let view = &loaded.hud.cinemascope;
        assert_eq!(view.bar, bar_size);
        let p = view.portraits[1];
        assert_eq!((p.portrait, p.frame, p.fade), (1000, 101, Fade::shown()));
        assert_eq!(
            (view.texts[1].text, view.texts[1].fade),
            (20008, Fade::shown())
        );
        assert_eq!(loaded.sim().unwrap().game_speed, 10);
        assert_eq!(
            loaded.cinemascope_speed(),
            Some(14),
            "remembered speed of the host part"
        );
        assert_eq!(
            loaded.presentation.highlight,
            session.presentation.highlight
        );
        assert!(
            loaded.presentation.highlight.is_some(),
            "marker at the speaker"
        );
        assert!(!loaded.input_gate().pointer_visible());
        let mut loaded_now = 0;
        run_to_step(&mut loaded, &gpu, &vfs, &mut loaded_now, 1_200);
        run_to_step(&mut session, &gpu, &vfs, &mut now, 1_200);
        assert_eq!(
            (loaded.steps(), loaded.checksum()),
            (session.steps(), session.checksum())
        );
        // Step 1,600: cinemascope and lock off, command and status bars back.
        run_to_step(&mut session, &gpu, &vfs, &mut now, 1_600);
        assert!(!session.presentation.cinemascope);
        assert_eq!(session.hud.cinemascope.bar, 0);
        assert_eq!(session.presentation_state(true).bars, [0, 0]);
        assert!(session.hud.toolbar.as_ref().unwrap().shown);
        assert!(session.hud.status.as_ref().unwrap().shown);
        let gate = session.input_gate();
        assert!(!gate.ui_locked && gate.world_click() && gate.pointer_visible());
        // Step 1,622: tip 20100 (slot 1 of the tips) on text slot 3, fades in.
        run_to_step(&mut session, &gpu, &vfs, &mut now, 1_622);
        let tip = session.hud.cinemascope.texts[3];
        assert_eq!((tip.text, tip.fade.state), (20100, 3));
        assert_eq!(session.scene_fade.state, state::OFF);
    }

    // ---- Info windows in the match -----------------------------------------------------------------------------

    /// One frame of the info windows with `layer` at the session's size ([`GameSession::desktop_cmds`]; measures
    /// windows not yet measured), then text ID and rectangle (grid) of all open windows, head first.
    fn draw_frames(
        session: &GameSession,
        layer: &mut crate::ui_layer::UiLayer,
        vfs: &Vfs,
    ) -> Vec<(i32, [i32; 4])> {
        let mut ctx = layer.context(vfs, screen_of(session.size));
        let texts = ctx.texts;
        session.desktop_cmds(&mut ctx, texts);
        session.info_frames.rects()
    }

    fn ids(frames: &[(i32, [i32; 4])]) -> Vec<i32> {
        frames.iter().map(|f| f.0).collect()
    }

    /// The presentation events open and close the host's info windows: 168 (`InfoFrame`) as a window, 213
    /// (`InfoSpeechFrame`) as a speech window, both at the head (O-360); `InfoFrameClose` closes exactly the ID, an
    /// unknown ID is ignored; a second 168 of the same ID replaces the window (E-249). Session start: none.
    // QUIRK E-249: kept
    #[test]
    fn info_frame_events_open_and_close_frames() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        assert!(
            draw_frames(&session, &mut layer, &vfs).is_empty(),
            "session start"
        );
        for event in [
            Presentation::InfoFrame {
                text: 20950,
                x: 10,
                y: 20,
                style: 1,
            },
            Presentation::InfoSpeechFrame {
                text: 20002,
                slot: 1,
                portrait: 1000,
                frame: 101,
            },
        ] {
            session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
        }
        let frames = draw_frames(&session, &mut layer, &vfs);
        assert_eq!(ids(&frames), [20002, 20950], "the newer one at the head");
        let speech = frames[0].1;
        assert_eq!(
            (speech[0], speech[1], speech[2]),
            (650, 40, 1280),
            "213 at pos 1"
        );
        let window = frames[1].1;
        assert_eq!((window[1], window[2]), (20, 1280), "168 with flag 1");
        // Unknown ID: nothing; known: exactly this window, at once.
        for (text, left) in [(5, vec![20002, 20950]), (20002, vec![20950])] {
            session
                .present_by_hand(&gpu, &vfs, None, &Presentation::InfoFrameClose { text })
                .unwrap();
            assert_eq!(ids(&session.info_frames.rects()), left, "Close {text}");
        }
        // 168 of the same ID replaces the window (flag 2: x = 0).
        session
            .present_by_hand(
                &gpu,
                &vfs,
                None,
                &Presentation::InfoFrame {
                    text: 20950,
                    x: 10,
                    y: 30,
                    style: 2,
                },
            )
            .unwrap();
        let frames = draw_frames(&session, &mut layer, &vfs);
        assert_eq!(ids(&frames), [20950]);
        assert_eq!((frames[0].1[0], frames[0].1[1]), (0, 30));
    }

    /// A speech window (213 with pos >= 0, line on slot >= 4) closes at the end of speech via the simulation's
    /// `InfoFrameClose` (`FUN_00444C10` -> `FUN_00443450` -> `FUN_004401C0`), not earlier; the host has no clock of its
    /// own for it.
    // QUIRK E-240, E-249: kept
    #[test]
    fn speech_frame_closes_at_speech_end() {
        use opennml_sim::script::InfoFrameMemory;
        use opennml_sim::script::present::Presentation;
        use opennml_sim::script::speech::SpeechLine;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        session
            .present_by_hand(
                &gpu,
                &vfs,
                None,
                &Presentation::InfoSpeechFrame {
                    text: 302,
                    slot: 0,
                    portrait: 1000,
                    frame: 101,
                },
            )
            .unwrap();
        // State of the simulation as after 213 with pos 0: line 302 on slot 4 for another 300 ms, window in the table.
        let mission = &mut session.sim_mut().unwrap().state.script.mission;
        mission.speech.line = Some(SpeechLine {
            text: 302,
            remaining_ms: 300,
            slot: 4,
        });
        mission.presentation.info_frames.insert(
            302,
            InfoFrameMemory::Speech {
                slot: 0,
                portrait: 1000,
                frame: 101,
            },
        );
        assert_eq!(ids(&draw_frames(&session, &mut layer, &vfs)), [302]);
        let mut frames = 0;
        while session
            .sim()
            .unwrap()
            .state
            .script
            .mission
            .speech
            .line
            .is_some()
        {
            assert_eq!(ids(&session.info_frames.rects()), [302], "still speaking");
            session.advance_by(&gpu, &vfs, 16.0, true, None);
            frames += 1;
            assert!(frames < 200, "line does not end");
        }
        assert!(
            session.info_frames.rects().is_empty(),
            "end of speech closes"
        );
        assert!(draw_frames(&session, &mut layer, &vfs).is_empty());
    }

    /// After loading the info windows come from `presentation.info_frames` in key order (text ID; the table does not
    /// know an opening order), so each at the head: the largest ID is at the bottom (E-249). Windows only in the host
    /// (without an entry in the state) no longer exist afterwards; a restart without windows in the state empties the
    /// list.
    // QUIRK E-249: kept
    // QUIRK E-239: deviation
    #[test]
    fn loading_restores_info_frames_in_text_id_order() {
        use opennml_sim::script::InfoFrameMemory;
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = tutorial_session(&gpu, &vfs);
        let memory = &mut session
            .sim_mut()
            .unwrap()
            .state
            .script
            .mission
            .presentation
            .info_frames;
        memory.insert(
            30,
            InfoFrameMemory::Window {
                x: 100,
                y: 200,
                style: 0,
            },
        );
        memory.insert(
            10,
            InfoFrameMemory::Speech {
                slot: 1,
                portrait: 1000,
                frame: 101,
            },
        );
        memory.insert(
            20,
            InfoFrameMemory::Window {
                x: 300,
                y: 0,
                style: 2,
            },
        );
        session
            .present_by_hand(
                &gpu,
                &vfs,
                None,
                &Presentation::InfoFrame {
                    text: 40,
                    x: 0,
                    y: 0,
                    style: 0,
                },
            )
            .unwrap();
        let host = HostPart {
            camera: session.camera(),
            level_info: opennml_ui::draft::LevelInfoData::default(),
            game_type: -1,
            local_name: String::new(),
            game_speed: 10,
            cinemascope_speed: None,
            script_camera: Default::default(),
            control_groups: crate::game::control_groups::empty_save(),
            next_guid_index: crate::game::control_groups::empty_next_guid_index(),
            idle_worker_index: [0; 8],
            masks: crate::game::diplomacy::Masks::initial(),
            lock_teams: false,
            fog_show_mode: 0,
            net: None,
            log: None,
            save_replay: false,
        };
        let file = GameSaveFile::from_bytes(&session.save(host).unwrap().to_bytes()).unwrap();
        let mut loaded = GameSession::resume_from(
            &gpu,
            &vfs,
            file,
            &ViewOptions::default(),
            (1024, 768),
            &mut |_| {},
        )
        .unwrap();
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let frames = draw_frames(&loaded, &mut layer, &vfs);
        assert_eq!(ids(&frames), [30, 20, 10], "10, 20, 30 each at the head");
        assert_eq!((frames[0].1[0], frames[0].1[1]), (100, 200));
        assert_eq!((frames[1].1[0], frames[1].1[1]), (0, 0), "flag 2");
        assert_eq!((frames[2].1[0], frames[2].1[1]), (650, 40), "213 at pos 1");
        // Restart of the view (viewer `sim_load`/restart) from a state without windows: empty.
        loaded
            .sim_mut()
            .unwrap()
            .state
            .script
            .mission
            .presentation
            .info_frames
            .clear();
        loaded.reset_view(&gpu, &vfs);
        assert!(draw_frames(&loaded, &mut layer, &vfs).is_empty());
    }

    /// re/campaign-ui D6.4 step 4: the info windows lie as desktop above the scene fade ([`GameSession::desktop_cmds`]
    /// as the last part of [`GameSession::over_world_cmds`]); game menu, dialogs and pointer are drawn by the host
    /// afterwards. They draw (and measure themselves) in every frame with a match, also with the game menu open and in
    /// the cinemascope without the command and status bars; not without a drawn UI (viewer without `--hud`).
    // QUIRK O-360: kept
    #[test]
    fn info_frames_draw_after_the_fade_and_before_menus() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.advance(&gpu, &vfs, 1_000, true, None);
        for event in [
            Presentation::Cinemascope { on: true },
            Presentation::Fade { into: false, ms: 0 },
            Presentation::InfoSpeechFrame {
                text: 20002,
                slot: 1,
                portrait: 1000,
                frame: 101,
            },
        ] {
            session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
        }
        // Game menu open: no step; the fade is black.
        session.menu_open = true;
        session.advance(&gpu, &vfs, 1_100, false, None);
        assert_eq!(session.scene_fade.alpha(), 255);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = layer.context(&vfs, screen_of(session.size));
        let texts = ctx.texts;
        let all = session.over_world_cmds(&mut ctx, texts);
        assert_eq!(
            ids(&session.info_frames.rects()),
            [20002],
            "measured in the frame with the menu open"
        );
        let world = session.combat_cmds(&mut ctx);
        let hud = session.hud_cmds(&mut ctx, texts);
        let fade = session.fade_cmds(ctx.screen);
        let desktop = session.desktop_cmds(&mut ctx, texts);
        assert!(!hud.is_empty(), "bars of the cinemascope");
        assert_eq!(fade.len(), 1, "scene fade");
        assert!(
            desktop.iter().any(|c| matches!(
                c,
                DrawCmd::Quad {
                    texture: Some(_),
                    ..
                }
            )),
            "background of the speech window"
        );
        assert_eq!(
            all,
            [world, hud, fade, desktop].concat(),
            "info windows last"
        );
        session.hud_enabled = false;
        assert!(session.desktop_cmds(&mut ctx, texts).is_empty());
    }

    /// O-361: info windows take no mouse clicks (all mouse slots of vtable `0x0061A3A8` return 0); a click in the
    /// window rectangle goes to the world or to the bar's button below.
    // QUIRK O-361: kept
    #[test]
    fn clicks_go_through_info_frames() {
        use opennml_sim::script::present::Presentation;
        use opennml_ui::coords::{to_screen_rect, to_virtual_x, to_virtual_y};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = toolbar_session(&gpu, &vfs);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut modes = CountingModes::default();
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let (button, msg) = clickable_button(&session);
        let world = [500, 300];
        // One window 168 just above and left of each of the two points (text 5 and 6: the own number).
        let screen = screen_of(session.size);
        for (text, [x, y]) in [(5, world), (6, button)] {
            let event = Presentation::InfoFrame {
                text,
                x: to_virtual_x(screen, x) - 2,
                y: to_virtual_y(screen, y) - 2,
                style: 0,
            };
            session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
        }
        let frames = draw_frames(&session, &mut layer, &vfs);
        assert_eq!(ids(&frames), [6, 5]);
        for ((_, rect), [x, y]) in frames.iter().zip([button, world]) {
            let [l, t, r, b] = to_screen_rect(screen, *rect);
            assert!(
                l <= x && x < r && t <= y && y < b,
                "{rect:?} covers ({x}, {y})"
            );
        }
        // First the button (the mouse over it, one frame, then the click), afterwards the world (a world click changes
        // the selection and with it the buttons).
        move_to(&mut session, &vfs, &mut modes, button);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        left_click(&mut session, &vfs, &mut modes, button, 1_000.0);
        assert_eq!((modes.presses, modes.releases), (0, 0), "not the world");
        assert_eq!(session.last_button, Some(msg), "button under the window");
        left_click(&mut session, &vfs, &mut modes, world, 5_000.0);
        assert_eq!(
            (modes.presses, modes.releases),
            (1, 1),
            "world under the window"
        );
    }

    // ---- Countdown, screen text, control channel --------------------------------------------------------------------

    /// Font cells of the countdown (color white or red with alpha `0xC8`) as `(x, y, color)`.
    fn countdown_quads(cmds: &[DrawCmd]) -> Vec<(i32, i32, u32)> {
        use opennml_ui::hud::countdown::{COUNTDOWN_COLOR, COUNTDOWN_RED};
        cmds.iter()
            .filter_map(|c| match *c {
                DrawCmd::Quad {
                    texture: Some(_),
                    dst,
                    color,
                    ..
                } if color == COUNTDOWN_COLOR || color == COUNTDOWN_RED => {
                    Some((dst[0], dst[1], color))
                }
                _ => None,
            })
            .collect()
    }

    /// re/campaign-ui D6.9 (O-367): the countdown appears only with a registered container (`GE+0x608 != 0`, action 15)
    /// with the value `countdown_s`, right-aligned at y = 20, red below 10 s; without a container nothing, also not in
    /// the control channel.
    // QUIRK O-367: kept
    #[test]
    fn countdown_only_with_a_container() {
        use opennml_ui::hud::countdown::{COUNTDOWN_COLOR, COUNTDOWN_RED};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = layer.context(&vfs, screen_of(session.size));
        let set = |session: &mut GameSession, container: i32, seconds: i32| {
            let mission = &mut session.sim_mut().unwrap().state.script.mission;
            mission.countdown_container = container;
            mission.countdown_s = seconds;
        };
        set(&mut session, 0, 65);
        assert!(countdown_quads(&session.combat_cmds(&mut ctx)).is_empty());
        assert_eq!(session.presentation_state(true).countdown, None);
        set(&mut session, 1, 65);
        let quads = countdown_quads(&session.combat_cmds(&mut ctx));
        assert_eq!(quads.len(), 4, "\"1:05\": {quads:?}");
        assert!(
            quads
                .iter()
                .all(|&(_, y, c)| y == 20 && c == COUNTDOWN_COLOR)
        );
        assert!(quads[0].0 > 900, "right: {quads:?}");
        assert_eq!(
            session.presentation_state(true).countdown.as_deref(),
            Some("1:05")
        );
        set(&mut session, 1, 9);
        let quads = countdown_quads(&session.combat_cmds(&mut ctx));
        assert!(quads.len() == 4 && quads.iter().all(|&(.., c)| c == COUNTDOWN_RED));
        assert_eq!(
            session.presentation_state(true).countdown.as_deref(),
            Some("0:09")
        );
    }

    /// `FUN_00435EE0` part 2: every entry of `SimState::countdowns` stands as `m:ss` at the top left (position `{0,
    /// 0x28}`, at 1024 x 768 `(0, 30)`), white from 10 s, red below; the number is `ms / 1000` (truncating: 999 ms ->
    /// "0:00"). Without an entry no number and `timers` empty; the time limit of game type 4 is in the same list (ID
    /// 0). Script countdown (part 1) and timers (part 2) both draw, the script countdown first.
    // QUIRK D-302: kept
    #[test]
    fn timers_show_the_remaining_time_top_left() {
        use opennml_ui::coords::game_y;
        use opennml_ui::hud::countdown::{COUNTDOWN_COLOR, COUNTDOWN_RED};
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut ctx = layer.context(&vfs, screen_of(session.size));
        let y = game_y(screen_of(session.size), 0x28);
        assert_eq!(y, 30);
        // The timer's number: the cells on the left (the script countdown is on the right at y = 20).
        let timer_quads = |cmds: &[DrawCmd]| -> Vec<(i32, i32, u32)> {
            countdown_quads(cmds)
                .into_iter()
                .filter(|&(x, ..)| x < 200)
                .collect()
        };
        let set = |session: &mut GameSession, ms: Option<i32>| {
            let countdowns = &mut session.sim_mut().unwrap().state.countdowns;
            countdowns.clear();
            if let Some(ms) = ms {
                countdowns.insert(opennml_sim::flags::LEADER_COUNTDOWN, ms);
            }
        };

        set(&mut session, None);
        assert!(timer_quads(&session.combat_cmds(&mut ctx)).is_empty());
        assert!(session.presentation_state(true).timers.is_empty());

        for (ms, text, color) in [
            (59_950, "0:59", COUNTDOWN_COLOR),
            (10_000, "0:10", COUNTDOWN_COLOR),
            (9_999, "0:09", COUNTDOWN_RED),
            (999, "0:00", COUNTDOWN_RED),
            // Time limit of game type 4: 20 minutes, ID 0 like the leader timer.
            (1_200_000, "20:00", COUNTDOWN_COLOR),
        ] {
            set(&mut session, Some(ms));
            let quads = timer_quads(&session.combat_cmds(&mut ctx));
            assert_eq!(quads.len(), text.len(), "\"{text}\": {quads:?}");
            assert!(quads.iter().all(|&(_, qy, c)| qy == y && c == color));
            assert_eq!(quads[0].0, 0, "top left corner");
            assert!(quads.windows(2).all(|w| w[0].0 < w[1].0));
            assert_eq!(session.presentation_state(true).timers, [text]);
        }

        // Script countdown (part 1) and timer (part 2) together: both, the script countdown first.
        set(&mut session, Some(59_950));
        let mission = &mut session.sim_mut().unwrap().state.script.mission;
        mission.countdown_container = 1;
        mission.countdown_s = 65;
        let all = countdown_quads(&session.combat_cmds(&mut ctx));
        assert_eq!(all.len(), 8, "\"1:05\" and \"0:59\": {all:?}");
        assert!(all[..4].iter().all(|&(x, qy, _)| x > 900 && qy == 20));
        assert!(all[4..].iter().all(|&(x, qy, _)| x < 200 && qy == y));
        let report = session.presentation_state(true);
        assert_eq!(report.countdown.as_deref(), Some("1:05"));
        assert_eq!(report.timers, ["0:59"]);
    }

    /// re/campaign-ui D6.4/D6.9/D6.12: screen text and countdown draw with the world at the end of
    /// [`GameSession::combat_cmds`], the screen text before the countdown (`0x00438F2A` before `0x00438F74`), both
    /// below the whole UI (status bar, bars, info windows; O-367) and also in the cinemascope. The checksum stays.
    // QUIRK O-367, O-373: kept
    #[test]
    fn countdown_draws_with_the_world_below_the_hud() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let mission = &mut session.sim_mut().unwrap().state.script.mission;
        mission.countdown_container = 1;
        mission.countdown_s = 65;
        // Text 987,654 is missing in the table: the own number appears, white with shadow at (0, H/2).
        let event = Presentation::ScreenText {
            text: 987_654,
            seconds: 3,
        };
        session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
        let before = session.checksum();
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        for cinemascope in [false, true] {
            if cinemascope {
                let event = Presentation::Cinemascope { on: true };
                session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
                assert!(session.hud.cinemascope.bar > 0, "bars");
            }
            let mut ctx = layer.context(&vfs, screen_of(session.size));
            let texts = ctx.texts;
            assert_eq!(texts.get(987_654), None);
            let all = session.over_world_cmds(&mut ctx, texts);
            let world = session.combat_cmds(&mut ctx);
            let hud = session.hud_cmds(&mut ctx, texts);
            let fade = session.fade_cmds(ctx.screen);
            let desktop = session.desktop_cmds(&mut ctx, texts);
            assert!(!hud.is_empty(), "UI there ({cinemascope})");
            assert_eq!(all, [world.clone(), hud, fade, desktop].concat());
            // The countdown is the last four commands of the world.
            let tail = countdown_quads(&world[world.len() - 4..]);
            assert_eq!(tail.len(), 4, "countdown last ({cinemascope})");
            let text_at = world
                .iter()
                .position(|c| {
                    matches!(*c, DrawCmd::Quad { texture: Some(_), dst, color: 0xFFFF_FFFF, .. }
                        if dst[1] == 384)
                })
                .expect("screen text at H/2");
            assert!(text_at < world.len() - 4, "text before the countdown");
        }
        assert_eq!(session.checksum(), before, "presentation only");
    }

    /// `opennml-game` draws the floating numbers (digits of the dynamite charge, credits) in the world pass with the screen
    /// texts (`FUN_00493B80` -> `FUN_004926D0` -> `FUN_004931A0`), before the countdown: in `0xC8C8C8` at the truncated
    /// pixel (text at x + 2), after 1,000 ms of the bar's game time higher with alpha 127, from 2,000 ms nothing more.
    /// The checksum stays.
    // QUIRK D-244: kept
    #[test]
    fn floats_draw_in_the_world_pass_of_the_game() {
        use crate::game::notices::FLOAT_MS;
        use crate::game::world_effects::DIGIT_COLOR;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let mission = &mut session.sim_mut().unwrap().state.script.mission;
        mission.countdown_container = 1;
        mission.countdown_s = 65;
        let now = session.clock.now_ms;
        let params =
            session
                .map
                .view_params(session.size, session.view.aspect.into(), false, now as f32);
        let view_proj = params.projection * params.view;
        // A world point on the ray of the image center.
        let center = view_proj.inverse().project_point3(Vec3::new(0.0, 0.0, 0.5));
        let bar = session.hud.bar_ms;
        session
            .map
            .notices
            .push_float(center, "9".into(), DIGIT_COLOR, bar, true);
        let [x, y] = crate::game::notices::device_project(view_proj, center, session.size).unwrap();
        let (x, y) = (x as i32, y as i32);
        let before = session.checksum();
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let digit = |session: &GameSession, layer: &mut crate::ui_layer::UiLayer| {
            let mut ctx = layer.context(&vfs, screen_of(session.size));
            let world = session.combat_cmds(&mut ctx);
            let at = world.iter().position(|c| {
                matches!(*c, DrawCmd::Quad { texture: Some(_), color, .. }
                    if color & 0x00FF_FFFF == DIGIT_COLOR)
            });
            at.map(|i| {
                assert!(i < world.len() - 4, "digit before the countdown");
                assert_eq!(countdown_quads(&world[world.len() - 4..]).len(), 4);
                let DrawCmd::Quad { dst, color, .. } = world[i] else {
                    panic!("digit as quad");
                };
                let DrawCmd::Quad {
                    dst: shadow_dst,
                    color: shadow_color,
                    ..
                } = world[i - 1]
                else {
                    panic!("shadow as quad");
                };
                assert_eq!(
                    (shadow_dst, shadow_color),
                    (
                        [dst[0] - 2, dst[1] + 2, dst[2] - 2, dst[3] + 2],
                        color & 0xFF00_0000
                    ),
                    "shadow before"
                );
                (dst[0], dst[1], color)
            })
        };
        assert_eq!(
            digit(&session, &mut layer),
            Some((x + 2, y, 0xFF00_0000 | DIGIT_COLOR))
        );
        session.hud.bar_ms = bar + 1000;
        let (_, later_y, color) = digit(&session, &mut layer).expect("still there after 1 s");
        assert!(later_y < y, "risen");
        assert_eq!(color, 0x7F00_0000 | DIGIT_COLOR);
        session.hud.bar_ms = bar + FLOAT_MS;
        assert_eq!(digit(&session, &mut layer), None, "gone after 2,000 ms");
        assert_eq!(session.checksum(), before, "presentation only");
    }

    /// The floating numbers run like the screen texts of the same list on the message bar's game time
    /// ([`crate::game::hud::Hud::bar_ms`], sum of `dt` of the tick `FUN_00494A00`). In pause
    /// ([`GameSession::advance_paused`]) clock, position and alpha stand still; at speed 19 (factor 29/20) the number
    /// is gone after 87 frames of 16 ms = 1,392 ms of frame time (sum of `dt` 2,018 >= 2,000; after 86 frames 1,995),
    /// not only after 2,000 ms.
    // QUIRK D-244: kept
    #[test]
    fn floats_run_on_the_bar_clock() {
        use crate::game::world_effects::DIGIT_COLOR;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        session.sim_mut().unwrap().game_speed = 19;
        let params = session.map.view_params(
            session.size,
            session.view.aspect.into(),
            false,
            session.clock.now_ms as f32,
        );
        let view_proj = params.projection * params.view;
        // The camera target on the ground: far enough in front of the camera that the rising number stays in the image.
        let center = session.map.camera.target;
        let start = session.hud.bar_ms;
        session
            .map
            .notices
            .push_float(center, "9".into(), DIGIT_COLOR, start, true);
        let shown = |s: &GameSession| {
            s.map
                .notices
                .float_texts(view_proj, s.size, s.hud.bar_ms)
                .into_iter()
                .filter(|f| f.text == "9" && f.color & 0x00FF_FFFF == DIGIT_COLOR)
                .collect::<Vec<_>>()
        };
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let first = shown(&session);
        assert_eq!(session.hud.bar_ms - start, 23, "16 * 29 * 5 / 100 = 23.2");
        assert_eq!(first.len(), 1);
        // Pause: 10 s of frame time, the bar's clock and the number stand still.
        for _ in 0..625 {
            session.advance_paused(&vfs, 16.0, None);
        }
        assert_eq!(session.hud.bar_ms - start, 23, "pause");
        assert_eq!(shown(&session), first, "unchanged in pause");
        let mut frame_ms = 16.0;
        while !shown(&session).is_empty() {
            assert!(frame_ms < 3_000.0, "number does not expire");
            session.advance_by(&gpu, &vfs, 16.0, true, None);
            frame_ms += 16.0;
        }
        assert_eq!(frame_ms, 1_392.0, "~ 2,000 / 1.45");
        assert!(session.hud.bar_ms - start >= crate::game::notices::FLOAT_MS);
    }

    /// re/campaign-ui D6.12: the screen texts run on the message bar's game time (`FUN_00494A00` from `0x0043D1DB`): in
    /// pause and with the game menu open (no tick of the match) the text including alpha stands as long as you like; on
    /// resuming it ends after the remainder of its duration. Speed 10 = program time.
    // QUIRK O-373: kept
    #[test]
    fn screen_text_stands_in_pause() {
        use crate::game::hud::{Hud, HudOptions};
        use opennml_sim::script::present::Presentation;
        let messages = opennml_formats::messages::parse_messages("");
        let mut hud = Hud::new(
            &messages,
            &std::collections::BTreeMap::new(),
            &HudOptions::default(),
            opennml_ui::coords::Screen {
                width: 1024,
                height: 768,
            },
        );
        let mut clock = SessionClock::default();
        let frame = |hud: &mut Hud, clock: &mut SessionClock, now: u64, running: bool| {
            if let Some(dt) = clock.frame_ms(now, running) {
                hud.tick(dt, 10, &mut Vec::new());
            }
        };
        let event = Presentation::ScreenText {
            text: 7,
            seconds: 2,
        };
        let start = 1_000_000;
        frame(&mut hud, &mut clock, start, true);
        screen_text_event(&mut hud.screen_texts, &event);
        frame(&mut hud, &mut clock, start + 1_000, true);
        let shown = hud.screen_texts.entries()[0].clone();
        assert_eq!(
            (shown.kind, shown.text, shown.ms, shown.elapsed, shown.alpha),
            (2, 7, 2_000, 1_000, 127)
        );
        assert_eq!(shown.color, 0x00FF_FFFF);
        // Pause or game menu: 60 s of program clock, the text stands.
        for now in (start + 2_000..=start + 61_000).step_by(1_000) {
            frame(&mut hud, &mut clock, now, false);
        }
        assert_eq!(hud.screen_texts.entries(), [shown]);
        // Resuming: 999 ms later it still stands, 1 ms after that it is gone.
        frame(&mut hud, &mut clock, start + 61_999, true);
        assert_eq!(hud.screen_texts.entries()[0].elapsed, 1_999);
        frame(&mut hud, &mut clock, start + 62_000, true);
        assert!(hud.screen_texts.entries().is_empty());
        // Loading and restart clear the list (not in the savegame).
        screen_text_event(&mut hud.screen_texts, &event);
        hud.clear();
        assert!(hud.screen_texts.entries().is_empty());
    }

    /// `ui_state.game.presentation`: the measured info windows with rectangle in the grid (head first), the countdown
    /// as `m:ss` and the text ID of the screen text while it stands.
    #[test]
    fn ui_state_reports_info_frames_and_countdown() {
        use opennml_sim::script::present::Presentation;
        let Some((vfs, device, queue)) = corpus_gpu() else {
            return;
        };
        let gpu = gpu_of(&device, &queue);
        let mut session = campaign_session(&gpu, &vfs);
        session.advance_by(&gpu, &vfs, 16.0, true, None);
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        for (text, x) in [(5, 100), (6, 600)] {
            let event = Presentation::InfoFrame {
                text,
                x,
                y: 100,
                style: 0,
            };
            session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
        }
        let unmeasured = session.presentation_state(true);
        assert!(unmeasured.info_frames.is_empty(), "not drawn yet");
        let frames = draw_frames(&session, &mut layer, &vfs);
        assert_eq!(ids(&frames), [6, 5]);
        let mission = &mut session.sim_mut().unwrap().state.script.mission;
        mission.countdown_container = 1;
        mission.countdown_s = 42;
        let event = Presentation::ScreenText {
            text: 7,
            seconds: 1,
        };
        session.present_by_hand(&gpu, &vfs, None, &event).unwrap();
        let report = session.presentation_state(true);
        assert_eq!(report.info_frames, frames);
        assert_eq!(report.countdown.as_deref(), Some("0:42"));
        assert_eq!(report.screen_text, Some(7));
        // After its duration (game time of the bar) the screen text is gone.
        session.hud.tick(1_000.0, 10, &mut Vec::new());
        assert_eq!(session.presentation_state(true).screen_text, None);
    }
}
