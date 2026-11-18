//! Sound management like `SoundManager` (re/audio): playback calls with the database,
//! path cache, distance filter and channel allocation; channel volumes by category; fades of the
//! streams; the sound thread as a 10 ms tick on the virtual clock (advance channels, release
//! finished ones, `FSOUND_Update` with the listener); the mixer provides the samples.

use std::collections::{BTreeSet, HashMap};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::Vec3;
use opennml_rda::{Vfs, normalize_path};
use opennml_world::rand::MsvcRand;

use crate::db::SoundDb;
use crate::decode::{OggStream, Pcm, PcmStream, parse_wav};
use crate::mixer::{Mixer, Source};
use crate::pool::{Allocation, CHANNELS, ChannelPool, Flags, SlotInfo};
use crate::spatial::FmodListener;
use crate::volume::{
    Category, DEFAULT_VOLUMES, FIXED_CATEGORY_VOLUME, SPEECH_BOOST, channel_volume,
    fmod_get_volume, fmod_volume,
};
use crate::{AUDIO_SEED, SAMPLE_RATE};

/// Frames per tick of the sound thread (10 ms; the original sleeps 10 ms per loop).
pub const TICK_FRAMES: u64 = SAMPLE_RATE as u64 / 100;
/// Tick of the sound thread in ms.
pub const TICK_MS: i32 = 10;
/// Min/max distance of the 3D sounds in tiles (`SoundManager_SetMinMaxDistance(4, 20)`).
pub const MIN_DISTANCE: f32 = 4.0;
pub const MAX_DISTANCE: f32 = 20.0;

/// ID of a sound (running counter `SoundManager+0x44`).
pub type SoundId = u32;

/// Fade of a stream channel (`StreamChannel_HandleMessage` `0x004E79C0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    start: f32,
    slope: f32,
    /// Elapsed time and duration in ms.
    pub t: i32,
    pub duration: i32,
    /// Bit 1: fade is running.
    pub active: bool,
    /// Bit 2: stop at the end (stays set even with a new fade-in).
    pub stop: bool,
}

/// What a channel plays, from the game's point of view.
#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    pub id: SoundId,
    /// Game path of the file.
    pub file: String,
    pub category: Category,
    pub flags: u32,
    /// `v24`: 1000 for dialog speech (`PlayMemoryStream`).
    pub boost: i32,
    /// `ftol(factor · 1000)` for 3D database sounds.
    pub factor: Option<i32>,
    /// World position of a 3D sound.
    pub position: Option<Vec3>,
    pub stream: bool,
    pub fade: Option<Fade>,
}

/// A sample in the path cache: the first use fixes 2D/3D and loop.
#[derive(Debug, Clone)]
struct CachedSample {
    pcm: Arc<Pcm>,
    spatial: bool,
    looping: bool,
    key: u64,
}

/// The sound manager.
#[derive(Debug)]
pub struct SoundManager {
    pub db: SoundDb,
    rng: MsvcRand,
    volumes: [i32; 3],
    /// Listener in tiles (`+0xE4`), set immediately; distance filter and priority.
    listener_tiles: Vec3,
    /// FMOD listener that the next tick of the sound thread takes over.
    pending_listener: FmodListener,
    pool: ChannelPool,
    mixer: Mixer,
    voices: Vec<Option<Voice>>,
    samples: HashMap<String, Option<CachedSample>>,
    /// Music files that could not be opened or decoded: otherwise the music state machine retries
    /// in every tick (and reports it every time).
    failed_streams: BTreeSet<PathBuf>,
    /// Counts the actual attempts to open music files (tests only).
    #[cfg(test)]
    stream_open_attempts: u32,
    next_key: u64,
    next_id: SoundId,
    /// Frames mixed since the start and the next tick of the sound thread.
    frame: u64,
    next_tick: u64,
}

impl SoundManager {
    pub fn new(db: SoundDb) -> Self {
        let mut mixer = Mixer::new();
        mixer.set_min_max(MIN_DISTANCE, MAX_DISTANCE);
        Self {
            db,
            rng: MsvcRand::new(AUDIO_SEED),
            volumes: DEFAULT_VOLUMES,
            listener_tiles: Vec3::ZERO,
            pending_listener: FmodListener::default(),
            pool: ChannelPool::new(),
            mixer,
            voices: (0..CHANNELS).map(|_| None).collect(),
            samples: HashMap::new(),
            failed_streams: BTreeSet::new(),
            #[cfg(test)]
            stream_open_attempts: 0,
            next_key: 1,
            next_id: 0,
            frame: 0,
            next_tick: 0,
        }
    }

    pub fn rng(&mut self) -> &mut MsvcRand {
        &mut self.rng
    }

    /// Frames mixed since the start.
    pub fn frames(&self) -> u64 {
        self.frame
    }

    /// `kat(k)` (`SoundManager_GetCategoryVolume` `0x00550AF0`): slider for k < 3, otherwise fixed 1998.
    pub fn category_volume(&self, category: Category) -> i32 {
        match category {
            Category::Fixed => FIXED_CATEGORY_VOLUME,
            _ => self.volumes[category as usize],
        }
    }

    pub fn volumes(&self) -> [i32; 3] {
        self.volumes
    }

    /// `SoundManager_SetCategoryVolume`: message 1 to all channels of the category. 3D channels
    /// compute `new · f / 1000`, streams set `new` (not during a fade), 2D samples
    /// keep their volume. [`Category::Fixed`] has no slider: the call has no effect.
    pub fn set_category_volume(&mut self, category: Category, volume: i32) {
        if category == Category::Fixed {
            return;
        }
        self.volumes[category as usize] = volume;
        for slot in 0..CHANNELS {
            let Some(voice) = &self.voices[slot] else {
                continue;
            };
            if voice.category != category {
                continue;
            }
            let game = match (voice.stream, voice.factor) {
                (true, _) if voice.fade.is_some_and(|f| f.active) => continue,
                (true, _) => volume,
                (false, Some(f)) => f * volume / 1000,
                (false, None) => continue,
            };
            self.mixer.set_volume(slot, fmod_volume(game));
        }
    }

    /// `SoundManager_SetListenerFromCamera` (`0x00551EA0`): listener = camera target in tiles, forward
    /// from the eye (height 0) to the target (terrain height), up +y. FMOD takes it over in the next tick.
    pub fn set_listener(&mut self, target: Vec3, target_height: f32, eye: Vec3) {
        let tiles = Vec3::new(target.x * 0.0078125, 0.0, target.z * 0.0078125);
        let forward = (Vec3::new(target.x, target_height, target.z) - Vec3::new(eye.x, 0.0, eye.z))
            .normalize_or_zero();
        self.listener_tiles = tiles;
        self.pending_listener =
            FmodListener::new(tiles.to_array(), forward.to_array(), [0.0, 1.0, 0.0]);
    }

    /// Listener in tiles.
    pub fn listener_tiles(&self) -> Vec3 {
        self.listener_tiles
    }

    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Occupied channels in mixing order with their sound.
    pub fn voices(&self) -> impl Iterator<Item = (usize, &Voice)> {
        self.mixer
            .order()
            .iter()
            .filter_map(|&slot| Some((slot, self.voices[slot].as_ref()?)))
    }

    fn slot_of(&self, id: SoundId) -> Option<usize> {
        self.voices
            .iter()
            .position(|v| v.as_ref().is_some_and(|v| v.id == id))
    }

    /// Loads a WAV through the VFS into the path cache (`SoundManager_LoadSoundFile`); the first
    /// use fixes 2D/3D and loop. If the file is missing, the sound stays silent (logged
    /// once).
    fn load_sample(
        &mut self,
        vfs: &Vfs,
        path: &str,
        spatial: bool,
        looping: bool,
    ) -> Option<CachedSample> {
        let key = normalize_path(path);
        if let Some(cached) = self.samples.get(&key) {
            return cached.clone();
        }
        let loaded = match vfs
            .read(&key)
            .map_err(anyhow::Error::from)
            .and_then(|b| parse_wav(&b))
        {
            Ok(pcm) => {
                self.next_key += 1;
                Some(CachedSample {
                    pcm: Arc::new(pcm),
                    spatial,
                    looping,
                    key: self.next_key,
                })
            }
            Err(e) => {
                log::warn!("Sound {path} not loadable: {e:#}");
                None
            }
        };
        self.samples.insert(key, loaded.clone());
        loaded
    }

    /// Occupies a slot and starts the source; returns the new ID.
    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        slot: usize,
        stopped: &[usize],
        info: SlotInfo,
        source: Source,
        looping: bool,
        game_volume: i32,
        spatial: Option<[f32; 3]>,
        voice: Voice,
    ) -> SoundId {
        for &s in stopped {
            self.release(s);
        }
        self.pool.occupy(slot, info);
        self.mixer
            .start(slot, source, looping, fmod_volume(game_volume), spatial);
        let id = voice.id;
        self.voices[slot] = Some(voice);
        id
    }

    fn release(&mut self, slot: usize) {
        self.mixer.stop(slot);
        self.pool.free(slot);
        self.voices[slot] = None;
    }

    fn next_id(&mut self) -> SoundId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// `SoundManager_PlayObjectSound3D` (`0x00553080`): selection from the database, loading as a
    /// 3D sample, distance filter (`> 20` tiles: nothing), allocation with distance as priority;
    /// at the instance limit with `0x100` a more distant instance is moved. Volume with the
    /// factor of `soundguidlist.txt`.
    #[allow(clippy::too_many_arguments)]
    pub fn play_object_3d(
        &mut self,
        vfs: &Vfs,
        action: i32,
        flags: u32,
        position: Vec3,
        guid: i32,
        index: i32,
        category: Category,
    ) -> bool {
        let tiles = Vec3::new(position.x * 0.0078125, 0.0, position.z * 0.0078125);
        let Some(sound) = self.db.pick(guid, action, index, &mut self.rng) else {
            return false;
        };
        let Some(entry) = self.db.sound(sound).cloned() else {
            log::warn!("Soundguid {sound} not in db found");
            return false;
        };
        let Some(sample) = self.load_sample(vfs, &entry.path, true, false) else {
            return false;
        };
        let distance = (self.listener_tiles - tiles).length();
        if distance > MAX_DISTANCE {
            return false;
        }
        let factor = (entry.factor * 1000.0) as i32;
        match self.pool.allocate(Flags(flags), distance, sample.key, true) {
            Allocation::Moved { slot, stopped } => {
                for s in stopped {
                    self.release(s);
                }
                self.mixer.set_position(slot, tiles.to_array());
                if let Some(v) = &mut self.voices[slot] {
                    v.position = Some(position);
                }
                false
            }
            Allocation::Rejected { stopped } => {
                for s in stopped {
                    self.release(s);
                }
                false
            }
            Allocation::Slot { slot, stopped } => {
                let id = self.next_id();
                let game = channel_volume(self.category_volume(category), 0, Some(entry.factor));
                self.start(
                    slot,
                    &stopped,
                    SlotInfo {
                        category: Flags(flags).category(),
                        priority: distance,
                        sample: sample.key,
                    },
                    Source::Sample {
                        pcm: sample.pcm.clone(),
                        position: 0,
                    },
                    sample.looping,
                    game,
                    sample.spatial.then_some(tiles.to_array()),
                    Voice {
                        id,
                        file: entry.path.clone(),
                        category,
                        flags,
                        boost: 0,
                        factor: Some(factor),
                        position: Some(position),
                        stream: false,
                        fade: None,
                    },
                );
                true
            }
        }
    }

    /// 2D sample from a file (common part of `PlayObjectSound2D`, `PlaySoundGuid2D`,
    /// `PlayFile2D`): volume without a factor.
    fn play_sample_2d(
        &mut self,
        vfs: &Vfs,
        path: &str,
        flags: u32,
        category: Category,
        looping: bool,
    ) -> Option<SoundId> {
        let sample = self.load_sample(vfs, path, false, looping)?;
        match self.pool.allocate(Flags(flags), 0.0, sample.key, false) {
            Allocation::Slot { slot, stopped } => {
                let id = self.next_id();
                let game = channel_volume(self.category_volume(category), 0, None);
                Some(self.start(
                    slot,
                    &stopped,
                    SlotInfo {
                        category: Flags(flags).category(),
                        priority: 0.0,
                        sample: sample.key,
                    },
                    Source::Sample {
                        pcm: sample.pcm.clone(),
                        position: 0,
                    },
                    sample.looping,
                    game,
                    // Started without a pause: FMOD puts the source on the listener (`Chan_Allocate`).
                    sample.spatial.then_some(self.mixer.listener().position),
                    Voice {
                        id,
                        file: path.to_string(),
                        category,
                        flags,
                        boost: 0,
                        factor: None,
                        position: None,
                        stream: false,
                        fade: None,
                    },
                ))
            }
            Allocation::Moved { stopped, .. } | Allocation::Rejected { stopped } => {
                for s in stopped {
                    self.release(s);
                }
                None
            }
        }
    }

    /// `SoundManager_PlayObjectSound2D` (`0x00553290`): database sound as 2D, factor discarded.
    pub fn play_object_2d(
        &mut self,
        vfs: &Vfs,
        action: i32,
        flags: u32,
        guid: i32,
        index: i32,
        category: Category,
    ) -> Option<SoundId> {
        let sound = self.db.pick(guid, action, index, &mut self.rng)?;
        self.play_sound_guid_2d(vfs, sound, flags, category, false)
    }

    /// `SoundManager_PlaySoundGuid2D` (`0x00552CF0`).
    pub fn play_sound_guid_2d(
        &mut self,
        vfs: &Vfs,
        sound: i32,
        flags: u32,
        category: Category,
        looping: bool,
    ) -> Option<SoundId> {
        let Some(entry) = self.db.sound(sound).cloned() else {
            log::warn!("Soundguid {sound} not in db found");
            return None;
        };
        self.play_sample_2d(vfs, &entry.path, flags, category, looping)
    }

    /// `SoundManager_PlayEvent2D` (`0x00552FB0`): event from `otherevents.ini`.
    pub fn play_event_2d(
        &mut self,
        vfs: &Vfs,
        name: &str,
        flags: u32,
        category: Category,
        looping: bool,
    ) -> Option<SoundId> {
        let sound = self.db.event(name)?;
        self.play_sound_guid_2d(vfs, sound, flags, category, looping)
    }

    /// `SoundManager_PlayFile2D` (`0x00552E50`): WAV file directly.
    pub fn play_file_2d(
        &mut self,
        vfs: &Vfs,
        path: &str,
        flags: u32,
        category: Category,
    ) -> Option<SoundId> {
        self.play_sample_2d(vfs, path, flags, category, false)
    }

    /// `SoundManager_PlayFile3D` (`0x005536A0`): WAV file as a 3D sound at `position`, without a factor.
    pub fn play_file_3d(
        &mut self,
        vfs: &Vfs,
        path: &str,
        flags: u32,
        position: Vec3,
        category: Category,
    ) -> bool {
        self.play_sample_3d(vfs, path, flags, position, category, None)
    }

    /// `SoundManager_PlayEvent3D` (`0x00553420`): event from `otherevents.ini`
    /// (`FindOtherEvent`) -> sound GUID -> 3D sample (`LoadSoundGuid(guid, 0, 1)`) at `position`,
    /// distance filter like [`SoundManager::play_file_3d`], volume with the factor of the entry
    /// like [`SoundManager::play_object_3d`] (`ftol` before `SoundChannel3D_Construct`). The bit
    /// `0x400` (hardware slot) is dropped in the reimplementation. Unknown event: silent.
    pub fn play_event_3d(
        &mut self,
        vfs: &Vfs,
        name: &str,
        flags: u32,
        position: Vec3,
        category: Category,
    ) -> bool {
        let Some(sound) = self.db.event(name) else {
            return false;
        };
        let Some(entry) = self.db.sound(sound).cloned() else {
            log::warn!("Soundguid {sound} not in db found");
            return false;
        };
        self.play_sample_3d(
            vfs,
            &entry.path,
            flags,
            position,
            category,
            Some(entry.factor),
        )
    }

    /// Common part of `PlayFile3D` and `PlayEvent3D`: load the 3D sample, distance <= 20
    /// tiles, slot with the distance as priority; `factor` = volume factor of the entry.
    fn play_sample_3d(
        &mut self,
        vfs: &Vfs,
        path: &str,
        flags: u32,
        position: Vec3,
        category: Category,
        factor: Option<f32>,
    ) -> bool {
        let tiles = Vec3::new(position.x * 0.0078125, 0.0, position.z * 0.0078125);
        let Some(sample) = self.load_sample(vfs, path, true, false) else {
            return false;
        };
        let distance = (self.listener_tiles - tiles).length();
        if distance > MAX_DISTANCE {
            return false;
        }
        match self.pool.allocate(Flags(flags), distance, sample.key, true) {
            Allocation::Slot { slot, stopped } => {
                let id = self.next_id();
                let game = channel_volume(self.category_volume(category), 0, factor);
                self.start(
                    slot,
                    &stopped,
                    SlotInfo {
                        category: Flags(flags).category(),
                        priority: distance,
                        sample: sample.key,
                    },
                    Source::Sample {
                        pcm: sample.pcm.clone(),
                        position: 0,
                    },
                    sample.looping,
                    game,
                    sample.spatial.then_some(tiles.to_array()),
                    Voice {
                        id,
                        file: path.to_string(),
                        category,
                        flags,
                        boost: 0,
                        factor: factor.map(|f| (f * 1000.0) as i32),
                        position: Some(position),
                        stream: false,
                        fade: None,
                    },
                );
                true
            }
            Allocation::Moved { slot, stopped } => {
                for s in stopped {
                    self.release(s);
                }
                self.mixer.set_position(slot, tiles.to_array());
                false
            }
            Allocation::Rejected { stopped } => {
                for s in stopped {
                    self.release(s);
                }
                false
            }
        }
    }

    /// Starts a stream (2D) on an allocated slot.
    #[allow(clippy::too_many_arguments)]
    fn play_stream(
        &mut self,
        stream: Box<dyn PcmStream>,
        file: &str,
        flags: u32,
        category: Category,
        looping: bool,
        game_volume: i32,
        boost: i32,
    ) -> Option<SoundId> {
        self.next_key += 1;
        let key = self.next_key;
        match self.pool.allocate(Flags(flags), 0.0, key, false) {
            Allocation::Slot { slot, stopped } => {
                let id = self.next_id();
                Some(self.start(
                    slot,
                    &stopped,
                    SlotInfo {
                        category: Flags(flags).category(),
                        priority: 0.0,
                        sample: key,
                    },
                    Source::Stream(stream),
                    looping,
                    game_volume,
                    None,
                    Voice {
                        id,
                        file: file.to_string(),
                        category,
                        flags,
                        boost,
                        factor: None,
                        position: None,
                        stream: true,
                        fade: None,
                    },
                ))
            }
            Allocation::Moved { stopped, .. } | Allocation::Rejected { stopped } => {
                for s in stopped {
                    self.release(s);
                }
                None
            }
        }
    }

    /// `SoundManager_PlayFileStream` (`0x00552540`): music as a file stream (category music),
    /// `FSOUND_Stream_SetTime(start_ms)` before the start, volume `kat(0)`.
    pub fn play_file_stream(
        &mut self,
        file: &Path,
        label: &str,
        flags: u32,
        start_ms: u64,
        looping: bool,
    ) -> Option<SoundId> {
        // A broken file stays broken: do not reopen and report it in every tick.
        // (Rejected channel allocations below are not remembered, otherwise the title would stay silent.)
        if self.failed_streams.contains(file) {
            return None;
        }
        #[cfg(test)]
        {
            self.stream_open_attempts += 1;
        }
        let opened = std::fs::File::open(file)
            .map_err(anyhow::Error::from)
            .and_then(|f| OggStream::open(Box::new(f)));
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(e) => {
                log::warn!("Music {label} not playable: {e:#}");
                self.failed_streams.insert(file.to_path_buf());
                return None;
            }
        };
        if start_ms > 0
            && let Err(e) = stream.seek_ms(start_ms)
        {
            log::warn!("Music {label}: seek to {start_ms} ms: {e:#}");
        }
        let volume = channel_volume(self.category_volume(Category::Music), 0, None);
        self.play_stream(
            Box::new(stream),
            label,
            flags,
            Category::Music,
            looping,
            volume,
            0,
        )
    }

    /// `SoundManager_PlayMemoryStream` (`0x00552060`): speech from the VFS as a memory stream,
    /// then `v24 = 1000` (doubling). Returns ID and length in ms.
    pub fn play_memory_stream(
        &mut self,
        vfs: &Vfs,
        path: &str,
        flags: u32,
        category: Category,
    ) -> Option<(SoundId, u64)> {
        let opened = vfs
            .read(path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| OggStream::open(Box::new(Cursor::new(bytes))));
        let stream = match opened {
            Ok(stream) => stream,
            Err(e) => {
                log::warn!("Speech {path} not playable: {e:#}");
                return None;
            }
        };
        self.play_pcm_stream(Box::new(stream), path, flags, category)
    }

    /// An already opened stream like [`SoundManager::play_memory_stream`] (allocation by `flags`, then
    /// `v24 = 1000`, doubling), e.g. the converted Bink audio track of a video. `label` appears as the
    /// file of the sound. Returns ID and length in ms (0 if unknown); the mixer does not play streams with a channel
    /// count other than 1 or 2 (warning, `None`).
    pub fn play_pcm_stream(
        &mut self,
        stream: Box<dyn PcmStream>,
        label: &str,
        flags: u32,
        category: Category,
    ) -> Option<(SoundId, u64)> {
        let channels = stream.channels();
        if !(1..=2).contains(&channels) {
            log::warn!("Stream {label} not playable: {channels} channels");
            return None;
        }
        let length = stream.length_ms().unwrap_or(0);
        let volume = channel_volume(self.category_volume(category), SPEECH_BOOST, None);
        let id = self.play_stream(stream, label, flags, category, false, volume, SPEECH_BOOST)?;
        Some((id, length))
    }

    /// `SoundManager_StopSound`: immediately, without a fade.
    pub fn stop(&mut self, id: SoundId) {
        if let Some(slot) = self.slot_of(id) {
            self.release(slot);
        }
    }

    /// Stops all sounds.
    pub fn stop_all(&mut self) {
        for slot in 0..CHANNELS {
            if self.voices[slot].is_some() {
                self.release(slot);
            }
        }
    }

    /// `SoundManager_FadeInSound` (message `0x10000`): volume immediately 0, then linearly to
    /// `kat(category)` in `duration_ms` (streams only).
    pub fn fade_in(&mut self, id: SoundId, duration_ms: i32, category: Category) {
        let target = self.category_volume(category);
        let Some(slot) = self.slot_of(id) else { return };
        let Some(voice) = self.voices[slot].as_mut().filter(|v| v.stream) else {
            return;
        };
        let stop = voice.fade.is_some_and(|f| f.stop);
        voice.fade = Some(Fade {
            start: 0.0,
            slope: target as f32 / duration_ms as f32,
            t: 0,
            duration: duration_ms,
            active: true,
            stop,
        });
        self.mixer.set_volume(slot, fmod_volume(0));
    }

    /// `SoundManager_FadeOutSound` (message `0x10001`): from `GetVolume` (half scale, the
    /// volume jumps) linearly to 0, with `stop` release afterwards (streams only).
    pub fn fade_out(&mut self, id: SoundId, duration_ms: i32, stop: bool) {
        let Some(slot) = self.slot_of(id) else { return };
        let current = self.mixer.channel(slot).map_or(0, |c| c.volume());
        let Some(voice) = self.voices[slot].as_mut().filter(|v| v.stream) else {
            return;
        };
        let start = fmod_get_volume(current) as f32;
        let keep_stop = voice.fade.is_some_and(|f| f.stop);
        voice.fade = Some(Fade {
            start,
            slope: -start / duration_ms as f32,
            t: 0,
            duration: duration_ms,
            active: true,
            stop: stop || keep_stop,
        });
    }

    /// `FSOUND_Stream_GetTime` (0 for an unknown ID).
    pub fn position_ms(&self, id: SoundId) -> u64 {
        self.slot_of(id)
            .and_then(|s| self.mixer.channel(s))
            .map_or(0, |c| c.position_ms())
    }

    /// `FSOUND_Stream_GetLengthMs` (0 for an unknown ID).
    pub fn length_ms(&self, id: SoundId) -> u64 {
        self.slot_of(id)
            .and_then(|s| self.mixer.channel(s))
            .and_then(|c| c.length_ms())
            .unwrap_or(0)
    }

    pub fn is_playing(&self, id: SoundId) -> bool {
        self.slot_of(id).is_some_and(|s| self.mixer.is_playing(s))
    }

    pub fn voice(&self, id: SoundId) -> Option<&Voice> {
        self.voices.iter().flatten().find(|v| v.id == id)
    }

    /// One tick of the sound thread (`SoundManager_ThreadLoop` `0x00550D50`): advance the fades of the
    /// streams, release finished channels, `FSOUND_Update` with the most recently set listener.
    fn thread_tick(&mut self) {
        let mut finished = Vec::new();
        for slot in 0..CHANNELS {
            let Some(voice) = self.voices[slot].as_mut() else {
                continue;
            };
            if let Some(fade) = voice.fade.as_mut().filter(|f| f.active) {
                fade.t = (fade.t + TICK_MS).min(fade.duration);
                let game =
                    (f64::from(fade.start) + f64::from(fade.t) * f64::from(fade.slope)) as i32;
                self.mixer.set_volume(slot, fmod_volume(game));
                if fade.t >= fade.duration {
                    fade.active = false;
                    if fade.stop {
                        fade.stop = false;
                        finished.push(slot);
                        continue;
                    }
                }
            }
            if !self.mixer.is_playing(slot) {
                finished.push(slot);
            }
        }
        for slot in finished {
            self.release(slot);
        }
        self.mixer.update_3d(self.pending_listener);
    }

    /// Mixes `out.len() / 2` stereo frames; the sound thread ticks every 441 frames (10 ms).
    pub fn render(&mut self, out: &mut [i16]) {
        let frames = out.len() as u64 / 2;
        let mut done = 0;
        while done < frames {
            if self.frame == self.next_tick {
                self.thread_tick();
                self.next_tick += TICK_FRAMES;
            }
            let n = (frames - done).min(self.next_tick - self.frame);
            self.mixer
                .render(&mut out[(done * 2) as usize..((done + n) * 2) as usize]);
            done += n;
            self.frame += n;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db::tests::test_db;
    use crate::decode::tests::wav_bytes;
    use crate::resample::ResampledStream;
    use opennml_rda::testutil::{TestFile, build_rda};

    /// VFS with the WAVs of the test database (`a.wav` ... `e.wav`, `surf.wav`, `rain.wav`):
    /// constant values so that the volume can be read off directly.
    pub(crate) fn test_vfs() -> (tempfile::TempDir, Vfs) {
        let dir = tempfile::tempdir().unwrap();
        let wavs: Vec<(String, Vec<u8>)> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|n| (format!("{n}.wav"), wav_bytes(1, &[20000; 2000])))
            .chain([
                ("surf.wav".to_string(), wav_bytes(1, &[20000; 44100])),
                ("rain.wav".to_string(), wav_bytes(1, &[10000; 441])),
            ])
            .collect();
        let files: Vec<TestFile<'_>> = wavs
            .iter()
            .map(|(name, data)| TestFile {
                path: name,
                data,
                compress: false,
            })
            .collect();
        std::fs::write(dir.path().join("nml0.rda"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    fn manager() -> SoundManager {
        SoundManager::new(test_db())
    }

    /// Stream with a constant value (stereo, 44,100 Hz) over `ms` milliseconds.
    pub(crate) fn tone_stream(ms: u64) -> Box<dyn PcmStream> {
        let mut left = ms * 441 / 10;
        Box::new(ResampledStream::new(
            Box::new(move |out: &mut Vec<f32>| {
                let n = left.min(1000);
                left -= n;
                out.extend(std::iter::repeat_n(0.25, n as usize * 2));
                left > 0
            }),
            SAMPLE_RATE,
            2,
            Some(ms),
        ))
    }

    // QUIRK O-69, O-119: kept
    #[test]
    fn fixed_category_plays_at_full_volume_and_ignores_sliders() {
        let mut m = manager();
        assert_eq!(m.category_volume(Category::Fixed), FIXED_CATEGORY_VOLUME);
        assert_eq!(Category::Fixed.name(), "fixed");
        let (id, length) = m
            .play_pcm_stream(tone_stream(2000), "video", 0x19, Category::Fixed)
            .unwrap();
        assert_eq!(length, 2000);
        let voice = m.voice(id).unwrap().clone();
        assert_eq!(
            (
                voice.category,
                voice.boost,
                voice.stream,
                voice.file.as_str()
            ),
            (Category::Fixed, SPEECH_BOOST, true, "video")
        );
        let slot = m.slot_of(id).unwrap();
        // `kat 1998` with doubling: 3996 -> 509 -> FMOD 255.
        assert_eq!(m.mixer().channel(slot).unwrap().volume(), 255);
        m.set_category_volume(Category::Speech, 0);
        m.set_category_volume(Category::Fixed, 0);
        render(&mut m, 4410);
        assert_eq!(m.mixer().channel(slot).unwrap().volume(), 255);
        assert_eq!(m.category_volume(Category::Fixed), FIXED_CATEGORY_VOLUME);
        assert_eq!(
            m.volumes(),
            [349, 599, 0],
            "only the speech slider has moved"
        );
        let out = render(&mut m, 1024);
        assert!(out.iter().any(|&s| s != 0), "the stream sounds");
        // Six mix blocks of 1024 frames have been read.
        assert_eq!(m.position_ms(id), 6 * 1024 * 1000 / 44_100);
        assert_eq!(m.length_ms(id), 2000);
        // The mixer does not accept channel counts other than 1 or 2.
        let wide = Box::new(ResampledStream::new(
            Box::new(|_: &mut Vec<f32>| false),
            SAMPLE_RATE,
            3,
            None,
        ));
        assert!(
            m.play_pcm_stream(wide, "wide", 0, Category::Fixed)
                .is_none()
        );
    }

    #[test]
    fn a_broken_music_file_is_opened_and_reported_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken.ogg");
        std::fs::write(&broken, b"this is not an Ogg file").unwrap();
        let mut m = manager();
        assert_eq!(m.play_file_stream(&broken, "broken", 0, 0, false), None);
        assert_eq!(m.stream_open_attempts, 1);
        // Every further call (the music state machine retries in every tick) does not reopen it.
        for _ in 0..3 {
            assert_eq!(m.play_file_stream(&broken, "broken", 0, 0, false), None);
        }
        assert_eq!(m.stream_open_attempts, 1);
        // Another broken file, on the other hand, is tried.
        let other = dir.path().join("also_broken.ogg");
        std::fs::write(&other, b"empty").unwrap();
        assert_eq!(m.play_file_stream(&other, "also_broken", 0, 0, false), None);
        assert_eq!(m.stream_open_attempts, 2);
    }

    fn render(m: &mut SoundManager, frames: usize) -> Vec<i16> {
        let mut out = vec![0; frames * 2];
        m.render(&mut out);
        out
    }

    #[test]
    fn object_sounds_in_2d_use_the_category_volume_and_no_factor() {
        let (_dir, vfs) = test_vfs();
        let mut m = manager();
        let id = m
            .play_object_2d(&vfs, 100, 0x12, 2215, 0, Category::Sfx)
            .unwrap();
        assert_eq!(m.voice(id).unwrap().file, "a.wav");
        let out = render(&mut m, 1024);
        // SFX 599 -> FMOD 76 -> center 53/53: floor(20000 * 53/256) = 4140.
        assert_eq!(&out[600..602], &[4140, 4140]);
        assert!(
            m.play_object_2d(&vfs, 110, 0x12, 2215, -1, Category::Sfx)
                .is_none()
        );
    }

    #[test]
    fn spatial_object_sounds_use_factor_distance_filter_and_move_instead_of_restarting() {
        let (_dir, vfs) = test_vfs();
        let mut m = manager();
        // Listener at the origin, looking toward +z.
        m.set_listener(Vec3::ZERO, 0.0, Vec3::new(0.0, 500.0, -500.0));
        // 21 tiles away: not started.
        assert!(!m.play_object_3d(
            &vfs,
            200,
            0x113,
            Vec3::new(21.0 * 128.0, 0.0, 0.0),
            4104,
            -1,
            Category::Sfx
        ));
        assert!(m.play_object_3d(
            &vfs,
            200,
            0x113,
            Vec3::new(8.0 * 128.0, 0.0, 0.0),
            4104,
            -1,
            Category::Sfx
        ));
        let (slot, voice) = m.voices().next().map(|(s, v)| (s, v.clone())).unwrap();
        assert_eq!(voice.factor, Some(200));
        let c = m.mixer().channel(slot).unwrap();
        // 599 * 200 / 1000 = 119 -> FMOD 15; 8 tiles to the right -> vol3d 127, fully right.
        assert_eq!((c.volume(), c.vol3d(), c.gains()), (15, 127, (0, 7)));
        // Closer (2 tiles to the left): the same instance is moved, no second sound.
        assert!(!m.play_object_3d(
            &vfs,
            200,
            0x113,
            Vec3::new(-2.0 * 128.0, 0.0, 0.0),
            4104,
            -1,
            Category::Sfx
        ));
        assert_eq!(m.voices().count(), 1);
        // Farther away than the stored priority (2): rejected, no move.
        assert!(!m.play_object_3d(
            &vfs,
            200,
            0x113,
            Vec3::new(0.0, 0.0, 5.0 * 128.0),
            4104,
            -1,
            Category::Sfx
        ));
        render(&mut m, 441);
        let c = m.mixer().channel(slot).unwrap();
        assert_eq!(
            (c.vol3d(), c.gains()),
            (255, (15, 0)),
            "left after the tick"
        );
    }

    // QUIRK D-273: deviation
    #[test]
    fn spatial_events_use_their_entry_factor_and_the_distance_filter() {
        let (_dir, vfs) = test_vfs();
        let mut m = manager();
        m.set_listener(Vec3::ZERO, 0.0, Vec3::new(0.0, 500.0, -500.0));
        let far = Vec3::new(21.0 * 128.0, 0.0, 0.0);
        assert!(!m.play_event_3d(&vfs, "SFX_WATERWALK3", 0x11, far, Category::Sfx));
        assert!(!m.play_event_3d(&vfs, "SFX_WATERWALK9", 0x11, Vec3::ZERO, Category::Sfx));
        assert_eq!(m.voices().count(), 0);
        let near = Vec3::new(8.0 * 128.0, 0.0, 0.0);
        assert!(m.play_event_3d(&vfs, "SFX_WATERWALK3", 0x11, near, Category::Sfx));
        let (slot, voice) = m.voices().next().map(|(s, v)| (s, v.clone())).unwrap();
        // `SoundManager_PlayEvent3D` (`0x00553420`): event -> sound GUID 6 (factor 0.2) as a
        // 3D sample at the position.
        assert_eq!(
            (
                voice.file.as_str(),
                voice.factor,
                voice.flags,
                voice.position
            ),
            ("surf.wav", Some(200), 0x11, Some(near))
        );
        let c = m.mixer().channel(slot).unwrap();
        // Like `play_object_3d`: 599 * 0.2 = 119 -> FMOD 15; 8 tiles to the right.
        assert_eq!((c.volume(), c.vol3d(), c.gains()), (15, 127, (0, 7)));
    }

    #[test]
    fn looping_rain_follows_the_music_volume_and_stops_hard() {
        let (_dir, vfs) = test_vfs();
        let mut m = manager();
        let id = m
            .play_event_2d(&vfs, "SFX_REGEN", 0x14, Category::Music, true)
            .unwrap();
        // A second rain is rejected at the instance limit 1.
        assert!(
            m.play_event_2d(&vfs, "SFX_REGEN", 0x14, Category::Music, true)
                .is_none()
        );
        let out = render(&mut m, 4096);
        // Music 349 -> FMOD 44 -> center 31/31; the loop (441 frames) keeps running.
        assert_eq!(i32::from(out[2 * 3000]), (10000 * 31) >> 8);
        assert!(m.is_playing(id));
        m.set_category_volume(Category::Music, 1000);
        assert_eq!(
            m.mixer().channel(m.slot_of(id).unwrap()).unwrap().volume(),
            44,
            "2D samples do not react"
        );
        m.stop(id);
        assert!(!m.is_playing(id) && m.voices().count() == 0);
    }

    #[test]
    fn finished_sounds_free_their_channel_in_the_next_tick() {
        let (_dir, vfs) = test_vfs();
        let mut m = manager();
        let id = m.play_file_2d(&vfs, "a.wav", 0x12, Category::Sfx).unwrap();
        // 2000 frames: the second block (from frame 1024) reaches the end already while mixing,
        // the next tick (frame 1323) releases the channel.
        render(&mut m, 1300);
        assert!(m.voice(id).is_some(), "not released yet");
        render(&mut m, 100);
        assert!(m.voice(id).is_none());
        // Missing file: silent, no crash.
        assert!(
            m.play_file_2d(&vfs, "missing.wav", 0x12, Category::Sfx)
                .is_none()
        );
    }

    #[test]
    fn rendering_is_independent_of_how_the_frames_are_split() {
        let (_dir, vfs) = test_vfs();
        let run = |chunks: &[usize]| {
            let mut m = manager();
            m.set_listener(Vec3::ZERO, 0.0, Vec3::new(0.0, 500.0, -500.0));
            m.play_object_3d(
                &vfs,
                200,
                0x113,
                Vec3::new(300.0, 0.0, 900.0),
                4104,
                -1,
                Category::Sfx,
            );
            m.play_object_2d(&vfs, 100, 0x12, 2215, -1, Category::Sfx);
            let mut all = Vec::new();
            for &n in chunks {
                all.extend(render(&mut m, n));
            }
            all
        };
        let whole = run(&[5000]);
        assert_eq!(whole, run(&[1000, 4000]));
        assert_eq!(whole, run(&[1, 440, 441, 1024, 3094]));
    }
}
