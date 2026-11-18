//! View and control settings of the game modules (light, weather, aspect ratio,
//! camera control, game speed, [`ViewOptions`]): independent of the
//! viewer's settings file. Its `config.rs` repeats the types for TOML and the command line
//! and builds `ViewOptions` from them (`Config::view_options`).

use std::str::FromStr;

use opennml_world::camera::AspectMode;

use crate::game::hud::HudOptions;
use serde::Deserialize;

/// `GameSpeed` default of the settings (`Engine.ini`).
pub const DEFAULT_GAME_SPEED: u8 = 14;

/// Display name of the level light.
pub const LEVEL_LIGHT_NAME: &str = "level";

/// Aspect ratio of the projection (see `opennml_world::camera::AspectMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AspectSetting {
    /// Undistorted.
    #[default]
    Correct,
    /// The original's visual impression at 4:3 (stretch 4/3 in x).
    Original43,
    /// Literally as in the original (`AspectRatio` 1: stretch by width/height).
    Stretch,
}

impl AspectSetting {
    /// Order for the F4 key.
    pub fn next(self) -> Self {
        match self {
            Self::Correct => Self::Original43,
            Self::Original43 => Self::Stretch,
            Self::Stretch => Self::Correct,
        }
    }
}

impl From<AspectSetting> for AspectMode {
    fn from(setting: AspectSetting) -> Self {
        match setting {
            AspectSetting::Correct => AspectMode::Correct,
            AspectSetting::Original43 => AspectMode::Original43,
            AspectSetting::Stretch => AspectMode::Stretch,
        }
    }
}

impl FromStr for AspectSetting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "correct" => Ok(Self::Correct),
            "original43" => Ok(Self::Original43),
            "stretch" => Ok(Self::Stretch),
            other => Err(format!(
                "unknown aspect ratio \"{other}\" (allowed: correct, original43, stretch)"
            )),
        }
    }
}

/// Light at the start of the map.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(try_from = "String")]
pub enum LightSetting {
    /// As in the game: start light of the level script (`level_start_environment`), otherwise `.wld`.
    #[default]
    Start,
    /// Plain values of the `.wld`.
    Level,
    /// A preset from `lightdata.ini` (name is case-insensitive).
    Preset(String),
}

impl FromStr for LightSetting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "" => Err("empty light name (allowed: start, level or a preset)".into()),
            "start" => Ok(Self::Start),
            "level" => Ok(Self::Level),
            _ => Ok(Self::Preset(s.to_string())),
        }
    }
}

impl TryFrom<String> for LightSetting {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Weather at the start of the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(try_from = "String")]
pub enum WeatherSetting {
    /// As in the game: start weather of the level script.
    #[default]
    Start,
    Off,
    Rain,
    Snow,
}

impl FromStr for WeatherSetting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "start" => Ok(Self::Start),
            "none" => Ok(Self::Off),
            "rain" => Ok(Self::Rain),
            "snow" => Ok(Self::Snow),
            other => Err(format!(
                "unknown weather \"{other}\" (allowed: start, none, rain, snow)"
            )),
        }
    }
}

impl TryFrom<String> for WeatherSetting {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Camera control like the same-named values of `Engine.ini` (defaults = the user's
/// installation).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Controls {
    /// Speed when scrolling at the screen edge: `(n + 1) * 0.1`.
    pub mouse_scroll: u32,
    /// Speed when scrolling with keys: `(n + 1) * 0.1`.
    pub keyboard_scroll: u32,
    /// Scroll when the mouse pointer is at most 3 pixels from the edge.
    pub enable_mouse_scroll: bool,
    /// Locks rotating the camera (default in the shipped data: on).
    pub lock_camera: bool,
    /// Mouse wheel zoom.
    pub enable_zoom: bool,
    /// Double click: maximum distance in ms between two mouse button releases
    /// (`Engine.ini DoubleClickTimeout`, `EngineConfig+0x7C`).
    pub double_click_ms: u32,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            mouse_scroll: 10,
            keyboard_scroll: 10,
            enable_mouse_scroll: true,
            lock_camera: true,
            enable_zoom: true,
            double_click_ms: 200,
        }
    }
}

/// View settings of a match (map view, frame loop, voice lines): the subset of the
/// viewer settings that the game modules read; defaults like the viewer's `Config::default()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewOptions {
    /// Light at the start of the map.
    pub light: LightSetting,
    /// Weather at the start of the map.
    pub weather: WeatherSetting,
    /// `TerrainQuality` 0-2 (from 2, water in detail texture mode).
    pub terrain_quality: u8,
    /// `ShadowQuality` 0-3.
    pub shadow_quality: u8,
    /// `ParticlesQuality` 0-2.
    pub particles_quality: u8,
    /// Window width at start, by which the font of the map view is chosen.
    pub text_width: u32,
    /// Aspect ratio of the projection.
    pub aspect: AspectSetting,
    /// Distance fog.
    pub fog: bool,
    /// Play animations.
    pub animate: bool,
    /// Unit voice lines in response to commands (`UnitResponse`).
    pub unit_response: bool,
    /// Game UI from `Engine.ini` (`TimePerChar`, window switch).
    pub hud: HudOptions,
    /// `BackColor` of `Engine.ini` (`EngineConfig+0xF8`, color `0x..RRGGBB`): clear color of the world in
    /// cinemascope without fog (E-245); default 0.
    pub back_color: u32,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            light: LightSetting::Start,
            weather: WeatherSetting::Start,
            terrain_quality: 0,
            shadow_quality: 2,
            particles_quality: 2,
            text_width: 1280,
            aspect: AspectSetting::Correct,
            fog: false,
            animate: true,
            unit_response: true,
            hud: HudOptions::default(),
            back_color: 0,
        }
    }
}
