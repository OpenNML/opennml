//! Sound of the viewer: sound manager, level and menu music, rain, surf, manual combat music,
//! muting, device output and offline mix - all on the virtual clock. In each logic step,
//! [`AudioEngine::advance`] mixes exactly `dt · 44,1` frames (fractions carry over); while
//! paused nothing is mixed and the device gets silence.

use std::path::Path;

use anyhow::Result;
use glam::Vec3;
use nml_formats::sound::MusicConfig;
use nml_rda::Vfs;
use nml_world::environment::{GameMode, MusicAction};

use crate::SAMPLE_RATE;
use crate::capture::{Capture, CaptureSummary};
use crate::db::SoundDb;
use crate::decode::{OggStream, PcmStream};
use crate::driver::{self, SoundDriver};
use crate::manager::{SoundId, SoundManager};
use crate::music::{MenuMusic, MusicManager, menu_track_loops};
use crate::output::Output;
use crate::volume::Category;

/// `SFX_REGEN` as a 2D loop with the music volume (`GameEngine_SetWeather` `0x0043E420`).
pub const RAIN_EVENT: &str = "SFX_REGEN";
pub const RAIN_FLAGS: u32 = 0x14;
/// Surf: action 200, rank 3, one instance per sample, move (`SurfEffect_Tick`).
pub const SURF_ACTION: i32 = 200;
pub const SURF_FLAGS: u32 = 0x113;
/// Dialog speech (`Dialog_PlaySpeech` `0x00436A50`): rank 9 (aborts category 8).
pub const SPEECH_FLAGS: u32 = 0x29;
/// Speech track of a video (`FUN_0045D560`, `PUSH 0x19` at `0x0045D7C7`): rank 9 (first stops all channels of
/// category 8), at most one instance; played with [`Category::Fixed`].
pub const VIDEO_SPEECH_FLAGS: u32 = 0x19;
/// Bink audio track of a video: in the original without an FMOD channel (`binkw32.dll` plays it itself); in the
/// reimplementation a stream without rank and without an instance rule, played with [`Category::Fixed`] (deviation
/// from the original).
pub const VIDEO_TRACK_FLAGS: u32 = 0;

/// Source of a sound for [`AudioEngine::play_sound`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundSource<'a> {
    /// Action of an object type from `sounddatabase.dat`.
    Action { guid: u32, action: i32 },
    /// File (`.wav` or `.ogg`).
    File(&'a str),
}

/// Title music of the menu background levels (`start.dlg`).
pub const TITLE_MUSIC: &str = "data/music/title.ogg";

/// Flags and volume category with which the game plays an action of an object type
/// (re/audio, section "Triggers"): responses (12, 14) and the selection of a unit (100) as
/// speech with the exclusive category 8 (`0x18`), selection of other objects `0x12`, otherwise the
/// flags of the 3D calls, category sound effects.
pub fn action_flags(action: i32, unit: bool) -> (u32, Category) {
    match action {
        12 | 14 => (0x18, Category::Speech),
        100 if unit => (0x18, Category::Speech),
        1 => (0x22, Category::Sfx),
        2..=4 | 7 | 30 | 31 => (0x42, Category::Sfx),
        5 => (0x23, Category::Sfx),
        6 | 11 | 13 => (0x13, Category::Sfx),
        10 | 102 | 105 => (0x11, Category::Sfx),
        101 | 104 => (0x43, Category::Sfx),
        110 => (0x03, Category::Sfx),
        200 => (0x113, Category::Sfx),
        _ => (0x12, Category::Sfx),
    }
}

/// The combat points that a step passes to the music state machine: when forced
/// `max(start, stop + 1)`, otherwise the points of the simulation.
pub fn music_points(config: &MusicConfig, attack_override: bool, points: i32) -> i32 {
    if attack_override {
        config
            .attack_start_points
            .max(config.attack_stop_points + 1)
    } else {
        points
    }
}

/// The sound of the viewer.
#[derive(Debug)]
pub struct AudioEngine {
    pub sound: SoundManager,
    pub music: MusicManager,
    pub menu: MenuMusic,
    output: Option<Output>,
    /// Driver list of the system (`m_DriverID` is an index into it); empty without a device.
    drivers: Vec<SoundDriver>,
    /// Opened with device output: only then does [`Self::set_sound_driver`] create a device.
    device_enabled: bool,
    capture: Option<Capture>,
    mute: bool,
    /// Combat music forced by hand (Shift+F9): combat points above both thresholds.
    attack_override: bool,
    /// Combat points of the active player from the simulation (`SimEvent::AttackPoints`).
    attack_points: i32,
    rain: Option<SoundId>,
    /// Loop of the menu title (`MenuMusic+0xD8`, `!MENUFRAME_DONOTLOOPMUSIC`); `MenuMusic_Restart` plays with it.
    menu_looping: bool,
    /// Fractions of a frame or a millisecond from earlier steps.
    frame_carry: f64,
    ms_carry: f64,
    buffer: Vec<i16>,
}

impl AudioEngine {
    /// Sound with a database and optional device output.
    pub fn new(db: SoundDb, output: Option<Output>) -> Self {
        let music = MusicManager::new(db.music.clone());
        Self {
            sound: SoundManager::new(db),
            music,
            menu: MenuMusic::default(),
            device_enabled: output.is_some(),
            output,
            drivers: Vec::new(),
            capture: None,
            mute: false,
            attack_override: false,
            attack_points: 0,
            rain: None,
            menu_looping: true,
            frame_carry: 0.0,
            ms_carry: 0.0,
            buffer: Vec::new(),
        }
    }

    /// Loads the tables and opens the default device (silent without a device, with a log line).
    pub fn open(vfs: &Vfs, device: bool) -> Self {
        Self::open_with_driver(vfs, device.then_some(0))
    }

    /// Loads the tables; with `driver` (`m_DriverID`) it enumerates the system's drivers and opens the device of the
    /// entry (`SoundManager_CreateInterface`, `SoundManager_InitDevice`); without `driver` it stays without a device.
    pub fn open_with_driver(vfs: &Vfs, driver: Option<i32>) -> Self {
        let mut engine = Self::new(SoundDb::load(vfs), None);
        if let Some(index) = driver {
            engine.drivers = driver::enumerate();
            engine.device_enabled = true;
            engine.set_sound_driver(index);
        }
        engine
    }

    /// Display names of the driver list (`MenuOptions.sound_drivers`, combo box `sounddriver`); empty without a device.
    pub fn sound_drivers(&self) -> Vec<String> {
        driver::names(&self.drivers)
    }

    /// Creates the device anew like `sounddriver` code 4 in the original (`SoundManager_CreateInterface(−1)`,
    /// `SoundManager_InitDevice(index, −1)`): close the old stream, open the device of entry `index`. If the index
    /// lies outside the list (device gone since the last start), it plays through the default device; the
    /// original would stay silent (D-372). Mixer, channels and music keep running. Opened without a device: nothing.
    pub fn set_sound_driver(&mut self, index: i32) {
        if !self.device_enabled {
            return;
        }
        let (key, fallback) = driver::select(&self.drivers, index);
        if fallback {
            log::warn!(
                "Sound: driver {index} not in the list ({} entries), output through the default device",
                self.drivers.len()
            );
        }
        // Close the old stream first: some devices allow only one stream.
        self.output = None;
        self.output = Output::open(&key);
    }

    /// Name of the output device (`None`: without a device).
    pub fn device(&self) -> Option<&str> {
        self.output.as_ref().map(Output::device)
    }

    /// Music at the start of a level (re/audio, section "Music at level start"): fresh
    /// `MusicManager`, queue the nation music of the local player; multiplayer maps start it
    /// (status 1), campaigns run the music actions of their start blocks, menu backgrounds
    /// play the title music.
    pub fn start_level(
        &mut self,
        vfs: &Vfs,
        mode: GameMode,
        nation: Option<usize>,
        actions: &[MusicAction],
    ) {
        self.music.stop_music(&mut self.sound);
        // The menu level behind the main menu (`GameMode::Menu`) lets the running title music continue: `play`
        // below does not restart the same file while a handle is running (`MenuMusic_Play` `0x0045D4C0`).
        // All other modes fade out the menu music.
        if mode != GameMode::Menu {
            self.menu.stop(&mut self.sound);
        }
        self.music = MusicManager::new(self.sound.db.music.clone());
        self.start_nation_music(vfs, mode, nation);
        match mode {
            // `start_nation_music` already starts the playlist of the nation music (multiplayer only, `0x0042EFAC`).
            GameMode::Multiplayer => {}
            GameMode::Menu => self.menu.play(&mut self.sound, vfs, TITLE_MUSIC, true),
            GameMode::Campaign => {
                for &action in actions {
                    self.run_action(vfs, action);
                }
            }
        }
    }

    /// `Game_StartNationMusic` (`0x0042EF90`): nation music of the nation `nation` of the local player. Without a
    /// nation (outside 0-5) nothing happens (`0x0042EF99`/`0x0042EF9B`); otherwise `MusicManager_SelectNation` in every
    /// mode and only in multiplayer (`GameEngine+0x108 > −1`, `0x0042EFAC`) `MusicManager_StartPlaylist`. Runs
    /// at level start ([`Self::start_level`]) and when the local player changes (`GameEngine_SetLocalPlayer`
    /// `0x00430140`, command 28 `0x0047B510`; in the reimplementation only the viewer's player switch).
    pub fn start_nation_music(&mut self, vfs: &Vfs, mode: GameMode, nation: Option<usize>) {
        let Some(n) = nation else {
            return;
        };
        self.music.select_nation(n);
        if mode == GameMode::Multiplayer {
            self.music.start_playlist(&mut self.sound, vfs);
        }
    }

    /// A music action of a level script; an unknown ID in `sound.txt` has no effect.
    pub fn run_action(&mut self, vfs: &Vfs, action: MusicAction) {
        let track = |id: i32| self.sound.db.tracks.get(&id).cloned();
        match action {
            MusicAction::Play(id) => {
                if let Some(path) = track(id) {
                    self.music.play_script(&mut self.sound, vfs, &path);
                }
            }
            MusicAction::Attack(id) => {
                if let Some(path) = track(id) {
                    self.music.set_attack_music(&path);
                }
            }
            MusicAction::AttackStartPoints(points) => self.music.set_attack_start_points(points),
        }
    }

    /// Menu music (`title`, `win`, `lose`, `credits` or a path): the level music fades out
    /// and stays silent afterwards; `win`/`lose` do not loop.
    pub fn play_menu_music(&mut self, vfs: &Vfs, track: &str) {
        let path = if track.contains(['/', '\\', '.']) {
            track.to_string()
        } else {
            format!("data/music/{}.ogg", track.to_ascii_lowercase())
        };
        self.play_menu_music_looping(vfs, &path, menu_track_loops(&path));
    }

    /// Menu music of a frame (`0x160`, `MenuMusic_Play` `0x0045D4C0`): `path` as read from the `.dlg`, `looping` =
    /// `!MENUFRAME_DONOTLOOPMUSIC` of the frame (instead of the name rule [`menu_track_loops`]). The level music fades
    /// out; the same file while a handle is running does not restart (re/audio, section "Menu and game end").
    pub fn play_menu_music_looping(&mut self, vfs: &Vfs, path: &str, looping: bool) {
        self.music.stop_and_reset(&mut self.sound);
        // `MenuMusic_Play` writes `+0xD8` only when it restarts (different file or handle -1).
        let restarts = self.menu.handle.is_none() || self.menu.file.as_deref() != Some(path);
        self.menu.play(&mut self.sound, vfs, path, looping);
        if restarts {
            self.menu_looping = looping;
        }
    }

    /// Restarts the menu music (`0x161`, `MenuMusic_Restart` `0x0045CA20`): stop a running handle (fade out
    /// 1000 ms), restart the same file from 0 with the remembered loop and fade in 1000 ms. Without a previous
    /// title nothing happens (the original started an empty path, which does not play).
    pub fn restart_menu_music(&mut self, vfs: &Vfs) {
        let Some(path) = self.menu.file.clone() else {
            return;
        };
        self.menu.stop(&mut self.sound);
        self.menu
            .play(&mut self.sound, vfs, &path, self.menu_looping);
    }

    /// Length of a music file in ms (`FSOUND_Stream_GetLengthMs` on the opened stream), synchronous: open the loose
    /// file `path` and read the header. `None` if the file is missing, not readable or does not know its length.
    /// Needed for the duration of the credits (`Broadcast::MusicLength`).
    pub fn music_length_ms(vfs: &Vfs, path: &str) -> Option<u32> {
        let file = vfs.loose_file(path)?;
        let stream = std::fs::File::open(&file)
            .map_err(anyhow::Error::from)
            .and_then(|f| OggStream::open(Box::new(f)))
            .inspect_err(|e| log::warn!("Music {path}: length not readable: {e:#}"))
            .ok()?;
        stream
            .length_ms()
            .map(|ms| u32::try_from(ms).unwrap_or(u32::MAX))
    }

    /// Fades out the menu music (`MenuMusic_Stop` `0x0045C9F0`: 1000 ms, then stopped), e.g. at the start of a video
    /// (`FUN_0045D560`); the level music is left untouched.
    pub fn stop_menu_music(&mut self) {
        self.menu.stop(&mut self.sound);
    }

    /// `MusicManager_ResumeMusic` (`0x004E6860`): plays the game music `+0x94` from 0 without a fade. The menu level
    /// has none (`+0x94` empty, no nation): then nothing happens and the title music stays at its frame (video end).
    pub fn resume_game_music(&mut self, vfs: &Vfs) {
        self.music.resume_music(&mut self.sound, vfs);
    }

    /// Fades out all music (level 2000 ms, menu 1000 ms).
    pub fn stop_music(&mut self) {
        self.music.stop_and_reset(&mut self.sound);
        self.menu.stop(&mut self.sound);
    }

    /// Combat points of the active player that every step passes to the music state machine
    /// (re/combat-host 5); the original passes on the last reported value, also while
    /// paused (C-161).
    pub fn set_attack_points(&mut self, points: i32) {
        self.attack_points = points;
    }

    pub fn attack_points(&self) -> i32 {
        self.attack_points
    }

    /// Combat music by hand (Shift+F9): on = in every step combat points above both
    /// thresholds, instead of the points of the simulation; the timers of the state machine still apply.
    pub fn set_attack_override(&mut self, on: bool) {
        self.attack_override = on;
    }

    pub fn attack_override(&self) -> bool {
        self.attack_override
    }

    /// The points that a step passes to the music state machine: the override
    /// `max(start, stop + 1)`, otherwise the combat points of the simulation.
    pub fn music_points(&self) -> i32 {
        music_points(
            self.music.config(),
            self.attack_override,
            self.attack_points,
        )
    }

    /// Rain on/off like `GameEngine_SetWeather`: changes stop the running rain immediately,
    /// rain starts `SFX_REGEN` as a loop with the music volume.
    pub fn set_rain(&mut self, vfs: &Vfs, on: bool) {
        if on == self.rain.is_some() {
            return;
        }
        if let Some(id) = self.rain.take() {
            self.sound.stop(id);
        }
        if on {
            self.rain =
                self.sound
                    .play_event_2d(vfs, RAIN_EVENT, RAIN_FLAGS, Category::Music, true);
            if self.rain.is_none() {
                // No sound, but the "rain" state remains so that it is not retried
                // in every step.
                self.rain = Some(SoundId::MAX);
            }
        }
    }

    pub fn raining(&self) -> bool {
        self.rain.is_some()
    }

    /// Surf sound of a game tick (`SurfEffect_Tick`): action 200 of the GUID at `position`.
    pub fn play_surf(&mut self, vfs: &Vfs, guid: u32, position: Vec3) {
        self.sound.play_object_3d(
            vfs,
            SURF_ACTION,
            SURF_FLAGS,
            position,
            guid as i32,
            -1,
            Category::Sfx,
        );
    }

    /// Action of an object type as a 2D sound (model browser, Enter): flags and category according to
    /// [`action_flags`]; responses (12, 14) come in turn via `responses` (one counter for both,
    /// like `Unit_PlayResponseSound`), all others at random.
    pub fn play_action(
        &mut self,
        vfs: &Vfs,
        guid: u32,
        action: i32,
        unit: bool,
        responses: &mut i32,
    ) -> Option<SoundId> {
        let (flags, category) = action_flags(action, unit);
        let index = if matches!(action, 12 | 14) {
            let i = *responses;
            *responses += 1;
            i
        } else {
            -1
        };
        self.sound
            .play_object_2d(vfs, action, flags, guid as i32, index, category)
    }

    /// Control channel command `play_sound`: `guid` + `action` from the database (flags according to
    /// [`action_flags`], as a 3D sound with `position`, otherwise 2D) or a file (`.ogg` as
    /// dialog speech `0x29` with doubling, `.wav` as a sound effect). Returns whether a sound started
    /// and its ID (3D sounds return none, as in the game).
    pub fn play_sound(
        &mut self,
        vfs: &Vfs,
        source: SoundSource<'_>,
        position: Option<Vec3>,
    ) -> (bool, Option<SoundId>) {
        match source {
            SoundSource::Action { guid, action } => {
                let (flags, category) = action_flags(action, false);
                match position {
                    Some(p) => (
                        self.sound
                            .play_object_3d(vfs, action, flags, p, guid as i32, -1, category),
                        None,
                    ),
                    None => {
                        let id = self.sound.play_object_2d(
                            vfs,
                            action,
                            flags,
                            guid as i32,
                            -1,
                            category,
                        );
                        (id.is_some(), id)
                    }
                }
            }
            SoundSource::File(path) if path.to_ascii_lowercase().ends_with(".ogg") => {
                let id = self
                    .sound
                    .play_memory_stream(vfs, path, SPEECH_FLAGS, Category::Speech)
                    .map(|(id, _)| id);
                (id.is_some(), id)
            }
            SoundSource::File(path) => match position {
                Some(p) => (
                    self.sound.play_file_3d(vfs, path, 0x13, p, Category::Sfx),
                    None,
                ),
                None => {
                    let id = self.sound.play_file_2d(vfs, path, 0x12, Category::Sfx);
                    (id.is_some(), id)
                }
            },
        }
    }

    /// Sound on/off (F9): when muted the device gets nothing, mixer and capture keep running.
    pub fn set_mute(&mut self, mute: bool) {
        self.mute = mute;
        if mute && let Some(output) = &self.output {
            output.clear();
        }
    }

    pub fn mute(&self) -> bool {
        self.mute
    }

    /// Sets the volume slider (0-1000; `SoundManager_SetCategoryVolume`).
    pub fn set_volume(&mut self, category: Category, volume: i32) {
        self.sound
            .set_category_volume(category, volume.clamp(0, crate::volume::MAX_VOLUME));
    }

    /// Starts the offline mix into a WAV file (a running one is ended first).
    pub fn start_capture(&mut self, path: &Path) -> Result<Option<CaptureSummary>> {
        let previous = self.stop_capture()?;
        self.capture = Some(Capture::create(path)?);
        Ok(previous)
    }

    /// Ends the offline mix; `None` if none was running.
    pub fn stop_capture(&mut self) -> Result<Option<CaptureSummary>> {
        self.capture.take().map(Capture::finish).transpose()
    }

    /// Running capture: path and frames written so far.
    pub fn capture_status(&self) -> Option<(&Path, u64)> {
        self.capture.as_ref().map(|c| (c.path(), c.frames()))
    }

    /// One logic step of `dt_ms`: music state machine (whole ms with carry), then mix exactly as many
    /// frames as the time yields; to the capture and - unless muted - to the
    /// device. Returns the number of frames.
    pub fn advance(&mut self, vfs: &Vfs, dt_ms: f32) -> usize {
        let ms = self.ms_carry + f64::from(dt_ms);
        let whole_ms = ms.floor();
        self.ms_carry = ms - whole_ms;
        let points = self.music_points();
        self.music
            .update(&mut self.sound, vfs, whole_ms as i32, points);
        let frames = self.frame_carry + f64::from(dt_ms) * f64::from(SAMPLE_RATE) / 1000.0;
        let whole = frames.floor();
        self.frame_carry = frames - whole;
        let n = whole as usize;
        self.buffer.resize(n * 2, 0);
        self.sound.render(&mut self.buffer);
        if let Some(capture) = &mut self.capture
            && let Err(e) = capture.write(&self.buffer)
        {
            log::error!("Sound capture aborted: {e:#}");
            self.capture = None;
        }
        if !self.mute
            && let Some(output) = &self.output
        {
            output.push(&self.buffer);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::test_db;
    use crate::manager::tests::{test_vfs, tone_stream};
    use crate::music::MusicStatus;

    fn engine() -> AudioEngine {
        AudioEngine::new(test_db(), None)
    }

    #[test]
    fn attack_points_drive_the_music_and_the_override_forces_it() {
        let mut e = engine();
        e.set_attack_points(390);
        assert_eq!(e.music_points(), 390);
        assert_eq!(e.attack_points(), 390);
        e.set_attack_override(true);
        assert!(e.attack_override());
        let c = e.music.config().clone();
        assert_eq!(
            e.music_points(),
            c.attack_start_points.max(c.attack_stop_points + 1)
        );
        e.set_attack_override(false);
        assert_eq!(
            e.music_points(),
            390,
            "without the override the simulation again"
        );
    }

    #[test]
    fn steps_mix_exactly_the_elapsed_time_with_carry() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        let frames: usize = (0..3).map(|_| e.advance(&vfs, 16.5)).sum();
        // 3 * 727.65 = 2182.95 frames.
        assert_eq!(frames, 2182);
        assert_eq!(e.advance(&vfs, 0.05), 3, "carry 0.95 + 2.205");
        assert_eq!(e.sound.frames(), 2185);
    }

    #[test]
    fn captures_are_bit_identical_for_the_same_steps_however_they_are_split() {
        let (dir, vfs) = test_vfs();
        let run = |name: &str, chunks: &[u32]| {
            let mut e = engine();
            e.start_capture(&dir.path().join(name)).unwrap();
            e.sound
                .set_listener(Vec3::ZERO, 0.0, Vec3::new(0.0, 500.0, -500.0));
            e.set_rain(&vfs, true);
            e.play_surf(&vfs, 4104, Vec3::new(600.0, 0.0, 300.0));
            for &n in chunks {
                for _ in 0..n {
                    e.advance(&vfs, 16.0);
                }
            }
            let summary = e.stop_capture().unwrap().unwrap();
            (summary, std::fs::read(dir.path().join(name)).unwrap())
        };
        let (a, bytes_a) = run("a.wav", &[30]);
        let (b, bytes_b) = run("b.wav", &[10, 20]);
        assert_eq!(bytes_a, bytes_b);
        assert_eq!(a.frames, 21_168);
        assert!(a.peak[0] > 0.0 && a.envelope.len() == 5);
        assert_eq!((a.peak, a.rms), (b.peak, b.rms));
    }

    #[test]
    fn browser_actions_use_the_game_flags_and_answers_come_in_turn() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        assert_eq!(action_flags(100, true), (0x18, Category::Speech));
        assert_eq!(action_flags(100, false), (0x12, Category::Sfx));
        assert_eq!(action_flags(101, false), (0x43, Category::Sfx));
        let mut responses = 0;
        let first = e.play_action(&vfs, 2215, 12, true, &mut responses).unwrap();
        assert_eq!(e.sound.voice(first).unwrap().file, "d.wav");
        // Category 8: a second response is rejected as long as the first one is running.
        assert!(
            e.play_action(&vfs, 2215, 12, true, &mut responses)
                .is_none()
        );
        assert_eq!(responses, 2);
        for _ in 0..10 {
            e.advance(&vfs, 10.0);
        }
        let third = e.play_action(&vfs, 2215, 12, true, &mut responses).unwrap();
        assert_eq!(e.sound.voice(third).unwrap().file, "d.wav", "Index 2 % 2");
        assert_eq!(e.sound.voice(third).unwrap().category, Category::Speech);
        assert!(
            e.play_action(&vfs, 2215, 110, false, &mut responses)
                .is_none()
        );
    }

    #[test]
    fn play_sound_takes_database_actions_and_files_in_2d_or_3d() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        let (played, id) = e.play_sound(
            &vfs,
            SoundSource::Action {
                guid: 2215,
                action: 100,
            },
            None,
        );
        assert!(played && id.is_some());
        let (played, id) = e.play_sound(
            &vfs,
            SoundSource::Action {
                guid: 4104,
                action: 200,
            },
            Some(Vec3::new(256.0, 0.0, 0.0)),
        );
        assert_eq!((played, id), (true, None));
        let (played, id) = e.play_sound(&vfs, SoundSource::File("b.wav"), None);
        assert!(played && id.is_some());
        assert_eq!(
            e.play_sound(&vfs, SoundSource::File("missing.ogg"), None),
            (false, None)
        );
        assert_eq!(e.sound.voices().count(), 3);
    }

    #[test]
    fn rain_starts_once_and_stops_with_the_weather_and_mute_keeps_mixing() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        e.set_rain(&vfs, true);
        e.set_rain(&vfs, true);
        assert_eq!(e.sound.voices().count(), 1);
        e.set_mute(true);
        assert_eq!(e.advance(&vfs, 10.0), 441);
        assert_eq!(e.sound.voices().count(), 1);
        e.set_rain(&vfs, false);
        assert!(!e.raining() && e.sound.voices().count() == 0);
    }

    #[test]
    fn campaign_levels_run_their_start_actions_and_attack_music_follows_the_timers() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        e.sound.db.music.playlists[2] = vec!["data/music/spa.ogg".into()];
        e.sound.db.music.attack_start_points = 500;
        e.sound.db.music.attack_start_time = 1000;
        e.sound.db.music.attack_stop_time = 5000;
        e.sound
            .db
            .tracks
            .insert(115, "data/music/spa_attack.ogg".into());
        e.start_level(
            &vfs,
            GameMode::Campaign,
            Some(2),
            &[
                MusicAction::AttackStartPoints(1000),
                MusicAction::Attack(115),
            ],
        );
        assert_eq!(e.music.status, MusicStatus::Normal);
        assert_eq!(e.music.attack, "data/music/spa_attack.ogg");
        e.set_attack_override(true);
        for _ in 0..10 {
            e.advance(&vfs, 100.0);
        }
        assert_eq!(
            e.music.status,
            MusicStatus::Attack,
            "after 1 s above the threshold"
        );
        e.set_attack_override(false);
        for _ in 0..49 {
            e.advance(&vfs, 100.0);
        }
        assert_eq!(e.music.status, MusicStatus::Attack);
        e.advance(&vfs, 100.0);
        assert_eq!(e.music.status, MusicStatus::Normal, "5 s without points");
        // Without music actions a campaign stays silent (status 0).
        e.start_level(&vfs, GameMode::Campaign, Some(2), &[]);
        assert_eq!(
            (e.music.status, e.music.handle),
            (MusicStatus::Script, None)
        );
    }

    #[test]
    fn nation_music_selects_in_every_mode_and_starts_only_in_multiplayer() {
        // `Game_StartNationMusic` (`0x0042EF90`): without a nation nothing, otherwise `SelectNation`; the playlist
        // only in multiplayer. A player switch (command 28) selects anew without a fresh music manager.
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        e.sound.db.music.playlists[2] = vec!["data/music/spa.ogg".into()];
        e.sound.db.music.playlists[4] = vec!["data/music/eng.ogg".into()];
        e.start_level(&vfs, GameMode::Multiplayer, None, &[]);
        assert_eq!(
            (e.music.nation(), e.music.status, e.music.handle),
            (None, MusicStatus::Script, None),
            "no playlist without a nation"
        );
        e.start_level(&vfs, GameMode::Multiplayer, Some(4), &[]);
        assert_eq!(e.music.nation(), Some(4));
        assert_eq!(e.music.current, "data/music/eng.ogg");
        assert_eq!(e.music.status, MusicStatus::Normal);
        e.start_nation_music(&vfs, GameMode::Multiplayer, Some(2));
        assert_eq!(
            (e.music.nation(), e.music.current.as_str(), e.music.status),
            (Some(2), "data/music/spa.ogg", MusicStatus::Normal),
            "player switch starts the playlist of the new nation"
        );
        e.start_nation_music(&vfs, GameMode::Multiplayer, None);
        assert_eq!(
            e.music.nation(),
            Some(2),
            "without a nation everything stays"
        );
        e.start_level(&vfs, GameMode::Campaign, Some(2), &[]);
        e.start_nation_music(&vfs, GameMode::Campaign, Some(4));
        assert_eq!(
            (e.music.nation(), e.music.current.as_str(), e.music.status),
            (Some(4), "data/music/eng.ogg", MusicStatus::Script),
            "campaign: only queued"
        );
    }

    #[test]
    fn video_speech_fades_in_to_full_volume() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        let (id, _) = e
            .sound
            .play_pcm_stream(
                tone_stream(3000),
                "data/sprache/ind-1.ogg",
                VIDEO_SPEECH_FLAGS,
                Category::Fixed,
            )
            .unwrap();
        let volume = |e: &AudioEngine| {
            let slot = e.sound.voices().find(|(_, v)| v.id == id).unwrap().0;
            e.sound.mixer().channel(slot).unwrap().volume()
        };
        assert_eq!(volume(&e), 255, "start with doubling");
        // `FadeInSound(channel, 1000, 3)`: immediately 0, then to `kat(3) = 1998` without doubling.
        e.sound.fade_in(id, 1000, Category::Fixed);
        assert_eq!(volume(&e), 0);
        for _ in 0..50 {
            e.advance(&vfs, 10.0);
        }
        // 500 ms: ftol(500 * 1.998) = 999 -> FMOD 127.
        assert_eq!(volume(&e), 127);
        for _ in 0..50 {
            e.advance(&vfs, 10.0);
        }
        // 1000 ms: 1998 -> FMOD 254 (one step below the start), unchanged afterwards.
        assert_eq!(volume(&e), 254);
        e.set_volume(Category::Speech, 1000);
        for _ in 0..20 {
            e.advance(&vfs, 10.0);
        }
        assert_eq!(volume(&e), 254);
        assert!(!e.sound.voice(id).unwrap().fade.unwrap().active);
    }

    #[test]
    fn video_speech_flags_stop_category_eight() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        assert_eq!((VIDEO_SPEECH_FLAGS, VIDEO_TRACK_FLAGS), (0x19, 0));
        let answer = e
            .sound
            .play_file_2d(&vfs, "a.wav", 0x18, Category::Speech)
            .unwrap();
        let (track, _) = e
            .sound
            .play_pcm_stream(
                tone_stream(2000),
                "bink",
                VIDEO_TRACK_FLAGS,
                Category::Fixed,
            )
            .unwrap();
        assert!(
            e.sound.voice(answer).is_some(),
            "the Bink track stops nothing"
        );
        let (speech, _) = e
            .sound
            .play_pcm_stream(
                tone_stream(2000),
                "data/sprache/sid.ogg",
                VIDEO_SPEECH_FLAGS,
                Category::Fixed,
            )
            .unwrap();
        assert!(
            e.sound.voice(answer).is_none(),
            "category 9 stops category 8"
        );
        assert!(e.sound.voice(track).is_some() && e.sound.voice(speech).is_some());
        // As long as the speech track is running, a response (category 8) is rejected.
        assert!(
            e.sound
                .play_file_2d(&vfs, "b.wav", 0x18, Category::Speech)
                .is_none()
        );
    }

    /// Title running in the dummy (handle and file as after `MenuMusic_Play`).
    fn running_title(e: &mut AudioEngine) -> SoundId {
        let (id, _) = e
            .sound
            .play_pcm_stream(tone_stream(5000), "title", 0x06, Category::Music)
            .unwrap();
        e.menu.file = Some(TITLE_MUSIC.into());
        e.menu.handle = Some(id);
        id
    }

    #[test]
    fn menu_mode_keeps_the_title_running() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        let id = running_title(&mut e);
        e.start_level(&vfs, GameMode::Menu, None, &[]);
        assert_eq!(e.menu.handle, Some(id), "the same handle");
        assert_eq!(e.menu.file.as_deref(), Some(TITLE_MUSIC));
        let voice = e.sound.voice(id).unwrap();
        assert!(voice.fade.is_none_or(|f| !f.active), "no fade-out");
        // Even after 2 s still the same voice (no stop, no restart).
        for _ in 0..200 {
            e.advance(&vfs, 10.0);
        }
        assert!(e.sound.voice(id).is_some());
        assert_eq!(e.menu.handle, Some(id));
        assert!(e.music.handle.is_none(), "no game music");
    }

    #[test]
    fn campaign_mode_still_stops_the_menu_music() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        let id = running_title(&mut e);
        e.start_level(&vfs, GameMode::Campaign, None, &[]);
        assert_eq!(e.menu.handle, None);
        let fade = e.sound.voice(id).unwrap().fade.unwrap();
        assert!(fade.active && fade.stop && fade.duration == 1000);
    }

    #[test]
    fn resume_game_music_without_game_music_leaves_the_title_alone() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        let id = running_title(&mut e);
        e.start_level(&vfs, GameMode::Menu, None, &[]);
        e.resume_game_music(&vfs);
        assert_eq!(e.menu.handle, Some(id));
        assert!(e.music.handle.is_none());
    }

    #[test]
    fn stop_menu_music_fades_out() {
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        e.stop_menu_music();
        assert_eq!(e.menu.handle, None, "nothing happens without menu music");
        let (id, _) = e
            .sound
            .play_pcm_stream(tone_stream(5000), "title", 0x06, Category::Music)
            .unwrap();
        e.menu.file = Some(TITLE_MUSIC.into());
        e.menu.handle = Some(id);
        e.stop_menu_music();
        assert_eq!(
            (e.menu.handle, e.menu.file.as_deref()),
            (None, Some(TITLE_MUSIC))
        );
        let fade = e.sound.voice(id).unwrap().fade.unwrap();
        assert!(fade.active && fade.stop && fade.duration == 1000);
        for _ in 0..50 {
            e.advance(&vfs, 10.0);
        }
        assert!(e.sound.voice(id).is_some(), "still audible after 500 ms");
        for _ in 0..51 {
            e.advance(&vfs, 10.0);
        }
        assert!(e.sound.voice(id).is_none(), "stopped after 1000 ms");
    }

    #[test]
    fn menu_music_keeps_the_frame_loop_flag_and_restarts_the_same_file() {
        // Without a title the restart does nothing.
        let (_dir, vfs) = test_vfs();
        let mut e = engine();
        e.restart_menu_music(&vfs);
        assert_eq!((e.menu.file.as_deref(), e.menu.handle), (None, None));
        assert_eq!(
            AudioEngine::music_length_ms(&vfs, "data/music/title.ogg"),
            None
        );
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        // Loop according to the frame's flag, not the name (`title.ogg` would loop otherwise).
        e.play_menu_music_looping(&vfs, "DATA/MUSIC/TITLE.OGG", false);
        let first = e.menu.handle.expect("title.ogg plays");
        assert!(!e.menu_looping);
        // The same file while a handle is running does not restart and keeps its loop (`+0xD8` only at start).
        e.play_menu_music_looping(&vfs, "DATA/MUSIC/TITLE.OGG", true);
        assert_eq!(e.menu.handle, Some(first));
        assert!(!e.menu_looping);
        // `0x161`: new handle, same file, loop stays.
        e.restart_menu_music(&vfs);
        let second = e.menu.handle.expect("restart plays");
        assert_ne!(second, first);
        assert_eq!(e.menu.file.as_deref(), Some("DATA/MUSIC/TITLE.OGG"));
        assert!(!e.menu_looping);
        let length = AudioEngine::music_length_ms(&vfs, "DATA/MUSIC/TITLE.OGG").unwrap();
        assert_eq!(u64::from(length), e.sound.length_ms(second));
        assert!(length > 10_000, "{length}");
    }
}
