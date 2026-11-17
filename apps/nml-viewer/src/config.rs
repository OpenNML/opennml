//! Settings: TOML file, overridden by the command line.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nml_client::game::hud::HudOptions;
use nml_client::game::sim_host::SimHost;
use nml_client::window::WindowMode;
use nml_control::{SlotSetup, SlotSetupKind};
use nml_formats::campaign::{CampaignTable, EntryKind};
use nml_formats::sound::nation_index;
use nml_randommap::RandomMapSpec;
use nml_render::BackendChoice;
use nml_sim::ki::KiSetup;
use nml_sim::rules::Rules;
use nml_sim::setup::{MatchSetup, SlotKind, ki_kind_handicap};
use serde::Deserialize;

pub use nml_client::game::options::{
    AspectSetting, Controls, DEFAULT_GAME_SPEED, LightSetting, ViewOptions, WeatherSetting,
};

/// Highest value of the volume controls (`m_Volumes`, resolution 1000).
pub const MAX_VOLUME: u16 = 1000;

/// Largest values of the quality levels (`Engine.ini`).
pub const MAX_SHADOW_QUALITY: u8 = 3;
pub const MAX_PARTICLES_QUALITY: u8 = 2;
pub const MAX_TERRAIN_QUALITY: u8 = 2;

/// Largest `GameSpeed` of the settings (`Engine.ini`).
pub const MAX_GAME_SPEED: u8 = 30;

/// Largest resource setting (`resourcenconfig.ini` has levels 1-3) and living space level
/// (0-5); equal to the limits of the control channel (`sim_setup`).
pub const MAX_RESOURCE_SETTING: i32 = nml_control::MAX_RESOURCE_SETTING;
pub const MAX_LIVING_SPACE: i32 = nml_control::MAX_LIVING_SPACE;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub backend: BackendChoice,
    pub window_mode: WindowMode,
    /// Monitor index (0-based); without a value, the monitor the window is on.
    pub monitor: Option<usize>,
    /// Logical window size in windowed mode (also the size of screenshots).
    pub width: u32,
    pub height: u32,
    /// Game directory; without a value as in `nml-extract` (`NML_GAME_DIR`, `../original/...`).
    pub game_dir: Option<PathBuf>,
    /// Level as the game path of the `.lvl`.
    pub level: String,
    /// Random map instead of `level` (only `--random-map`, not in the file): preview without simulation.
    #[serde(skip)]
    pub random_map: Option<RandomMapSpec>,
    pub aspect: AspectSetting,
    /// Fog (`m_EnableFog`; switched by an event in the game, default off).
    pub fog: bool,
    /// Objects on the map play their idle animation (F5).
    pub animate: bool,
    /// `Engine.ini ShadowQuality` 0-3.
    pub shadow_quality: u8,
    /// `Engine.ini ParticlesQuality` 0-2.
    pub particles_quality: u8,
    /// `Engine.ini TerrainQuality` 0-2; from 2 on, water in detail texture mode.
    pub terrain_quality: u8,
    /// Light at the start (F6 switches to the next one).
    pub light: LightSetting,
    /// Weather at the start (F7 switches to the next one).
    pub weather: WeatherSetting,
    /// `Engine.ini m_Volumes` (0-1000): music, sound effects, speech.
    pub volume_music: u16,
    pub volume_sfx: u16,
    pub volume_speech: u16,
    /// No device output (F9 toggles); mixer and recording keep running.
    pub mute: bool,
    /// Game UI (`--hud`, command `hud`): message bar and status bar through the UI renderer as in
    /// `nml-game` instead of the message lines in the overlay. Default off.
    pub hud: bool,
    /// `Engine.ini UnitResponse` (original 1): selection voice of the units and command acknowledgment.
    pub unit_response: bool,
    /// `Engine.ini GameSpeed` 0-30: game time = clock * (n + 10) / 20 (14 = 1.2 times).
    pub game_speed: u8,
    /// Game type of the match (-1 campaign, 0 normal, 1 kill hero, 3 King of the Hill, 4/5 Railroad); without a
    /// value 0 on multiplayer maps, otherwise -1.
    pub game_mode: Option<i32>,
    /// Start index of the random sequence of the simulation (the original starts at 0).
    pub seed: u32,
    /// Nations of the lobby slots in order (`WID`, `PRI`, `SPA`, `SID`, `ENG`, `PAT`); empty =
    /// those of the `.lvl`.
    pub nations: Vec<String>,
    /// Teams of the lobby slots in order; empty = each slot its own team.
    pub teams: Vec<i32>,
    /// Resource setting of the match: 0 = starting balance of the map, 1-3 = levels of
    /// `resourcenconfig.ini` (`GameEngine_ApplyResourceConfigStartResources`).
    pub resource_setting: i32,
    /// Living space level 0-5 = 75, 100, 125, 150, 175, 200 (default 2 = 125).
    pub living_space: i32,
    /// Map explored (`MapExplored`): "explored" fully set for all players at the start.
    pub map_explored: bool,
    /// Lobby slots in order (kind, nation, team, color, handicap or AI kind; protocol 26);
    /// excludes `nations` and `teams`. Empty = default of the map (all slots human).
    pub slots: Vec<SlotSetup>,
    /// Start the AI of the AI slots (`KiSetup::enabled`; default on in the viewer).
    pub ki_enabled: bool,
    /// Campaign difficulty 0-2 (`KiSetup::campaign_difficulty`, default 1).
    pub campaign_difficulty: i32,
    /// Start the level script (`ScriptSetup::enabled`; default off in the viewer; `--mission` switches it on).
    pub script_enabled: bool,
    /// Speaking time per character without a speech file in ms, 1-1000 (`ScriptSetup::time_per_char`, default 55).
    pub time_per_char: i32,
    /// Time limit in minutes, 1-35791 (`MatchSetup::time_limit`): victory time of the flag game types 2/3 and
    /// time limit of game type 4. Without a value the default of the setup (20 minutes).
    pub time_limit: Option<i32>,
    pub controls: Controls,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: BackendChoice::Auto,
            window_mode: WindowMode::Borderless,
            monitor: None,
            width: 1280,
            height: 720,
            game_dir: None,
            level: DEFAULT_LEVEL.to_string(),
            random_map: None,
            aspect: AspectSetting::Correct,
            fog: false,
            animate: true,
            shadow_quality: 2,
            particles_quality: 2,
            terrain_quality: 0,
            light: LightSetting::Start,
            weather: WeatherSetting::Start,
            volume_music: 349,
            volume_sfx: 599,
            volume_speech: 499,
            mute: false,
            hud: false,
            unit_response: true,
            game_speed: DEFAULT_GAME_SPEED,
            game_mode: None,
            seed: 0,
            nations: Vec::new(),
            teams: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            slots: Vec::new(),
            ki_enabled: true,
            campaign_difficulty: 1,
            script_enabled: false,
            time_per_char: 55,
            time_limit: None,
            controls: Controls::default(),
        }
    }
}

/// Settings file when none is given (relative to the working directory).
pub const DEFAULT_PATH: &str = "nml-viewer.toml";

/// Level that is loaded when none is given.
pub const DEFAULT_LEVEL: &str = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";

/// Loads `path`; if the file is missing, that is an error for `required`, otherwise the defaults apply.
fn load_from(path: &Path, required: bool) -> Result<Config> {
    if !path.exists() {
        if required {
            bail!("settings file {} not found", path.display());
        }
        return Ok(Config::default());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Config::from_toml(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Values from the command line; `None` = take over from the file.
#[derive(Debug, Default)]
pub struct Overrides {
    pub backend: Option<BackendChoice>,
    pub window_mode: Option<WindowMode>,
    pub monitor: Option<usize>,
    pub game_dir: Option<PathBuf>,
    pub level: Option<String>,
    /// `--random-map` and `--random-*`.
    pub random_map: Option<RandomMapSpec>,
    pub aspect: Option<AspectSetting>,
    pub fog: Option<bool>,
    pub shadow_quality: Option<u8>,
    pub particles_quality: Option<u8>,
    pub terrain_quality: Option<u8>,
    pub light: Option<LightSetting>,
    pub weather: Option<WeatherSetting>,
    pub volume_music: Option<u16>,
    pub volume_sfx: Option<u16>,
    pub volume_speech: Option<u16>,
    pub mute: Option<bool>,
    /// `--hud`.
    pub hud: Option<bool>,
    pub game_speed: Option<u8>,
    pub game_mode: Option<i32>,
    pub seed: Option<u32>,
    pub nations: Option<Vec<String>>,
    pub teams: Option<Vec<i32>>,
    pub map_explored: Option<bool>,
    /// Protocol 26 (`--slots`, `--ki`, `--campaign-difficulty`).
    pub slots: Option<Vec<SlotSetup>>,
    pub ki_enabled: Option<bool>,
    pub campaign_difficulty: Option<i32>,
    /// Protocol 28 (`--script`).
    pub script_enabled: Option<bool>,
}

/// A campaign mission from `campaign.ini` for `--mission` and the "Match" window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mission {
    /// Campaign 0 (tutorials) to 3.
    pub campaign: usize,
    /// File stem of the `.lvl` ("kamp1-1"): the name for `--mission`.
    pub name: String,
    /// Game path of the `.lvl` (with `/`).
    pub path: String,
}

impl Mission {
    /// Text of the selection list: "Tutorial: kamp0-1" or "Campaign 1: kamp1-1".
    pub fn label(&self) -> String {
        match self.campaign {
            0 => format!("Tutorial: {}", self.name),
            c => format!("Campaign {c}: {}", self.name),
        }
    }
}

/// All levels of the four campaigns in file order (videos and headlines do not count). The name is the
/// file stem of the path without `.lvl`; `\` in the path counts like `/`.
pub fn campaign_missions(table: &CampaignTable) -> Vec<Mission> {
    let mut missions = Vec::new();
    for (campaign, entries) in table.campaigns.iter().enumerate() {
        for entry in entries
            .iter()
            .filter(|e| e.kind == EntryKind::Level && !e.path.is_empty())
        {
            let path = entry.path.trim().replace('\\', "/");
            let file = path.rsplit('/').next().unwrap_or(&path);
            let name = file
                .rsplit_once('.')
                .filter(|(_, ext)| ext.eq_ignore_ascii_case("lvl"))
                .map_or(file, |(stem, _)| stem);
            missions.push(Mission {
                campaign,
                name: name.to_string(),
                path,
            });
        }
    }
    missions
}

/// The mission named `name` (case-insensitive); unknown: error message with the known
/// names.
pub fn find_mission<'a>(missions: &'a [Mission], name: &str) -> Result<&'a Mission, String> {
    missions
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| {
            if missions.is_empty() {
                format!("Mission \"{name}\" unknown (no campaign levels in campaign.ini)")
            } else {
                let known: Vec<&str> = missions.iter().map(|m| m.name.as_str()).collect();
                format!("Mission \"{name}\" unknown (known: {})", known.join(", "))
            }
        })
}

/// An entry of `--slots` (comma list): `c` closed, `h` human, `a1` ... `a4` AI of kind 1-4 (handicap
/// 5/2/0/0 as in the original, `nml_sim::setup::ki_kind_handicap`), `a:h0` ... `a:h5` AI with free handicap 0-5.
/// Nation and team still come from `--nations`/`--teams` ([`merge_lobby_lists`]).
pub fn parse_slot(text: &str) -> Result<SlotSetup, String> {
    let entry = text.trim().to_ascii_lowercase();
    let slot = |kind, handicap, ki_kind| SlotSetup {
        kind,
        nation: None,
        team: None,
        color: None,
        handicap,
        ki_kind,
    };
    let parsed = match entry.as_str() {
        "c" => Some(slot(SlotSetupKind::Closed, None, None)),
        "h" => Some(slot(SlotSetupKind::Human, None, None)),
        _ => match entry.strip_prefix("a:h") {
            Some(digit) => digit
                .parse::<i32>()
                .ok()
                .filter(|h| digit.len() == 1 && (0..=nml_control::MAX_SLOT_HANDICAP).contains(h))
                .map(|h| slot(SlotSetupKind::Ai, Some(h), None)),
            None => entry
                .strip_prefix('a')
                .and_then(|digit| digit.parse::<u8>().ok().filter(|_| digit.len() == 1))
                .filter(|k| nml_control::KI_KIND_RANGE.contains(k))
                .map(|k| slot(SlotSetupKind::Ai, None, Some(k))),
        },
    };
    parsed.ok_or_else(|| {
        format!("\"{text}\" is not a lobby slot (allowed: c, h, a1 to a4, a:h0 to a:h5)")
    })
}

/// `--ki on|off` (default on).
pub fn parse_ki_switch(text: &str) -> Result<bool, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("\"{text}\" invalid (allowed: on or off)")),
    }
}

/// Enters the nations (`--nations`) and teams (`--teams`) of the command line in order into the slots of
/// `--slots` (on the command line, nation and team stay with `--nations`/`--teams`; in the settings,
/// `slots` and `nations`/`teams` exclude each other). More nations or teams than slots are an error.
pub fn merge_lobby_lists(
    mut slots: Vec<SlotSetup>,
    nations: Option<&[String]>,
    teams: Option<&[i32]>,
) -> Result<Vec<SlotSetup>> {
    for (name, count) in [
        ("--nations", nations.map_or(0, <[String]>::len)),
        ("--teams", teams.map_or(0, <[i32]>::len)),
    ] {
        if count > slots.len() {
            bail!("{name}: {count} entries, --slots only {}", slots.len());
        }
    }
    for (slot, nation) in slots.iter_mut().zip(nations.unwrap_or_default()) {
        let index = nation_index(nation).with_context(|| {
            format!("nation \"{nation}\" unknown (allowed: WID, PRI, SPA, SID, ENG, PAT)")
        })?;
        slot.nation = Some(index as i32);
    }
    for (slot, &team) in slots.iter_mut().zip(teams.unwrap_or_default()) {
        slot.team = Some(team);
    }
    Ok(slots)
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        config.check()?;
        Ok(config)
    }

    /// Checks the value ranges that the type does not express.
    pub fn check(&self) -> Result<()> {
        for (name, value, max) in [
            ("shadow_quality", self.shadow_quality, MAX_SHADOW_QUALITY),
            (
                "particles_quality",
                self.particles_quality,
                MAX_PARTICLES_QUALITY,
            ),
            ("terrain_quality", self.terrain_quality, MAX_TERRAIN_QUALITY),
        ] {
            if value > max {
                bail!("{name} = {value} invalid (allowed: 0 to {max})");
            }
        }
        for (name, value) in [
            ("volume_music", self.volume_music),
            ("volume_sfx", self.volume_sfx),
            ("volume_speech", self.volume_speech),
        ] {
            if value > MAX_VOLUME {
                bail!("{name} = {value} invalid (allowed: 0 to {MAX_VOLUME})");
            }
        }
        if self.game_speed > MAX_GAME_SPEED {
            bail!(
                "game_speed = {} invalid (allowed: 0 to {MAX_GAME_SPEED})",
                self.game_speed
            );
        }
        if !(0..=MAX_RESOURCE_SETTING).contains(&self.resource_setting) {
            bail!(
                "resource_setting = {} invalid (allowed: 0 = map to {MAX_RESOURCE_SETTING})",
                self.resource_setting
            );
        }
        if !(0..=MAX_LIVING_SPACE).contains(&self.living_space) {
            bail!(
                "living_space = {} invalid (allowed: 0 to {MAX_LIVING_SPACE})",
                self.living_space
            );
        }
        for nation in &self.nations {
            if nation_index(nation).is_none() {
                bail!("nation \"{nation}\" unknown (allowed: WID, PRI, SPA, SID, ENG, PAT)");
            }
        }
        if !self.slots.is_empty() && (!self.nations.is_empty() || !self.teams.is_empty()) {
            bail!("slots excludes nations and teams");
        }
        nml_control::check_slots(&self.slots).map_err(anyhow::Error::msg)?;
        if !nml_control::CAMPAIGN_DIFFICULTY_RANGE.contains(&self.campaign_difficulty) {
            bail!(
                "campaign_difficulty = {} invalid (allowed: 0 to 2)",
                self.campaign_difficulty
            );
        }
        if !nml_control::TIME_PER_CHAR_RANGE.contains(&self.time_per_char) {
            bail!(
                "time_per_char = {} invalid (allowed: 1 to 1000 ms per character)",
                self.time_per_char
            );
        }
        if let Some(t) = self
            .time_limit
            .filter(|t| !nml_control::TIME_LIMIT_RANGE.contains(t))
        {
            bail!(
                "time_limit = {t} invalid (allowed: {} to {} minutes)",
                nml_control::TIME_LIMIT_RANGE.start(),
                nml_control::TIME_LIMIT_RANGE.end()
            );
        }
        Ok(())
    }

    /// Loads the explicitly given file (must exist) or, without one, [`DEFAULT_PATH`] in the working
    /// directory (may be missing: then the defaults apply).
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        match explicit {
            Some(path) => load_from(path, true),
            None => load_from(Path::new(DEFAULT_PATH), false),
        }
    }

    /// Sets the lobby lists like `sim_setup` (protocol 26): named lists replace the previous ones; `slots`
    /// clears `nations` and `teams`, `nations` or `teams` clear `slots` (they exclude each other). Shared
    /// path of the command line ([`Self::with_overrides`]), control channel and the "Match" window
    /// (`app::SimSetupArgs::apply`).
    pub fn set_lobby(
        &mut self,
        nations: Option<Vec<String>>,
        teams: Option<Vec<i32>>,
        slots: Option<Vec<SlotSetup>>,
    ) {
        if slots.is_some() {
            self.nations.clear();
            self.teams.clear();
        }
        if nations.is_some() || teams.is_some() {
            self.slots.clear();
        }
        if let Some(nations) = nations {
            self.nations = nations;
        }
        if let Some(teams) = teams {
            self.teams = teams;
        }
        if let Some(slots) = slots {
            self.slots = slots;
        }
    }

    /// Sets up a campaign mission like the campaign start of the game (`match_request::campaign_setup`): level
    /// (instead of a random map), game type -1 without a lobby (`slots`, `nations`, `teams` are dropped), AI and
    /// level script on. The difficulty (`--campaign-difficulty`, default 1) and the speaking time stay.
    pub fn use_mission(&mut self, path: &str) {
        self.level = path.to_string();
        self.random_map = None;
        self.game_mode = Some(-1);
        self.slots.clear();
        self.nations.clear();
        self.teams.clear();
        self.ki_enabled = true;
        self.script_enabled = true;
    }

    /// `--mission <name>`: [`Self::use_mission`] with the level from `campaign.ini`; an explicit
    /// `--script on|off` (`script`) wins over the mission switching it on. Unknown name: error message.
    pub fn with_mission(
        mut self,
        table: &CampaignTable,
        name: &str,
        script: Option<bool>,
    ) -> Result<Self, String> {
        let missions = campaign_missions(table);
        let mission = find_mission(&missions, name)?;
        self.use_mission(&mission.path);
        if let Some(on) = script {
            self.script_enabled = on;
        }
        Ok(self)
    }

    pub fn with_overrides(self, o: Overrides) -> Self {
        let mut config = Self {
            backend: o.backend.unwrap_or(self.backend),
            window_mode: o.window_mode.unwrap_or(self.window_mode),
            monitor: o.monitor.or(self.monitor),
            game_dir: o.game_dir.or(self.game_dir),
            level: o.level.unwrap_or(self.level),
            random_map: o.random_map.or(self.random_map),
            aspect: o.aspect.unwrap_or(self.aspect),
            fog: o.fog.unwrap_or(self.fog),
            shadow_quality: o.shadow_quality.unwrap_or(self.shadow_quality),
            particles_quality: o.particles_quality.unwrap_or(self.particles_quality),
            terrain_quality: o.terrain_quality.unwrap_or(self.terrain_quality),
            light: o.light.unwrap_or(self.light),
            weather: o.weather.unwrap_or(self.weather),
            volume_music: o.volume_music.unwrap_or(self.volume_music),
            volume_sfx: o.volume_sfx.unwrap_or(self.volume_sfx),
            volume_speech: o.volume_speech.unwrap_or(self.volume_speech),
            mute: o.mute.unwrap_or(self.mute),
            hud: o.hud.unwrap_or(self.hud),
            game_speed: o.game_speed.unwrap_or(self.game_speed),
            game_mode: o.game_mode.or(self.game_mode),
            seed: o.seed.unwrap_or(self.seed),
            map_explored: o.map_explored.unwrap_or(self.map_explored),
            ki_enabled: o.ki_enabled.unwrap_or(self.ki_enabled),
            campaign_difficulty: o.campaign_difficulty.unwrap_or(self.campaign_difficulty),
            script_enabled: o.script_enabled.unwrap_or(self.script_enabled),
            ..self
        };
        config.set_lobby(o.nations, o.teams, o.slots);
        config
    }

    /// View settings of the game modules (`MapView::load`, frame loop, voices).
    pub fn view_options(&self) -> ViewOptions {
        ViewOptions {
            light: self.light.clone(),
            weather: self.weather,
            terrain_quality: self.terrain_quality,
            shadow_quality: self.shadow_quality,
            particles_quality: self.particles_quality,
            text_width: self.width,
            aspect: self.aspect,
            fog: self.fog,
            animate: self.animate,
            unit_response: self.unit_response,
            // `Engine.ini` default (`EngineConfig::default()`: `TimePerChar` 55, all windows on); not
            // `Config::time_per_char`, which is the speaking time of the scripts.
            hud: HudOptions::default(),
            // `BackColor` of the `Engine.ini` default (0, black).
            back_color: 0,
        }
    }
}

/// Match setup from the settings: default of the map ([`MatchSetup::default_for`]),
/// on top game type, seed, resource and living space setting (effective only on
/// multiplayer maps, [`nml_sim::setup::new_match`]), "map explored", nations and teams of the
/// lobby slots in order, or instead of them the slots `slots` (protocol 26: kind, nation, team,
/// color, handicap or AI kind -> handicap via [`ki_kind_handicap`]; slots after them stay like
/// the default of the map), the AI settings (`ki_enabled`, `campaign_difficulty`) and the
/// script setup (`script_enabled`, `time_per_char`; protocol 28).
pub fn match_setup(config: &Config, rules: &Rules) -> anyhow::Result<MatchSetup> {
    let mut setup = MatchSetup::default_for(rules);
    setup.seed = config.seed;
    setup.resource_setting = config.resource_setting;
    setup.living_space = config.living_space;
    setup.map_explored = config.map_explored;
    setup.time_limit = config.time_limit.unwrap_or(setup.time_limit);
    if let Some(mode) = config.game_mode {
        if mode >= 0 && setup.lobby.is_empty() {
            bail!("game type {mode} needs a multiplayer map (data/levels/multiplayer/...)");
        }
        if mode < 0 {
            setup.lobby.clear();
        }
        setup.mode = mode;
    }
    if !config.slots.is_empty() && setup.lobby.is_empty() {
        bail!(
            "slots needs a multiplayer map with game type 0 or higher (data/levels/multiplayer/...); the campaign has no lobby"
        );
    }
    for (name, count) in [
        ("nations", config.nations.len()),
        ("teams", config.teams.len()),
        ("slots", config.slots.len()),
    ] {
        if count > setup.lobby.len() {
            bail!(
                "{name}: {count} entries, the lobby has only {} slots",
                setup.lobby.len()
            );
        }
    }
    for (entry, nation) in setup.lobby.iter_mut().zip(&config.nations) {
        let index = nation_index(nation).with_context(|| format!("nation \"{nation}\" unknown"))?;
        entry.nation = index as i32;
    }
    for (entry, &team) in setup.lobby.iter_mut().zip(&config.teams) {
        entry.team = team;
    }
    for (entry, slot) in setup.lobby.iter_mut().zip(&config.slots) {
        entry.kind = match slot.kind {
            SlotSetupKind::Closed => SlotKind::Closed,
            SlotSetupKind::Human => SlotKind::Human,
            SlotSetupKind::Ai => SlotKind::Ai,
        };
        entry.nation = slot.nation.unwrap_or(entry.nation);
        entry.team = slot.team.unwrap_or(entry.team);
        entry.color = slot.color.unwrap_or(entry.color);
        entry.handicap = match slot.ki_kind {
            Some(kind) => ki_kind_handicap(kind)
                .with_context(|| format!("AI kind {kind} unknown (allowed 1 to 4)"))?,
            None => slot.handicap.unwrap_or(entry.handicap),
        };
    }
    setup.ki = KiSetup {
        enabled: config.ki_enabled,
        campaign_difficulty: config.campaign_difficulty,
    };
    setup.script = nml_sim::script::ScriptSetup {
        enabled: config.script_enabled,
        time_per_char: config.time_per_char,
        ..Default::default()
    };
    Ok(setup)
}

/// `sim_setup`: new match according to the settings (`restart`, setup from [`match_setup`]) and
/// game speed. On an error everything stays unchanged (the game speed is taken over only after the
/// successful restart, [`SimHost::apply_setup`]).
pub fn apply_config(host: &mut SimHost, config: &Config, restart: bool) -> Result<(), String> {
    let setup = if restart {
        Some(match_setup(config, &host.rules).map_err(|e| format!("{e:#}"))?)
    } else {
        None
    };
    host.apply_setup(setup, config.game_speed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_client::game::sim_host::plain_testkit::{TYPES, host};
    use nml_world::camera::AspectMode;

    /// The defaults of the view in `nml-client` are those of the settings of the viewer.
    #[test]
    fn default_view_options_are_the_config_defaults() {
        assert_eq!(Config::default().view_options(), ViewOptions::default());
    }

    #[test]
    fn slots_ki_and_difficulty_are_read_and_checked() {
        use nml_control::{SlotSetup, SlotSetupKind};
        let c = Config::from_toml(
            "ki_enabled = true
campaign_difficulty = 0
             [[slots]]
kind = \"human\"
[[slots]]
kind = \"ai\"
nation = 3
ki_kind = 1
",
        )
        .unwrap();
        assert!(c.ki_enabled);
        assert_eq!(c.campaign_difficulty, 0);
        assert_eq!(c.slots.len(), 2);
        assert_eq!(
            c.slots[1],
            SlotSetup {
                kind: SlotSetupKind::Ai,
                nation: Some(3),
                team: None,
                color: None,
                handicap: None,
                ki_kind: Some(1),
            }
        );
        for (text, error) in [
            (
                "campaign_difficulty = 3
",
                "campaign_difficulty = 3",
            ),
            (
                "time_per_char = 0
",
                "time_per_char = 0 invalid",
            ),
            (
                "nations = [\"SPA\"]
[[slots]]
kind = \"human\"
",
                "slots excludes nations and teams",
            ),
            (
                "[[slots]]
kind = \"human\"
ki_kind = 2
",
                "slots[0]: ki_kind only with kind ai",
            ),
        ] {
            let e = format!("{:#}", Config::from_toml(text).unwrap_err());
            assert!(e.contains(error), "{text}: {e}");
        }
    }

    // QUIRK E-40: kept
    #[test]
    fn slots_argument_parses_kinds_and_handicaps() {
        use nml_control::SlotSetupKind;
        let slot = |kind, handicap, ki_kind| SlotSetup {
            kind,
            nation: None,
            team: None,
            color: None,
            handicap,
            ki_kind,
        };
        assert_eq!(parse_slot("c"), Ok(slot(SlotSetupKind::Closed, None, None)));
        assert_eq!(parse_slot("h"), Ok(slot(SlotSetupKind::Human, None, None)));
        for kind in 1..=4u8 {
            assert_eq!(
                parse_slot(&format!("a{kind}")),
                Ok(slot(SlotSetupKind::Ai, None, Some(kind)))
            );
        }
        for handicap in 0..=5 {
            assert_eq!(
                parse_slot(&format!("a:h{handicap}")),
                Ok(slot(SlotSetupKind::Ai, Some(handicap), None))
            );
        }
        // Capitalization and whitespace do not matter, as with the other switches.
        assert_eq!(
            parse_slot(" A3 "),
            Ok(slot(SlotSetupKind::Ai, None, Some(3)))
        );
        for bad in [
            "", "x", "a", "a0", "a5", "a:h", "a:h6", "a:h-1", "h1", "c2", "a:3",
        ] {
            let error = parse_slot(bad).unwrap_err();
            assert!(
                error.contains("allowed: c, h, a1 to a4, a:h0 to a:h5"),
                "{bad}: {error}"
            );
        }
        assert_eq!(parse_ki_switch("on"), Ok(true));
        assert_eq!(parse_ki_switch("OFF"), Ok(false));
        assert!(parse_ki_switch("ja").unwrap_err().contains("on or off"));
        // Nation and team of the command line go into the slots in order.
        let slots = vec![
            slot(SlotSetupKind::Human, None, None),
            slot(SlotSetupKind::Ai, None, Some(3)),
            slot(SlotSetupKind::Ai, Some(2), None),
        ];
        let nations = ["spa".to_string(), "ENG".to_string()];
        let merged = merge_lobby_lists(slots.clone(), Some(&nations), Some(&[1, 0, 1])).unwrap();
        assert_eq!(
            merged
                .iter()
                .map(|s| (s.nation, s.team))
                .collect::<Vec<_>>(),
            [(Some(2), Some(1)), (Some(4), Some(0)), (None, Some(1))]
        );
        assert_eq!(merged[1].ki_kind, Some(3));
        assert_eq!(merge_lobby_lists(slots.clone(), None, None).unwrap(), slots);
        let error = merge_lobby_lists(slots[..1].to_vec(), Some(&nations), None).unwrap_err();
        assert!(
            format!("{error:#}").contains("--nations: 2 entries, --slots only 1"),
            "{error:#}"
        );
        let error = merge_lobby_lists(slots[..1].to_vec(), None, Some(&[0, 1])).unwrap_err();
        assert!(
            format!("{error:#}").contains("--teams: 2 entries, --slots only 1"),
            "{error:#}"
        );
        let error = merge_lobby_lists(slots, Some(&["FRA".to_string()]), None).unwrap_err();
        assert!(format!("{error:#}").contains("FRA"), "{error:#}");
    }

    #[test]
    fn empty_toml_gives_defaults() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn toml_values_are_read() {
        let c = Config::from_toml(
            "backend = \"vulkan\"\nwindow_mode = \"exclusive\"\nmonitor = 1\nwidth = 1600\nheight = 900\n\
             game_dir = \"games/NML\"\nlevel = \"data/levels/kampagne2/kamp2-1/kamp2-1.lvl\"\n\
             aspect = \"original43\"\nfog = true\nanimate = false\nshadow_quality = 3\nparticles_quality = 1\n\
             terrain_quality = 2\nlight = \"Sommer Nacht\"\nweather = \"snow\"\n\
             volume_music = 0\nvolume_sfx = 1000\nvolume_speech = 250\nmute = true\nhud = true\nunit_response = false\n\
             game_speed = 30\ngame_mode = 3\nseed = 7\nnations = [\"spa\", \"ENG\"]\nteams = [1, 1]\n\
             resource_setting = 2\nliving_space = 5\nmap_explored = true\n\
             script_enabled = true\ntime_per_char = 70\ntime_limit = 45\n\
             [controls]\nkeyboard_scroll = 4\nlock_camera = false\n",
        )
        .unwrap();
        assert_eq!(
            c,
            Config {
                backend: BackendChoice::Vulkan,
                window_mode: WindowMode::Exclusive,
                monitor: Some(1),
                width: 1600,
                height: 900,
                game_dir: Some(PathBuf::from("games/NML")),
                level: "data/levels/kampagne2/kamp2-1/kamp2-1.lvl".into(),
                random_map: None,
                aspect: AspectSetting::Original43,
                fog: true,
                animate: false,
                shadow_quality: 3,
                particles_quality: 1,
                terrain_quality: 2,
                light: LightSetting::Preset("Sommer Nacht".into()),
                weather: WeatherSetting::Snow,
                volume_music: 0,
                volume_sfx: 1000,
                volume_speech: 250,
                mute: true,
                hud: true,
                unit_response: false,
                game_speed: 30,
                game_mode: Some(3),
                seed: 7,
                nations: vec!["spa".into(), "ENG".into()],
                teams: vec![1, 1],
                resource_setting: 2,
                living_space: 5,
                map_explored: true,
                slots: Vec::new(),
                ki_enabled: true,
                campaign_difficulty: 1,
                script_enabled: true,
                time_per_char: 70,
                time_limit: Some(45),
                controls: Controls {
                    keyboard_scroll: 4,
                    lock_camera: false,
                    ..Controls::default()
                },
            }
        );
    }

    #[test]
    fn example_file_matches_the_defaults() {
        let example = Config::from_toml(include_str!("../nml-viewer.example.toml")).unwrap();
        assert_eq!(example, Config::default());
    }

    #[test]
    fn unknown_keys_and_values_are_errors() {
        assert!(Config::from_toml("backnd = \"vulkan\"").is_err());
        assert!(Config::from_toml("backend = \"opengl\"").is_err());
        assert!(Config::from_toml("aspect = \"wide\"").is_err());
        assert!(Config::from_toml("[controls]\nmouse = 1").is_err());
        let error = Config::from_toml("weather = \"hail\"").unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("hail") && text.contains("allowed"), "{text}");
        assert!(Config::from_toml("light = \"\"").is_err());
        let error = Config::from_toml("shadow_quality = 4").unwrap_err();
        assert!(format!("{error:#}").contains("0 to 3"), "{error:#}");
        assert!(Config::from_toml("particles_quality = 3").is_err());
        assert!(Config::from_toml("terrain_quality = 3").is_err());
        assert!(Config::from_toml("terrain_quality = -1").is_err());
        let error = Config::from_toml("volume_sfx = 1001").unwrap_err();
        assert!(format!("{error:#}").contains("0 to 1000"), "{error:#}");
        assert!(Config::from_toml("volume_music = -1").is_err());
        let error = Config::from_toml("game_speed = 31").unwrap_err();
        assert!(format!("{error:#}").contains("0 to 30"), "{error:#}");
        let error = Config::from_toml("resource_setting = 4").unwrap_err();
        assert!(
            format!("{error:#}").contains("resource_setting"),
            "{error:#}"
        );
        assert!(Config::from_toml("resource_setting = -1").is_err());
        let error = Config::from_toml("living_space = 6").unwrap_err();
        assert!(format!("{error:#}").contains("0 to 5"), "{error:#}");
        assert!(Config::from_toml("living_space = -1").is_err());
        let error = Config::from_toml("nations = [\"SPA\", \"FRA\"]").unwrap_err();
        assert!(format!("{error:#}").contains("FRA"), "{error:#}");
    }

    #[test]
    fn light_and_weather_settings_parse() {
        assert_eq!("START".parse(), Ok(LightSetting::Start));
        assert_eq!("level".parse(), Ok(LightSetting::Level));
        assert_eq!(
            "Spa 5 Nacht".parse(),
            Ok(LightSetting::Preset("Spa 5 Nacht".into()))
        );
        assert_eq!("none".parse(), Ok(WeatherSetting::Off));
        assert_eq!("Rain".parse(), Ok(WeatherSetting::Rain));
        assert!("off".parse::<WeatherSetting>().is_err());
        // TOML and command line read the same (case does not matter).
        assert_eq!(
            Config::from_toml("weather = \"Snow\"").unwrap().weather,
            WeatherSetting::Snow
        );
        let c = Config::from_toml("light = \"level\"\nweather = \"none\"").unwrap();
        assert_eq!(
            (c.light, c.weather),
            (LightSetting::Level, WeatherSetting::Off)
        );
    }

    #[test]
    fn missing_file_is_an_error_only_when_given_explicitly() {
        let missing = std::env::temp_dir().join(format!(
            "nml-viewer-does-not-exist-{}.toml",
            std::process::id()
        ));
        assert_eq!(load_from(&missing, false).unwrap(), Config::default());
        let error = load_from(&missing, true).unwrap_err();
        assert!(format!("{error:#}").contains("not found"), "{error:#}");
        assert!(Config::load(Some(&missing)).is_err());
    }

    #[test]
    fn overrides_replace_only_given_fields() {
        let base = Config {
            monitor: Some(1),
            fog: true,
            ..Config::default()
        };
        let c = base.clone().with_overrides(Overrides {
            backend: Some(BackendChoice::Dx12),
            level: Some("x.lvl".into()),
            game_mode: Some(-1),
            teams: Some(vec![0, 0]),
            map_explored: Some(true),
            ..Overrides::default()
        });
        assert_eq!(
            c,
            Config {
                backend: BackendChoice::Dx12,
                level: "x.lvl".into(),
                game_mode: Some(-1),
                teams: vec![0, 0],
                map_explored: true,
                ..base
            }
        );
    }

    #[test]
    fn aspect_cycles_and_parses() {
        assert_eq!(AspectSetting::Stretch.next(), AspectSetting::Correct);
        assert_eq!("ORIGINAL43".parse(), Ok(AspectSetting::Original43));
        assert_eq!(
            AspectMode::from(AspectSetting::Stretch),
            AspectMode::Stretch
        );
    }

    fn multiplayer_rules() -> Rules {
        let mut rules = Rules::synthetic(TYPES, 16);
        rules.level_path = "data/levels/multiplayer/x/x.lvl".into();
        for slot in 0..3u8 {
            rules.level.players.push(nml_formats::config::LevelPlayer {
                slot,
                name: format!("P{slot}"),
                nation: 2,
                kind: 0,
                resources: Vec::new(),
            });
        }
        rules
    }

    #[test]
    fn settings_override_mode_seed_nations_and_teams_of_the_lobby() {
        let rules = multiplayer_rules();
        let config = Config {
            seed: 9,
            nations: vec!["eng".into(), "WID".into()],
            teams: vec![1, 1],
            resource_setting: 3,
            living_space: 5,
            map_explored: true,
            ..Config::default()
        };
        let setup = match_setup(&config, &rules).unwrap();
        assert_eq!((setup.mode, setup.seed, setup.lobby.len()), (0, 9, 3));
        assert_eq!((setup.resource_setting, setup.living_space), (3, 5));
        assert!(setup.map_explored);
        let nations: Vec<i32> = setup.lobby.iter().map(|e| e.nation).collect();
        let teams: Vec<i32> = setup.lobby.iter().map(|e| e.team).collect();
        assert_eq!(nations, [4, 0, 2]);
        assert_eq!(teams, [1, 1, 2]);
        // Campaign mode on a multiplayer map: without a lobby.
        let campaign = Config {
            game_mode: Some(-1),
            ..Config::default()
        };
        assert!(match_setup(&campaign, &rules).unwrap().lobby.is_empty());
        let too_many = Config {
            teams: vec![0; 4],
            ..Config::default()
        };
        let error = match_setup(&too_many, &rules).unwrap_err();
        assert!(format!("{error}").contains("only 3 slots"), "{error}");
        // Game type >= 0 needs a lobby.
        let mode0 = Config {
            game_mode: Some(0),
            ..Config::default()
        };
        assert!(match_setup(&mode0, &Rules::synthetic(TYPES, 16)).is_err());
        // Game type 2 (Domination) is allowed; game type 6 starts just the same, nobody checks the setup.
        let domination = match_setup(
            &Config {
                game_mode: Some(2),
                ..Config::default()
            },
            &rules,
        )
        .unwrap();
        assert_eq!(domination.mode, 2);
        assert!(SimHost::new(rules.clone(), domination, DEFAULT_GAME_SPEED).is_ok());
        let six = match_setup(
            &Config {
                game_mode: Some(6),
                ..Config::default()
            },
            &rules,
        )
        .unwrap();
        assert!(SimHost::new(rules, six, DEFAULT_GAME_SPEED).is_ok());
    }

    /// `time_limit` of the settings goes into the setup (without a value the default 20); the range
    /// 1 to 35791 is checked when the settings are read.
    #[test]
    fn time_limit_reaches_the_setup_and_is_checked() {
        let rules = multiplayer_rules();
        let default = match_setup(&Config::default(), &rules).unwrap();
        assert_eq!(default.time_limit, 20);
        for minutes in [1, 45, 35_791] {
            let config = Config {
                game_mode: Some(2),
                time_limit: Some(minutes),
                ..Config::default()
            };
            assert_eq!(config.check().ok(), Some(()), "{minutes}");
            let setup = match_setup(&config, &rules).unwrap();
            assert_eq!((setup.mode, setup.time_limit), (2, minutes));
        }
        for bad in [0, -5, 99_999, 35_792] {
            let config = Config {
                time_limit: Some(bad),
                ..Config::default()
            };
            let error = config.check().unwrap_err().to_string();
            assert!(
                error.contains(&format!("time_limit = {bad} invalid")) && error.contains("35791"),
                "{error}"
            );
        }
        assert_eq!(
            nml_sim::setup::MAX_TIME_LIMIT,
            *nml_control::TIME_LIMIT_RANGE.end()
        );
    }

    /// `sim_setup` with `time_limit` and game type 2 goes into the settings (they also apply to later
    /// restarts) and asks for a new match; without a value the last value stays.
    #[test]
    fn sim_setup_args_carry_the_time_limit() {
        let mut config = Config::default();
        let args = crate::app::SimSetupArgs {
            mode: Some(2),
            time_limit: Some(30),
            ..Default::default()
        };
        assert!(args.apply(&mut config), "new match needed");
        assert_eq!((config.game_mode, config.time_limit), (Some(2), Some(30)));
        let seed_only = crate::app::SimSetupArgs {
            seed: Some(4),
            ..Default::default()
        };
        assert!(seed_only.apply(&mut config));
        assert_eq!(config.time_limit, Some(30), "as last time");
        let speed_only = crate::app::SimSetupArgs {
            game_speed: Some(10),
            ..Default::default()
        };
        assert!(!speed_only.apply(&mut config), "only the game speed");
    }

    #[test]
    fn broken_script_falls_back_to_a_match_without_script() {
        // `script_enabled = true` in the TOML and a script that does not pass the load check:
        // the map starts without a script (steps as in `start_sim`).
        use crate::testworld_viewer::sim_setup;
        use nml_client::game::sim_host::script_fallback;
        use nml_client::game::testworld::{abl, rules, script_rules};
        let mut bad = rules();
        bad.scripts = nml_sim::script::testing::rules_with(&abl(&[(&[], &["n25;t"])]), &[], &[]);
        assert_eq!(bad.scripts.errors.len(), 1);
        let config = Config {
            script_enabled: true,
            ..Config::default()
        };
        let mut setup = match_setup(&config, &bad).unwrap();
        assert!(setup.script.enabled);
        // Without the fallback the match fails.
        assert!(SimHost::new(bad.clone(), setup.clone(), DEFAULT_GAME_SPEED).is_err());
        let warning = script_fallback(&mut setup, &bad).expect("warning");
        assert_eq!(
            warning,
            "Script not loaded, match without script (load errors: 1): Script synthetisch.abl, \
             line 3: action 25 needs at least 2 parameters (has 1)"
        );
        assert!(!setup.script.enabled);
        let mut host = SimHost::new(bad.clone(), setup, DEFAULT_GAME_SPEED).unwrap();
        assert_eq!(host.state.script, nml_sim::script::ScriptState::default());
        host.step_once();
        assert!(host.state.script.level.is_none(), "no script is running");
        // An explicit `sim_setup` with a script keeps answering with the load error.
        let mut viewer_config = config.clone();
        let error = sim_setup(
            &mut host,
            &mut viewer_config,
            crate::app::SimSetupArgs {
                script_enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("action 25 needs"), "{error}");
        // Without a script in the setup or with a valid script there is no fallback.
        let mut off = match_setup(&Config::default(), &bad).unwrap();
        assert_eq!(script_fallback(&mut off, &bad), None);
        let good = script_rules(&abl(&[(&[], &["n12;an"])]));
        let mut on = match_setup(&config, &good).unwrap();
        assert_eq!(script_fallback(&mut on, &good), None);
        assert!(on.script.enabled);
    }

    /// The path command line -> `Config` -> `match_setup` (the `sim_setup` side is checked by
    /// `sim_setup_with_ai_slots_starts_the_ki`).
    #[test]
    fn match_setup_uses_slots_and_ki() {
        use clap::Parser;
        use nml_sim::setup::SlotKind;
        let config_of = |base: Config, args: &[&str]| {
            let cli = crate::Cli::try_parse_from(args).unwrap();
            let config = base.with_overrides(crate::overrides(&cli)?);
            config.check()?;
            anyhow::Ok(config)
        };
        let rules = nml_client::game::testworld::ki_rules();
        let config = config_of(
            Config::default(),
            &[
                "nml-viewer",
                "--slots",
                "h,a1,c,a:h3",
                "--nations",
                "SPA,ENG",
                "--teams",
                "0,1",
                "--ki",
                "off",
                "--campaign-difficulty",
                "2",
            ],
        )
        .unwrap();
        // Nation and team are in the slots; `nations`/`teams` stay empty (they exclude each other).
        assert!(config.nations.is_empty() && config.teams.is_empty());
        assert_eq!(config.slots.len(), 4);
        let setup = match_setup(&config, &rules).unwrap();
        let lobby: Vec<_> = setup
            .lobby
            .iter()
            .map(|e| (e.kind, e.nation, e.team, e.color, e.handicap))
            .collect();
        assert_eq!(
            lobby,
            [
                (SlotKind::Human, 2, 0, 0, 0),
                (SlotKind::Ai, 4, 1, 1, 5),
                (SlotKind::Closed, 2, 2, 2, 0),
                (SlotKind::Ai, 3, 3, 3, 3),
            ]
        );
        assert_eq!((setup.ki.enabled, setup.ki.campaign_difficulty), (false, 2));
        // Without --ki: default on, difficulty 1; slots after the list like the map.
        let config = config_of(Config::default(), &["nml-viewer", "--slots", "h,a3"]).unwrap();
        let setup = match_setup(&config, &rules).unwrap();
        assert_eq!((setup.ki.enabled, setup.ki.campaign_difficulty), (true, 1));
        let kinds: Vec<_> = setup.lobby.iter().map(|e| (e.kind, e.handicap)).collect();
        assert_eq!(
            kinds,
            [
                (SlotKind::Human, 0),
                (SlotKind::Ai, 0),
                (SlotKind::Human, 0),
                (SlotKind::Human, 0)
            ]
        );
        // --slots replaces `nations` of the settings file, --nations without --slots replaces the `slots`.
        let file = Config {
            nations: vec!["PAT".into()],
            ..Config::default()
        };
        let config = config_of(file, &["nml-viewer", "--slots", "a2"]).unwrap();
        assert!(config.nations.is_empty());
        assert_eq!(match_setup(&config, &rules).unwrap().lobby[0].handicap, 2);
        let config = config_of(config, &["nml-viewer", "--nations", "WID"]).unwrap();
        assert!(config.slots.is_empty());
        assert_eq!(match_setup(&config, &rules).unwrap().lobby[0].nation, 0);
        // Errors: more nations than slots, difficulty 3, unknown slot kind.
        let error = config_of(
            Config::default(),
            &["nml-viewer", "--slots", "h", "--nations", "SPA,ENG"],
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("--nations: 2 entries"),
            "{error:#}"
        );
        assert!(crate::Cli::try_parse_from(["nml-viewer", "--campaign-difficulty", "3"]).is_err());
        assert!(crate::Cli::try_parse_from(["nml-viewer", "--slots", "h,x"]).is_err());
        assert!(crate::Cli::try_parse_from(["nml-viewer", "--ki", "maybe"]).is_err());
    }

    #[test]
    fn a_failing_setup_changes_neither_the_speed_nor_the_match() {
        let mut host = host();
        host.step_once();
        let before = (host.game_speed, host.checksum(), host.state.steps);
        // Game type 0 needs a multiplayer map: error, the new game speed must not apply.
        let config = Config {
            game_mode: Some(0),
            game_speed: 20,
            ..Config::default()
        };
        let error = apply_config(&mut host, &config, true).unwrap_err();
        assert!(error.contains("multiplayer map"), "{error}");
        assert_eq!((host.game_speed, host.checksum(), host.state.steps), before);
        // Without a restart, the game speed applies alone.
        apply_config(&mut host, &config, false).unwrap();
        assert_eq!(host.game_speed, 20);
    }

    #[test]
    fn sim_setup_with_ai_slots_starts_the_ki() {
        use crate::testworld_viewer::sim_setup;
        use nml_client::game::testworld::{ai_slot, human_slot, ki_rules};
        use nml_sim::ki::start::START_PLAYER;
        use nml_sim::state::Control;
        let rules = ki_rules();
        let setup = nml_sim::setup::MatchSetup::default_for(&rules);
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        // Default: all slots human, AI off, no engine.
        assert!(host.state.ki.engines.iter().all(Option::is_none));
        let mut config = Config::default();
        assert!(config.ki_enabled, "default of the viewer: on");
        let closed = nml_control::SlotSetup {
            kind: nml_control::SlotSetupKind::Closed,
            ..human_slot()
        };
        let args = crate::app::SimSetupArgs {
            slots: Some(vec![human_slot(), ai_slot(3, 1), closed]),
            ki_enabled: Some(true),
            campaign_difficulty: Some(0),
            ..Default::default()
        };
        sim_setup(&mut host, &mut config, args).unwrap();
        assert_eq!(host.setup.lobby[1].handicap, 5, "AI kind 1 -> handicap 5");
        assert_eq!(host.setup.ki.campaign_difficulty, 0);
        assert!(host.setup.ki.enabled);
        let p = &host.state.players;
        assert_eq!(
            [p[0].control, p[1].control, p[2].control, p[3].control],
            [Control::Human, Control::Ai, Control::Empty, Control::Human]
        );
        assert_eq!((p[1].handicap, p[1].nation), (5, 3));
        // Engines for every occupied slot; slot 1 has `multi_startplayer` with level 5 - 5 = 0.
        let present: Vec<bool> = host.state.ki.engines.iter().map(Option::is_some).collect();
        assert_eq!(
            present,
            [true, true, false, true, false, false, false, false]
        );
        let e = host.state.ki.engines[1].as_ref().unwrap();
        assert_eq!(e.mem.values.get(&302), Some(&1000));
        let threads: Vec<_> = e
            .threads
            .iter()
            .map(|(_, t)| (t.module, t.stack.clone()))
            .collect();
        assert_eq!(threads, [(START_PLAYER, vec![1, 3, 0])]);
        // The values stay in the settings; `nations` clears `slots` and vice versa.
        assert_eq!(config.slots.len(), 3);
        let args = crate::app::SimSetupArgs {
            nations: Some(vec!["SPA".into()]),
            ..Default::default()
        };
        sim_setup(&mut host, &mut config, args).unwrap();
        assert!(config.slots.is_empty());
        assert!(host.state.players[1].control == Control::Human);
        assert!(config.ki_enabled);

        // Contradictory slots -> error message, no restart.
        let (checksum, steps) = (nml_sim::save::checksum(&host.state), host.state.steps);
        host.step_once();
        let before = (nml_sim::save::checksum(&host.state), host.state.steps);
        assert_ne!(before, (checksum, steps));
        let five = vec![human_slot(); 5];
        let error = sim_setup(
            &mut host,
            &mut config,
            crate::app::SimSetupArgs {
                slots: Some(five),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            error.contains("slots: 5 entries, the lobby has only 4 slots"),
            "{error}"
        );
        let error = sim_setup(
            &mut host,
            &mut config,
            crate::app::SimSetupArgs {
                mode: Some(-1),
                slots: Some(vec![ai_slot(1, 1)]),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("campaign has no lobby"), "{error}");
        let mut bad = ai_slot(1, 1);
        bad.handicap = Some(2);
        let error = sim_setup(
            &mut host,
            &mut config,
            crate::app::SimSetupArgs {
                slots: Some(vec![bad]),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("mutually exclusive"), "{error}");
        let error = sim_setup(
            &mut host,
            &mut config,
            crate::app::SimSetupArgs {
                campaign_difficulty: Some(5),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("campaign_difficulty = 5"), "{error}");
        assert_eq!(
            (nml_sim::save::checksum(&host.state), host.state.steps),
            before,
            "no restart"
        );

        // The test kits of `nml-client` build the same matches without the settings of the viewer.
        let (viewer, _) = crate::testworld_viewer::ki_host();
        let client = nml_client::game::testworld::ki_host();
        assert_eq!(client.setup, viewer.setup);
        assert_eq!(client.checksum(), viewer.checksum());
        let rules = ki_rules();
        assert_eq!(
            nml_client::game::sim_host::testkit::default_setup(&rules).unwrap(),
            match_setup(&Config::default(), &rules).unwrap()
        );
    }

    // --- Campaign start (`--script`, `--mission`) ----------------------------------------------------

    /// Table with two campaigns: `campaign.ini` lines like the original (entries without a path arise only from
    /// headlines).
    fn mission_table() -> nml_formats::campaign::CampaignTable {
        nml_formats::campaign::parse_campaign_ini(
            "CAMPAIGN 0\nHEADLINE 1\nLEVELNAME 2\nLEVELPATH data/levels/tutorial0-1/kamp0-1.lvl\n\
             CAMPAIGN 1\nHEADLINE 3\nLEVELNAME 4\nLEVELPATH data/levels/kampagne1/kamp1-1/Kamp1-1.LVL\n\
             VIDEONAME 5\nVIDEOPATH data/videos/a.bik\nLEVELNAME 6\n\
             LEVELPATH data\\levels\\kampagne1\\kamp1-2\\kamp1-2.lvl\n",
        )
    }

    #[test]
    fn missions_are_listed_over_all_campaigns_by_file_stem() {
        let missions = campaign_missions(&mission_table());
        let list: Vec<_> = missions
            .iter()
            .map(|m| (m.campaign, m.name.as_str()))
            .collect();
        // Videos and headlines do not count; the stem is the file name without `.lvl` (also with `\`).
        assert_eq!(list, [(0, "kamp0-1"), (1, "Kamp1-1"), (1, "kamp1-2")]);
        assert_eq!(
            find_mission(&missions, "KAMP1-1").unwrap().path,
            "data/levels/kampagne1/kamp1-1/Kamp1-1.LVL"
        );
        // With a file extension or a path it is not a stem.
        assert!(find_mission(&missions, "kamp1-1.lvl").is_err());
        let error = find_mission(&missions, "kamp9-9").unwrap_err();
        assert_eq!(
            error,
            "Mission \"kamp9-9\" unknown (known: kamp0-1, Kamp1-1, kamp1-2)"
        );
        let none = campaign_missions(&nml_formats::campaign::CampaignTable::default());
        assert_eq!(
            find_mission(&none, "x").unwrap_err(),
            "Mission \"x\" unknown (no campaign levels in campaign.ini)"
        );
    }

    #[test]
    fn script_switch_and_mission_arguments_parse() {
        use clap::Parser;
        let cli = |args: &[&str]| crate::Cli::try_parse_from(args);
        assert_eq!(cli(&["nml-viewer"]).unwrap().script, None);
        assert_eq!(
            cli(&["nml-viewer", "--script", "on"]).unwrap().script,
            Some(true)
        );
        assert_eq!(
            cli(&["nml-viewer", "--script", "OFF"]).unwrap().script,
            Some(false)
        );
        assert!(cli(&["nml-viewer", "--script", "maybe"]).is_err());
        let parsed = cli(&["nml-viewer", "--mission", "kamp1-1"]).unwrap();
        assert_eq!(parsed.mission.as_deref(), Some("kamp1-1"));
        // The mission brings level, game type and slots itself.
        for other in [
            ["--level", "data/x.lvl"],
            ["--game-mode", "0"],
            ["--slots", "h"],
            ["--nations", "SPA"],
            ["--teams", "0"],
        ] {
            assert!(
                cli(&["nml-viewer", "--mission", "kamp1-1", other[0], other[1]]).is_err(),
                "{other:?}"
            );
        }
        // `--script` applies in the settings like `sim_setup script_enabled`.
        let config = Config::default().with_overrides(Overrides {
            script_enabled: Some(true),
            ..Overrides::default()
        });
        assert!(config.script_enabled);
        let config = config.with_overrides(Overrides::default());
        assert!(config.script_enabled, "without a value the value stays");
    }

    /// The random map comes only from the command line (`with_overrides`); in the settings file `random_map`
    /// is unknown, and a mission replaces it.
    #[test]
    fn random_map_comes_from_the_command_line_only() {
        let spec = RandomMapSpec {
            seed_text: "ZKA".into(),
            size: 1,
            players: 4,
            terrain: 0,
            landscape: 4,
            daytime: 0,
            game_type: 0,
        };
        assert_eq!(Config::default().random_map, None);
        let config = Config::default().with_overrides(Overrides {
            random_map: Some(spec.clone()),
            ..Overrides::default()
        });
        assert_eq!(config.random_map, Some(spec));
        // Without a value the value stays.
        let kept = config.clone().with_overrides(Overrides::default());
        assert_eq!(kept.random_map, config.random_map);
        assert!(
            Config::from_toml(
                "random_map = \"ZKA\"
"
            )
            .is_err()
        );
        let mut mission = config;
        mission.use_mission("data/levels/kampagne1/kamp1-1/kamp1-1.lvl");
        assert_eq!(mission.random_map, None);
    }

    #[test]
    fn a_mission_sets_campaign_mode_ki_and_script() {
        let table = mission_table();
        // Settings with lobby, map and script off: the mission replaces them.
        let base = Config {
            level: "data/levels/multiplayer/x.lvl".into(),
            game_mode: Some(0),
            slots: vec![parse_slot("h").unwrap()],
            ki_enabled: false,
            script_enabled: false,
            campaign_difficulty: 2,
            ..Config::default()
        };
        let config = base.clone().with_mission(&table, "kamp1-2", None).unwrap();
        assert_eq!(config.level, "data/levels/kampagne1/kamp1-2/kamp1-2.lvl");
        assert_eq!(config.game_mode, Some(-1));
        assert!(config.ki_enabled && config.script_enabled);
        assert!(config.slots.is_empty() && config.nations.is_empty() && config.teams.is_empty());
        assert_eq!(config.campaign_difficulty, 2, "difficulty stays");
        config.check().unwrap();
        // An explicit `--script off` wins over the mission.
        let config = base
            .clone()
            .with_mission(&table, "kamp1-2", Some(false))
            .unwrap();
        assert!(config.ki_enabled && !config.script_enabled);
        // Unknown: message with the known names, settings unchanged.
        let error = base.clone().with_mission(&table, "nope", None).unwrap_err();
        assert!(
            error.starts_with("Mission \"nope\" unknown (known: kamp0-1"),
            "{error}"
        );
    }

    /// `--mission kamp1-1` finds the level in the real `campaign.ini` (all four campaigns), switches on
    /// game type -1, AI and script and yields the setup of `match_request::campaign_setup`.
    #[test]
    fn mission_switch_resolves_kamp1_1_and_turns_on_ki_and_script() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let table = nml_ui::profiles::load_campaigns(&vfs);
        let missions = campaign_missions(&table);
        for campaign in 0..4 {
            assert!(
                missions.iter().any(|m| m.campaign == campaign),
                "Campaign {campaign} without a level"
            );
        }
        // The German edition (reference) has 2 + 6 + 10 + 14 levels; the English one is checked only for the shape.
        if nml_rda::testutil::game_version(&vfs) == Ok(nml_rda::testutil::GameVersion::German) {
            assert_eq!(missions.len(), 32);
        }
        let config = Config::default()
            .with_mission(&table, "kamp1-1", None)
            .unwrap();
        assert_eq!(config.level, "data/levels/kampagne1/kamp1-1/kamp1-1.lvl");
        assert!(
            vfs.exists(&config.level),
            "the level is in the installation"
        );
        assert_eq!(config.game_mode, Some(-1));
        assert!(config.ki_enabled && config.script_enabled);
        assert_eq!(config.campaign_difficulty, 1, "default 1");
        // Setup like the campaign start of the game.
        let rules = nml_client::game::testworld::rules();
        let setup = match_setup(&config, &rules).unwrap();
        let expected =
            nml_client::match_request::campaign_setup(1, config.time_per_char, None, &[]);
        assert_eq!(setup, expected);
        // Every name in the list can be found unambiguously.
        for m in &missions {
            assert_eq!(&find_mission(&missions, &m.name).unwrap().path, &m.path);
        }
        let error = find_mission(&missions, "kamp9-9").unwrap_err();
        assert!(
            error.contains("kamp1-1") && error.contains("kamp3-14"),
            "{error}"
        );
    }
}
