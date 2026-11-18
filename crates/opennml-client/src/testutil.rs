//! Shared test installation of the crate (tests only): text file in UTF-16, font in a 16 x 16 grid, white TGA images, a
//! small installation with a menu dialog and `mousepointer.ini` in a temporary directory, a fake sound output for
//! the menu host, a fake video ([`FakeVideo`]), a fake match ([`FakeSession`]) and a capture of the warnings per
//! thread.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;

use image::RgbaImage;
use opennml_audio::decode::PcmStream;
use opennml_audio::manager::SoundId;
use opennml_audio::volume::Category;
use opennml_rda::Vfs;
use opennml_rda::testutil::{TestFile, build_rda};
use opennml_ui::video::VideoInfo;

use opennml_sim::rules::Rules;
use opennml_sim::setup::MatchSetup;
use opennml_ui::coords::Screen;

use anyhow::Context;
use opennml_formats::engine_ini::EngineConfig;

use crate::game::hotkeys::{HotkeyCommand, KeyContext};
use crate::game::input_gate::{InputGate, sim_gate};
use crate::game::map::{MapLoad, MapSource};
use crate::game::net_driver::{NetDriver, NetFrame, NetPaths};
use crate::game::options::{Controls, ViewOptions};
use crate::game::pause::PauseState;
use crate::game::presentation::{HostRequest, ScriptPresentation};
use crate::game::selection::Selection;
use crate::game::sim_host::{SimHost, plain_testkit};
use crate::game_flow::{SessionView, controls, view_options};
use crate::game_save::{CameraSave, GameSaveFile};
use crate::game_session::{LoadPhase, SessionClock};
use crate::keys::InputEvent;
use crate::menu_host::MenuAudio;
use crate::video::{VideoFile, VideoOpener};

/// Settings of the test scaffolds: the defaults with `IsMenuLevel 0`. With `1` (the default of `Engine.ini`) `START`
/// and `OPTIONS` build their menu level version and send `0x21` instead of `0x20`; the existing tests check the path
/// without a menu level, the section `menu_level` switches it on itself.
pub(crate) fn test_config(vfs: &Vfs) -> EngineConfig {
    EngineConfig {
        is_menu_level: false,
        ..crate::settings::default_config(vfs)
    }
}

/// Text file of the installation: UTF-16 LE with BOM.
pub(crate) fn utf16(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    out
}

/// `.fnt` in a 16 x 16 grid: every character 10 wide, 15 high.
pub(crate) fn fnt() -> Vec<u8> {
    let mut out = String::from("FONTFILE V1.0\r\n\r\n[CHARINFOS]\r\n");
    for c in 0..256u32 {
        let (u, v) = ((c % 16) as f32 / 16.0, (c / 16) as f32 / 16.0);
        out.push_str(&format!(
            "{c} 10 15 {u:.6} {:.6} {v:.6} {:.6}\r\n",
            u + 1.0 / 16.0,
            v + 1.0 / 16.0
        ));
    }
    out.into_bytes()
}

/// TGA type 2, 32 bits, `w` x `h`, white.
pub(crate) fn tga(w: u16, h: u16) -> Vec<u8> {
    let mut tga = vec![0u8; 18];
    tga[2] = 2;
    tga[12..14].copy_from_slice(&w.to_le_bytes());
    tga[14..16].copy_from_slice(&h.to_le_bytes());
    tga[16] = 32;
    tga.extend(std::iter::repeat_n(
        255u8,
        usize::from(w) * usize::from(h) * 4,
    ));
    tga
}

/// Installation from `files` (one compressed archive `NML0.RDA`) in a temporary directory.
pub(crate) fn install(files: &[(&str, Vec<u8>)]) -> (tempfile::TempDir, Vfs) {
    let entries: Vec<TestFile> = files
        .iter()
        .map(|(path, data)| TestFile {
            path,
            data,
            compress: true,
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("NML0.RDA"), build_rda(&entries)).unwrap();
    let vfs = Vfs::open_game_dir(dir.path()).unwrap();
    (dir, vfs)
}

/// Language file with a text file `data\txt\t.txt` and the font `MenuButton` (relative 27: 21 px at 1024).
pub(crate) const LOCALISATION: &str = "TextDatei data\\txt\\t.txt\n\
Font MenuButton\nRelativeFontSize 27\nTextureFont\nFontName Test\n";

/// Small installation: a font, a dialog `TEST` (music, a button, an input field), `mousepointer.ini` with pointer 7
/// (5 x 7, `OFFSET 2 2`), pointer 6 without an image (the file is missing) and pointer 9 (`OFFSET 3 4`) with a normal
/// (4 x 4) and a green image (6 x 6), whose red file is missing.
pub(crate) fn world() -> (tempfile::TempDir, Vfs) {
    world_with(false)
}

/// Like [`world`], plus the main menu `START` (an empty frame) for the start sequence.
pub(crate) fn start_world() -> (tempfile::TempDir, Vfs) {
    world_with(true)
}

fn world_with(start: bool) -> (tempfile::TempDir, Vfs) {
    let dlg = "FRAME TEST\n\tFLAGS MENUFRAME_NOFRAME MENUFRAME_MUSIC /FLAGS\n\tEXTENT 0 0 1280 1024\n\
\tMUSIC DATA/MUSIC/X.OGG\n\tBUTTON OK\n\t\tFLAGS MENUBUTTON_TEXT /FLAGS\n\t\tTEXT 1\n\
\t\tEXTENT 100 100 300 200\n\t/BUTTON\n\tEDITFIELD NAME\n\t\tEXTENT 100 300 600 360\n\t/EDITFIELD\n/FRAME\n";
    let pointers = "MOUSEPOINTERID 6\r\nOFFSET 9 9\r\nNORMAL data/pointer/p6.tga\r\n/MOUSEPOINTERID\r\n\
MOUSEPOINTERID 7\r\nOFFSET 2 2\r\nNORMAL data/pointer/pointer-normal.tga\r\n/MOUSEPOINTERID\r\n\
MOUSEPOINTERID 9\r\nOFFSET 3 4\r\nNORMAL data/pointer/p9.tga\r\nRED data/pointer/p9-red.tga\r\n\
GREEN data/pointer/p9-green.tga\r\n/MOUSEPOINTERID\r\n";
    let mut map = b"TEST data/men\xFC/test.dlg\r\n".to_vec();
    if start {
        map.extend_from_slice(b"START data/men\xFC/start.dlg\r\n");
    }
    let start_dlg =
        "FRAME START\n\tFLAGS MENUFRAME_NOFRAME /FLAGS\n\tEXTENT 0 0 1280 1024\n/FRAME\n";
    install(&[
        (
            "data/txt/localisation.deu",
            LOCALISATION.as_bytes().to_vec(),
        ),
        ("data/txt/t.txt", utf16("1=OK\r\n")),
        ("data/fonts/Test_21_0_0_0_0.fnt", fnt()),
        ("data/fonts/Test_21_0_0_0_0.tga", tga(256, 256)),
        ("data/menü/dialogs.map", map),
        ("data/menü/test.dlg", dlg.as_bytes().to_vec()),
        ("data/menü/start.dlg", start_dlg.as_bytes().to_vec()),
        ("data/gifcon/mousepointer.ini", pointers.as_bytes().to_vec()),
        ("data/pointer/pointer-normal.tga", tga(5, 7)),
        ("data/pointer/p9.tga", tga(4, 4)),
        ("data/pointer/p9-green.tga", tga(6, 6)),
    ])
}

/// Fake sound output of the menu host: records the calls.
#[derive(Debug, Default)]
pub(crate) struct FakeAudio {
    pub(crate) calls: Vec<String>,
    pub(crate) speech_running: bool,
    /// Asked music lengths (`music_length_ms` takes `&self`).
    pub(crate) lengths_asked: RefCell<Vec<String>>,
    /// Started video sounds: the n-th Bink sound gets the ID `100 + n`, the n-th speech track `200 + n`.
    video_tracks: u32,
    video_speeches: u32,
    /// Paths without a file: `play_speech` returns `None` for them (the speech of the game).
    missing: Vec<String>,
    /// Started speech lines: the n-th gets the ID `41 + n` (the first 42).
    speeches: u32,
}

impl FakeAudio {
    /// Dummy in which the speech files `paths` are missing.
    pub(crate) fn without(paths: &[&str]) -> Self {
        Self {
            missing: paths.iter().map(|p| (*p).to_string()).collect(),
            ..Self::default()
        }
    }
}

impl MenuAudio for FakeAudio {
    fn play_event_2d(
        &mut self,
        _vfs: &Vfs,
        name: &str,
        flags: u32,
        category: Category,
        looping: bool,
    ) {
        self.calls.push(format!(
            "event {name} {flags:#x} {} {looping}",
            category.name()
        ));
    }
    fn play_speech(&mut self, _vfs: &Vfs, path: &str) -> Option<SoundId> {
        self.calls.push(format!("speech {path}"));
        if self.missing.iter().any(|m| m == path) {
            return None;
        }
        self.speech_running = true;
        self.speeches += 1;
        Some(41 + self.speeches)
    }
    fn play_menu_music_looping(&mut self, _vfs: &Vfs, path: &str, looping: bool) {
        self.calls.push(format!("music {path} {looping}"));
    }
    fn restart_menu_music(&mut self, _vfs: &Vfs) {
        self.calls.push("restart".into());
    }
    fn resume_game_music(&mut self, _vfs: &Vfs) {
        self.calls.push("resume".into());
    }
    fn set_volume(&mut self, category: Category, value: i32) {
        self.calls
            .push(format!("volume {} {value}", category.name()));
    }
    fn set_sound_driver(&mut self, index: i32) {
        self.calls.push(format!("driver {index}"));
    }
    fn sound_drivers(&self) -> Vec<String> {
        Vec::new()
    }
    fn speech_playing(&self, id: SoundId) -> bool {
        self.speech_running && (42..42 + self.speeches).contains(&id)
    }
    fn music_length_ms(&self, _vfs: &Vfs, path: &str) -> Option<u32> {
        self.lengths_asked.borrow_mut().push(path.to_string());
        (path == "DATA/MUSIC/X.OGG").then_some(4321)
    }
    fn stop_menu_music(&mut self) {
        self.calls.push("stop-menu-music".into());
    }
    fn play_video_speech(&mut self, _vfs: &Vfs, path: &str) -> Option<SoundId> {
        self.calls.push(format!("video-speech {path}"));
        self.video_speeches += 1;
        Some(199 + self.video_speeches)
    }
    fn play_video_track(&mut self, _stream: Box<dyn PcmStream>, label: &str) -> Option<SoundId> {
        self.calls.push(format!("video-track {label}"));
        self.video_tracks += 1;
        Some(99 + self.video_tracks)
    }
    fn stop_sound(&mut self, id: SoundId) {
        self.calls.push(format!("stop {id}"));
    }
    fn fade_out_sound(&mut self, id: SoundId, ms: i32) {
        self.calls.push(format!("fade-out {id} {ms}"));
    }
}

/// Silent sound stream of the fake video (two channels, 120 ms, delivers nothing).
struct SilentStream;

impl PcmStream for SilentStream {
    fn channels(&self) -> usize {
        2
    }
    fn read(&mut self, _out: &mut [i16]) -> usize {
        0
    }
    fn length_ms(&self) -> Option<u64> {
        Some(120)
    }
    fn position_ms(&self) -> u64 {
        0
    }
    fn rewind(&mut self) -> bool {
        false
    }
}

/// What the fake videos of an opener experienced.
#[derive(Debug, Default)]
pub(crate) struct FakeVideoLog {
    /// Opened paths in sequence.
    pub(crate) opened: Vec<String>,
    /// Calls of `image_at` (path, frame) in sequence.
    pub(crate) images: Vec<(String, u32)>,
}

/// Fake video: three frames at 25 frames/s, 16 x 16; frame k is a single color with red = k. With `track` there is a
/// silent sound stream; at frame `fail_at` it reports an error.
pub(crate) struct FakeVideo {
    path: String,
    track: bool,
    fail_at: Option<u32>,
    image: RgbaImage,
    log: Rc<RefCell<FakeVideoLog>>,
}

/// Header of the fake video.
pub(crate) const FAKE_VIDEO: VideoInfo = VideoInfo {
    width: 16,
    height: 16,
    frames: 3,
    fps_num: 25,
    fps_den: 1,
};

impl VideoFile for FakeVideo {
    fn info(&self) -> VideoInfo {
        FAKE_VIDEO
    }
    fn image_at(&mut self, k: u32) -> Result<&RgbaImage, String> {
        self.log.borrow_mut().images.push((self.path.clone(), k));
        if self.fail_at == Some(k) {
            return Err(format!("Video {}: frame {k} not decodable", self.path));
        }
        let red = u8::try_from(k).unwrap_or(u8::MAX);
        self.image = RgbaImage::from_pixel(16, 16, image::Rgba([red, 0, 0, 255]));
        Ok(&self.image)
    }
    fn track(&mut self) -> Option<Result<Box<dyn PcmStream>, String>> {
        self.track
            .then(|| Ok(Box::new(SilentStream) as Box<dyn PcmStream>))
    }
}

/// Opener for [`crate::menu_host::MenuHost::set_video_opener`]: every video becomes a [`FakeVideo`] (with a sound
/// stream for `track`, error at frame `fail_at`), the calls end up in `log`.
pub(crate) fn fake_opener(
    log: &Rc<RefCell<FakeVideoLog>>,
    track: bool,
    fail_at: Option<u32>,
) -> VideoOpener {
    let log = Rc::clone(log);
    Box::new(move |_vfs: &Vfs, path: &str| {
        log.borrow_mut().opened.push(path.to_string());
        Ok(Box::new(FakeVideo {
            path: path.to_string(),
            track,
            fail_at,
            image: RgbaImage::new(1, 1),
            log: Rc::clone(&log),
        }) as Box<dyn VideoFile>)
    })
}

/// Capture of the warnings (`log::warn!` and worse) per thread: tests run in parallel, each reads only its own.
struct WarnCapture;

thread_local! {
    static WARNINGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

impl log::Log for WarnCapture {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            WARNINGS.with(|w| w.borrow_mut().push(record.args().to_string()));
        }
    }
    fn flush(&self) {}
}

static CAPTURE: WarnCapture = WarnCapture;

/// Switches the capture on (once per test program) and clears the warnings of this thread.
pub(crate) fn capture_warnings() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if log::set_logger(&CAPTURE).is_ok() {
            log::set_max_level(log::LevelFilter::Warn);
        }
    });
    WARNINGS.with(|w| w.borrow_mut().clear());
}

/// The warnings reported in this thread since [`capture_warnings`].
pub(crate) fn take_warnings() -> Vec<String> {
    WARNINGS.with(|w| std::mem::take(&mut *w.borrow_mut()))
}

/// Fake match for `game_flow`: `SimHost` without a map (`plain_testkit`, rules `Rules::synthetic`) with the time base
/// of `GameSession` ([`SessionClock`]); inputs are only recorded. With `real_levels` it builds the simulation like
/// `GameSession` GPU-free from the map of the installation ([`MapLoad::open`]: `load_level` or the random map from its
/// description, rules, setup from `setup`, [`MapLoad::start_sim`]); a savegame is checked against the rules of its map.
/// Loading fails for `fail_level` (after phase 0, like an unreadable level).
/// Settings and music like [`crate::game_flow::LiveSession`]: `view`/`controls` are the values of the host that a start
/// takes over.
pub(crate) struct FakeSession {
    pub(crate) game: Option<FakeGame>,
    pub(crate) fail_level: Option<String>,
    /// Inputs to the map in sequence.
    pub(crate) inputs: Vec<InputEvent>,
    /// Setup of the last start.
    pub(crate) last_setup: Option<MatchSetup>,
    /// View of the host for the next start (last from [`SessionView::apply_settings`]).
    pub(crate) view: ViewOptions,
    /// Camera control of the host (last from [`SessionView::apply_settings`]).
    pub(crate) controls: Controls,
    /// Calls to the music in sequence: `stop` (menu music before loading), `menu-level` (start of the menu level, the
    /// menu music keeps running), `restart` (after a failed load without a match), `stop-game` (game music at the end
    /// of the game).
    pub(crate) music: Vec<&'static str>,
    /// Real map and rules of the installation instead of the smallest test world.
    pub(crate) real_levels: bool,
    /// Key commands to the match in sequence ([`SessionView::run_hotkey`]; the fake accepts every one).
    pub(crate) hotkeys: Vec<HotkeyCommand>,
    /// Game type from the flow ([`SessionView::set_match_info`]).
    pub(crate) game_type: i32,
    /// Menu state from the flow ([`SessionView::set_match_info`]).
    pub(crate) menu_open: bool,
    /// Keyboard context for `keys.ini` ([`SessionView::key_context`]); `None` = default (type 1, nation 0, selection
    /// 0).
    pub(crate) key_context: Option<KeyContext>,
    /// Last reported game type and game menu state ([`SessionView::set_match_info`]).
    pub(crate) match_info: Option<(i32, bool)>,
    /// Requests to the flow that the next [`SessionView::take_ui_requests`] collects.
    pub(crate) ui_requests: Vec<crate::game_session::UiRequest>,
    /// Chat and tribute masks like `GameSession::masks`; every new match begins with the start state.
    pub(crate) masks: crate::game::diplomacy::Masks,
    /// Number of calls of [`SessionView::end`] (the menu level ends before loading).
    pub(crate) ends: u32,
    /// Number of commands at the last [`SessionView::begin_playback`]; `None` without playback.
    pub(crate) playback_begun: Option<usize>,
}

impl Default for FakeSession {
    /// Empty fake; the game type is -1 like with `GameSession` (campaign, direct start) until the flow sets it
    /// (otherwise `InputGate::pointer_visible` without a start would compute differently from the match).
    fn default() -> Self {
        Self {
            game: None,
            fail_level: None,
            inputs: Vec::new(),
            last_setup: None,
            view: ViewOptions::default(),
            controls: Controls::default(),
            music: Vec::new(),
            real_levels: false,
            hotkeys: Vec::new(),
            game_type: -1,
            menu_open: false,
            key_context: None,
            match_info: None,
            ui_requests: Vec::new(),
            masks: crate::game::diplomacy::Masks::initial(),
            ends: 0,
            playback_begun: None,
        }
    }
}

/// The running match of the fake.
pub(crate) struct FakeGame {
    pub(crate) host: SimHost,
    pub(crate) level: String,
    pub(crate) clock: SessionClock,
    /// View of the match (taken over at the start, `unit_response` per frame).
    pub(crate) view: ViewOptions,
    /// Camera control of the match (taken over at the start, then per frame).
    pub(crate) controls: Controls,
    /// GPU-free part of the presentation of the scripts like in `GameSession` (speed of the cinemascope, view mode,
    /// requests to the flow).
    pub(crate) presentation: ScriptPresentation,
    /// Selection of the host (empty; the fake selects nothing).
    pub(crate) selection: Selection,
    /// Pause bits like `GameSession::pause`: `togglepause` through [`SessionView::run_hotkey`] and
    /// [`SessionView::toggle_pause`]; in the pause only the ticks without world tick. Every new match begins without.
    pub(crate) pause: PauseState,
    /// `running` of the last frame (for [`SimHost::halted`] when switching the pause).
    pub(crate) running: bool,
    /// Turn driver of the network match like `GameSession::net`; `None` in single player.
    pub(crate) net: Option<NetDriver>,
    /// Control entries, dropouts and failure for the flow ([`SessionView::take_net_frame`]).
    pub(crate) net_frame: NetFrame,
    /// Speed messages of the network match `(tick, message 9/10)` instead of the message bar of `GameSession`.
    pub(crate) speed_messages: Vec<(u64, i32)>,
    /// Messages of the simulation to the local slot in the network match `(tick, message)`, e.g. 200 + p (text
    /// 17200 + p "... was eliminated!") instead of the message bar of `GameSession`.
    pub(crate) player_messages: Vec<(u64, i32)>,
    /// How often the presentation was rebuilt after an adopted state (like after loading).
    pub(crate) rebuilds: u32,
    /// Time of the held frames of the menu level in ms that the camera of the scripts catches up in the first running
    /// frame afterwards (like `GameSession::advance_by` from [`SessionClock::take_held_ms`], O-447), per such frame.
    pub(crate) camera_catch_up: Vec<f32>,
    /// Game camera like `GameSession` (for the host part): [`FAKE_CAMERA`] at the start and after centering on the own
    /// slot in the network, the camera of the savegame after resuming.
    pub(crate) camera: CameraSave,
}

impl FakeGame {
    fn new(host: SimHost, level: String, view: &ViewOptions, controls: &Controls) -> Self {
        Self {
            host,
            level,
            clock: SessionClock::default(),
            view: view.clone(),
            controls: controls.clone(),
            presentation: ScriptPresentation::new(view),
            selection: Selection::default(),
            pause: PauseState::default(),
            running: true,
            net: None,
            net_frame: NetFrame::default(),
            speed_messages: Vec::new(),
            player_messages: Vec::new(),
            rebuilds: 0,
            camera_catch_up: Vec::new(),
            camera: FAKE_CAMERA,
        }
    }

    /// One frame of the network match like `GameSession::advance`: clock and ticks independent of `running`, in the
    /// network pause only the ticks without world tick; speed messages and rebuild after the sync from the frame of the
    /// driver ([`NetFrame::into_session`]).
    fn advance_net(&mut self, now: u64, running: bool) -> u32 {
        let Some(net) = self.net.as_mut() else {
            return 0;
        };
        self.running = running;
        self.host.halted = net.halted();
        let (ticks, frame) = if net.pause_shown() {
            let ticks = self.clock.pause_frame(now, self.host.game_speed).1;
            (ticks, net.advance(&mut self.host, 0.0, now))
        } else {
            let dt = self.clock.frame_ms(now, true).unwrap_or(0.0);
            let before = self.host.state.steps;
            let frame = net.advance(&mut self.host, f64::from(dt), now);
            let ticks = self.host.state.steps.saturating_sub(before);
            (u32::try_from(ticks).unwrap_or(u32::MAX), frame)
        };
        self.pause = net.pause_state();
        self.host.halted = net.halted();
        let step = frame.into_session(&mut self.net_frame);
        for change in &step.speed {
            if let Some(id) = change.message() {
                self.speed_messages.push((change.step, id));
            }
        }
        if step.adopted {
            // Like `GameSession::rebuild_after_adopt`: presentation from the adopted state; remembered speed and
            // camera of the scripts stay.
            let tempo = self.presentation.tempo_memory;
            let camera = self.presentation.camera.clone();
            let mission = self.host.state.script.mission.clone();
            self.presentation = ScriptPresentation::new(&self.view);
            self.presentation
                .restore_host(&mission, tempo, &mut self.host);
            self.presentation.camera = camera;
            self.rebuilds += 1;
        }
        let me = self.host.active_player;
        for event in &step.events {
            match event {
                opennml_sim::state::SimEvent::Present(p) => {
                    self.presentation
                        .apply_host(p, &mut self.host, &mut self.selection);
                }
                opennml_sim::state::SimEvent::PlayerMessage { to, id, .. }
                    if to.is_none_or(|to| to == me) =>
                {
                    self.player_messages.push((self.host.state.steps, *id));
                }
                _ => {}
            }
        }
        ticks
    }
}

/// Delay per direction of the network machines of the tests: host (without a line), client 1, client 2.
pub(crate) const NET_DELAYS: [u64; 3] = [0, 40, 130];
/// Frame length of the network tests in ms.
pub(crate) const NET_FRAME_MS: u64 = 25;
/// Fixed local time of the network machines of the tests.
pub(crate) const NET_CLOCK: opennml_net::LocalTime = opennml_net::LocalTime {
    year: 2026,
    month: 11,
    day: 6,
    hour: 9,
    minute: 30,
    second: 0,
};
/// Session name of the network match of the tests.
pub(crate) const NET_SESSION: &str = "Testpartie";

/// Storage locations of the network machine `i` under `dir`.
pub(crate) fn net_paths(dir: &std::path::Path, i: usize) -> NetPaths {
    let dir = dir.join(format!("machine{i}"));
    NetPaths {
        saves: dir.join("savegame"),
        netlog: dir.join("netlog"),
    }
}

fn net_hello(name: &str) -> opennml_net::Hello {
    opennml_net::Hello {
        listen_port: None,
        net_version: opennml_net::NET_VERSION,
        protocol: 1,
        format: 1,
        edition: "test".into(),
        name: name.into(),
        build: "test".into(),
        seat: None,
    }
}

/// Host core of the tests: lobby with `players` open rows and raster `cfg`, rules "everything allowed"; started
/// unchecked with `HostCore::start`.
pub(crate) fn net_host_core(
    hello: opennml_net::Hello,
    cfg: opennml_net::TurnConfig,
    players: u8,
) -> opennml_net::HostCore {
    let session =
        opennml_net::SessionInfo::plain(NET_SESSION, "multiplayer/test.lvl", players, cfg);
    let book = opennml_net::LobbyBook::open(session, &hello.name).expect("valid test session");
    opennml_net::HostCore::new(hello, book, Box::new(opennml_net::AcceptAll))
}

/// Host and two clients of the network match (`testworld::net_host`, slots 0 ... 2) over a [`opennml_net::MemNet`] with
/// the delays [`NET_DELAYS`]: sign-on, start, loading and `Go` only with `pump`, so still without a tick. `now` is the
/// played time (frames of [`NET_FRAME_MS`]) and stands at the end of the setup afterwards.
pub(crate) fn connect_net(
    dir: &std::path::Path,
    now: &mut u64,
) -> (opennml_net::MemNet, Vec<NetDriver>) {
    use crate::game::testworld::{self, NET_SLOTS};
    use opennml_net::{ClientCore, Phase, Seat, StartInfo, TurnConfig};
    let net = opennml_net::MemNet::new();
    let mut drivers = vec![NetDriver::host(
        net_host_core(net_hello("Host"), TurnConfig::default(), 8),
        net_paths(dir, 0),
    )];
    for slot in 1..NET_SLOTS {
        let (h, c) = net.pair(NET_DELAYS[usize::from(slot)]);
        drivers[0].add_link(slot, Box::new(h));
        drivers.push(NetDriver::client(
            ClientCore::new(net_hello(&format!("Client {slot}"))),
            Box::new(c),
            net_paths(dir, usize::from(slot)),
        ));
    }
    for d in &mut drivers {
        d.set_fixed_clock(Some(NET_CLOCK));
    }
    let mut pump_until = |drivers: &mut Vec<NetDriver>, cond: &dyn Fn(&[NetDriver]) -> bool| {
        for _ in 0..2_000 {
            if cond(drivers) {
                return;
            }
            *now += NET_FRAME_MS;
            net.advance_to(*now);
            for d in drivers.iter_mut() {
                d.pump(*now);
            }
        }
        panic!("setup of the network match hangs");
    };
    pump_until(&mut drivers, &|d| {
        d[1..]
            .iter()
            .all(|d| d.client_core().is_some_and(|c| c.peer().is_some()))
    });
    let package = {
        let sim = testworld::net_host(0, true);
        opennml_sim::setup::start_package(&sim.rules, &sim.setup)
    };
    let info = |peer: u8| StartInfo {
        package: package.clone(),
        seats: std::array::from_fn(|slot| Seat {
            peer: (slot < usize::from(NET_SLOTS)).then_some(slot as u8),
            name: format!("P{slot}"),
        }),
        your_slot: peer,
        spacing: 8,
        lead: 4,
        game_speed: crate::game::options::DEFAULT_GAME_SPEED,
        session: NET_SESSION.into(),
        saved: None,
    };
    drivers[0].host_core_mut().expect("Host").start(info);
    pump_until(&mut drivers, &|d| {
        d.iter().all(|d| d.status().phase == Phase::Loading)
    });
    for d in &mut drivers {
        d.loaded(None);
    }
    pump_until(&mut drivers, &|d| {
        d.iter().all(|d| d.status().phase == Phase::Running)
    });
    (net, drivers)
}

/// Camera of the fake match.
pub(crate) const FAKE_CAMERA: CameraSave = CameraSave {
    from: [100.0, 2000.0, -300.0],
    at: [800.0, 150.0, 900.0],
    fov: 0.75,
};

impl FakeSession {
    fn begin(&mut self, level: &str, progress: &mut dyn FnMut(LoadPhase)) -> anyhow::Result<()> {
        self.begin_with(level, progress, "stop")
    }

    /// Begin loading: `music` notes `call` (`stop` before a start or load, `menu-level` for the menu level).
    fn begin_with(
        &mut self,
        level: &str,
        progress: &mut dyn FnMut(LoadPhase),
        call: &'static str,
    ) -> anyhow::Result<()> {
        self.music.push(call);
        progress(LoadPhase::Start);
        if self.fail_level.as_deref() == Some(level) {
            anyhow::bail!("Level {level} not readable");
        }
        progress(LoadPhase::LevelRead);
        Ok(())
    }

    /// Start like [`SessionView::start`]; `music` notes `call`.
    fn start_source(
        &mut self,
        vfs: &Vfs,
        source: &MapSource,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        progress: &mut dyn FnMut(LoadPhase),
        call: &'static str,
    ) -> anyhow::Result<()> {
        let level = source.label();
        self.begin_with(&level, progress, call)?;
        let host = if self.real_levels {
            // Like `GameSession::start_source` without graphics: fixed map through `load_level`, random map from the
            // description (`MapLoad::open`).
            let mut load = MapLoad::open(vfs, source)?;
            let mut last = None;
            load.start_sim(vfs, game_speed, |rules| {
                let setup = setup(rules)?;
                last = Some(setup.clone());
                Ok(setup)
            })?;
            self.last_setup = last;
            load.sim.context("rules of the map not loaded")?
        } else {
            let mut host = plain_testkit::host();
            self.last_setup = Some(setup(&host.rules)?);
            host.game_speed = game_speed;
            host
        };
        progress(LoadPhase::SimReady);
        progress(LoadPhase::RenderReady);
        self.masks = crate::game::diplomacy::Masks::initial();
        self.game = Some(FakeGame::new(host, level, &self.view, &self.controls));
        Ok(())
    }

    pub(crate) fn steps(&self) -> u64 {
        self.game.as_ref().map_or(0, |g| g.host.state.steps)
    }

    pub(crate) fn checksum(&self) -> u64 {
        self.game.as_ref().map_or(0, |g| g.host.checksum())
    }

    /// Network match without loading: host `host` in turn mode and its driver `net` (after `Go`).
    pub(crate) fn start_net(&mut self, host: SimHost, net: NetDriver) {
        let level = host.rules.level_path.clone();
        let mut game = FakeGame::new(host, level, &self.view, &self.controls);
        game.net = Some(net);
        self.masks = crate::game::diplomacy::Masks::initial();
        self.game = Some(game);
    }

    /// Focus loss like `GameSession::focus_lost`: in the network a pause request ([`NetDriver::focus_lost`]), otherwise
    /// `togglepause` without a visible pause.
    pub(crate) fn focus_lost(&mut self) -> bool {
        if let Some(net) = self.net_mut() {
            return net.focus_lost();
        }
        if self.pause().shown {
            return false;
        }
        self.toggle_pause()
    }
}

impl SessionView for FakeSession {
    fn start(
        &mut self,
        vfs: &Vfs,
        source: &MapSource,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()> {
        self.start_source(vfs, source, setup, game_speed, progress, "stop")
    }

    fn start_menu_level(
        &mut self,
        vfs: &Vfs,
        level: &str,
        setup: &mut dyn FnMut(&Rules) -> anyhow::Result<MatchSetup>,
        game_speed: u8,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()> {
        let source = MapSource::Level(level.into());
        self.start_source(vfs, &source, setup, game_speed, progress, "menu-level")
    }

    fn resume(
        &mut self,
        vfs: &Vfs,
        save: GameSaveFile,
        progress: &mut dyn FnMut(LoadPhase),
    ) -> anyhow::Result<()> {
        let level = save.sim.start.level.clone();
        self.begin(&level, progress)?;
        let mut host = if self.real_levels {
            // Like `GameSession::resume_from`: map from the start package (`MapSource::of_start`), a random map
            // regenerated from its description, not `start.level` as a file.
            let mut load = MapLoad::open(vfs, &MapSource::of_start(&save.sim.start))?;
            let setup = save.sim.start.setup.clone();
            load.start_sim(vfs, save.host.game_speed, |_| Ok(setup))?;
            load.sim.context("rules of the map not loaded")?
        } else {
            plain_testkit::host()
        };
        save.check(&host.rules)?;
        host.game_speed = save.host.game_speed;
        host.load(save.sim, save.host.log.clone());
        let mut presentation = ScriptPresentation::new(&self.view);
        let mission = host.state.script.mission.clone();
        presentation.restore_host(&mission, save.host.cinemascope_speed, &mut host);
        presentation.camera = save.host.script_camera;
        self.masks = save.host.masks;
        // Fog switch after the presentation state like `GameSession::resume_from`.
        host.set_fog_show_mode(save.host.fog_show_mode);
        progress(LoadPhase::SimReady);
        progress(LoadPhase::RenderReady);
        let mut game = FakeGame::new(host, level, &self.view, &self.controls);
        game.presentation = presentation;
        game.camera = save.host.camera;
        self.game = Some(game);
        Ok(())
    }

    fn sim(&self) -> Option<&SimHost> {
        self.game.as_ref().map(|g| &g.host)
    }

    fn sim_mut(&mut self) -> Option<&mut SimHost> {
        self.game.as_mut().map(|g| &mut g.host)
    }

    fn advance(&mut self, now: u64, running: bool) -> u32 {
        let Some(g) = self.game.as_mut() else {
            return 0;
        };
        if g.net.is_some() {
            return g.advance_net(now, running);
        }
        // Like `GameSession::advance`: no ticks without `running` or in the pause -> commands immediately.
        g.running = running;
        g.host.halted = !running || g.pause.sim;
        if running && g.pause.sim {
            return g.clock.pause_frame(now, g.host.game_speed).1;
        }
        let (steps, events) = g.clock.advance(&mut g.host, now, running);
        for event in &events {
            if let opennml_sim::state::SimEvent::Present(p) = event {
                g.presentation.apply_host(p, &mut g.host, &mut g.selection);
            }
        }
        // Like `GameSession::advance_by`: the camera of the scripts catches up the held time of the menu level (O-447).
        if running {
            let held = g.clock.take_held_ms();
            if held > 0.0 {
                g.camera_catch_up.push(held);
            }
        }
        steps
    }

    fn hold(&mut self, now: u64) {
        let Some(g) = self.game.as_mut() else {
            return;
        };
        // Like `GameSession::hold`: without ticks, the frame time accumulates for the camera of the scripts.
        g.running = false;
        g.host.halted = true;
        g.clock.hold_frame(now);
    }

    fn input(&mut self, ev: &InputEvent, _screen: Screen, _now: u64) {
        self.inputs.push(*ev);
    }

    fn level(&self) -> &str {
        self.game.as_ref().map_or("", |g| g.level.as_str())
    }

    fn camera(&self) -> CameraSave {
        self.game.as_ref().map_or(FAKE_CAMERA, |g| g.camera)
    }

    fn end(&mut self) {
        self.game = None;
        self.ends += 1;
    }

    fn apply_settings(&mut self, cfg: &EngineConfig) {
        self.view = view_options(cfg, self.view.text_width);
        self.controls = controls(cfg);
        if let Some(game) = self.game.as_mut() {
            game.controls = self.controls.clone();
            game.view.unit_response = self.view.unit_response;
        }
    }

    fn restore_menu_music(&mut self, _vfs: &Vfs) {
        self.music.push("restart");
    }

    fn stop_game_music(&mut self) {
        self.music.push("stop-game");
    }

    fn take_ui_requests(&mut self) -> Vec<crate::game_session::UiRequest> {
        std::mem::take(&mut self.ui_requests)
    }

    fn masks(&self) -> crate::game::diplomacy::Masks {
        self.masks
    }

    fn set_masks(&mut self, masks: crate::game::diplomacy::Masks) {
        self.masks = masks;
    }

    fn take_host_requests(&mut self) -> Vec<HostRequest> {
        self.game
            .as_mut()
            .map_or_else(Vec::new, |g| g.presentation.take_requests())
    }

    fn cinemascope_speed(&self) -> Option<u8> {
        self.game.as_ref().and_then(|g| g.presentation.tempo_memory)
    }

    fn script_camera(&self) -> crate::game::script_camera::ScriptCamera {
        self.game
            .as_ref()
            .map(|g| g.presentation.camera.clone())
            .unwrap_or_default()
    }

    fn input_gate(&self) -> InputGate {
        let camera = self.script_camera();
        InputGate {
            script_camera: camera.running(),
            campaign: self.game_type == -1,
            menu_open: self.menu_open,
            menu_level: self.game_type == opennml_sim::setup::MENU_LEVEL_MODE,
            ..self.sim().map_or_else(InputGate::default, sim_gate)
        }
    }

    fn run_hotkey(&mut self, cmd: &HotkeyCommand, _now: u64) -> bool {
        self.hotkeys.push(*cmd);
        if *cmd == HotkeyCommand::TogglePause {
            self.toggle_pause();
        }
        true
    }

    fn change_game_speed(&mut self, d: i32) -> Option<u8> {
        self.hotkeys.push(HotkeyCommand::ChangeGameSpeed(d));
        let g = self.game.as_mut()?;
        if let Some(net) = g.net.as_mut() {
            // Like `GameSession::change_game_speed`: in the network only the wish of the host (E-265, E-294).
            return net.change_game_speed(g.host.game_speed, d);
        }
        let v = crate::game_session::key_game_speed(g.host.game_speed, d)?;
        g.host.game_speed = v;
        Some(v)
    }

    fn key_context(&self) -> KeyContext {
        self.key_context.unwrap_or_default()
    }

    fn set_match_info(&mut self, game_type: i32, menu_open: bool) {
        self.game_type = game_type;
        self.menu_open = menu_open;
        self.match_info = Some((game_type, menu_open));
    }

    fn pause(&self) -> PauseState {
        self.game
            .as_ref()
            .map_or_else(PauseState::default, |g| g.pause)
    }

    fn toggle_pause(&mut self) -> bool {
        let game_type = self.game_type;
        let Some(g) = self.game.as_mut() else {
            return false;
        };
        if let Some(net) = g.net.as_mut() {
            // Like `GameSession::toggle_pause`: in the network a wish or lifting of the own pause (O-316).
            return net.toggle_pause();
        }
        let changed = g.pause.toggle(game_type);
        g.host.halted = !g.running || g.pause.sim;
        // Like `GameSession::toggle_pause`: when switching on, the waiting commands immediately.
        if changed && g.pause.sim {
            g.host.flush_pending();
        }
        changed
    }

    fn net(&self) -> Option<&NetDriver> {
        self.game.as_ref().and_then(|g| g.net.as_ref())
    }

    fn net_mut(&mut self) -> Option<&mut NetDriver> {
        self.game.as_mut().and_then(|g| g.net.as_mut())
    }

    fn take_net_frame(&mut self) -> NetFrame {
        self.game
            .as_mut()
            .map(|g| std::mem::take(&mut g.net_frame))
            .unwrap_or_default()
    }

    fn attach_net(&mut self, net: NetDriver) {
        match self.game.as_mut() {
            Some(game) => {
                // Like `LiveSession::attach_net`: the camera on the own slot, except in a loaded match.
                if net.start_info().is_none_or(|s| s.saved.is_none()) {
                    game.camera = FAKE_CAMERA;
                }
                game.net = Some(net);
            }
            None => log::error!("Network: driver discarded without a match"),
        }
    }

    fn begin_playback(&mut self, commands: Vec<opennml_sim::save::RecordedCommand>) {
        // Like `GameSession::begin_playback` without a bar: playback mode, fog off, selection empty and locked.
        let Some(g) = self.game.as_mut() else {
            log::error!("Playback discarded without a match");
            return;
        };
        self.playback_begun = Some(commands.len());
        g.host.begin_playback(commands);
        g.host.set_fog_shown(false);
        g.selection.clear();
        g.selection.locked = true;
    }
}
