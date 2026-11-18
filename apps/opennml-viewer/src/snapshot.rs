//! Conversion of viewer values into the types of the state snapshot (`opennml_control`), without winit and
//! GPU (testable). The state snapshot is assembled in `App::snapshot`.

use std::collections::VecDeque;

use opennml_audio::capture::CaptureSummary;
use opennml_audio::engine::AudioEngine;
use opennml_audio::spatial::Pan;
use opennml_client::clock::ClockMode;
use opennml_control::{
    AspectName, AudioState, CameraState, CaptureState, CaptureSummaryData,
    ClockMode as ClockModeName, EffectState, FreeCameraState, HoveredObject, MusicState,
    RecentSoundState, RecentVoiceState, ShadowState, VoiceState, WeatherName,
};
use opennml_effects::world::EffectCounts;
use opennml_render::ShadowStats;
use opennml_world::camera::GameCamera;
use opennml_world::environment::WeatherKind;
use opennml_world::level::PlacedObject;

use crate::config::AspectSetting;
use opennml_client::game::input::FreeCamera;
use opennml_client::game::notices::SoundRequest;
use opennml_client::game::voices::Voices;

pub fn aspect_name(aspect: AspectSetting) -> AspectName {
    match aspect {
        AspectSetting::Correct => AspectName::Correct,
        AspectSetting::Original43 => AspectName::Original43,
        AspectSetting::Stretch => AspectName::Stretch,
    }
}

pub fn weather_name(kind: Option<WeatherKind>) -> WeatherName {
    match kind {
        None => WeatherName::None,
        Some(WeatherKind::Rain) => WeatherName::Rain,
        Some(WeatherKind::Snow) => WeatherName::Snow,
    }
}

pub fn weather_kind(name: WeatherName) -> Option<WeatherKind> {
    match name {
        WeatherName::None => None,
        WeatherName::Rain => Some(WeatherKind::Rain),
        WeatherName::Snow => Some(WeatherKind::Snow),
    }
}

pub fn clock_mode_name(mode: ClockMode) -> ClockModeName {
    match mode {
        ClockMode::Realtime => ClockModeName::Realtime,
        ClockMode::Paused => ClockModeName::Paused,
    }
}

/// Original camera: distance, pitch, rotation (direction target -> eye, `atan2(x, z)` in degrees),
/// target and eye.
pub fn camera_state(camera: &GameCamera) -> CameraState {
    let offset = camera.eye - camera.target;
    CameraState {
        distance: camera.distance(),
        pitch_deg: camera.pitch_deg(),
        yaw_deg: offset.x.atan2(offset.z).to_degrees(),
        target: camera.target.to_array(),
        eye: camera.eye.to_array(),
    }
}

pub fn free_camera_state(free: &FreeCamera) -> FreeCameraState {
    FreeCameraState {
        position: free.position.to_array(),
        yaw_deg: free.yaw.to_degrees(),
        pitch_deg: free.pitch.to_degrees(),
    }
}

/// Overlay line of the game object under the mouse.
pub fn describe_object(o: &PlacedObject) -> String {
    let player = o.player.map_or("-".to_string(), |p| p.to_string());
    format!(
        "{} (GUID {}) #{} | Player {player} | ({:.0}, {:.0}, {:.0}) | {}",
        o.type_name, o.guid, o.index, o.position.x, o.position.y, o.position.z, o.model
    )
}

/// The same object, structured for the state snapshot, with base module and shadow kind.
pub fn object_state(o: &PlacedObject, base_module: Option<&str>, shadow_kind: u8) -> HoveredObject {
    HoveredObject {
        type_name: o.type_name.clone(),
        guid: o.guid,
        index: o.index,
        player: o.player,
        position: o.position.to_array(),
        model: o.model.clone(),
        base_module: base_module.map(str::to_string),
        parts: o.parts.clone(),
        shadow_kind,
        sim_id: o.sim_id,
    }
}

/// `particles_quality` reported by the model and effect browsers: they create all effects
/// regardless of the setting.
pub const BROWSER_EFFECT_QUALITY: u8 = 2;

/// Effect counts for the state snapshot.
pub fn effect_state(
    quality: u8,
    counts: &EffectCounts,
    quads: usize,
    models: usize,
) -> EffectState {
    EffectState {
        quality,
        owned: counts.owned,
        world: counts.world,
        surf: counts.surf,
        fish: counts.fish,
        eagles: counts.eagles,
        particles: counts.particles,
        frozen: counts.frozen,
        quads,
        models,
    }
}

/// Shadow counts for the state snapshot.
pub fn shadow_state(quality: u8, stats: &ShadowStats) -> ShadowState {
    ShadowState {
        quality,
        kinds: stats.kinds,
        decals: stats.decals,
        groundplanes: stats.groundplanes,
        specials: stats.specials,
        persistent: stats.persistent,
        per_frame: stats.per_frame,
        rendered: stats.rendered,
        atlas_used: stats.atlas_used,
        atlas_capacity: stats.atlas_capacity,
        atlas_full: stats.atlas_full,
    }
}

/// This many combat sounds are kept by `audio.recent_sounds` (protocol 11).
pub const RECENT_SOUNDS: usize = 16;

/// Remembers a requested combat sound (`Sound3d`, also when muted) for `audio.recent_sounds`; the
/// older ones drop out after [`RECENT_SOUNDS`] entries. Other sounds are ignored.
pub fn record_recent_sound(recent: &mut VecDeque<RecentSoundState>, request: &SoundRequest) {
    let SoundRequest::Object3d {
        action,
        flags,
        guid,
        object,
        ..
    } = *request
    else {
        return;
    };
    recent.push_back(RecentSoundState {
        action,
        flags,
        guid,
        object,
    });
    while recent.len() > RECENT_SOUNDS {
        recent.pop_front();
    }
}

/// The `audio` section of the state snapshot.
/// Sound including selection and command replies ([`Voices`]) and the last combat sounds.
pub fn audio_state(
    audio: &AudioEngine,
    replies: &Voices,
    sounds: &VecDeque<RecentSoundState>,
) -> AudioState {
    let sound = &audio.sound;
    let voices = sound
        .voices()
        .filter_map(|(channel, voice)| {
            let c = sound.mixer().channel(channel)?;
            let (l, r) = c.gains();
            Some(VoiceState {
                id: voice.id,
                channel,
                file: voice.file.clone(),
                category: voice.category.name().into(),
                stream: voice.stream,
                position: voice.position.map(|p| p.to_array()),
                volume: c.volume(),
                vol3d: c.vol3d(),
                pan: match c.pan() {
                    Pan::Position(p) => Some(p),
                    Pan::Stereo => None,
                },
                gains: [l, r],
                fading: voice.fade.is_some_and(|f| f.active),
            })
        })
        .collect();
    let music = &audio.music;
    AudioState {
        device: audio.device().map(Into::into),
        mute: audio.mute(),
        volumes: sound.volumes(),
        listener: sound.listener_tiles().to_array(),
        frames: sound.frames(),
        voices,
        music: MusicState {
            status: music.status.name().into(),
            playing: music.playing().map(Into::into),
            current: music.current.clone(),
            attack_track: music.attack.clone(),
            attack_enabled: audio.attack_override(),
            attack_points: audio.attack_points(),
            attack_source: if audio.attack_override() {
                "manual"
            } else {
                "sim"
            }
            .into(),
            timer_ms: music.timer,
            pause_ms: music.pause,
            menu: audio.menu.playing(sound).map(Into::into),
        },
        capture: audio.capture_status().map(|(path, frames)| CaptureState {
            path: path.display().to_string(),
            frames,
        }),
        unit_response: replies.unit_response,
        responses: replies.responses,
        select_lock: replies.lock,
        recent_voices: replies
            .recent
            .iter()
            .map(|v| RecentVoiceState {
                kind: v.request.kind.name().into(),
                object: v.request.object.0,
                guid: v.request.guid,
                action: v.request.action,
                flags: v.request.flags,
                category: v.request.category.name().into(),
                index: v.request.index,
                file: v.file.clone(),
            })
            .collect(),
        recent_sounds: sounds.iter().cloned().collect(),
    }
}

/// Result of `audio_capture stop`.
pub fn capture_data(summary: CaptureSummary) -> CaptureSummaryData {
    CaptureSummaryData {
        path: summary.path.display().to_string(),
        frames: summary.frames,
        length_ms: summary.length_ms,
        peak: summary.peak,
        rms: summary.rms,
        envelope: summary.envelope,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Affine3A, Vec3};
    use opennml_world::level::ObjectClass;

    fn palm() -> PlacedObject {
        PlacedObject {
            index: 12,
            guid: 1234,
            class: ObjectClass::Game,
            type_name: "Palm".into(),
            name: None,
            model: "data/r3d/palme.rdo".into(),
            player: Some(8),
            parts: vec![1234],
            no_shadow: false,
            position: Vec3::new(4100.4, 120.0, 2049.6),
            rotation: Vec3::ZERO,
            transform: Affine3A::IDENTITY,
            sim_id: None,
        }
    }

    #[test]
    fn hovered_object_line_and_state_describe_the_same_object() {
        assert_eq!(
            describe_object(&palm()),
            "Palm (GUID 1234) #12 | Player 8 | (4100, 120, 2050) | data/r3d/palme.rdo"
        );
        let state = object_state(&palm(), Some("Natur"), 1);
        assert_eq!(state.base_module.as_deref(), Some("Natur"));
        assert_eq!((state.parts.clone(), state.shadow_kind), (vec![1234], 1));
        assert_eq!(
            (
                state.type_name.as_str(),
                state.guid,
                state.index,
                state.player
            ),
            ("Palm", 1234, 12, Some(8))
        );
        assert_eq!(state.position, [4100.4, 120.0, 2049.6]);
        let neutral = PlacedObject {
            player: None,
            ..palm()
        };
        assert!(describe_object(&neutral).contains("Player -"));
    }

    #[test]
    fn camera_yaw_follows_the_eye_around_the_target() {
        let (mut camera, _) = opennml_client::game::input::testkit::setup();
        let before = camera_state(&camera);
        assert!((before.distance - camera.distance()).abs() < 1e-3);
        assert_eq!(before.target, camera.target.to_array());
        // `GameCamera::yaw` rotates by angle * 1.5 with the game's rotation matrix (+x -> +z, quirk O-331); `yaw_deg`
        // (`atan2(x, z)`) decreases in the process.
        assert!(camera.yaw(0.1));
        let after = camera_state(&camera);
        let turned = (after.yaw_deg - before.yaw_deg + 540.0) % 360.0 - 180.0;
        assert!((turned + 0.15_f32.to_degrees()).abs() < 1e-2, "{turned}");
        assert!((after.pitch_deg - before.pitch_deg).abs() < 1e-3);
    }

    #[test]
    fn free_camera_angles_are_degrees() {
        let free = FreeCamera {
            position: Vec3::new(1.0, 2.0, 3.0),
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: -0.5,
        };
        let state = free_camera_state(&free);
        assert_eq!(state.position, [1.0, 2.0, 3.0]);
        assert!((state.yaw_deg - 90.0).abs() < 1e-4);
        assert!((state.pitch_deg + 0.5_f32.to_degrees()).abs() < 1e-4);
    }

    #[test]
    fn recent_sounds_keep_the_last_sixteen_combat_sounds_only() {
        let mut recent = VecDeque::new();
        let sound = |n: u32| SoundRequest::Object3d {
            action: n as i32,
            flags: 0x12,
            position: Vec3::ZERO,
            guid: 1107,
            object: 0x0001_0000 + n,
        };
        record_recent_sound(&mut recent, &SoundRequest::Message(872));
        assert!(recent.is_empty(), "combat sounds only");
        for n in 0..20 {
            record_recent_sound(&mut recent, &sound(n));
        }
        assert_eq!(recent.len(), RECENT_SOUNDS);
        assert_eq!(
            recent.front(),
            Some(&RecentSoundState {
                action: 4,
                flags: 0x12,
                guid: 1107,
                object: 0x0001_0004,
            }),
            "the oldest four dropped out"
        );
        assert_eq!(recent.back().map(|s| s.action), Some(19));
    }

    #[test]
    fn names_convert_both_ways() {
        assert_eq!(
            aspect_name(AspectSetting::Original43),
            AspectName::Original43
        );
        assert_eq!(clock_mode_name(ClockMode::Paused), ClockModeName::Paused);
        for kind in [None, Some(WeatherKind::Rain), Some(WeatherKind::Snow)] {
            assert_eq!(weather_kind(weather_name(kind)), kind);
        }
    }
}
