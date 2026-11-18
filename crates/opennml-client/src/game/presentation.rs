//! Presentation of the scripts in the host: the host converts the presentation events of the
//! simulation (`SimEvent::Present`, [`Presentation`]) into its channels and restores them after loading from the
//! presentation state (`MissionState::presentation`). Shared by viewer and `opennml-game`.
//!
//! Channels (re/script-actions, re/campaign-ui 4, re/weather-and-light, re/audio):
//! - **Light** 171 (preset, case-insensitive; unknown -> only logged) and 381 (only the object light back to the
//!   `.wld`, E-236); an explicit setting of the viewer (`--light`) takes precedence.
//! - **Weather** 209/210 (1 rain, 2 snow, 0 off; other numbers do not change the image); `--weather` takes precedence.
//! - **Music** 9/370/371 via the music manager, **2D sound** 507 (flags `0x44`, sound effects).
//! - **Distance fog** 501/502 (on/off), 505 (immediately), 506 (crossfade in game time with swapped red and
//!   blue, E-237) as its own state of the map view ([`ScriptFog`]).
//! - **View mode** 503/504 and cinemascope via [`SimHost::set_fog_shown`] (`m_FogShowMode` 7 or 0, E-169).
//! - **Cinemascope** 147/148: game speed 10 and tempo memory (E-238), clear the host's selection, bars off
//!   ([`ScriptPresentation::cinemascope`]); bars and letterbox bars switch the UI messages of `FUN_00430920` on the
//!   game UI (`toolbar_host::presentation_events`; letterbox bars only in the campaign, O-350).
//! - **Lock** 18/19: `UiLock { on: true }` without cinemascope clears the host's selection; the lock of the
//!   map inputs is read by [`crate::game_session::GameSession`] from the state (`MissionState::ui_locked`, E-246).
//! - **Quicksave** 508 and **profile variables** 382 as requests to the flow ([`HostRequest`]).
//!
//! Portraits, dialog lines and tips go as UI messages to the cinemascope area (`GameSession::advance_by`;
//! after loading `GameSession::restore_presentation`). Fade, info window, ping and highlight
//! are not drawn by the host; it keeps the highlight (only for an existing object) and the
//! last of these events for the display in the viewer.
//! The **script camera** (2, 141-144, 170, 172, 175-178, 212, 216 and percentage forms) is kept by
//! [`ScriptPresentation::camera`] ([`ScriptCamera`]): jumps at the event, cut, moves, rotation and
//! binding per frame in [`ScriptPresentation::update`].
//!
//! The host changes the simulation only via the game speed (`SimHost::game_speed`) and the view mode (host state);
//! the checksum stays untouched.

use std::collections::VecDeque;

use opennml_audio::engine::AudioEngine;
use opennml_audio::volume::Category;
use opennml_formats::lvl::LevelFile;
use opennml_formats::wld::FogParams;
use opennml_rda::Vfs;
use opennml_sim::id::ObjectId;
use opennml_sim::script::MissionState;
use opennml_sim::script::present::{FogMemory, Presentation, PresentationMemory};
use opennml_world::environment::{
    MusicAction, WeatherKind, level_game_mode, level_start_environment, local_player_nation,
};

use crate::game::map::MapView;
use crate::game::options::{LightSetting, ViewOptions, WeatherSetting};
use crate::game::script_camera::ScriptCamera;
use crate::game::selection::Selection;
use crate::game::sim_host::SimHost;

/// Game speed in cinemascope (`m_GameSpeed = 10`, i.e. 1x game time, `FUN_00430920`).
pub const CINEMASCOPE_SPEED: u8 = 10;
/// Flags of the 2D sound of 507 (`SoundManager_PlaySoundGuid2D`, `0x44`: sound effect without loop).
pub const SOUND_2D_FLAGS: u32 = 0x44;
/// This many display events are kept by [`ScriptPresentation::recent`].
pub const RECENT_EVENTS: usize = 32;
/// `1/255` of the original (`0.003921569`, `FUN_0042F200`).
const BYTE_SCALE: f32 = 0.003_921_569;

/// Request of a presentation event to the flow of the match: `opennml-game` writes the savegame or
/// the profile (`GameFlow::frame`), the viewer only logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRequest {
    /// 508: quicksave under `AUTOSAVENAME` of the campaign entry.
    QuickSave,
    /// 382: profile variable `name` (lowercase) = `value` (E-166).
    ProfileVar { name: String, value: i32 },
}

/// Fog values like `Camera+0x84/0x7C/0x80`: color as D3D ARGB, start and length in world units like `[FOG]`
/// (`FOG_START` behind the camera target, `FOG_END` as length from the start;
/// [`opennml_world::camera::GameCamera::fog_range`] converts them into distances from the eye).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FogValues {
    pub color: u32,
    pub start: f32,
    pub end: f32,
}

impl FogValues {
    /// Values of the `.wld` (`[FOG]`: color `0xFFRRGGBB`) or of the camera constructor (color 0, start `default_start`,
    /// length 1000 like `fog_range`).
    pub fn of_level(fog: Option<&FogParams>, default_start: f32) -> Self {
        let byte = |c: i32| (c as u32) & 0xFF;
        Self {
            color: fog.map_or(0, |f| {
                0xFF00_0000 | byte(f.color[0]) << 16 | byte(f.color[1]) << 8 | byte(f.color[2])
            }),
            start: fog.and_then(|f| f.start).unwrap_or(default_start),
            end: fog.and_then(|f| f.end).unwrap_or(1000.0),
        }
    }

    /// Target values of a state ([`FogMemory`]): 505 takes the script number, the end of a crossfade 506
    /// (`swap_red_blue`) has red and blue swapped and alpha `FF` (E-237); start and end as `f32` (`FILD`).
    pub fn of_memory(fog: &FogMemory) -> Self {
        let raw = fog.color as u32;
        let color = if fog.swap_red_blue {
            0xFF00_0000 | (raw & 0xFF) << 16 | (raw & 0xFF00) | (raw >> 16) & 0xFF
        } else {
            raw
        };
        Self {
            color,
            start: fog.start as f32,
            end: fog.end as f32,
        }
    }

    /// As a fog block for the camera (`fog_range`): color R, G, B from `0x..RRGGBB`.
    pub fn params(&self) -> FogParams {
        let byte = |shift: u32| ((self.color >> shift) & 0xFF) as i32;
        FogParams {
            color: [byte(16), byte(8), byte(0)],
            start: Some(self.start),
            end: Some(self.end),
        }
    }
}

/// Distance fog of the scripts in the map view: `enabled` from 501/502 (`None` = switch of the view,
/// `m_EnableFog` of the application), `values` from 505/506 (`None` = values of the `.wld`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScriptFog {
    pub enabled: Option<bool>,
    pub values: Option<FogValues>,
}

/// Running crossfade 506 (`FUN_0042F200`/`FUN_0042F380`, `GE+0x2D38…+0x2D58`): three color channels in 0..1, their
/// steps per ms, the steps of start and length and the remaining time. Not in the savegame (deviation: loading
/// jumps to the target value).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FogFade {
    /// `GE+0x2D44/48/4C`: byte 0, 1, 2 of the source color.
    channels: [f32; 3],
    /// `GE+0x2D38/3C/40`.
    channel_steps: [f32; 3],
    /// `GE+0x2D50`, `GE+0x2D54`.
    start_step: f32,
    end_step: f32,
    /// `GE+0x2D58`.
    rest_ms: f32,
}

impl FogFade {
    /// Begins a crossfade from `current` to the script values in `ms > 0` milliseconds (`FUN_0042F200`): the
    /// source color bytewise (byte 0 = lowest), steps per ms to the bytes of the script color.
    pub fn new(current: FogValues, color: i32, start: i32, end: i32, ms: i32) -> Self {
        let ms = ms as f32;
        let byte = |c: u32, i: u32| ((c >> (8 * i)) & 0xFF) as f32 * BYTE_SCALE;
        let channels = [0, 1, 2].map(|i| byte(current.color, i));
        let target = [0, 1, 2].map(|i| byte(color as u32, i));
        Self {
            channels,
            channel_steps: [0, 1, 2].map(|i| (target[i] - channels[i]) / ms),
            start_step: (start as f32 - current.start) / ms,
            end_step: (end as f32 - current.end) / ms,
            rest_ms: ms,
        }
    }

    /// One frame with `dt_game_ms` of game time (`FUN_0042F380`): `dt = min(dt, rest)`, channels advance, then the
    /// color `0xFF | channel 0 << 16 | channel 1 << 8 | channel 2` - byte 0 lands in red, so red and blue swap (E-237,
    /// `0x0042F3EA`); start and length run linearly. `true` if the crossfade is finished.
    pub fn advance(&mut self, values: &mut FogValues, dt_game_ms: f32) -> bool {
        let dt = dt_game_ms.min(self.rest_ms);
        for (k, step) in self.channels.iter_mut().zip(self.channel_steps) {
            *k += dt * step;
        }
        // `ftol` truncates; `& 0xFF` as in the original.
        let byte = |k: f32| ((k * 255.0) as i32 as u32) & 0xFF;
        values.color = 0xFF00_0000
            | byte(self.channels[0]) << 16
            | byte(self.channels[1]) << 8
            | byte(self.channels[2]);
        values.start += dt * self.start_step;
        values.end += dt * self.end_step;
        self.rest_ms -= dt;
        self.rest_ms <= 0.0
    }
}

/// What the channels of an event need: the map view including the simulation (`map.sim`), the selection of the
/// host, the device queue (light), the audio system (without it music and sounds stay silent) and the archives.
pub struct PresentCtx<'a> {
    pub map: &'a mut MapView,
    pub selection: &'a mut Selection,
    pub queue: &'a wgpu::Queue,
    pub audio: Option<&'a mut AudioEngine>,
    pub vfs: &'a Vfs,
}

/// Presentation state of the scripts in the host: cinemascope ("bars off"), tempo memory, running
/// fog crossfade, highlight and the last display events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScriptPresentation {
    /// Cinemascope after the events (147/148) or the state: bars off.
    pub cinemascope: bool,
    /// Tempo memory `GE+0x4D4` (E-238): 147 remembers the game speed, 148 restores it; `None` until the first 147 of
    /// the match (148 then sets 0). In the savegame of `opennml-game` (`HostPart::cinemascope_speed`).
    pub tempo_memory: Option<u8>,
    /// Running crossfade 506.
    pub fog_fade: Option<FogFade>,
    /// Target highlight 152/155 (`GameWorld+0x318`), only for an existing object; drawn by the campaign UI.
    pub highlight: Option<ObjectId>,
    /// The last display events (fade, portrait, line, tip, info window, screen text, ping,
    /// highlight), at most [`RECENT_EVENTS`], oldest first.
    pub recent: VecDeque<Presentation>,
    /// Script camera (host state; in the savegame of `opennml-game` as `HostPart::script_camera`).
    pub camera: ScriptCamera,
    /// Explicit light setting of the view (`--light` != `start`): 171/381 have no effect.
    light_fixed: bool,
    /// Explicit weather setting of the view (`--weather` != `start`): 209/210 have no effect.
    weather_fixed: bool,
    /// A [`Self::restore`] without audio system: the music part follows in the next frame with sound.
    audio_pending: bool,
    /// Collected requests to the flow ([`Self::take_requests`]).
    requests: Vec<HostRequest>,
}

/// Weather kind of a script number (`GameEngine_SetWeather`): `Some(None)` off (0), rain (1), snow (2); other
/// numbers change only `+0x4F4` in the original -> `None`.
pub fn weather_of(kind: i32) -> Option<Option<WeatherKind>> {
    match kind {
        0 => Some(None),
        1 => Some(Some(WeatherKind::Rain)),
        2 => Some(Some(WeatherKind::Snow)),
        _ => None,
    }
}

/// Game time of a frame (`GameEngine_Update`: frame time * `(m_GameSpeed + 10) / 20`).
pub fn game_ms(dt_ms: f32, game_speed: u8) -> f32 {
    (f64::from(dt_ms) * SimHost::game_time_factor(game_speed)) as f32
}

/// Music actions of a state in the order threshold, combat music, script music: after loading the music manager runs
/// with status 1 as soon as 370 or 371 have taken effect, and the normal file starts anew (E-239).
pub fn memory_music(memory: &PresentationMemory) -> Vec<MusicAction> {
    let mut actions = Vec::new();
    if let Some(points) = memory.attack_threshold {
        actions.push(MusicAction::AttackStartPoints(points));
    }
    if let Some(id) = memory.attack_music {
        actions.push(MusicAction::Attack(id));
    }
    if let Some(id) = memory.music {
        actions.push(MusicAction::Play(id));
    }
    actions
}

/// Events that the host keeps only for display.
fn displayed(event: &Presentation) -> bool {
    matches!(
        event,
        Presentation::Fade { .. }
            | Presentation::Portrait { .. }
            | Presentation::PortraitHide { .. }
            | Presentation::Highlight { .. }
            | Presentation::Speech { .. }
            | Presentation::SpeechEnd { .. }
            | Presentation::Tip { .. }
            | Presentation::TipEnd { .. }
            | Presentation::InfoFrame { .. }
            | Presentation::InfoSpeechFrame { .. }
            | Presentation::InfoFrameClose { .. }
            | Presentation::ScreenText { .. }
            | Presentation::MinimapPing { .. }
    )
}

impl ScriptPresentation {
    /// New state of a match with the settings of the view (`--light`/`--weather` != `start` take precedence).
    pub fn new(view: &ViewOptions) -> Self {
        Self {
            light_fixed: view.light != LightSetting::Start,
            weather_fixed: view.weather != WeatherSetting::Start,
            ..Self::default()
        }
    }

    /// The requests to the flow since the last call ([`HostRequest`]).
    pub fn take_requests(&mut self) -> Vec<HostRequest> {
        std::mem::take(&mut self.requests)
    }

    /// One event: first the GPU-free part ([`Self::apply_host`]), then light, weather, fog and sound.
    pub fn apply(&mut self, event: &Presentation, ctx: &mut PresentCtx) {
        if let Some(sim) = ctx.map.sim.as_mut() {
            self.apply_host(event, sim, ctx.selection);
        }
        // Script camera: jumps immediately, cut and moves in the frame ([`Self::update`]).
        let map = &mut *ctx.map;
        if let Some(sim) = map.sim.as_ref() {
            self.camera.apply(
                event,
                &mut map.camera,
                &map.scene.world.cameras,
                &map.scene.terrain,
                sim,
            );
        }
        match event {
            Presentation::Light { preset } if !self.light_fixed => script_light(ctx, preset),
            Presentation::LightRestoreLevel if !self.light_fixed => {
                ctx.map.restore_object_light(ctx.queue);
            }
            Presentation::Weather { kind } if !self.weather_fixed => match weather_of(*kind) {
                Some(weather) => ctx.map.set_weather(weather),
                None => log::warn!("Script: unknown weather kind {kind}, the weather remains"),
            },
            Presentation::DistanceFog { on } => ctx.map.script_fog.enabled = Some(*on),
            Presentation::FogValues {
                color,
                start,
                end,
                ms,
                ..
            } => self.set_fog(ctx.map, *color, *start, *end, *ms),
            Presentation::Music { id } => run_music(ctx, MusicAction::Play(*id)),
            Presentation::AttackMusic { id } => run_music(ctx, MusicAction::Attack(*id)),
            Presentation::AttackThreshold { points } => {
                run_music(ctx, MusicAction::AttackStartPoints(*points));
            }
            Presentation::Sound2d { id } => {
                if let Some(audio) = ctx.audio.as_deref_mut() {
                    audio.sound.play_sound_guid_2d(
                        ctx.vfs,
                        *id,
                        SOUND_2D_FLAGS,
                        Category::Sfx,
                        false,
                    );
                }
            }
            _ => {}
        }
    }

    /// GPU-free part of an event (also for the flow's fake): game speed and view mode of the
    /// cinemascope, clearing the host's selection (147/148 always, 18 only without cinemascope), view mode
    /// 503/504, highlight, requests to the flow and the display events.
    pub fn apply_host(
        &mut self,
        event: &Presentation,
        sim: &mut SimHost,
        selection: &mut Selection,
    ) {
        match event {
            // `FUN_00430920`: on remembers the game speed and sets 10 (a second 147 overwrites the memory with 10),
            // off sets the memory (without a previous 147: 0, half speed; E-238); both clear the
            // selection and the highlight, `m_FogShowMode` 7 or 0 (E-169).
            Presentation::Cinemascope { on: true } => {
                self.tempo_memory = Some(sim.game_speed);
                sim.game_speed = CINEMASCOPE_SPEED;
                self.cinemascope = true;
                selection.clear();
                self.highlight = None;
                sim.set_fog_shown(false);
            }
            Presentation::Cinemascope { on: false } => {
                sim.game_speed = self.tempo_memory.unwrap_or(0);
                self.cinemascope = false;
                selection.clear();
                self.highlight = None;
                sim.set_fog_shown(true);
                // The original sets the alpha table here immediately (`0x00479160(1)`); the minimap rebuilds its
                // fog as well.
                sim.request_minimap_rebuild();
            }
            // `{0x19, 1}` clears the selection only with a visible bar (`0x00458510`).
            Presentation::UiLock { on: true } if !self.cinemascope => selection.clear(),
            Presentation::FogShowAll { on } => sim.set_fog_shown(!on),
            Presentation::Highlight { id } => {
                self.highlight = id.filter(|&id| sim.state.object(id, false).is_some());
            }
            Presentation::QuickSave => self.requests.push(HostRequest::QuickSave),
            Presentation::ProfileVar { name, value } => {
                self.requests.push(HostRequest::ProfileVar {
                    name: name.clone(),
                    value: *value,
                })
            }
            _ => {}
        }
        if displayed(event) {
            if self.recent.len() == RECENT_EVENTS {
                self.recent.pop_front();
            }
            self.recent.push_back(event.clone());
        }
    }

    /// One frame with `dt_game_ms` of game time ([`game_ms`]): the script camera (`GameEngine_UpdateCameraFollow`
    /// `0x00432590`, in the original after the ticks of the frame, `0x00445869`) with `camera_dt_game_ms` and the
    /// crossfade 506 (`FUN_0042F380`, once per frame from `GameEngine_UpdateCamera`; no frame comes during the pause)
    /// with `dt_game_ms`. The camera time is larger only in the first frame after resuming the paused menu level
    /// (the pause time, O-447).
    pub fn update(&mut self, dt_game_ms: f32, camera_dt_game_ms: f32, ctx: &mut PresentCtx) {
        let map = &mut *ctx.map;
        if let Some(sim) = map.sim.as_ref() {
            self.camera
                .update(camera_dt_game_ms, &mut map.camera, &map.scene.terrain, sim);
        }
        let Some(fade) = self.fog_fade.as_mut() else {
            return;
        };
        let mut values = ctx.map.fog_values();
        let done = fade.advance(&mut values, dt_game_ms);
        ctx.map.script_fog.values = Some(values);
        if done {
            self.fog_fade = None;
        }
    }

    /// 505 (`ms <= 0`: immediately, color unchanged, a running crossfade aborts) or 506 (crossfade).
    fn set_fog(&mut self, map: &mut MapView, color: i32, start: i32, end: i32, ms: i32) {
        if ms > 0 {
            self.fog_fade = Some(FogFade::new(map.fog_values(), color, start, end, ms));
        } else {
            self.fog_fade = None;
            map.script_fog.values = Some(FogValues::of_memory(&FogMemory {
                color,
                start,
                end,
                swap_red_blue: false,
            }));
        }
    }

    /// After loading (`resume_from`, `reset_view`): all channels from the state `mission` (deviation, E-239) -
    /// cinemascope with game speed 10 and memory `tempo_memory` (`HostPart::cinemascope_speed`; without it, in the
    /// viewer, derived from the state and the cinemascope before loading, see [`Self::restore_host`]), view mode,
    /// highlight, light and weather (only with a level script and without a setting of the view; without a script the
    /// start tables remain), fog on/off and values (a crossfade jumps to the target value) and the music including
    /// combat music and threshold. The script camera starts empty; `resume_from` then sets it from
    /// `HostPart::script_camera` (a deviation), the viewer starts with a free camera. Without audio system the music
    /// follows in the next frame with sound ([`Self::resume_audio`]).
    pub fn restore(
        &mut self,
        mission: &MissionState,
        tempo_memory: Option<u8>,
        ctx: &mut PresentCtx,
    ) {
        let memory = &mission.presentation;
        if let Some(sim) = ctx.map.sim.as_mut() {
            self.restore_host(mission, tempo_memory, sim);
        }
        let scripted = ctx.map.sim.as_ref().is_some_and(SimHost::runs_level_script);
        if scripted && !self.light_fixed {
            let light = memory.light.as_deref().unwrap_or_default();
            if light.is_empty() {
                ctx.map.set_light(ctx.queue, None);
            } else if !ctx.map.set_script_light(ctx.queue, light) {
                log::warn!("Savegame: unknown light preset \"{light}\", the level light remains");
                ctx.map.set_light(ctx.queue, None);
            }
            if memory.object_light_restored {
                ctx.map.restore_object_light(ctx.queue);
            }
        }
        if scripted && !self.weather_fixed {
            ctx.map
                .set_weather(weather_of(memory.weather).unwrap_or_default());
        }
        self.fog_fade = None;
        ctx.map.script_fog = ScriptFog {
            enabled: memory.distance_fog,
            values: memory.fog.as_ref().map(FogValues::of_memory),
        };
        self.audio_pending = true;
        if let Some(audio) = ctx.audio.as_deref_mut() {
            self.start_audio(ctx.map, audio, ctx.vfs);
        }
    }

    /// GPU-free part of [`Self::restore`]: cinemascope, game speed, memory, view mode and highlight; the script camera
    /// starts empty (`resume_from` then sets it from `HostPart::script_camera`, the viewer starts
    /// free, a deviation).
    ///
    /// Game speed and memory (E-238): `tempo_memory` is the memory of the host part (`opennml-game`) and takes precedence.
    /// Without it (viewer after `sim_load`) it follows from the loaded state and the cinemascope before loading: state
    /// with cinemascope -> game speed 10, memory stays (a cinemascope was already running) or becomes the game speed
    /// before loading; state without -> if a cinemascope ran before, the game speed returns to its memory (otherwise it
    /// would stay at 10), the memory is empty.
    pub fn restore_host(
        &mut self,
        mission: &MissionState,
        tempo_memory: Option<u8>,
        sim: &mut SimHost,
    ) {
        let memory = &mission.presentation;
        let previous = self.tempo_memory.filter(|_| self.cinemascope);
        self.cinemascope = mission.cinemascope;
        if mission.cinemascope {
            self.tempo_memory = tempo_memory.or(previous).or(Some(sim.game_speed));
            sim.game_speed = CINEMASCOPE_SPEED;
        } else {
            if let (None, Some(speed)) = (tempo_memory, previous) {
                sim.game_speed = speed;
            }
            self.tempo_memory = tempo_memory;
        }
        self.camera = ScriptCamera::default();
        if let Some(on) = memory.fog_show_all {
            sim.set_fog_shown(!on);
        }
        self.highlight = memory
            .highlight
            .filter(|&id| sim.state.object(id, false).is_some());
        self.recent.clear();
        self.requests.clear();
    }

    /// Is the music part still pending after a [`Self::restore`] without audio system?
    pub fn audio_pending(&self) -> bool {
        self.audio_pending
    }

    /// Catch up the pending music part (in the first frame with sound after loading).
    pub fn resume_audio(&mut self, map: &MapView, audio: &mut AudioEngine, vfs: &Vfs) {
        if self.audio_pending {
            self.start_audio(map, audio, vfs);
        }
    }

    /// Music of the level (re/audio, section "Music at level start"): fresh music manager with the nation music of
    /// the local player ([`music_nation`]); with a level script the music of the state (empty at the start, the scripts
    /// set it), without a script the start music of the table. The game mode follows the folder of the level
    /// ([`level_game_mode`]). After loading the nation comes from the loaded state - the same nation that the original
    /// restores with the music manager (`m_CurNation`) from the savegame.
    pub fn start_audio(&mut self, map: &MapView, audio: &mut AudioEngine, vfs: &Vfs) {
        self.audio_pending = false;
        let lvl = &map.scene.lvl_path;
        let actions = match map.sim.as_ref().filter(|s| s.runs_level_script()) {
            Some(sim) => memory_music(&sim.state.script.mission.presentation),
            None => level_start_environment(lvl).music.to_vec(),
        };
        audio.start_level(
            vfs,
            level_game_mode(lvl),
            music_nation(map.sim.as_ref(), &map.scene.level),
            &actions,
        );
    }

    /// The local player has changed (viewer: key P, `sim_view active_player`): nation music of the new player
    /// like `GameWorld_CmdSwitchLocalPlayer` (`0x0047B510`) -> `GameEngine_SetLocalPlayer` (`0x00430140`) ->
    /// `Game_StartNationMusic` (`0x0042EF90`), without a fresh music manager.
    pub fn switch_nation_music(&self, map: &MapView, audio: &mut AudioEngine, vfs: &Vfs) {
        audio.start_nation_music(
            vfs,
            level_game_mode(&map.scene.lvl_path),
            music_nation(map.sim.as_ref(), &map.scene.level),
        );
    }
}

/// Nation of the nation music: `Player+0x238` of the local player (`[GameEngine_GetLocalPlayer()+0x238]`, `0x00430153`
/// in `GameEngine_SetLocalPlayer`, `0x004C7828` in `Game_FinishLevelSetup`), read after
/// `GameEngine_SetupPlayersFromLobby` (`0x00432C00`): in free play the choice in the start dialog, in the campaign the
/// nation of the `.lvl`. In the reimplementation the nation of the active player of the simulation
/// (`SimHost::active_player`); outside 0..5 none (`Game_StartNationMusic` then does nothing). Only without simulation
/// (map without loadable rules) the static reading of the `.lvl` ([`local_player_nation`]).
pub fn music_nation(sim: Option<&SimHost>, level: &LevelFile) -> Option<usize> {
    match sim {
        Some(sim) => sim
            .state
            .players
            .get(usize::from(sim.active_player))
            .and_then(|p| usize::try_from(p.nation).ok())
            .filter(|&n| n < 6),
        None => local_player_nation(level),
    }
}

/// Light setting 171: an unknown preset (also the placeholder `fehlerhafte_parameterliste` of the parser)
/// changes nothing and is only logged (`GameEngine_ApplyLightPreset`).
fn script_light(ctx: &mut PresentCtx, preset: &str) {
    if !ctx.map.set_script_light(ctx.queue, preset) {
        log::warn!("Script: unknown light preset \"{preset}\", the light remains");
    }
}

/// A music action of the scripts (`AudioEngine::run_action`); without audio system nothing.
fn run_music(ctx: &mut PresentCtx, action: MusicAction) {
    if let Some(audio) = ctx.audio.as_deref_mut() {
        audio.run_action(ctx.vfs, action);
    }
}

#[cfg(test)]
mod tests;
