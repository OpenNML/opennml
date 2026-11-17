//! MCP tools (rmcp): thin wrapper around [`Session`]. Errors come back as a tool result with
//! `is_error` so that Claude sees the message.

use std::sync::Arc;

use nml_control::{
    CaptureCommand, Command, KeyAction, MouseButtonName, MusicCommand, ScreenshotData, WeatherName,
    WindowModeName,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::launch::{Profile, Program};
use crate::session::Session;
use crate::tools_ki::{KiParams, SlotParam};
use crate::tools_script::ScriptParams;
use crate::tools_ui::UiParams;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartParams {
    /// Further arguments for nml-viewer, e.g.
    /// ["--level", "data/levels/kampagne1/kamp1-1/kamp1-1.lvl", "--window-mode", "windowed"]
    #[serde(default)]
    pub args: Vec<String>,
    /// Program: viewer (default, nml-viewer) or game (nml-game, starts in the main menu; control channel with a
    /// subset of the commands, viewer_state/viewer_input/viewer_time return the state of the UI there)
    pub program: Option<Program>,
    /// Build profile: release (default) or debug
    #[serde(default)]
    pub profile: Profile,
    /// Start with the virtual clock stopped at 0 ms (default: true). This makes a fresh
    /// session reproducible; false lets the clock run in real time.
    #[serde(default = "default_paused")]
    pub paused: bool,
    /// Output sound audibly (without: --mute)
    #[serde(default)]
    pub audio: bool,
}

fn default_paused() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AttachParams {
    /// Port of the control channel on 127.0.0.1 (from the log line "Steuerkanal: ...")
    pub port: u16,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyActionParam {
    /// Press and hold the key
    Down,
    /// Release the key
    Up,
    /// press once and release immediately
    Tap,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ButtonParam {
    Left,
    Right,
    /// Middle button (nml-game only, `KD_CAM_FREEHAND`; not for ui click)
    Middle,
}

/// Description of the action `key` with all key names of the control channel (generated from
/// `nml_control::KEY_NAMES`, so no key is missing in the schema).
fn key_description() -> String {
    format!(
        "Key in winit notation: {}. In nml-game the German key layout applies: main-row + is BracketRight, \
         main-row - is Slash (Equal is ´ there, Minus ß). repeat (only with action down): that many further \
         press events as automatic repetition (autorepeat)",
        nml_control::KEY_NAMES.join(", ")
    )
}

/// One input; `type` selects the kind.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputAction {
    #[schemars(description = key_description())]
    Key {
        code: String,
        action: KeyActionParam,
        /// only with action down: number of the following repetitions (autorepeat), default 0
        #[serde(default)]
        repeat: u16,
    },
    /// Mouse pointer to window pixel (x, y)
    MouseMove { x: f64, y: f64 },
    /// Mouse pointer leaves the window
    MouseLeave,
    /// Press the mouse button (down = true) or release it
    MouseButton { button: ButtonParam, down: bool },
    /// one notch of the mouse wheel (up = closer)
    Wheel { up: bool },
    /// frames logic steps of frame_ms each (0 < frame_ms <= 100), then one frame; stops the clock
    Step { frames: u32, frame_ms: f64 },
}

impl From<InputAction> for Command {
    fn from(action: InputAction) -> Self {
        match action {
            InputAction::Key {
                code,
                action,
                repeat,
            } => Command::Key {
                code,
                action: match action {
                    KeyActionParam::Down => KeyAction::Down,
                    KeyActionParam::Up => KeyAction::Up,
                    KeyActionParam::Tap => KeyAction::Tap,
                },
                repeat,
            },
            InputAction::MouseMove { x, y } => Command::MouseMove { x, y },
            InputAction::MouseLeave => Command::MouseLeave,
            InputAction::MouseButton { button, down } => Command::MouseButton {
                button: match button {
                    ButtonParam::Left => MouseButtonName::Left,
                    ButtonParam::Right => MouseButtonName::Right,
                    ButtonParam::Middle => MouseButtonName::Middle,
                },
                down,
            },
            InputAction::Wheel { up } => Command::Wheel { up },
            InputAction::Step { frames, frame_ms } => Command::Step { frames, frame_ms },
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InputParams {
    /// Actions, executed in order
    pub actions: Vec<InputAction>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeAction {
    /// stop the virtual clock
    Pause,
    /// let it continue in real time
    Resume,
    /// frames steps of frame_ms each, then one frame; the clock stays stopped
    Step,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TimeParams {
    pub action: TimeAction,
    /// only with step: number of steps (1-10000)
    pub frames: Option<u32>,
    /// only with step: frame time per step in ms (0 < frame_ms <= 100)
    pub frame_ms: Option<f64>,
}

impl TimeParams {
    pub fn command(&self) -> Result<Command, String> {
        Ok(match self.action {
            TimeAction::Pause => Command::Pause,
            TimeAction::Resume => Command::Resume,
            TimeAction::Step => match (self.frames, self.frame_ms) {
                (Some(frames), Some(frame_ms)) => Command::Step { frames, frame_ms },
                _ => return Err("step needs frames and frame_ms".into()),
            },
        })
    }
}

/// Default for the longest side of the PNG (larger images exceed the image size of the response).
pub const SCREENSHOT_MAX_SIZE: u32 = 1280;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScreenshotParams {
    /// with debug overlay (default: without)
    #[serde(default)]
    pub overlay: bool,
    /// longest side of the PNG in pixels (default 1280, 0 = full size); the checksum always
    /// refers to the full image
    pub max_size: Option<u32>,
}

impl ScreenshotParams {
    pub fn max_size(&self) -> Option<u32> {
        match self.max_size {
            None => Some(SCREENSHOT_MAX_SIZE),
            Some(0) => None,
            Some(size) => Some(size),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowModeParam {
    Windowed,
    Borderless,
    Exclusive,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WindowParams {
    /// Window mode (like F11)
    pub mode: Option<WindowModeParam>,
    /// Monitor index from 0 (like F10; counts as chosen by the user)
    pub monitor: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAction {
    /// Names of the light presets (order of F6, `level` = values of the .wld)
    Lights,
    /// Set the light (parameter name)
    SetLight,
    /// Set the weather (parameter kind)
    SetWeather,
    /// Set quality levels (shadow, particles, terrain; each optional)
    SetQuality,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WeatherParam {
    None,
    Rain,
    Snow,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EnvironmentParams {
    pub action: EnvironmentAction,
    /// only with set_light: level or name of a preset (case-insensitive)
    pub name: Option<String>,
    /// only with set_weather: none, rain or snow
    pub kind: Option<WeatherParam>,
    /// only with set_quality: ShadowQuality 0-3
    pub shadow: Option<u8>,
    /// only with set_quality: ParticlesQuality 0-2
    pub particles: Option<u8>,
    /// only with set_quality: TerrainQuality 0-2 (from 2 water in detail texture mode)
    pub terrain: Option<u8>,
}

impl EnvironmentParams {
    pub fn command(&self) -> Result<Command, String> {
        Ok(match self.action {
            EnvironmentAction::Lights => Command::Lights,
            EnvironmentAction::SetLight => Command::SetLight {
                name: self.name.clone().ok_or("set_light needs name")?,
            },
            EnvironmentAction::SetWeather => Command::SetWeather {
                kind: match self.kind.ok_or("set_weather needs kind")? {
                    WeatherParam::None => WeatherName::None,
                    WeatherParam::Rain => WeatherName::Rain,
                    WeatherParam::Snow => WeatherName::Snow,
                },
            },
            EnvironmentAction::SetQuality => Command::SetQuality {
                shadow: self.shadow,
                particles: self.particles,
                terrain: self.terrain,
            },
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectsAction {
    /// Create an effect (path; position or object_index; scale)
    Spawn,
    /// remove effects created via spawn
    Clear,
    /// burn intensity in the model browser (value 0-3)
    SetDamage,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EffectsParams {
    pub action: EffectsAction,
    /// only with spawn: .pef path, e.g. data/effects/feuer.pef
    pub path: Option<String>,
    /// only with spawn: world position [x, y, z] (default: camera target or center of the ground plane)
    pub position: Option<[f32; 3]>,
    /// only with spawn on the map: index of the object (as hovered.index) that the effect follows
    pub object_index: Option<usize>,
    /// only with spawn: scale (default 1)
    pub scale: Option<f32>,
    /// only with set_damage: burn intensity 0-3
    pub value: Option<f32>,
}

impl EffectsParams {
    pub fn command(&self) -> Result<Command, String> {
        Ok(match self.action {
            EffectsAction::Spawn => Command::SpawnEffect {
                path: self.path.clone().ok_or("spawn needs path")?,
                position: self.position,
                object_index: self.object_index,
                scale: self.scale,
            },
            EffectsAction::Clear => Command::ClearEffects,
            EffectsAction::SetDamage => Command::SetDamage {
                value: self.value.ok_or("set_damage needs value")?,
            },
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioAction {
    /// Play a sound (guid + sound_action or path; position for 3D)
    PlaySound,
    /// Play menu music (track: title, win, lose, credits or path)
    MusicPlay,
    /// fade out all music
    MusicStop,
    /// combat music on/off by hand (on)
    MusicAttack,
    /// volume controls 0-1000 (music, sfx, speech) and muting (mute)
    SetVolume,
    /// start an offline mix into a WAV file (path)
    CaptureStart,
    /// end the offline mix: length, peak and RMS level, RMS envelope per 100 ms
    CaptureStop,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AudioParams {
    pub action: AudioAction,
    /// only with play_sound: object type (OBJ_GUID) of the sound database
    pub guid: Option<u32>,
    /// only with play_sound: action (e.g. 100 selection, 200 surf)
    pub sound_action: Option<i32>,
    /// play_sound: .wav or .ogg (speech); capture_start: target WAV
    pub path: Option<String>,
    /// only with play_sound: world position [x, y, z] for a 3D sound
    pub position: Option<[f32; 3]>,
    /// only with music_play: title, win, lose, credits or path
    pub track: Option<String>,
    /// only with music_attack
    pub on: Option<bool>,
    /// only with set_volume: controls 0-1000
    pub music: Option<u16>,
    pub sfx: Option<u16>,
    pub speech: Option<u16>,
    /// only with set_volume: no device output
    pub mute: Option<bool>,
    /// only with set_volume: Engine.ini UnitResponse (selection voice of the units, command acknowledgment)
    pub unit_response: Option<bool>,
}

impl AudioParams {
    pub fn command(&self) -> Result<Command, String> {
        Ok(match self.action {
            AudioAction::PlaySound => Command::PlaySound {
                guid: self.guid,
                action: self.sound_action,
                path: self.path.clone(),
                position: self.position,
            },
            AudioAction::MusicPlay => Command::Music {
                action: MusicCommand::Play,
                track: Some(self.track.clone().ok_or("music_play needs track")?),
                on: None,
            },
            AudioAction::MusicStop => Command::Music {
                action: MusicCommand::Stop,
                track: None,
                on: None,
            },
            AudioAction::MusicAttack => Command::Music {
                action: MusicCommand::Attack,
                track: None,
                on: Some(self.on.ok_or("music_attack needs on")?),
            },
            AudioAction::SetVolume => Command::SetVolume {
                music: self.music,
                sfx: self.sfx,
                speech: self.speech,
                mute: self.mute,
                unit_response: self.unit_response,
            },
            AudioAction::CaptureStart => Command::AudioCapture {
                action: CaptureCommand::Start,
                path: Some(self.path.clone().ok_or("capture_start needs path")?),
            },
            AudioAction::CaptureStop => Command::AudioCapture {
                action: CaptureCommand::Stop,
                path: None,
            },
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SimAction {
    /// new match (mode, seed, nations, teams) or speed only (game_speed)
    Setup,
    /// command to the simulation (command), takes effect at the next tick
    Command,
    /// query objects (player, guid, ids, limit) and all players
    Query,
    /// AI query (protocol 23): player, code, words, sight; read-only
    KiQuery,
    /// write the savegame, or with replay the replay, to path
    Save,
    /// load a savegame or replay (up to steps) from path
    Load,
    /// checksum and those of the last ticks
    Hash,
    /// Game result (protocol 29): slot player loses (result 1) or wins (2); command
    /// 0x56 of the active player, takes effect at the next tick
    Result,
    /// plan a path (unit or from + layer, to; allow_override, show)
    Path,
    /// layer 0-8 of the path grid, or 9 for occupancy (layer, rect)
    Grid,
    /// send own units (ids, empty = selection) to to as with a right click (append, cyclic)
    Move,
    /// build building guid at to (rotation_steps, ids = workers, empty = selection; append)
    Build,
    /// train or trade in the building building (guid)
    Train,
    /// cancel order index (0 running, 1 ... waiting) in the building building
    Cancel,
    /// Debug: complete immediately order index (0 running, 1 ... waiting, expected type guid) in the
    /// building building
    Complete,
    /// sell buildings ids (empty = selection)
    Sell,
    /// right click according to targets.ini: ids (empty = selection) on target or to (append, cyclic)
    Action,
    /// build list of the workers ids (empty = selection) or production list of building
    Menu,
    /// fog of a player (player, rect)
    Fog,
    /// view: active player (player), fog_shown, fog_overlay
    View,
    /// Button of the unit bar for the selection: 511-515 formation (none, line, square,
    /// column, pincer), 519 scatter, 405 dismount, 406 disembark, 432 dive, 433 surface,
    /// 449 train forward, 450 train backward, 451 train stop (button; 449-451 always go to the first
    /// own locomotive of the selection, even if the bar does not currently show the button), 485 arm,
    /// 486 disarm, the abilities 543, 544, 545, 549, 552, 557, 559, 560, 561, 563 and the counter-upgrades 3370-3377,
    /// 452 pack tent, 453 unpack travois (cursor), 430 open gate, 431 close gate,
    /// 434 camouflage, 435 uncamouflage, 437 dig in, 438 dig out, 458/554 start healing,
    /// 459/564 stop healing, 555 self-healing, 436 evoke ghost, 439/547/550/553/558/562
    /// banish, 441 wolf, 442 bear, 443 evoke wildcat (only the first selected own object);
    /// protocol 21: 480 explosion, 481 storm, 482 reincarnation, 484 scout area, 541 drain
    /// mana, 542 whirlwind, 454/455 dynamite at building/ship, 456/548 kill, 457/546
    /// snipe, 551 damage building, 556 scout map (target buttons with target or to);
    /// protocol 22: 540 rob (to every selected own robber, without a target)
    UnitBar,
    /// apply upgrade guid like the console or remove it with remove, on the target slot player
    /// (0-10; the sender is the active player)
    Upgrade,
    /// Trade with a player: carts ids (empty = selection) drive with the trade kind ware (0 buy
    /// wood, 1 buy food, 2 sell wood, 3 sell food) to the partner slot target
    /// (0-8); the sender is player
    Trade,
    /// Tribute: the slot player pays the slots 0-7 with the bit set in recipients amount of the
    /// resource resource (0 gold, 1 wood, 2 food)
    Tribute,
    /// Headhunter menu (protocol 22): player = observer, art 0 SPA/1 PRI/2 SID, optional
    /// target_player; result players (target players) and targets (target GUIDs of the target_player);
    /// hiring goes via command with HireHeadhunter
    HireMenu,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SimParams {
    pub action: SimAction,
    /// setup: game type -1 campaign, 0 normal, 1 kill hero, 2 Domination, 3 King of the Hill, 4/5 Railroad
    pub mode: Option<i32>,
    /// setup: start index of the random sequence (original 0)
    pub seed: Option<u32>,
    /// setup: nations of the lobby slots, e.g. ["SPA", "ENG"]
    pub nations: Option<Vec<String>>,
    /// setup: teams of the lobby slots, e.g. [0, 0, 1, 1]
    pub teams: Option<Vec<i32>>,
    /// setup: GameSpeed 0-30 (14 = 1.2 times the game time)
    pub game_speed: Option<u8>,
    /// setup: resource setting 0 = starting balance of the map, 1-3 = resourcenconfig.ini
    /// (multiplayer maps only)
    pub resource_setting: Option<i32>,
    /// setup: living space level 0-5 = 75, 100, 125, 150, 175, 200 (multiplayer maps only)
    pub living_space: Option<i32>,
    /// setup: map explored (MapExplored): "explored" fully set at the start
    pub map_explored: Option<bool>,
    /// setup (protocol 26): lobby slots in order, e.g. [{"kind": "human"}, {"kind": "ai", "nation": 1,
    /// "ki_kind": 3}]; excludes nations and teams; multiplayer maps only
    pub slots: Option<Vec<SlotParam>>,
    /// setup (protocol 26): start the AI of the AI slots
    pub ki_enabled: Option<bool>,
    /// setup (protocol 26): campaign difficulty 0-2
    pub campaign_difficulty: Option<i32>,
    /// setup (protocol 28): start the level script (.abl); viewer default off
    pub script_enabled: Option<bool>,
    /// setup (protocol 28): speaking time per character without a speech file in ms, 1-1000 (default 55)
    pub time_per_char: Option<i32>,
    /// setup: time limit in minutes, 1-35791 (default 20; if omitted, the last value stays): victory time of the flag
    /// game types 2/3, time limit of game type 4
    pub time_limit: Option<i32>,
    /// command: command as JSON, e.g. {"player": 0, "kind": {"Halt": {"units": [65537]}}} or
    /// {"player": 0, "kind": {"Diplomacy": {"p": 0, "q": 1, "mode": 0}}}
    pub command: Option<serde_json::Value>,
    /// query: only objects of this player slot (0-10); fog: observer 0-7 (default: active
    /// player); view: new active player (occupied slot 0-7); upgrade: target slot 0-10
    /// (required; 8 = neutrals); trade, tribute: sender (required; with tribute also the giver);
    /// hire_menu: observer 0-7 (required); ki_query: slot whose sight the query uses (0-7,
    /// required); result: slot whose result is set (0-7, required)
    pub player: Option<u8>,
    /// result (protocol 29): 1 lost, 2 won (required)
    pub result: Option<i32>,
    /// query: only objects of this type; build: building type; train: unit or trade (490-493);
    /// complete: expected type of the order (default: the current entry); upgrade: upgrade GUID
    /// (required, 2800-3399)
    pub guid: Option<u32>,
    /// query: only these IDs; move, build, action, menu, trade: these units; sell: these
    /// buildings (default in each case: selection)
    pub ids: Option<Vec<u32>>,
    /// query: at most this many objects (default 100)
    pub limit: Option<usize>,
    /// ki_query: query code `0x4E20`…`0x5130` (decimal number, e.g. 20004 = 0x4E24; required)
    pub code: Option<u32>,
    /// ki_query: request words from `[2]` of the field `{code, argc, …}` (default `[player]`, at most
    /// 128; argc is their number)
    pub words: Option<Vec<i32>>,
    /// ki_query: sight of the query, `omniscient` (default, like the AI of the original) or `fog`
    /// (only what the slot sees)
    pub sight: Option<String>,
    /// save/load: file
    pub path: Option<String>,
    /// save: replay instead of savegame
    pub replay: Option<bool>,
    /// load of a replay: up to this engine tick
    pub steps: Option<u64>,
    /// path: ID of the unit (layer from its walk module)
    pub unit: Option<u32>,
    /// path: start point in world coordinates [x, z] (instead of unit, then with layer)
    pub from: Option<[f32; 2]>,
    /// path, move, build, action: destination in world coordinates [x, z]; unit_bar: ground point of the
    /// target buttons (protocol 21, goes to the viewer as ground; 484 and 556 require it)
    pub to: Option<[f32; 2]>,
    /// move: world point at which the right button was released (formation drag from to to
    /// drag_to: facing direction and spread)
    pub drag_to: Option<[f32; 2]>,
    /// unit_bar: button 511-515 (formation), 519 (scatter), 405 (dismount), 406 (disembark),
    /// 432 (dive), 433 (surface), 449 (train forward), 450 (train backward) or 451 (train stop);
    /// 449-451 are always sent to the first own locomotive, even if the bar does not currently
    /// show the button (e.g. 449 with a moving locomotive); what comes of it is decided by the
    /// locomotive as with the command Train;
    /// 485 (arm), 486 (disarm), 543/544/545/549/552/557/559/560/561/563 (abilities) and 3370-3377
    /// (counter-upgrades) act according to the bar: only what it shows for the selection goes out as
    /// a command;
    /// 452 (pack tent: the first selected object, if it is an own tent) and 453 (unpack travois: opens the
    /// unpack cursor, the target follows with a left click on the map);
    /// 430 (open gate) and 431 (close gate): the first selected object, if it is an own,
    /// finished gate in the matching state (protocol 18);
    /// 434 (camouflage), 435 (uncamouflage), 437 (dig in), 438 (dig out), 458 and 554 (start
    /// healing), 459 and 564 (stop healing) and 555 (self-healing) go without a menu check of the
    /// viewer to all own selected objects (555 only to objects with the button in the menu); the
    /// simulation checks menu, prerequisite and state per object like the bar and rejects
    /// otherwise (protocol 19);
    /// 436 (evoke ghost), 439, 547, 550, 553, 558 and 562 (banish) as well as 441, 442 and 443
    /// (evoke wolf, bear, wildcat) act only on the first selected own object, and only if
    /// the bar shows the button for it (menu, prerequisite, target filter); the simulation checks
    /// again and deducts the cost (protocol 20);
    /// 480 (explosion), 481 (storm), 482 (reincarnation), 484 (scout area), 541 (drain
    /// mana), 542 (whirlwind), 454 and 455 (dynamite at building or ship), 456 and 548
    /// (kill), 457 and 546 (snipe), 551 (damage building) and 556 (scout map) act
    /// like 436 only on the first selected own object (482 and 542 on every selected own
    /// object with the button); all except 482 and 542 are target buttons and need target (object)
    /// or to (ground point, 484 and 556 only the ground): the command goes immediately, without targeting mode, and
    /// the simulation checks target, range, prerequisite and mana again (protocol 21);
    /// 540 (rob) goes like the abilities to every selected own robber whose bar
    /// shows the button (research 3326), without a target (protocol 22)
    pub button: Option<i32>,
    /// path: layer 0-8 (replaces that of the unit); grid: 0-8 or 9 = occupancy
    pub layer: Option<u8>,
    /// path: temporarily count the target tile to the start region (FINDTO 0x100)
    pub allow_override: Option<bool>,
    /// path: show the path in the view (default true)
    pub show: Option<bool>,
    /// grid, fog: section [x0, z0, x1, z1] in tiles, inclusive, at most 4096 cells
    pub rect: Option<[i32; 4]>,
    /// move, build, action: queue (Shift)
    pub append: Option<bool>,
    /// move, action: queue cyclically (Ctrl)
    pub cyclic: Option<bool>,
    /// build: quarter turns 0-3 (default 0)
    pub rotation_steps: Option<u8>,
    /// train, cancel, complete, menu: ID of the own building
    pub building: Option<u32>,
    /// cancel, complete: 0 = running order (default), 1 ... = waiting ones
    pub index: Option<i32>,
    /// action: target object (without to its position as ground point); trade: target slot 0-8 (required);
    /// unit_bar: target object of a target button (protocol 21)
    pub target: Option<u32>,
    /// view: fog on (false = "fog off", m_FogShowMode 7)
    pub fog_shown: Option<bool>,
    /// view: fog overlay 0 off, 1 explored, 3 seen, 7 cleared
    pub fog_overlay: Option<u8>,
    /// upgrade: remove the upgrade instead of applying it (default false)
    pub remove: Option<bool>,
    /// trade: trade kind 0 buy wood, 1 buy food, 2 sell wood, 3 sell food
    /// (required; the simulation checks the range)
    pub ware: Option<i32>,
    /// tribute: receivers as a bit mask over the slots 0-7 (required; the giver itself is
    /// skipped)
    pub recipients: Option<u32>,
    /// tribute: resource 0 gold, 1 wood, 2 food (required; the simulation checks the range)
    pub resource: Option<i32>,
    /// tribute: amount per receiver (required; the simulation rejects amounts <= 0)
    pub amount: Option<i32>,
    /// hire_menu: art 0-2 (0 SPA, 1 PRI, 2 SID; required)
    pub art: Option<u8>,
    /// hire_menu: target player 0-7; with it also the target GUIDs
    pub target_player: Option<u8>,
}

impl SimParams {
    pub fn command(&self) -> Result<Command, String> {
        let path = || self.path.clone().ok_or("save and load need path");
        Ok(match self.action {
            SimAction::Setup => Command::SimSetup {
                mode: self.mode,
                seed: self.seed,
                nations: self.nations.clone(),
                teams: self.teams.clone(),
                game_speed: self.game_speed,
                resource_setting: self.resource_setting,
                living_space: self.living_space,
                map_explored: self.map_explored,
                slots: self
                    .slots
                    .as_ref()
                    .map(|s| s.iter().map(Into::into).collect()),
                ki_enabled: self.ki_enabled,
                campaign_difficulty: self.campaign_difficulty,
                script_enabled: self.script_enabled,
                time_per_char: self.time_per_char,
                time_limit: self.time_limit,
            },
            SimAction::Command => Command::SimCommand {
                command: self.command.clone().ok_or("command needs command")?,
            },
            SimAction::Query => Command::SimQuery {
                player: self.player,
                guid: self.guid,
                ids: self.ids.clone(),
                limit: self.limit,
            },
            SimAction::KiQuery => Command::KiQuery {
                player: self.player.ok_or("ki_query needs player")?,
                code: self.code.ok_or("ki_query needs code")?,
                words: self.words.clone(),
                sight: self.sight.clone(),
            },
            SimAction::Save => Command::SimSave {
                path: path()?,
                replay: self.replay.unwrap_or(false),
            },
            SimAction::Load => Command::SimLoad {
                path: path()?,
                steps: self.steps,
            },
            SimAction::Hash => Command::SimHash,
            SimAction::Result => Command::SimResult {
                player: self.player.ok_or("result needs player")?,
                result: self.result.ok_or("result needs result (1 lost, 2 won)")?,
            },
            SimAction::Path => Command::SimPath {
                unit: self.unit,
                from: self.from,
                to: self.to.ok_or("path needs to")?,
                layer: self.layer,
                allow_override: self.allow_override.unwrap_or(false),
                show: self.show.unwrap_or(true),
            },
            SimAction::Grid => Command::SimGrid {
                layer: self.layer.ok_or("grid needs layer")?,
                rect: self.rect,
            },
            SimAction::Move => Command::SimMove {
                units: self.ids.clone().unwrap_or_default(),
                to: self.to.ok_or("move needs to")?,
                append: self.append.unwrap_or(false),
                cyclic: self.cyclic.unwrap_or(false),
                drag_to: self.drag_to,
            },
            SimAction::UnitBar => Command::SimUnitBar {
                button: self.button.ok_or("unit_bar needs button")?,
                target: self.target,
                ground: self.to,
            },
            SimAction::Upgrade => Command::SimUpgrade {
                player: self.player.ok_or("upgrade needs player (target slot)")?,
                guid: i32::try_from(self.guid.ok_or("upgrade needs guid")?)
                    .map_err(|_| "guid too large for an upgrade")?,
                remove: self.remove.unwrap_or(false),
            },
            SimAction::Trade => Command::SimTrade {
                player: self.player.ok_or("trade needs player (sender)")?,
                units: self.ids.clone().unwrap_or_default(),
                ware: self.ware.ok_or("trade needs ware")?,
                target: i32::try_from(self.target.ok_or("trade needs target (target slot)")?)
                    .map_err(|_| "target too large for a target slot")?,
            },
            SimAction::Tribute => Command::SimTribute {
                player: self.player.ok_or("tribute needs player (giver)")?,
                recipients: self.recipients.ok_or("tribute needs recipients")?,
                resource: self.resource.ok_or("tribute needs resource")?,
                amount: self.amount.ok_or("tribute needs amount")?,
            },
            SimAction::Build => Command::SimBuild {
                guid: self.guid.ok_or("build needs guid")?,
                to: self.to.ok_or("build needs to")?,
                rotation_steps: self.rotation_steps.unwrap_or(0),
                units: self.ids.clone().unwrap_or_default(),
                append: self.append.unwrap_or(false),
            },
            SimAction::Train => Command::SimTrain {
                building: self.building.ok_or("train needs building")?,
                guid: self.guid.ok_or("train needs guid")?,
            },
            SimAction::Cancel => Command::SimCancel {
                building: self.building.ok_or("cancel needs building")?,
                index: self.index.unwrap_or(0),
            },
            SimAction::Complete => Command::SimComplete {
                building: self.building.ok_or("complete needs building")?,
                index: self.index.unwrap_or(0),
                guid: self.guid,
            },
            SimAction::Sell => Command::SimSell {
                buildings: self.ids.clone().unwrap_or_default(),
            },
            SimAction::Action => Command::SimAction {
                units: self.ids.clone().unwrap_or_default(),
                target: self.target,
                to: self.to,
                append: self.append.unwrap_or(false),
                cyclic: self.cyclic.unwrap_or(false),
            },
            SimAction::Menu => match (&self.ids, self.building) {
                (Some(_), Some(_)) => return Err("ids and building are mutually exclusive".into()),
                (None, Some(building)) => Command::SimMenu {
                    units: None,
                    building: Some(building),
                },
                (ids, None) => Command::SimMenu {
                    units: Some(ids.clone().unwrap_or_default()),
                    building: None,
                },
            },
            SimAction::Fog => Command::SimFog {
                player: self.player,
                rect: self.rect,
            },
            SimAction::View => Command::SimView {
                active_player: self.player,
                fog_shown: self.fog_shown,
                fog_overlay: self.fog_overlay,
            },
            SimAction::HireMenu => Command::SimHireMenu {
                player: self.player.ok_or("hire_menu needs player")?,
                art: self.art.ok_or("hire_menu needs art")?,
                target_player: self.target_player,
            },
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LogParams {
    /// Number of the last lines (default 50)
    pub lines: Option<usize>,
}

fn text(message: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(message)])
}

fn failure(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// Result of a command: text or error message.
fn reply(result: Result<String, String>) -> CallToolResult {
    match result {
        Ok(message) => text(message),
        Err(message) => failure(message),
    }
}

/// State snapshot as indented JSON text.
fn state_text<T: serde::Serialize>(state: &T) -> Result<String, String> {
    serde_json::to_string_pretty(state).map_err(|e| e.to_string())
}

/// Screenshot as MCP image content (PNG) with size and checksum.
pub fn screenshot_result(data: ScreenshotData) -> CallToolResult {
    let size = if (data.png_width, data.png_height) == (data.width, data.height) {
        format!("{} x {} pixels", data.width, data.height)
    } else {
        format!(
            "{} x {} pixels (PNG scaled down to {} x {})",
            data.width, data.height, data.png_width, data.png_height
        )
    };
    CallToolResult::success(vec![
        ContentBlock::image(data.png_base64, "image/png"),
        ContentBlock::text(format!("{size}, checksum {}", data.rgba_hash)),
    ])
}

#[derive(Clone)]
pub struct ViewerServer {
    session: Arc<Mutex<Session>>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ViewerServer {
    pub fn new(session: Session) -> Self {
        Self {
            session: Arc::new(Mutex::new(session)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Starts target/<profile>/nml-viewer.exe (program: game = nml-game.exe, the game in the main menu) with a control channel (working directory: repo root) and connects. The virtual clock starts at 0 ms (paused, default true). Sound is muted (--mute) unless audio is true. Does not build (if the exe is missing, the message names the build command). Error if a viewer is already running."
    )]
    async fn viewer_start(
        &self,
        Parameters(p): Parameters<StartParams>,
    ) -> Result<CallToolResult, McpError> {
        Ok(reply(
            self.session
                .lock()
                .await
                .start(
                    p.args,
                    p.program.unwrap_or_default(),
                    p.profile,
                    p.paused,
                    p.audio,
                )
                .await,
        ))
    }

    #[tool(description = "Connects to a running viewer (nml-viewer --control 127.0.0.1:<port>).")]
    async fn viewer_attach(
        &self,
        Parameters(p): Parameters<AttachParams>,
    ) -> Result<CallToolResult, McpError> {
        Ok(reply(self.session.lock().await.attach(p.port).await))
    }

    #[tool(
        description = "Ends the viewer with quit; a self-started one is killed after 5 s if necessary."
    )]
    async fn viewer_stop(&self) -> Result<CallToolResult, McpError> {
        Ok(reply(self.session.lock().await.stop().await))
    }

    #[tool(
        description = "Synthetic inputs in order (keys, with key and action down optionally repeat = number of autorepeat repetitions; mouse, wheel, steps of the virtual clock); result: state snapshot afterwards as JSON."
    )]
    async fn viewer_input(
        &self,
        Parameters(p): Parameters<InputParams>,
    ) -> Result<CallToolResult, McpError> {
        let commands = p.actions.into_iter().map(Command::from).collect();
        let result = self.session.lock().await.input(commands).await;
        Ok(reply(result.and_then(|state| state_text(&state))))
    }

    #[tool(
        description = "Virtual clock: pause, resume or step (frames steps of frame_ms each, then one frame). Result: state snapshot as JSON."
    )]
    async fn viewer_time(
        &self,
        Parameters(p): Parameters<TimeParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = match p.command() {
            Ok(command) => command,
            Err(message) => return Ok(failure(message)),
        };
        let mut session = self.session.lock().await;
        let result = match session.command(command).await {
            Ok(_) => session.snapshot().await,
            Err(message) => Err(message),
        };
        Ok(reply(result.and_then(|state| state_text(&state))))
    }

    #[tool(
        description = "Screenshot of the current frame, rendered by the viewer (PNG), optionally with overlay. The checksum (FNV-1a over the RGBA pixels of the full image) shows pixel-identical images."
    )]
    async fn viewer_screenshot(
        &self,
        Parameters(p): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .session
            .lock()
            .await
            .screenshot(p.overlay, p.max_size())
            .await;
        Ok(match result {
            Ok(data) => screenshot_result(data),
            Err(message) => failure(message),
        })
    }

    #[tool(
        description = "State snapshot as JSON: view, window, clock, FPS/frame times, camera, object under the mouse (with class and shadow kind), switches, model browser, light and weather, shadows, mouse/keys, overlay. With nml-game (viewer_start with program: game) the state of the UI like viewer_ui state."
    )]
    async fn viewer_state(&self) -> Result<CallToolResult, McpError> {
        let result = self.session.lock().await.snapshot().await;
        Ok(reply(result.and_then(|state| state_text(&state))))
    }

    #[tool(
        description = "UI (nml-game, in the viewer only with --dialog in the window; protocol 24): state (dialogs, named elements with rectangle, visible, locked and value, focus, images, last 32 effects such as play_sfx, the match effects start_match, load_game, save_game, delete_save, reload_level, resume_game, write_profiles and delete_profile_saves; with nml-game and a match the field game {level, game_type, menu_state, steps, checksum, loading, load_phases}, since protocol 31 with hud {dialog, windows, messages, action_stack, status}: windows of the game UI, message bar per kind with text, remaining duration and queue, ping locations, status bar with texts and red; also input {paused, shown_paused, game_speed, config_speed, groups, last_group, pointer, pointer_variant, pointer_visible, key_context, gate}; also minimap {visible, mode, center, theta, pings [x, z, repetitions], idle_workers, fog_swaps}, diplomacy_open and chat_lines; the effect diplomacy {action, slot, value} and send_chat are in effects), click (dialog, element, button: click on the center of the element in the next frame, like the mouse by hand; result {x, y}; with dialog GAMEUI also the minimap: minimap, minimap_mode, minimap_idle, minimap_signal, minimap_idle_all, minimap_menu, minimap_diplomacy), key (chord of key names of the original such as ESCAPE, RETURN, \"CONTROL H\", char: key event in the next frame; result {vk}), open (dialog: queue for opening like command 5), video (name: queue the video data/videos/<name>.bik like videostart in the menu campaignvideo; state then shows video {path, frame, frames, abortable}, pointer_hidden and the effects play_video/stop_video; protocol 25), present (event: trigger a presentation event by hand, JSON form of the presentation such as {\"Portrait\": {\"slot\": 1, \"portrait\": 1000, \"frame\": 101}} or \"CameraBackup\"; presentation only (bars, portraits, texts, fade), the checksum stays the same until the next step (afterwards host consequences such as the emptied selection or the game speed can produce commands), game speed and fog mode of the host change as well; only with a match; state shows it under game.presentation {bars, portraits, texts, info_frames, countdown, screen_text, fade_alpha, pointer_visible, clear_color}), focus_lost (nml-game only: loss of the window focus, held keys released and the pause like the original, also with a control channel). With the clock stopped, click, key, open and video do not take effect until a step (viewer_time). Names are case-insensitive; errors name the known names."
    )]
    async fn viewer_ui(
        &self,
        Parameters(p): Parameters<UiParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut session = self.session.lock().await;
        Ok(reply(crate::tools_ui::execute(&mut session, &p).await))
    }

    #[tool(
        description = "AI match (protocol 26): state (AI state: paused; per engine slot, next_thread_id, threads in execution order with id, module, name from nml.db, pc, status, pause_ms, wait_semaphore, stack_depth, stack_top = top 8 values, child = running CALLM; semaphores, with values true also values = m_iValues; rejected = rejected AI commands per slot, code and reason; commands = AI commands per slot, code and outcome accepted/partial/rejected; counters since the last restart; slot filters; leaves the checksum unchanged), query (like viewer_sim ki_query: slot (required), code, words, sight), start (slot, module, args at most 64, sync: start the module on the engine of the slot, sync true = immediately to the end like .abl n0, otherwise as a thread like n1), pause (paused, default true: stop the whole AI or let it continue), step (slot, passes 1-1000, default 1: only with the AI stopped, per pass every thread of the slot makes exactly one step with dt = 20; the world stands still). start, pause and step are simulation commands: they take effect at the next tick (with the clock stopped, with viewer_time step), appear in the command log and in replays; result {step}. Setting up a match with AI: viewer_sim setup with slots and ki_enabled true."
    )]
    async fn viewer_ki(
        &self,
        Parameters(p): Parameters<KiParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut session = self.session.lock().await;
        Ok(reply(crate::tools_ki::execute(&mut session, &p).await))
    }

    #[tool(
        description = "Scripts of the campaign missions (protocol 33, nml-viewer only): state (script_state without block log: containers with blocks, namespaces, speech state, object names with names as filter, mission objectives, countdown, script commands, results, ui = {ui_locked, cinemascope, selection, last_button}; with block_container and block_index the conditions of this block with value), log (only the block log of the last steps steps, default 50, at most 1000), events (script_events: presentation events of the last steps steps with step, plus presentation = presentation state, ui, profile_vars and camera = script camera; all read-only, the checksum stays the same), skip_dialog (skip the running dialog, simulation command, result {step}), start (mission = file stem such as kamp1-1, difficulty 0-2, profile, paused: first viewer_stop, then restart of the viewer with --mission and --campaign-difficulty, muted in the window, script and AI on). The selection mirror and button for the conditions 35-38 are sent by viewer_sim command with ScriptSelection {ids} and ScriptButton {word}. New parameters do not appear in the schema until a new session."
    )]
    async fn viewer_script(
        &self,
        Parameters(p): Parameters<ScriptParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut session = self.session.lock().await;
        Ok(reply(crate::tools_script::execute(&mut session, &p).await))
    }

    #[tool(
        description = "Set window mode and/or monitor like F11/F10. The window may move across the screens of the user in the process."
    )]
    async fn viewer_window(
        &self,
        Parameters(p): Parameters<WindowParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = Command::Window {
            mode: p.mode.map(|m| match m {
                WindowModeParam::Windowed => WindowModeName::Windowed,
                WindowModeParam::Borderless => WindowModeName::Borderless,
                WindowModeParam::Exclusive => WindowModeName::Exclusive,
            }),
            monitor: p.monitor,
        };
        let mut session = self.session.lock().await;
        let result = match session.command(command).await {
            Ok(_) => session.state().await,
            Err(message) => Err(message),
        };
        Ok(reply(result.and_then(|state| state_text(&state))))
    }

    #[tool(
        description = "Light and weather of the map: lights (names of the presets), set_light (name: level or preset), set_weather (kind: none/rain/snow), set_quality (shadow 0-3: re-determines the shadows of all objects; particles 0-2, terrain 0-2). Result: name list or state snapshot as JSON (sections environment and shadows)."
    )]
    async fn viewer_environment(
        &self,
        Parameters(p): Parameters<EnvironmentParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = match p.command() {
            Ok(command) => command,
            Err(message) => return Ok(failure(message)),
        };
        let lights = command == Command::Lights;
        let mut session = self.session.lock().await;
        let result = match session.command(command).await {
            Ok(names) if lights => state_text(&names),
            Ok(_) => session.state().await.and_then(|state| state_text(&state)),
            Err(message) => Err(message),
        };
        Ok(reply(result))
    }

    #[tool(
        description = "Effects of the current view: spawn (path .pef; position [x,y,z] or object_index on the map; scale), clear (remove those created via spawn), set_damage (value 0-3, burn intensity in the model browser). Keys: N effect browser, V preview, Minus/Equal burn intensity or radius. Result: state snapshot as JSON (sections effects, browser, effect_browser)."
    )]
    async fn viewer_effects(
        &self,
        Parameters(p): Parameters<EffectsParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = match p.command() {
            Ok(command) => command,
            Err(message) => return Ok(failure(message)),
        };
        let mut session = self.session.lock().await;
        let result = match session.command(command).await {
            Ok(_) => session.state().await.and_then(|state| state_text(&state)),
            Err(message) => Err(message),
        };
        Ok(reply(result))
    }

    #[tool(
        description = "Sound: play_sound (guid + sound_action from sounddatabase.dat or path .wav/.ogg; position [x,y,z] = 3D), music_play (track: title/win/lose/credits or path), music_stop, music_attack (on), set_volume (music/sfx/speech 0-1000, mute, unit_response), capture_start (path .wav), capture_stop. Keys: F9 sound on/off, Shift+F9 combat music, in the model browser KeyT action, Enter play. Result: capture_stop length and levels (peak, RMS, RMS envelope per 100 ms); play_sound {played, id, audio}; otherwise the audio section of the state snapshot."
    )]
    async fn viewer_audio(
        &self,
        Parameters(p): Parameters<AudioParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = match p.command() {
            Ok(command) => command,
            Err(message) => return Ok(failure(message)),
        };
        let action = p.action;
        let mut session = self.session.lock().await;
        let result = match session.command(command).await {
            Ok(value) if matches!(action, AudioAction::CaptureStop) => state_text(&value),
            Ok(value) => session.state().await.and_then(|state| {
                let audio = serde_json::to_value(&state.audio).map_err(|e| e.to_string())?;
                if matches!(action, AudioAction::PlaySound) {
                    let mut combined = value;
                    if let Some(object) = combined.as_object_mut() {
                        object.insert("audio".into(), audio);
                    }
                    state_text(&combined)
                } else {
                    state_text(&audio)
                }
            }),
            Err(message) => Err(message),
        };
        Ok(reply(result))
    }

    #[tool(
        description = "Simulation of the map (with nml-game only hash, query, command and result, and only with a running match, otherwise the error \"no match\"): setup (mode, seed, nations, teams = new match; game_speed alone = speed only), command (command as JSON, takes effect at the next tick; result step), query (player, guid, ids, limit: objects with modules and all players), save (path, replay), load (path, steps: savegame or replay), hash (checksum and history), result (protocol 29: player = slot 0-7, result 1 lost or 2 won; command 0x56 SetPlayerResult of the active player like the cheat of the original, takes effect at the next tick, result step; game end in ui_state game.result), path (path like FINDTO: unit or from + layer, to [x, z] in world coordinates, allow_override, show: tile path, waypoints, substitute destination, message 0x500), grid (layer 0-8 path grid, 9 occupancy; rect [x0, z0, x1, z1]: cells -1 blocked, otherwise region ID), move (ids = own units, default selection; to [x, z]; append, cyclic: like right-click with Shift/Ctrl; drag_to [x, z] = release point of the formation drag; the command carries the formation chosen in the unit bar; walk state is in query under walk). Economy (protocol 9): setup also with resource_setting 0-3 and living_space 0-5; build (guid, to [x, z], rotation_steps 0-3, ids = workers, append: like the placement mode; errors with reason: invalid placement, blocked, no workers), train (building, guid), cancel (building, index: 0 running), complete (debug \"complete immediately\": building, index, guid = expected type of the entry, default the current one; if another one is there at execution, rejected; unpaid orders go through the start check, trade pays in the process; if funds are missing, the order stays with a message), sell (ids = buildings), action (ids, target and/or to, append, cyclic: right-click according to targets.ini; result button, action, command e.g. 400/ChopWood), menu (ids = workers: build list; building: production list and queue; per entry cost, affordable, lock reason). query shows per object building, gatherer, construct, quarter, resource, tree and per player population, counts, pending; the section sim market prices and the last messages (notices). Vision (protocol 10): setup also with map_explored; fog (player, default active player; rect: tile counts explored/seen/cleared with shared vision, cycle, sight sources, rows per tile 0/1/3/7, -2 outside; leaves the checksum unchanged), view (player = active player, occupied slots only; fog_shown false = \"fog off\"; fog_overlay 0/1/3/7; result section sim); query per player enemies and vision (bit masks), section sim fog_shown and fog_overlay. Melee (protocol 11): query shows per object with hit points or combat module combat (hp [current, maximum], hp_fraction, rank, experience, last_attacker, ticks_since_hit, render_bits, attack = status/target/automatic/aggressiveness/mode/stubbornness/search_counter, dying = status/die_counter/rob_gold/listeners); the section sim attack_points per target owner and attack_points_active, audio.music attack_points and attack_source, audio.recent_sounds the last 16 combat sounds. Combat commands via command: {\"player\": 0, \"kind\": {\"Attack\": {\"units\": [id], \"target\": id, \"pos\": null, \"mods\": 0}}} (mods 1 right double-click (run), 4 attack mode), {\"player\": 0, \"kind\": {\"SetBehavior\": {\"units\": [id], \"mode\": 0-4}}} (stance), {\"player\": 0, \"kind\": {\"GarrisonAttack\": {\"buildings\": [id], \"target\": id}}}; query leaves the checksum unchanged. Ranged combat (protocol 12): query shows per object ranged (min_dist, max_dist, shoot = flags/target/bullets/shoot_delay/busy/gun_select), animal (aggressive, shy, roamer, roaming, escaping, underwater, home, alpha), edible (meat, has_meat, rotting) and for hunters gatherer kind hunting with prey (there, source is also the hunting target); the section sim projectiles (id, kind arrow/ball, from protocol 21 also dynamite/explosion/storm, fire, remaining_ms, flight_ms, shooter, target, target_pos) and flights (trajectories drawn by the viewer). Hunting commands via command: {\"player\": 0, \"kind\": {\"Hunt\": {\"animal\": id, \"units\": [id], \"gut\": false}}}, follow {\"Follow\": {\"target\": id, \"units\": [id]}}. Keys: left click/selection box: selection, right click action according to targets.ini, H halt, Shift+1...5 stance (cease fire to defend area), X attack mode for the next right click, P/Shift+P player, I debug window, G/Shift+G pathfinding overlay, F fog overlay (off -> explored -> seen -> cleared), O fog off/on, T Economy window, R rotate. Groups and formations (protocol 13): unit_bar (button 511-515 chooses the formation none, line, square, column, pincer and re-forms the selection, 519 scatters; result step and formation); the section sim shows groups (id, kind, type_id, members, anchor, marching, matched_speed, targets, formation = spacing/depth/layer/rear/frames/ready/reported/arrived/end_max/end_min) and formation (formation chosen in the bar); query shows per unit under walk group and formation_num. Riding and diving (protocol 14): unit_bar also with button 405 (dismount), 406 (disembark), 432 (dive), 433 (surface), acts on the own units of the selection; query shows per object riding (list per module 13/14: index, boat, mount_target, going_to_horse, busy, next_mount_anim, lance_available) and diving (status, count, orig_layer, dive_layer, air, height_offset); the commands Mount, Dismount, BoardBoat, LeaveBoat, Dive, Surface also go via command. Ships, train and multipart objects (protocol 15): unit_bar also with button 449 (train forward), 450 (train backward), 451 (train stop), acts as command Train on the first own locomotive of the selection (449/450 when standing, 451 when moving); query shows per object train (locomotive, tender, wagon: slaves, master, speed, next_speed, acc_time, status, is_slave, dir, target, time_counter, has_supply, outside, raster_dir, last_block_tile), multipart (cart: head, linked, prev, next, offset, centre, last_pos) and event_bits (event word, absent at 0), in the section quarter meeting_point, waiting and status, in the section ranged.shoot is_steuerboard, cannon_counter and can_shoot (broadside); the train command Train also goes via command. Upgrades, spells and arming (protocol 16): upgrade (player = target slot 0-10, guid, remove: applies the upgrade like the console or removes it, without ownership check; sender is the active player, slot 8 writes the table of the neutrals, which is never read; result step, effect at the next tick); unit_bar also with button 485 (arm), 486 (disarm), the abilities 543, 544, 545, 549, 552, 557, 559, 560, 561, 563 (spells and temporary upgrades) and 3370-3377 (counter-upgrades); like the bar they act only on matching objects of the selection; query shows per player upgrades (applied GUIDs in order), in the result counter_upgrades (guid, rest_ms, source) and daytime, per object temp_upgrades (guid, flags, rest_ms), show_counter_upgrade, armed (only objects with module 39), spell (art, button, cost, anim, idx, reserve, mana, mana_max, casting) and exec_list (module 44); the commands ApplyUpgrade, RemoveUpgrade, CounterUpgrade, CastSpell, ExecTempUpgrade, ToggleArming and SetArrowType also go via command. Tents and trade (protocol 17): unit_bar also with button 452 (pack tent: the first selected object if it is an own, selectable tent) and 453 (unpack travois: opens the unpack cursor, the command is not given until a left click on the map); trade (player = sender, ids = carts, default selection, ware 0-3 = buy wood, buy food, sell wood, sell food, target = target slot 0-8: trade with a player, the simulation checks kind, target and trade building and rejects otherwise; foreign carts and units without a trade module are skipped; result step); tribute (player = giver and sender, recipients = bit mask of the slots 0-7, resource 0 gold, 1 wood, 2 food, amount; result step); query shows per object tent (status 0 idle, 1 fade-out, 2 fade-in, counter in ms, progress = crossfade factor, travois_guid, travois, hidden), transport_tent (tent, transporting), trade (ware, target, tax_level, max_tax, tax_interval, payers, load_amount, load_kind of the load) and in_collision (buildings only: whether it is in the collision manager); raw IDs without an object are 4294967295; the commands PackTent, UnpackTent, TradeWithPlayer and Tribute also go via command. Gates and enemy watch (protocol 18): unit_bar also with button 430 (open gate) and 431 (close gate: the first selected object if it is an own, finished gate in the matching state); query shows per building gate (gates only: open, closed, doors = door tiles x, z and blocked = layer 0 blocked), area_watch (only with a running watch: key, mode, rect, player, members) and watch_rect (stored watch rectangle, if not 0); the commands OpenGate and CloseGate also go via command. Camouflage, digging in, healing (protocol 19): unit_bar also with button 434 (camouflage), 435 (uncamouflage), 437 (dig in), 438 (dig out), 458 and 554 (start healing), 459 and 564 (stop healing), 555 (self-healing); the viewer sends them without a menu check to all own selected objects (555 only to objects with the button in the menu), the simulation checks per object like the bar (menu, prerequisite upgrade, camouflage, dig-in and trance state) and rejects otherwise; query shows per object masking (is_masked, is_busy, mask_time in ms, never_unmasked, energy = fraction of the extra energy 0-100000), assassin (is_down, target, attack_points), healing (ready, single, sick, new_target, healing_points, mana), message_filter (mode 0 only list allowed, 1 list forbidden; codes; absent at the default) and look (alpha and anim of the camouflage from the point of view of the player chosen in the viewer; absent without camouflage); empty targets are null; the commands Mask, Unmask, DigIn, DigOut, StartHealing, StopHealing and SelfHeal also go via command; query leaves the checksum unchanged. Ghosts, banishing, animal evocation, auras (protocol 20): unit_bar also with button 436 (evoke ghost), 439, 547, 550, 553, 558 and 562 (banish), 441 (wolf), 442 (bear) and 443 (evoke wildcat); unlike the buttons of protocol 19 they act only on the first selected own object, and only if the bar shows the button for it (menu, prerequisite upgrade, target filter), the simulation checks again and deducts the cost; query shows per object evoke_ghost (evoking, dead_body, ghost, extra_energy, mana, mana_base = base value of the mana, to which running ghosts bind their cost), ghost (status 0 calm, 1 appearing, 2 fading out, 3 suction strike due; alpha; target), banish (active = status ANIM_LOCKED, also the healing trance; effects = timer 0x41F running; spell_guid; radius in 1/4096 tile = sight * 4096 >> 3), evoke_animal (evoking, art 0 wolf/1 bear/2 wildcat, cost, reserve, spawn [x, y, z] in 1/4096 tile, mana) and influence (guids of the running influences, influenced = IDs of the influenced or null, ring = aura ring is shown; only with a running influence); empty IDs are null, stale ones stay; the commands EvokeGhost, EvokeAnimal and Banish also go via command; query leaves the checksum unchanged. Spells and special attacks (protocol 21): unit_bar also with the target buttons 480 (explosion), 481 (storm), 484 (scout area), 541 (drain mana), 454 and 455 (dynamite at building or ship), 456 and 548 (kill), 457 and 546 (snipe), 551 (damage building) and 556 (scout map) as well as 482 (reincarnation) and 542 (whirlwind); the target buttons require target (target object) or to (ground point [x, z] in world coordinates; 484 and 556 always take the ground), act only on the first selected own object and give the command immediately without targeting mode, without a target it is an error, 482 and 542 go to every selected own object with the button; the simulation checks target, range, prerequisite upgrade and mana again; query shows per object spell, now also with target (target point [x, y, z] of the spell kinds 1, 2 and 5 while casting) and corpse (corpse of the reincarnation), bomberman (target, extra_energy, return_point, busy = is laying right now, mana), killer (target, extra_energy, near_damage, far_damage, mana) and special_move (busy, mode 0 whirlwind/1 hammering, mana); in the section sim projectiles the kind dynamite (load), explosion and storm (pulse 0 or 1); empty targets are absent, stale ones stay; the commands CastSpell, Whirlwind, ScoutArea and the other special attacks also go via command; query leaves the checksum unchanged. Headhunters and robbing (protocol 22): unit_bar also with button 540 (rob: to every selected own robber whose bar shows the button, without a target); hire_menu (player = observer 0-7, art 0 SPA/1 PRI/2 SID, optional target_player 0-7: result players = target players like level 2 of the menu in the headquarters, targets = target GUIDs of the target_player like level 3, even if it is not a target player; leaves the checksum unchanged); hiring goes via command with {\"player\": 0, \"kind\": {\"HireHeadhunter\": {\"art\": 2, \"target_player\": 1, \"target_guid\": 2614, \"building\": id}}}, robbing also with {\"RobCorpses\": {\"unit\": id, \"button\": 540}}; query shows per object headhunter (art 0-2, 255 for a foreign type; state 0 searching/1 to the unit/2 to the building, target_guid, target_player, target, last_target, price, customer, customer_cost) and robbing (anim, robbing = robbing right now, attack_enabled = attack module on, mana), in the result headhunters (per art 0-2: art, hunter, price of the next hiring); in the section sim projectiles the kind throw (dynamite throw of the headhunter, impact 0xD22 unit/0xE22 building) and notices also with the text messages 10002, 10010 and 10011 (number from texte.deu, tile [-1, -1]); empty targets are absent, stale ones stay; query leaves the checksum unchanged. AI queries (protocol 23): ki_query (player = slot 0-7, code = query code 0x4E20...0x5130 as a decimal number, words = request words from [2], default [player], at most 128, sight omniscient (default) or fog: the query of the AI interface against the running match; result code, request = the whole request field {code, argc, [2], ...} after the response, padded to 2 + max(argc, 128) words, and direct = the direct changes the query would perform in the game, per kind mark/unmark (AI mark of the slot player on the object) or take_report (attack report of the victim object removed), only reported, not executed, marks of missing objects are absent; leaves the checksum unchanged); query shows per object ai_marks (slots 0-7 with a mark set, absent if empty) and per player attack_reports (victim, tile [x, z] in the order in which 0x512E selects them; absent if empty). AI match (protocol 26): setup also with slots (lobby slots in order: kind closed/human/ai, nation 0-5, team, color 0-8 (8 = white), handicap 0-5 or with ai ki_kind 1-4 = handicap 5/2/0/0; excludes nations and teams; multiplayer maps only), ki_enabled (start the AI of the AI slots) and campaign_difficulty 0-2; AI state and interventions via viewer_ki. Scripts (protocol 28): setup also with script_enabled (start the level script, viewer default off) and time_per_char 1-1000 (ms per character without a speech file, default 55); the script state is read with the tool viewer_script (command script_state of the raw control channel). Flag game modes: setup also with mode 2 (Domination) and time_limit 1-35791 (minutes; default 20; if omitted, the last value stays); slots color 0-8 (8 = white); the section sim shows flags {flags = [id, tile, color, holder, check_ms], counts (nine counters, 8 = neutral), leader, time_limit_s, remaining_ms} in game type 2/3 or with a living flag 4300, in nml-game ui_state game.flags; new parameters do not appear in the schema until a new session. World state of the scripts (protocol 30): query shows per object name (script name) and hidden (hidden) and in the result kill_counts [slot, GUID, count], button_locks [GUID, 0/1], slot_running (eleven slots) and area_views (named view rectangles); script_state also shows mission objectives, countdown display, task status and changed areas. Result: JSON of the command (setup/load/view: section sim)."
    )]
    async fn viewer_sim(
        &self,
        Parameters(p): Parameters<SimParams>,
    ) -> Result<CallToolResult, McpError> {
        let command = match p.command() {
            Ok(command) => command,
            Err(message) => return Ok(failure(message)),
        };
        let mut session = self.session.lock().await;
        let result = match session.command(command).await {
            Ok(value) => state_text(&value),
            Err(message) => Err(message),
        };
        Ok(reply(result))
    }

    #[tool(
        description = "Last lines of stderr of the viewer started with viewer_start (at most 2000 kept)."
    )]
    async fn viewer_log(
        &self,
        Parameters(p): Parameters<LogParams>,
    ) -> Result<CallToolResult, McpError> {
        Ok(text(self.session.lock().await.log(p.lines.unwrap_or(50))))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ViewerServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "nml-viewer-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Controls the nml-viewer (No Man's Land) through its control channel: viewer_start, \
                 then viewer_time pause and viewer_input with step actions for bit-exact \
                 repeatable checks; viewer_state and viewer_screenshot for reading values. \
                 Description: docs/viewer-control.md.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ContentBlock;

    #[test]
    fn viewer_start_is_paused_by_default() {
        let params: StartParams = serde_json::from_str("{}").unwrap();
        assert!(params.paused);
        assert!(params.args.is_empty());
        let params: StartParams = serde_json::from_str(r#"{"paused": false}"#).unwrap();
        assert!(!params.paused);
    }

    #[test]
    fn viewer_start_is_mute_by_default() {
        let params: StartParams = serde_json::from_str("{}").unwrap();
        assert!(!params.audio);
        let params: StartParams = serde_json::from_str(r#"{"audio": true}"#).unwrap();
        assert!(params.audio);
    }

    /// `viewer_ui` offers the action `present` with the parameter `event`.
    #[test]
    fn viewer_ui_describes_the_minimap_elements() {
        // The elements of the minimap are in the description of the parameter `element`.
        let tools = ViewerServer::tool_router().list_all();
        let tool = tools
            .iter()
            .find(|t| t.name == "viewer_ui")
            .expect("tool viewer_ui");
        let schema = serde_json::Value::Object((*tool.input_schema).clone()).to_string();
        for word in [
            "minimap_mode",
            "minimap_idle",
            "minimap_signal",
            "minimap_idle_all",
            "minimap_menu",
            "minimap_diplomacy",
            "GAMEUI",
        ] {
            assert!(schema.contains(word), "viewer_ui: {word} missing");
        }
        let description = tool.description.as_deref().unwrap_or_default();
        for word in ["minimap", "diplomacy_open", "chat_lines", "fog_swaps"] {
            assert!(description.contains(word), "description: {word} missing");
        }
    }

    #[test]
    fn viewer_ui_offers_present() {
        let tools = ViewerServer::tool_router().list_all();
        let tool = tools
            .iter()
            .find(|t| t.name == "viewer_ui")
            .expect("tool viewer_ui");
        let schema = serde_json::Value::Object((*tool.input_schema).clone()).to_string();
        for word in ["\"present\"", "\"event\""] {
            assert!(schema.contains(word), "viewer_ui: {word} missing");
        }
    }

    #[test]
    fn the_tool_schema_offers_viewer_ui_and_the_program() {
        // Tool viewer_ui with its actions and fields, viewer_start with program.
        let tools = ViewerServer::tool_router().list_all();
        let schema = |name: &str| {
            let tool = tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("tool {name} missing"));
            serde_json::Value::Object((*tool.input_schema).clone()).to_string()
        };
        let ui = schema("viewer_ui");
        for word in [
            "\"state\"",
            "\"click\"",
            "\"key\"",
            "\"open\"",
            "\"video\"",
            "\"name\"",
            "\"dialog\"",
            "\"element\"",
            "\"button\"",
            "\"chord\"",
            "\"char\"",
            "\"focus_lost\"",
        ] {
            assert!(ui.contains(word), "viewer_ui: {word} missing");
        }
        // Tool viewer_ki, viewer_sim setup with slots.
        let ki = schema("viewer_ki");
        for word in [
            "\"state\"",
            "\"query\"",
            "\"start\"",
            "\"pause\"",
            "\"step\"",
            "\"slot\"",
            "\"values\"",
            "\"module\"",
            "\"args\"",
            "\"sync\"",
            "\"paused\"",
            "\"passes\"",
            "\"code\"",
        ] {
            assert!(ki.contains(word), "viewer_ki: {word} missing");
        }
        let sim = schema("viewer_sim");
        for word in [
            "\"slots\"",
            "\"ki_enabled\"",
            "\"campaign_difficulty\"",
            "\"script_enabled\"",
            "\"time_per_char\"",
            "\"time_limit\"",
            "\"ki_kind\"",
            "\"closed\"",
        ] {
            assert!(sim.contains(word), "viewer_sim: {word} missing");
        }
        // Tool viewer_script.
        let script = schema("viewer_script");
        for word in [
            "\"state\"",
            "\"log\"",
            "\"events\"",
            "\"skip_dialog\"",
            "\"start\"",
            "\"steps\"",
            "\"mission\"",
            "\"difficulty\"",
            "\"block_container\"",
        ] {
            assert!(script.contains(word), "viewer_script: {word} missing");
        }
        let start = schema("viewer_start");
        for word in ["\"program\"", "\"viewer\"", "\"game\""] {
            assert!(start.contains(word), "viewer_start: {word} missing");
        }
        let params: StartParams = serde_json::from_str(r#"{"program": "game"}"#).unwrap();
        assert_eq!(params.program, Some(Program::Game));
        let params: StartParams = serde_json::from_str("{}").unwrap();
        assert_eq!(params.program, None, "default: nml-viewer");
    }

    #[test]
    fn screenshots_become_png_image_content() {
        let result = screenshot_result(ScreenshotData {
            width: 2,
            height: 1,
            rgba_hash: "0123456789abcdef".into(),
            png_width: 2,
            png_height: 1,
            png_base64: "iVBORw0KGgo=".into(),
        });
        assert_eq!(result.is_error, Some(false));
        match &result.content[0] {
            ContentBlock::Image(image) => {
                assert_eq!(image.mime_type, "image/png");
                assert_eq!(image.data, "iVBORw0KGgo=");
            }
            other => panic!("image content expected: {other:?}"),
        }
        assert!(
            matches!(&result.content[1], ContentBlock::Text(t) if t.text == "2 x 1 pixels, checksum 0123456789abcdef")
        );
        let scaled = screenshot_result(ScreenshotData {
            width: 3072,
            height: 1728,
            rgba_hash: "00".into(),
            png_width: 1280,
            png_height: 720,
            png_base64: String::new(),
        });
        assert!(
            matches!(&scaled.content[1], ContentBlock::Text(t) if t.text == "3072 x 1728 pixels (PNG scaled down to 1280 x 720), checksum 00")
        );
    }

    #[test]
    fn input_actions_are_read_from_json_and_mapped_to_commands() {
        let params: InputParams = serde_json::from_value(serde_json::json!({"actions": [
            {"type": "key", "code": "ArrowUp", "action": "down"},
            {"type": "mouse_move", "x": 10.0, "y": 20.5},
            {"type": "mouse_leave"},
            {"type": "mouse_button", "button": "right", "down": true},
            {"type": "mouse_button", "button": "middle", "down": false},
            {"type": "wheel", "up": false},
            {"type": "step", "frames": 30, "frame_ms": 16.0}
        ]}))
        .unwrap();
        let commands: Vec<Command> = params.actions.into_iter().map(Command::from).collect();
        assert_eq!(
            commands,
            [
                Command::Key {
                    code: "ArrowUp".into(),
                    action: KeyAction::Down,
                    repeat: 0
                },
                Command::MouseMove { x: 10.0, y: 20.5 },
                Command::MouseLeave,
                Command::MouseButton {
                    button: MouseButtonName::Right,
                    down: true
                },
                Command::MouseButton {
                    button: MouseButtonName::Middle,
                    down: false
                },
                Command::Wheel { up: false },
                Command::Step {
                    frames: 30,
                    frame_ms: 16.0
                },
            ]
        );
    }

    /// `viewer_input` offers `repeat` with `key` and names all key names of the control channel
    /// (generated from `KEY_NAMES`); `repeat` goes to `key`, without `repeat` 0; `repeat` with `up` is an error before
    /// anything is sent (`Command::validate`).
    #[test]
    fn viewer_input_offers_repeat() {
        let tools = ViewerServer::tool_router().list_all();
        let input = tools.iter().find(|t| t.name == "viewer_input").unwrap();
        let schema = serde_json::Value::Object((*input.input_schema).clone()).to_string();
        assert!(schema.contains("\"repeat\""), "{schema}");
        for name in nml_control::KEY_NAMES {
            assert!(schema.contains(name), "viewer_input: key {name} missing");
        }
        let params: InputParams = serde_json::from_value(serde_json::json!({"actions": [
            {"type": "key", "code": "BracketRight", "action": "down", "repeat": 5},
            {"type": "key", "code": "BracketRight", "action": "up"}
        ]}))
        .unwrap();
        let commands: Vec<Command> = params.actions.into_iter().map(Command::from).collect();
        assert_eq!(
            commands,
            [
                Command::Key {
                    code: "BracketRight".into(),
                    action: KeyAction::Down,
                    repeat: 5
                },
                Command::Key {
                    code: "BracketRight".into(),
                    action: KeyAction::Up,
                    repeat: 0
                },
            ]
        );
        let wrong = Command::from(
            serde_json::from_value::<InputAction>(
                serde_json::json!({"type": "key", "code": "KeyP", "action": "up", "repeat": 2}),
            )
            .unwrap(),
        );
        assert_eq!(wrong.validate(), Err("repeat only with down".to_string()));
    }

    #[test]
    fn screenshot_size_defaults_to_1280_and_zero_means_full_size() {
        let params = |max_size| ScreenshotParams {
            overlay: false,
            max_size,
        };
        assert_eq!(params(None).max_size(), Some(1280));
        assert_eq!(params(Some(0)).max_size(), None);
        assert_eq!(params(Some(640)).max_size(), Some(640));
    }

    #[test]
    fn time_step_needs_frames_and_frame_ms() {
        let params = |frames, frame_ms| TimeParams {
            action: TimeAction::Step,
            frames,
            frame_ms,
        };
        assert_eq!(
            params(Some(10), Some(16.0)).command(),
            Ok(Command::Step {
                frames: 10,
                frame_ms: 16.0
            })
        );
        assert!(params(Some(10), None).command().is_err());
        assert!(params(None, Some(16.0)).command().is_err());
        let pause = TimeParams {
            action: TimeAction::Pause,
            frames: None,
            frame_ms: None,
        };
        assert_eq!(pause.command(), Ok(Command::Pause));
    }

    #[test]
    fn environment_actions_map_to_commands() {
        let params = |value: serde_json::Value| -> EnvironmentParams {
            serde_json::from_value(value).unwrap()
        };
        assert_eq!(
            params(serde_json::json!({"action": "lights"})).command(),
            Ok(Command::Lights)
        );
        assert_eq!(
            params(serde_json::json!({"action": "set_light", "name": "5"})).command(),
            Ok(Command::SetLight { name: "5".into() })
        );
        assert_eq!(
            params(serde_json::json!({"action": "set_weather", "kind": "snow"})).command(),
            Ok(Command::SetWeather {
                kind: WeatherName::Snow
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "set_quality", "terrain": 2})).command(),
            Ok(Command::SetQuality {
                shadow: None,
                particles: None,
                terrain: Some(2)
            })
        );
        assert!(
            params(serde_json::json!({"action": "set_light"}))
                .command()
                .is_err()
        );
        assert!(
            params(serde_json::json!({"action": "set_weather"}))
                .command()
                .is_err()
        );
    }

    #[test]
    fn sim_result_action_maps_to_the_command() {
        // viewer_sim result -> sim_result; player and result are required, `Command::validate` checks the ranges.
        let params =
            |value: serde_json::Value| -> SimParams { serde_json::from_value(value).unwrap() };
        assert_eq!(
            params(serde_json::json!({"action": "result", "player": 1, "result": 2})).command(),
            Ok(Command::SimResult {
                player: 1,
                result: 2
            })
        );
        assert!(
            params(serde_json::json!({"action": "result", "result": 2}))
                .command()
                .unwrap_err()
                .contains("player")
        );
        assert!(
            params(serde_json::json!({"action": "result", "player": 0}))
                .command()
                .unwrap_err()
                .contains("result")
        );
        let schema = serde_json::to_value(schemars::schema_for!(SimParams)).unwrap();
        assert!(
            schema["$defs"]["SimAction"]
                .to_string()
                .contains("\"result\"")
        );
        assert!(schema["properties"].get("result").is_some());
        let tools = ViewerServer::tool_router().list_all();
        let sim = tools.iter().find(|t| t.name == "viewer_sim").unwrap();
        assert!(
            sim.description
                .as_deref()
                .unwrap_or_default()
                .contains("result (protocol 29")
        );
    }

    #[test]
    fn sim_actions_map_to_commands() {
        let params =
            |value: serde_json::Value| -> SimParams { serde_json::from_value(value).unwrap() };
        assert_eq!(
            params(serde_json::json!({"action": "setup", "mode": 0, "teams": [0, 0]})).command(),
            Ok(Command::SimSetup {
                mode: Some(0),
                seed: None,
                nations: None,
                teams: Some(vec![0, 0]),
                game_speed: None,
                resource_setting: None,
                living_space: None,
                map_explored: None,
                slots: None,
                ki_enabled: None,
                campaign_difficulty: None,
                script_enabled: None,
                time_per_char: None,
                time_limit: None,
            })
        );
        // Protocol 26: slots, AI on, difficulty.
        assert_eq!(
            params(serde_json::json!({"action": "setup", "slots": [{"kind": "human"}, {"kind": "ai", "nation": 1, "ki_kind": 3}], "ki_enabled": true, "campaign_difficulty": 0})).command(),
            Ok(Command::SimSetup {
                mode: None,
                seed: None,
                nations: None,
                teams: None,
                game_speed: None,
                resource_setting: None,
                living_space: None,
                map_explored: None,
                slots: Some(vec![
                    nml_control::SlotSetup {
                        kind: nml_control::SlotSetupKind::Human,
                        nation: None,
                        team: None,
                        color: None,
                        handicap: None,
                        ki_kind: None,
                    },
                    nml_control::SlotSetup {
                        kind: nml_control::SlotSetupKind::Ai,
                        nation: Some(1),
                        team: None,
                        color: None,
                        handicap: None,
                        ki_kind: Some(3),
                    },
                ]),
                ki_enabled: Some(true),
                campaign_difficulty: Some(0),
                script_enabled: None,
                time_per_char: None,
                time_limit: None,
            })
        );
        // Protocol 28: script on, speaking time.
        assert_eq!(
            params(
                serde_json::json!({"action": "setup", "script_enabled": true, "time_per_char": 80})
            )
            .command(),
            Ok(Command::SimSetup {
                mode: None,
                seed: None,
                nations: None,
                teams: None,
                game_speed: None,
                resource_setting: None,
                living_space: None,
                map_explored: None,
                slots: None,
                ki_enabled: None,
                campaign_difficulty: None,
                script_enabled: Some(true),
                time_per_char: Some(80),
                time_limit: None,
            })
        );
        // `time_limit` and game type 2 go into the command unchanged.
        let Ok(Command::SimSetup {
            mode, time_limit, ..
        }) = params(serde_json::json!({"action": "setup", "mode": 2, "time_limit": 30})).command()
        else {
            panic!("sim_setup with time_limit");
        };
        assert_eq!((mode, time_limit), (Some(2), Some(30)));
        let halt = serde_json::json!({"player": 0, "kind": {"Halt": {"units": [65537]}}});
        assert_eq!(
            params(serde_json::json!({"action": "command", "command": halt.clone()})).command(),
            Ok(Command::SimCommand { command: halt })
        );
        assert_eq!(
            params(serde_json::json!({"action": "save", "path": "a.nmlrepl", "replay": true}))
                .command(),
            Ok(Command::SimSave {
                path: "a.nmlrepl".into(),
                replay: true,
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "load", "path": "a", "steps": 9})).command(),
            Ok(Command::SimLoad {
                path: "a".into(),
                steps: Some(9),
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "query", "player": 1})).command(),
            Ok(Command::SimQuery {
                player: Some(1),
                guid: None,
                ids: None,
                limit: None,
            })
        );
        assert_eq!(
            params(serde_json::json!({
                "action": "ki_query", "player": 1, "code": 20_004, "words": [1, 1, 10, 10, 0, 1402],
                "sight": "fog"
            }))
            .command(),
            Ok(Command::KiQuery {
                player: 1,
                code: 20_004,
                words: Some(vec![1, 1, 10, 10, 0, 1402]),
                sight: Some("fog".into()),
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "ki_query", "player": 0, "code": 20_782}))
                .command(),
            Ok(Command::KiQuery {
                player: 0,
                code: 20_782,
                words: None,
                sight: None,
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "ki_query", "code": 20_782})).command(),
            Err("ki_query needs player".into())
        );
        assert_eq!(
            params(serde_json::json!({"action": "ki_query", "player": 0})).command(),
            Err("ki_query needs code".into())
        );
        assert_eq!(
            params(serde_json::json!({"action": "hash"})).command(),
            Ok(Command::SimHash)
        );
        assert_eq!(
            params(serde_json::json!({"action": "path", "unit": 65537, "to": [1000.0, 2000.0]}))
                .command(),
            Ok(Command::SimPath {
                unit: Some(65537),
                from: None,
                to: [1000.0, 2000.0],
                layer: None,
                allow_override: false,
                show: true,
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "grid", "layer": 9, "rect": [0, 0, 3, 3]}))
                .command(),
            Ok(Command::SimGrid {
                layer: 9,
                rect: Some([0, 0, 3, 3]),
            })
        );
        assert!(
            params(serde_json::json!({"action": "path", "unit": 1}))
                .command()
                .is_err()
        );
        assert!(
            params(serde_json::json!({"action": "grid"}))
                .command()
                .is_err()
        );
        assert_eq!(
            params(serde_json::json!({"action": "move", "ids": [65537], "to": [1.0, 2.0], "cyclic": true}))
                .command(),
            Ok(Command::SimMove {
                units: vec![65537],
                to: [1.0, 2.0],
                append: false,
                cyclic: true,
                drag_to: None,
            })
        );
        assert!(
            params(serde_json::json!({"action": "move"}))
                .command()
                .is_err()
        );
        // Unit bar and formation drag (protocol 13).
        assert_eq!(
            params(serde_json::json!({"action": "unit_bar", "button": 513})).command(),
            Ok(Command::SimUnitBar {
                button: 513,
                target: None,
                ground: None,
            })
        );
        // Protocol 21: `target` and `to` (as `ground`) go to the target buttons.
        assert_eq!(
            params(serde_json::json!({"action": "unit_bar", "button": 480, "target": 65537}))
                .command(),
            Ok(Command::SimUnitBar {
                button: 480,
                target: Some(65_537),
                ground: None,
            })
        );
        assert_eq!(
            params(
                serde_json::json!({"action": "unit_bar", "button": 484, "to": [2624.0, 1600.0]})
            )
            .command(),
            Ok(Command::SimUnitBar {
                button: 484,
                target: None,
                ground: Some([2624.0, 1600.0]),
            })
        );
        assert!(
            params(serde_json::json!({"action": "unit_bar"}))
                .command()
                .is_err()
        );
        assert_eq!(
            params(serde_json::json!({"action": "move", "ids": [1], "to": [1.0, 2.0], "drag_to": [3.0, 2.0]}))
                .command(),
            Ok(Command::SimMove {
                units: vec![1],
                to: [1.0, 2.0],
                append: false,
                cyclic: false,
                drag_to: Some([3.0, 2.0]),
            })
        );
        // Economy (protocol 9).
        assert_eq!(
            params(
                serde_json::json!({"action": "setup", "resource_setting": 2, "living_space": 4})
            )
            .command(),
            Ok(Command::SimSetup {
                mode: None,
                seed: None,
                nations: None,
                teams: None,
                game_speed: None,
                resource_setting: Some(2),
                living_space: Some(4),
                map_explored: None,
                slots: None,
                ki_enabled: None,
                campaign_difficulty: None,
                script_enabled: None,
                time_per_char: None,
                time_limit: None,
            })
        );
        // Vision (protocol 10).
        assert_eq!(
            params(serde_json::json!({"action": "setup", "map_explored": true})).command(),
            Ok(Command::SimSetup {
                mode: None,
                seed: None,
                nations: None,
                teams: None,
                game_speed: None,
                resource_setting: None,
                living_space: None,
                map_explored: Some(true),
                slots: None,
                ki_enabled: None,
                campaign_difficulty: None,
                script_enabled: None,
                time_per_char: None,
                time_limit: None,
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "fog", "player": 1, "rect": [0, 0, 3, 3]}))
                .command(),
            Ok(Command::SimFog {
                player: Some(1),
                rect: Some([0, 0, 3, 3])
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "view", "player": 2, "fog_shown": false}))
                .command(),
            Ok(Command::SimView {
                active_player: Some(2),
                fog_shown: Some(false),
                fog_overlay: None
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "build", "guid": 1001, "to": [996.0, 996.0], "rotation_steps": 1, "ids": [65537], "append": true}))
                .command(),
            Ok(Command::SimBuild {
                guid: 1001,
                to: [996.0, 996.0],
                rotation_steps: 1,
                units: vec![65537],
                append: true,
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "train", "building": 9, "guid": 1100})).command(),
            Ok(Command::SimTrain {
                building: 9,
                guid: 1100
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "cancel", "building": 9})).command(),
            Ok(Command::SimCancel {
                building: 9,
                index: 0
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "complete", "building": 9, "index": 2})).command(),
            Ok(Command::SimComplete {
                building: 9,
                index: 2,
                guid: None,
            })
        );
        assert_eq!(
            params(
                serde_json::json!({"action": "complete", "building": 9, "index": 2, "guid": 1100})
            )
            .command(),
            Ok(Command::SimComplete {
                building: 9,
                index: 2,
                guid: Some(1100),
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "sell", "ids": [9]})).command(),
            Ok(Command::SimSell { buildings: vec![9] })
        );
        assert_eq!(
            params(serde_json::json!({"action": "action", "target": 5, "append": true})).command(),
            Ok(Command::SimAction {
                units: Vec::new(),
                target: Some(5),
                to: None,
                append: true,
                cyclic: false,
            })
        );
        // Upgrades (protocol 16): player is the target slot.
        assert_eq!(
            params(serde_json::json!({"action": "upgrade", "player": 8, "guid": 2806})).command(),
            Ok(Command::SimUpgrade {
                player: 8,
                guid: 2806,
                remove: false,
            })
        );
        assert_eq!(
            params(
                serde_json::json!({"action": "upgrade", "player": 0, "guid": 2806, "remove": true})
            )
            .command(),
            Ok(Command::SimUpgrade {
                player: 0,
                guid: 2806,
                remove: true,
            })
        );
        for missing in [
            serde_json::json!({"action": "upgrade", "guid": 2806}),
            serde_json::json!({"action": "upgrade", "player": 0}),
            serde_json::json!({"action": "upgrade", "player": 0, "guid": 4294967295u32}),
        ] {
            assert!(params(missing.clone()).command().is_err(), "{missing}");
        }
        // Tents, trade and tribute (protocol 17): player is the sender, target the target slot.
        assert_eq!(
            params(serde_json::json!({"action": "trade", "player": 1, "ware": 2, "target": 3}))
                .command(),
            Ok(Command::SimTrade {
                player: 1,
                units: Vec::new(),
                ware: 2,
                target: 3,
            })
        );
        assert_eq!(
            params(
                serde_json::json!({"action": "trade", "player": 0, "ids": [7, 8], "ware": 0, "target": 8})
            )
            .command(),
            Ok(Command::SimTrade {
                player: 0,
                units: vec![7, 8],
                ware: 0,
                target: 8,
            })
        );
        assert_eq!(
            params(serde_json::json!({
                "action": "tribute", "player": 0, "recipients": 6, "resource": 1, "amount": 100
            }))
            .command(),
            Ok(Command::SimTribute {
                player: 0,
                recipients: 6,
                resource: 1,
                amount: 100,
            })
        );
        for missing in [
            serde_json::json!({"action": "trade", "ware": 2, "target": 3}),
            serde_json::json!({"action": "trade", "player": 1, "target": 3}),
            serde_json::json!({"action": "trade", "player": 1, "ware": 2}),
            serde_json::json!({"action": "trade", "player": 1, "ware": 2, "target": 4294967295u32}),
            serde_json::json!({"action": "tribute", "recipients": 6, "resource": 1, "amount": 5}),
            serde_json::json!({"action": "tribute", "player": 0, "resource": 1, "amount": 5}),
            serde_json::json!({"action": "tribute", "player": 0, "recipients": 6, "amount": 5}),
            serde_json::json!({"action": "tribute", "player": 0, "recipients": 6, "resource": 1}),
        ] {
            assert!(params(missing.clone()).command().is_err(), "{missing}");
        }
        assert_eq!(
            params(serde_json::json!({"action": "menu", "building": 9})).command(),
            Ok(Command::SimMenu {
                units: None,
                building: Some(9)
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "menu"})).command(),
            Ok(Command::SimMenu {
                units: Some(Vec::new()),
                building: None
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "menu", "ids": [3], "building": 9})).command(),
            Err("ids and building are mutually exclusive".into())
        );
        assert_eq!(
            params(serde_json::json!({"action": "hire_menu", "player": 0, "art": 2, "target_player": 1}))
                .command(),
            Ok(Command::SimHireMenu {
                player: 0,
                art: 2,
                target_player: Some(1),
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "hire_menu", "player": 1, "art": 0})).command(),
            Ok(Command::SimHireMenu {
                player: 1,
                art: 0,
                target_player: None,
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "hire_menu", "art": 0})).command(),
            Err("hire_menu needs player".into())
        );
        assert_eq!(
            params(serde_json::json!({"action": "hire_menu", "player": 0})).command(),
            Err("hire_menu needs art".into())
        );
        for missing in [
            serde_json::json!({"action": "build", "to": [1.0, 2.0]}),
            serde_json::json!({"action": "build", "guid": 1001}),
            serde_json::json!({"action": "train", "guid": 1100}),
            serde_json::json!({"action": "train", "building": 9}),
            serde_json::json!({"action": "cancel"}),
            serde_json::json!({"action": "complete"}),
        ] {
            assert!(params(missing.clone()).command().is_err(), "{missing}");
        }
        assert!(
            params(serde_json::json!({"action": "save"}))
                .command()
                .is_err()
        );
        assert!(
            params(serde_json::json!({"action": "command"}))
                .command()
                .is_err()
        );
    }

    #[test]
    fn the_sim_tool_schema_offers_the_economy_actions_and_parameters() {
        let schema = serde_json::to_value(schemars::schema_for!(SimParams)).unwrap();
        let text = schema["$defs"]["SimAction"].to_string();
        for action in [
            "build",
            "train",
            "cancel",
            "complete",
            "sell",
            "action",
            "menu",
            "upgrade",
            "trade",
            "tribute",
            "hire_menu",
        ] {
            assert!(
                text.contains(&format!("\"{action}\"")),
                "action {action} missing"
            );
        }
        let properties = &schema["properties"];
        for field in [
            "rotation_steps",
            "building",
            "index",
            "target",
            "resource_setting",
            "living_space",
            "remove",
            "ware",
            "recipients",
            "resource",
            "amount",
            "art",
            "target_player",
        ] {
            assert!(properties.get(field).is_some(), "parameter {field} missing");
        }
    }

    #[test]
    fn the_sim_tool_documents_ki_query_of_protocol_23() {
        // The action ki_query with its parameters, protocol 23 and the new fields of the query are in
        // the descriptions; the protocol 22 test stays untouched.
        let schema = serde_json::to_value(schemars::schema_for!(SimParams)).unwrap();
        let action = schema["$defs"]["SimAction"].to_string();
        let tools = ViewerServer::tool_router().list_all();
        let sim = tools
            .iter()
            .find(|t| t.name == "viewer_sim")
            .expect("tool viewer_sim");
        let description = sim.description.as_deref().unwrap_or_default();
        assert!(action.contains("ki_query"), "{action}");
        for part in [
            "protocol 23",
            "ki_query",
            "omniscient",
            "ai_marks",
            "attack_reports",
            "leaves the checksum unchanged",
        ] {
            assert!(description.contains(part), "description: {part}");
        }
        for (field, part) in [
            ("code", "ki_query"),
            ("words", "ki_query"),
            ("sight", "omniscient"),
            ("player", "ki_query"),
        ] {
            let text = schema["properties"][field]["description"].to_string();
            assert!(text.contains(part), "{field}: {text}");
        }
    }

    #[test]
    fn the_sim_tool_documents_the_headhunter_of_protocol_22() {
        // Button 540, the action hire_menu, the command HireHeadhunter, protocol 22 and the new
        // fields are in the descriptions. All assertions of the protocol 21 test stay: buttons
        // 454-457, 480-482, 484, 541, 542, 546, 548, 551, 556, the parameters target/to and
        // protocol 21, the buttons of the protocols 18 (430/431), 19
        // (434/435/437/438/458/459/554/555/564) and 20 (436/439/441-443/547/550/553/558/562).
        const GATES: [&str; 2] = ["430", "431"];
        const ABILITIES: [&str; 9] = [
            "434", "435", "437", "438", "458", "459", "554", "555", "564",
        ];
        const EVOCATIONS: [&str; 10] = [
            "436", "439", "441", "442", "443", "547", "550", "553", "558", "562",
        ];
        const SPELLS: [&str; 14] = [
            "454", "455", "456", "457", "480", "481", "482", "484", "541", "542", "546", "548",
            "551", "556",
        ];
        const ROB: [&str; 1] = ["540"];
        let schema = serde_json::to_value(schemars::schema_for!(SimParams)).unwrap();
        let button = schema["properties"]["button"]["description"].to_string();
        let target = schema["properties"]["target"]["description"].to_string();
        let to = schema["properties"]["to"]["description"].to_string();
        let action = schema["$defs"]["SimAction"].to_string();
        let tools = ViewerServer::tool_router().list_all();
        let sim = tools
            .iter()
            .find(|t| t.name == "viewer_sim")
            .expect("tool viewer_sim");
        let description = sim.description.as_deref().unwrap_or_default();
        for number in GATES
            .into_iter()
            .chain(ABILITIES)
            .chain(EVOCATIONS)
            .chain(SPELLS)
            .chain(ROB)
        {
            assert!(button.contains(number), "button: {number}");
            assert!(action.contains(number), "SimAction: {number}");
            assert!(description.contains(number), "description: {number}");
        }
        for version in [
            "protocol 18",
            "protocol 19",
            "protocol 20",
            "protocol 21",
            "protocol 22",
        ] {
            assert!(description.contains(version), "description: {version}");
        }
        for version in ["protocol 19", "protocol 20", "protocol 21", "protocol 22"] {
            assert!(button.contains(version), "button: {version}");
        }
        // Protocol 22: the menu and the hiring command.
        assert!(action.contains("hire_menu"), "{action}");
        assert!(action.contains("HireHeadhunter"), "{action}");
        assert!(description.contains("hire_menu"));
        assert!(description.contains("HireHeadhunter"));
        assert!(target.contains("protocol 21"), "{target}");
        assert!(to.contains("protocol 21"), "{to}");
        // The sections of the query of protocol 19.
        for field in ["masking", "assassin", "healing", "message_filter", "look"] {
            assert!(description.contains(field), "description: {field}");
        }
        // The sections of protocol 20 precisely: "ghost" is also contained in "evoke_ghost", "animal"
        // already in the text of protocol 12, hence with the bracket of the field layout.
        for field in [
            "evoke_ghost (evoking",
            "ghost (status",
            "banish (active",
            "evoke_animal (evoking",
            "influence (guids",
        ] {
            assert!(description.contains(field), "description: {field}");
        }
        // The sections of protocol 21 with their field layout and the new projectile kinds.
        for field in [
            "bomberman (target, extra_energy, return_point, busy",
            "killer (target, extra_energy, near_damage, far_damage",
            "special_move (busy, mode",
            "target (target point",
            "corpse (corpse",
            "dynamite",
            "explosion",
            "storm",
            "pulse 0 or 1",
        ] {
            assert!(description.contains(field), "description: {field}");
        }
        // The sections of protocol 22 with their field layout, the table and the projectile kind.
        for field in [
            "headhunter (art",
            "robbing (anim",
            "headhunters (",
            "throw",
            "impact",
        ] {
            assert!(description.contains(field), "description: {field}");
        }
        // The buttons of protocol 20 and 21 act only on the first selected own object (unlike those
        // of protocol 19); the hint belongs in both button texts.
        assert!(description.contains("first selected own object"));
        assert!(button.contains("first selected own object"), "{button}");
    }

    #[test]
    fn effects_actions_map_to_commands() {
        let params =
            |value: serde_json::Value| -> EffectsParams { serde_json::from_value(value).unwrap() };
        assert_eq!(
            params(serde_json::json!({"action": "spawn", "path": "data/effects/feuer.pef", "object_index": 5})).command(),
            Ok(Command::SpawnEffect {
                path: "data/effects/feuer.pef".into(),
                position: None,
                object_index: Some(5),
                scale: None,
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "clear"})).command(),
            Ok(Command::ClearEffects)
        );
        assert_eq!(
            params(serde_json::json!({"action": "set_damage", "value": 2.5})).command(),
            Ok(Command::SetDamage { value: 2.5 })
        );
        assert!(
            params(serde_json::json!({"action": "spawn"}))
                .command()
                .is_err()
        );
        assert!(
            params(serde_json::json!({"action": "set_damage"}))
                .command()
                .is_err()
        );
    }

    #[test]
    fn audio_actions_map_to_commands() {
        let params =
            |value: serde_json::Value| -> AudioParams { serde_json::from_value(value).unwrap() };
        assert_eq!(
            params(serde_json::json!({"action": "play_sound", "guid": 4104, "sound_action": 200, "position": [1.0, 0.0, 2.0]})).command(),
            Ok(Command::PlaySound {
                guid: Some(4104),
                action: Some(200),
                path: None,
                position: Some([1.0, 0.0, 2.0]),
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "music_attack", "on": true})).command(),
            Ok(Command::Music {
                action: MusicCommand::Attack,
                track: None,
                on: Some(true),
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "set_volume", "mute": true})).command(),
            Ok(Command::SetVolume {
                music: None,
                sfx: None,
                speech: None,
                mute: Some(true),
                unit_response: None,
            })
        );
        assert_eq!(
            params(serde_json::json!({"action": "capture_stop"})).command(),
            Ok(Command::AudioCapture {
                action: CaptureCommand::Stop,
                path: None,
            })
        );
        for missing in ["music_play", "music_attack", "capture_start"] {
            assert!(
                params(serde_json::json!({ "action": missing }))
                    .command()
                    .is_err(),
                "{missing}"
            );
        }
    }

    #[tokio::test]
    async fn audio_tool_returns_the_audio_section() {
        let (port, rx) = crate::session::tests::fake_viewer(usize::MAX);
        let server = ViewerServer::new(Session::new(Err("no repo".into())));
        server
            .viewer_attach(Parameters(AttachParams { port }))
            .await
            .unwrap();
        let audio = |value: serde_json::Value| Parameters(serde_json::from_value(value).unwrap());
        let result = server
            .viewer_audio(audio(
                serde_json::json!({"action": "music_play", "track": "title"}),
            ))
            .await
            .unwrap();
        let ContentBlock::Text(text) = &result.content[0] else {
            panic!("text expected");
        };
        let state: nml_control::AudioState = serde_json::from_str(&text.text).unwrap();
        assert_eq!(state.music.status, "normal");
        let sent: Vec<Command> = rx.try_iter().collect();
        assert!(sent.contains(&Command::Music {
            action: MusicCommand::Play,
            track: Some("title".into()),
            on: None,
        }));
    }

    #[tokio::test]
    async fn environment_tool_lists_lights_and_returns_the_state() {
        let (port, rx) = crate::session::tests::fake_viewer(usize::MAX);
        let server = ViewerServer::new(Session::new(Err("no repo".into())));
        server
            .viewer_attach(Parameters(AttachParams { port }))
            .await
            .unwrap();
        let env = |value: serde_json::Value| Parameters(serde_json::from_value(value).unwrap());
        let result = server
            .viewer_environment(env(serde_json::json!({"action": "lights"})))
            .await
            .unwrap();
        let ContentBlock::Text(names) = &result.content[0] else {
            panic!("text expected");
        };
        let names: Vec<String> = serde_json::from_str(&names.text).unwrap();
        assert_eq!(names, ["level", "Sommer Tag"]);
        let result = server
            .viewer_environment(env(
                serde_json::json!({"action": "set_weather", "kind": "rain"}),
            ))
            .await
            .unwrap();
        let ContentBlock::Text(state) = &result.content[0] else {
            panic!("text expected");
        };
        let state: nml_control::StateSnapshot = serde_json::from_str(&state.text).unwrap();
        assert_eq!(state.environment.unwrap().light, "Sommer Nacht");
        let result = server
            .viewer_environment(env(serde_json::json!({"action": "set_light"})))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true), "nothing sent without name");
        let sent: Vec<Command> = rx.try_iter().collect();
        assert!(sent.contains(&Command::SetWeather {
            kind: WeatherName::Rain
        }));
        assert!(!sent.iter().any(|c| matches!(c, Command::SetLight { .. })));
    }

    #[tokio::test]
    async fn tools_answer_with_text_and_errors() {
        let (port, _rx) = crate::session::tests::fake_viewer(usize::MAX);
        let server = ViewerServer::new(Session::new(Err("no repo".into())));
        let result = server.viewer_state().await.unwrap();
        assert_eq!(result.is_error, Some(true), "no viewer yet");
        let result = server
            .viewer_attach(Parameters(AttachParams { port }))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        let result = server.viewer_state().await.unwrap();
        let ContentBlock::Text(state) = &result.content[0] else {
            panic!("text expected");
        };
        let state: nml_control::StateSnapshot = serde_json::from_str(&state.text).unwrap();
        assert_eq!(state, nml_control::StateSnapshot::example());
        let result = server
            .viewer_screenshot(Parameters(ScreenshotParams {
                overlay: true,
                max_size: None,
            }))
            .await
            .unwrap();
        assert!(matches!(&result.content[0], ContentBlock::Image(_)));
    }
}
