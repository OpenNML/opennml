//! Requests and responses: one JSON line each, `{"id": …, "cmd": "…", …}` and
//! `{"id": …, "ok": …}` / `{"id": …, "error": "…"}`.

use serde::{Deserialize, Serialize};

use crate::keys::check_key;

/// Version of the protocol (response to `hello`); it increases with every extension of the control channel
/// (2: light and weather, 3: shadows, 4: effects, 5: sound, 6: simulation, 7: pathfinding, 8: movement,
/// 9: economy, 10: sight, 11: melee, 12: ranged combat, projectiles, animals and hunting, 13: groups and formations,
/// 14: riding, canoes, swimming and diving, 15: ships, train and multipart objects, 16: upgrades,
/// counter-upgrades, temporary upgrades, spells and arming, 17: tents, travois, trade and
/// tribute, 18: gates and enemy watch: gate state, door tiles and area guard in `sim_query`,
/// 19 = camouflage, digging in, healing (module states, message filters, presentation per viewer),
/// 20 = ghosts, banishing, animal evocation, auras (module states, influence),
/// 21 = spells and special attacks (target point, corpse, demolition expert, killer, smith, world effects,
/// target buttons), 22 = headhunters and robbing (headhunter and robber state, table, throw,
/// hire menu, button 540), 23 = AI queries (`ki_query`, `ai_marks` and `attack_reports` in
/// `sim_query`), 24 = menu UI (`ui_state`, `ui_click`, `ui_key`, `ui_open`), 25 = videos (`ui_video`,
/// `ui_state.video`/`pointer_hidden`, effects `play_video`/`stop_video`), 26 = AI match (`ki_state`,
/// `ki_command`, `slots`/`ki_enabled`/`campaign_difficulty` in `sim_setup`), 27 = match from the menu
/// (`ui_state.game`, game effects in `effects`, `sim_hash`/`sim_query`/`sim_command` in `opennml-game`), 28 =
/// `script_state`, `sim_setup` `script_enabled`/`time_per_char`, 29 = game end, accounting, campaign (`sim_result`,
/// `ui_state.game.result`, `ui_state.accounting`/`play_time`, effects `game_end`, `accounting`,
/// `surrender`, `start_campaign_level`, `book_campaign`), 30 = world state and mission objectives of the scripts
/// (`sim_query` `name`/`hidden` per object, `kill_counts`, `button_locks`, `slot_running`, `area_views`;
/// `script_state` `main_quests`/`sub_quests`, `countdown_container`/`countdown_s`, `task_status`, `area_overrides`;
/// the level container has ID 1), 31 = game UI (`ui_state.game.hud`, viewer command `hud`), 32 =
/// `script_command` (`skip_dialog`), script commands in the block log (`script_state` `log` carries the new
/// message `ScriptTrace::Command` as JSON entry `{"Command": …}`; plus `commands` and `results`), `sim_command`
/// accepts `DebugDestroy`), 33 = `script_events` (presentation events of the
/// last steps, presentation state, UI state, profile variables, camera of the scripts), `script_state.ui`,
/// `sim_command` accepts `ScriptSelection` and `ScriptButton`.
/// 34 = `ui_click`/`ui_state` for `GAMEUI`, mouse commands in `opennml-game`.
/// 35 = `ui_present`, `ui_state.game.presentation` (with `info_frames`, `countdown`, `screen_text`).
/// 36 = `key` with `repeat`, `ui_focus_lost`, `ui_state.game.input`.
/// 37 = effect `diplomacy` in `ui_state.effects` (fixed form with `action`,
/// `slot`, `value`), effect `send_chat` and simulation command `Chat` (`0x5D`), simulation command
/// `Signal` (`0x5C`), `ui_state.game.minimap`, `diplomacy_open` and `chat_lines` as well as the `GAMEUI` elements
/// `minimap`, `minimap_mode`, `minimap_idle`, `minimap_signal`, `minimap_idle_all`, `minimap_menu` and
/// `minimap_diplomacy`.
/// 38 = `world_display` in the state snapshot and `ui_state.game.world` (state of the world display: flag,
/// marks, wheel track points, water rings, silhouettes, beams, charges); `sim_build` sends the ground height at the
/// target point (at least the water height) as the height instead of 0, fields unchanged.
/// 39 = `sim.flags`/`ui_state.game.flags`, lobby color 0 to 8, `sim_setup` with `time_limit` and game type 2,
/// `ui_state.game.presentation.timers`.
/// 40 = `random_map` (random map of the preview: seed, script, defines, size, player areas,
/// random number consumption, checksums).
/// 41 = `random_map` with objects (counts and checksums of the objects, object count per GUID, occupied slots,
/// start points); `random_map` also in `opennml-game` (random map of the running match, otherwise `null`);
/// `ui_state.effects` reports `start_match` of a random map with `level` = `randommap:<Seed>`.
/// 42 = network match in `opennml-game` (`net_status`, `net_part_hashes`, `net_save`, `net_debug`; `pause` and
/// `step` are rejected in the network, `sim_command`/`sim_result` only for the own slot).
/// 43 = quirk O-331: `mouse_button` with `"middle"` (middle button, `KD_CAM_FREEHAND`, only `opennml-game`); `ui_click`
/// rejects `"middle"`.
/// 44 = `ui_state.effects` with `net` (`code`, `command`), `search` (`on`) and `lobby`
/// (`action`); `net_status` with `role` as `null` during the search, the phases `searching`, `connecting` and `lobby`
/// and the fields `spacing`, `lead`, `build`, `rtt_ms`, `lobby` and `found`; `ui_*` are allowed during search and lobby
/// (locked only while loading and on the command-line path until `Go`), `pause`/`step` stay locked there.
/// 45 = menu level: `ui_state.menu_level` (`level`, `shown`, `steps`, `checksum`; missing without a menu level)
/// and `ui_state.effects` with `menu_level` (`load` = path for `0x21`, missing for `0x20`); `sim_hash` and
/// `sim_query` in `opennml-game` also read the menu level, `sim_command` and `sim_result` reject it.
/// 46 = `net_status.lobby.saved` (name of the network savegame of a loaded session, otherwise `null`) and
/// `net_status.lobby.rows[].seat` (player slot of the row in a loaded lobby, otherwise `null`).
/// 47 = `MAX_REPLAY_STEPS` 72,000 -> 576,000 ticks (eight hours of game time);
/// `ui_state.effects` with `play_replay` (`name`, command `0x19`); `ui_state.game.replay` (`mode` `record`/`play`,
/// `file`, `step`, `ended`; missing without recording and replay).
/// 48 = 3D mouse pointer: `screenshot.pointer` (default `false`;
/// draws the pointer, only `opennml-game`) and `ui_state.pointer_model` (`id`, `animation`, `tint`, `phase_ms`,
/// `software`; missing in the viewer and without a loaded model).
/// 49 = sound driver: `ui_state.effects` with `set_sound_driver`
/// (`index`, device switch in `SOUNDOPTIONS`).
/// 50 = host migration: `net_status.successors` (successor list: `peer`, `slot`,
/// `addr`) and `net_status.migration` (`phase` `dialing`/`rejoining`/`gathering`, `peer`, `expected`, `arrived`);
/// `net_debug` with `mute` and `unmute`.
/// Next version (number set on release) = game data check: `net_status.lobby.session_revision` (lobby revision of
/// the last session change), `net_status.lobby.rules_checksum` (the host's rules checksum as hex text or `null`) and
/// `net_status.lobby.rows[].data` (`pending`, `equal`, `missing` or `differs` with `files` and `more`).
pub const PROTOCOL_VERSION: u32 = 50;

/// Error message of the control channel when the viewer no longer accepts commands (event loop
/// ended); after that it closes the connection.
pub const VIEWER_GONE: &str = "Viewer terminated";

/// Maximum number of steps of a `step`.
pub const MAX_STEP_FRAMES: u32 = 10_000;

/// Largest `GameSpeed` (`sim_setup`).
pub const MAX_GAME_SPEED: u8 = 30;

/// Maximum number of objects in a `sim_query` response (default 100).
pub const MAX_QUERY_LIMIT: usize = 10_000;

/// Largest engine tick of a replay (`sim_load`): eight hours of game time at 50 ms ticks.
pub const MAX_REPLAY_STEPS: u64 = 576_000;

/// Maximum number of words of `words` in `ki_query` (protocol 23): the query fields of the AI have
/// up to 128 words from `[2]` on (`opennml_sim::ki::query::QUERY_WORDS`).
pub const MAX_KI_QUERY_WORDS: usize = 128;

/// Maximum number of arguments of `ki_command start` (protocol 26; equal to
/// `opennml_sim::ki::run::MAX_START_ARGS`).
pub const MAX_KI_ARGS: usize = 64;

/// Maximum number of passes of `ki_command step` (protocol 26; equal to
/// `opennml_sim::ki::run::MAX_STEP_PASSES`).
pub const MAX_KI_STEP_PASSES: u32 = 1000;

/// Campaign difficulty of `sim_setup` (protocol 26; equal to `opennml_sim::setup::DIFFICULTY_RANGE`).
pub const CAMPAIGN_DIFFICULTY_RANGE: std::ops::RangeInclusive<i32> = 0..=2;

/// Speech time per character of `sim_setup` in ms (protocol 28). Only the control channel limits the value; the
/// simulation computes with any value (`speech_ms` wrapped) and no longer checks it.
pub const TIME_PER_CHAR_RANGE: std::ops::RangeInclusive<i32> = 1..=1000;

/// Time limit of `sim_setup` in minutes (equal to `1..=opennml_sim::setup::MAX_TIME_LIMIT`, `opennml-control`
/// does not depend on `opennml-sim`; `· 60 · 1000` then fits in `i32`).
pub const TIME_LIMIT_RANGE: std::ops::RangeInclusive<i32> = 1..=35_791;

/// Maximum number of steps of the block log of `script_state` (protocol 28; the viewer collects this
/// many steps).
pub const MAX_SCRIPT_LOG_STEPS: u32 = 1000;

/// Default number of steps of the block log of `script_state` (protocol 28).
pub const DEFAULT_SCRIPT_LOG_STEPS: u32 = 50;

/// Largest free handicap of a slot in `sim_setup` (the six levels 0-5 of the lobby).
pub const MAX_SLOT_HANDICAP: i32 = 5;

/// AI kinds of an AI slot in `sim_setup` (1-4; handicap 5/2/0/0, `opennml_sim::setup::ki_kind_handicap`).
pub const KI_KIND_RANGE: std::ops::RangeInclusive<u8> = 1..=4;

/// Maximum number of slots of `sim_setup` `slots`.
pub const MAX_SLOTS: usize = 8;

/// Maximum number of `stack_top` values per thread in `ki_state`.
pub const KI_STACK_TOP: usize = 8;

/// Maximum number of cells of a `sim_grid` section (64 x 64).
pub const MAX_GRID_CELLS: i64 = 4096;

/// Display kind of the occupancy grid in `sim_grid` (the layers are 0-8).
pub const GRID_OCCUPANCY: u8 = 9;

/// Largest number of quarter turns of `sim_build` (0-3 = 0, 90, 180, 270 degrees).
pub const MAX_ROTATION_STEPS: u8 = 3;

/// Largest target slot of `sim_upgrade` (slots 0-10, `opennml_sim::id::SLOT_COUNT − 1`).
pub const MAX_UPGRADE_SLOT: u8 = 10;

/// Largest index of `sim_cancel` and `sim_complete`: 0 = running order, 1-26 = waiting ones (queue
/// `m_MakeQueue` with at most 26 entries).
pub const MAX_CANCEL_INDEX: i32 = 26;

/// Largest resource setting (`sim_setup` `resource_setting`: 0 = start balance of the map, 1-3 =
/// `resourcenconfig.ini`).
pub const MAX_RESOURCE_SETTING: i32 = 3;

/// Largest living space level (`sim_setup` `living_space`: 0-5 = 75 ... 200, default 2 = 125).
pub const MAX_LIVING_SPACE: i32 = 5;

fn yes() -> bool {
    true
}

/// `repeat` of `key` is missing from the line when it is 0 (older lines stay the same).
fn is_zero(value: &u16) -> bool {
    *value == 0
}

/// A request: ID and command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

/// `down`/`up` like a held key, `tap` = down and immediately up again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    Down,
    Up,
    Tap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButtonName {
    Left,
    Right,
    /// Middle button (quirk O-331): only for `mouse_button`; `ui_click` rejects it.
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowModeName {
    Windowed,
    Borderless,
    Exclusive,
}

/// Weather of the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherName {
    None,
    Rain,
    Snow,
}

/// Largest values of `set_quality` (`Engine.ini` `ShadowQuality`, `ParticlesQuality`,
/// `TerrainQuality`).
pub const MAX_QUALITY: [u8; 3] = [3, 2, 2];

/// Highest value of the volume controls (`set_volume`).
pub const MAX_VOLUME: u16 = 1000;

/// Subcommand of `music`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicCommand {
    /// Play menu music (`track`); the level music fades out.
    Play,
    /// Fade out all music.
    Stop,
    /// Combat music on/off by hand (`on`), like Shift+F9.
    Attack,
}

/// Check aid of `net_debug`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetDebugAction {
    /// Corrupt the own state (time of day, `SimHost::debug_corrupt`): the next checksum comparison is a
    /// desync.
    Corrupt,
    /// Cut all connections without `Bye`: the counterparts see a dropout or abort.
    Drop,
    /// Mute all connections: they send nothing any more, not even keepalives. The counterparts see a dropout
    /// after 30 s of silence.
    Mute,
    /// Cancels `mute`. Discarded messages are not delivered later; the match can no longer be continued for
    /// the counterparts afterwards.
    Unmute,
}

/// Subcommand of `audio_capture`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCommand {
    Start,
    Stop,
}

/// Commands (field `cmd`), parameters as further fields of the line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Protocol version, PID, backend and adapter.
    Hello,
    /// Key in the spelling of the winit `KeyCode` variants (`KEY_NAMES`). With `repeat` (only with `action: down`)
    /// the press is followed by `repeat` further press events as automatic repetition (autorepeat, each
    /// its own event as in the original, O-300); if it is missing, 0.
    Key {
        code: String,
        action: KeyAction,
        #[serde(default, skip_serializing_if = "is_zero")]
        repeat: u16,
    },
    /// Mouse pointer in window pixels.
    MouseMove {
        x: f64,
        y: f64,
    },
    /// Mouse pointer leaves the window.
    MouseLeave,
    MouseButton {
        button: MouseButtonName,
        down: bool,
    },
    /// One notch of the mouse wheel.
    Wheel {
        up: bool,
    },
    /// Stop the virtual clock.
    Pause,
    /// Let the clock run on in real time.
    Resume,
    /// `frames` logic steps of `frame_ms` each, then one frame; the clock stays stopped.
    Step {
        frames: u32,
        frame_ms: f64,
    },
    /// Current frame as PNG (Base64), optionally with overlay; with `max_size` the PNG is
    /// scaled down (longest side at most `max_size`), the checksum applies to the full image.
    Screenshot {
        #[serde(default)]
        overlay: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_size: Option<u32>,
        /// With the mouse pointer at the current mouse position: sprite or 3D model as in the window, only
        /// in `opennml-game`; the viewer shows the system pointer and ignores the field. Default off.
        #[serde(default)]
        pointer: bool,
    },
    /// State snapshot.
    State,
    /// State of the menu UI (protocol 24): dialogs, elements, focus, images, last
    /// effects ([`crate::UiStateData`]). Only with an open UI (viewer with `--dialog` in a window,
    /// `opennml-game`); like all `ui_*` commands.
    UiState,
    /// Click on the center of an element (part of the last frame): movement, press, release in the input of the
    /// next frame, like the mouse by hand (protocol 24). With `dialog: "GAMEUI"` an element of the command bar
    /// of the running match in `opennml-game` (`docs/viewer-control.md`, section "Command bar").
    UiClick {
        dialog: String,
        element: String,
        /// Default `left`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<MouseButtonName>,
    },
    /// Key event of the UI: `chord` of key names of the original (`ESCAPE`, `CONTROL H`; case-insensitive,
    /// in press order), `char` the character (cp1252), optional (protocol 24).
    UiKey {
        chord: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        char: Option<u16>,
    },
    /// Queue a dialog for opening like command 5 (protocol 24).
    UiOpen {
        dialog: String,
    },
    /// Simulate the loss of window focus (only `opennml-game`): releases held keys and triggers the focus pause of the
    /// original (O-154), also with an active control channel.
    UiFocusLost,
    /// Queue a video by name like `videostart` in the menu `campaignvideo`: closes open menus (`0x34`, `0x15`), `0x1D`,
    /// `0x1A`, `0x28`, reopens the menus (protocol 25); `name` without path and extension.
    UiVideo {
        name: String,
    },
    /// Trigger a presentation event by hand: `event` in the JSON form of
    /// `opennml_sim::script::Presentation` (e.g. `{"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}}`, unit variants
    /// as a string). Presentation only, no `SimHost` command, the checksum stays; the form is not checked until
    /// execution.
    UiPresent {
        event: serde_json::Value,
    },
    /// Set window mode and/or monitor.
    Window {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<WindowModeName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitor: Option<usize>,
    },
    /// Game UI of the viewer on/off (like `--hud`): message bar and status bar drawn by the
    /// UI renderer instead of the message lines in the overlay. Drawing only, the checksum stays.
    Hud {
        on: bool,
    },
    /// Respond, then quit cleanly.
    Quit,
    /// Random map of the preview (`--random-map`) or, in `opennml-game`, of the running match:
    /// `RandomMapState` or `null` without a random map. Read-only.
    RandomMap,
    /// Network match (only `opennml-game`): state of the network session ([`crate::NetStatusData`]: role,
    /// phase, port, own slot, slot assignment, turn, next execution tick, wait reason, missing slots,
    /// desyncs, silence per connection). Read-only; an error without a network session.
    NetStatus,
    /// Network match: checksums per state part of the current state ([`crate::NetPartHashesData`]). Read-only.
    NetPartHashes,
    /// Network match: save request like "Save" in the game menu: all machines write
    /// `savegame/multiplayer/<name>` after the execution tick of the turn that accepts the request.
    NetSave {
        name: String,
    },
    /// Network match, for check runs only: `corrupt` corrupts the own state at a harmless point (desync),
    /// `drop` cuts all connections without `Bye`.
    NetDebug {
        action: NetDebugAction,
    },
    /// Names of the light presets in the order of F6 (`level` = `.wld` first).
    Lights,
    /// Set light: `level` or the name of a preset (case-insensitive).
    SetLight {
        name: String,
    },
    /// Set weather (particles are created anew).
    SetWeather {
        kind: WeatherName,
    },
    /// Set quality levels (each optional); `shadow` recomputes the shadows of all objects,
    /// `particles` creates the effects anew.
    SetQuality {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shadow: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        particles: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terrain: Option<u8>,
    },
    /// Effect `path` (`.pef`) as a world effect of the current view: at `position` (world), at
    /// object `object_index` of the map (follows it) or - with neither - at the camera target of the map
    /// or at the origin of the browser; with scale `scale` (default 1).
    SpawnEffect {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<[f32; 3]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        object_index: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f32>,
    },
    /// Remove all effects of the current view created via `spawn_effect`.
    ClearEffects,
    /// Burn intensity in the model browser (0 to 3, steps of 0.25).
    SetDamage {
        value: f32,
    },
    /// Play a sound: `guid` + `action` (database, selection as in the game) or `path` (`.wav`;
    /// `.ogg` as dialog speech); with `position` as 3D sound, otherwise 2D (protocol 5).
    PlaySound {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        guid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<[f32; 3]>,
    },
    /// Music: `play` (+ `track`: `title`/`win`/`lose`/`credits` or path), `stop`,
    /// `attack` (+ `on`).
    Music {
        action: MusicCommand,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        track: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<bool>,
    },
    /// Volume controls (0-1000), muting and `UnitResponse` (selection voice of the units
    /// and command acknowledgment), each optional.
    SetVolume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        music: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sfx: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speech: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mute: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit_response: Option<bool>,
    },
    /// Offline mix: `start` (+ `path`) or `stop` (returns length and level).
    AudioCapture {
        action: CaptureCommand,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// New match on the same map; missing values as at start (settings); only
    /// `game_speed` changes the game speed without a restart.
    SimSetup {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nations: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        teams: Option<Vec<i32>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        game_speed: Option<u8>,
        /// Resource setting 0-3 (protocol 9; 0 = start balance of the map, 1-3 =
        /// `resourcenconfig.ini`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_setting: Option<i32>,
        /// Living space level 0-5 (protocol 9; 75, 100, 125, 150, 175, 200).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        living_space: Option<i32>,
        /// "Map explored" (protocol 10; `MapExplored`, default off).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        map_explored: Option<bool>,
        /// Lobby slots in order (protocol 26): kind, nation, team, color, handicap or AI kind;
        /// excludes `nations` and `teams`; only on multiplayer maps.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slots: Option<Vec<SlotSetup>>,
        /// Start the AI of the AI slots (protocol 26, `KiSetup::enabled`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ki_enabled: Option<bool>,
        /// Campaign difficulty 0-2 (protocol 26, `KiSetup::campaign_difficulty`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        campaign_difficulty: Option<i32>,
        /// Start the level script (protocol 28, `ScriptSetup::enabled`; viewer default off).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        script_enabled: Option<bool>,
        /// Speech time per character without a speech file in ms, 1-1000 (protocol 28,
        /// `ScriptSetup::time_per_char`, default 55).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_per_char: Option<i32>,
        /// Time limit in minutes, 1-35791 (`MatchSetup::time_limit`, default 20): victory time of game types 2/3
        /// and time limit of game type 4; as before if it is missing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_limit: Option<i32>,
    },
    /// Command to the simulation (JSON form of `opennml_sim::command::Command`), takes effect at the next
    /// tick.
    SimCommand {
        command: serde_json::Value,
    },
    /// Set the game result (protocol 29): slot `player` (0-7) loses
    /// (`result` 1) or wins (2), like the cheat of the original via command `0x56`
    /// (`CommandKind::SetPlayerResult`, sender the active player); takes effect at the next tick and is in
    /// the command log. Response `{"step"}` like `sim_command`.
    SimResult {
        player: u8,
        result: i32,
    },
    /// Query objects and players.
    SimQuery {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        player: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        guid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ids: Option<Vec<u32>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// AI query against the running match (protocol 23, re/ki-queries): the
    /// query `code` of slot `player` with the query words `words` (from `[2]` of the field
    /// `{code, argc, …}`; default `[player]`, at most 128) and the sight `sight`
    /// (`"omniscient"`, default, or `"fog"`). Read-only: direct changes (marking, taking
    /// an attack report) are reported, not executed.
    KiQuery {
        player: u8,
        code: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        words: Option<Vec<i32>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sight: Option<String>,
    },
    /// Write the savegame (or, with `replay`, the replay since the start) to a file.
    SimSave {
        path: String,
        #[serde(default)]
        replay: bool,
    },
    /// Load a savegame or replay (recognized by its ID); a replay runs up to
    /// engine tick `steps` (default: one tick after its last command).
    SimLoad {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u64>,
    },
    /// Checksum and that of the last ticks.
    SimHash,
    /// Plan a path like a FINDTO (`GameWorld_PlanHighPath`): for a unit (`unit`, layer from
    /// the walk module, `layer` replaces it) or from a world point `from` (then with `layer`) to the
    /// world point `to` (x, z); `allow_override` = FINDTO `0x100`; `show` shows the path in the frame.
    SimPath {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<[f32; 2]>,
        to: [f32; 2],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layer: Option<u8>,
        #[serde(default)]
        allow_override: bool,
        #[serde(default = "yes")]
        show: bool,
    },
    /// Walk like a right-click of the active player (command `0x04`): own units from
    /// `units` (empty = selection) to the world point `to` (x, z); `append` appends (Shift),
    /// `cyclic` appends cyclically (Ctrl, includes `append`). `drag_to` (protocol 13) is the
    /// world point where the right button was released: facing direction and spread of the
    /// formation come from dragging from `to` to `drag_to`; the command carries the selected
    /// formation of the unit bar.
    SimMove {
        #[serde(default)]
        units: Vec<u32>,
        to: [f32; 2],
        #[serde(default)]
        append: bool,
        #[serde(default)]
        cyclic: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drag_to: Option<[f32; 2]>,
    },
    /// Press a button of the unit bar for the current selection (protocol 13): 511-515 choose
    /// the formation (none, line, square, column, pincer), 519 scatters; protocol 14: 405
    /// dismount, 406 disembark, 432 dive, 433 surface; protocol 15: 449 train forward, 450
    /// train backward, 451 train halt (first locomotive of the selection); protocol 16: 485 arm, 486
    /// disarm, the abilities (543, 544, 545, 549, 552, 557, 559, 560, 561, 563; spells and
    /// temporary upgrades) and the counter-upgrades (3370-3377); protocol 17: 452 pack tent
    /// (the first selected object if it is an own tent), 453 unpack travois (opens the
    /// unpack cursor, the command is not issued until a left click on the map); protocol 21: the
    /// spell buttons 480, 481, 482 (without target), 484 and 541 and the special attacks 454-457, 542 (without target),
    /// 546, 548, 551, 556. The target buttons (all except 482 and 542) require a target: `target`
    /// (object ID) or `ground` (world coordinates `[x, z]`); the command goes immediately to the first
    /// selected own object, without targeting mode. 484 and 556 always take the ground point.
    /// Protocol 22: 540 robbing (`RobCorpses` to every selected own robber, without target).
    SimUnitBar {
        button: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ground: Option<[f32; 2]>,
    },
    /// Build like in the placement mode of the active player (command `0x01`, protocol 9): type
    /// `guid` at the world point `to` (x, z) with `rotation_steps` quarter turns (0-3), workers
    /// `units` (empty = selection); `append` queues the construction for the workers (Shift).
    SimBuild {
        guid: u32,
        to: [f32; 2],
        #[serde(default)]
        rotation_steps: u8,
        #[serde(default)]
        units: Vec<u32>,
        #[serde(default)]
        append: bool,
    },
    /// Train or trade in the building `building` (command `0x0E`, protocol 9).
    SimTrain {
        building: u32,
        guid: u32,
    },
    /// Cancel order `index` of the building `building` (command `0x0F`, protocol 9): 0 =
    /// running, 1 ... = waiting.
    SimCancel {
        building: u32,
        #[serde(default)]
        index: i32,
    },
    /// Debug "complete immediately" (command `DebugCompleteOrder`, no counterpart in the original):
    /// order `index` of the building `building` (0 = running, 1 ... = waiting) through the normal
    /// completion; an unpaid one runs through the start check first, a trade pays then.
    /// `guid` = expected type of the entry (the command carries it along; the simulation rejects
    /// it if a different one is there at execution); if it is missing, the viewer fills it from the
    /// current entry.
    SimComplete {
        building: u32,
        #[serde(default)]
        index: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        guid: Option<u32>,
    },
    /// Sell buildings (command `0x67`, protocol 9); empty = the own buildings of the selection.
    SimSell {
        #[serde(default)]
        buildings: Vec<u32>,
    },
    /// Right-click of the active player (protocol 9): action according to `targets.ini` for `units`
    /// (empty = selection) on the object `target` or the world point `to` (x, z; without `to` the position
    /// of `target`); `append`/`cyclic` like Shift/Ctrl (only for the actions that the original
    /// wraps; `cyclic` includes `append`).
    SimAction {
        #[serde(default)]
        units: Vec<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<[f32; 2]>,
        #[serde(default)]
        append: bool,
        #[serde(default)]
        cyclic: bool,
    },
    /// Build list of the workers `units` (empty = selection) or production list and queue
    /// of the building `building` (protocol 9).
    SimMenu {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        units: Option<Vec<u32>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        building: Option<u32>,
    },
    /// Apply upgrade `guid` like the console (command `0x39`) or remove it with `remove`
    /// (`0x3A`), protocol 16. The sender is the active player; `player` is the target slot 0-10
    /// (slot 8 writes the type table of the neutrals, W-213). Apply: `guid` < 0 applies all
    /// GUIDs 2800 to 3399; remove: `guid` > 0. Without an ownership check, as in the original.
    SimUpgrade {
        player: u8,
        guid: i32,
        #[serde(default)]
        remove: bool,
    },
    /// Trade with a player (command `0x45`, protocol 17): per cart from `units` (empty = selection)
    /// `{0x12B, ware, target}` with abort. `ware` 0 to 3 (buy wood, buy food, sell wood, sell food),
    /// `target` the partner slot 0 to 8. The sender is `player`
    /// (not the active player), so that rejections and skipping foreign carts can be checked;
    /// the simulation checks the values (a rejection is in the debug log).
    SimTrade {
        player: u8,
        #[serde(default)]
        units: Vec<u32>,
        ware: i32,
        target: i32,
    },
    /// Tribute (command `0x65`, protocol 17): slot `player` pays every slot 0 to 7 with
    /// its bit set in `recipients` `amount` of the resource `resource` (0 gold, 1 wood, 2
    /// food). The sender is `player`; the simulation checks the values.
    SimTribute {
        player: u8,
        recipients: u32,
        resource: i32,
        amount: i32,
    },
    /// Path grid layer 0-8 or occupancy grid (9): key figures and, with `rect` (x0, z0, x1, z1,
    /// inclusive), the cells of a section.
    SimGrid {
        layer: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<[i32; 4]>,
    },
    /// Fog of a player (protocol 10): tile counts with shared vision, cycle and, with
    /// `rect` (x0, z0, x1, z1, inclusive), the states 0/1/3/7 of the tiles.
    SimFog {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        player: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rect: Option<[i32; 4]>,
    },
    /// View of the host (protocol 10): active player, "Fog off" (`fog_shown: false`,
    /// `m_FogShowMode` 7) and fog overlay (0 off, 1 explored, 3 seen, 7 revealed).
    SimView {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_player: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fog_shown: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fog_overlay: Option<u8>,
    },
    /// Headhunter menu (protocol 22, re/headhunter-robbing 2.5): for slot `player` and the
    /// kind `art` (0 SPA, 1 PRI, 2 SID) the target players (`FUN_0044A8C0`) and, with `target_player`,
    /// additionally its target GUIDs (`FUN_0044AA10`, even if it is not among the target players).
    /// Read-only; hiring is done via `sim_command` with `HireHeadhunter`.
    SimHireMenu {
        player: u8,
        art: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_player: Option<u8>,
    },
    /// Read AI state (protocol 26, [`crate::KiStateResult`]): per engine, the threads in
    /// execution order, with `values` also `m_iValues`; counters of the AI commands from the events.
    /// With `slot` only that slot, otherwise all. Read-only.
    KiState {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u8>,
        /// Also return `m_iValues` (default off).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        values: Option<bool>,
    },
    /// Read script state (protocol 28, [`crate::ScriptStateResult`]): containers with blocks,
    /// namespaces and countdown, speech state, object names (with `names` only those whose name or
    /// value contains the text), the block log of the last `log` steps (default
    /// [`DEFAULT_SCRIPT_LOG_STEPS`], at most [`MAX_SCRIPT_LOG_STEPS`]) and, with `block`, the values of the
    /// conditions of this block (evaluated on a copy). Read-only.
    ScriptState {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block: Option<ScriptBlockRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        names: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        log: Option<u32>,
    },
    /// Read presentation events of the scripts (protocol 33, [`crate::ScriptEventsResult`]): the `SimEvent::Present`
    /// of the last `steps` steps (default [`DEFAULT_SCRIPT_LOG_STEPS`], at most [`MAX_SCRIPT_LOG_STEPS`]) with
    /// their step, plus presentation state, UI state (lock, cinemascope, selection mirror, last
    /// button), profile variables and the camera of the scripts. Read-only; only in the viewer (not in `opennml-game`).
    ScriptEvents {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u32>,
    },
    /// Debug intervention in the script flow as a simulation command (protocol 32): `skip_dialog`
    /// ends the running dialog at the next tick (like the skip flag `WaitForKIDebugger`). Takes effect at the
    /// next tick and is in the command log (`CommandKind::ScriptSkipDialog`); an error without a running dialog.
    /// Response `{"step"}` like `sim_command`.
    ScriptCommand {
        action: ScriptAction,
    },
    /// AI intervention as a simulation command (protocol 26): `start` (`slot`, `module`,
    /// `args`, `sync`), `pause` (`paused`, default `true`), `step` (`slot`, `passes`, default 1).
    /// Takes effect at the next tick and is in the command log.
    KiCommand {
        action: KiAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        module: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Vec<i32>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sync: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paused: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passes: Option<u32>,
    },
}

impl Command {
    /// Checks the parameters that can be checked without viewer state (key name, value ranges).
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Key {
                code,
                action,
                repeat,
            } => {
                check_key(code).map_err(|e| e.to_string())?;
                if *repeat > 0 && *action != KeyAction::Down {
                    return Err("repeat only with down".into());
                }
                Ok(())
            }
            Self::MouseMove { x, y } if !x.is_finite() || !y.is_finite() => Err(format!(
                "mouse position ({x}, {y}) invalid: finite numbers expected"
            )),
            Self::Step { frames, frame_ms } => {
                if !(1..=MAX_STEP_FRAMES).contains(frames) {
                    return Err(format!(
                        "frames = {frames} invalid: allowed 1 to {MAX_STEP_FRAMES}"
                    ));
                }
                if !frame_ms.is_finite() || *frame_ms <= 0.0 || *frame_ms > 100.0 {
                    return Err(format!(
                        "frame_ms = {frame_ms} invalid: allowed a finite number in (0, 100]"
                    ));
                }
                Ok(())
            }
            Self::Screenshot {
                max_size: Some(0), ..
            } => Err("max_size = 0 invalid: at least 1 pixel".into()),
            Self::UiClick { .. }
            | Self::UiKey { .. }
            | Self::UiOpen { .. }
            | Self::UiVideo { .. }
            | Self::UiPresent { .. } => crate::ui::validate(self),
            Self::SetLight { name } if name.trim().is_empty() => Err(
                "empty light name (allowed: level or a preset, see lights)".into(),
            ),
            Self::SpawnEffect {
                path,
                position,
                object_index,
                scale,
            } => {
                if path.trim().is_empty() {
                    return Err("empty effect path (expected: a .pef file)".into());
                }
                if position.is_some() && object_index.is_some() {
                    return Err("position and object_index are mutually exclusive".into());
                }
                if position.is_some_and(|p| p.iter().any(|v| !v.is_finite())) {
                    return Err(format!(
                        "position = {position:?} invalid: finite numbers expected"
                    ));
                }
                if let Some(s) = scale
                    && !(s.is_finite() && *s > 0.0)
                {
                    return Err(format!(
                        "scale = {s} invalid: allowed a finite number > 0"
                    ));
                }
                Ok(())
            }
            Self::SetDamage { value } if !(0.0..=3.0).contains(value) => {
                Err(format!("value = {value} invalid: allowed 0 to 3"))
            }
            Self::PlaySound {
                guid,
                action,
                path,
                position,
            } => {
                match (guid.is_some() || action.is_some(), path) {
                    (true, Some(_)) => {
                        return Err("guid/action and path are mutually exclusive".into());
                    }
                    (false, None) => {
                        return Err("play_sound needs guid and action or path".into());
                    }
                    (false, Some(p)) if p.trim().is_empty() => {
                        return Err("empty path (expected: .wav or .ogg)".into());
                    }
                    (true, None) if guid.is_none() || action.is_none() => {
                        return Err("play_sound needs guid and action together".into());
                    }
                    _ => {}
                }
                if position.is_some_and(|p| p.iter().any(|v| !v.is_finite())) {
                    return Err(format!(
                        "position = {position:?} invalid: finite numbers expected"
                    ));
                }
                Ok(())
            }
            Self::Music {
                action: MusicCommand::Play,
                track,
                ..
            } if track.as_deref().is_none_or(|t| t.trim().is_empty()) => {
                Err("music play needs track (title, win, lose, credits or path)".into())
            }
            Self::Music {
                action: MusicCommand::Attack,
                on: None,
                ..
            } => Err("music attack needs on".into()),
            Self::SetVolume {
                music, sfx, speech, ..
            } => {
                for (name, value) in [("music", music), ("sfx", sfx), ("speech", speech)] {
                    if let Some(value) = value
                        && *value > MAX_VOLUME
                    {
                        return Err(format!(
                            "{name} = {value} invalid: allowed 0 to {MAX_VOLUME}"
                        ));
                    }
                }
                Ok(())
            }
            Self::AudioCapture {
                action: CaptureCommand::Start,
                path,
            } if path.as_deref().is_none_or(|p| p.trim().is_empty()) => {
                Err("audio_capture start needs path (WAV file)".into())
            }
            Self::SimSetup {
                nations,
                teams,
                game_speed,
                resource_setting,
                living_space,
                slots,
                campaign_difficulty,
                time_per_char,
                time_limit,
                ..
            } => {
                if let Some(slots) = slots {
                    if nations.is_some() || teams.is_some() {
                        return Err("slots excludes nations and teams".into());
                    }
                    check_slots(slots)?;
                }
                if let Some(t) = time_per_char.filter(|t| !TIME_PER_CHAR_RANGE.contains(t)) {
                    return Err(format!(
                        "time_per_char = {t} invalid: allowed {} to {} (ms per character)",
                        TIME_PER_CHAR_RANGE.start(),
                        TIME_PER_CHAR_RANGE.end()
                    ));
                }
                if let Some(t) = time_limit.filter(|t| !TIME_LIMIT_RANGE.contains(t)) {
                    return Err(format!(
                        "time_limit = {t} invalid: allowed {} to {} (minutes)",
                        TIME_LIMIT_RANGE.start(),
                        TIME_LIMIT_RANGE.end()
                    ));
                }
                if let Some(d) =
                    campaign_difficulty.filter(|d| !CAMPAIGN_DIFFICULTY_RANGE.contains(d))
                {
                    return Err(format!(
                        "campaign_difficulty = {d} invalid: allowed 0 to 2 (as in the campaign menu)"
                    ));
                }
                if let Some(speed) = game_speed.filter(|&s| s > MAX_GAME_SPEED) {
                    return Err(format!(
                        "game_speed = {speed} invalid: allowed 0 to {MAX_GAME_SPEED}"
                    ));
                }
                if let Some(r) =
                    resource_setting.filter(|r| !(0..=MAX_RESOURCE_SETTING).contains(r))
                {
                    return Err(format!(
                        "resource_setting = {r} invalid: allowed 0 (map) to {MAX_RESOURCE_SETTING}"
                    ));
                }
                if let Some(l) = living_space.filter(|l| !(0..=MAX_LIVING_SPACE).contains(l)) {
                    return Err(format!(
                        "living_space = {l} invalid: allowed 0 to {MAX_LIVING_SPACE} (living space 75 to 200)"
                    ));
                }
                Ok(())
            }
            Self::NetSave { name } if name.trim().is_empty() => {
                Err("empty savegame name (expected: e.g. \"interim\")".into())
            }
            Self::SimCommand { command } if !command.is_object() => Err(
                "command must be an object, e.g. {\"player\": 0, \"kind\": {\"Halt\": {\"units\": [65537]}}}"
                    .into(),
            ),
            // Like the check of command `0x56` in the simulation: slot and result,
            // not "slot = sender".
            Self::SimResult { player, .. } if *player > 7 => {
                Err(format!("player = {player} invalid: allowed 0 to 7"))
            }
            Self::SimResult { result, .. } if !matches!(result, 1 | 2) => Err(format!(
                "result = {result} invalid: allowed 1 (lost) or 2 (won)"
            )),
            Self::SimQuery {
                limit: Some(limit), ..
            } if !(1..=MAX_QUERY_LIMIT).contains(limit) => Err(format!(
                "limit = {limit} invalid: allowed 1 to {MAX_QUERY_LIMIT}"
            )),
            Self::SimLoad {
                steps: Some(steps),
                ..
            } if *steps > MAX_REPLAY_STEPS => Err(format!(
                "steps = {steps} invalid: allowed at most {MAX_REPLAY_STEPS} (eight hours of game time)"
            )),
            Self::SimSave { path, .. } | Self::SimLoad { path, .. } if path.trim().is_empty() => {
                Err("empty path".into())
            }
            Self::SimPath {
                unit, from, to, layer, ..
            } => {
                if to.iter().chain(from.iter().flatten()).any(|v| !v.is_finite()) {
                    return Err("to/from invalid: finite world coordinates (x, z) expected".into());
                }
                if let Some(l) = layer.filter(|&l| l >= GRID_OCCUPANCY) {
                    return Err(format!("layer = {l} invalid: allowed 0 to 8"));
                }
                match (unit, from, layer) {
                    (None, None, _) => Err("sim_path needs unit or from".into()),
                    (Some(_), Some(_), _) => Err("unit and from are mutually exclusive".into()),
                    (None, Some(_), None) => Err("sim_path with from needs layer".into()),
                    _ => Ok(()),
                }
            }
            Self::SimMove { to, .. } if to.iter().any(|v| !v.is_finite()) => {
                Err("to invalid: finite world coordinates (x, z) expected".into())
            }
            Self::SimMove {
                drag_to: Some(d), ..
            } if d.iter().any(|v| !v.is_finite()) => {
                Err("drag_to invalid: finite world coordinates (x, z) expected".into())
            }
            Self::SimUnitBar {
                ground: Some(g), ..
            } if g.iter().any(|v| !v.is_finite()) => {
                Err("ground invalid: finite world coordinates (x, z) expected".into())
            }
            Self::SimBuild {
                to, rotation_steps, ..
            } => {
                if to.iter().any(|v| !v.is_finite()) {
                    return Err("to invalid: finite world coordinates (x, z) expected".into());
                }
                if *rotation_steps > MAX_ROTATION_STEPS {
                    return Err(format!(
                        "rotation_steps = {rotation_steps} invalid: allowed 0 to {MAX_ROTATION_STEPS} (quarter turns)"
                    ));
                }
                Ok(())
            }
            Self::SimTrain { guid: 0, .. }
            | Self::SimComplete {
                guid: Some(0), ..
            } => Err("guid = 0 invalid".into()),
            Self::SimCancel { index, .. } | Self::SimComplete { index, .. }
                if !(0..=MAX_CANCEL_INDEX).contains(index) =>
            {
                Err(format!(
                    "index = {index} invalid: allowed 0 (running) to {MAX_CANCEL_INDEX}"
                ))
            }
            Self::SimUpgrade {
                player,
                guid,
                remove,
            } => {
                if *player > MAX_UPGRADE_SLOT {
                    return Err(format!(
                        "player = {player} invalid: allowed 0 to {MAX_UPGRADE_SLOT} (target slot)"
                    ));
                }
                match (*guid, *remove) {
                    (0, _) => Err("guid = 0 invalid".into()),
                    (g, true) if g < 0 => Err(format!(
                        "guid = {g} invalid: remove needs a GUID > 0"
                    )),
                    _ => Ok(()),
                }
            }
            Self::SimAction { target, to, .. } => {
                if to.is_some_and(|t| t.iter().any(|v| !v.is_finite())) {
                    return Err("to invalid: finite world coordinates (x, z) expected".into());
                }
                if target.is_none() && to.is_none() {
                    return Err("sim_action needs target or to".into());
                }
                Ok(())
            }
            Self::KiQuery {
                player,
                words,
                sight,
                ..
            } => {
                if *player > 7 {
                    return Err(format!("player = {player} invalid: allowed 0 to 7"));
                }
                if let Some(w) = words.as_ref().filter(|w| w.len() > MAX_KI_QUERY_WORDS) {
                    return Err(format!(
                        "words has {} words: allowed at most {MAX_KI_QUERY_WORDS}",
                        w.len()
                    ));
                }
                if let Some(s) = sight.as_deref().filter(|s| !matches!(*s, "omniscient" | "fog")) {
                    return Err(format!(
                        "sight = \"{s}\" invalid: allowed \"omniscient\" or \"fog\""
                    ));
                }
                Ok(())
            }
            Self::KiState { .. } | Self::KiCommand { .. } => validate_ki(self),
            Self::ScriptState {
                log: Some(log), ..
            } if *log > MAX_SCRIPT_LOG_STEPS => Err(format!(
                "log = {log} invalid: allowed 0 to {MAX_SCRIPT_LOG_STEPS} steps"
            )),
            Self::ScriptEvents { steps: Some(steps) } if *steps > MAX_SCRIPT_LOG_STEPS => {
                Err(format!(
                    "steps = {steps} invalid: allowed 0 to {MAX_SCRIPT_LOG_STEPS} steps"
                ))
            }
            Self::SimHireMenu {
                player,
                art,
                target_player,
            } => {
                if *player > 7 {
                    return Err(format!("player = {player} invalid: allowed 0 to 7"));
                }
                if *art > 2 {
                    return Err(format!("art = {art} invalid: allowed 0 to 2"));
                }
                if let Some(t) = target_player.filter(|&t| t > 7) {
                    return Err(format!("target_player = {t} invalid: allowed 0 to 7"));
                }
                Ok(())
            }
            Self::SimMenu { units, building } => match (units, building) {
                (None, None) => Err("sim_menu needs units or building".into()),
                (Some(_), Some(_)) => Err("units and building are mutually exclusive".into()),
                _ => Ok(()),
            },
            Self::SimGrid { layer, rect } => {
                if *layer > GRID_OCCUPANCY {
                    return Err(format!(
                        "layer = {layer} invalid: allowed 0 to 8 (path grid) or 9 (occupancy)"
                    ));
                }
                check_rect(rect)
            }
            Self::SimFog { player, rect } => {
                if player.is_some_and(|p| p > 7) {
                    return Err("player invalid: allowed 0 to 7".into());
                }
                check_rect(rect)
            }
            Self::SimView {
                active_player,
                fog_overlay,
                ..
            } => {
                if active_player.is_some_and(|p| p > 7) {
                    return Err("active_player invalid: allowed 0 to 7".into());
                }
                if fog_overlay.is_some_and(|o| !matches!(o, 0 | 1 | 3 | 7)) {
                    return Err("fog_overlay invalid: allowed 0, 1, 3 or 7".into());
                }
                Ok(())
            }
            Self::SetQuality {
                shadow,
                particles,
                terrain,
            } => {
                for ((name, value), max) in [
                    ("shadow", shadow),
                    ("particles", particles),
                    ("terrain", terrain),
                ]
                .into_iter()
                .zip(MAX_QUALITY)
                {
                    if let Some(value) = value
                        && *value > max
                    {
                        return Err(format!("{name} = {value} invalid: allowed 0 to {max}"));
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Section `rect` (x0, z0, x1, z1, inclusive) of `sim_grid` and `sim_fog`: ascending and
/// at most [`MAX_GRID_CELLS`] cells.
fn check_rect(rect: &Option<[i32; 4]>) -> Result<(), String> {
    if let Some([x0, z0, x1, z1]) = rect {
        let cells = (i64::from(*x1) - i64::from(*x0) + 1) * (i64::from(*z1) - i64::from(*z0) + 1);
        if x1 < x0 || z1 < z0 || cells > MAX_GRID_CELLS {
            return Err(format!(
                "rect invalid: x0 <= x1, z0 <= z1 and at most {MAX_GRID_CELLS} cells"
            ));
        }
    }
    Ok(())
}

// --- AI match (protocol 26) ------------------------------------------------------------------

/// Kind of a lobby slot in `sim_setup` `slots`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotSetupKind {
    Closed,
    Human,
    Ai,
}

/// A lobby slot of `sim_setup` (protocol 26). Missing values as the default of the map (nation
/// of the `.lvl`, team and color = slot, handicap 0). The AI kind 1-4 sets the handicap like the
/// original (5/2/0/0); a free handicap 0-5 excludes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotSetup {
    pub kind: SlotSetupKind,
    /// Nation 0-5 (0 WID, 1 PRI, 2 SPA, 3 SID, 4 ENG, 5 PAT).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nation: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    /// Color 0-8 (8 = white).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<i32>,
    /// Free handicap 0-5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handicap: Option<i32>,
    /// AI kind 1-4, only with `ai`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ki_kind: Option<u8>,
}

/// Block of a script container in `script_state` (protocol 28): container ID (level 1, ...; dialog -1)
/// and index of the block in the script file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptBlockRef {
    pub container: i32,
    pub index: u32,
}

/// Action of `script_command` (protocol 32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptAction {
    /// End the running dialog at the next tick.
    SkipDialog,
}

/// Action of `ki_command`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KiAction {
    Start,
    Pause,
    Step,
}

/// Checks the slots of `sim_setup` (also the settings file of the viewer): at most
/// [`MAX_SLOTS`], nation 0-5, color 0-8, handicap 0-[`MAX_SLOT_HANDICAP`], AI kind 1-4 only with `ai`
/// and not together with `handicap`.
pub fn check_slots(slots: &[SlotSetup]) -> Result<(), String> {
    if slots.len() > MAX_SLOTS {
        return Err(format!(
            "slots has {} entries: allowed at most {MAX_SLOTS}",
            slots.len()
        ));
    }
    for (i, s) in slots.iter().enumerate() {
        if let Some(n) = s.nation.filter(|n| !(0..=5).contains(n)) {
            return Err(format!("slots[{i}]: nation = {n} invalid: allowed 0 to 5"));
        }
        if let Some(c) = s.color.filter(|c| !(0..=8).contains(c)) {
            return Err(format!("slots[{i}]: color = {c} invalid: allowed 0 to 8"));
        }
        if let Some(h) = s.handicap.filter(|h| !(0..=MAX_SLOT_HANDICAP).contains(h)) {
            return Err(format!(
                "slots[{i}]: handicap = {h} invalid: allowed 0 to {MAX_SLOT_HANDICAP}"
            ));
        }
        if let Some(k) = s.ki_kind {
            if !KI_KIND_RANGE.contains(&k) {
                return Err(format!("slots[{i}]: ki_kind = {k} invalid: allowed 1 to 4"));
            }
            if s.kind != SlotSetupKind::Ai {
                return Err(format!("slots[{i}]: ki_kind only with kind ai"));
            }
            if s.handicap.is_some() {
                return Err(format!(
                    "slots[{i}]: ki_kind and handicap are mutually exclusive (the AI kind sets the handicap)"
                ));
            }
        }
    }
    Ok(())
}

/// Check of `ki_state` and `ki_command`: slot 0-7; per action the required fields and only its
/// fields; `args` at most [`MAX_KI_ARGS`], `passes` 1-[`MAX_KI_STEP_PASSES`].
fn validate_ki(command: &Command) -> Result<(), String> {
    let slot = match command {
        Command::KiState { slot, .. } | Command::KiCommand { slot, .. } => *slot,
        _ => None,
    };
    if let Some(s) = slot.filter(|&s| s > 7) {
        return Err(format!("slot = {s} invalid: allowed 0 to 7"));
    }
    let Command::KiCommand {
        action,
        slot,
        module,
        args,
        sync,
        paused,
        passes,
    } = command
    else {
        return Ok(());
    };
    let name = match action {
        KiAction::Start => "start",
        KiAction::Pause => "pause",
        KiAction::Step => "step",
    };
    let only = |present: bool, field: &str, allowed: &str| {
        if present {
            Err(format!(
                "ki_command {name}: {field} applies only to {allowed}"
            ))
        } else {
            Ok(())
        }
    };
    let start = *action == KiAction::Start;
    only(!start && module.is_some(), "module", "start")?;
    only(!start && args.is_some(), "args", "start")?;
    only(!start && sync.is_some(), "sync", "start")?;
    only(
        *action != KiAction::Pause && paused.is_some(),
        "paused",
        "pause",
    )?;
    only(
        *action != KiAction::Step && passes.is_some(),
        "passes",
        "step",
    )?;
    only(
        *action == KiAction::Pause && slot.is_some(),
        "slot",
        "start and step",
    )?;
    if *action != KiAction::Pause && slot.is_none() {
        return Err(format!("ki_command {name} needs slot"));
    }
    if start && module.is_none() {
        return Err("ki_command start needs module".into());
    }
    if let Some(a) = args.as_ref().filter(|a| a.len() > MAX_KI_ARGS) {
        return Err(format!(
            "args has {} values: allowed at most {MAX_KI_ARGS}",
            a.len()
        ));
    }
    if let Some(p) = passes.filter(|p| !(1..=MAX_KI_STEP_PASSES).contains(p)) {
        return Err(format!(
            "passes = {p} invalid: allowed 1 to {MAX_KI_STEP_PASSES}"
        ));
    }
    Ok(())
}

/// Result or error message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok(serde_json::Value),
    Error(String),
}

/// A response: ID of the request and result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
}

impl Response {
    /// Success with a serializable result.
    pub fn ok(id: u64, value: impl Serialize) -> Self {
        let outcome = match serde_json::to_value(value) {
            Ok(value) => Outcome::Ok(value),
            Err(e) => Outcome::Error(format!("result not serializable: {e}")),
        };
        Self { id, outcome }
    }

    pub fn error(id: u64, message: impl Into<String>) -> Self {
        Self {
            id,
            outcome: Outcome::Error(message.into()),
        }
    }
}

/// Result of `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloInfo {
    pub protocol: u32,
    pub pid: u32,
    /// `None` as long as the GPU is not ready yet.
    pub backend: Option<String>,
    pub adapter: Option<String>,
}

/// Result of `play_sound`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaySoundResult {
    /// A sound was started (for 3D database sounds also: one instance moved = `false`).
    pub played: bool,
    /// ID of the sound (not for 3D sounds, as in the game).
    pub id: Option<u32>,
}

/// Result of `audio_capture stop`: length and level (full scale = 1) of the WAV file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureSummaryData {
    pub path: String,
    pub frames: u64,
    pub length_ms: f64,
    /// Peak level left/right.
    pub peak: [f32; 2],
    /// RMS over the whole recording left/right.
    pub rms: [f32; 2],
    /// RMS per 100 ms window left/right.
    pub envelope: Vec<[f32; 2]>,
}

/// Result of `screenshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotData {
    /// Size of the rendered image.
    pub width: u32,
    pub height: u32,
    /// FNV-1a (64 bit, hexadecimal) over the RGBA pixels of the rendered image: equal
    /// checksum = pixel-identical images.
    pub rgba_hash: String,
    /// Size of the PNG (smaller than the image if `max_size` required it).
    pub png_width: u32,
    pub png_height: u32,
    pub png_base64: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip(line: serde_json::Value, expected: Command) {
        let request: Request = serde_json::from_value(line.clone()).unwrap();
        assert_eq!(
            request,
            Request {
                id: 7,
                command: expected
            }
        );
        assert_eq!(serde_json::to_value(&request).unwrap(), line);
    }

    /// `random_map` queries the random map of the preview (without parameters).
    #[test]
    fn random_map_command_round_trips() {
        round_trip(json!({"id": 7, "cmd": "random_map"}), Command::RandomMap);
        assert_eq!(Command::RandomMap.validate(), Ok(()));
    }

    /// The four network commands of `opennml-game` as JSON lines.
    #[test]
    fn net_commands_round_trip() {
        round_trip(json!({"id": 7, "cmd": "net_status"}), Command::NetStatus);
        round_trip(
            json!({"id": 7, "cmd": "net_part_hashes"}),
            Command::NetPartHashes,
        );
        round_trip(
            json!({"id": 7, "cmd": "net_save", "name": "interim"}),
            Command::NetSave {
                name: "interim".into(),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "net_debug", "action": "corrupt"}),
            Command::NetDebug {
                action: NetDebugAction::Corrupt,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "net_debug", "action": "drop"}),
            Command::NetDebug {
                action: NetDebugAction::Drop,
            },
        );
        for command in [
            Command::NetStatus,
            Command::NetPartHashes,
            Command::NetSave { name: "a".into() },
            Command::NetDebug {
                action: NetDebugAction::Drop,
            },
        ] {
            assert_eq!(command.validate(), Ok(()), "{command:?}");
        }
        // Empty name: rejected before the request reaches the turn core.
        let err = Command::NetSave { name: "  ".into() }
            .validate()
            .unwrap_err();
        assert!(err.contains("savegame name"), "{err}");
        // Unknown check aid: error when reading the line.
        assert!(
            serde_json::from_value::<Request>(
                json!({"id": 7, "cmd": "net_debug", "action": "boom"})
            )
            .is_err()
        );
        // A response of `net_status` during the search, as a check script reads it (without role,
        // with a found game, without grid and lobby).
        let searching: crate::NetStatusData = serde_json::from_value(json!({
            "role": null, "phase": "searching", "halt": null, "paused_by": null, "port": null,
            "players": null, "peers": [], "own_slot": null, "seats": [], "round": 0,
            "next_exec_step": 0, "steps": null, "waiting": null, "waiting_turn": null,
            "missing": [], "desyncs": null, "links": [], "started": false, "finished": false,
            "failure": null, "spacing": null, "lead": null, "build": "unbekannt", "rtt_ms": [],
            "lobby": null,
            "found": [{"instance": "00000000000000ff", "name": "Session - Anna",
                       "addr": "127.0.0.1:2302"}]
        }))
        .unwrap();
        assert_eq!(searching.role, None);
        assert_eq!(searching.phase, crate::NetPhaseName::Searching);
        assert_eq!(searching.found[0].name, "Session - Anna");
        // A role other than `host`/`client` is an error.
        assert!(serde_json::from_value::<crate::NetRoleName>(json!("observer")).is_err());
    }

    /// `net_debug` with `mute` and `unmute`.
    #[test]
    fn net_debug_mute_round_trips() {
        for (name, action) in [
            ("mute", NetDebugAction::Mute),
            ("unmute", NetDebugAction::Unmute),
        ] {
            round_trip(
                json!({"id": 7, "cmd": "net_debug", "action": name}),
                Command::NetDebug { action },
            );
            assert_eq!(Command::NetDebug { action }.validate(), Ok(()));
        }
    }

    #[test]
    fn every_command_round_trips() {
        round_trip(json!({"id": 7, "cmd": "hello"}), Command::Hello);
        round_trip(
            json!({"id": 7, "cmd": "key", "code": "ArrowUp", "action": "tap"}),
            Command::Key {
                code: "ArrowUp".into(),
                action: KeyAction::Tap,
                repeat: 0,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ui_focus_lost"}),
            Command::UiFocusLost,
        );
        round_trip(
            json!({"id": 7, "cmd": "mouse_move", "x": 1.5, "y": 2.0}),
            Command::MouseMove { x: 1.5, y: 2.0 },
        );
        round_trip(json!({"id": 7, "cmd": "mouse_leave"}), Command::MouseLeave);
        round_trip(
            json!({"id": 7, "cmd": "mouse_button", "button": "right", "down": true}),
            Command::MouseButton {
                button: MouseButtonName::Right,
                down: true,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "mouse_button", "button": "middle", "down": false}),
            Command::MouseButton {
                button: MouseButtonName::Middle,
                down: false,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "wheel", "up": false}),
            Command::Wheel { up: false },
        );
        round_trip(json!({"id": 7, "cmd": "pause"}), Command::Pause);
        round_trip(json!({"id": 7, "cmd": "resume"}), Command::Resume);
        round_trip(
            json!({"id": 7, "cmd": "step", "frames": 30, "frame_ms": 16.0}),
            Command::Step {
                frames: 30,
                frame_ms: 16.0,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "screenshot", "overlay": true, "max_size": 1280, "pointer": false}),
            Command::Screenshot {
                overlay: true,
                max_size: Some(1280),
                pointer: false,
            },
        );
        round_trip(json!({"id": 7, "cmd": "state"}), Command::State);
        round_trip(json!({"id": 7, "cmd": "ui_state"}), Command::UiState);
        round_trip(
            json!({"id": 7, "cmd": "ui_click", "dialog": "start", "element": "options", "button": "left"}),
            Command::UiClick {
                dialog: "start".into(),
                element: "options".into(),
                button: Some(MouseButtonName::Left),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ui_key", "chord": "ESCAPE", "char": 27}),
            Command::UiKey {
                chord: "ESCAPE".into(),
                char: Some(27),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ui_open", "dialog": "options"}),
            Command::UiOpen {
                dialog: "options".into(),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ui_video", "name": "eng"}),
            Command::UiVideo { name: "eng".into() },
        );
        round_trip(
            json!({"id": 7, "cmd": "window", "mode": "exclusive", "monitor": 2}),
            Command::Window {
                mode: Some(WindowModeName::Exclusive),
                monitor: Some(2),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "hud", "on": true}),
            Command::Hud { on: true },
        );
        assert_eq!(Command::Hud { on: false }.validate(), Ok(()));
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"cmd":"hud"}"#).is_err());
        round_trip(json!({"id": 7, "cmd": "quit"}), Command::Quit);
        round_trip(json!({"id": 7, "cmd": "lights"}), Command::Lights);
        round_trip(
            json!({"id": 7, "cmd": "set_light", "name": "Sommer Nacht"}),
            Command::SetLight {
                name: "Sommer Nacht".into(),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "set_weather", "kind": "none"}),
            Command::SetWeather {
                kind: WeatherName::None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "set_quality", "shadow": 3, "terrain": 2}),
            Command::SetQuality {
                shadow: Some(3),
                particles: None,
                terrain: Some(2),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "spawn_effect", "path": "data/effects/feuer.pef", "position": [1.0, 2.0, 3.0], "scale": 2.0}),
            Command::SpawnEffect {
                path: "data/effects/feuer.pef".into(),
                position: Some([1.0, 2.0, 3.0]),
                object_index: None,
                scale: Some(2.0),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "clear_effects"}),
            Command::ClearEffects,
        );
        round_trip(
            json!({"id": 7, "cmd": "set_damage", "value": 1.25}),
            Command::SetDamage { value: 1.25 },
        );
        round_trip(
            json!({"id": 7, "cmd": "play_sound", "guid": 4104, "action": 200, "position": [1.0, 0.0, 2.0]}),
            Command::PlaySound {
                guid: Some(4104),
                action: Some(200),
                path: None,
                position: Some([1.0, 0.0, 2.0]),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "music", "action": "play", "track": "title"}),
            Command::Music {
                action: MusicCommand::Play,
                track: Some("title".into()),
                on: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "set_volume", "music": 1000, "mute": true}),
            Command::SetVolume {
                music: Some(1000),
                sfx: None,
                speech: None,
                mute: Some(true),
                unit_response: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "set_volume", "unit_response": false}),
            Command::SetVolume {
                music: None,
                sfx: None,
                speech: None,
                mute: None,
                unit_response: Some(false),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "audio_capture", "action": "start", "path": "a.wav"}),
            Command::AudioCapture {
                action: CaptureCommand::Start,
                path: Some("a.wav".into()),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_query", "player": 1, "code": 20_004, "words": [1, 5, 5], "sight": "fog"}),
            Command::KiQuery {
                player: 1,
                code: 20_004,
                words: Some(vec![1, 5, 5]),
                sight: Some("fog".into()),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_hire_menu", "player": 1, "art": 0, "target_player": 0}),
            Command::SimHireMenu {
                player: 1,
                art: 0,
                target_player: Some(0),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_state", "slot": 1}),
            Command::KiState {
                slot: Some(1),
                values: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_command", "action": "step", "slot": 1, "passes": 2}),
            Command::KiCommand {
                action: KiAction::Step,
                slot: Some(1),
                module: None,
                args: None,
                sync: None,
                paused: None,
                passes: Some(2),
            },
        );
    }

    /// `pointer` with and without the field; old scripts (without the field) read unchanged.
    // QUIRK O-297: kept
    #[test]
    fn screenshot_pointer_flag_round_trip() {
        let old: Request =
            serde_json::from_str(r#"{"id":1,"cmd":"screenshot","overlay":true}"#).unwrap();
        assert_eq!(
            old.command,
            Command::Screenshot {
                overlay: true,
                max_size: None,
                pointer: false
            }
        );
        let with: Request =
            serde_json::from_str(r#"{"id":1,"cmd":"screenshot","pointer":true}"#).unwrap();
        assert_eq!(
            with.command,
            Command::Screenshot {
                overlay: false,
                max_size: None,
                pointer: true
            }
        );
        assert_eq!(with.command.validate(), Ok(()));
        let json = serde_json::to_value(&with).unwrap();
        assert_eq!(json["pointer"], true);
        let back: Request = serde_json::from_value(json).unwrap();
        assert_eq!(back.command, with.command);
    }

    #[test]
    fn optional_parameters_have_defaults() {
        let r: Request = serde_json::from_str(r#"{"id":1,"cmd":"screenshot"}"#).unwrap();
        assert_eq!(
            r.command,
            Command::Screenshot {
                overlay: false,
                max_size: None,
                pointer: false
            }
        );
        let r: Request = serde_json::from_str(r#"{"id":1,"cmd":"window","monitor":0}"#).unwrap();
        assert_eq!(
            r.command,
            Command::Window {
                mode: None,
                monitor: Some(0)
            }
        );
    }

    #[test]
    fn unknown_commands_and_missing_parameters_are_errors() {
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"cmd":"fly"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"cmd":"key","code":"F1"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"hello"}"#).is_err());
    }

    /// `key` with `repeat` (autorepeat, O-300) goes there and back; without `repeat` 0 and the line as before;
    /// `repeat` only with `down`.
    #[test]
    fn key_repeat_round_trips_and_validates() {
        let key = |action, repeat| Command::Key {
            code: "BracketRight".into(),
            action,
            repeat,
        };
        round_trip(
            json!({"id": 7, "cmd": "key", "code": "BracketRight", "action": "down", "repeat": 5}),
            key(KeyAction::Down, 5),
        );
        round_trip(
            json!({"id": 7, "cmd": "key", "code": "BracketRight", "action": "down"}),
            key(KeyAction::Down, 0),
        );
        assert_eq!(key(KeyAction::Down, 5).validate(), Ok(()));
        assert_eq!(key(KeyAction::Down, u16::MAX).validate(), Ok(()));
        assert_eq!(key(KeyAction::Up, 0).validate(), Ok(()));
        for action in [KeyAction::Up, KeyAction::Tap] {
            assert_eq!(
                key(action, 1).validate(),
                Err("repeat only with down".to_string()),
                "{action:?}"
            );
        }
        // An unknown name takes precedence.
        let unknown = Command::Key {
            code: "Hyper".into(),
            action: KeyAction::Tap,
            repeat: 3,
        };
        assert!(unknown.validate().unwrap_err().contains("unknown key"));
        // Negative or too large: not a `u16`.
        for repeat in [json!(-1), json!(65_536)] {
            let line =
                json!({"id": 1, "cmd": "key", "code": "KeyA", "action": "down", "repeat": repeat});
            assert!(serde_json::from_value::<Request>(line).is_err());
        }
        assert_eq!(Command::UiFocusLost.validate(), Ok(()));
    }

    #[test]
    fn responses_round_trip() {
        let ok = Response::ok(
            3,
            HelloInfo {
                protocol: 1,
                pid: 42,
                backend: None,
                adapter: None,
            },
        );
        let line = serde_json::to_string(&ok).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap(),
            json!({"id": 3, "ok": {"protocol": 1, "pid": 42, "backend": null, "adapter": null}})
        );
        assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), ok);
        let error = Response::error(4, "unknown key");
        let line = serde_json::to_string(&error).unwrap();
        assert_eq!(line, r#"{"id":4,"error":"unknown key"}"#);
        assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), error);
        let null = Response::ok(5, ());
        assert_eq!(
            serde_json::to_string(&null).unwrap(),
            r#"{"id":5,"ok":null}"#
        );
        assert_eq!(
            serde_json::from_str::<Response>(r#"{"id":5,"ok":null}"#).unwrap(),
            null
        );
    }

    #[test]
    fn validation_checks_keys_and_ranges() {
        let key = |code: &str| Command::Key {
            code: code.into(),
            action: KeyAction::Down,
            repeat: 0,
        };
        assert_eq!(key("F11").validate(), Ok(()));
        assert!(key("F13").validate().unwrap_err().contains("unknown key"));
        let step = |frames, frame_ms| Command::Step { frames, frame_ms };
        assert_eq!(step(1, 0.001).validate(), Ok(()));
        assert_eq!(step(MAX_STEP_FRAMES, 100.0).validate(), Ok(()));
        for (frames, frame_ms) in [(0, 16.0), (MAX_STEP_FRAMES + 1, 16.0)] {
            assert!(
                step(frames, frame_ms)
                    .validate()
                    .unwrap_err()
                    .contains("frames")
            );
        }
        for frame_ms in [0.0, -1.0, 100.5, f64::NAN, f64::INFINITY] {
            assert!(
                step(1, frame_ms)
                    .validate()
                    .unwrap_err()
                    .contains("frame_ms"),
                "{frame_ms}"
            );
        }
        assert!(
            Command::MouseMove {
                x: f64::NAN,
                y: 0.0
            }
            .validate()
            .is_err()
        );
        assert_eq!(Command::MouseMove { x: -5.0, y: 1e6 }.validate(), Ok(()));
        let shot = |max_size| Command::Screenshot {
            overlay: false,
            max_size,
            pointer: false,
        };
        assert!(shot(Some(0)).validate().unwrap_err().contains("max_size"));
        assert_eq!(shot(Some(1)).validate(), Ok(()));
        assert_eq!(shot(None).validate(), Ok(()));
        let quality = |shadow, particles, terrain| Command::SetQuality {
            shadow,
            particles,
            terrain,
        };
        assert_eq!(quality(Some(3), Some(2), Some(2)).validate(), Ok(()));
        assert_eq!(quality(None, None, None).validate(), Ok(()));
        assert!(
            quality(Some(4), None, None)
                .validate()
                .unwrap_err()
                .contains("shadow = 4")
        );
        assert!(quality(None, Some(3), None).validate().is_err());
        assert!(quality(None, None, Some(3)).validate().is_err());
        let light = |name: &str| Command::SetLight { name: name.into() };
        assert!(light(" ").validate().is_err());
        assert_eq!(light("level").validate(), Ok(()));
        let spawn = |path: &str, position, object_index, scale| Command::SpawnEffect {
            path: path.into(),
            position,
            object_index,
            scale,
        };
        assert_eq!(spawn("a.pef", None, None, None).validate(), Ok(()));
        assert_eq!(spawn("a.pef", None, Some(3), Some(0.5)).validate(), Ok(()));
        assert!(spawn(" ", None, None, None).validate().is_err());
        assert!(
            spawn("a.pef", Some([0.0; 3]), Some(1), None)
                .validate()
                .unwrap_err()
                .contains("mutually exclusive")
        );
        assert!(
            spawn("a.pef", Some([f32::NAN, 0.0, 0.0]), None, None)
                .validate()
                .is_err()
        );
        for scale in [0.0, -1.0, f32::INFINITY] {
            assert!(spawn("a.pef", None, None, Some(scale)).validate().is_err());
        }
        let damage = |value| Command::SetDamage { value };
        assert_eq!(damage(0.0).validate(), Ok(()));
        assert_eq!(damage(3.0).validate(), Ok(()));
        for value in [-0.25, 3.25, f32::NAN] {
            assert!(damage(value).validate().unwrap_err().contains("value"));
        }
    }

    // QUIRK E-332: deviation
    #[test]
    fn replay_steps_limit_is_eight_hours() {
        assert_eq!(MAX_REPLAY_STEPS, 576_000);
        let load = |steps| Command::SimLoad {
            path: "a.nmlrepl".into(),
            steps: Some(steps),
        };
        assert_eq!(load(576_000).validate(), Ok(()));
        let error = load(576_001).validate().unwrap_err();
        assert!(error.contains("eight hours"), "{error}");
    }

    #[test]
    fn simulation_commands_round_trip_and_check_their_parameters() {
        round_trip(
            json!({"id": 7, "cmd": "sim_setup", "mode": 0, "teams": [0, 0, 1]}),
            Command::SimSetup {
                mode: Some(0),
                seed: None,
                nations: None,
                teams: Some(vec![0, 0, 1]),
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
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_command", "command": {"player": 0, "kind": {"Halt": {"units": [65537]}}}}),
            Command::SimCommand {
                command: json!({"player": 0, "kind": {"Halt": {"units": [65537]}}}),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_query", "player": 2, "limit": 5}),
            Command::SimQuery {
                player: Some(2),
                guid: None,
                ids: None,
                limit: Some(5),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_save", "path": "a.nmlsave", "replay": true}),
            Command::SimSave {
                path: "a.nmlsave".into(),
                replay: true,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_load", "path": "a.nmlsave"}),
            Command::SimLoad {
                path: "a.nmlsave".into(),
                steps: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_load", "path": "a.nmlrepl", "steps": 400}),
            Command::SimLoad {
                path: "a.nmlrepl".into(),
                steps: Some(400),
            },
        );
        round_trip(json!({"id": 7, "cmd": "sim_hash"}), Command::SimHash);
        let r: Request = serde_json::from_str(r#"{"id":1,"cmd":"sim_save","path":"x"}"#).unwrap();
        assert_eq!(
            r.command,
            Command::SimSave {
                path: "x".into(),
                replay: false
            }
        );
        let setup = |game_speed| Command::SimSetup {
            mode: None,
            seed: None,
            nations: None,
            teams: None,
            game_speed,
            resource_setting: None,
            living_space: None,
            map_explored: None,
            slots: None,
            ki_enabled: None,
            campaign_difficulty: None,
            script_enabled: None,
            time_per_char: None,
            time_limit: None,
        };
        assert_eq!(setup(Some(30)).validate(), Ok(()));
        assert!(setup(Some(31)).validate().unwrap_err().contains("0 to 30"));
        assert!(
            Command::SimCommand { command: json!(3) }
                .validate()
                .unwrap_err()
                .contains("object")
        );
        let query = |limit| Command::SimQuery {
            player: None,
            guid: None,
            ids: None,
            limit,
        };
        assert_eq!(query(None).validate(), Ok(()));
        assert!(query(Some(0)).validate().is_err());
        assert!(query(Some(MAX_QUERY_LIMIT + 1)).validate().is_err());
        let load = |steps| Command::SimLoad {
            path: "a.nmlrepl".into(),
            steps,
        };
        assert_eq!(load(Some(MAX_REPLAY_STEPS)).validate(), Ok(()));
        let error = load(Some(MAX_REPLAY_STEPS + 1)).validate().unwrap_err();
        assert!(error.contains("576000"), "{error}");
        assert_eq!(load(None).validate(), Ok(()));
        assert!(
            Command::SimLoad {
                path: " ".into(),
                steps: None
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn path_commands_round_trip_and_check_their_parameters() {
        round_trip(
            json!({"id": 7, "cmd": "sim_path", "unit": 65537, "to": [1000.0, 2000.0], "show": true, "allow_override": false}),
            Command::SimPath {
                unit: Some(65537),
                from: None,
                to: [1000.0, 2000.0],
                layer: None,
                allow_override: false,
                show: true,
            },
        );
        let r: Request = serde_json::from_value(
            json!({"id": 1, "cmd": "sim_path", "from": [1.0, 2.0], "to": [3.0, 4.0], "layer": 3}),
        )
        .unwrap();
        let path = |unit, from, layer| Command::SimPath {
            unit,
            from,
            to: [3.0, 4.0],
            layer,
            allow_override: false,
            show: true,
        };
        assert_eq!(r.command, path(None, Some([1.0, 2.0]), Some(3)));
        assert_eq!(r.command.validate(), Ok(()));
        assert!(
            path(None, None, None)
                .validate()
                .unwrap_err()
                .contains("unit or from")
        );
        assert!(
            path(None, Some([1.0, 2.0]), None)
                .validate()
                .unwrap_err()
                .contains("layer")
        );
        assert!(path(Some(1), Some([1.0, 2.0]), None).validate().is_err());
        assert!(
            path(Some(1), None, Some(9))
                .validate()
                .unwrap_err()
                .contains("0 to 8")
        );
        assert!(
            path(Some(1), Some([f32::NAN, 0.0]), None)
                .validate()
                .is_err()
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_grid", "layer": 9, "rect": [0, 0, 63, 63]}),
            Command::SimGrid {
                layer: 9,
                rect: Some([0, 0, 63, 63]),
            },
        );
        let grid = |layer, rect| Command::SimGrid { layer, rect };
        assert_eq!(grid(0, None).validate(), Ok(()));
        assert!(grid(10, None).validate().is_err());
        assert!(
            grid(0, Some([0, 0, 64, 63]))
                .validate()
                .unwrap_err()
                .contains("4096")
        );
        assert!(grid(0, Some([5, 0, 4, 0])).validate().is_err());
        assert!(
            grid(0, Some([i32::MIN, 0, i32::MAX, 0]))
                .validate()
                .is_err()
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_fog", "player": 1, "rect": [0, 0, 9, 9]}),
            Command::SimFog {
                player: Some(1),
                rect: Some([0, 0, 9, 9]),
            },
        );
        let fog = |player, rect| Command::SimFog { player, rect };
        assert_eq!(fog(None, None).validate(), Ok(()));
        assert!(
            fog(Some(8), None)
                .validate()
                .unwrap_err()
                .contains("0 to 7")
        );
        assert!(
            fog(None, Some([0, 0, 64, 63]))
                .validate()
                .unwrap_err()
                .contains("4096")
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_view", "active_player": 2, "fog_shown": false, "fog_overlay": 3}),
            Command::SimView {
                active_player: Some(2),
                fog_shown: Some(false),
                fog_overlay: Some(3),
            },
        );
        let view = |p, o| Command::SimView {
            active_player: p,
            fog_shown: None,
            fog_overlay: o,
        };
        assert!(view(Some(8), None).validate().is_err());
        assert!(
            view(None, Some(2))
                .validate()
                .unwrap_err()
                .contains("0, 1, 3 or 7")
        );
        assert_eq!(view(None, Some(0)).validate(), Ok(()), "0 = overlay off");
        assert_eq!(PROTOCOL_VERSION, 50);
    }

    #[test]
    fn hire_menu_round_trips_and_checks_its_parameters() {
        // Protocol 22: without a target player the field is missing, with it the field is in the line.
        round_trip(
            json!({"id": 7, "cmd": "sim_hire_menu", "player": 0, "art": 2}),
            Command::SimHireMenu {
                player: 0,
                art: 2,
                target_player: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_hire_menu", "player": 0, "art": 2, "target_player": 1}),
            Command::SimHireMenu {
                player: 0,
                art: 2,
                target_player: Some(1),
            },
        );
        let menu = |player, art, target_player| Command::SimHireMenu {
            player,
            art,
            target_player,
        };
        assert_eq!(menu(7, 2, Some(7)).validate(), Ok(()));
        assert_eq!(
            menu(9, 0, None).validate().unwrap_err(),
            "player = 9 invalid: allowed 0 to 7"
        );
        assert_eq!(
            menu(0, 3, None).validate().unwrap_err(),
            "art = 3 invalid: allowed 0 to 2"
        );
        assert_eq!(
            menu(0, 0, Some(8)).validate().unwrap_err(),
            "target_player = 8 invalid: allowed 0 to 7"
        );
        // `player` and `art` are required.
        assert!(
            serde_json::from_str::<Request>(r#"{"id":1,"cmd":"sim_hire_menu","player":0}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<Request>(r#"{"id":1,"cmd":"sim_hire_menu","art":0}"#).is_err()
        );
    }

    #[test]
    fn ki_query_round_trips_and_checks_its_parameters() {
        // Protocol 23: `words` and `sight` are optional; without them the fields are missing from the line.
        round_trip(
            json!({"id": 7, "cmd": "ki_query", "player": 1, "code": 20_004}),
            Command::KiQuery {
                player: 1,
                code: 20_004,
                words: None,
                sight: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_query", "player": 7, "code": 20_782, "words": [], "sight": "omniscient"}),
            Command::KiQuery {
                player: 7,
                code: 20_782,
                words: Some(Vec::new()),
                sight: Some("omniscient".into()),
            },
        );
        let query = |player, words: Option<Vec<i32>>, sight: Option<&str>| Command::KiQuery {
            player,
            code: 0x4E24,
            words,
            sight: sight.map(String::from),
        };
        assert_eq!(query(7, None, None).validate(), Ok(()));
        assert_eq!(
            query(0, Some(vec![-1; MAX_KI_QUERY_WORDS]), Some("fog")).validate(),
            Ok(()),
            "128 words are allowed"
        );
        assert_eq!(
            query(8, None, None).validate().unwrap_err(),
            "player = 8 invalid: allowed 0 to 7"
        );
        assert_eq!(
            query(0, Some(vec![0; MAX_KI_QUERY_WORDS + 1]), None)
                .validate()
                .unwrap_err(),
            "words has 129 words: allowed at most 128"
        );
        assert_eq!(
            query(0, None, Some("blind")).validate().unwrap_err(),
            "sight = \"blind\" invalid: allowed \"omniscient\" or \"fog\""
        );
        // `player` and `code` are required.
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"cmd":"ki_query","code":1}"#).is_err());
        assert!(
            serde_json::from_str::<Request>(r#"{"id":1,"cmd":"ki_query","player":1}"#).is_err()
        );
    }

    #[test]
    fn move_commands_round_trip_and_check_their_parameters() {
        round_trip(
            json!({"id": 7, "cmd": "sim_move", "units": [65537], "to": [1000.0, 2000.0], "append": true, "cyclic": false}),
            Command::SimMove {
                units: vec![65537],
                to: [1000.0, 2000.0],
                append: true,
                cyclic: false,
                drag_to: None,
            },
        );
        let r: Request =
            serde_json::from_value(json!({"id": 1, "cmd": "sim_move", "to": [3.0, 4.0]})).unwrap();
        let mv = |to| Command::SimMove {
            units: Vec::new(),
            to,
            append: false,
            cyclic: false,
            drag_to: None,
        };
        assert_eq!(r.command, mv([3.0, 4.0]));
        assert_eq!(r.command.validate(), Ok(()));
        assert!(mv([f32::NAN, 0.0]).validate().unwrap_err().contains("to"));
        assert!(mv([0.0, f32::INFINITY]).validate().is_err());
    }

    #[test]
    fn unit_bar_and_drag_round_trip() {
        round_trip(
            json!({"id": 7, "cmd": "sim_unit_bar", "button": 512}),
            Command::SimUnitBar {
                button: 512,
                target: None,
                ground: None,
            },
        );
        // A ground point must be finite.
        assert!(
            Command::SimUnitBar {
                button: 484,
                target: None,
                ground: Some([f32::NAN, 1.0]),
            }
            .validate()
            .unwrap_err()
            .contains("ground")
        );
        // Protocol 21: target buttons with object or ground point; without either the fields are omitted.
        round_trip(
            json!({"id": 7, "cmd": "sim_unit_bar", "button": 480, "target": 65537}),
            Command::SimUnitBar {
                button: 480,
                target: Some(65_537),
                ground: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_unit_bar", "button": 484, "ground": [2624.0, 1600.0]}),
            Command::SimUnitBar {
                button: 484,
                target: None,
                ground: Some([2624.0, 1600.0]),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_move", "units": [1], "to": [3.0, 4.0], "append": false, "cyclic": false, "drag_to": [5.0, 4.0]}),
            Command::SimMove {
                units: vec![1],
                to: [3.0, 4.0],
                append: false,
                cyclic: false,
                drag_to: Some([5.0, 4.0]),
            },
        );
        // Without drag_to as before.
        let r: Request =
            serde_json::from_value(json!({"id": 1, "cmd": "sim_move", "to": [3.0, 4.0]})).unwrap();
        assert!(matches!(r.command, Command::SimMove { drag_to: None, .. }));
        // NaN in the drag point is rejected.
        assert!(
            Command::SimMove {
                units: vec![],
                to: [1.0, 1.0],
                append: false,
                cyclic: false,
                drag_to: Some([f32::NAN, 0.0]),
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn sim_upgrade_round_trips_and_checks_its_guid() {
        round_trip(
            json!({"id": 7, "cmd": "sim_upgrade", "player": 8, "guid": 2806, "remove": false}),
            Command::SimUpgrade {
                player: 8,
                guid: 2806,
                remove: false,
            },
        );
        // `remove` missing = apply.
        let r: Request =
            serde_json::from_value(json!({"id": 1, "cmd": "sim_upgrade", "player": 0, "guid": -1}))
                .unwrap();
        assert_eq!(
            r.command,
            Command::SimUpgrade {
                player: 0,
                guid: -1,
                remove: false
            }
        );
        let up = |player, guid, remove| Command::SimUpgrade {
            player,
            guid,
            remove,
        };
        assert_eq!(up(0, 2806, false).validate(), Ok(()));
        assert_eq!(
            up(10, 2806, true).validate(),
            Ok(()),
            "slot 10 is reachable"
        );
        assert_eq!(up(0, -1, false).validate(), Ok(()), "apply all upgrades");
        assert!(up(0, 0, false).validate().unwrap_err().contains("guid = 0"));
        assert!(up(0, 0, true).validate().unwrap_err().contains("guid = 0"));
        assert!(
            up(0, -1, true)
                .validate()
                .unwrap_err()
                .contains("guid = -1")
        );
        assert!(
            up(11, 2806, false)
                .validate()
                .unwrap_err()
                .contains("player = 11")
        );
    }

    #[test]
    fn sim_trade_and_sim_tribute_round_trip() {
        round_trip(
            json!({"id": 7, "cmd": "sim_trade", "player": 1, "units": [65537, 65538], "ware": 2, "target": 3}),
            Command::SimTrade {
                player: 1,
                units: vec![65537, 65538],
                ware: 2,
                target: 3,
            },
        );
        // `units` missing = selection (empty list); `validate` requires nothing.
        let r: Request = serde_json::from_value(
            json!({"id": 1, "cmd": "sim_trade", "player": 0, "ware": 0, "target": 8}),
        )
        .unwrap();
        assert_eq!(
            r.command,
            Command::SimTrade {
                player: 0,
                units: Vec::new(),
                ware: 0,
                target: 8
            }
        );
        assert_eq!(r.command.validate(), Ok(()));
        round_trip(
            json!({"id": 7, "cmd": "sim_tribute", "player": 0, "recipients": 6, "resource": 1, "amount": 100}),
            Command::SimTribute {
                player: 0,
                recipients: 6,
                resource: 1,
                amount: 100,
            },
        );
        // The simulation checks the values (resource, amount, recipients), not the channel.
        let wild = Command::SimTribute {
            player: 9,
            recipients: u32::MAX,
            resource: -5,
            amount: -1,
        };
        assert_eq!(wild.validate(), Ok(()));
        let wild = Command::SimTrade {
            player: 9,
            units: Vec::new(),
            ware: 99,
            target: -2,
        };
        assert_eq!(wild.validate(), Ok(()));
    }

    #[test]
    fn economy_commands_round_trip_with_defaults() {
        round_trip(
            json!({"id": 7, "cmd": "sim_build", "guid": 1001, "to": [996.0, 996.0], "rotation_steps": 3, "units": [65537, 65538], "append": true}),
            Command::SimBuild {
                guid: 1001,
                to: [996.0, 996.0],
                rotation_steps: 3,
                units: vec![65537, 65538],
                append: true,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_train", "building": 65539, "guid": 1100}),
            Command::SimTrain {
                building: 65539,
                guid: 1100,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_cancel", "building": 65539, "index": 2}),
            Command::SimCancel {
                building: 65539,
                index: 2,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_complete", "building": 65539, "index": 1}),
            Command::SimComplete {
                building: 65539,
                index: 1,
                guid: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_complete", "building": 65539, "index": 1, "guid": 1100}),
            Command::SimComplete {
                building: 65539,
                index: 1,
                guid: Some(1100),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_sell", "buildings": [65539]}),
            Command::SimSell {
                buildings: vec![65539],
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_action", "units": [65537], "target": 65540, "to": [1.0, 2.0], "append": false, "cyclic": true}),
            Command::SimAction {
                units: vec![65537],
                target: Some(65540),
                to: Some([1.0, 2.0]),
                append: false,
                cyclic: true,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_menu", "building": 65539}),
            Command::SimMenu {
                units: None,
                building: Some(65539),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_menu", "units": []}),
            Command::SimMenu {
                units: Some(Vec::new()),
                building: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "sim_setup", "resource_setting": 3, "living_space": 0, "map_explored": true}),
            Command::SimSetup {
                mode: None,
                seed: None,
                nations: None,
                teams: None,
                game_speed: None,
                resource_setting: Some(3),
                living_space: Some(0),
                map_explored: Some(true),
                slots: None,
                ki_enabled: None,
                campaign_difficulty: None,
                script_enabled: None,
                time_per_char: None,
                time_limit: None,
            },
        );
        // Defaults: selection, without rotation, direct; index 0 = running order; empty list.
        let parse = |line| serde_json::from_value::<Request>(line).unwrap().command;
        assert_eq!(
            parse(json!({"id": 1, "cmd": "sim_build", "guid": 1001, "to": [3.0, 4.0]})),
            Command::SimBuild {
                guid: 1001,
                to: [3.0, 4.0],
                rotation_steps: 0,
                units: Vec::new(),
                append: false,
            }
        );
        assert_eq!(
            parse(json!({"id": 1, "cmd": "sim_cancel", "building": 9})),
            Command::SimCancel {
                building: 9,
                index: 0
            }
        );
        assert_eq!(
            parse(json!({"id": 1, "cmd": "sim_complete", "building": 9})),
            Command::SimComplete {
                building: 9,
                index: 0,
                guid: None,
            }
        );
        assert_eq!(
            parse(json!({"id": 1, "cmd": "sim_sell"})),
            Command::SimSell {
                buildings: Vec::new()
            }
        );
        assert_eq!(
            parse(json!({"id": 1, "cmd": "sim_action", "to": [3.0, 4.0]})),
            Command::SimAction {
                units: Vec::new(),
                target: None,
                to: Some([3.0, 4.0]),
                append: false,
                cyclic: false,
            }
        );
        // Required fields missing: error when reading.
        for line in [
            json!({"id": 1, "cmd": "sim_build", "to": [3.0, 4.0]}),
            json!({"id": 1, "cmd": "sim_build", "guid": 1001}),
            json!({"id": 1, "cmd": "sim_train", "building": 9}),
            json!({"id": 1, "cmd": "sim_train", "guid": 1100}),
            json!({"id": 1, "cmd": "sim_cancel", "index": 1}),
            json!({"id": 1, "cmd": "sim_complete", "index": 1}),
            json!({"id": 1, "cmd": "sim_build", "guid": 1001, "to": [3.0, 4.0], "rotation_steps": 256}),
        ] {
            assert!(
                serde_json::from_value::<Request>(line.clone()).is_err(),
                "{line}"
            );
        }
    }

    #[test]
    fn economy_commands_check_their_parameters() {
        let build = |to, rotation_steps| Command::SimBuild {
            guid: 1001,
            to,
            rotation_steps,
            units: Vec::new(),
            append: false,
        };
        assert_eq!(build([1.0, 2.0], 0).validate(), Ok(()));
        assert_eq!(build([1.0, 2.0], MAX_ROTATION_STEPS).validate(), Ok(()));
        assert!(
            build([1.0, 2.0], 4)
                .validate()
                .unwrap_err()
                .contains("rotation_steps = 4")
        );
        assert!(
            build([f32::NAN, 2.0], 0)
                .validate()
                .unwrap_err()
                .contains("to")
        );
        assert!(build([1.0, f32::INFINITY], 0).validate().is_err());
        let train = |guid| Command::SimTrain { building: 9, guid };
        assert_eq!(train(1100).validate(), Ok(()));
        assert!(train(0).validate().unwrap_err().contains("guid"));
        let cancel = |index| Command::SimCancel { building: 9, index };
        assert_eq!(cancel(0).validate(), Ok(()));
        assert_eq!(cancel(MAX_CANCEL_INDEX).validate(), Ok(()));
        assert!(cancel(-1).validate().unwrap_err().contains("index = -1"));
        assert!(cancel(MAX_CANCEL_INDEX + 1).validate().is_err());
        let complete = |index| Command::SimComplete {
            building: 9,
            index,
            guid: None,
        };
        assert_eq!(complete(0).validate(), Ok(()));
        let typed = |guid| Command::SimComplete {
            building: 9,
            index: 1,
            guid: Some(guid),
        };
        assert_eq!(typed(1100).validate(), Ok(()));
        assert!(typed(0).validate().unwrap_err().contains("guid = 0"));
        assert_eq!(complete(MAX_CANCEL_INDEX).validate(), Ok(()));
        assert!(complete(-1).validate().unwrap_err().contains("index = -1"));
        assert!(complete(MAX_CANCEL_INDEX + 1).validate().is_err());
        let action = |target, to| Command::SimAction {
            units: Vec::new(),
            target,
            to,
            append: true,
            cyclic: false,
        };
        assert_eq!(action(Some(5), None).validate(), Ok(()));
        assert_eq!(action(None, Some([1.0, 2.0])).validate(), Ok(()));
        assert_eq!(action(Some(5), Some([1.0, 2.0])).validate(), Ok(()));
        assert!(
            action(None, None)
                .validate()
                .unwrap_err()
                .contains("target or to")
        );
        assert!(action(Some(5), Some([f32::NAN, 0.0])).validate().is_err());
        let menu = |units, building| Command::SimMenu { units, building };
        assert_eq!(menu(Some(Vec::new()), None).validate(), Ok(()));
        assert_eq!(menu(None, Some(9)).validate(), Ok(()));
        assert!(
            menu(None, None)
                .validate()
                .unwrap_err()
                .contains("units or building")
        );
        assert!(
            menu(Some(vec![1]), Some(9))
                .validate()
                .unwrap_err()
                .contains("mutually exclusive")
        );
        assert_eq!(
            Command::SimSell {
                buildings: Vec::new()
            }
            .validate(),
            Ok(())
        );
        let setup = |resource_setting, living_space| Command::SimSetup {
            mode: None,
            seed: None,
            nations: None,
            teams: None,
            game_speed: None,
            resource_setting,
            living_space,
            map_explored: None,
            slots: None,
            ki_enabled: None,
            campaign_difficulty: None,
            script_enabled: None,
            time_per_char: None,
            time_limit: None,
        };
        assert_eq!(setup(Some(0), Some(0)).validate(), Ok(()));
        assert_eq!(
            setup(Some(MAX_RESOURCE_SETTING), Some(MAX_LIVING_SPACE)).validate(),
            Ok(())
        );
        assert!(
            setup(Some(4), None)
                .validate()
                .unwrap_err()
                .contains("resource_setting = 4")
        );
        assert!(setup(Some(-1), None).validate().is_err());
        assert!(
            setup(None, Some(6))
                .validate()
                .unwrap_err()
                .contains("living_space = 6")
        );
        assert!(setup(None, Some(-1)).validate().is_err());
    }

    #[test]
    fn audio_commands_check_their_parameters() {
        let play = |guid, action, path: Option<&str>, position| Command::PlaySound {
            guid,
            action,
            path: path.map(Into::into),
            position,
        };
        assert_eq!(play(Some(2215), Some(100), None, None).validate(), Ok(()));
        assert_eq!(
            play(None, None, Some("a.wav"), Some([0.0; 3])).validate(),
            Ok(())
        );
        assert!(play(None, None, None, None).validate().is_err());
        assert!(
            play(Some(1), None, None, None)
                .validate()
                .unwrap_err()
                .contains("together")
        );
        assert!(
            play(Some(1), Some(2), Some("a.wav"), None)
                .validate()
                .unwrap_err()
                .contains("mutually exclusive")
        );
        assert!(play(None, None, Some(" "), None).validate().is_err());
        assert!(
            play(None, None, Some("a.wav"), Some([f32::NAN, 0.0, 0.0]))
                .validate()
                .is_err()
        );
        let music = |action, track: Option<&str>, on| Command::Music {
            action,
            track: track.map(Into::into),
            on,
        };
        assert_eq!(music(MusicCommand::Stop, None, None).validate(), Ok(()));
        assert!(music(MusicCommand::Play, None, None).validate().is_err());
        assert!(music(MusicCommand::Attack, None, None).validate().is_err());
        assert_eq!(
            music(MusicCommand::Attack, None, Some(true)).validate(),
            Ok(())
        );
        let volume = |music| Command::SetVolume {
            music,
            sfx: None,
            speech: None,
            mute: None,
            unit_response: None,
        };
        assert_eq!(volume(Some(1000)).validate(), Ok(()));
        assert!(
            volume(Some(1001))
                .validate()
                .unwrap_err()
                .contains("0 to 1000")
        );
        let capture = |action, path: Option<&str>| Command::AudioCapture {
            action,
            path: path.map(Into::into),
        };
        assert!(capture(CaptureCommand::Start, None).validate().is_err());
        assert_eq!(capture(CaptureCommand::Stop, None).validate(), Ok(()));
    }

    // --- AI match (protocol 26) ---------------------------------------------------------------

    #[test]
    fn protocol_version_is_34() {
        // 27 = match from the menu; 28 = scripts: `script_state`,
        // `script_enabled`/`time_per_char` in `sim_setup`; 29 = game end, accounting, campaign;
        // 30 = world state and mission objectives of the scripts; 31 = game UI;
        // 32 = script commands, `script_command`, `DebugDestroy`; 33 = `script_events`, `script_state.ui`,
        // `ScriptSelection`/`ScriptButton`; 34 = `ui_click`/`ui_state` for
        // `GAMEUI`, mouse commands in `opennml-game`; 35 = `ui_present`,
        // `ui_state.game.presentation`; 36 = `key` with `repeat`,
        // `ui_focus_lost`, `ui_state.game.input`; 37 = minimap,
        // diplomacy, chat, `Signal`/`Chat`; 38 = world display
        // `world_display`/`ui_state.game.world`, `sim_build` with ground height; 39 =
        // flags `sim.flags`/`ui_state.game.flags`, lobby color 0 to 8, `time_limit`, `presentation.timers`;
        // 40 = `random_map`; 41 =
        // `random_map` with objects, also in `opennml-game`; 42 = network match in
        // `opennml-game`, `net_*`; 43 = `mouse_button` with `middle` (quirk
        // O-331); 44 = lobby, search and run time in the network match; 45 = menu level;
        // 46 = network savegames; 47 = replays; 48 = 3D pointer;
        // 49 = sound driver; 50 = host migration.
        assert_eq!(PROTOCOL_VERSION, 50);
    }

    fn ki_command(action: KiAction) -> Command {
        kc(action, None, None, None, None, None, None)
    }

    /// `ki_command` with all fields (action, slot, module, arguments, sync, paused, passes).
    fn kc(
        action: KiAction,
        slot: Option<u8>,
        module: Option<u32>,
        args: Option<Vec<i32>>,
        sync: Option<bool>,
        paused: Option<bool>,
        passes: Option<u32>,
    ) -> Command {
        Command::KiCommand {
            action,
            slot,
            module,
            args,
            sync,
            paused,
            passes,
        }
    }

    #[test]
    fn ki_state_and_ki_command_round_trip_and_check_their_parameters() {
        round_trip(
            json!({"id": 7, "cmd": "ki_state"}),
            Command::KiState {
                slot: None,
                values: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_state", "slot": 3, "values": true}),
            Command::KiState {
                slot: Some(3),
                values: Some(true),
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_command", "action": "start", "slot": 1, "module": 195_235, "args": [1, 4, -3], "sync": false}),
            Command::KiCommand {
                action: KiAction::Start,
                slot: Some(1),
                module: Some(195_235),
                args: Some(vec![1, 4, -3]),
                sync: Some(false),
                paused: None,
                passes: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_command", "action": "pause", "paused": false}),
            kc(KiAction::Pause, None, None, None, None, Some(false), None),
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_command", "action": "pause"}),
            ki_command(KiAction::Pause),
        );
        round_trip(
            json!({"id": 7, "cmd": "ki_command", "action": "step", "slot": 7, "passes": 1000}),
            kc(KiAction::Step, Some(7), None, None, None, None, Some(1000)),
        );
        // Check: required fields per action, limits, fields of other actions.
        let start = |slot, module, args: Option<Vec<i32>>| {
            kc(KiAction::Start, slot, module, args, None, None, None)
        };
        let step = |slot, passes| kc(KiAction::Step, slot, None, None, None, None, passes);
        assert_eq!(start(Some(0), Some(9826), None).validate(), Ok(()));
        assert_eq!(
            start(Some(7), Some(1), Some(vec![-1; MAX_KI_ARGS])).validate(),
            Ok(()),
            "64 arguments are allowed"
        );
        assert_eq!(
            start(Some(1), Some(1), Some(vec![0; MAX_KI_ARGS + 1]))
                .validate()
                .unwrap_err(),
            "args has 65 values: allowed at most 64"
        );
        assert_eq!(
            start(None, Some(1), None).validate().unwrap_err(),
            "ki_command start needs slot"
        );
        assert_eq!(
            start(Some(1), None, None).validate().unwrap_err(),
            "ki_command start needs module"
        );
        assert_eq!(
            start(Some(8), Some(1), None).validate().unwrap_err(),
            "slot = 8 invalid: allowed 0 to 7"
        );
        assert_eq!(step(Some(1), None).validate(), Ok(()));
        assert_eq!(step(Some(1), Some(1)).validate(), Ok(()));
        assert_eq!(step(Some(1), Some(MAX_KI_STEP_PASSES)).validate(), Ok(()));
        for passes in [0, MAX_KI_STEP_PASSES + 1] {
            assert_eq!(
                step(Some(1), Some(passes)).validate().unwrap_err(),
                format!("passes = {passes} invalid: allowed 1 to 1000")
            );
        }
        assert_eq!(
            step(None, None).validate().unwrap_err(),
            "ki_command step needs slot"
        );
        assert_eq!(ki_command(KiAction::Pause).validate(), Ok(()));
        for (wrong, field) in [
            (
                kc(KiAction::Start, Some(1), Some(1), None, None, None, Some(1)),
                "passes",
            ),
            (
                kc(KiAction::Step, Some(1), Some(1), None, None, None, None),
                "module",
            ),
            (
                kc(KiAction::Pause, Some(1), None, None, None, None, None),
                "slot",
            ),
            (
                kc(KiAction::Step, Some(1), None, None, None, Some(true), None),
                "paused",
            ),
            (
                kc(KiAction::Pause, None, None, None, Some(true), None, None),
                "sync",
            ),
        ] {
            let error = wrong.validate().unwrap_err();
            assert!(error.contains(field), "{wrong:?}: {error}");
        }
        assert_eq!(
            Command::KiState {
                slot: Some(8),
                values: None
            }
            .validate()
            .unwrap_err(),
            "slot = 8 invalid: allowed 0 to 7"
        );
        // `action` is required, unknown actions are errors.
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"cmd":"ki_command"}"#).is_err());
        assert!(
            serde_json::from_str::<Request>(r#"{"id":1,"cmd":"ki_command","action":"run"}"#)
                .is_err()
        );
    }

    fn slot(kind: SlotSetupKind) -> SlotSetup {
        SlotSetup {
            kind,
            nation: None,
            team: None,
            color: None,
            handicap: None,
            ki_kind: None,
        }
    }

    fn setup_with(
        slots: Option<Vec<SlotSetup>>,
        nations: Option<Vec<String>>,
        teams: Option<Vec<i32>>,
        campaign_difficulty: Option<i32>,
    ) -> Command {
        Command::SimSetup {
            mode: None,
            seed: None,
            nations,
            teams,
            game_speed: None,
            resource_setting: None,
            living_space: None,
            map_explored: None,
            slots,
            ki_enabled: None,
            campaign_difficulty,
            script_enabled: None,
            time_per_char: None,
            time_limit: None,
        }
    }

    #[test]
    fn sim_setup_with_slots_round_trips() {
        round_trip(
            json!({"id": 7, "cmd": "sim_setup", "mode": 0, "slots": [
                {"kind": "human", "nation": 2},
                {"kind": "ai", "nation": 1, "team": 1, "color": 5, "ki_kind": 1},
                {"kind": "closed"},
                {"kind": "ai", "handicap": 3}
            ], "ki_enabled": true, "campaign_difficulty": 0}),
            Command::SimSetup {
                mode: Some(0),
                seed: None,
                nations: None,
                teams: None,
                game_speed: None,
                resource_setting: None,
                living_space: None,
                map_explored: None,
                ki_enabled: Some(true),
                campaign_difficulty: Some(0),
                slots: Some(vec![
                    SlotSetup {
                        nation: Some(2),
                        ..slot(SlotSetupKind::Human)
                    },
                    SlotSetup {
                        nation: Some(1),
                        team: Some(1),
                        color: Some(5),
                        ki_kind: Some(1),
                        ..slot(SlotSetupKind::Ai)
                    },
                    slot(SlotSetupKind::Closed),
                    SlotSetup {
                        handicap: Some(3),
                        ..slot(SlotSetupKind::Ai)
                    },
                ]),
                script_enabled: None,
                time_per_char: None,
                time_limit: None,
            },
        );
        assert_eq!(
            setup_with(Some(vec![slot(SlotSetupKind::Ai); 8]), None, None, Some(2)).validate(),
            Ok(())
        );
        // Unknown kind: reading already fails.
        assert!(
            serde_json::from_str::<Request>(
                r#"{"id":1,"cmd":"sim_setup","slots":[{"kind":"passive"}]}"#
            )
            .is_err()
        );
    }

    /// Lobby color 0 ... 8 (8 = white), 9 and negative values are not accepted.
    #[test]
    fn slot_color_8_is_accepted_9_is_not() {
        let with_color = |c| SlotSetup {
            color: Some(c),
            ..slot(SlotSetupKind::Human)
        };
        for ok in 0..=8 {
            assert_eq!(check_slots(&[with_color(ok)]), Ok(()), "color {ok}");
        }
        for bad in [9, 10, -1] {
            assert_eq!(
                check_slots(&[with_color(bad)]).unwrap_err(),
                format!("slots[0]: color = {bad} invalid: allowed 0 to 8")
            );
        }
        // The line with color 8 goes there and back.
        round_trip(
            json!({"id": 7, "cmd": "sim_setup", "slots": [{"kind": "human", "color": 8}]}),
            setup_with(Some(vec![with_color(8)]), None, None, None),
        );
    }

    /// Time limit in minutes (game type 2/3), 1 ... 35,791; game type 2 goes there and back.
    #[test]
    fn sim_setup_rejects_a_bad_time_limit() {
        let with_limit = |time_limit| {
            let mut command = setup_with(None, None, None, None);
            if let Command::SimSetup {
                mode,
                time_limit: limit,
                ..
            } = &mut command
            {
                *mode = Some(2);
                *limit = time_limit;
            }
            command
        };
        round_trip(
            json!({"id": 7, "cmd": "sim_setup", "mode": 2, "time_limit": 30}),
            with_limit(Some(30)),
        );
        // Without the field it is missing from the line (older responses still read).
        let plain = serde_json::to_value(setup_with(None, None, None, None)).unwrap();
        assert!(plain.get("time_limit").is_none(), "{plain}");
        for ok in [1, 20, 35_791] {
            assert_eq!(with_limit(Some(ok)).validate(), Ok(()), "{ok}");
        }
        assert_eq!(with_limit(None).validate(), Ok(()));
        for bad in [0, -5, 99_999, 35_792] {
            assert_eq!(
                with_limit(Some(bad)).validate().unwrap_err(),
                format!("time_limit = {bad} invalid: allowed 1 to 35791 (minutes)")
            );
        }
        assert_eq!(TIME_LIMIT_RANGE, 1..=35_791);
    }

    // QUIRK E-40: kept
    #[test]
    fn sim_setup_rejects_inconsistent_slots() {
        let ai = |ki_kind, handicap| SlotSetup {
            ki_kind,
            handicap,
            ..slot(SlotSetupKind::Ai)
        };
        let one =
            |s: SlotSetup| setup_with(Some(vec![slot(SlotSetupKind::Human), s]), None, None, None);
        let cases = [
            (
                setup_with(
                    Some(vec![ai(Some(1), None)]),
                    Some(vec!["SPA".into()]),
                    None,
                    None,
                ),
                "slots excludes nations and teams",
            ),
            (
                setup_with(Some(vec![ai(Some(1), None)]), None, Some(vec![0]), None),
                "slots excludes nations and teams",
            ),
            (
                setup_with(Some(vec![slot(SlotSetupKind::Human); 9]), None, None, None),
                "slots has 9 entries: allowed at most 8",
            ),
            (
                one(ai(Some(0), None)),
                "slots[1]: ki_kind = 0 invalid: allowed 1 to 4",
            ),
            (
                one(ai(Some(5), None)),
                "slots[1]: ki_kind = 5 invalid: allowed 1 to 4",
            ),
            (
                one(SlotSetup {
                    ki_kind: Some(2),
                    ..slot(SlotSetupKind::Human)
                }),
                "slots[1]: ki_kind only with kind ai",
            ),
            (
                one(ai(Some(2), Some(1))),
                "slots[1]: ki_kind and handicap are mutually exclusive (the AI kind sets the handicap)",
            ),
            (
                one(ai(None, Some(6))),
                "slots[1]: handicap = 6 invalid: allowed 0 to 5",
            ),
            (
                one(ai(None, Some(-1))),
                "slots[1]: handicap = -1 invalid: allowed 0 to 5",
            ),
            (
                one(SlotSetup {
                    nation: Some(6),
                    ..slot(SlotSetupKind::Human)
                }),
                "slots[1]: nation = 6 invalid: allowed 0 to 5",
            ),
            (
                one(SlotSetup {
                    color: Some(9),
                    ..slot(SlotSetupKind::Closed)
                }),
                "slots[1]: color = 9 invalid: allowed 0 to 8",
            ),
            (
                one(SlotSetup {
                    color: Some(-1),
                    ..slot(SlotSetupKind::Human)
                }),
                "slots[1]: color = -1 invalid: allowed 0 to 8",
            ),
            (
                setup_with(None, None, None, Some(3)),
                "campaign_difficulty = 3 invalid: allowed 0 to 2 (as in the campaign menu)",
            ),
            (
                setup_with(None, None, None, Some(-1)),
                "campaign_difficulty = -1 invalid: allowed 0 to 2 (as in the campaign menu)",
            ),
        ];
        for (command, message) in cases {
            assert_eq!(command.validate().unwrap_err(), message, "{command:?}");
        }
        // Valid: AI kind 1-4 or handicap 0-5, difficulty 0-2, nations without slots.
        for ok in [
            one(ai(Some(1), None)),
            one(ai(Some(4), None)),
            one(ai(None, Some(0))),
            one(ai(None, Some(5))),
            setup_with(None, Some(vec!["SPA".into()]), Some(vec![0]), Some(0)),
            setup_with(Some(Vec::new()), None, None, Some(2)),
        ] {
            assert_eq!(ok.validate(), Ok(()), "{ok:?}");
        }
    }

    // --- Scripts (protocol 28) -----------------------------------------------------------------

    #[test]
    fn script_state_round_trips_and_checks_its_parameters() {
        round_trip(
            json!({"id": 7, "cmd": "script_state"}),
            Command::ScriptState {
                block: None,
                names: None,
                log: None,
            },
        );
        round_trip(
            json!({"id": 7, "cmd": "script_state", "block": {"container": -1, "index": 3}, "names": "hero", "log": 1000}),
            Command::ScriptState {
                block: Some(ScriptBlockRef {
                    container: -1,
                    index: 3,
                }),
                names: Some("hero".into()),
                log: Some(1000),
            },
        );
        let state = |log| Command::ScriptState {
            block: None,
            names: None,
            log: Some(log),
        };
        assert_eq!(state(0).validate(), Ok(()), "0 = without log");
        assert_eq!(state(MAX_SCRIPT_LOG_STEPS).validate(), Ok(()));
        assert_eq!(
            state(1001).validate().unwrap_err(),
            "log = 1001 invalid: allowed 0 to 1000 steps"
        );
        assert_eq!((MAX_SCRIPT_LOG_STEPS, DEFAULT_SCRIPT_LOG_STEPS), (1000, 50));
        // A block needs container and index.
        assert!(
            serde_json::from_str::<Request>(
                r#"{"id":1,"cmd":"script_state","block":{"container":0}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn script_events_round_trip_and_check_their_steps() {
        // Protocol 33: without `steps` the field is missing (default 50), otherwise at most 1000.
        round_trip(
            json!({"id": 7, "cmd": "script_events"}),
            Command::ScriptEvents { steps: None },
        );
        round_trip(
            json!({"id": 7, "cmd": "script_events", "steps": 1000}),
            Command::ScriptEvents { steps: Some(1000) },
        );
        assert_eq!(Command::ScriptEvents { steps: Some(0) }.validate(), Ok(()));
        assert_eq!(
            Command::ScriptEvents {
                steps: Some(MAX_SCRIPT_LOG_STEPS)
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            Command::ScriptEvents { steps: Some(1001) }
                .validate()
                .unwrap_err(),
            "steps = 1001 invalid: allowed 0 to 1000 steps"
        );
        assert!(
            serde_json::from_str::<Request>(r#"{"id":1,"cmd":"script_events","steps":-1}"#)
                .is_err()
        );
    }

    #[test]
    fn script_command_round_trips_and_rejects_unknown_actions() {
        // Protocol 32: `skip_dialog` is the only action.
        round_trip(
            json!({"id": 7, "cmd": "script_command", "action": "skip_dialog"}),
            Command::ScriptCommand {
                action: ScriptAction::SkipDialog,
            },
        );
        assert_eq!(
            Command::ScriptCommand {
                action: ScriptAction::SkipDialog
            }
            .validate(),
            Ok(())
        );
        // Without an action and with an unknown action: error when reading the line.
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"cmd":"script_command"}"#).is_err());
        let error =
            serde_json::from_str::<Request>(r#"{"id":1,"cmd":"script_command","action":"skip"}"#)
                .unwrap_err()
                .to_string();
        assert!(error.contains("skip_dialog"), "{error}");
    }

    fn script_setup(script_enabled: Option<bool>, time_per_char: Option<i32>) -> Command {
        Command::SimSetup {
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
            script_enabled,
            time_per_char,
            time_limit: None,
        }
    }

    #[test]
    fn sim_setup_script_fields_round_trip_and_are_checked() {
        round_trip(
            json!({"id": 7, "cmd": "sim_setup", "script_enabled": true, "time_per_char": 80}),
            script_setup(Some(true), Some(80)),
        );
        // Without the fields they are missing from the line (older responses still read).
        round_trip(
            json!({"id": 7, "cmd": "sim_setup"}),
            script_setup(None, None),
        );
        for ok in [1, 55, 1000] {
            assert_eq!(script_setup(Some(false), Some(ok)).validate(), Ok(()));
        }
        for bad in [0, -5, 1001] {
            assert_eq!(
                script_setup(None, Some(bad)).validate().unwrap_err(),
                format!("time_per_char = {bad} invalid: allowed 1 to 1000 (ms per character)")
            );
        }
        assert_eq!(TIME_PER_CHAR_RANGE, 1..=1000);
    }

    #[test]
    fn sim_result_round_trips_and_validates() {
        // `sim_result {player, result}` - slot 0-7, result 1 (lost) or 2 (won);
        // the slot does not have to be the local one.
        round_trip(
            json!({"id": 7, "cmd": "sim_result", "player": 1, "result": 2}),
            Command::SimResult {
                player: 1,
                result: 2,
            },
        );
        for (player, result) in [(0, 1), (0, 2), (7, 1), (7, 2)] {
            assert_eq!(Command::SimResult { player, result }.validate(), Ok(()));
        }
        assert_eq!(
            Command::SimResult {
                player: 8,
                result: 2
            }
            .validate()
            .unwrap_err(),
            "player = 8 invalid: allowed 0 to 7"
        );
        for bad in [0, 3, -1] {
            assert_eq!(
                Command::SimResult {
                    player: 0,
                    result: bad
                }
                .validate()
                .unwrap_err(),
                format!("result = {bad} invalid: allowed 1 (lost) or 2 (won)")
            );
        }
        // Required fields missing -> error when reading.
        assert!(
            serde_json::from_value::<Request>(json!({"id": 1, "cmd": "sim_result", "player": 0}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<Request>(json!({"id": 1, "cmd": "sim_result", "result": 2}))
                .is_err()
        );
    }
}
