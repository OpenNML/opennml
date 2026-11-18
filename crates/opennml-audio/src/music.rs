//! Music as in the original (re/audio, section "Music"): the `MusicManager` per level
//! (nation music, script music, combat music switching with a 2000 ms crossfade and remembered
//! positions; `MusicManager_Update` `0x004E6B10`) and the menu music (`MenuMusic_Play`
//! `0x0045D4C0`, 1000 ms fades).

use std::collections::BTreeSet;
use std::path::PathBuf;

use opennml_formats::sound::MusicConfig;
use opennml_rda::Vfs;

use crate::manager::{SoundId, SoundManager};
use crate::volume::Category;

/// Flags of the music streams (rank 6, without an instance limit).
pub const MUSIC_FLAGS: u32 = 0x06;
/// Crossfade of the level music and the menu music in ms.
pub const MUSIC_FADE_MS: i32 = 2000;
pub const MENU_FADE_MS: i32 = 1000;

/// `m_Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicStatus {
    /// Script/loop music only, no combat monitoring.
    Script,
    /// Normal music with combat monitoring.
    Normal,
    /// Combat music.
    Attack,
}

impl MusicStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Normal => "normal",
            Self::Attack => "attack",
        }
    }
}

/// Resolves a music path: music lies loose in the game directory (`DATA/MUSIC`), FMOD opens it
/// bypassing the VFS. A missing path is logged once.
fn music_file(vfs: &Vfs, path: &str, failed: &mut BTreeSet<String>) -> Option<PathBuf> {
    let file = vfs.loose_file(path);
    if file.is_none() && failed.insert(path.to_string()) {
        log::warn!("Music {path} not found");
    }
    file
}

/// The `MusicManager` of a level.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicManager {
    config: MusicConfig,
    /// `+0x94` / `+0x98`: current normal and combat music.
    pub current: String,
    pub attack: String,
    /// `+0x9C`.
    pub handle: Option<SoundId>,
    pub status: MusicStatus,
    /// `+0xCC` switch timer, `+0xD4` lock, `+0xD8` maximum duration (ms).
    pub timer: i32,
    pub pause: i32,
    pub max: i32,
    /// `+0xDC`: advance the playlist.
    advance: bool,
    nation: Option<usize>,
    index: usize,
    /// `+0xC0` / `+0xC4`: remembered positions of the normal and combat music (ms).
    pub position_normal: u64,
    pub position_attack: u64,
    failed: BTreeSet<String>,
}

impl MusicManager {
    /// Fresh `MusicManager` (constructor `0x004E7470`): status 0, no handle, timer =
    /// `ATTACKSTARTTIME`.
    pub fn new(config: MusicConfig) -> Self {
        Self {
            timer: config.attack_start_time,
            config,
            current: String::new(),
            attack: String::new(),
            handle: None,
            status: MusicStatus::Script,
            pause: 0,
            max: 0,
            advance: false,
            nation: None,
            index: 0,
            position_normal: 0,
            position_attack: 0,
            failed: BTreeSet::new(),
        }
    }

    pub fn config(&self) -> &MusicConfig {
        &self.config
    }

    /// Selected nation (`m_CurNation`, `+0xB8`); `None` before the first [`Self::select_nation`].
    pub fn nation(&self) -> Option<usize> {
        self.nation
    }

    /// `MusicManager_SelectNation` (`0x004E6A80`): queue the nation music, play nothing. A nation
    /// without a playlist is ignored (the original aborts there).
    pub fn select_nation(&mut self, nation: usize) {
        let Some(first) = self.config.playlists.get(nation).and_then(|p| p.first()) else {
            return;
        };
        self.status = MusicStatus::Script;
        self.current = first.clone();
        self.attack = self.config.attack[nation].clone().unwrap_or_default();
        self.nation = Some(nation);
        self.index = 0;
        self.position_normal = 0;
        self.position_attack = 0;
        self.pause = 0;
        self.advance = false;
        self.timer = self.config.attack_start_time;
    }

    /// `MusicManager_StartPlaylist` (`0x004E6970`, multiplayer only): advancing on,
    /// fade in the normal music, status 1.
    pub fn start_playlist(&mut self, sound: &mut SoundManager, vfs: &Vfs) {
        self.advance = true;
        self.stop_music(sound);
        self.play_normal(sound, vfs);
    }

    /// `MusicManager_StopMusic`: fade out the running stream over 2000 ms and stop it.
    pub fn stop_music(&mut self, sound: &mut SoundManager) {
        if let Some(h) = self.handle.take() {
            sound.fade_out(h, MUSIC_FADE_MS, true);
        }
    }

    /// `MusicManager_ResumeMusic` (`0x004E6860`): play `+0x94` from 0 without a fade; without normal music (`+0x94`
    /// empty, e.g. in the menu level) nothing happens.
    pub fn resume_music(&mut self, sound: &mut SoundManager, vfs: &Vfs) {
        if self.current.is_empty() {
            return;
        }
        self.stop_music(sound);
        let path = self.current.clone();
        let handle = music_file(vfs, &path, &mut self.failed)
            .and_then(|file| sound.play_file_stream(&file, &path, MUSIC_FLAGS, 0, false));
        self.handle = handle;
    }

    /// Fade out and status 0: afterwards nothing starts by itself.
    pub fn stop_and_reset(&mut self, sound: &mut SoundManager) {
        self.stop_music(sound);
        self.status = MusicStatus::Script;
    }

    fn play_stream(
        &mut self,
        sound: &mut SoundManager,
        vfs: &Vfs,
        path: &str,
        start: u64,
        looping: bool,
    ) {
        let handle = music_file(vfs, path, &mut self.failed)
            .and_then(|file| sound.play_file_stream(&file, path, MUSIC_FLAGS, start, looping));
        if let Some(h) = handle {
            sound.fade_in(h, MUSIC_FADE_MS, Category::Music);
        }
        self.handle = handle;
    }

    /// `MusicManager_PlayNormalMusic` (`0x004E68B0`): fade out the old stream, fade in `+0x94` from the
    /// remembered position without a loop; status 0 -> 1.
    fn play_normal(&mut self, sound: &mut SoundManager, vfs: &Vfs) {
        self.stop_music(sound);
        let path = self.current.clone();
        self.play_stream(sound, vfs, &path, self.position_normal, false);
        if self.status == MusicStatus::Script {
            self.status = MusicStatus::Normal;
        }
    }

    /// `MusicManager_PlayAttackMusic` (`0x004E6910`).
    fn play_attack(&mut self, sound: &mut SoundManager, vfs: &Vfs) {
        self.stop_music(sound);
        let path = self.attack.clone();
        self.play_stream(sound, vfs, &path, self.position_attack, false);
    }

    /// Script action "Play music" (`MusicManager_PlayScriptMusic` `0x004E6D20`): status 0 ->
    /// new stream **with a loop**; status 1 -> crossfade like normal music; status 2 -> only remember.
    pub fn play_script(&mut self, sound: &mut SoundManager, vfs: &Vfs, path: &str) {
        self.advance = false;
        self.position_normal = 0;
        self.current = path.to_string();
        match self.status {
            MusicStatus::Script => {
                self.stop_music(sound);
                self.play_stream(sound, vfs, path, 0, true);
            }
            MusicStatus::Normal => self.play_normal(sound, vfs),
            MusicStatus::Attack => {}
        }
    }

    /// Script action "Set combat music" (`MusicManager_SetAttackMusic`): status 0 -> 1.
    pub fn set_attack_music(&mut self, path: &str) {
        self.attack = path.to_string();
        if self.status == MusicStatus::Script {
            self.status = MusicStatus::Normal;
        }
    }

    /// Script action 371 (`MusicManager_SetAttackStartPoints`): start threshold, status 0 -> 1.
    pub fn set_attack_start_points(&mut self, points: i32) {
        self.config.attack_start_points = points;
        if self.status == MusicStatus::Script {
            self.status = MusicStatus::Normal;
        }
    }

    /// `MusicManager_Update(dt)` with the combat points of this step.
    pub fn update(&mut self, sound: &mut SoundManager, vfs: &Vfs, dt: i32, points: i32) {
        let position = |s: &SoundManager, h: Option<SoundId>| h.map_or(0, |h| s.position_ms(h));
        let length = |s: &SoundManager, h: Option<SoundId>| h.map_or(0, |h| s.length_ms(h));
        match self.status {
            MusicStatus::Script => {}
            MusicStatus::Normal => {
                if self.pause < 1 {
                    if points < self.config.attack_start_points {
                        self.timer = self.config.attack_start_time;
                    } else {
                        self.timer -= dt;
                        if self.timer < 1 {
                            self.position_normal = position(sound, self.handle);
                            self.stop_music(sound);
                            self.play_attack(sound, vfs);
                            self.status = MusicStatus::Attack;
                            self.timer = self.config.attack_stop_time;
                            self.pause = self.config.attack_pause;
                            self.max = self.config.attack_max_time;
                            return;
                        }
                    }
                } else {
                    self.pause -= dt;
                }
                if length(sound, self.handle) <= position(sound, self.handle) + 1000 {
                    self.position_normal = 0;
                    if self.advance
                        && let Some(list) = self.nation.and_then(|n| self.config.playlists.get(n))
                        && !list.is_empty()
                    {
                        self.index = (self.index + 1) % list.len();
                        self.current = list[self.index].clone();
                    }
                    self.play_normal(sound, vfs);
                }
            }
            MusicStatus::Attack => {
                let left = if self.config.attack_stop_points < points {
                    self.max -= dt;
                    self.timer = self.config.attack_stop_time;
                    self.max
                } else {
                    self.timer -= dt;
                    self.timer
                };
                if left < 1 {
                    self.position_attack = position(sound, self.handle);
                    self.stop_music(sound);
                    self.play_normal(sound, vfs);
                    self.status = MusicStatus::Normal;
                    self.timer = self.config.attack_start_time;
                    return;
                }
                if length(sound, self.handle) <= position(sound, self.handle) + 1000 {
                    self.position_attack = 0;
                    self.play_attack(sound, vfs);
                }
            }
        }
    }

    /// File of the running stream (normal or combat music).
    pub fn playing(&self) -> Option<&str> {
        self.handle.map(|_| match self.status {
            MusicStatus::Attack => self.attack.as_str(),
            _ => self.current.as_str(),
        })
    }
}

/// Menu music (`MenuMusic`): title, victory, defeat and credits music.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MenuMusic {
    pub file: Option<String>,
    pub handle: Option<SoundId>,
    failed: BTreeSet<String>,
}

impl MenuMusic {
    /// `MenuMusic_Play(path, loop)`: only with a different file or without a running handle; fade out the
    /// old stream over 1000 ms, start the new one from 0 and fade it in over 1000 ms.
    pub fn play(&mut self, sound: &mut SoundManager, vfs: &Vfs, path: &str, looping: bool) {
        if self.handle.is_some() && self.file.as_deref() == Some(path) {
            return;
        }
        self.stop(sound);
        let handle = music_file(vfs, path, &mut self.failed)
            .and_then(|file| sound.play_file_stream(&file, path, MUSIC_FLAGS, 0, looping));
        if let Some(h) = handle {
            sound.fade_in(h, MENU_FADE_MS, Category::Music);
        }
        self.file = Some(path.to_string());
        self.handle = handle;
    }

    /// Indicator: the menu title as long as its voice sounds. The original resets the handle only
    /// in `MenuMusic_Stop` and never at the end of the title; `handle` stays set afterwards.
    pub fn playing(&self, sound: &SoundManager) -> Option<&str> {
        self.handle
            .filter(|&h| sound.voice(h).is_some())
            .and(self.file.as_deref())
    }

    /// `MenuMusic_Stop`: fade out over 1000 ms and stop.
    pub fn stop(&mut self, sound: &mut SoundManager) {
        if let Some(h) = self.handle.take() {
            sound.fade_out(h, MENU_FADE_MS, true);
        }
    }
}

/// Does a menu title loop? `win`/`lose` carry `MENUFRAME_DONOTLOOPMUSIC`, `title`/`credits`
/// do not.
pub fn menu_track_loops(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    !matches!(name.as_str(), "win.ogg" | "lose.ogg")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::tests::test_vfs;

    /// Configuration as in the shipped data with two nations (the paths do not exist in the test VFS: the
    /// streams do not start, the state machine runs anyway).
    fn config() -> MusicConfig {
        let mut c = MusicConfig {
            attack_start_points: 500,
            attack_start_time: 1000,
            attack_stop_points: 0,
            attack_stop_time: 5000,
            attack_pause: 10000,
            attack_max_time: 720000,
            ..MusicConfig::default()
        };
        c.playlists[2] = vec!["data/music/spa.ogg".into()];
        c.attack[2] = Some("data/music/spa_attack.ogg".into());
        c
    }

    #[test]
    fn campaign_scripts_preselect_nation_music_and_switch_to_attack_music_with_timers() {
        let (_dir, vfs) = test_vfs();
        let mut sound = SoundManager::new(Default::default());
        let mut music = MusicManager::new(config());
        music.select_nation(2);
        assert_eq!(
            (music.status, music.current.as_str()),
            (MusicStatus::Script, "data/music/spa.ogg")
        );
        music.update(&mut sound, &vfs, 16, 0);
        assert_eq!(music.status, MusicStatus::Script, "status 0 checks nothing");
        // n371: status 1; without a running stream this counts as the end of the title -> nation music.
        music.set_attack_start_points(1000);
        music.play_script(&mut sound, &vfs, "data/music/atmo_karibik.ogg");
        assert_eq!(music.status, MusicStatus::Normal);
        // Combat points below the threshold keep the timer at ATTACKSTARTTIME.
        music.update(&mut sound, &vfs, 600, 999);
        assert_eq!(music.timer, 1000);
        music.update(&mut sound, &vfs, 600, 1000);
        assert_eq!((music.status, music.timer), (MusicStatus::Normal, 400));
        music.update(&mut sound, &vfs, 400, 1000);
        assert_eq!(music.status, MusicStatus::Attack);
        assert_eq!((music.timer, music.pause, music.max), (5000, 10000, 720000));
        // 5 s without points: back to the normal music; the lock does not expire until there.
        for _ in 0..4 {
            music.update(&mut sound, &vfs, 1000, 0);
        }
        assert_eq!(music.status, MusicStatus::Attack);
        music.update(&mut sound, &vfs, 1000, 0);
        assert_eq!((music.status, music.pause), (MusicStatus::Normal, 10000));
        music.update(&mut sound, &vfs, 9999, 5000);
        assert_eq!(
            (music.status, music.pause, music.timer),
            (MusicStatus::Normal, 1, 1000)
        );
        music.update(&mut sound, &vfs, 1, 5000);
        music.update(&mut sound, &vfs, 999, 5000);
        assert_eq!(music.status, MusicStatus::Normal);
        music.update(&mut sound, &vfs, 1, 5000);
        assert_eq!(music.status, MusicStatus::Attack);
        // With points above the stop threshold the combat ends only after ATTACKMAXTIME.
        music.update(&mut sound, &vfs, 719_999, 1);
        assert_eq!((music.status, music.max), (MusicStatus::Attack, 1));
        music.update(&mut sound, &vfs, 1, 1);
        assert_eq!(music.status, MusicStatus::Normal);
    }

    #[test]
    fn menu_music_is_shown_only_while_its_voice_lives() {
        let (_dir, vfs) = test_vfs();
        let mut sound = SoundManager::new(Default::default());
        let id = sound
            .play_file_2d(&vfs, "a.wav", 0x12, Category::Music)
            .unwrap();
        let menu = MenuMusic {
            file: Some("data/music/win.ogg".into()),
            handle: Some(id),
            ..MenuMusic::default()
        };
        assert_eq!(menu.playing(&sound), Some("data/music/win.ogg"));
        // End of the title without a loop: the handle stays set as in the original (the same title
        // does not restart), but nothing sounds any more.
        let mut out = vec![0; 3000 * 2];
        sound.render(&mut out);
        assert!(menu.handle.is_some());
        assert_eq!(menu.playing(&sound), None);
        assert_eq!(MenuMusic::default().playing(&sound), None);
    }

    #[test]
    fn script_music_in_status_two_is_only_remembered() {
        let (_dir, vfs) = test_vfs();
        let mut sound = SoundManager::new(Default::default());
        let mut music = MusicManager::new(config());
        music.select_nation(2);
        music.set_attack_music("data/music/spa_attack.ogg");
        assert_eq!(music.status, MusicStatus::Normal);
        music.status = MusicStatus::Attack;
        music.play_script(&mut sound, &vfs, "data/music/wid.ogg");
        assert_eq!(
            (music.status, music.current.as_str()),
            (MusicStatus::Attack, "data/music/wid.ogg")
        );
        // A nation without a playlist changes nothing.
        let mut other = MusicManager::new(config());
        other.select_nation(4);
        assert_eq!(other.current, "");
        assert!(menu_track_loops("data/music/title.ogg"));
        assert!(!menu_track_loops("DATA\\MUSIC\\WIN.OGG"));
    }
}
