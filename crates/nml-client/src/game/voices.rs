//! Selection and command acknowledgments as in the original (re/audio, section "Acknowledgments"): the selection
//! sound after every selection change (`FUN_00457980` -> `Selection_PlaySelectSound` `0x0044BB60`) and the
//! voice for the command acknowledgments of the simulation (`SimEvent::UnitResponse`, in the original
//! `Unit_PlayResponseSound` `0x0047BBC0`). Both are 2D from `sounddatabase.dat` of the object type; voice
//! category 8 (flags `0x18`) rejects a new voice while one is running (`nml_audio::pool::ChannelPool`). Testable
//! without a device and GPU; the tests check the playback requests and the channels of the sound manager, never
//! audibly.

use std::collections::VecDeque;

use nml_audio::engine::AudioEngine;
use nml_audio::manager::SoundId;
use nml_audio::volume::Category;
use nml_rda::Vfs;
use nml_sim::id::ObjectId;
use nml_sim::module::building::flags::CONSTRUCTED_FINISHED;
use nml_sim::object::status;
use nml_sim::script::present::Presentation;
use nml_sim::state::{SimEvent, SimState};

/// Action "Selection" of the sound database.
pub const SELECT_ACTION: i32 = 100;
/// Action "while building": selection of a construction site.
pub const BUILDING_SITE_ACTION: i32 = 103;
/// Voices: category 8 (exclusive), at most one instance per sample.
pub const VOICE_FLAGS: u32 = 0x18;
/// Selection sound of buildings and resources: rank 2, one instance per sample.
pub const OBJECT_FLAGS: u32 = 0x12;
/// Neutral slot (animals): their selection sound also plays for the local player.
pub const NEUTRAL_SLOT: u8 = 8;
/// The state snapshot shows this many of the most recently requested voices.
pub const RECENT_VOICES: usize = 8;
/// Module 0 (unit) and 3 (building).
const UNIT: usize = 0;
const BUILDING: usize = 3;

/// Where a request comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceKind {
    /// Selection sound (`Selection_PlaySelectSound`).
    Select,
    /// Command acknowledgment (`Unit_PlayResponseSound`).
    Response,
}

impl VoiceKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Response => "response",
        }
    }
}

/// A playback call `SoundManager_PlayObjectSound2D(action, flags, type, index, category)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceRequest {
    pub kind: VoiceKind,
    pub object: ObjectId,
    /// Type GUID of the object (key of the sound database).
    pub guid: u32,
    pub action: i32,
    pub flags: u32,
    pub category: Category,
    /// Variant: -1 random (generator of the sound selection), otherwise counter value (entry `index % n`).
    pub index: i32,
}

/// A requested voice and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentVoice {
    pub request: VoiceRequest,
    /// File of the sound that was started; `None`: no voice in the database or rejected (e.g. category 8
    /// busy).
    pub file: Option<String>,
}

/// State of the acknowledgments (viewer, not part of the simulation).
#[derive(Debug, Clone, PartialEq)]
pub struct Voices {
    /// `Engine.ini UnitResponse` (`EngineConfig+0x134`): selection voice of units and command
    /// acknowledgment; the selection sound of buildings stays.
    pub unit_response: bool,
    /// GUID lock of the selection sound (`Ui+0x24`): type GUID of the most recently played selection sound.
    pub lock: Option<u32>,
    /// Counter of the acknowledgment variants (`GameWorld+0x2424`): rises with every acknowledgment of an
    /// own unit, even if the sound is rejected.
    pub responses: i32,
    /// Most recently requested voices, oldest first (at most [`RECENT_VOICES`]).
    pub recent: VecDeque<RecentVoice>,
}

impl Default for Voices {
    fn default() -> Self {
        Self::new(true)
    }
}

impl Voices {
    pub fn new(unit_response: bool) -> Self {
        Self {
            unit_response,
            lock: None,
            responses: 0,
            recent: VecDeque::new(),
        }
    }

    /// Level start, loading, restart: resets lock, counter and history (the option stays).
    pub fn reset(&mut self) {
        *self = Self::new(self.unit_response);
    }

    /// Selection changed (`FUN_00457980`): only with exactly one selected object that belongs to the local
    /// player or the neutral slot 8, then `Selection_PlaySelectSound`: dead -> nothing; construction site ->
    /// 103 as noise, finished building, resource etc. -> 100 as noise, both only if the type GUID is not the
    /// most recently played one; unit -> 100 as voice (category 8) only with `UnitResponse`, without a lock
    /// check. Every selection sound that is played - including that of a unit - remembers its type GUID.
    pub fn selection(
        &mut self,
        state: &SimState,
        selection: &[ObjectId],
        local: u8,
    ) -> Option<VoiceRequest> {
        let &[id] = selection else {
            return None;
        };
        let o = state.object(id, false)?;
        if o.owner() != local && o.owner() != NEUTRAL_SLOT {
            return None;
        }
        if o.status & status::DIED != 0 {
            return None;
        }
        let site = o.has_module(BUILDING, false)
            && !o
                .module(BUILDING)
                .is_some_and(|m| m.base.has_flags(CONSTRUCTED_FINISHED));
        let (action, flags, category) = if site {
            (BUILDING_SITE_ACTION, OBJECT_FLAGS, Category::Sfx)
        } else if !o.has_module(UNIT, false) {
            (SELECT_ACTION, OBJECT_FLAGS, Category::Sfx)
        } else if self.unit_response {
            (SELECT_ACTION, VOICE_FLAGS, Category::Speech)
        } else {
            return None;
        };
        if category == Category::Sfx && self.lock == Some(o.guid) {
            return None;
        }
        self.lock = Some(o.guid);
        Some(VoiceRequest {
            kind: VoiceKind::Select,
            object: id,
            guid: o.guid,
            action,
            flags,
            category,
            index: -1,
        })
    }

    /// Command acknowledgments of a step in event order (`Unit_PlayResponseSound`): only with `UnitResponse`
    /// and for units of the local player; each one increments the counter and takes its old value as the
    /// variant. Owner and type come from the event, i.e. from the time of the call (`0x0047BBF8`), not from
    /// the state after the step. In cinemascope (`Engine+0x104`, actions 147/148) the acknowledgment is silent
    /// and does not count; `cinemascope` is the state before the step, the switches of the step apply from
    /// their position in the event sequence.
    pub fn responses(
        &mut self,
        events: &[SimEvent],
        local: u8,
        mut cinemascope: bool,
    ) -> Vec<VoiceRequest> {
        let mut out = Vec::new();
        if !self.unit_response {
            return out;
        }
        for event in events {
            if let SimEvent::Present(Presentation::Cinemascope { on }) = event {
                cinemascope = *on;
                continue;
            }
            if cinemascope {
                continue;
            }
            let SimEvent::UnitResponse {
                unit,
                action,
                owner,
                guid,
            } = *event
            else {
                continue;
            };
            if owner != local {
                continue;
            }
            let index = self.responses;
            self.responses = self.responses.wrapping_add(1);
            out.push(VoiceRequest {
                kind: VoiceKind::Response,
                object: unit,
                guid,
                action,
                flags: VOICE_FLAGS,
                category: Category::Speech,
                index,
            });
        }
        out
    }

    /// Plays a request as a 2D sound (`SoundManager_PlayObjectSound2D`) and records it in the history; `None`
    /// if no voice started.
    pub fn play(
        &mut self,
        audio: &mut AudioEngine,
        vfs: &Vfs,
        request: VoiceRequest,
    ) -> Option<SoundId> {
        let id = audio.sound.play_object_2d(
            vfs,
            request.action,
            request.flags,
            request.guid as i32,
            request.index,
            request.category,
        );
        let file = id.and_then(|id| audio.sound.voice(id).map(|v| v.file.clone()));
        self.recent.push_back(RecentVoice { request, file });
        while self.recent.len() > RECENT_VOICES {
            self.recent.pop_front();
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::economy::testkit::{COLONIST, HOUSE, HQ, TREE, host, site, spawn};
    use nml_audio::db::SoundDb;
    use nml_formats::sound::{parse_sound_database, parse_sound_guid_list};
    use nml_rda::testutil::{TestFile, build_rda};
    use nml_sim::command::{Command, CommandKind};
    use nml_sim::motion::NO_DIRECTION;

    const GOLD_RESOURCE: u32 = crate::game::economy::testkit::GOLD;

    /// WAV PCM16 mono 44.1 kHz with `n` samples.
    fn wav(n: usize) -> Vec<u8> {
        let data: Vec<u8> = std::iter::repeat_n(1000i16, n)
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&44_100u32.to_le_bytes());
        out.extend_from_slice(&88_200u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    /// Sound without a device: colonist with selection (two voices) and acknowledgment (three), HQ with
    /// selection, house with "while building"; all files 0.1 s long.
    fn audio() -> (tempfile::TempDir, Vfs, AudioEngine) {
        let db = SoundDb {
            database: parse_sound_database(
                "GUID 1100\n\tACTIONID 100\n\t\tSOUNDGUID 1\n\t\tSOUNDGUID 2\n\tACTIONID 12\n\t\tSOUNDGUID 3\n\t\tSOUNDGUID 4\n\t\tSOUNDGUID 5\nGUID 1000\n\tACTIONID 100\n\t\tSOUNDGUID 6\nGUID 1001\n\tACTIONID 103\n\t\tSOUNDGUID 7\n",
            ),
            sounds: parse_sound_guid_list(
                "1=1.0:kol_select_1.wav\n2=1.0:kol_select_2.wav\n3=1.0:kol_done_1.wav\n4=1.0:kol_done_2.wav\n5=1.0:kol_done_3.wav\n6=1.0:hq.wav\n7=1.0:bauen.wav\n",
            ),
            ..SoundDb::default()
        };
        let names = [
            "kol_select_1.wav",
            "kol_select_2.wav",
            "kol_done_1.wav",
            "kol_done_2.wav",
            "kol_done_3.wav",
            "hq.wav",
            "bauen.wav",
        ];
        let data = wav(4410);
        let files: Vec<TestFile<'_>> = names
            .iter()
            .map(|path| TestFile {
                path,
                data: &data,
                compress: false,
            })
            .collect();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("nml0.rda"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs, AudioEngine::new(db, None))
    }

    fn speech(audio: &AudioEngine) -> Vec<String> {
        audio
            .sound
            .voices()
            .filter(|(_, v)| v.category == Category::Speech)
            .map(|(_, v)| v.file.clone())
            .collect()
    }

    #[test]
    fn one_own_unit_speaks_on_selection_two_or_foreign_ones_stay_silent() {
        let mut host = host();
        let a = spawn(&mut host, COLONIST, 0, (10, 10));
        let b = spawn(&mut host, COLONIST, 0, (12, 10));
        let foreign = spawn(&mut host, COLONIST, 1, (14, 10));
        let animal = spawn(&mut host, COLONIST, NEUTRAL_SLOT as i32, (16, 10));
        let mut v = Voices::default();
        let r = v.selection(&host.state, &[a], 0).unwrap();
        assert_eq!(
            (
                r.kind, r.object, r.guid, r.action, r.flags, r.category, r.index
            ),
            (
                VoiceKind::Select,
                a,
                COLONIST,
                SELECT_ACTION,
                0x18,
                Category::Speech,
                -1
            )
        );
        // Units do not check the lock: the same unit speaks again.
        assert!(v.selection(&host.state, &[a], 0).is_some());
        assert_eq!(v.selection(&host.state, &[a, b], 0), None);
        assert_eq!(v.selection(&host.state, &[], 0), None);
        assert_eq!(v.selection(&host.state, &[foreign], 0), None);
        // Animals (slot 8) also speak for the local player; for slot 1, `foreign` is own.
        assert!(v.selection(&host.state, &[animal], 0).is_some());
        assert!(v.selection(&host.state, &[foreign], 1).is_some());
        // Dead: nothing.
        host.state.tables[0].objects[a.index()]
            .as_mut()
            .unwrap()
            .status |= status::DIED;
        assert_eq!(v.selection(&host.state, &[a], 0), None);
        // Without `UnitResponse` units are silent, buildings are not.
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let mut off = Voices::new(false);
        assert_eq!(off.selection(&host.state, &[b], 0), None);
        let r = off.selection(&host.state, &[hq], 0).unwrap();
        assert_eq!(
            (r.action, r.flags, r.category),
            (SELECT_ACTION, 0x12, Category::Sfx)
        );
    }

    #[test]
    fn building_selections_are_locked_per_type_guid_and_units_move_the_lock() {
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let unit = spawn(&mut host, COLONIST, 0, (10, 10));
        let house_site = site(&mut host, HOUSE, 0, (20, 10));
        let gold = spawn(&mut host, GOLD_RESOURCE, 0, (40, 40));
        let mut v = Voices::default();
        assert_eq!(v.selection(&host.state, &[hq], 0).unwrap().action, 100);
        assert_eq!(v.lock, Some(HQ));
        assert_eq!(v.selection(&host.state, &[hq], 0), None, "lock");
        // A unit in between sets the lock to its type (`0x0044BBBC`): the town hall plays again
        // afterwards.
        assert!(v.selection(&host.state, &[unit], 0).is_some());
        assert_eq!(v.lock, Some(COLONIST));
        assert!(v.selection(&host.state, &[hq], 0).is_some());
        // Without `UnitResponse` the unit does not play and leaves the lock in place.
        v.unit_response = false;
        assert_eq!(v.selection(&host.state, &[unit], 0), None);
        assert_eq!(v.selection(&host.state, &[hq], 0), None);
        // Construction site: 103 as noise; resource (not a unit) 100.
        let r = v.selection(&host.state, &[house_site], 0).unwrap();
        assert_eq!(
            (r.action, r.flags, r.category),
            (BUILDING_SITE_ACTION, 0x12, Category::Sfx)
        );
        assert_eq!(v.selection(&host.state, &[house_site], 0), None);
        assert_eq!(v.selection(&host.state, &[gold], 0).unwrap().action, 100);
        v.reset();
        assert_eq!(v.lock, None);
    }

    /// Acknowledgment "Execute" (12) of the unit `unit` with its owner and type as in
    /// `nml_sim::command::respond`.
    fn response(state: &SimState, unit: ObjectId) -> SimEvent {
        let o = state.object(unit, false).unwrap();
        SimEvent::UnitResponse {
            unit,
            action: 12,
            owner: o.owner(),
            guid: o.guid,
        }
    }

    /// Owner and type apply at the time of the call (`0x0047BBF8`): if the unit disappears in the same step
    /// or changes owner afterwards, it speaks anyway and counts.
    #[test]
    fn responses_use_owner_and_type_at_the_time_of_the_command() {
        let mut host = host();
        let gone = spawn(&mut host, COLONIST, 0, (10, 10));
        let handed_over = spawn(&mut host, COLONIST, 0, (12, 10));
        let events = [
            response(&host.state, gone),
            response(&host.state, handed_over),
        ];
        let rules = host.rules.clone();
        nml_sim::world::destroy_object(&mut nml_sim::sim::Sim::new(&mut host.state, &rules), gone);
        assert!(host.state.object(gone, false).is_none());
        let mut v = Voices::default();
        let r = v.responses(&events, 0, false);
        assert_eq!(
            r.iter()
                .map(|r| (r.object, r.guid, r.index))
                .collect::<Vec<_>>(),
            [(gone, COLONIST, 0), (handed_over, COLONIST, 1)]
        );
        // A foreign acknowledgment does not count, even if the unit now belongs to the local player.
        let foreign = SimEvent::UnitResponse {
            unit: handed_over,
            action: 12,
            owner: 1,
            guid: COLONIST,
        };
        assert!(v.responses(&[foreign], 0, false).is_empty());
        assert_eq!(v.responses, 2);
    }

    #[test]
    fn responses_count_in_turn_for_own_units_only_and_need_unit_response() {
        let mut host = host();
        let w: Vec<_> = (0..3)
            .map(|i| spawn(&mut host, COLONIST, 0, (10 + 2 * i, 10)))
            .collect();
        let foreign = spawn(&mut host, COLONIST, 1, (20, 20));
        let event = |unit| response(&host.state, unit);
        let events = [
            event(w[0]),
            event(foreign),
            event(w[1]),
            SimEvent::Created { id: w[2] },
            event(w[2]),
        ];
        let mut v = Voices {
            responses: 5,
            ..Voices::default()
        };
        let r = v.responses(&events, 0, false);
        assert_eq!(
            r.iter()
                .map(|r| (r.object, r.action, r.index, r.flags, r.category))
                .collect::<Vec<_>>(),
            [
                (w[0], 12, 5, 0x18, Category::Speech),
                (w[1], 12, 6, 0x18, Category::Speech),
                (w[2], 12, 7, 0x18, Category::Speech),
            ]
        );
        assert_eq!(v.responses, 8, "the foreign unit does not count");
        let mut off = Voices::new(false);
        assert!(off.responses(&events, 0, false).is_empty());
        assert_eq!(off.responses, 0);
    }

    /// `Unit_PlayResponseSound` (`0x0047BBC0`) is silent in cinemascope (`Engine+0x104`) and then does not
    /// count; 147/148 in the step apply from their position in the event sequence.
    #[test]
    fn responses_are_silent_during_cinemascope() {
        let mut host = host();
        let w: Vec<_> = (0..3)
            .map(|i| spawn(&mut host, COLONIST, 0, (10 + 2 * i, 10)))
            .collect();
        let event = |unit| response(&host.state, unit);
        let scope = |on| SimEvent::Present(Presentation::Cinemascope { on });
        let mut v = Voices::default();
        assert!(v.responses(&[event(w[0])], 0, true).is_empty());
        assert_eq!(v.responses, 0, "nothing counts in cinemascope");
        let events = [
            event(w[0]),
            scope(true),
            event(w[1]),
            scope(false),
            event(w[2]),
        ];
        let r = v.responses(&events, 0, false);
        assert_eq!(
            r.iter().map(|r| (r.object, r.index)).collect::<Vec<_>>(),
            [(w[0], 0), (w[2], 1)]
        );
        let r = v.responses(&[scope(false), event(w[1])], 0, true);
        assert_eq!(r.iter().map(|r| r.object).collect::<Vec<_>>(), [w[1]]);
    }

    #[test]
    fn only_the_first_voice_of_a_command_plays_and_a_running_voice_is_not_cut() {
        let (_dir, vfs, mut audio) = audio();
        let mut host = host();
        let w: Vec<_> = (0..3)
            .map(|i| spawn(&mut host, COLONIST, 0, (10 + 2 * i, 10)))
            .collect();
        let mut v = Voices::default();
        // Selection of a unit: voice runs (category 8).
        let select = v.selection(&host.state, &[w[0]], 0).unwrap();
        assert!(v.play(&mut audio, &vfs, select).is_some());
        let selected = speech(&audio);
        assert_eq!(selected.len(), 1);
        assert!(selected[0].starts_with("kol_select_"));
        // Command to three units while the selection voice runs: all rejected, the counter runs anyway.
        let events: Vec<_> = w.iter().map(|&unit| response(&host.state, unit)).collect();
        for r in v.responses(&events, 0, false) {
            assert_eq!(v.play(&mut audio, &vfs, r), None);
        }
        assert_eq!((v.responses, speech(&audio)), (3, selected));
        // After the voice ends only the first unit speaks, with variant 3 % 3.
        for _ in 0..20 {
            audio.advance(&vfs, 10.0);
        }
        assert!(speech(&audio).is_empty());
        let played: Vec<_> = v
            .responses(&events, 0, false)
            .into_iter()
            .map(|r| v.play(&mut audio, &vfs, r).is_some())
            .collect();
        assert_eq!(played, [true, false, false]);
        assert_eq!(speech(&audio), ["kol_done_1.wav"]);
        assert_eq!(v.responses, 6);
        // History: all seven requests, with a file only if a voice started.
        assert_eq!(v.recent.len(), 7);
        let last: Vec<_> = v.recent.iter().rev().take(3).collect();
        assert_eq!(last[2].file.as_deref(), Some("kol_done_1.wav"));
        assert_eq!((last[0].file.as_ref(), last[0].request.index), (None, 5));
    }

    #[test]
    fn building_selection_sounds_play_as_2d_noises() {
        let (_dir, vfs, mut audio) = audio();
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let mut v = Voices::default();
        let r = v.selection(&host.state, &[hq], 0).unwrap();
        let id = v.play(&mut audio, &vfs, r).unwrap();
        let voice = audio.sound.voice(id).unwrap();
        assert_eq!(
            (
                voice.file.as_str(),
                voice.category,
                voice.flags,
                voice.position
            ),
            ("hq.wav", Category::Sfx, 0x12, None)
        );
    }

    #[test]
    fn the_simulation_reports_direct_commands_and_the_viewer_keeps_its_own_units() {
        let mut host = host();
        let w = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let walk = CommandKind::Move {
            units: vec![w],
            x: 30 << 12,
            z: 30 << 12,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: nml_sim::command::NO_SPREAD,
            flags: 0,
        };
        let mut v = Voices::default();
        // Queued: no acknowledgment on submission.
        host.submit(Command {
            player: 0,
            kind: CommandKind::Queue {
                cyclic: false,
                inner: Box::new(walk),
            },
        });
        let events = host.step_once();
        assert!(v.responses(&events, 0, false).is_empty());
        // Direct: chopping wood acknowledges in the execution tick, not for slot 1.
        host.submit(Command {
            player: 0,
            kind: CommandKind::ChopWood {
                tree,
                units: vec![w],
            },
        });
        let events = host.step_once();
        assert!(v.responses(&events, 1, false).is_empty());
        let r = v.responses(&events, 0, false);
        assert_eq!(
            r.iter().map(|r| (r.object, r.action)).collect::<Vec<_>>(),
            [(w, 12)]
        );
    }
}
