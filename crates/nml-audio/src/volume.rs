//! Game volume (re/audio, section "Volume"): categories (`m_Volumes`), the channel formula
//! (`SoundChannel_ComputeVolume` `0x00550800`) and the conversion to the FMOD scale with the
//! halving (`FmodSound_SetVolume` `0x00427D30`) and back (`FmodSound_GetVolume` `0x00426D30`,
//! not the inverse).

/// Volume category of a playback call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Music = 0,
    Sfx = 1,
    Speech = 2,
    /// Category >= 3 without a slider (`SoundManager_GetCategoryVolume` `0x00550AF0`: `kat(k) = 1998` for k >= 3);
    /// in the game only the speech track of the videos (`FUN_0045D560`, category 3), in the reimplementation also the
    /// Bink sound.
    Fixed = 3,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Self::Music => "music",
            Self::Sfx => "sfx",
            Self::Speech => "speech",
            Self::Fixed => "fixed",
        }
    }
}

/// Defaults of `Engine.ini` (`m_Volumes`): music 349, sound effects 599, speech 499.
pub const DEFAULT_VOLUMES: [i32; 3] = [349, 599, 499];
/// Maximum value of the sliders (`resolution` 1000).
pub const MAX_VOLUME: i32 = 1000;
/// `kat(k)` for k >= 3 ([`Category::Fixed`]): fixed, no slider reaches it.
pub const FIXED_CATEGORY_VOLUME: i32 = 1998;

/// `v24` of a channel; dialog speech sets 1000 and thereby compensates for the halving.
pub const SPEECH_BOOST: i32 = 1000;

/// Channel volume on the game scale: `kat · ((1000 + v24) · v28 / 1000) / 1000` (integer, `v28` =
/// 1000), for 3D sounds additionally `· f / 1000` with `f = ftol(factor · 1000)`.
pub fn channel_volume(category_volume: i32, boost: i32, factor: Option<f32>) -> i32 {
    let base = category_volume * ((1000 + boost) * 1000 / 1000) / 1000;
    match factor {
        Some(f) => base * ((f * 1000.0) as i32) / 1000,
        None => base,
    }
}

/// `FmodSound_SetVolume`: `ftol((v div 2) · 0,001 · 255)` (0-255).
pub fn fmod_volume(volume: i32) -> i32 {
    ((f64::from(volume / 2) * 0.001 * 255.0) as i32).clamp(0, 255)
}

/// `FmodSound_GetVolume`: `ftol(FMOD · 1000 / 255)` - does not give back the roughly doubled game
/// scale; fade-outs therefore start at about half the volume.
pub fn fmod_get_volume(fmod: i32) -> i32 {
    (f64::from(fmod) * 1000.0 / 255.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_volumes_give_the_documented_fmod_values() {
        let [music, sfx, speech] = DEFAULT_VOLUMES;
        assert_eq!(fmod_volume(channel_volume(music, 0, None)), 44);
        assert_eq!(fmod_volume(channel_volume(sfx, 0, None)), 76);
        assert_eq!(channel_volume(speech, SPEECH_BOOST, None), 998);
        assert_eq!(fmod_volume(channel_volume(speech, SPEECH_BOOST, None)), 127);
        assert_eq!(fmod_volume(channel_volume(1000, 0, None)), 127);
        assert_eq!(fmod_volume(channel_volume(1000, SPEECH_BOOST, None)), 255);
        // Surf: factor 0.2 -> 599 * 200 / 1000 = 119 -> 59 * 0.255 = 15.
        assert_eq!(channel_volume(sfx, 0, Some(0.2)), 119);
        assert_eq!(fmod_volume(119), 15);
        assert_eq!(fmod_volume(0), 0);
        // Way back: 44 -> 172 instead of 349.
        assert_eq!(fmod_get_volume(44), 172);
        assert_eq!(fmod_get_volume(255), 1000);
    }
}
