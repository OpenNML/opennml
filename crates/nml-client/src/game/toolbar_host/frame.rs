//! Integration of the command bar into the match: distribution of the mouse, frame step and world rendering (build
//! preview, target point decal, defense ring).
//!
//! # Per frame
//!
//! 1. **Input** between the frames ([`GameSession::toolbar_input`] from `GameSession::input_with`): every raw mouse
//!    input goes to the window tree first ([`Hud::input`], UI pixels). If a window takes it, the world does not see it
//!    (O-222); the host applies the outputs of the bar (last button, intents -> commands at the next tick boundary,
//!    sounds, decal) at once. An open cursor of the bar takes the clicks in the world and so replaces the modes of the
//!    host.
//! 2. **Frame step** at the end of `GameSession::advance_by` ([`GameSession::toolbar_step`]): cursor frame (world
//!    point, snapping, target validity), snapshot of the match, the events of the host (`ToolbarHost::frame_events`:
//!    `{8}`, `{0x10, n}`, rows, `{0x29}`), tooltip request of the areas of the last drawing, then frame event 6
//!    (`Toolbar::update`), then the verdict for exactly the new position of the cursor and the build preview.
//! 3. **Drawing**, read-only: world pass with preview and decals (`GameSession::mark_ground`), then the game windows
//!    (`Hud::draw_windows`, tooltip last).
//!
//! With the game menu open (the match is not running) the session passes nothing to the bar, and the frame step rests
//! (`advance_by` returns early): buttons, object under the mouse and tooltip stay as they are until the game runs again
//! (the game menu covers the bar). During the pause, on the other hand, the bar takes clicks, and its frame step runs
//! in the pause step (`GameSession::advance_paused`; in the original event 6 comes once per frame from the UI,
//! `0x004BB16D`); the simulation executes its commands at once (O-318).

use glam::Vec3;
use nml_audio::engine::AudioEngine;
use nml_formats::gifcon::button_kind as kinds;
use nml_rda::Vfs;
use nml_render::WorldDecal;
use nml_sim::command::validate::button_locked_in;
use nml_sim::id::ObjectId;
use nml_ui::geom::contains;
use nml_ui::hud::HudEventKind;
use nml_ui::hud::toolbar::{
    ButtonSet, Cursor, CursorFrame, ToolbarEvent, ToolbarKeys, ToolbarOut, ToolbarSound,
};
use nml_ui::hud::tree::WindowKind;
use winit::keyboard::KeyCode;

use crate::game::economy::PlacementPreview;
use crate::game::notices::SoundRequest;
use crate::game::pointer::{PointerArea, PointerChoice, PointerCursor, PointerFrame};
use crate::game_session::{GameSession, Pointer, SessionGpu, play_sim_sound};
use crate::keys::{InputEvent, MouseButton};

use super::{TargetDecal, active_target_buttons, cursor_frame, toolbar_model};

/// Texture of the target point (`0x00619F18`, loaded in the constructor of the bar `0x00456630` into `+0xC4`).
pub const TARGET_DECAL_TEXTURE: &str = "data/icons/zielpunkt.tga";
/// Texture of the defense ring (`0x00618DF8`, `GameEngine_RenderDefenceRadiusMarker` `0x0042F750`).
pub const DEFENSE_RING_TEXTURE: &str = "data/textures/selektion/defenceradius.tga";
/// Duration of the target point in program time (`GetTickCount() + 300`, re/game-ui N.5.8).
pub const TARGET_DECAL_MS: f64 = 300.0;
/// Color of the target point (`0xFFFFFFFF`, `0x004574B0`) as RGBA.
const TARGET_DECAL_COLOR: [u8; 4] = [0xFF; 4];
/// Color of the defense ring `0x800064C8` (ARGB) as RGBA.
const DEFENSE_RING_COLOR: [u8; 4] = [0x00, 0x64, 0xC8, 0x80];
/// Radius of the defense ring in tile widths (`GameEngine+0x31C` from `0x00450070`).
const DEFENSE_RING_TILES: f32 = 6.0;

/// A running target point (`Toolbar+0x5C0...+0x5C8`, end `+0xC8`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveTargetDecal {
    pub decal: TargetDecal,
    /// End in program time (ms, [`crate::game_session::SessionClock::program_ms`]).
    pub until_ms: f64,
}

/// Half side length of the target point at program time `now_ms` (`0x004574B0`): `rest * 0.0033333334 * 32 + 1` with
/// `rest = end - now` in `f32`, so shrinking from 33 to 1; `None` once the 300 ms are up (`GetTickCount() < +0xC8`).
pub fn target_decal_size(until_ms: f64, now_ms: f64) -> Option<f32> {
    (now_ms < until_ms).then_some((until_ms - now_ms) as f32 * 0.003_333_333_4_f32 * 32.0 + 1.0)
}

/// Keys of the bar from the pointer (`KD_ADD_TO_GROUP`, `KD_SHIFT_BUTTON`, `KD_CONTROL_BUTTON`, `KD_ALT_BUTTON`).
/// Deviation: the reimplementation reads the defaults as fixed (Shift, Ctrl, Alt) like the selection of the world
/// (`Pointer::shift`), not the freely assignable keys of the settings.
fn toolbar_keys(pointer: &Pointer) -> ToolbarKeys {
    let alt = pointer.keys.contains(&KeyCode::AltLeft) || pointer.keys.contains(&KeyCode::AltRight);
    ToolbarKeys {
        add_to_group: pointer.shift(),
        shift: pointer.shift(),
        control: pointer.ctrl(),
        alt,
    }
}

/// Window pixels of the pointer -> UI pixels: in the viewer the area of the pointer can differ from the surface, in
/// `nml-game` both are equal. The selection box calculates the same way (`GameSession::selection_rect`).
pub(crate) fn surface_point(p: (f64, f64), size: (u32, u32), screen: (u32, u32)) -> [i32; 2] {
    let scale = |v: f64, s: u32, w: u32| {
        if w == 0 {
            v
        } else {
            v * f64::from(s) / f64::from(w)
        }
    };
    [
        scale(p.0, size.0, screen.0).floor() as i32,
        scale(p.1, size.1, screen.1).floor() as i32,
    ]
}

/// Raw mouse kinds of an input for the window tree (`0x004B90BD`): movement 1, left 3/2 (+`0x13` on a double-click),
/// right 5/4 (+`0x12`), wheel `0x2D`/`0x2E`. Keys and leaving the window do not go to the windows.
fn raw_kinds(ev: &InputEvent, double: bool) -> Vec<HudEventKind> {
    use HudEventKind as K;
    match *ev {
        InputEvent::CursorMoved { .. } => vec![K::MouseMove],
        InputEvent::Button {
            button: MouseButton::Left,
            pressed: true,
        } => vec![K::LeftDown],
        InputEvent::Button {
            button: MouseButton::Left,
            pressed: false,
        } => {
            if double {
                vec![K::LeftUp, K::DoubleLeft]
            } else {
                vec![K::LeftUp]
            }
        }
        InputEvent::Button {
            button: MouseButton::Right,
            pressed: true,
        } => vec![K::RightDown],
        InputEvent::Button {
            button: MouseButton::Right,
            pressed: false,
        } => {
            if double {
                vec![K::RightUp, K::DoubleRight]
            } else {
                vec![K::RightUp]
            }
        }
        InputEvent::Wheel { up } => vec![if up { K::WheelUp } else { K::WheelDown }],
        _ => Vec::new(),
    }
}

impl GameSession {
    /// The windows of the game UI (command bar, minimap) take input: game UI on (`hud_enabled`; `--hud` in the viewer),
    /// the match is running (no game menu; the pause does not stop it) and there is a simulation. A bar is no longer
    /// needed: with `IsMap` and without `IsToolBar` the minimap alone takes input. The bar learns of the visible pause
    /// through `ToolbarModel::visible_pause` ([`crate::game::pause::PauseState`]): build buttons then have no effect
    /// (O-231).
    pub(crate) fn toolbar_active(&self) -> bool {
        self.hud_enabled && self.running && self.map.sim.is_some()
    }

    /// One input first to the window tree (see the module header); `double` is the result of the double-click count
    /// of the release (`clicks.release`, as `FUN_004145B0` for every release). `true` if a window took it: then it does
    /// not belong to the world. The session remembers the pointer position anyway (edge scrolling, object under the
    /// mouse); a started selection box or right-click of the world lapses if a window takes the counterpart. The
    /// release of a running selection box does not come here: it belongs to the root before all windows (O-385,
    /// `GameSession::input_with`).
    pub(crate) fn toolbar_input(
        &mut self,
        ev: &InputEvent,
        double: bool,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) -> bool {
        self.toolbar_raws(ev, raw_kinds(ev, double), vfs, audio)
    }

    /// Only the left double-click (`0x13`) of the release `ev` to the window tree: with a running selection box the
    /// root takes the release itself before all windows (O-385), `0x13` then goes the usual way.
    pub(crate) fn toolbar_double_left(
        &mut self,
        ev: &InputEvent,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) -> bool {
        self.toolbar_raws(ev, vec![HudEventKind::DoubleLeft], vfs, audio)
    }

    /// [`Self::toolbar_input`] with the raw kinds `raws` of the input `ev`.
    fn toolbar_raws(
        &mut self,
        ev: &InputEvent,
        raws: Vec<HudEventKind>,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) -> bool {
        if !self.toolbar_active() {
            return false;
        }
        // Keys and leaving the window do not go to the windows: then build neither model nor target list (both
        // read-only, without outputs there is nothing to apply).
        if raws.is_empty() {
            return false;
        }
        let point = match *ev {
            InputEvent::CursorMoved { x, y } => Some((x, y)),
            _ => self.pointer.cursor,
        };
        let Some(point) = point else {
            return false;
        };
        let at = surface_point(point, self.size, self.screen);
        let gate = self.input_gate();
        let mut outs = Vec::new();
        let mut clicks = Vec::new();
        let mut taken = false;
        {
            let Some(sim) = self.map.sim.as_mut() else {
                return false;
            };
            let targets = active_target_buttons(sim, &self.selection.ids);
            let mut model = toolbar_model(sim, &self.selection.ids, self.pause.shown);
            model.targets = targets;
            model.keys = toolbar_keys(&self.pointer);
            model.formation = self.toolbar_host.bar.formation();
            for raw in raws {
                taken |= self
                    .hud
                    .input(raw, at, &model, gate, &mut outs, &mut clicks);
            }
        }
        if taken {
            match *ev {
                InputEvent::CursorMoved { x, y } => self.pointer.cursor = Some((x, y)),
                InputEvent::Button {
                    button: MouseButton::Left,
                    pressed,
                } => {
                    self.pointer.left = pressed;
                    self.select_start = None;
                }
                InputEvent::Button {
                    button: MouseButton::Right,
                    pressed,
                } => {
                    self.pointer.right = pressed;
                    self.right_start = None;
                    self.right_drag = None;
                }
                _ => {}
            }
        }
        // Effects of the minimap in sequence of the raw kinds (right double-click: 4, 4, `0x12` -> three commands,
        // O-40).
        for click in clicks {
            self.minimap_click(click);
        }
        self.apply_toolbar_outputs(outs, vfs, audio);
        taken
    }

    /// Bar shortcuts from `keys.ini` (re/game-ui D5.3, O-309): event 9 to the bar like a left click (`0x186AA` ->
    /// [`ToolbarEvent::Shortcut`] -> click path `FUN_00458130`), only with a visible bar (a hidden bar does not take
    /// event 9, re/game-ui N.1). The host applies the outputs at once like those of a click.
    ///
    /// - `checked` (`button(n)`, `FUN_0044B090`): query `0x186A9` (`FUN_00452FB0`) searches the 14 normal slots
    ///   (`+0x20C`) for the first row with `BUTTONMSG n`, even without an icon; if it is missing or locked
    ///   (`Toolbar_IsButtonLocked` `0x0044AF60` including `REQUIRED_GUID_NATION`, [`button_locked_in`]), nothing.
    ///   Otherwise `{BUTTONMSG, kind}` of the row.
    /// - otherwise (`xbutton(n)`, `FUN_0044B1B0`): directly `{n, DO}`, without a button and lock check.
    ///
    /// Unlike the mouse, the shortcut needs no running match: keys reach `keys.ini` even with the game menu open
    /// (as in the original).
    pub(crate) fn toolbar_shortcut(
        &mut self,
        n: i32,
        checked: bool,
        vfs: &Vfs,
        audio: Option<&mut AudioEngine>,
    ) {
        if !self.hud_enabled {
            return;
        }
        let (Some(sim), Some(toolbar)) = (self.map.sim.as_mut(), self.hud.toolbar.as_mut()) else {
            return;
        };
        if !toolbar.shown {
            return;
        }
        let event = if checked {
            let Some(row) = toolbar.set.slots.iter().flatten().find(|r| r.msg == n) else {
                return;
            };
            let slot = i32::from(sim.active_player);
            if button_locked_in(&sim.state, &sim.rules, slot, row) {
                return;
            }
            ToolbarEvent::Shortcut {
                msg: row.msg,
                kind: row.flags,
            }
        } else {
            ToolbarEvent::Shortcut {
                msg: n,
                kind: kinds::DO,
            }
        };
        let mut outs = Vec::new();
        {
            let targets = active_target_buttons(sim, &self.selection.ids);
            let mut model = toolbar_model(sim, &self.selection.ids, self.pause.shown);
            model.targets = targets;
            model.keys = toolbar_keys(&self.pointer);
            model.formation = self.toolbar_host.bar.formation();
            toolbar.press(event, &model, &mut outs);
        }
        self.apply_toolbar_outputs(outs, vfs, audio);
    }

    /// Apply the outputs of the bar: last button and intents through [`GameSession::toolbar_outputs`] (`execute`
    /// already applies the selection sound and camera of the selection buttons), then the sounds of the bar (click
    /// sound `SFX_KLICK` with flags `0x14`, category sound effects like the menu host; selection sound of the full
    /// rebuilds through [`crate::game::voices::Voices::selection`]) and the target point decal.
    pub(crate) fn apply_toolbar_outputs(
        &mut self,
        outs: Vec<ToolbarOut>,
        vfs: &Vfs,
        mut audio: Option<&mut AudioEngine>,
    ) {
        if outs.is_empty() {
            return;
        }
        // Only the intents need the button set (`execute`); without them no clone.
        let intents = outs.iter().any(|o| matches!(o, ToolbarOut::Intent(_)));
        let groups = match self.hud.toolbar.as_ref() {
            Some(t) if intents => t.set.clone(),
            _ => ButtonSet::default(),
        };
        let fx = self.toolbar_outputs(outs, &groups, vfs, audio.as_deref_mut());
        for sound in fx.sounds {
            match sound {
                ToolbarSound::Click => {
                    if let Some(audio) = audio.as_deref_mut() {
                        let name = "SFX_KLICK".to_string();
                        play_sim_sound(audio, vfs, SoundRequest::Event2d { name });
                    }
                }
                ToolbarSound::Select => {
                    let Some(sim) = self.map.sim.as_ref() else {
                        continue;
                    };
                    let request =
                        self.voices
                            .selection(&sim.state, &self.selection.ids, sim.active_player);
                    if let (Some(request), Some(audio)) = (request, audio.as_deref_mut()) {
                        self.voices.play(audio, vfs, request);
                    }
                }
            }
        }
        if let Some(decal) = fx.target_decal {
            self.start_target_decal(decal);
        }
    }

    /// Set a target point (event `0xFA {3, x, y, z}` to the bar: position to `+0x5C0`, end `GetTickCount() + 300`).
    pub(crate) fn start_target_decal(&mut self, decal: TargetDecal) {
        self.target_decal = Some(ActiveTargetDecal {
            decal,
            until_ms: self.clock.program_ms + TARGET_DECAL_MS,
        });
    }

    /// The mouse pointer of the match (re/game-ui D5.2): ID and variant, last set by [`GameSession::pointer_step`], the
    /// sending of a right-click or the game menu. Menus and dialogs above it are chosen by the flow
    /// (`GameFlow::pointer_choice`).
    pub fn pointer_choice(&self) -> PointerChoice {
        self.mouse_pointer.choice
    }

    /// Position of the pointer for the frame step ([`crate::game::pointer::PointerState::step`]): minimap (rectangle of
    /// the node, also hidden: `0x0044A4C0` does not check the visibility), bar (rectangle, also hidden, `0x00453790`),
    /// otherwise world; the open cursor of the bar with pointer and verdict of the targeting mode; an open right-drag
    /// of the world. Without a game UI (viewer without `--hud`) everything is world.
    fn pointer_frame(&self) -> PointerFrame {
        let mouse = self
            .pointer
            .cursor
            .map(|c| surface_point(c, self.size, self.screen));
        let toolbar = self.hud.toolbar.as_ref().filter(|_| self.hud_enabled);
        let windows = &self.hud.windows;
        let minimap = windows.find(WindowKind::Minimap).map(|id| windows.rect(id));
        let inside =
            |r: Option<[i32; 4]>| r.zip(mouse).is_some_and(|(r, [x, y])| contains(r, x, y));
        let area = if !self.hud_enabled {
            PointerArea::World
        } else if inside(minimap) {
            PointerArea::Minimap
        } else if inside(toolbar.map(|t| t.rect())) {
            PointerArea::Toolbar
        } else {
            PointerArea::World
        };
        let cursor = match toolbar.and_then(|t| t.cursor.as_ref()) {
            None => PointerCursor::None,
            Some(Cursor::Build(_)) => PointerCursor::Build {
                valid: self.toolbar_previews.first().map(|p| p.valid),
                software: self.software_pointer,
            },
            Some(Cursor::Fort(_)) => PointerCursor::Wall,
            Some(Cursor::Defense(_)) => PointerCursor::Defense,
            Some(Cursor::Target(t)) => PointerCursor::Target {
                icon: t.mouse_icon,
                valid: t.valid,
            },
        };
        PointerFrame {
            area,
            cursor,
            right_drag: self.pointer.right && self.right_start.is_some(),
        }
    }

    /// Pointer frame step after the frame step of the bar (see [`crate::game::pointer`]): the pointer of the world is
    /// [`crate::game::sim_host::SimHost::hover_pointer`] for the right-click target under the mouse
    /// (`MapView::pick_right_click_target`, ranks of `0x00480E80`), 7 without a mouse or simulation. Does not change
    /// the simulation.
    pub(crate) fn pointer_step(&mut self) {
        let frame = self.pointer_frame();
        let cursor = self.pointer.cursor;
        let size = self.screen;
        let params = self.pick_params();
        let ids = &self.selection.ids;
        let map = &mut self.map;
        self.mouse_pointer.step(&frame, || {
            let Some(c) = cursor else {
                return nml_formats::gifcon::POINTER_NORMAL;
            };
            let target = map.pick_right_click_target(&params, c, size, ids);
            map.sim
                .as_mut()
                .map_or(nml_formats::gifcon::POINTER_NORMAL, |sim| {
                    sim.hover_pointer(ids, target)
                })
        });
    }

    /// Frame step of the bar (see the module header), once per frame at the end of `advance_by` or of the pause step
    /// (`GameSession::advance_paused`), only under the conditions of [`GameSession::toolbar_active`] without the
    /// running condition (both run only without the game menu).
    pub(crate) fn toolbar_step(&mut self, vfs: &Vfs, audio: Option<&mut AudioEngine>) {
        if !self.hud_enabled || self.hud.toolbar.is_none() || self.map.sim.is_none() {
            // If the step rests, there is no verdict and no preview of the bar cursor (otherwise the preview of the
            // last frame would stay after `hud {on:false}`).
            self.toolbar_previews.clear();
            return;
        }
        let mouse = self
            .pointer
            .cursor
            .map(|c| surface_point(c, self.size, self.screen));
        let world = self.pointer.cursor.and_then(|c| self.ground_at(c));
        let picked = self.hovered;
        let ids = self.selection.ids.clone();
        let width = self.hud.screen().width;
        let (Some(sim), Some(toolbar)) = (self.map.sim.as_mut(), self.hud.toolbar.as_mut()) else {
            return;
        };
        let mouse = mouse.unwrap_or(toolbar.mouse_pos);
        // Cursor frame before the step: world point, snapped point with validity, target validity
        // (`FUN_004515B0`).
        let frame = toolbar.cursor.as_ref().map(|c| {
            let mut f = cursor_frame(sim, c, world, mouse, &ids, picked);
            if let Cursor::Target(t) = c {
                f.valid = sim.button_target_valid(t.msg, &ids, picked);
            }
            f
        });
        let mut outs = Vec::new();
        let targets = active_target_buttons(sim, &ids);
        {
            let mut model = toolbar_model(sim, &ids, self.pause.shown);
            model.targets = targets;
            model.keys = toolbar_keys(&self.pointer);
            model.formation = self.toolbar_host.bar.formation();
            model.cursor = frame;
            let mut events = self.toolbar_host.frame_events.clone();
            // New screen width: heroes and headhunters hang on the right edge (O-35); create the same rows anew
            // (the original builds the tree only at the start of the match).
            if self.toolbar_width.is_some_and(|w| w != width) {
                events.extend(self.toolbar_host.row_events(sim));
            }
            self.toolbar_width = Some(width);
            for e in &events {
                toolbar.event(e, &model, &mut outs);
            }
            // Tooltip: frame start, then the areas of the last drawing (`0x00453B50`), then the buttons (the last
            // wins, O-44, re/game-ui N.14.6). When hidden the bar draws nothing, so there are no areas.
            self.hud.tooltip.begin_frame();
            // The tree draws the corner buttons of the minimap before the bar (children of the minimap): their
            // request comes first, areas and buttons of the bar win afterwards.
            self.hud
                .minimap_buttons
                .update(&mut self.hud.tooltip.request);
            if toolbar.shown {
                if let Some(r) = self.hud.areas.borrow().hit(toolbar.mouse_pos) {
                    self.hud.tooltip.set(r);
                }
            } else {
                self.hud.areas.borrow_mut().clear();
            }
            toolbar.update(&model, &mut self.hud.tooltip.request);
        }
        // Verdict for exactly the position after the step, in the same frame; plus the build preview.
        let (valid, ring_valid, previews) =
            cursor_verdict(sim, toolbar.cursor.as_ref(), &ids, frame);
        toolbar.set_cursor_validity(valid, ring_valid);
        self.toolbar_previews = previews;
        self.hud.sync_toolbar_visibility();
        self.apply_toolbar_outputs(outs, vfs, audio);
    }

    /// World rendering of the bar for the frame (from `mark_ground`): build preview of the bar cursor (it takes
    /// precedence over a preview of the viewer; cleared once without a cursor), target point decal (O-237) and defense
    /// ring. Only with a drawn game UI (the target point belongs to the drawing routine of the bar `0x004574B0`);
    /// without it the world stays as before the bar was integrated: the build preview also disappears at
    /// once instead of waiting for the next frame step (the viewer can be stopped).
    pub(crate) fn toolbar_world(&mut self, gpu: &SessionGpu, vfs: &Vfs) {
        let now = self.clock.program_ms;
        if !self.hud_enabled {
            self.toolbar_previews.clear();
        }
        if self
            .target_decal
            .is_some_and(|d| target_decal_size(d.until_ms, now).is_none())
        {
            self.target_decal = None;
        }
        if !self.toolbar_previews.is_empty() || self.previews_shown {
            self.map
                .set_build_previews(gpu.device, gpu.queue, vfs, &self.toolbar_previews);
            self.previews_shown = !self.toolbar_previews.is_empty();
        }
        let decals = if self.hud_enabled {
            self.world_decals(now)
        } else {
            Vec::new()
        };
        if !decals.is_empty() || self.decals_shown {
            self.map
                .set_world_decals(gpu.device, gpu.queue, vfs, &decals);
            self.decals_shown = !decals.is_empty();
        }
    }

    /// World point `(x, z)` of the defense ring ([`nml_ui::hud::toolbar::Toolbar::defense_ring`]).
    fn defense_ring(&self) -> Option<[f32; 2]> {
        self.hud.toolbar.as_ref().and_then(|t| t.defense_ring())
    }

    /// The decals of the frame without an object: defense ring (step 6 of `GameEngine_RenderWorld`), then the target
    /// point (drawing routine of the bar). Ground targets (`y = 0`) as a terrain decal (`TerrainDecal_DrawSquare`),
    /// object targets flat at their height (`0x0054BFA0`); the switch is `y == 0`, as in the original.
    fn world_decals(&self, now: f64) -> Vec<WorldDecal> {
        let mut out = Vec::new();
        let terrain = &self.map.scene.terrain;
        if let (Some([x, z]), Some(sim)) = (self.defense_ring(), self.map.sim.as_ref()) {
            out.push(WorldDecal {
                texture: DEFENSE_RING_TEXTURE.into(),
                position: Vec3::new(x, terrain.ground_height(x, z), z),
                radius: sim.rules.map.cell[0] as f32 * DEFENSE_RING_TILES,
                color: DEFENSE_RING_COLOR,
                flat: false,
            });
        }
        if let Some(d) = self.target_decal
            && let Some(radius) = target_decal_size(d.until_ms, now)
        {
            out.push(WorldDecal {
                texture: TARGET_DECAL_TEXTURE.into(),
                position: Vec3::from_array(d.decal.pos),
                radius,
                color: TARGET_DECAL_COLOR,
                flat: d.decal.pos[1] != 0.0,
            });
        }
        out
    }
}

/// Verdict for the position of the cursor after the step and the build preview (`Cursor3D_RenderWorldModel`):
/// - Build/tent: if the position is the one just checked (snapped world point of the frame, no rotating), its verdict
///   applies; otherwise exactly the position and rotation of the cursor is checked without snapping again. Preview: one
///   model.
/// - Wall: per part `BuildCursor_CheckWallSegments` (`0x00450870`), ring valid if all parts are valid; per part a
///   preview with its own verdict.
/// - Target, defense: the verdict of the frame (both have no position of their own).
fn cursor_verdict(
    sim: &mut crate::game::sim_host::SimHost,
    cursor: Option<&Cursor>,
    selection: &[ObjectId],
    frame: Option<CursorFrame>,
) -> (bool, Option<bool>, Vec<PlacementPreview>) {
    let frame = frame.unwrap_or_default();
    match cursor {
        Some(Cursor::Build(b)) => {
            let bits = |p: [f32; 3]| p.map(f32::to_bits);
            let checked = !b.dragging && frame.world.map(bits) == Some(bits(b.pos));
            let valid = if checked {
                frame.valid
            } else {
                let tent: Vec<ObjectId> = b.tent.map(ObjectId).into_iter().collect();
                let workers = if b.tent.is_some() {
                    &tent[..]
                } else {
                    selection
                };
                sim.placement_pose_valid(b.guid, b.pos, b.rotation, workers)
            };
            let preview = frame
                .world
                .is_some()
                .then(|| PlacementPreview::model(b.guid, b.pos, b.rotation, valid));
            (valid, None, preview.into_iter().collect())
        }
        Some(Cursor::Fort(f)) => {
            let parts: Vec<(u32, [f32; 3], f32)> = f
                .parts
                .iter()
                .map(|p| (p.guid, p.pos, p.rotation))
                .collect();
            let each = sim.wall_cursor_parts_valid(&parts, !f.big_enough());
            let ring = (!each.is_empty()).then(|| each.iter().all(|&v| v));
            let previews = parts
                .iter()
                .zip(&each)
                .map(|(&(guid, pos, rot), &v)| PlacementPreview::model(guid, pos, rot, v))
                .collect();
            (false, ring, previews)
        }
        Some(Cursor::Target(_) | Cursor::Defense(_)) => (frame.valid, None, Vec::new()),
        None => (false, None, Vec::new()),
    }
}
