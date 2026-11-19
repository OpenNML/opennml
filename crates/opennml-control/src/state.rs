//! State snapshot of the viewer (result of `state` and `step`).

use serde::{Deserialize, Serialize};

use crate::protocol::{WeatherName, WindowModeName};

/// What the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewName {
    Map,
    Browser,
    /// Effect browser (key N, protocol 4).
    Effects,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub mode: WindowModeName,
    /// Selected monitor (0-based).
    pub monitor: usize,
    pub monitor_count: usize,
    /// Inner window size in pixels.
    pub inner_size: [u32; 2],
    /// Size of the surface (can be smaller than the window; minimized: last size != 0).
    pub surface_size: [u32; 2],
    pub minimized: bool,
    /// Last frame reported "occluded" or "no surface".
    pub occluded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockMode {
    Realtime,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockState {
    pub mode: ClockMode,
    /// Virtual time since the start in milliseconds.
    pub now_ms: f64,
}

/// Real frame times of the most recently shown frames (ms).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameTimes {
    pub count: usize,
    pub min_ms: f64,
    pub mean_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    /// Completed frames per second (as in the window title).
    pub fps: u32,
    /// `None` as long as fewer than two frames have been shown.
    pub frame_times: Option<FrameTimes>,
    /// Frames shown since the start.
    pub presented: u64,
}

/// Original camera of the map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraState {
    pub distance: f32,
    pub pitch_deg: f32,
    /// Rotation around the vertical axis: direction target -> eye, `atan2(x, z)` in degrees.
    pub yaw_deg: f32,
    pub target: [f32; 3],
    pub eye: [f32; 3],
}

/// Free debug camera (F2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreeCameraState {
    pub position: [f32; 3],
    pub yaw_deg: f32,
    pub pitch_deg: f32,
}

/// Game object under the mouse pointer (the same pick as the overlay line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoveredObject {
    pub type_name: String,
    pub guid: u32,
    pub index: usize,
    pub player: Option<i32>,
    pub position: [f32; 3],
    pub model: String,
    /// Base module of the first created type (`Einheit`, `Gebäude`, ...); `None` without an entry in
    /// `property.ini` (protocol 3).
    #[serde(default)]
    pub base_module: Option<String>,
    /// Types that the game creates (rider: rider and horse).
    #[serde(default)]
    pub parts: Vec<u32>,
    /// Shadow kind 0-3 for the current `shadow_quality`.
    #[serde(default)]
    pub shadow_kind: u8,
    /// ID of the simulation object (protocol 6); `index` is then the position in the
    /// object list of the map (ambient effects first, then the simulation objects).
    #[serde(default)]
    pub sim_id: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AspectName {
    Correct,
    Original43,
    Stretch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Toggles {
    pub fog: bool,
    pub aspect: AspectName,
    pub animate: bool,
    pub help: bool,
}

/// Running animation in the model browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimationState {
    /// Index in the animation list of the model (0-based).
    pub index: usize,
    /// File of the animation from the `.cfg`.
    pub file: Option<String>,
    pub elapsed_ms: f32,
    /// `None` if the animation cannot be loaded.
    pub duration_ms: Option<f32>,
    pub looping: bool,
}

/// Orbit camera of the model browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrbitState {
    pub distance: f32,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
}

/// Animation switch remembered during the 200 ms lock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingSwitch {
    /// Target; `None` = bind pose.
    pub animation: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowserState {
    pub index: usize,
    pub count: usize,
    pub path: String,
    /// Error message of the selected model.
    pub status: Option<String>,
    /// Camera: left mouse button rotates, mouse wheel zooms.
    pub orbit: OrbitState,
    /// `None` = bind pose.
    pub animation: Option<AnimationState>,
    /// Space bar pause of the browser.
    pub paused: bool,
    pub crossfade_active: bool,
    /// Remaining time of the 200 ms lock (0 = free).
    pub lock_remaining_ms: f32,
    pub pending: Option<PendingSwitch>,
    /// Shadow kind 0-3 of the model (F8 choice or that of the type; protocol 3).
    #[serde(default)]
    pub shadow_kind: u8,
    /// Object type of the model from the project file.
    #[serde(default)]
    pub type_guid: Option<u32>,
    /// Shadows of the last frame.
    #[serde(default)]
    pub shadows: Option<ShadowState>,
    /// Burn intensity 0-3 (protocol 4).
    #[serde(default)]
    pub damage: f32,
    /// Preview switch (`aus`, `Kielwasser`, `Radstaub`, `Bremsfunken`, `Pfiff`).
    #[serde(default)]
    pub preview: String,
    /// Hit zones of the model (`tzone%02d`).
    #[serde(default)]
    pub hit_zones: usize,
    /// Effects of the model.
    #[serde(default)]
    pub effects: Option<EffectState>,
    /// Sound actions of the type (`sounddatabase.dat`) and the selected one (T/Shift+T;
    /// protocol 5).
    #[serde(default)]
    pub sound_actions: Vec<i32>,
    #[serde(default)]
    pub sound_action: Option<i32>,
}

/// Effects of a view (protocol 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectState {
    /// Map: `particles_quality` with which the effects are built; model and
    /// effect browser: always 2 (they create all effects independent of the setting).
    pub quality: u8,
    /// Own effects of the objects (permanent effects, locomotive smoke, fishing basket fish, preview).
    pub owned: usize,
    /// World effects with `.pef` (e.g. `schiff_stehend`, `spawn_effect`, HEF).
    pub world: usize,
    /// Ambient effects: surf, fish (layer 2 and fishing baskets), eagles.
    pub surf: usize,
    pub fish: usize,
    pub eagles: usize,
    /// Living particles.
    pub particles: usize,
    /// Frozen effects (200 ms rule).
    pub frozen: usize,
    /// Quads and models drawn in the last frame.
    pub quads: usize,
    pub models: usize,
}

/// Effect browser (key N, protocol 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectBrowserState {
    pub index: usize,
    pub count: usize,
    pub path: String,
    pub status: Option<String>,
    /// Run time since selection or restart.
    pub time_ms: f32,
    /// Radius of the `.hef` preview.
    pub radius: f32,
    pub orbit: OrbitState,
    pub effects: EffectState,
}

/// Shadows of the last frame (protocol 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowState {
    /// `shadow_quality` (map) or setting of the browser.
    pub quality: u8,
    /// Objects per shadow kind 0-3 (all, independent of visibility).
    pub kinds: [usize; 4],
    /// Drawn shadow decals, ground planes, decals of the special class.
    pub decals: usize,
    pub groundplanes: usize,
    pub specials: usize,
    /// Drawn texture shadows kind 2 and kind 3.
    pub persistent: usize,
    pub per_frame: usize,
    /// Shadow textures rendered in the last frame.
    pub rendered: usize,
    /// Occupied atlas cells (128^2, quarters count 1/4), number of cells, shadows without a cell.
    pub atlas_used: f32,
    pub atlas_capacity: usize,
    pub atlas_full: usize,
}

/// An occupied sound channel (protocol 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceState {
    /// Sound ID (`SoundManager+0x44`).
    pub id: u32,
    /// FMOD channel (0-31), listed in mixing order.
    pub channel: usize,
    pub file: String,
    /// `music`, `sfx` or `speech`.
    pub category: String,
    /// Ogg stream (music, speech) instead of a sample.
    pub stream: bool,
    /// World position of a 3D sound.
    pub position: Option<[f32; 3]>,
    /// `FSOUND_SetVolume` (0-255), 3D attenuation (0-255) and pan (0-255, `None` = stereo).
    pub volume: i32,
    pub vol3d: i32,
    pub pan: Option<u8>,
    /// Gain left/right (0-255) in the next block.
    pub gains: [i32; 2],
    /// Stream is fading right now.
    pub fading: bool,
}

/// A recently requested voice: selection sound or command acknowledgment (protocol 9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentVoiceState {
    /// `select` (selection sound) or `response` (command acknowledgment).
    pub kind: String,
    /// ID of the object and its type GUID (key of the sound database).
    pub object: u32,
    pub guid: u32,
    /// Action (100 selection, 103 while building, 12 execute, 14 attack), flags, category.
    pub action: i32,
    pub flags: u32,
    pub category: String,
    /// Variant: -1 random, otherwise counter value.
    pub index: i32,
    /// File of the started sound; `null`: no voice in the database or rejected.
    pub file: Option<String>,
}

/// A recently played combat sound (`SimEvent::Sound3d`, protocol 11; re/combat-host 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentSoundState {
    /// Action of the sound database of the type (e.g. 5 hit) and flags of the call.
    pub action: i32,
    pub flags: u32,
    /// Type GUID with which the database was queried.
    pub guid: u32,
    /// ID of the object at which the sound arose (0 without).
    pub object: u32,
}

/// Music state machine (protocol 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicState {
    /// `script` (status 0), `normal` (1) or `attack` (2).
    pub status: String,
    /// File of the running level music stream.
    pub playing: Option<String>,
    /// Normal and combat music (`+0x94`, `+0x98`).
    pub current: String,
    pub attack_track: String,
    /// Combat music switched on by hand (Shift+F9, `music attack`): override of the attack points.
    pub attack_enabled: bool,
    /// Attack points that the music state machine received last (protocol 11; value of the simulation for the
    /// active player, re/combat-host 5).
    #[serde(default)]
    pub attack_points: i32,
    /// Where the combat music comes from: `sim` (attack points of the simulation) or `manual` (override
    /// `attack_enabled`; protocol 11).
    #[serde(default = "attack_source_default")]
    pub attack_source: String,
    /// Switch timer and lock in ms.
    pub timer_ms: i32,
    pub pause_ms: i32,
    /// Running menu music.
    pub menu: Option<String>,
}

/// Running offline mix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureState {
    pub path: String,
    pub frames: u64,
}

/// Sound (protocol 5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioState {
    /// Output device (`None`: muted without a device).
    pub device: Option<String>,
    /// F9: no device output (mixer and recording keep running).
    pub mute: bool,
    /// Volume controls for music, sound effects, speech (0-1000).
    pub volumes: [i32; 3],
    /// Listener in tiles (`x/128, 0, z/128`).
    pub listener: [f32; 3],
    /// Mixed frames since the start (44,100 per second of virtual time).
    pub frames: u64,
    pub voices: Vec<VoiceState>,
    pub music: MusicState,
    pub capture: Option<CaptureState>,
    /// `Engine.ini UnitResponse`: selection voice of the units and command acknowledgment.
    #[serde(default = "unit_response_default")]
    pub unit_response: bool,
    /// Counter of the acknowledgment variants (`GameWorld+0x2424`).
    #[serde(default)]
    pub responses: i32,
    /// Type GUID of the GUID lock of the selection sound (`Ui+0x24`).
    #[serde(default)]
    pub select_lock: Option<u32>,
    /// Most recently requested voices, oldest first (at most 8).
    #[serde(default)]
    pub recent_voices: Vec<RecentVoiceState>,
    /// Most recently played combat sounds (`Sound3d`), oldest first, at most 16 (protocol 11).
    #[serde(default)]
    pub recent_sounds: Vec<RecentSoundState>,
}

/// Without an override the simulation determines the combat music.
fn attack_source_default() -> String {
    "sim".into()
}

/// As in the original (`Engine.ini <UnitResponse>1</>`).
fn unit_response_default() -> bool {
    true
}

/// Light, weather and quality levels of the map (protocol 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentState {
    /// `level` (values of the `.wld`) or name of the light preset.
    pub light: String,
    pub weather: WeatherName,
    /// Active weather particles (snow: one per tile, rain: about half).
    pub weather_active: usize,
    /// Particles moved in the last logic step (visible tiles only).
    pub weather_moved: usize,
    /// Particles drawn in the last frame.
    pub weather_drawn: u32,
    pub shadow_quality: u8,
    pub particles_quality: u8,
    pub terrain_quality: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorState {
    /// Mouse position in window pixels.
    pub position: Option<[f64; 2]>,
    pub left_button: bool,
    pub right_button: bool,
    /// Keys held by the user (sorted).
    pub keys_real: Vec<String>,
    /// Keys held via the control channel (sorted).
    pub keys_control: Vec<String>,
}

/// State of the world display (re/world-display): rally point flag, ground marks, wheel tracks,
/// water rings, silhouettes, beams and dynamite charges of the map. Display of the host only (not in the savegame);
/// viewer `state.world_display`, `opennml-game` `ui_state.game.world`. Missing fields read with their defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldDisplayState {
    /// Rally point flag of the last frame (`None` = none; re/world-display W.1).
    pub rally_flag: Option<RallyFlagState>,
    /// Footprints and craters per texture, sorted by path (layer 4; a pair counts once).
    pub marks: Vec<MarkCount>,
    /// Points of all wheel tracks (gaps included).
    pub tread_points: u32,
    /// Living water rings of the units (`SurfRing`, layer 1).
    pub surf_rings: u32,
    /// Object IDs whose renderer entry currently carries the silhouette bit `0x80` (sorted; re/world-display W.2.1).
    pub stealth: Vec<u32>,
    /// Running suction beams and animal evoker beams, sorted by (owner, effect).
    pub beams: Vec<BeamState>,
    /// Dynamite charges by projectile ID.
    pub charges: Vec<ChargeState>,
}

/// Response of `random_map`: the random map of the preview (`--random-map`) with seed, script, defines,
/// size, player areas, random number consumption and checksums of the generator (`opennml_randommap::RandomMap`).
/// Without a random map the viewer answers `null`. Read-only; there is no simulation for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RandomMapState {
    /// Seed text as given.
    pub seed_text: String,
    /// Seed = sum of `(i+1) · wchar[i]` of the seed text (`Game_StartRandomMapFromSeedText` `0x004BF3E0`).
    pub hash: u32,
    /// Script path (`MSCRIPT` of the map type).
    pub script: String,
    /// Defines from the menu in list order (size, game mode, landscape, time of day).
    pub defines: Vec<String>,
    /// Edge length in tiles (64/128/256).
    pub dim: u32,
    /// Player areas `[id, cx, cz, min_size]` in list order.
    pub players: Vec<[i32; 4]>,
    /// `rand()` draws per phase: preprocessor, land areas, player areas, terrain areas, texture GUIDs,
    /// objects.
    pub rand_counts: [u32; 6],
    /// State of the `rand()` sequence at the end of the generator.
    pub rand_state: u32,
    /// Checksums (FNV-1a 64, 16 hex digits) of heights, texture numbers, tile GUIDs, steepness, area IDs,
    /// placed objects and blocking map of the object step.
    pub digests: [String; 7],
    /// Placed objects as `[GUID, count]`, sorted by GUID.
    pub objects: Vec<[u32; 2]>,
    /// Slots 0...7 on which an object of this owner was placed (`RM+0x50`).
    pub slots_used: [bool; 8],
    /// Positions of the town centers (`TOWN_CENTER`, GUID 4200) in placement order `[x, y, z]` as the generator
    /// places them (y = 0; only the world supplies the height).
    pub start_points: Vec<[f32; 3]>,
}

/// Rally point flag (`PropertyBuilding_RenderMeetingFlag` `0x00507BB0`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RallyFlagState {
    /// ID of the selected building.
    pub building: u32,
    /// Position of the flag in the last frame (world coordinates; with a rally target on top of the target).
    pub position: [f32; 3],
    /// Flag color (table `0x00664158`).
    pub color: [f32; 3],
    /// Animation time (clock of the effect world in ms; stands still in pause and the game menu).
    pub anim_ms: f32,
}

/// Ground marks of a texture (`EffectWorld::mark_counts`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarkCount {
    pub texture: String,
    /// All footprints or craters with this texture.
    pub count: u32,
    /// Of these in the render list (visible since the last game tick).
    pub visible: u32,
}

/// A beam (suction beam `Geist_angriff.pef` of the ghost, beam `Tierbeschwoerer.pef` of the animal evoker).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeamState {
    /// ID of the ghost or of the evoker.
    pub owner: u32,
    /// Effect path.
    pub path: String,
    /// Position of the effect (`+0x138`, position of the last logic step).
    pub position: [f32; 3],
    /// Target point (`+0x160`), once set.
    pub target: Option<[f32; 3]>,
    /// Flight time to the target in s (`+0x16C`, default 1).
    pub flight_s: f32,
    /// Living particles.
    pub particles: u32,
}

/// A dynamite charge (`DynamiteEffect`, re/world-display W.5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChargeState {
    /// ID of the projectile.
    pub projectile: u32,
    /// Position of the last frame (place of the explosion).
    pub position: [f32; 3],
    /// Yaw of the last frame (radians).
    pub yaw: f32,
    /// Last displayed digit (`m_LastCount`, 10 before the first digit, then 9 ... 0).
    pub last_digit: i32,
    /// Follows the target (placed with a target, not found after loading).
    pub follows: bool,
}

/// State snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub view: ViewName,
    pub window: WindowState,
    pub clock: ClockState,
    pub frames: FrameStats,
    /// Original camera of the map (`None` without a map); it stands still while the free camera is active.
    pub camera: Option<CameraState>,
    /// Free camera (F2), otherwise `None`.
    pub free_camera: Option<FreeCameraState>,
    pub hovered: Option<HoveredObject>,
    pub toggles: Toggles,
    /// `None` as long as the browser has not been created.
    pub browser: Option<BrowserState>,
    /// Light and weather of the map (`None` without a map).
    #[serde(default)]
    pub environment: Option<EnvironmentState>,
    /// Shadows of the map (`None` without a map; protocol 3).
    #[serde(default)]
    pub shadows: Option<ShadowState>,
    /// Effects of the map (`None` without a map; protocol 4).
    #[serde(default)]
    pub effects: Option<EffectState>,
    /// `None` as long as the effect browser has not been created (protocol 4).
    #[serde(default)]
    pub effect_browser: Option<EffectBrowserState>,
    /// Sound (protocol 5).
    #[serde(default)]
    pub audio: Option<AudioState>,
    /// Simulation of the map (`None` without a map or without loadable rules; protocol 6).
    #[serde(default)]
    pub sim: Option<crate::sim::SimSection>,
    /// State of the world display of the map (`None` without a simulation; protocol 38).
    #[serde(default)]
    pub world_display: Option<WorldDisplayState>,
    pub cursor: CursorState,
    /// Overlay lines of the last frame.
    pub overlay: Vec<String>,
}

impl StateSnapshot {
    /// Example values (tests, documentation).
    pub fn example() -> Self {
        Self {
            view: ViewName::Map,
            window: WindowState {
                mode: WindowModeName::Windowed,
                monitor: 0,
                monitor_count: 3,
                inner_size: [1280, 720],
                surface_size: [1280, 720],
                minimized: false,
                occluded: false,
            },
            clock: ClockState {
                mode: ClockMode::Paused,
                now_ms: 1500.0,
            },
            frames: FrameStats {
                fps: 144,
                frame_times: Some(FrameTimes {
                    count: 120,
                    min_ms: 6.5,
                    mean_ms: 6.9,
                    max_ms: 8.25,
                }),
                presented: 900,
            },
            camera: Some(CameraState {
                distance: 18447.0,
                pitch_deg: 38.5,
                yaw_deg: 180.0,
                target: [4096.0, 120.0, 2048.0],
                eye: [4096.0, 11600.0, -12400.0],
            }),
            free_camera: None,
            hovered: Some(HoveredObject {
                type_name: "Palm".into(),
                guid: 1234,
                index: 12,
                player: None,
                position: [4100.0, 120.0, 2050.0],
                model: "data/r3d/palme.rdo".into(),
                base_module: Some("Natur".into()),
                parts: vec![1234],
                shadow_kind: 1,
                sim_id: Some(0x0009_4801),
            }),
            toggles: Toggles {
                fog: false,
                aspect: AspectName::Correct,
                animate: true,
                help: false,
            },
            browser: Some(BrowserState {
                index: 3,
                count: 812,
                path: "data/r3d/a.rdo".into(),
                status: None,
                orbit: OrbitState {
                    distance: 200.0,
                    yaw_deg: 180.0,
                    pitch_deg: 20.0,
                },
                animation: Some(AnimationState {
                    index: 0,
                    file: Some("walk.rdm".into()),
                    elapsed_ms: 250.0,
                    duration_ms: Some(1000.0),
                    looping: true,
                }),
                paused: false,
                crossfade_active: true,
                lock_remaining_ms: 120.0,
                pending: Some(PendingSwitch { animation: Some(2) }),
                shadow_kind: 3,
                type_guid: Some(2006),
                shadows: None,
                damage: 1.25,
                preview: "Kielwasser".into(),
                hit_zones: 4,
                effects: None,
                sound_actions: vec![12, 14, 100, 101],
                sound_action: Some(100),
            }),
            environment: Some(EnvironmentState {
                light: "Sommer Nacht".into(),
                weather: WeatherName::Snow,
                weather_active: 16384,
                weather_moved: 640,
                weather_drawn: 640,
                shadow_quality: 2,
                particles_quality: 2,
                terrain_quality: 0,
            }),
            shadows: Some(ShadowState {
                quality: 2,
                kinds: [120, 2400, 40, 12],
                decals: 830,
                groundplanes: 14,
                specials: 3,
                persistent: 9,
                per_frame: 4,
                rendered: 4,
                atlas_used: 13.0,
                atlas_capacity: 4096,
                atlas_full: 0,
            }),
            effects: Some(EffectState {
                quality: 2,
                owned: 53,
                world: 30,
                surf: 482,
                fish: 48,
                eagles: 0,
                particles: 512,
                frozen: 30,
                quads: 662,
                models: 79,
            }),
            effect_browser: None,
            audio: Some(AudioState {
                device: Some("Speakers".into()),
                mute: false,
                volumes: [349, 599, 499],
                listener: [32.0, 0.0, 16.0],
                frames: 66150,
                voices: vec![VoiceState {
                    id: 4,
                    channel: 1,
                    file: "Data\\SFX\\Natureffekte\\NAT_Brandung_02.wav".into(),
                    category: "sfx".into(),
                    stream: false,
                    position: Some([4300.0, 0.0, 2100.0]),
                    volume: 15,
                    vol3d: 255,
                    pan: Some(200),
                    gains: [5, 13],
                    fading: false,
                }],
                music: MusicState {
                    status: "normal".into(),
                    playing: Some("data/Music/atmo_karibik.ogg".into()),
                    current: "data/Music/atmo_karibik.ogg".into(),
                    attack_track: "data/Music/spa_attack.ogg".into(),
                    attack_enabled: false,
                    attack_points: 390,
                    attack_source: "sim".into(),
                    timer_ms: 1000,
                    pause_ms: 0,
                    menu: None,
                },
                capture: None,
                unit_response: true,
                responses: 3,
                select_lock: Some(1100),
                recent_voices: vec![RecentVoiceState {
                    kind: "response".into(),
                    object: 0x0001_0001,
                    guid: 1100,
                    action: 12,
                    flags: 0x18,
                    category: "speech".into(),
                    index: 2,
                    file: Some("Data/SFX/Einheiten_SPA/SPA_Kolonist_done_03.wav".into()),
                }],
                recent_sounds: vec![RecentSoundState {
                    action: 5,
                    flags: 0x12,
                    guid: 1107,
                    object: 0x0001_0001,
                }],
            }),
            sim: Some(crate::sim::SimSection {
                tick: 15,
                steps: 30,
                mode: 0,
                seed: 0,
                game_speed: 14,
                active_player: 0,
                checksum: "3a628284f537f62e".into(),
                objects: 3504,
                selection: vec![0x0001_0001],
                commands: 1,
                grid_layer: Some(0),
                path: None,
                moving: 2,
                wood_price: 1024,
                food_price: 1351,
                max_living_space: 125,
                notices: vec![crate::sim::SimNotice {
                    step: 20,
                    id: 2,
                    tile: [-1, -1],
                    text: Some("Not enough wood".into()),
                }],
                target_marker: Some(crate::sim::SimTargetMarker {
                    id: 0x0009_4801,
                    remaining_ms: 150.0,
                }),
                ground_markers: vec![0x0001_0001, 0x0009_4801],
                fog_shown: true,
                fog_overlay: Some(3),
                attack_points: [390, 0, 0, 0, 0, 0, 0, 0],
                attack_points_active: 390,
                projectiles: vec![crate::sim::SimProjectileInfo {
                    id: 3,
                    kind: "arrow".into(),
                    fire: false,
                    remaining_ms: 120,
                    flight_ms: 320,
                    shooter: Some(0x0001_0001),
                    target: Some(0x0002_0001),
                    target_pos: [1280.0, 40.0, 1024.0],
                    pulse: None,
                    impact: None,
                }],
                flights: 1,
                groups: vec![],
                formation: 0,
                flags: None,
            }),
            world_display: Some(WorldDisplayState {
                rally_flag: Some(RallyFlagState {
                    building: 0x0001_0005,
                    position: [1024.0, 25.0, 2048.0],
                    color: [1.0, 0.5, 0.25],
                    anim_ms: 1500.0,
                }),
                marks: vec![MarkCount {
                    texture: "data/effects/textures/fuss.tga".into(),
                    count: 4,
                    visible: 3,
                }],
                tread_points: 12,
                surf_rings: 2,
                stealth: vec![0x0002_0003],
                beams: vec![
                    BeamState {
                        owner: 0x0001_0007,
                        path: "data/effects/Geist_angriff.pef".into(),
                        position: [512.0, 50.0, 768.0],
                        target: Some([530.0, 80.0, 770.0]),
                        flight_s: 0.25,
                        particles: 14,
                    },
                    BeamState {
                        owner: 0x0001_0008,
                        path: "data/effects/Tierbeschwoerer.pef".into(),
                        position: [0.0; 3],
                        target: None,
                        flight_s: 1.0,
                        particles: 0,
                    },
                ],
                charges: vec![ChargeState {
                    projectile: 4,
                    position: [300.0, 12.5, 700.0],
                    yaw: 1.5,
                    last_digit: 7,
                    follows: true,
                }],
            }),
            cursor: CursorState {
                position: Some([640.0, 360.0]),
                left_button: false,
                right_button: false,
                keys_real: vec![],
                keys_control: vec!["ArrowUp".into()],
            },
            overlay: vec!["144 FPS | Vulkan | Test adapter".into()],
        }
    }
}

/// Role of the machine in the network session (`net_status`); none during the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetRoleName {
    Host,
    Client,
}

/// Phase of the network session (`net_status`: waiting for clients, loading, running, halted, ended; search,
/// connecting, lobby of the menu path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetPhaseName {
    /// `LAN1` is searching for open games (without a role).
    Searching,
    /// Join in progress: connection setup, registration, first lobby state (menu path and `--join`).
    Connecting,
    /// In the lobby `LAN2` of the menu path (host and client).
    Lobby,
    /// Registration: the host waits for clients, the client for `Start`.
    Gathering,
    /// `Start` has been distributed, the machines are loading; tick 0 not until `Go`.
    Loading,
    /// Turns are running.
    Running,
    /// Halt after an execution tick (pause or resync, [`NetStatusData::halt`]).
    Halted,
    /// Stopped after the third desync, for good.
    Stopped,
    /// The session is over (end, abort, rejection, error, leaving).
    Ended,
}

/// Reason of a halt (`net_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetHaltName {
    Pause,
    Resync,
}

/// Why the machine is currently not computing (`net_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetWaitName {
    /// Before `Go`: registration or loading.
    Starting,
    /// Before the execution tick of a turn whose bundle is missing ([`NetStatusData::waiting_turn`]).
    Bundle,
    /// Standing after an execution tick (pause or resync).
    Halted,
    /// Standing after a stop for good.
    Stopped,
    /// The session is over.
    Ended,
}

/// A slot of the slot assignment from the start (`net_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetSeatData {
    pub slot: u8,
    /// Machine of the slot: 0 host, 1 ... 7 clients; `None` = AI or closed.
    pub peer: Option<u8>,
    pub name: String,
}

/// A registered machine (`net_status`; host first, clients in order of joining).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetPeerData {
    pub peer: u8,
    pub name: String,
}

/// State of a connection (`net_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetLinkData {
    /// Connection number (on the client: the host, 0).
    pub peer: u8,
    /// Slot of the remote end after the start.
    pub slot: Option<u8>,
    /// Time since the last reception in ms (keep-alives included).
    pub silent_ms: u64,
}

/// Round-trip time to a machine (`net_status`; host only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetRttData {
    pub peer: u8,
    /// Most recently measured round-trip time (there and back) in ms.
    pub ms: u32,
}

/// A lobby row (`net_status.lobby.rows`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetLobbyRowData {
    /// Row 0 ... 7. In a new lobby the host is in row 0, in a loaded one in the row of its player slot; `peer` names
    /// the machine of the row.
    pub row: u8,
    /// 0 open, 1 ... 4 computer (Easy, Normal, Hard, Manual), 999 human.
    pub kind: i32,
    pub name: String,
    pub nation: i32,
    pub team: i32,
    pub handicap: i32,
    pub color: i32,
    pub ready: bool,
    /// Machine of the row (0 host); `None` without a machine.
    pub peer: Option<u8>,
    /// Player slot 0 ... 7 of the row in a loaded match; `None` in a new lobby.
    pub seat: Option<u8>,
    /// Verdict of the game data check for the row (`equal` for the host row, computers, open rows and without the
    /// check). Old responses without the field read as `equal`.
    #[serde(default)]
    pub data: NetDataState,
}

/// Verdict of the game data check for a lobby row (`net_status.lobby.rows[].data`): `"pending"`, `"equal"`,
/// `"missing"` or `{"differs": {"files": [...], "more": n}}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetDataState {
    /// The machine of the row has not reported for the current session revision yet.
    Pending,
    /// Same rules checksum as the host.
    #[default]
    Equal,
    /// The fixed map is missing on the machine or its map checksum differs.
    Missing,
    /// The rules differ: the first differing paths (at most three, ascending) and the number of the others.
    Differs { files: Vec<String>, more: u16 },
}

/// Lobby state (`net_status.lobby`): for the host its own, for the client the one last received from the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetLobbyData {
    /// Session name.
    pub session: String,
    /// Path of the `.lvl` of a fixed map, otherwise the map name of the session.
    pub map: String,
    /// Stage of the turn raster: `original` (8/4), `lan` (4/2), `lan_fast` (2/1), otherwise `T/V`.
    pub stage: String,
    /// Own row.
    pub own_row: Option<u8>,
    pub rows: Vec<NetLobbyRowData>,
    /// Chat lines of the lobby, fully formatted, oldest first.
    pub chat: Vec<String>,
    /// Largest round-trip time of all clients (state of the host).
    pub max_rtt_ms: Option<u32>,
    /// Name of the network savegame of a loaded session (`savegame/multiplayer/<name>`); otherwise `None`.
    pub saved: Option<String>,
    /// Lobby revision of the last session change (creation or map change); the data verdicts refer to it.
    #[serde(default)]
    pub session_revision: u32,
    /// The host's rules checksum for `session_revision` as 16 hex digits; `None` while it computes, when its
    /// computation failed, and without the game data check (command-line path, loaded lobby).
    #[serde(default)]
    pub rules_checksum: Option<String>,
}

/// A found game of the search (`net_status.found`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetFoundData {
    /// Session ID of the announcement as 16 hex digits (selection in `LAN1`, E-300).
    pub instance: String,
    /// Displayed session name.
    pub name: String,
    /// Address of the host (IP of the response, TCP port of the announcement).
    pub addr: String,
}

/// Response of `net_status` (turn raster, build ID, round-trip times, lobby, search).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetStatusData {
    /// Host or client; `None` during the search.
    pub role: Option<NetRoleName>,
    pub phase: NetPhaseName,
    /// Reason of the halt in phase `halted`.
    pub halt: Option<NetHaltName>,
    /// Slot of the running pause.
    pub paused_by: Option<u8>,
    /// Host: game port it listens on (log line "Network: host listening on ..."); client: port of the host.
    pub port: Option<u16>,
    /// Host: number of humans in the match (`--net-players`, including the host).
    pub players: Option<u8>,
    /// Host: registered machines (host first); client: empty.
    pub peers: Vec<NetPeerData>,
    /// Own slot from the start.
    pub own_slot: Option<u8>,
    /// Slot assignment from the start (eight slots); empty before the start.
    pub seats: Vec<NetSeatData>,
    /// Turn whose bundle is executed next.
    pub round: u32,
    /// Execution tick of this turn.
    pub next_exec_step: u64,
    /// Engine tick of the match (`SimState::steps`); `None` without a loaded match.
    pub steps: Option<u64>,
    /// Wait reason; `None` as long as the machine is computing.
    pub waiting: Option<NetWaitName>,
    /// Turn whose bundle it is waiting for (with `waiting = bundle`).
    pub waiting_turn: Option<u32>,
    /// Host: slots whose contribution to the next turn is missing; client: empty.
    pub missing: Vec<u8>,
    /// Host: number of desyncs of the match; client: `None`.
    pub desyncs: Option<u32>,
    pub links: Vec<NetLinkData>,
    /// `Go` arrived: the match was running.
    pub started: bool,
    /// The network match is over for this machine (message shown or left).
    pub finished: bool,
    /// Reason if the session failed (message text).
    pub failure: Option<String>,
    /// Turn spacing `T` in ticks (session or running match); `None` during the search.
    pub spacing: Option<u8>,
    /// Lead `V` in ticks.
    pub lead: Option<u8>,
    /// Build ID of this program.
    pub build: String,
    /// Host: round-trip time per client in the lobby; otherwise empty.
    pub rtt_ms: Vec<NetRttData>,
    /// Lobby state before the start; `None` afterwards and during the search.
    pub lobby: Option<NetLobbyData>,
    /// Visible games of the search in `LAN1`; otherwise empty.
    pub found: Vec<NetFoundData>,
    /// Successor list: host from its core, client from `Go` or the last `Migrated`;
    /// empty before the start.
    #[serde(default)]
    pub successors: Vec<NetSuccessorData>,
    /// Running host migration of this machine, otherwise `None`.
    #[serde(default)]
    pub migration: Option<NetMigrationData>,
}

/// Entry of the successor list (`net_status.successors`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetSuccessorData {
    pub peer: u8,
    pub slot: u8,
    /// Address under which the client listens; `None` = not reachable from the host (no candidate).
    pub addr: Option<String>,
}

/// Phase of the host migration (`net_status.migration.phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetMigrationPhaseName {
    /// Client: connecting to the candidate.
    Dialing,
    /// Client: has sent its `Rejoin` and waits for `Migrated`.
    Rejoining,
    /// New host: collecting the `Rejoin`s of the clients.
    Gathering,
}

/// State of a running host migration (`net_status.migration`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetMigrationData {
    pub phase: NetMigrationPhaseName,
    /// Dialing/Rejoining: the candidate; otherwise `None`.
    pub peer: Option<u8>,
    /// Gathering: expected clients; otherwise empty.
    #[serde(default)]
    pub expected: Vec<u8>,
    /// Gathering: clients already accepted; otherwise empty.
    #[serde(default)]
    pub arrived: Vec<u8>,
}

/// Checksum of a state part (`net_part_hashes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetPartHash {
    /// Field name in `SimState`.
    pub part: String,
    /// FNV-64 as 16 hex digits like `sim_hash`.
    pub checksum: String,
}

/// Response of `net_part_hashes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetPartHashesData {
    pub steps: u64,
    /// Checksum of the whole state like `sim_hash`.
    pub checksum: String,
    /// One entry per field of `SimState` in declaration order.
    pub parts: Vec<NetPartHash>,
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::{Value, json};

    /// The state of the world display round-trips; missing fields (older or sparse rows) read with their
    /// defaults.
    #[test]
    fn world_display_round_trips_with_defaults() {
        let world = StateSnapshot::example().world_display.unwrap();
        let value = serde_json::to_value(&world).unwrap();
        assert_eq!(
            value["charges"][0],
            json!({"projectile": 4, "position": [300.0, 12.5, 700.0], "yaw": 1.5,
                   "last_digit": 7, "follows": true})
        );
        assert_eq!(
            serde_json::from_value::<WorldDisplayState>(value).unwrap(),
            world
        );
        let empty: WorldDisplayState = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, WorldDisplayState::default());
        assert!(empty.rally_flag.is_none() && empty.beams.is_empty());
        let beam: BeamState = serde_json::from_value(json!({"owner": 9})).unwrap();
        assert_eq!((beam.owner, beam.target, beam.particles), (9, None, 0));
        let flag: RallyFlagState = serde_json::from_value(json!({"building": 5})).unwrap();
        assert_eq!((flag.building, flag.anim_ms), (5, 0.0));
        let mark: MarkCount = serde_json::from_value(json!({"texture": "a.tga"})).unwrap();
        assert_eq!((mark.count, mark.visible), (0, 0));
        let charge: ChargeState = serde_json::from_value(json!({"projectile": 2})).unwrap();
        assert_eq!((charge.last_digit, charge.follows), (0, false));
    }

    /// Response of `random_map` round-trips; field names fixed, checksums as hex text.
    #[test]
    fn random_map_state_round_trip() {
        let state = RandomMapState {
            seed_text: "ZKA".into(),
            hash: 435,
            script: "data/randommapscripts/continental.rnd".into(),
            defines: vec!["MAP_SIZE_MEDIUM".into(), "NORMAL".into(), "MEADOW".into()],
            dim: 128,
            players: vec![[100, 30, 40, 200], [101, 90, 80, 200]],
            rand_counts: [3, 514, 28493, 137322, 8557, 160_264],
            rand_state: 2_063_471_058,
            digests: std::array::from_fn(|i| format!("{:016x}", i + 1)),
            objects: vec![[3500, 8], [4200, 4]],
            slots_used: [true, true, true, true, false, false, false, false],
            start_points: vec![[1000.0, 5.5, 2000.0]],
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["seed_text"], "ZKA");
        assert_eq!(value["players"][1], json!([101, 90, 80, 200]));
        assert_eq!(
            value["rand_counts"],
            json!([3, 514, 28493, 137322, 8557, 160264])
        );
        assert_eq!(value["digests"][0], "0000000000000001");
        assert_eq!(value["digests"][6], "0000000000000007");
        assert_eq!(value["objects"], json!([[3500, 8], [4200, 4]]));
        assert_eq!(value["slots_used"][3], true);
        assert_eq!(value["slots_used"][4], false);
        assert_eq!(value["start_points"], json!([[1000.0, 5.5, 2000.0]]));
        assert_eq!(
            serde_json::from_value::<RandomMapState>(value).unwrap(),
            state
        );
        // Without a random map the viewer answers `null`.
        let none: Option<RandomMapState> = None;
        assert_eq!(serde_json::to_value(none).unwrap(), json!(null));
    }

    #[test]
    fn snapshot_round_trips_with_stable_field_names() {
        let snapshot = StateSnapshot::example();
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["view"], "map");
        assert_eq!(value["clock"]["mode"], "paused");
        assert_eq!(value["window"]["mode"], "windowed");
        assert_eq!(value["toggles"]["aspect"], "correct");
        assert_eq!(value["camera"]["pitch_deg"], 38.5);
        assert_eq!(value["browser"]["pending"]["animation"], 2);
        assert_eq!(value["browser"]["orbit"]["yaw_deg"], 180.0);
        assert_eq!(value["cursor"]["keys_control"][0], "ArrowUp");
        assert_eq!(value["environment"]["light"], "Sommer Nacht");
        assert_eq!(value["sim"]["checksum"], "3a628284f537f62e");
        assert_eq!(value["sim"]["attack_points"][0], 390);
        assert_eq!(value["sim"]["attack_points_active"], 390);
        assert_eq!(value["sim"]["projectiles"][0]["kind"], "arrow");
        assert_eq!(value["sim"]["flights"], 1);
        assert_eq!(value["audio"]["music"]["attack_points"], 390);
        assert_eq!(value["audio"]["music"]["attack_source"], "sim");
        assert_eq!(value["audio"]["recent_sounds"][0]["action"], 5);
        assert_eq!(value["audio"]["recent_sounds"][0]["guid"], 1107);
        assert_eq!(value["sim"]["selection"][0], 0x0001_0001);
        assert_eq!(value["sim"]["target_marker"]["id"], 0x0009_4801);
        assert_eq!(value["sim"]["target_marker"]["remaining_ms"], 150.0);
        assert_eq!(value["sim"]["ground_markers"][1], 0x0009_4801);
        assert_eq!(value["hovered"]["sim_id"], 0x0009_4801);
        assert_eq!(value["environment"]["weather"], "snow");
        assert_eq!(value["shadows"]["kinds"][2], 40);
        assert_eq!(value["hovered"]["base_module"], "Natur");
        assert_eq!(value["browser"]["shadow_kind"], 3);
        assert_eq!(value["browser"]["preview"], "Kielwasser");
        assert_eq!(value["effects"]["surf"], 482);
        assert_eq!(value["audio"]["music"]["status"], "normal");
        assert_eq!(value["audio"]["voices"][0]["gains"][1], 13);
        assert_eq!(value["audio"]["recent_voices"][0]["action"], 12);
        assert_eq!(value["browser"]["sound_action"], 100);
        assert!(value["effect_browser"].is_null());
        assert!(value["free_camera"].is_null());
        // State of the world display.
        let world = &value["world_display"];
        assert_eq!(world["rally_flag"]["building"], 0x0001_0005);
        assert_eq!(world["rally_flag"]["position"][1], 25.0);
        assert_eq!(world["rally_flag"]["anim_ms"], 1500.0);
        assert_eq!(
            world["marks"][0]["texture"],
            "data/effects/textures/fuss.tga"
        );
        assert_eq!(world["marks"][0]["visible"], 3);
        assert_eq!(
            (world["tread_points"].clone(), world["surf_rings"].clone()),
            (json!(12), json!(2))
        );
        assert_eq!(world["stealth"][0], 0x0002_0003);
        assert_eq!(world["beams"][0]["path"], "data/effects/Geist_angriff.pef");
        assert_eq!(world["beams"][0]["particles"], 14);
        assert!(world["beams"][1]["target"].is_null());
        assert_eq!(world["charges"][0]["last_digit"], 7);
        assert_eq!(world["charges"][0]["follows"], true);
        let back: StateSnapshot = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back, snapshot);
        // State snapshots of protocol 1 (without `environment`) stay readable.
        let mut old = value;
        old.as_object_mut().unwrap().remove("environment");
        // Protocol 2: without shadows, class and browser shadows.
        old.as_object_mut().unwrap().remove("shadows");
        for key in ["base_module", "parts", "shadow_kind"] {
            old["hovered"].as_object_mut().unwrap().remove(key);
        }
        for key in ["shadow_kind", "type_guid", "shadows"] {
            old["browser"].as_object_mut().unwrap().remove(key);
        }
        // Protocol 3: without effects.
        for key in ["effects", "effect_browser"] {
            old.as_object_mut().unwrap().remove(key);
        }
        for key in ["damage", "preview", "hit_zones", "effects"] {
            old["browser"].as_object_mut().unwrap().remove(key);
        }
        // Older state snapshots of protocol 9: sound without acknowledgments (default `UnitResponse` on).
        let mut audio = old["audio"].clone();
        for key in ["unit_response", "responses", "select_lock", "recent_voices"] {
            audio.as_object_mut().unwrap().remove(key);
        }
        let audio: AudioState = serde_json::from_value(audio).unwrap();
        assert!(audio.unit_response && audio.responses == 0 && audio.select_lock.is_none());
        assert!(audio.recent_voices.is_empty());
        // Older state snapshots of protocol 10: music and sounds without combat fields.
        let mut audio = old["audio"].clone();
        audio.as_object_mut().unwrap().remove("recent_sounds");
        for key in ["attack_points", "attack_source"] {
            audio["music"].as_object_mut().unwrap().remove(key);
        }
        let audio: AudioState = serde_json::from_value(audio).unwrap();
        assert!(audio.recent_sounds.is_empty());
        assert_eq!(
            (
                audio.music.attack_points,
                audio.music.attack_source.as_str()
            ),
            (0, "sim")
        );
        // Before protocol 38: without the world display.
        old.as_object_mut().unwrap().remove("world_display");
        // Protocol 4: without sound.
        old.as_object_mut().unwrap().remove("audio");
        for key in ["sound_actions", "sound_action"] {
            old["browser"].as_object_mut().unwrap().remove(key);
        }
        // Older state snapshots of protocol 9: without markers.
        for key in ["target_marker", "ground_markers"] {
            old["sim"].as_object_mut().unwrap().remove(key);
        }
        let old: StateSnapshot = serde_json::from_value(old).unwrap();
        let sim = old.sim.as_ref().unwrap();
        assert_eq!((&sim.target_marker, sim.ground_markers.len()), (&None, 0));
        assert_eq!(old.audio, None);
        assert_eq!(old.world_display, None);
        assert_eq!(old.environment, None);
        assert_eq!(old.shadows, None);
        let hovered = old.hovered.unwrap();
        assert_eq!((hovered.base_module, hovered.shadow_kind), (None, 0));
        assert!(hovered.parts.is_empty());
    }

    /// `net_status` with grid, build ID, round-trip times, lobby state and found games round-trips as JSON;
    /// during the search without a role (`null`).
    #[test]
    fn net_status_reports_lobby_and_raster() {
        let lobby = NetStatusData {
            role: Some(NetRoleName::Host),
            phase: NetPhaseName::Lobby,
            halt: None,
            paused_by: None,
            port: Some(2302),
            players: None,
            peers: vec![NetPeerData {
                peer: 0,
                name: "Anna".into(),
            }],
            own_slot: None,
            seats: vec![],
            round: 0,
            next_exec_step: 0,
            steps: None,
            waiting: Some(NetWaitName::Starting),
            waiting_turn: None,
            missing: vec![],
            desyncs: Some(0),
            links: vec![],
            started: false,
            finished: false,
            failure: None,
            spacing: Some(2),
            lead: Some(1),
            build: "0123456789ab-89abcdef".into(),
            rtt_ms: vec![NetRttData { peer: 1, ms: 12 }],
            lobby: Some(NetLobbyData {
                session: "Session - Anna".into(),
                map: "data/levels/multiplayer/m.lvl".into(),
                stage: "lan_fast".into(),
                own_row: Some(0),
                rows: vec![NetLobbyRowData {
                    row: 1,
                    kind: 999,
                    name: "Bert".into(),
                    nation: 2,
                    team: 1,
                    handicap: 0,
                    color: 3,
                    ready: true,
                    peer: Some(1),
                    seat: None,
                    data: NetDataState::Equal,
                }],
                chat: vec!["Anna: Hello".into()],
                max_rtt_ms: Some(12),
                saved: None,
                session_revision: 1,
                rules_checksum: None,
            }),
            found: vec![],
            successors: vec![],
            migration: None,
        };
        let value = serde_json::to_value(&lobby).unwrap();
        assert_eq!(value["role"], "host");
        assert_eq!(value["phase"], "lobby");
        assert_eq!(
            (value["spacing"].clone(), value["lead"].clone()),
            (json!(2), json!(1))
        );
        assert_eq!(value["rtt_ms"], json!([{"peer": 1, "ms": 12}]));
        assert_eq!(value["lobby"]["stage"], "lan_fast");
        assert_eq!(value["lobby"]["rows"][0]["kind"], 999);
        assert_eq!(value["lobby"]["rows"][0]["peer"], 1);
        assert_eq!(value["lobby"]["chat"][0], "Anna: Hello");
        assert_eq!(
            serde_json::from_value::<NetStatusData>(value).unwrap(),
            lobby
        );

        let searching = NetStatusData {
            role: None,
            phase: NetPhaseName::Searching,
            port: None,
            peers: vec![],
            waiting: None,
            desyncs: None,
            spacing: None,
            lead: None,
            rtt_ms: vec![],
            lobby: None,
            found: vec![NetFoundData {
                instance: "00000000deadbeef".into(),
                name: "Session - Anna".into(),
                addr: "127.0.0.1:2302".into(),
            }],
            ..lobby
        };
        let value = serde_json::to_value(&searching).unwrap();
        assert_eq!(value["role"], Value::Null);
        assert_eq!(value["phase"], "searching");
        assert_eq!(value["lobby"], Value::Null);
        assert_eq!(value["found"][0]["instance"], "00000000deadbeef");
        assert_eq!(
            serde_json::from_value::<NetStatusData>(value).unwrap(),
            searching
        );
        for (phase, name) in [
            (NetPhaseName::Searching, "searching"),
            (NetPhaseName::Connecting, "connecting"),
            (NetPhaseName::Lobby, "lobby"),
            (NetPhaseName::Gathering, "gathering"),
        ] {
            assert_eq!(serde_json::to_value(phase).unwrap(), name);
        }
    }

    /// The loaded lobby carries the savegame name (`lobby.saved`) and the player slot per row
    /// (`rows[].seat`); both round-trip as JSON. A lobby state without the fields reads with
    /// `None`.
    #[test]
    fn net_lobby_round_trips_with_saved_and_seats() {
        let row = |row: u8, seat: u8, peer: Option<u8>| NetLobbyRowData {
            row,
            kind: if peer.is_some() { 999 } else { 0 },
            name: if peer == Some(0) {
                "Anna".into()
            } else {
                String::new()
            },
            nation: 1,
            team: 0,
            handicap: 0,
            color: i32::from(seat),
            ready: false,
            peer,
            seat: Some(seat),
            data: NetDataState::Equal,
        };
        let lobby = NetLobbyData {
            session: "Session - Anna".into(),
            map: "net1".into(),
            stage: "lan".into(),
            own_row: Some(1),
            rows: vec![row(0, 1, None), row(1, 3, Some(0))],
            chat: vec![],
            max_rtt_ms: None,
            saved: Some("net1".into()),
            session_revision: 0,
            rules_checksum: None,
        };
        let value = serde_json::to_value(&lobby).unwrap();
        assert_eq!(value["saved"], "net1");
        assert_eq!(value["rows"][0]["seat"], 1);
        assert_eq!(value["rows"][1]["seat"], 3);
        assert_eq!(value["rows"][1]["peer"], 0);
        assert_eq!(
            serde_json::from_value::<NetLobbyData>(value.clone()).unwrap(),
            lobby
        );
        let mut old = value;
        old.as_object_mut().unwrap().remove("saved");
        for r in old["rows"].as_array_mut().unwrap() {
            r.as_object_mut().unwrap().remove("seat");
        }
        let old: NetLobbyData = serde_json::from_value(old).unwrap();
        assert_eq!(old.saved, None);
        assert!(old.rows.iter().all(|r| r.seat.is_none()));
    }

    /// `net_status.lobby` carries the session revision, the host's rules checksum (hex text or `null`) and the data
    /// verdict per row in all four forms; round trip over JSON, old lobby states without the fields read as revision
    /// 0, `null` and `equal`.
    #[test]
    fn net_status_reports_data_states() {
        let row = |row: u8, data: NetDataState| NetLobbyRowData {
            row,
            kind: 999,
            name: format!("P{row}"),
            nation: 0,
            team: 0,
            handicap: 0,
            color: i32::from(row),
            ready: false,
            peer: Some(row),
            seat: None,
            data,
        };
        let mut lobby = NetLobbyData {
            session: "Session - Anna".into(),
            map: "data/levels/multiplayer/m.lvl".into(),
            stage: "original".into(),
            own_row: Some(0),
            rows: vec![
                row(0, NetDataState::Equal),
                row(1, NetDataState::Pending),
                row(2, NetDataState::Missing),
                row(
                    3,
                    NetDataState::Differs {
                        files: vec!["data/a.ini".into(), "data/b.ini".into()],
                        more: 4,
                    },
                ),
            ],
            chat: vec![],
            max_rtt_ms: None,
            saved: None,
            session_revision: 7,
            rules_checksum: Some(crate::hex_checksum(0x0123_4567_89ab_cdef)),
        };
        let value = serde_json::to_value(&lobby).unwrap();
        assert_eq!(value["session_revision"], 7);
        assert_eq!(value["rules_checksum"], "0123456789abcdef");
        assert_eq!(value["rows"][0]["data"], "equal");
        assert_eq!(value["rows"][1]["data"], "pending");
        assert_eq!(value["rows"][2]["data"], "missing");
        assert_eq!(
            value["rows"][3]["data"],
            json!({"differs": {"files": ["data/a.ini", "data/b.ini"], "more": 4}})
        );
        assert_eq!(
            serde_json::from_value::<NetLobbyData>(value.clone()).unwrap(),
            lobby
        );

        lobby.rules_checksum = None;
        let value = serde_json::to_value(&lobby).unwrap();
        assert_eq!(value["rules_checksum"], Value::Null);
        assert_eq!(
            serde_json::from_value::<NetLobbyData>(value.clone()).unwrap(),
            lobby
        );

        let mut old = value;
        for key in ["session_revision", "rules_checksum"] {
            old.as_object_mut().unwrap().remove(key);
        }
        for r in old["rows"].as_array_mut().unwrap() {
            r.as_object_mut().unwrap().remove("data");
        }
        let old: NetLobbyData = serde_json::from_value(old).unwrap();
        assert_eq!((old.session_revision, &old.rules_checksum), (0, &None));
        assert!(old.rows.iter().all(|r| r.data == NetDataState::Equal));
    }

    /// `net_status` carries the successor list and migration; round trip over JSON, old responses without the
    /// fields stay readable.
    #[test]
    fn net_status_lists_successors_and_migration() {
        let base = serde_json::to_value(NetStatusData {
            role: Some(NetRoleName::Client),
            phase: NetPhaseName::Running,
            halt: None,
            paused_by: None,
            port: None,
            players: None,
            peers: vec![],
            own_slot: Some(1),
            seats: vec![],
            round: 3,
            next_exec_step: 6,
            steps: Some(6),
            waiting: None,
            waiting_turn: None,
            missing: vec![],
            desyncs: None,
            links: vec![],
            started: true,
            finished: false,
            failure: None,
            spacing: Some(2),
            lead: Some(1),
            build: "x".into(),
            rtt_ms: vec![],
            lobby: None,
            found: vec![],
            successors: vec![],
            migration: None,
        })
        .unwrap();
        let mut old = base.clone();
        old.as_object_mut().unwrap().remove("successors");
        old.as_object_mut().unwrap().remove("migration");
        let old: NetStatusData = serde_json::from_value(old).unwrap();
        assert!(old.successors.is_empty() && old.migration.is_none());

        let mut status: NetStatusData = serde_json::from_value(base).unwrap();
        status.successors = vec![
            NetSuccessorData {
                peer: 1,
                slot: 1,
                addr: Some("127.0.0.1:2302".into()),
            },
            NetSuccessorData {
                peer: 2,
                slot: 2,
                addr: None,
            },
        ];
        status.migration = Some(NetMigrationData {
            phase: NetMigrationPhaseName::Gathering,
            peer: None,
            expected: vec![2],
            arrived: vec![],
        });
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(
            value["successors"],
            json!([{"peer": 1, "slot": 1, "addr": "127.0.0.1:2302"},
                   {"peer": 2, "slot": 2, "addr": null}])
        );
        assert_eq!(
            value["migration"],
            json!({"phase": "gathering", "peer": null, "expected": [2], "arrived": []})
        );
        assert_eq!(
            serde_json::from_value::<NetStatusData>(value).unwrap(),
            status
        );
        for (phase, name) in [
            (NetMigrationPhaseName::Dialing, "dialing"),
            (NetMigrationPhaseName::Rejoining, "rejoining"),
            (NetMigrationPhaseName::Gathering, "gathering"),
        ] {
            assert_eq!(serde_json::to_value(phase).unwrap(), name);
        }
    }
}
