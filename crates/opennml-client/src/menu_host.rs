//! The host of the menu UI: drives [`MenuApp`] with input from the window, carries out its effects (sound, music,
//! settings file) and passes on what only the window can do (display mode, quitting, hardware pointer) as
//! [`HostRequests`]. GPU-free: the host delivers draw commands, drawing happens with the layer (`UiLayer::draw`) in
//! the window scaffolding of the program.
//!
//! One frame of the host: input ([`MenuHost::on_input`], [`MenuHost::on_key`]) -> [`MenuHost::frame_cmds`] (keyboard
//! -> mouse -> frame event -> pass -> drawing -> one command, `MenuApp::frame`) -> [`MenuHost::apply_effects`]
//! (afterwards the host tells the UI whether the speech sample is still playing); the pointer
//! ([`MenuHost::pointer_draw`]) is added last when drawing.
//!
//! **Mouse pointer** (re/menu-system 12.9, re/game-ui D5.2): The host loads all 12 pointers from `mousepointer.ini`
//! in three variants (normal, red, green; the first file of each, hotspot `OFFSET`) and the 3D model `mouse.rdo`
//! ([`PointerModel`], re/pointer-3d). It draws the chosen pointer ([`MenuHost::set_pointer`], default 7/0) last
//! ([`MenuHost::pointer_draw`]): with `HardwareCursor 0` the animated, tinted model, with `HardwareCursor 1` the TGA
//! of the variant, pixel 1:1, white. With `HardwareCursor 0` the TGA applies only as a fallback if `mouse.rdo` is not
//! loadable (warning when loading). If an image is missing, pointer 7 variant 0 (deviation from the original). As long
//! as the UI has the pointer off (start sequence, campaign video, campaign cinemascope) or the mouse is outside the
//! window, there is no pointer; the loop of the model keeps running. The end of a video restarts the loop of 7.
//!
//! **Videos** (re/video, sections "Opening" and "After the video"): On `UiEffect::PlayVideo` the host opens the file
//! via its [`VideoOpener`] (default `video::open_bink`, the `.bik` of the installation), stores frame 0 under
//! `VIDEO_TEXTURE` and reports the video with `MenuApp::video_started`; sound as in `FUN_0045D560`. Before every frame
//! it stores the due video frame if it differs from the one shown. On `UiEffect::StopVideo` it closes file and sound
//! (`FUN_0045CA70`).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Result;
use opennml_audio::decode::PcmStream;
use opennml_audio::engine::{AudioEngine, SPEECH_FLAGS, VIDEO_SPEECH_FLAGS, VIDEO_TRACK_FLAGS};
use opennml_audio::manager::SoundId;
use opennml_audio::volume::Category;
use opennml_control::UiPointerModelData;
use opennml_formats::dlg::desc::{WidgetDesc, WidgetKind};
use opennml_formats::engine_ini::EngineConfig;
use opennml_formats::gifcon::{MOUSE_POINTER_PATH, POINTER_NORMAL, parse_mouse_pointers};
use opennml_formats::text::decode_cp1252;
use opennml_rda::{Vfs, normalize_path};
use opennml_render::pointer::PointerScene;
use opennml_ui::app::{MenuApp, MenuOptions};
use opennml_ui::coords::Screen;
use opennml_ui::dialog::DialogLibrary;
use opennml_ui::draw::{Blend, DrawCmd, Filter, TextureId};
use opennml_ui::input::{MouseButton as UiButton, UiInput};
use opennml_ui::msg::{DisplayMode, UiEffect};
use opennml_ui::version_line::{VERSION_TEXT, draw_version_line};
use opennml_ui::video::{VIDEO_TEXTURE, speech_path};
use winit::keyboard::KeyCode;
use winit::window::Window;

use crate::game::pointer::{POINTER_COUNT, PointerChoice};
use crate::keys::{InputEvent, MouseButton};
use crate::pointer_model::{MOUSE_MODEL_PATH, PointerModel, PointerModelData};
use crate::save_files::FsSaveFiles;
use crate::settings::{load_profiles, profiles_path, save_profiles, save_settings, savegame_dir};
use crate::ui_layer::UiLayer;
use crate::video::{VideoFile, VideoOpener, open_bink};
use crate::vk::{HeldVk, MOUSE_LEFT, MOUSE_MIDDLE, MOUSE_RIGHT};

/// Sound of the menu UI: the calls its effects need. [`AudioEngine`] implements them; tests use a fake.
pub trait MenuAudio {
    /// `SoundManager_PlayEvent2D` (click, sample sound).
    fn play_event_2d(
        &mut self,
        vfs: &Vfs,
        name: &str,
        flags: u32,
        category: Category,
        looping: bool,
    );
    /// Speech sample: `PlayMemoryStream(path, 0x29, cat. 2)` (`SOUNDOPTIONS`, `0x004D1C31`...`0x004D1C8A`); the host
    /// afterwards queries it by ID with [`Self::speech_playing`].
    fn play_speech(&mut self, vfs: &Vfs, path: &str) -> Option<SoundId>;
    /// Menu music of a `FRAME` (`0x160`).
    fn play_menu_music_looping(&mut self, vfs: &Vfs, path: &str, looping: bool);
    /// Restart the menu music (`0x161`, `MenuMusic_Restart` `0x0045CA20`).
    fn restart_menu_music(&mut self, vfs: &Vfs);
    /// `MusicManager_ResumeMusic` (`0x004E6860`): resume the game music; after a video with a running menu level
    /// instead of [`Self::restart_menu_music`].
    fn resume_game_music(&mut self, vfs: &Vfs);
    /// Volume of a category (0...1000).
    fn set_volume(&mut self, category: Category, value: i32);
    /// Recreate the sound device with the driver `index` (`m_DriverID`) (`sounddriver`,
    /// [`AudioEngine::set_sound_driver`]).
    fn set_sound_driver(&mut self, index: i32);
    /// Display names of the driver list (`MenuOptions.sound_drivers`; empty without a device).
    fn sound_drivers(&self) -> Vec<String>;
    /// Is the speech sample `id` still playing (`IsSoundPlaying`)?
    fn speech_playing(&self, id: SoundId) -> bool;
    /// Length of a music file in ms (synchronous, for the credits).
    fn music_length_ms(&self, vfs: &Vfs, path: &str) -> Option<u32>;
    /// `MenuMusic_Stop` (`0x0045C9F0`, 1000 ms) when a video starts (`FUN_0045D560`).
    fn stop_menu_music(&mut self);
    /// Speech track of a video: `PlayMemoryStream(path, 0x19, cat. 3)` and `FadeInSound(channel, 1000, 3)`
    /// (`FUN_0045D560` `0x0045D7C0`...`0x0045D7EB`). If the file is missing, `None` without a warning (the original
    /// gets -1 and plays only the Bink sound).
    fn play_video_speech(&mut self, vfs: &Vfs, path: &str) -> Option<SoundId>;
    /// Bink sound track of a video as a stream (`VIDEO_TRACK_FLAGS`, fixed volume, as in the original, O-69).
    fn play_video_track(&mut self, stream: Box<dyn PcmStream>, label: &str) -> Option<SoundId>;
    /// `SoundManager_StopSound`: immediately, without fade.
    fn stop_sound(&mut self, id: SoundId);
    /// `SoundManager_FadeOutSound(id, ms, 1)`: fade out, then release.
    fn fade_out_sound(&mut self, id: SoundId, ms: i32);
}

impl MenuAudio for AudioEngine {
    fn play_event_2d(
        &mut self,
        vfs: &Vfs,
        name: &str,
        flags: u32,
        category: Category,
        looping: bool,
    ) {
        self.sound
            .play_event_2d(vfs, name, flags, category, looping);
    }

    fn play_speech(&mut self, vfs: &Vfs, path: &str) -> Option<SoundId> {
        // Missing speech files are common in the shipped data (`24021 .ogg`): silent, without a warning per line (as
        // with `play_video_speech`).
        if !vfs.exists(path) {
            return None;
        }
        self.sound
            .play_memory_stream(vfs, path, SPEECH_FLAGS, Category::Speech)
            .map(|(id, _)| id)
    }

    fn play_menu_music_looping(&mut self, vfs: &Vfs, path: &str, looping: bool) {
        AudioEngine::play_menu_music_looping(self, vfs, path, looping);
    }

    fn restart_menu_music(&mut self, vfs: &Vfs) {
        AudioEngine::restart_menu_music(self, vfs);
    }

    fn resume_game_music(&mut self, vfs: &Vfs) {
        AudioEngine::resume_game_music(self, vfs);
    }

    fn set_volume(&mut self, category: Category, value: i32) {
        AudioEngine::set_volume(self, category, value);
    }

    fn set_sound_driver(&mut self, index: i32) {
        AudioEngine::set_sound_driver(self, index);
    }

    fn sound_drivers(&self) -> Vec<String> {
        AudioEngine::sound_drivers(self)
    }

    fn speech_playing(&self, id: SoundId) -> bool {
        self.sound.is_playing(id)
    }

    fn music_length_ms(&self, vfs: &Vfs, path: &str) -> Option<u32> {
        AudioEngine::music_length_ms(vfs, path)
    }

    fn stop_menu_music(&mut self) {
        AudioEngine::stop_menu_music(self);
    }

    fn play_video_speech(&mut self, vfs: &Vfs, path: &str) -> Option<SoundId> {
        // `play_memory_stream` warns about a missing file; most videos have no speech track.
        if !vfs.exists(path) {
            return None;
        }
        let (id, _) =
            self.sound
                .play_memory_stream(vfs, path, VIDEO_SPEECH_FLAGS, Category::Fixed)?;
        self.sound.fade_in(id, VIDEO_FADE_MS, Category::Fixed);
        Some(id)
    }

    fn play_video_track(&mut self, stream: Box<dyn PcmStream>, label: &str) -> Option<SoundId> {
        self.sound
            .play_pcm_stream(stream, label, VIDEO_TRACK_FLAGS, Category::Fixed)
            .map(|(id, _)| id)
    }

    fn stop_sound(&mut self, id: SoundId) {
        self.sound.stop(id);
    }

    fn fade_out_sound(&mut self, id: SoundId, ms: i32) {
        self.sound.fade_out(id, ms, true);
    }
}

/// Fade-in and fade-out time of the speech track of a video (`FUN_0045D560` `FadeInSound(..., 1000, 3)`,
/// `FUN_0045CA70` `FadeOutSound(..., 1000, 1)`).
const VIDEO_FADE_MS: i32 = 1000;

/// Length of the window in which a `RestartMusic` counts as "after the video" (frames, see `MenuHost::after_video`).
const AFTER_VIDEO_FRAMES: u8 = 4;

/// The video the host has open (for playback in `MenuApp::video`).
struct HostVideo {
    /// Path as in the command.
    path: String,
    file: Box<dyn VideoFile>,
    /// Frame under `VIDEO_TEXTURE`.
    shown: u32,
    /// Bink sound track in the mixer.
    track: Option<SoundId>,
    /// Speech track (root `+0xD4`).
    speech: Option<SoundId>,
}

/// What the UI wants from the window (from the effects of a frame, the last one applies).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostRequests {
    /// `ACCEPT` in `VIDEOOPTIONS`: change the window size or the fullscreen mode (deviation from the original); the
    /// fonts stay (O-58).
    pub display_mode: Option<DisplayMode>,
    /// Switch "hardware mouse" (`HardwareCursor`); the reimplementation draws the pointer itself in both cases.
    pub hardware_cursor: Option<bool>,
    /// Gamma slider in `VIDEOOPTIONS` (code 1): set the gamma ramp immediately (`Renderer_SetGammaRamp(G, G, G)`,
    /// O-99).
    pub gamma: Option<f32>,
    /// `0x25`: quit the program.
    pub quit: bool,
    /// The game effects in the order they arise (`StartMatch`, `LoadGame`, `SaveGame`, `DeleteSave`, `ReloadLevel`,
    /// `ResumeGame`, `EndMatch`, `Surrender` and `StartCampaignLevel`, `Net`, `Search`, `Lobby` for the network
    /// session): in `opennml-game` to `game_flow::GameFlow::apply_all`, in the viewer into the log
    /// ([`log_game_effect`]).
    pub game: Vec<UiEffect>,
}

/// A game effect without match flow (viewer): log only.
pub fn log_game_effect(effect: &UiEffect) {
    match effect {
        UiEffect::StartMatch(req) => log::info!(
            "Match start (command 0xd) has no effect in the viewer: {} (game type {}, {} slots)",
            req.level,
            req.game_type,
            req.slots.len()
        ),
        other => log::info!("Game effect has no effect in the viewer: {other:?}"),
    }
}

/// A loaded pointer image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PointerSprite {
    texture: TextureId,
    /// Size in pixels.
    size: [i32; 2],
    /// `OFFSET`: the pixel on the mouse position.
    hotspot: [i32; 2],
}

/// The pointers from `mousepointer.ini` (12 IDs x 3 variants), the chosen pointer and the position of the mouse in
/// the window.
#[derive(Debug, Clone, Default)]
struct Pointer {
    /// Image per (ID, variant 0 normal, 1 red, 2 green); missing if the row or the file is missing.
    sprites: BTreeMap<(i32, u8), PointerSprite>,
    /// The chosen pointer ([`MenuHost::set_pointer`]); default 7/0.
    choice: PointerChoice,
    /// ID and variant for which a warning is already in the log (once per ID and variant).
    warned: BTreeSet<(i32, u8)>,
    /// Mouse position in window pixels; `None` outside the window.
    at: Option<(i32, i32)>,
}

impl Pointer {
    /// All pointers from `mousepointer.ini` (`Cursor3D_Load` `0x004CDC40` -> `FUN_004CD790`): per ID 0...11 and variant
    /// the first file (`NORMAL`, `RED`, `GREEN`; the hardware pointer `0x004CD4C0`/`0x004CD5D0` takes only element 0),
    /// hotspot `OFFSET` of the ID. What is missing is reported only by the choice ([`Self::set`]).
    fn load(vfs: &Vfs, layer: &mut UiLayer) -> Pointer {
        let pointers = match vfs.read(MOUSE_POINTER_PATH) {
            Ok(bytes) => parse_mouse_pointers(&decode_cp1252(&bytes)),
            Err(e) => {
                log::warn!("Mouse pointer \"{MOUSE_POINTER_PATH}\" not readable: {e}");
                return Pointer::default();
            }
        };
        let mut sprites = BTreeMap::new();
        for (&id, def) in pointers.range(0..POINTER_COUNT) {
            for (variant, files) in [&def.normal, &def.red, &def.green].into_iter().enumerate() {
                let Some(file) = files.first() else {
                    continue;
                };
                let Some(texture) = layer.assets.texture(vfs, file) else {
                    log::warn!("Mouse pointer \"{file}\" not loadable");
                    continue;
                };
                let (w, h) = layer.assets.image(texture).dimensions();
                let size = [
                    i32::try_from(w).unwrap_or(i32::MAX),
                    i32::try_from(h).unwrap_or(i32::MAX),
                ];
                let sprite = PointerSprite {
                    texture,
                    size,
                    hotspot: def.offset,
                };
                sprites.insert((id, variant as u8), sprite);
            }
        }
        Pointer {
            sprites,
            ..Pointer::default()
        }
    }

    /// Choose a pointer (`SetPointer` `0x004CD4C0` and `SetVariant` `0x004CD5D0` in one). If the ID and variant do not
    /// exist (ID outside 0...11, variant outside 0...2, row or file missing), [`Self::quad`] draws pointer 7 variant 0
    /// with a warning in the log, once per ID and variant (deviation from the original, O-298; with the hardware
    /// pointer the original threw an exception or left the old pointer in place).
    fn set(&mut self, choice: PointerChoice) {
        self.choice = choice;
        let key = (choice.id, choice.variant);
        if !self.sprites.contains_key(&key) && self.warned.insert(key) {
            log::warn!(
                "Mouse pointer {} variant {} is missing: pointer {POINTER_NORMAL} variant 0 (deviation from the original)",
                choice.id,
                choice.variant
            );
        }
    }

    /// Image of the chosen pointer, otherwise pointer 7 variant 0.
    fn sprite(&self) -> Option<PointerSprite> {
        let key = (self.choice.id, self.choice.variant);
        let normal = (PointerChoice::NORMAL.id, PointerChoice::NORMAL.variant);
        self.sprites
            .get(&key)
            .or_else(|| self.sprites.get(&normal))
            .copied()
    }

    /// Quad of the pointer: pixel `OFFSET` on the mouse position, 1:1, white, point filter.
    fn quad(&self) -> Option<DrawCmd> {
        let PointerSprite {
            texture,
            size: [w, h],
            hotspot: [hx, hy],
        } = self.sprite()?;
        let (x, y) = self.at?;
        let (x0, y0) = (x.saturating_sub(hx), y.saturating_sub(hy));
        Some(DrawCmd::Quad {
            texture: Some(texture),
            dst: [x0, y0, x0.saturating_add(w), y0.saturating_add(h)],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: 0xFFFF_FFFF,
            filter: Filter::Point,
            blend: Blend::Alpha,
        })
    }
}

/// What the host draws as the pointer ([`MenuHost::pointer_draw`]): the TGA quad (`HardwareCursor 1` and the fallback
/// without a model) or the 3D pointer (`HardwareCursor 0`).
#[derive(Debug, Clone)]
pub enum PointerDraw {
    Sprite(DrawCmd),
    Model(PointerScene),
}

/// All `MUSIC` paths of the frames of a library (as read).
fn music_paths(library: &DialogLibrary) -> Vec<String> {
    fn walk(w: &WidgetDesc, out: &mut Vec<String>) {
        if let WidgetKind::Frame(f) = &w.kind
            && let Some(path) = &f.music
        {
            out.push(path.clone());
        }
        for child in &w.children {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    for name in library.names() {
        if let Some(expanded) = library.expanded(&name) {
            for w in &expanded.widgets {
                walk(w, &mut out);
            }
        }
    }
    out
}

/// Category of the effect (0 music, 1 sound effects, 2 speech).
fn category(n: u8) -> Option<Category> {
    match n {
        0 => Some(Category::Music),
        1 => Some(Category::Sfx),
        2 => Some(Category::Speech),
        _ => None,
    }
}

/// Builds the UI on the texts of `layer`. Without a supplier in `opts.music_length` the host now queries the length of
/// each `MUSIC` file of the dialogs via `audio` (`Broadcast::MusicLength` queries synchronously when the frame is
/// built, before the music effect runs) - before the first frame, i.e. before any frame is built.
fn build_app(
    vfs: &Vfs,
    layer: &UiLayer,
    config: EngineConfig,
    mut opts: MenuOptions,
    audio: &dyn MenuAudio,
) -> Result<MenuApp> {
    let lengths: Rc<RefCell<BTreeMap<String, Option<u32>>>> = Rc::default();
    let fill = opts.music_length.is_none();
    if fill {
        let known = lengths.clone();
        opts.music_length = Some(Box::new(move |path| {
            known.borrow().get(&normalize_path(path)).copied().flatten()
        }));
    }
    let app = MenuApp::new(vfs, &layer.texts, config, opts).map_err(anyhow::Error::msg)?;
    if fill {
        let mut known = lengths.borrow_mut();
        for path in music_paths(app.menu().library()) {
            known
                .entry(normalize_path(&path))
                .or_insert_with(|| audio.music_length_ms(vfs, &path));
        }
    }
    Ok(app)
}

/// The menu host.
pub struct MenuHost {
    pub app: MenuApp,
    pub layer: UiLayer,
    pointer: Pointer,
    /// The 3D pointer (`HardwareCursor 0`); `None` if `mouse.rdo` is not loadable (fallback to the TGA pointers).
    model: Option<PointerModel>,
    held: HeldVk,
    /// Settings file (checked with `settings::settings_path_for`); `None`: settings only in memory, nothing is ever
    /// written (viewer without `--settings`). `profiles.ini` and `savegame/` lie next to it (deviating from the
    /// original); without a settings file the profiles also stay in memory.
    settings: Option<PathBuf>,
    /// ID of the last speech sample.
    speech: Option<SoundId>,
    /// Opens the videos (default `video::open_bink`).
    opener: VideoOpener,
    /// The open video.
    video: Option<HostVideo>,
    /// Frames in which a `RestartMusic` still counts as "after the video": `close_video` sets [`AFTER_VIDEO_FRAMES`],
    /// every frame counts down. The first `RestartMusic` in the window consumes it (`0`), a later one calls
    /// `MenuMusic_Restart` again. The `AfterVideo` of the UI comes two frames after the end of the video.
    after_video: u8,
    /// Time of the last frame ([`Self::frame_cmds`]); a video starts at this time.
    now: u64,
    /// Folder `savegame/` next to the settings file (deviating from the original); none without a settings file.
    save_files: Option<FsSaveFiles>,
    /// Value of `IsMenuLevel` in the settings file as long as the run has set it to off only in memory (`--host`,
    /// `--join`): [`Self::write_settings`] writes it back as long as it is off.
    saved_menu_level: Option<bool>,
}

impl MenuHost {
    /// Builds the UI on `layer` with the settings `config` (`settings::load_settings`) and loads pointer 7. Music
    /// lengths as in `build_app`; `opts.display_modes` are the raw modes ([`Self::raw_display_modes`]). `settings` is
    /// a location checked with `settings::settings_path_for` (never in the installation); `None` keeps the settings
    /// only in memory (`WriteSettings` and [`Self::shutdown`] then write nothing). With a settings file the host reads
    /// the profiles from `profiles.ini` next to it (`settings::load_profiles`, replaces `opts.profiles`; missing or
    /// broken: no profiles, message in the log); without one `opts.profiles` applies in memory. The savegames of the
    /// menus then come from `savegame/` next to it ([`FsSaveFiles`], replaces a missing `opts.saves`); without a
    /// settings file `opts.saves` applies, or no storage.
    pub fn new(
        vfs: &Vfs,
        mut layer: UiLayer,
        config: EngineConfig,
        mut opts: MenuOptions,
        audio: &dyn MenuAudio,
        settings: Option<PathBuf>,
    ) -> Result<Self> {
        let save_files = settings.as_deref().map(|path| FsSaveFiles {
            dir: savegame_dir(path),
        });
        if let Some(path) = &settings {
            opts.profiles = load_profiles(&profiles_path(path));
        }
        if let (None, Some(files)) = (&opts.saves, &save_files) {
            opts.saves = Some(Rc::new(files.clone()));
        }
        let app = build_app(vfs, &layer, config, opts, audio)?;
        let pointer = Pointer::load(vfs, &mut layer);
        // The loop of pointer 7 starts only in the first drawn frame; the load time therefore has no effect.
        let model = match PointerModel::load(vfs, 0.0) {
            Ok(model) => Some(model),
            Err(e) => {
                log::warn!(
                    "3D mouse pointer \"{MOUSE_MODEL_PATH}\" not loadable: {e:#}; the TGA pointers apply"
                );
                None
            }
        };
        Ok(MenuHost {
            app,
            layer,
            pointer,
            model,
            held: HeldVk::default(),
            settings,
            speech: None,
            opener: Box::new(open_bink),
            video: None,
            after_video: 0,
            now: 0,
            save_files,
            saved_menu_level: None,
        })
    }

    /// Remembers the value of `IsMenuLevel` from the settings file that the run has set to off only in memory
    /// (command-line paths without a menu level): the settings file stays unchanged as long as the checkbox stays off
    /// in the run; if the user turns it on in the options, the user's value applies.
    pub fn keep_saved_menu_level(&mut self, saved: bool) {
        self.saved_menu_level = Some(saved);
    }

    /// The savegames in the folder `savegame/` (with a settings file).
    pub fn save_files(&self) -> Option<&FsSaveFiles> {
        self.save_files.as_ref()
    }

    /// Replaces the opener of the videos (tests use a fake).
    pub fn set_video_opener(&mut self, opener: VideoOpener) {
        self.opener = opener;
    }

    /// New UI with the current settings on the same layer (images, fonts, texts, pointers stay): the dialog browser of
    /// the viewer reopens a dialog this way with Ctrl+Page Up/Down. Held keys and the mouse position stay, as do the
    /// profiles (they replace `opts.profiles`).
    pub fn restart(
        &mut self,
        vfs: &Vfs,
        mut opts: MenuOptions,
        audio: &dyn MenuAudio,
    ) -> Result<()> {
        opts.profiles = self.app.profiles().file.clone();
        if let (None, Some(files)) = (&opts.saves, &self.save_files) {
            opts.saves = Some(Rc::new(files.clone()));
        }
        self.app = build_app(vfs, &self.layer, self.app.config().clone(), opts, audio)?;
        self.speech = None;
        // An open video including its sound is closed by the next execution of the effects (the new UI has none).
        Ok(())
    }

    /// The TGA quad of the chosen pointer ([`Self::set_pointer`]) at the mouse position (last quad of a frame in the
    /// window); `None` without a mouse position or without a loadable pointer image. It applies to `HardwareCursor 1`
    /// and in the fallback without a model; which pointer is drawn in a frame is chosen by [`Self::pointer_draw`]. The
    /// commands of a frame ([`Self::frame_cmds`]) do not contain it: the host appends it when drawing at the current
    /// mouse position, screenshots leave it out (as with the hardware pointer). As long as the UI has the pointer off
    /// (`MenuApp::pointer_hidden`, `FUN_004CD6A0(0)`), there is none.
    pub fn pointer_quad(&self) -> Option<DrawCmd> {
        if self.app.pointer_hidden() {
            return None;
        }
        self.pointer.quad()
    }

    /// Choose a pointer: `opennml-game` sets the pointer of the match or 7/0 for menus and dialogs every frame
    /// (`GameFlow::pointer_choice`). The choice goes to the TGA (variant normal, red, green; if the image is missing,
    /// pointer 7 variant 0 with a warning per ID and variant, deviation from the original, O-298) and to the 3D
    /// pointer ([`PointerModel::set`] in the branch of `HardwareCursor`, live from the settings): with software, tint
    /// and a restart of the loop for a new ID, with hardware only the ID (O-462). Because this happens every frame,
    /// the same ID never restarts, not even with another variant or tint.
    pub fn set_pointer(&mut self, choice: PointerChoice) {
        self.pointer.set(choice);
        let software = !self.app.config().hardware_cursor;
        if let Some(model) = &mut self.model {
            model.set(choice, software);
        }
    }

    /// The chosen pointer (before the fallback to 7/0).
    pub fn pointer_choice(&self) -> PointerChoice {
        self.pointer.choice
    }

    /// Software pointer with model (`HardwareCursor 0` live from the settings and `mouse.rdo` loaded).
    fn model_pointer(&self) -> bool {
        !self.app.config().hardware_cursor && self.model.is_some()
    }

    /// Pointer of a drawn frame at program clock `now_ms` on the screen `size`: with `HardwareCursor 0` and a model
    /// the 3D pointer ([`PointerModel::frame`]: an open restart of the loop becomes fixed here), otherwise the TGA
    /// quad like [`Self::pointer_quad`]. `None` without a mouse position in the window or as long as the UI has the
    /// pointer off (`MenuApp::pointer_hidden`); the phase keeps running then.
    pub fn pointer_draw(&mut self, now_ms: f32, size: (u32, u32)) -> Option<PointerDraw> {
        if self.app.pointer_hidden() {
            return None;
        }
        let at = self.pointer.at?;
        if self.model_pointer()
            && let Some(model) = &mut self.model
        {
            return Some(PointerDraw::Model(model.frame(now_ms, at, size)));
        }
        self.pointer.quad().map(PointerDraw::Sprite)
    }

    /// Like [`Self::pointer_draw`] for queries (screenshot, `ui_state`): does not fix the start of a loop
    /// ([`PointerModel::preview`]).
    pub fn pointer_preview(&self, now_ms: f32, size: (u32, u32)) -> Option<PointerDraw> {
        if self.app.pointer_hidden() {
            return None;
        }
        let at = self.pointer.at?;
        if self.model_pointer()
            && let Some(model) = &self.model
        {
            return Some(PointerDraw::Model(model.preview(now_ms, at, size)));
        }
        self.pointer.quad().map(PointerDraw::Sprite)
    }

    /// State of the 3D pointer for queries; `None` without a model (fallback).
    pub fn pointer_model_data(&self, now_ms: f32) -> Option<PointerModelData> {
        self.model.as_ref().map(|m| m.data(now_ms))
    }

    /// The 3D pointer for `ui_state`: [`Self::pointer_model_data`] with the setting (`software` = `HardwareCursor 0`);
    /// read-only, fixes no restart. `None` without a model.
    pub fn pointer_model_report(&self, now_ms: f32) -> Option<UiPointerModelData> {
        let data = self.pointer_model_data(now_ms)?;
        Some(UiPointerModelData {
            id: data.id,
            animation: data.animation,
            tint: data.tint,
            phase_ms: data.phase_ms,
            software: !self.app.config().hardware_cursor,
        })
    }

    /// Raw display modes of the monitor the window is on (winit, order of the system); `VIDEOOPTIONS` filters them
    /// like the original (`opennml_ui::screens::filter_display_modes`).
    pub fn raw_display_modes(window: &Window) -> Vec<DisplayMode> {
        let Some(monitor) = window.current_monitor() else {
            return Vec::new();
        };
        monitor
            .video_modes()
            .map(|m| DisplayMode {
                width: i32::try_from(m.size().width).unwrap_or(i32::MAX),
                height: i32::try_from(m.size().height).unwrap_or(i32::MAX),
                bits: i32::from(m.bit_depth()),
            })
            .collect()
    }

    /// Location of the settings file; `None`: memory only.
    pub fn settings_path(&self) -> Option<&Path> {
        self.settings.as_deref()
    }

    /// Window input to the UI (buffered for the next frame): movement only when the position changed, press,
    /// release, wheel; keys without a character via [`Self::on_key`]. `MenuApp::input` detects the double click.
    pub fn on_input(&mut self, ev: &InputEvent, screen: Screen, now: u64) {
        let button = |b: &MouseButton| match b {
            MouseButton::Left => Some(UiButton::Left),
            MouseButton::Right => Some(UiButton::Right),
            // The middle button is no widget event (`FUN_004B8E70`), only VK 4 of the table (quirk O-331).
            MouseButton::Middle => None,
        };
        match ev {
            InputEvent::CursorMoved { x, y } => {
                // `as` saturates; fractions are truncated like the integer window coordinates of the original.
                let at = (x.floor() as i32, y.floor() as i32);
                if self.pointer.at != Some(at) {
                    self.pointer.at = Some(at);
                    self.app
                        .input(UiInput::Move { x: at.0, y: at.1 }, screen, now);
                }
            }
            InputEvent::CursorLeft => self.pointer.at = None,
            InputEvent::Button { button: b, pressed } => {
                let bit = match b {
                    MouseButton::Left => MOUSE_LEFT,
                    MouseButton::Right => MOUSE_RIGHT,
                    MouseButton::Middle => MOUSE_MIDDLE,
                };
                self.held.set_mouse(bit, *pressed);
                if let Some(b) = button(b) {
                    let input = if *pressed {
                        UiInput::Down(b)
                    } else {
                        UiInput::Up(b)
                    };
                    self.app.input(input, screen, now);
                }
            }
            InputEvent::Wheel { up } => {
                let input = if *up {
                    UiInput::WheelUp
                } else {
                    UiInput::WheelDown
                };
                self.app.input(input, screen, now);
            }
            InputEvent::Key { code, pressed, .. } => {
                self.on_key(*code, *pressed, None, screen, now)
            }
        }
    }

    /// Key with a character (`ch` from `vk::char_of(KeyEvent::text)`): pressing (also auto-repeat) enters the VK and
    /// sends a key event with the whole table and the character (`FUN_00414B70`, re/menu-system 12.7); releasing only
    /// notes the physical state, the table follows at the start of the next round ([`HeldVk::begin_round`] at the end
    /// of [`Self::frame_cmds`]). A key without a VK in the reimplementation does not count as held; without a
    /// character it sends no event (the original sends every `WM_KEYDOWN`, where every key has a VK). Exception Print
    /// key: its release without a prior press is a key event as in the original.
    pub fn on_key(
        &mut self,
        code: KeyCode,
        pressed: bool,
        ch: Option<u16>,
        screen: Screen,
        now: u64,
    ) {
        if !pressed {
            // Windows reports the Print key only on release (`WM_KEYUP`); the original sends a key event for it
            // (`FUN_0042C1D0` `0x0042C357` -> `FUN_00414B70`), otherwise there would be no screenshot (`KD_HARDCOPY`).
            // If the press did arrive (other systems), its event was already there.
            if code == KeyCode::PrintScreen
                && !self.held.is_down(code)
                && self.held.press(code).is_some()
            {
                self.app.input(
                    UiInput::Key {
                        held: self.held.held(),
                        ch: None,
                    },
                    screen,
                    now,
                );
            }
            self.held.release(code);
            return;
        }
        if self.held.press(code).is_none() && ch.is_none() {
            return;
        }
        self.app.input(
            UiInput::Key {
                held: self.held.held(),
                ch,
            },
            screen,
            now,
        );
    }

    /// The table of held keys including mouse buttons ([`HeldVk::held`]) for the `KD_` queries of the camera
    /// (`GameSession::held_vk`).
    pub fn held_vk(&self) -> Vec<u8> {
        self.held.held()
    }

    /// Does an input field have keyboard focus (chat window open)? The match then locks the camera keys
    /// (`GameSession::text_focus`, O-393).
    pub fn text_focus(&self) -> bool {
        self.app.text_focus()
    }

    /// Focus loss: release all keys.
    pub fn release_keys(&mut self) {
        self.held.clear();
    }

    /// Draw commands of a frame on `screen` (`MenuApp::frame`), without the pointer ([`Self::pointer_quad`]).
    /// Beforehand the host stores the due video frame.
    pub fn frame_cmds(&mut self, vfs: &Vfs, screen: Screen, now: u64) -> Vec<DrawCmd> {
        self.now = now;
        self.show_video_frame(now);
        let mut ctx = self.layer.context(vfs, screen);
        let cmds = self.app.frame(&mut ctx, now);
        // The next round of the window loop begins: reconcile released keys and the mouse buttons
        // (`FUN_00414830` before the messages of the round, O-116).
        self.held.begin_round();
        cmds
    }

    /// Version line of this frame (`opennml_ui::version_line`): `GameApp_RenderUiAndPresent` draws it only with
    /// `(GameApp+0xD5E0 & 0x41) == 0` (`0x004C0756`, O-440), i.e. neither with a visible world (`world_shown`: running
    /// match or visible menu level, bit `0x1`) nor during a running video (bit `0x40`). The host draws it last, above
    /// the pointer (second scene after `Cursor3D_Render`, re/game-ui 1.2).
    pub fn version_cmds(&self, screen: Screen, world_shown: bool) -> Vec<DrawCmd> {
        let mut out = Vec::new();
        if !world_shown && self.app.video().is_none() {
            draw_version_line(VERSION_TEXT, screen, &self.layer.fonts, &mut out);
        }
        out
    }

    /// Frame step of the host (instead of `FUN_0045CB30` -> `FUN_0058FEB0`, deviation from the original): store the
    /// frame for the clock time under `VIDEO_TEXTURE` if it differs from the one shown. If it cannot be decoded, the
    /// video ends as with an abort by key (warning, `MenuApp::end_video`); the sequence continues.
    fn show_video_frame(&mut self, now: u64) {
        let Some(v) = &mut self.video else {
            return;
        };
        let Some(k) = self
            .app
            .video()
            .filter(|p| p.path == v.path)
            .and_then(|p| p.frame_at(now))
        else {
            return;
        };
        if k == v.shown {
            return;
        }
        match v.file.image_at(k) {
            Ok(image) => {
                self.layer.assets.insert_image(VIDEO_TEXTURE, image.clone());
                v.shown = k;
            }
            Err(e) => {
                log::warn!("{e}; the video ends");
                self.app.end_video(now);
            }
        }
    }

    /// `UiEffect::PlayVideo` (`FUN_0045D560`): end a running video including its sound (`0x0045D584`), open (if that
    /// fails, the sequence continues without video, `0x0045D60D`), store frame 0 and report, Bink sound (in the
    /// original `binkw32.dll` plays it from `_BinkOpen`), `MenuMusic_Stop`, speech track.
    fn play_video(&mut self, path: String, audio: &mut dyn MenuAudio, vfs: &Vfs) {
        self.close_video(audio);
        let mut file = match (self.opener)(vfs, &path) {
            Ok(file) => file,
            Err(e) => {
                log::warn!("{e}; continuing without video");
                return;
            }
        };
        let info = file.info();
        let texture = match file.image_at(0) {
            Ok(image) => self.layer.assets.insert_image(VIDEO_TEXTURE, image.clone()),
            Err(e) => {
                log::warn!("{e}; continuing without video");
                return;
            }
        };
        self.app.video_started(&path, info, texture, self.now);
        let track = match file.track() {
            Some(Ok(stream)) => audio.play_video_track(stream, &path),
            Some(Err(e)) => {
                log::warn!("{e}; the video runs without Bink sound");
                None
            }
            None => None,
        };
        audio.stop_menu_music();
        let speech = audio.play_video_speech(vfs, &speech_path(&path));
        self.video = Some(HostVideo {
            path,
            file,
            shown: 0,
            track,
            speech,
        });
    }

    /// After the video (`FUN_0045CA70`): close the source, stop the Bink sound (in the original with `BinkClose`),
    /// fade out the speech track in 1000 ms.
    fn close_video(&mut self, audio: &mut dyn MenuAudio) {
        let Some(v) = self.video.take() else {
            return;
        };
        self.after_video = AFTER_VIDEO_FRAMES;
        // After the video `SetPointer(0, 1)`, `SetPointer(7, 1)` (`0x004C9C23...0x004C9C4B` among others): the loop of
        // pointer 7 starts anew.
        if let Some(model) = &mut self.model {
            model.restart_seven();
        }
        if let Some(id) = v.track {
            audio.stop_sound(id);
        }
        if let Some(id) = v.speech {
            audio.fade_out_sound(id, VIDEO_FADE_MS);
        }
    }

    /// Execute the effects of the last frame: sound and music immediately, settings file at `WriteSettings`, the rest
    /// as a request to the window. Afterwards the UI learns whether the speech sample is still playing.
    pub fn apply_effects(&mut self, audio: &mut dyn MenuAudio, vfs: &Vfs) -> HostRequests {
        let effects = self.app.take_effects();
        self.apply(effects, audio, vfs)
    }

    pub(crate) fn apply(
        &mut self,
        effects: Vec<UiEffect>,
        audio: &mut dyn MenuAudio,
        vfs: &Vfs,
    ) -> HostRequests {
        let mut req = HostRequests::default();
        self.after_video = self.after_video.saturating_sub(1);
        for effect in effects {
            match effect {
                UiEffect::PlaySfx {
                    event,
                    flags,
                    category: c,
                } => match category(c) {
                    Some(c) => audio.play_event_2d(vfs, &event, flags, c, false),
                    None => log::warn!("Sound {event}: unknown category {c}"),
                },
                UiEffect::PlaySpeech { path } => self.speech = audio.play_speech(vfs, &path),
                UiEffect::MenuMusic { path, looping } => {
                    audio.play_menu_music_looping(vfs, &path, looping);
                }
                // After a video with a running menu level (session present, even invisible) the original calls
                // `MusicManager_ResumeMusic` instead of `MenuMusic_Restart` (re/menu-level ML.4); the title music keeps
                // running in the menu level and returns with the next frame with `MUSIC`.
                UiEffect::RestartMusic => {
                    if self.after_video > 0 && self.app.level_world().present {
                        // The window applies only to the first `RestartMusic` afterwards.
                        self.after_video = 0;
                        audio.resume_game_music(vfs);
                    } else {
                        audio.restart_menu_music(vfs);
                    }
                }
                UiEffect::SetSoundDriver(index) => audio.set_sound_driver(index),
                UiEffect::SetVolume { category: c, value } => match category(c) {
                    Some(c) => audio.set_volume(c, value),
                    None => log::warn!("Volume: unknown category {c}"),
                },
                UiEffect::SetGamma(g) => req.gamma = Some(g),
                UiEffect::SetHardwareCursor(on) => req.hardware_cursor = Some(on),
                UiEffect::SetDisplayMode(mode) => req.display_mode = Some(mode),
                UiEffect::WriteSettings => self.write_settings(),
                UiEffect::WriteProfiles => self.write_profiles(),
                UiEffect::DeleteProfileSaves { guid } => self.delete_profile_saves(guid),
                UiEffect::Quit => req.quit = true,
                UiEffect::Unhandled(cmd) => log::info!(
                    "Application command {:#x} has no effect in the reimplementation yet: {cmd:?}",
                    cmd.code()
                ),
                // The match: `opennml-game` passes it to `GameFlow`, the viewer to the log; also the network session
                // (network commands, search, lobby requests to `GameFlow`, which passes them to `NetSession`).
                game @ (UiEffect::Net(_)
                | UiEffect::Search(_)
                | UiEffect::Lobby(_)
                | UiEffect::StartMatch(_)
                | UiEffect::ResumeGame { .. }
                | UiEffect::ReloadLevel
                | UiEffect::LoadGame { .. }
                | UiEffect::SaveGame { .. }
                | UiEffect::DeleteSave { .. }
                | UiEffect::PlayReplay { .. }
                | UiEffect::EndMatch
                | UiEffect::Surrender { .. }
                | UiEffect::StartCampaignLevel { .. }
                | UiEffect::Diplomacy(_)
                | UiEffect::SendChat { .. }
                | UiEffect::MenuLevel(_)) => req.game.push(game),
                // Report only: the UI has already queued the end screen, writing `profiles.ini` after a booking
                // follows as `WriteProfiles`.
                UiEffect::GameEnd { .. } | UiEffect::BookCampaign { .. } => {}
                UiEffect::PlayVideo { path, .. } => self.play_video(path, audio, vfs),
                // A `StopVideo` that reports `video_started` for an already closed video arrives only after the new
                // one was opened: then the new one is already running in the UI, and it stays open.
                UiEffect::StopVideo { .. } => {
                    if self.app.video().is_none() {
                        self.close_video(audio);
                    }
                }
            }
        }
        // If the UI has no video anymore (e.g. after `restart`), the host closes its own.
        if self.app.video().is_none() {
            self.close_video(audio);
        }
        let playing = self.speech.is_some_and(|id| audio.speech_playing(id));
        self.app.set_speech_playing(playing);
        req
    }

    fn write_settings(&self) {
        let Some(path) = &self.settings else {
            log::info!("Settings in memory only: not saved");
            return;
        };
        let mut config = self.app.config().clone();
        if let Some(saved) = self.saved_menu_level
            && !config.is_menu_level
        {
            config.is_menu_level = saved;
        }
        if let Err(e) = save_settings(path, &config) {
            log::warn!("Settings not saved ({}): {e:#}", path.display());
        }
    }

    /// Write `profiles.ini` (`FUN_004C6A90`), synchronously via `.tmp`; without a settings file only in memory.
    fn write_profiles(&self) {
        let Some(settings) = &self.settings else {
            log::info!("Profiles in memory only: not saved");
            return;
        };
        let path = profiles_path(settings);
        if let Err(e) = save_profiles(&path, &self.app.profiles().file) {
            log::warn!("Profiles not saved ({}): {e:#}", path.display());
        }
    }

    /// Savegames of a deleted profile (`FUN_004C6D70`): in `savegame/` delete every file matching `*.<GUID %03X>`
    /// (`FindFirstFile` compares case-insensitively; not the subfolders `multiplayer`/`autoplay`, O-29). Without a
    /// settings file there is no folder.
    fn delete_profile_saves(&self, guid: i32) {
        let Some(settings) = &self.settings else {
            log::info!("Savegames in memory only: nothing to delete (GUID {guid})");
            return;
        };
        let dir = savegame_dir(settings);
        // `%03X` of an `int`: negative values as unsigned 32 bits.
        let suffix = format!(".{:03X}", guid as u32);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                log::warn!("Savegames \"{}\" not readable: {e}", dir.display());
                return;
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let matches = name.len() >= suffix.len()
                && name.is_char_boundary(name.len() - suffix.len())
                && name[name.len() - suffix.len()..].eq_ignore_ascii_case(&suffix);
            if !matches || !entry.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => log::info!("Savegame \"{name}\" of the deleted profile deleted"),
                Err(e) => log::warn!("Savegame \"{name}\" could not be deleted: {e}"),
            }
        }
    }

    /// Program end: write settings like the destructor `FUN_00496E50` (O-32).
    pub fn shutdown(&mut self) {
        self.write_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::pointer::PointerTint;
    use crate::settings::load_settings;
    use crate::testutil::{
        FakeAudio, FakeVideoLog, capture_warnings, fake_opener, start_world, take_warnings, world,
    };
    use opennml_ui::msg::AppCmd;
    use opennml_ui::video::VIDEO_TEXTURE;

    const S1024: Screen = Screen {
        width: 1024,
        height: 768,
    };
    const SIZE: (u32, u32) = (1024, 768);

    /// Host at 1024 x 768 on the small installation; the settings file lies in `home`.
    fn host(vfs: &Vfs, home: &Path, audio: &FakeAudio) -> MenuHost {
        let layer = UiLayer::new(vfs, "DEU", 1024, None).unwrap();
        let settings = home.join("OpenNML").join("Engine.ini");
        let config = load_settings(&settings, vfs);
        MenuHost::new(
            vfs,
            layer,
            config,
            MenuOptions::default(),
            audio,
            Some(settings),
        )
        .unwrap()
    }

    /// Opens `TEST` and runs frames until it settles.
    fn open_test(h: &mut MenuHost, vfs: &Vfs, now: &mut u64) {
        h.app.open_by_name("test").unwrap();
        for _ in 0..3 {
            h.frame_cmds(vfs, S1024, *now);
            *now += 16;
        }
        assert!(!h.app.report(S1024, *now).busy);
    }

    // QUIRK O-32: kept
    #[test]
    fn menu_effects_reach_audio_and_requests() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let mut now = 0;
        open_test(&mut h, &vfs, &mut now);
        // The music of the frame (`0x160`, loop without DONOTLOOPMUSIC) runs via the fake.
        assert_eq!(h.apply_effects(&mut audio, &vfs), HostRequests::default());
        assert_eq!(audio.calls, ["music DATA/MUSIC/X.OGG true"]);
        // A real click: SFX_KLICK, flags 0x14, sound effects, without loop.
        audio.calls.clear();
        let (x, y) = h.app.element_center("test", "ok", S1024).unwrap();
        for ev in [
            InputEvent::CursorMoved {
                x: f64::from(x),
                y: f64::from(y),
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: true,
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: false,
            },
        ] {
            h.on_input(&ev, S1024, now);
        }
        h.frame_cmds(&vfs, S1024, now);
        assert_eq!(h.apply_effects(&mut audio, &vfs), HostRequests::default());
        assert_eq!(audio.calls, ["event SFX_KLICK 0x14 sfx false"]);
        // All kinds of effects.
        audio.calls.clear();
        let mode = DisplayMode {
            width: 1280,
            height: 1024,
            bits: 32,
        };
        let settings = h.settings_path().unwrap().to_path_buf();
        assert!(!settings.exists());
        let req = h.apply(
            vec![
                UiEffect::SetVolume {
                    category: 2,
                    value: 700,
                },
                UiEffect::PlaySpeech {
                    path: "data/sprache/testtext.ogg".into(),
                },
                UiEffect::SetSoundDriver(1),
                UiEffect::RestartMusic,
                UiEffect::SetGamma(0.6),
                UiEffect::SetHardwareCursor(false),
                UiEffect::SetDisplayMode(mode),
                UiEffect::Unhandled(AppCmd::Video(0)),
                UiEffect::WriteSettings,
                UiEffect::ReloadLevel,
                UiEffect::Quit,
                UiEffect::SaveGame { name: "x".into() },
            ],
            &mut audio,
            &vfs,
        );
        assert_eq!(
            audio.calls,
            [
                "volume speech 700",
                "speech data/sprache/testtext.ogg",
                "driver 1",
                "restart"
            ]
        );
        assert_eq!(
            req,
            HostRequests {
                display_mode: Some(mode),
                hardware_cursor: Some(false),
                gamma: Some(0.6),
                quit: true,
                // The game effects go in order to the match flow.
                game: vec![
                    UiEffect::ReloadLevel,
                    UiEffect::SaveGame { name: "x".into() }
                ],
            }
        );
        // WriteSettings writes the file (folder created), readable with the same values.
        assert_eq!(load_settings(&settings, &vfs), *h.app.config());
        // After the frame the host reports whether the speech sample is playing.
        assert!(h.app.speech_playing());
        audio.speech_running = false;
        h.apply(Vec::new(), &mut audio, &vfs);
        assert!(!h.app.speech_playing());
    }

    #[test]
    fn music_lengths_are_known_before_the_first_frame() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let h = host(&vfs, home.path(), &audio);
        // Each MUSIC file of the dialogs once, when the host is built (before the first frame).
        assert_eq!(*audio.lengths_asked.borrow(), ["DATA/MUSIC/X.OGG"]);
        assert_eq!(music_paths(h.app.menu().library()), ["DATA/MUSIC/X.OGG"]);
    }

    // QUIRK O-58: kept
    #[test]
    fn display_mode_change_keeps_the_font_size() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let button = h.layer.fonts.by_name("MenuButton");
        assert_eq!(h.layer.fonts.font(button).px, 21, "width 1024 at start");
        let mut now = 0;
        open_test(&mut h, &vfs, &mut now);
        let mode = DisplayMode {
            width: 1280,
            height: 1024,
            bits: 32,
        };
        let req = h.apply(vec![UiEffect::SetDisplayMode(mode)], &mut audio, &vfs);
        assert_eq!(req.display_mode, Some(mode));
        // New size, minimized (0 x 0) and back: position follows, fonts stay (O-58), no panic.
        for screen in [
            Screen {
                width: 1280,
                height: 1024,
            },
            Screen {
                width: 0,
                height: 0,
            },
            Screen {
                width: 1280,
                height: 1024,
            },
        ] {
            h.frame_cmds(&vfs, screen, now);
            now += 16;
            assert_eq!(h.layer.fonts.font(button).px, 21);
        }
        let s1280 = Screen {
            width: 1280,
            height: 1024,
        };
        let (x, _) = h.app.element_center("test", "ok", s1280).unwrap();
        let (x1024, _) = h.app.element_center("test", "ok", S1024).unwrap();
        assert_eq!((x, x1024), (200, 160), "position in the 1280 x 1024 grid");
    }

    #[test]
    fn pointer_seven_sits_at_its_hotspot_outside_the_frame_commands() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        // No mouse position yet: no pointer.
        assert_eq!(h.pointer_quad(), None);
        h.on_input(&InputEvent::CursorMoved { x: 10.7, y: 20.2 }, S1024, 16);
        // The commands of a frame never contain the pointer (the host appends it when drawing).
        assert!(h.frame_cmds(&vfs, S1024, 16).is_empty());
        let Some(DrawCmd::Quad {
            texture: Some(t),
            dst,
            uv,
            color,
            filter,
            blend: Blend::Alpha,
        }) = h.pointer_quad()
        else {
            panic!("pointer missing: {:?}", h.pointer_quad())
        };
        assert_eq!(h.layer.assets.path(t), "data/pointer/pointer-normal.tga");
        assert_eq!(dst, [8, 18, 13, 25], "hotspot 2,2; image 5 x 7");
        assert_eq!(
            (uv, color, filter),
            ([0.0, 0.0, 1.0, 1.0], 0xFFFF_FFFF, Filter::Point)
        );
        // No pointer outside the window.
        h.on_input(&InputEvent::CursorLeft, S1024, 48);
        assert_eq!(h.pointer_quad(), None);
    }

    /// Image path and destination rectangle of the drawn pointer.
    fn drawn_pointer(h: &MenuHost) -> (String, [i32; 4]) {
        match h.pointer_quad() {
            Some(DrawCmd::Quad {
                texture: Some(t),
                dst,
                ..
            }) => (h.layer.assets.path(t).to_string(), dst),
            other => panic!("pointer missing: {other:?}"),
        }
    }

    /// Pointer 7 variant 0 as the fallback (deviation from the original, O-298): IDs outside 0...11, a variant outside
    /// 0...2, a missing row or file draw pointer 7 variant 0, with a warning per ID and variant, without a crash.
    // QUIRK O-298: deviation
    #[test]
    fn unknown_or_missing_pointer_falls_back_to_seven() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        h.on_input(&InputEvent::CursorMoved { x: 100.0, y: 50.0 }, S1024, 16);
        let seven = (
            "data/pointer/pointer-normal.tga".to_string(),
            [98, 48, 103, 55],
        );
        assert_eq!(h.pointer_choice(), PointerChoice::NORMAL);
        assert_eq!(drawn_pointer(&h), seven);
        // Existing variants: normal and green of pointer 9 with its hotspot (3, 4).
        h.set_pointer(PointerChoice {
            id: 9,
            variant: 0,
            ..PointerChoice::NORMAL
        });
        assert_eq!(
            drawn_pointer(&h),
            ("data/pointer/p9.tga".to_string(), [97, 46, 101, 50])
        );
        h.set_pointer(PointerChoice {
            id: 9,
            variant: 2,
            ..PointerChoice::NORMAL
        });
        assert_eq!(
            drawn_pointer(&h),
            ("data/pointer/p9-green.tga".to_string(), [97, 46, 103, 52])
        );
        assert!(h.pointer.warned.is_empty());
        // Missing red file, missing image of pointer 6, pointer without a row (5), IDs 12 and -1, variant 3: pointer 7.
        let missing = [(9, 1), (6, 0), (5, 0), (12, 0), (-1, 0), (7, 3)];
        for _ in 0..2 {
            for (id, variant) in missing {
                h.set_pointer(PointerChoice {
                    id,
                    variant,
                    ..PointerChoice::NORMAL
                });
                assert_eq!(
                    h.pointer_choice(),
                    PointerChoice {
                        id,
                        variant,
                        ..PointerChoice::NORMAL
                    }
                );
                assert_eq!(drawn_pointer(&h), seven, "{id}/{variant}");
            }
        }
        // One warning per ID and variant, also after the second choice.
        let warned: Vec<(i32, u8)> = h.pointer.warned.iter().copied().collect();
        assert_eq!(warned, [(-1, 0), (5, 0), (6, 0), (7, 3), (9, 1), (12, 0)]);
    }

    /// Host at 1024 x 768 with `HardwareCursor` = `hardware` on `vfs`, mouse at (100, 50).
    fn host_with_cursor(vfs: &Vfs, home: &Path, audio: &FakeAudio, hardware: bool) -> MenuHost {
        let layer = UiLayer::new(vfs, "DEU", 1024, None).unwrap();
        let settings = home.join("OpenNML").join("Engine.ini");
        let mut config = load_settings(&settings, vfs);
        config.hardware_cursor = hardware;
        let mut h = MenuHost::new(
            vfs,
            layer,
            config,
            MenuOptions::default(),
            audio,
            Some(settings),
        )
        .unwrap();
        assert_eq!(h.app.config().hardware_cursor, hardware);
        h.on_input(&InputEvent::CursorMoved { x: 100.0, y: 50.0 }, S1024, 0);
        h
    }

    /// Without a loadable `mouse.rdo` (small installation) `HardwareCursor 0` also draws the TGA of the variant,
    /// untinted, with a warning when loading; the choice still applies.
    // QUIRK O-297: kept
    #[test]
    fn missing_model_falls_back_to_the_tga() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        capture_warnings();
        let mut h = host_with_cursor(&vfs, home.path(), &audio, false);
        let warnings = take_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains(MOUSE_MODEL_PATH) && w.contains("TGA")),
            "{warnings:?}"
        );
        assert_eq!(h.pointer_model_data(0.0), None);
        let sprite = |draw: Option<PointerDraw>| match draw {
            Some(PointerDraw::Sprite(cmd)) => cmd,
            other => panic!("TGA pointer expected: {other:?}"),
        };
        assert_eq!(Some(sprite(h.pointer_draw(16.0, SIZE))), h.pointer_quad());
        assert_eq!(drawn_pointer(&h).0, "data/pointer/pointer-normal.tga");
        h.set_pointer(PointerChoice {
            id: 9,
            variant: 2,
            tint: PointerTint::TargetValid,
        });
        let DrawCmd::Quad { texture, color, .. } = sprite(h.pointer_draw(32.0, SIZE)) else {
            panic!("no quad");
        };
        assert_eq!(
            texture.map(|t| h.layer.assets.path(t).to_string()),
            Some("data/pointer/p9-green.tga".into())
        );
        assert_eq!(
            color, 0xFFFF_FFFF,
            "without tint (that belongs to the 3D pointer)"
        );
        assert!(matches!(
            h.pointer_preview(32.0, SIZE),
            Some(PointerDraw::Sprite(_))
        ));
    }

    /// Host with `HardwareCursor` = `hardware` on the installation; `None` without an installation.
    fn corpus_cursor_host(hardware: bool) -> Option<(Vfs, tempfile::TempDir, MenuHost)> {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return None;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let home = tempfile::tempdir().unwrap();
        let h = host_with_cursor(&vfs, home.path(), &FakeAudio::default(), hardware);
        Some((vfs, home, h))
    }

    /// Clicks the checkbox `hardwaremaus` of the open `gameplayoptions` and runs a frame.
    fn toggle_hardware_cursor(h: &mut MenuHost, vfs: &Vfs, now: &mut u64) {
        let (x, y) = h
            .app
            .element_center("gameplayoptions", "hardwaremaus", S1024)
            .unwrap();
        for ev in [
            InputEvent::CursorMoved {
                x: f64::from(x),
                y: f64::from(y),
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: true,
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: false,
            },
        ] {
            h.on_input(&ev, S1024, *now);
        }
        h.frame_cmds(vfs, S1024, *now);
        *now += 16;
    }

    /// The options menu switches `HardwareCursor` live; in the very next frame the host draws the model instead of the
    /// TGA and back. The choice (also with tint) reaches the model only with software (O-462).
    // QUIRK O-297, O-462: kept
    #[test]
    fn pointer_draw_follows_hardware_cursor_live() {
        let Some((vfs, _home, mut h)) = corpus_cursor_host(true) else {
            return;
        };
        let mut now = 0;
        assert!(matches!(
            h.pointer_draw(0.0, SIZE),
            Some(PointerDraw::Sprite(_))
        ));
        h.app.open_by_name("gameplayoptions").unwrap();
        for _ in 0..10 {
            h.frame_cmds(&vfs, S1024, now);
            now += 16;
        }
        toggle_hardware_cursor(&mut h, &vfs, &mut now);
        assert!(!h.app.config().hardware_cursor, "checkbox off");
        let tinted = PointerChoice {
            tint: PointerTint::BuildValid,
            ..PointerChoice::NORMAL
        };
        h.set_pointer(tinted);
        let Some(PointerDraw::Model(scene)) = h.pointer_draw(now as f32, SIZE) else {
            panic!("3D pointer expected");
        };
        assert_eq!(scene.tint, PointerTint::BuildValid.rgba());
        assert_eq!(Some(scene.mouse), h.pointer.at);
        assert_eq!(
            h.pointer_model_data(now as f32).unwrap().phase_ms,
            Some(0.0)
        );
        toggle_hardware_cursor(&mut h, &vfs, &mut now);
        assert!(h.app.config().hardware_cursor, "checkbox on");
        assert!(matches!(
            h.pointer_draw(now as f32, SIZE),
            Some(PointerDraw::Sprite(_))
        ));
        // With the hardware pointer the choice sets only the ID, animation and tint stay (O-462).
        h.set_pointer(PointerChoice::NORMAL.with_id(6));
        let data = h.pointer_model_data(now as f32).unwrap();
        assert_eq!((data.id, data.animation), (6, 7));
        assert_eq!(data.tint, PointerTint::BuildValid.rgba());
    }

    /// `ui_state.pointer_model` reports ID, animation, tint, phase and the setting; the query does not fix the restart
    /// of the loop.
    // QUIRK O-297: kept
    #[test]
    fn pointer_model_data_in_ui_state() {
        for hardware in [false, true] {
            let Some((_vfs, _home, mut h)) = corpus_cursor_host(hardware) else {
                return;
            };
            let tinted = PointerChoice {
                tint: PointerTint::BuildValid,
                ..PointerChoice::NORMAL.with_id(6)
            };
            h.set_pointer(tinted);
            // Open restart: no phase yet, and the query leaves it open.
            let report = h.pointer_model_report(500.0).unwrap();
            assert_eq!(report.phase_ms, None, "hardware {hardware}");
            assert_eq!(report.software, !hardware);
            assert_eq!(h.pointer_model_report(900.0).unwrap().phase_ms, None);
            h.pointer_draw(1000.0, SIZE).unwrap();
            let report = h.pointer_model_report(1000.0).unwrap();
            let data = h.pointer_model_data(1000.0).unwrap();
            assert_eq!(
                (report.id, report.animation, report.tint, report.phase_ms),
                (data.id, data.animation, data.tint, data.phase_ms)
            );
            assert_eq!(report.id, 6);
            if hardware {
                // O-462: with hardware the choice sets only the ID, the tint stays white.
                assert_eq!(report.tint, PointerTint::White.rgba());
            } else {
                assert_eq!(report.tint, PointerTint::BuildValid.rgba());
                assert_eq!(report.phase_ms, Some(0.0));
            }
        }
    }

    /// As long as the UI has the pointer off or the mouse is outside the window, there is no pointer in either
    /// setting; the phase keeps running.
    // QUIRK O-459: kept
    #[test]
    fn hidden_pointer_draws_nothing() {
        for hardware in [false, true] {
            let Some((_vfs, _home, mut h)) = corpus_cursor_host(hardware) else {
                return;
            };
            assert!(h.pointer_draw(0.0, SIZE).is_some(), "visible ({hardware})");
            h.app.set_pointer_hidden(true);
            assert!(h.pointer_draw(300.0, SIZE).is_none(), "off ({hardware})");
            assert!(h.pointer_preview(300.0, SIZE).is_none());
            h.app.set_pointer_hidden(false);
            h.on_input(&InputEvent::CursorLeft, S1024, 400);
            assert!(
                h.pointer_draw(400.0, SIZE).is_none(),
                "outside ({hardware})"
            );
            assert!(h.pointer_preview(400.0, SIZE).is_none());
            if !hardware {
                let phase = h.pointer_model_data(1300.0).unwrap().phase_ms.unwrap();
                assert!((phase - 300.0).abs() < 1e-2, "phase {phase}");
            }
        }
    }

    /// End of video (`SetPointer(0)`, `SetPointer(7)`): if the host closes a video, the loop of 7 restarts in the
    /// next drawn frame.
    // QUIRK O-459: kept
    #[test]
    fn video_end_restarts_the_pointer_loop() {
        let Some((vfs, _home, mut h)) = corpus_cursor_host(false) else {
            return;
        };
        let mut audio = FakeAudio::default();
        h.pointer_draw(0.0, SIZE).unwrap();
        h.pointer_draw(400.0, SIZE).unwrap();
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        h.set_video_opener(fake_opener(&log, false, None));
        let mut now = 1000;
        run(&mut h, &mut audio, &vfs, now);
        h.app.play_video_by_name("x").unwrap();
        while h.app.video().is_none() {
            now += 16;
            run(&mut h, &mut audio, &vfs, now);
            assert!(now < 3000, "video does not start");
        }
        let phase = h.pointer_model_data(now as f32).unwrap().phase_ms;
        assert!(phase.is_some(), "no restart when opening");
        while h.app.video().is_some() {
            now += 16;
            run(&mut h, &mut audio, &vfs, now);
            assert!(now < 5000, "video does not end");
        }
        assert!(h.video.is_none(), "video closed");
        assert_eq!(h.pointer_model_data(now as f32).unwrap().phase_ms, None);
        while h.pointer_draw(now as f32, SIZE).is_none() {
            now += 16;
            run(&mut h, &mut audio, &vfs, now);
            assert!(now < 7000, "pointer does not come back");
        }
        assert_eq!(
            h.pointer_model_data(now as f32).unwrap().phase_ms,
            Some(0.0)
        );
    }

    /// All 12 pointers in three variants from the installation, 64 x 64, the hotspot `OFFSET` on the mouse.
    // QUIRK O-297: kept
    #[test]
    fn pointer_quads_use_the_hotspot() {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let hotspots = [
            (14, 16),
            (12, 11),
            (20, 16),
            (16, 16),
            (15, 15),
            (15, 14),
            (2, 2),
            (2, 2),
            (17, 14),
            (13, 13),
            (14, 21),
            (22, 12),
        ];
        assert_eq!(h.pointer.sprites.len(), 36, "12 pointers x 3 variants");
        h.on_input(&InputEvent::CursorMoved { x: 300.0, y: 200.0 }, S1024, 16);
        let mut paths = BTreeSet::new();
        for (id, &(hx, hy)) in (0..).zip(&hotspots) {
            for variant in 0..3 {
                h.set_pointer(PointerChoice {
                    id,
                    variant,
                    ..PointerChoice::NORMAL
                });
                let (path, dst) = drawn_pointer(&h);
                assert_eq!(
                    dst,
                    [300 - hx, 200 - hy, 364 - hx, 264 - hy],
                    "{id}/{variant}"
                );
                assert!(path.starts_with("data/pointer/"), "{path}");
                paths.insert(path);
            }
        }
        assert_eq!(paths.len(), 36, "one file per pointer and variant");
        assert!(h.pointer.warned.is_empty(), "{:?}", h.pointer.warned);
    }

    #[test]
    fn keys_reach_the_menu_with_held_chords_and_characters() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let mut now = 0;
        open_test(&mut h, &vfs, &mut now);
        let key = |code, pressed| InputEvent::Key {
            code,
            pressed,
            repeat: false,
        };
        let name = |h: &MenuHost, now| {
            h.app
                .report(S1024, now)
                .elements
                .into_iter()
                .find(|e| e.name == "name")
                .and_then(|e| e.text)
        };
        // Shift held, then N with a character: the input field (focus) takes "N"; the umlaut a (0xE4) as cp1252.
        h.on_input(&key(KeyCode::ShiftLeft, true), S1024, now);
        h.on_key(KeyCode::KeyN, true, Some(u16::from(b'N')), S1024, now);
        assert_eq!(h.held.held(), [0x10, 0x4E]);
        h.on_input(&key(KeyCode::KeyN, false), S1024, now);
        h.on_input(&key(KeyCode::ShiftLeft, false), S1024, now);
        h.on_key(KeyCode::Quote, true, Some(0xE4), S1024, now);
        h.on_key(KeyCode::Quote, false, None, S1024, now);
        // Release happens only at the start of the next round (after the frame).
        assert_eq!(h.held.held(), [0x10, 0x4E, 0xDE]);
        h.frame_cmds(&vfs, S1024, now);
        assert!(h.held.held().is_empty());
        assert_eq!(name(&h, now).as_deref(), Some("N\u{e4}"));
        // Key without a VK and without a character: nothing; focus loss releases everything.
        h.on_key(KeyCode::Fn, true, None, S1024, now);
        h.on_key(KeyCode::KeyA, true, Some(u16::from(b'a')), S1024, now);
        h.release_keys();
        assert!(h.held.held().is_empty());
        h.frame_cmds(&vfs, S1024, now);
        assert_eq!(name(&h, now).as_deref(), Some("N\u{e4}a"));
    }

    // QUIRK O-116, O-331: kept
    #[test]
    fn mouse_buttons_reach_the_key_table_a_round_later() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let left = |pressed| InputEvent::Button {
            button: MouseButton::Left,
            pressed,
        };
        // In the frame of the press VK 1 is still missing, from the next one on it stands before ESC (no ESC
        // broadcast, O-14/O-116).
        h.on_input(&left(true), S1024, 0);
        h.on_key(KeyCode::Escape, true, None, S1024, 0);
        assert_eq!(h.held.held(), [0x1B]);
        h.on_key(KeyCode::Escape, false, None, S1024, 0);
        h.frame_cmds(&vfs, S1024, 0);
        h.on_key(KeyCode::Escape, true, None, S1024, 16);
        assert_eq!(
            opennml_ui::keys::chord_name(&h.held.held()),
            "LBUTTON ESCAPE"
        );
        h.on_key(KeyCode::Escape, false, None, S1024, 16);
        h.on_input(&left(false), S1024, 16);
        h.frame_cmds(&vfs, S1024, 16);
        assert!(h.held.held().is_empty());

        // O-331: the middle button is in the table as VK 4 (`MBUTTON`, `KD_CAM_FREEHAND`) from the next round on and
        // does not reach the UI (no widget event, `FUN_004B8E70`).
        let middle = |pressed| InputEvent::Button {
            button: MouseButton::Middle,
            pressed,
        };
        h.on_input(&middle(true), S1024, 32);
        h.frame_cmds(&vfs, S1024, 32);
        assert_eq!(h.held_vk(), [4]);
        assert_eq!(opennml_ui::keys::chord_name(&h.held_vk()), "MBUTTON");
        h.on_input(&middle(false), S1024, 48);
        h.frame_cmds(&vfs, S1024, 48);
        assert!(h.held_vk().is_empty());
    }

    #[test]
    fn installation_host_loads_pointer_seven_and_starts_the_title_music() {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut audio = AudioEngine::open(&vfs, false);
        let settings = home.path().join("Engine.ini");
        let config = load_settings(&settings, &vfs);
        assert_eq!(config.key_map.len(), 460, "defaults from defaultkeys.ini");
        let opts = MenuOptions {
            start_menu: true,
            ..MenuOptions::default()
        };
        let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let mut h = MenuHost::new(&vfs, layer, config, opts, &audio, Some(settings)).unwrap();
        let PointerSprite {
            texture,
            size,
            hotspot,
        } = h.pointer.sprite().expect("pointer 7");
        assert_eq!(
            h.layer.assets.path(texture),
            "data/pointer/pointer-normal.tga"
        );
        assert!(size[0] > 0 && size[1] > 0);
        assert_eq!(hotspot, [2, 2]);
        // Start sequence up to the main menu; its music runs via the real sound system (without a device). The three
        // videos of the installation each run up to frame 2 and are then aborted by key.
        let mut now = 0;
        let mut videos = Vec::new();
        while audio.menu.file.is_none() && now < 60_000 {
            if h.app.video_frame(now) == Some(2) {
                h.on_key(KeyCode::KeyX, true, Some(u16::from(b'x')), S1024, now);
                h.on_key(KeyCode::KeyX, false, None, S1024, now);
            }
            h.frame_cmds(&vfs, S1024, now);
            h.apply_effects(&mut audio, &vfs);
            if let Some(v) = h.app.video()
                && videos.last() != Some(&v.path)
            {
                videos.push(v.path.clone());
                let image = h.layer.assets.image(v.texture);
                assert_eq!(image.dimensions(), (640, v.info.height), "{}", v.path);
            }
            now += 16;
        }
        assert_eq!(videos, opennml_ui::video::INTRO_VIDEOS);
        assert_eq!(audio.menu.file.as_deref(), Some("DATA/MUSIC/TITLE.OGG"));
        assert!(audio.menu.handle.is_some());
    }

    #[test]
    fn restart_closes_every_dialog_and_keeps_the_settings() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let mut now = 0;
        open_test(&mut h, &vfs, &mut now);
        assert_eq!(h.app.menu().open_names(), ["test"]);
        let config = h.app.config().clone();
        h.restart(&vfs, MenuOptions::default(), &audio).unwrap();
        assert!(h.app.menu().open_names().is_empty());
        assert!(h.app.queue().is_empty());
        assert_eq!(*h.app.config(), config);
        // The music lengths are known again before the first frame; the dialog can be reopened.
        assert_eq!(audio.lengths_asked.borrow().len(), 2);
        open_test(&mut h, &vfs, &mut now);
        assert_eq!(h.app.menu().open_names(), ["test"]);
    }

    #[test]
    fn settings_only_in_memory_are_never_written() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let mut audio = FakeAudio::default();
        let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let config = crate::testutil::test_config(&vfs);
        let mut h =
            MenuHost::new(&vfs, layer, config, MenuOptions::default(), &audio, None).unwrap();
        assert_eq!(h.settings_path(), None);
        h.apply(vec![UiEffect::WriteSettings], &mut audio, &vfs);
        h.shutdown();
        // Nothing in the temporary directory and no location that could be written to.
        assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
    }

    /// One frame of the host: commands, then effects.
    fn run(h: &mut MenuHost, audio: &mut FakeAudio, vfs: &Vfs, now: u64) -> Vec<DrawCmd> {
        let cmds = h.frame_cmds(vfs, S1024, now);
        h.apply_effects(audio, vfs);
        cmds
    }

    /// ID and revision of the video frame and its red value (the fake colors frame k with red = k).
    fn video_texture(h: &MenuHost) -> Option<(TextureId, u32, u8)> {
        let t = h.layer.assets.find(VIDEO_TEXTURE)?;
        let red = h.layer.assets.image(t).get_pixel(0, 0)[0];
        Some((t, h.layer.assets.revision(t), red))
    }

    fn shows_video(cmds: &[DrawCmd], texture: TextureId) -> bool {
        cmds.iter()
            .any(|c| matches!(c, DrawCmd::Quad { texture: Some(t), .. } if *t == texture))
    }

    /// Host of the start sequence (`start_menu`, `IsIntro`) on the installation with a main menu, mouse in the window.
    fn start_host(vfs: &Vfs, home: &Path, audio: &FakeAudio) -> MenuHost {
        let layer = UiLayer::new(vfs, "DEU", 1024, None).unwrap();
        let settings = home.join("Engine.ini");
        let mut config = load_settings(&settings, vfs);
        config.is_intro = true;
        let opts = MenuOptions {
            start_menu: true,
            ..MenuOptions::default()
        };
        let mut h = MenuHost::new(vfs, layer, config, opts, audio, None).unwrap();
        h.on_input(&InputEvent::CursorMoved { x: 50.0, y: 60.0 }, S1024, 0);
        h
    }

    // QUIRK O-119: kept
    #[test]
    fn host_plays_and_ends_a_video() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        for track in [true, false] {
            let log = Rc::new(RefCell::new(FakeVideoLog::default()));
            let mut audio = FakeAudio::default();
            let mut h = host(&vfs, home.path(), &audio);
            h.set_video_opener(fake_opener(&log, track, None));
            h.app.play_video_by_name("t").unwrap();
            // Frame 0 lies under `VIDEO_TEXTURE` immediately; order as `FUN_0045D560`: Bink sound (only with a
            // track), `MenuMusic_Stop`, speech track.
            run(&mut h, &mut audio, &vfs, 0);
            let (texture, revision, red) = video_texture(&h).expect("frame 0");
            assert_eq!((revision, red), (0, 0));
            let mut start = vec![
                "stop-menu-music".to_string(),
                "video-speech data/sprache/t.ogg".to_string(),
            ];
            if track {
                start.insert(0, "video-track data/videos/t.bik".into());
            }
            assert_eq!(audio.calls, start, "with sound: {track}");
            let v = h.app.video().expect("video running").clone();
            assert_eq!(
                (v.path.as_str(), v.started_ms, v.texture, v.info),
                ("data/videos/t.bik", 0, texture, crate::testutil::FAKE_VIDEO)
            );
            audio.calls.clear();
            // The revision rises only when a new frame is due (at 40 and 80 ms).
            for (now, expected) in [(16, 0), (32, 0), (40, 1), (56, 1), (80, 2), (112, 2)] {
                let cmds = run(&mut h, &mut audio, &vfs, now);
                let (_, revision, red) = video_texture(&h).unwrap();
                assert_eq!((revision, u32::from(red)), (expected, expected), "{now} ms");
                assert!(shows_video(&cmds, texture), "{now} ms");
            }
            assert!(audio.calls.is_empty(), "{:?}", audio.calls);
            // End at 3 * 40 ms (`StopVideo`): Bink sound off, speech track off in 1000 ms (`FUN_0045CA70`).
            let cmds = run(&mut h, &mut audio, &vfs, 120);
            assert!(!shows_video(&cmds, texture));
            assert!(h.app.video().is_none());
            let mut end = vec!["fade-out 200 1000".to_string()];
            if track {
                end.insert(0, "stop 100".into());
            }
            assert_eq!(audio.calls, end, "with sound: {track}");
            // `0x1A` ends, the end of the probe video (`AfterProbeVideo`) restarts the menu music.
            audio.calls.clear();
            for now in [136, 152, 168] {
                run(&mut h, &mut audio, &vfs, now);
            }
            assert_eq!(audio.calls, ["restart"]);
            assert!(h.app.queue().is_empty());
            let images: Vec<u32> = log.borrow().images.iter().map(|(_, k)| *k).collect();
            assert_eq!(images, [0, 1, 2]);
            assert_eq!(log.borrow().opened, ["data/videos/t.bik"]);
        }
    }

    /// O-440: the version line only without a visible world and without a video (`(D5E0 & 0x41) == 0`), one quad per
    /// character of [`VERSION_TEXT`]; the menu commands never contain it (the host draws it above the pointer).
    // QUIRK O-440: kept
    #[test]
    fn version_line_only_without_world_and_video() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let chars = VERSION_TEXT.chars().count();
        assert_eq!(h.version_cmds(S1024, false).len(), chars);
        assert!(h.version_cmds(S1024, true).is_empty(), "visible world");
        h.set_video_opener(fake_opener(&log, false, None));
        h.app.play_video_by_name("t").unwrap();
        run(&mut h, &mut audio, &vfs, 0);
        assert!(h.app.video().is_some());
        assert!(h.version_cmds(S1024, false).is_empty(), "video running");
        run(&mut h, &mut audio, &vfs, 5_000);
        assert!(h.app.video().is_none());
        assert_eq!(h.version_cmds(S1024, false).len(), chars);
    }

    #[test]
    fn big_steps_decode_in_order_and_convert_once() {
        // Clock jump 0 -> 80 ms: the source gets exactly one call for frame 2 (it decodes the intermediate frames
        // itself, `BinkVideoFile`), clock stopped: no further one; jump past the end: none anymore.
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        h.set_video_opener(fake_opener(&log, false, None));
        h.app.play_video_by_name("t").unwrap();
        run(&mut h, &mut audio, &vfs, 0);
        run(&mut h, &mut audio, &vfs, 80);
        assert_eq!(video_texture(&h).map(|(_, r, red)| (r, red)), Some((1, 2)));
        run(&mut h, &mut audio, &vfs, 80);
        run(&mut h, &mut audio, &vfs, 80);
        assert_eq!(video_texture(&h).map(|(_, r, _)| r), Some(1));
        run(&mut h, &mut audio, &vfs, 5_000);
        assert!(h.app.video().is_none());
        let images: Vec<u32> = log.borrow().images.iter().map(|(_, k)| *k).collect();
        assert_eq!(images, [0, 2]);
    }

    #[test]
    fn missing_video_is_skipped_and_start_opens() {
        // Default opener (`open_bink`) on an installation without `.bik`: one warning per video, the sequence
        // continues without video (`FUN_0045D560` `0x0045D60D`), no `MenuMusic_Stop`; after `0x24` the pointer is there
        // and `start` open.
        let (_dir, vfs) = start_world();
        let home = tempfile::tempdir().unwrap();
        let mut audio = FakeAudio::default();
        let mut h = start_host(&vfs, home.path(), &audio);
        capture_warnings();
        let mut now = 0;
        let mut frames = 0;
        while !h.app.menu().open_names().contains(&"start".to_string()) {
            assert!(h.pointer_quad().is_none() || !h.app.pointer_hidden());
            run(&mut h, &mut audio, &vfs, now);
            assert!(h.app.video().is_none());
            now += 16;
            frames += 1;
            assert!(frames < 30, "start not open after {frames} frames");
        }
        let missing: Vec<String> = take_warnings()
            .into_iter()
            .filter(|w| w.contains("is missing from the installation"))
            .collect();
        assert_eq!(missing.len(), 3, "{missing:?}");
        for (warning, path) in missing.iter().zip(opennml_ui::video::INTRO_VIDEOS) {
            assert!(warning.contains(path), "{warning}");
        }
        assert!(!h.app.pointer_hidden());
        assert!(h.pointer_quad().is_some(), "pointer back after 0x24");
        assert!(
            !audio
                .calls
                .iter()
                .any(|c| c.starts_with("stop-menu-music") || c.starts_with("video-"))
        );
        assert!(h.layer.assets.find(VIDEO_TEXTURE).is_none());
    }

    #[test]
    fn decoder_error_ends_the_video() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        // Error at frame 1: warning with file name, the video ends as with a key press, the sequence continues.
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        h.set_video_opener(fake_opener(&log, true, Some(1)));
        h.app.play_video_by_name("t").unwrap();
        run(&mut h, &mut audio, &vfs, 0);
        audio.calls.clear();
        capture_warnings();
        run(&mut h, &mut audio, &vfs, 40);
        let warnings = take_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("t.bik") && warnings[0].contains("frame 1"),
            "{}",
            warnings[0]
        );
        assert!(h.app.video().is_none());
        assert_eq!(audio.calls, ["stop 100", "fade-out 200 1000"]);
        assert_eq!(video_texture(&h).map(|(_, r, _)| r), Some(0));
        let stops: Vec<_> = h
            .app
            .report(S1024, 40)
            .effects
            .into_iter()
            .filter(|(_, e)| matches!(e, UiEffect::StopVideo { .. }))
            .collect();
        assert_eq!(
            stops,
            [(
                40,
                UiEffect::StopVideo {
                    path: "data/videos/t.bik".into()
                }
            )]
        );
        audio.calls.clear();
        for now in [56, 72, 88] {
            run(&mut h, &mut audio, &vfs, now);
        }
        assert_eq!(audio.calls, ["restart"]);
        assert!(h.app.queue().is_empty());
        // Error already at frame 0: the video is dropped like one that cannot be opened.
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        h.set_video_opener(fake_opener(&log, true, Some(0)));
        h.app.play_video_by_name("t").unwrap();
        capture_warnings();
        for now in [0, 16, 32, 48] {
            run(&mut h, &mut audio, &vfs, now);
        }
        assert_eq!(take_warnings().len(), 1);
        assert_eq!(audio.calls, ["restart"]);
        assert!(h.app.video().is_none() && h.app.queue().is_empty());
    }

    #[test]
    fn host_switches_videos_and_their_sound() {
        // A new video during a running one (`FUN_0045D560` stops first, `0x0045D584`): sound of the old one off, the
        // new one begins at frame 0 with its own sound; the late `StopVideo` of the old one does not close the new one.
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        h.set_video_opener(fake_opener(&log, true, None));
        h.app.play_video_by_name("a").unwrap();
        run(&mut h, &mut audio, &vfs, 0);
        run(&mut h, &mut audio, &vfs, 40);
        audio.calls.clear();
        let b = "data/videos/b.bik";
        h.apply(
            vec![UiEffect::PlayVideo {
                path: b.into(),
                abortable: true,
            }],
            &mut audio,
            &vfs,
        );
        assert_eq!(
            audio.calls,
            [
                "stop 100",
                "fade-out 200 1000",
                "video-track data/videos/b.bik",
                "stop-menu-music",
                "video-speech data/sprache/b.ogg"
            ]
        );
        let v = h.app.video().expect("b running").clone();
        assert_eq!((v.path.as_str(), v.started_ms), (b, 40));
        assert_eq!(video_texture(&h).map(|(_, r, red)| (r, red)), Some((2, 0)));
        audio.calls.clear();
        run(&mut h, &mut audio, &vfs, 56);
        assert!(audio.calls.is_empty(), "{:?}", audio.calls);
        assert_eq!(h.app.video().map(|v| v.path.as_str()), Some(b));
        run(&mut h, &mut audio, &vfs, 80);
        assert_eq!(video_texture(&h).map(|(_, _, red)| red), Some(1));
        run(&mut h, &mut audio, &vfs, 160);
        assert!(h.app.video().is_none());
        assert_eq!(audio.calls, ["stop 101", "fade-out 201 1000"]);
        // The queue of "a" (`0x1A`, `AfterProbeVideo`) does not hang.
        audio.calls.clear();
        for now in [176, 192, 208] {
            run(&mut h, &mut audio, &vfs, now);
        }
        assert_eq!(audio.calls, ["restart"]);
        assert!(h.app.queue().is_empty());
        let opened = log.borrow().opened.clone();
        assert_eq!(opened, ["data/videos/a.bik", b]);
    }

    #[test]
    fn pointer_is_hidden_during_the_start_sequence() {
        // Start sequence: the pointer is off (`0x004C96D9`) until `0x24` turns it on; a key aborts the first video,
        // the others run to the end, each with its own speech track.
        let (_dir, vfs) = start_world();
        let home = tempfile::tempdir().unwrap();
        let log = Rc::new(RefCell::new(FakeVideoLog::default()));
        let mut audio = FakeAudio::default();
        let mut h = start_host(&vfs, home.path(), &audio);
        h.set_video_opener(fake_opener(&log, false, None));
        assert!(h.pointer_quad().is_none());
        let mut now = 0;
        while !h.app.menu().open_names().contains(&"start".to_string()) {
            if now == 48 {
                h.on_key(KeyCode::KeyX, true, Some(u16::from(b'x')), S1024, now);
                h.on_key(KeyCode::KeyX, false, None, S1024, now);
            }
            run(&mut h, &mut audio, &vfs, now);
            if h.app.video().is_some() {
                assert!(
                    h.pointer_quad().is_none(),
                    "pointer during video at {now} ms"
                );
            }
            now += 16;
            assert!(now < 2_000, "start not open");
        }
        assert!(h.pointer_quad().is_some());
        assert_eq!(log.borrow().opened, opennml_ui::video::INTRO_VIDEOS);
        let speech: Vec<&String> = audio
            .calls
            .iter()
            .filter(|c| c.starts_with("video-speech"))
            .collect();
        assert_eq!(
            speech,
            [
                "video-speech data/sprache/NML_Logo_CDV.ogg",
                "video-speech data/sprache/NML_Logo_Related.ogg",
                "video-speech data/sprache/intro.ogg"
            ]
        );
        let fades: Vec<&String> = audio
            .calls
            .iter()
            .filter(|c| c.starts_with("fade-out"))
            .collect();
        assert_eq!(
            fades,
            [
                "fade-out 200 1000",
                "fade-out 201 1000",
                "fade-out 202 1000"
            ]
        );
        // The first video ended by key after frame 1 (at 48 ms), the others after all three frames.
        let images: Vec<(String, u32)> = log.borrow().images.clone();
        let per_video = |p: &str| images.iter().filter(|(q, _)| q == p).count();
        assert_eq!(per_video(opennml_ui::video::INTRO_VIDEOS[0]), 2);
        assert_eq!(per_video(opennml_ui::video::INTRO_VIDEOS[1]), 3);
        assert_eq!(per_video(opennml_ui::video::INTRO_VIDEOS[2]), 3);
    }

    #[test]
    fn video_speech_without_file_is_silent() {
        // If the speech track is missing in the VFS, there is only the Bink sound: `None` without a warning
        // (`PlayMemoryStream` returns -1, re/video, section "Opening" 6).
        let (_dir, vfs) = world();
        let mut audio = AudioEngine::open(&vfs, false);
        capture_warnings();
        assert_eq!(
            MenuAudio::play_video_speech(&mut audio, &vfs, "data/sprache/t.ogg"),
            None
        );
        assert!(take_warnings().is_empty());
    }

    #[test]
    fn shutdown_keeps_the_saved_menu_level_of_a_command_line_run() {
        // `--host`/`--join` set `IsMenuLevel` to off only in memory; the file keeps the stored value as long as the
        // checkbox stays off.
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let h = host(&vfs, home.path(), &audio);
        let path = h.settings_path().unwrap().to_path_buf();
        let mut config = h.app.config().clone();
        config.is_menu_level = true;
        save_settings(&path, &config).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(load_settings(&path, &vfs).is_menu_level);
        let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        config.is_menu_level = false;
        let mut run = MenuHost::new(
            &vfs,
            layer,
            config,
            MenuOptions::default(),
            &audio,
            Some(path.clone()),
        )
        .unwrap();
        run.keep_saved_menu_level(true);
        run.shutdown();
        assert_eq!(std::fs::read(&path).unwrap(), before, "file unchanged");
    }

    // QUIRK O-32: kept
    #[test]
    fn shutdown_writes_the_settings() {
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let path = h.settings_path().unwrap().to_path_buf();
        assert!(!path.exists());
        h.shutdown();
        assert_eq!(load_settings(&path, &vfs), *h.app.config());
        // An unwritable location (a folder in the temporary directory) reports only a warning and leaves nothing
        // behind.
        let blocked = home.path().join("folder");
        std::fs::create_dir(&blocked).unwrap();
        let mut bad = host(&vfs, home.path(), &audio);
        bad.settings = Some(blocked.clone());
        bad.shutdown();
        assert!(blocked.is_dir());
        assert!(std::fs::read_dir(&blocked).unwrap().next().is_none());
        assert!(!home.path().join("folder.tmp").exists());
    }

    /// Small installation with `SELECTPROFILE`, `INFOFRAME` and `SINGLEPLAYERMENU` (names as in the original, without
    /// images and flight), plus the font and `mousepointer.ini` of the small world.
    fn profile_world() -> (tempfile::TempDir, Vfs) {
        use crate::testutil::{LOCALISATION, fnt, install, tga, utf16};
        let button = |name: &str, extent: &str| {
            format!(
                "\tBUTTON {name}\n\t\tFLAGS MENUBUTTON_TEXT /FLAGS\n\t\tTEXT 1\n\t\tEXTENT {extent}\n\t/BUTTON\n"
            )
        };
        let select = format!(
            "FRAME SELECTPROFILE\n\tFLAGS MENUFRAME_NOFRAME /FLAGS\n\tEXTENT 0 0 1280 1024\n\
             \tEDITFIELD PROFILENAME\n\t\tEXTENT 10 33 500 90\n\t/EDITFIELD\n\
             \tLISTBOX PROFILELIST\n\t\tEXTENT 10 128 500 550\n\t/LISTBOX\n{}{}{}{}/FRAME\n",
            button("OK", "10 585 250 635"),
            button("CREATE", "270 585 500 635"),
            button("CANCEL", "10 650 250 700"),
            button("DELETE", "270 650 500 700"),
        );
        let info = format!(
            "FRAME INFOFRAME\n\tFLAGS MENUFRAME_NOFRAME /FLAGS\n\tEXTENT 600 0 1280 400\n{}{}/FRAME\n",
            button("OK", "10 10 200 60"),
            button("CANCEL", "10 100 200 150"),
        );
        let single = "FRAME SINGLEPLAYERMENU\n\tFLAGS MENUFRAME_NOFRAME /FLAGS\n\tEXTENT 0 0 1280 1024\n/FRAME\n";
        let pointers = "MOUSEPOINTERID 7\r\nOFFSET 2 2\r\nNORMAL data/pointer/pointer-normal.tga\r\n\
                        /MOUSEPOINTERID\r\n";
        let map = b"SELECTPROFILE data/men\xFC/selectprofile.dlg\r\n\
                    INFOFRAME data/men\xFC/infoframe.dlg\r\n\
                    SINGLEPLAYERMENU data/men\xFC/singleplayermenu.dlg\r\n"
            .to_vec();
        install(&[
            (
                "data/txt/localisation.deu",
                LOCALISATION.as_bytes().to_vec(),
            ),
            ("data/txt/t.txt", utf16("1=OK\r\n")),
            ("data/fonts/Test_21_0_0_0_0.fnt", fnt()),
            ("data/fonts/Test_21_0_0_0_0.tga", tga(256, 256)),
            ("data/menü/dialogs.map", map),
            ("data/menü/selectprofile.dlg", select.into_bytes()),
            ("data/menü/infoframe.dlg", info.into_bytes()),
            ("data/menü/singleplayermenu.dlg", single.as_bytes().to_vec()),
            ("data/gifcon/mousepointer.ini", pointers.as_bytes().to_vec()),
            ("data/pointer/pointer-normal.tga", tga(5, 7)),
        ])
    }

    /// Frames with effects until the UI rests, and one more.
    fn settle(h: &mut MenuHost, audio: &mut FakeAudio, vfs: &Vfs, now: &mut u64) {
        for _ in 0..200 {
            run(h, audio, vfs, *now);
            *now += 16;
            let r = h.app.report(S1024, *now);
            if !r.busy && r.queued == 0 && !h.app.queue().waiting() {
                break;
            }
        }
        run(h, audio, vfs, *now);
        *now += 16;
    }

    /// Left click on an element via the window inputs, then rest.
    fn click_at(
        h: &mut MenuHost,
        audio: &mut FakeAudio,
        vfs: &Vfs,
        now: &mut u64,
        dialog: &str,
        element: &str,
    ) {
        let (x, y) = h
            .app
            .element_center(dialog, element, S1024)
            .unwrap_or_else(|e| panic!("{e}"));
        for ev in [
            InputEvent::CursorMoved {
                x: f64::from(x),
                y: f64::from(y),
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: true,
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: false,
            },
        ] {
            h.on_input(&ev, S1024, *now);
        }
        settle(h, audio, vfs, now);
    }

    fn written(path: &Path) -> opennml_formats::profiles::ProfilesFile {
        let text = decode_cp1252(&std::fs::read(path).expect("profiles.ini written"));
        opennml_formats::profiles::parse_profiles(&text).unwrap()
    }

    #[test]
    fn profiles_written_on_each_action() {
        // Creating, choosing and deleting write `profiles.ini` next to the settings file (deviating from the
        // original), each synchronously via `.tmp`.
        let (_dir, vfs) = profile_world();
        let home = tempfile::tempdir().unwrap();
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let path = home.path().join("OpenNML").join("profiles.ini");
        let mut now = 0;
        h.app.open_by_name("selectprofile").unwrap();
        settle(&mut h, &mut audio, &vfs, &mut now);
        assert!(!path.exists(), "nothing created without a profile action");
        click_at(
            &mut h,
            &mut audio,
            &vfs,
            &mut now,
            "selectprofile",
            "profilename",
        );
        for c in "Anna".chars() {
            h.on_key(KeyCode::KeyA, true, Some(c as u16), S1024, now);
            h.on_key(KeyCode::KeyA, false, None, S1024, now);
        }
        settle(&mut h, &mut audio, &vfs, &mut now);
        click_at(
            &mut h,
            &mut audio,
            &vfs,
            &mut now,
            "selectprofile",
            "create",
        );
        let file = written(&path);
        assert_eq!(file.profiles.len(), 1);
        assert_eq!(file.current, -1);
        assert!(!path.with_file_name("profiles.ini.tmp").exists());
        // OK: aktiv.
        click_at(&mut h, &mut audio, &vfs, &mut now, "selectprofile", "ok");
        assert_eq!(written(&path).current, 0);
        assert_eq!(h.app.menu().open_names(), ["singleplayermenu"]);
        // Deleting via the confirmation prompt.
        h.app.open_by_name("selectprofile").unwrap();
        settle(&mut h, &mut audio, &vfs, &mut now);
        click_at(
            &mut h,
            &mut audio,
            &vfs,
            &mut now,
            "selectprofile",
            "delete",
        );
        click_at(&mut h, &mut audio, &vfs, &mut now, "infoframe", "ok");
        let file = written(&path);
        assert!(file.profiles.is_empty());
        assert_eq!((file.current, file.next_guid), (-1, 1));
        assert_eq!(h.app.profiles().file, file);
        // A new host reads the file.
        let again = host(&vfs, home.path(), &audio);
        assert_eq!(again.app.profiles().file, file);
    }

    // QUIRK O-29: kept
    #[test]
    fn deleting_a_profile_removes_only_its_saves() {
        // `FUN_004C6D70(g)`: only `savegame/*.<GUID %03X>`, not the subfolders (O-29); extension case-insensitive like
        // `FindFirstFile`, folders stay.
        let (_dir, vfs) = world();
        let home = tempfile::tempdir().unwrap();
        let mut audio = FakeAudio::default();
        let mut h = host(&vfs, home.path(), &audio);
        let saves = home.path().join("OpenNML").join("savegame");
        std::fs::create_dir_all(saves.join("multiplayer")).unwrap();
        std::fs::create_dir_all(saves.join("autoplay")).unwrap();
        std::fs::create_dir_all(saves.join("folder.00A")).unwrap();
        for name in [
            "one.000",
            "two.000",
            "foreign.001",
            "ten.00A",
            "small.00a",
            "long.0000",
            "multiplayer/m.000",
            "autoplay/a.000",
        ] {
            std::fs::write(saves.join(name), b"x").unwrap();
        }
        let beside = home.path().join("OpenNML").join("beside.000");
        std::fs::write(&beside, b"x").unwrap();
        h.apply(
            vec![
                UiEffect::DeleteProfileSaves { guid: 0 },
                UiEffect::DeleteProfileSaves { guid: 10 },
            ],
            &mut audio,
            &vfs,
        );
        let mut left: Vec<String> = std::fs::read_dir(&saves)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "autoplay",
                "folder.00A",
                "foreign.001",
                "long.0000",
                "multiplayer"
            ]
        );
        assert!(saves.join("multiplayer").join("m.000").exists());
        assert!(saves.join("autoplay").join("a.000").exists());
        assert!(beside.exists());
        // Without a settings file: nothing deleted, nothing written.
        let layer = UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let config = crate::testutil::test_config(&vfs);
        let mut memory =
            MenuHost::new(&vfs, layer, config, MenuOptions::default(), &audio, None).unwrap();
        memory.apply(
            vec![
                UiEffect::WriteProfiles,
                UiEffect::DeleteProfileSaves { guid: 1 },
            ],
            &mut audio,
            &vfs,
        );
        assert!(saves.join("foreign.001").exists());
        assert!(!home.path().join("OpenNML").join("profiles.ini").exists());
    }
}
