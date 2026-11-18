//! Driver list of the sound options (`SOUNDOPTIONS`, combo box `sounddriver`, index = `m_DriverID`) and choice of the
//! output device (re/audio, section "Driver selection").
//!
//! When creating the sound device, the original enumerates the FMOD drivers (`FmodSound_EnumerateDrivers` `0x004274B0`:
//! `FSOUND_GetDriverName(i)` for `i < FSOUND_GetNumDrivers()`); the combo box shows them in that order
//! (`SoundManager_FillDriverList` `0x00551310`, per name `FUN_005600D0`: ANSI bytes sign-extended to UTF-16,
//! the low byte is drawn). FMOD 3 with DirectSound output takes the list from `DirectSoundEnumerateA`:
//! entry 0 is the primary driver without a GUID (localized by Windows: "Primärer Soundtreiber" on German,
//! "Primary Sound Driver" on English Windows), followed by each device's description.
//! `FSOUND_SetDriver(m_DriverID)` selects the device (`SoundManager_InitDevice` `0x00551740`).
//!
//! On Windows the reimplementation likewise gets the list from DirectSound and plays through WASAPI (`cpal`) on the
//! endpoint with the same GUID; entry 0 is the default device. Without Windows the list comes from `cpal`.

/// Device behind an entry of the driver list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverKey {
    /// Primary driver: the system's default output device.
    Default,
    /// A specific device: on Windows the DirectSound GUID (lowercase, without braces), which is also part of the ID of
    /// the WASAPI endpoint; otherwise the device ID from `cpal`.
    Device(String),
}

/// An entry of the driver list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundDriver {
    /// Display name as in the original (one character U+00XX per ANSI byte, like `FUN_005600D0` with the low byte).
    pub name: String,
    pub key: DriverKey,
}

/// The system's drivers in the original's order (empty if the enumeration fails).
pub fn enumerate() -> Vec<SoundDriver> {
    let drivers = system::enumerate();
    for (i, d) in drivers.iter().enumerate() {
        log::info!("Sound: driver {i} \"{}\" ({:?})", d.name, d.key);
    }
    drivers
}

/// Display names of the list (`MenuOptions.sound_drivers`).
pub fn names(drivers: &[SoundDriver]) -> Vec<String> {
    drivers.iter().map(|d| d.name.clone()).collect()
}

/// Device for `m_DriverID`. If the index lies outside the list (the selected device has vanished since the last
/// start), the default device is used and the second field is `true`. The original would stay silent then:
/// `FSOUND_SetDriver` fails and `SoundManager_InitDevice` aborts (D-372).
pub fn select(drivers: &[SoundDriver], index: i32) -> (DriverKey, bool) {
    match usize::try_from(index).ok().and_then(|i| drivers.get(i)) {
        Some(d) => (d.key.clone(), false),
        None => (DriverKey::Default, true),
    }
}

/// Does the `cpal` ID `id` belong to the key `key` of a specific device? On Windows the ID of the WASAPI
/// endpoint is `{flow direction and kind}.{GUID}` (e.g. `{0.0.0.00000000}.{…}`, over Remote Desktop
/// `{3.0.0.00000001}.{…}`); DirectSound reports the same device with this GUID.
pub fn device_matches(id: &str, key: &str) -> bool {
    if cfg!(windows) {
        id.to_ascii_lowercase().contains(&format!("{{{key}}}"))
    } else {
        id == key
    }
}

/// Display name from ANSI bytes: per byte the character U+00XX (same low byte as in the original).
pub fn ansi_name(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// GUID as text like in the WASAPI IDs: lowercase, with hyphens, without braces.
pub fn guid_text(guid: u128) -> String {
    let hex = format!("{guid:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(windows)]
mod system {
    use std::ffi::c_void;

    use windows::Win32::Media::Audio::DirectSound::DirectSoundEnumerateA;
    use windows::core::{BOOL, GUID, PCSTR};

    use super::{DriverKey, SoundDriver, ansi_name, guid_text};

    /// Callback of `DirectSoundEnumerateA`: one entry per call, continue with `TRUE`. Must not panic
    /// (callback across the system boundary).
    unsafe extern "system" fn add(
        guid: *mut GUID,
        description: PCSTR,
        _module: PCSTR,
        context: *mut c_void,
    ) -> BOOL {
        // SAFETY: `context` is the `Vec` from `enumerate`, which lives during the call; DirectSound calls
        // synchronously.
        let list = unsafe { &mut *context.cast::<Vec<SoundDriver>>() };
        let name = if description.is_null() {
            String::new()
        } else {
            // SAFETY: DirectSound passes a null-terminated string.
            ansi_name(unsafe { description.as_bytes() })
        };
        let key = if guid.is_null() {
            DriverKey::Default
        } else {
            // SAFETY: not null, valid for the duration of the call as passed by DirectSound.
            DriverKey::Device(guid_text(unsafe { (*guid).to_u128() }))
        };
        list.push(SoundDriver { name, key });
        BOOL::from(true)
    }

    pub fn enumerate() -> Vec<SoundDriver> {
        let mut list: Vec<SoundDriver> = Vec::new();
        let context: *mut Vec<SoundDriver> = &mut list;
        // SAFETY: `add` matches the signature `LPDSENUMCALLBACKA`, `context` points to `list` (lives past the call).
        if let Err(e) =
            unsafe { DirectSoundEnumerateA(Some(add), Some(context.cast_const().cast())) }
        {
            log::warn!("Sound: driver list from DirectSound not read ({e})");
        }
        list
    }
}

#[cfg(not(windows))]
mod system {
    use cpal::traits::{DeviceTrait, HostTrait};

    use super::{DriverKey, SoundDriver};

    /// Without DirectSound: entry 0 is the default device (with its name), followed by all output devices from `cpal`.
    pub fn enumerate() -> Vec<SoundDriver> {
        let host = cpal::default_host();
        let name = |d: &cpal::Device| d.description().map(|d| d.name().to_string()).ok();
        let mut list = vec![SoundDriver {
            name: host
                .default_output_device()
                .as_ref()
                .and_then(name)
                .unwrap_or_else(|| "Default device".to_string()),
            key: DriverKey::Default,
        }];
        if let Ok(devices) = host.output_devices() {
            for device in devices {
                if let (Some(n), Ok(id)) = (name(&device), device.id()) {
                    list.push(SoundDriver {
                        name: n,
                        key: DriverKey::Device(id.id().to_string()),
                    });
                }
            }
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, key: &str) -> SoundDriver {
        SoundDriver {
            name: name.into(),
            key: DriverKey::Device(key.into()),
        }
    }

    // QUIRK D-372: deviation
    #[test]
    fn select_takes_the_entry_and_falls_back_to_the_default_device() {
        let drivers = vec![
            SoundDriver {
                name: "Primary Sound Driver".into(),
                key: DriverKey::Default,
            },
            device("Speakers", "a"),
            device("Headphones", "b"),
        ];
        assert_eq!(select(&drivers, 0), (DriverKey::Default, false));
        assert_eq!(select(&drivers, 2), (DriverKey::Device("b".into()), false));
        // Device gone (index past the list), nonsense from the file, empty list: default device with a notice.
        assert_eq!(select(&drivers, 3), (DriverKey::Default, true));
        assert_eq!(select(&drivers, -1), (DriverKey::Default, true));
        assert_eq!(select(&[], 0), (DriverKey::Default, true));
        assert_eq!(
            names(&drivers),
            ["Primary Sound Driver", "Speakers", "Headphones"]
        );
    }

    #[test]
    fn ansi_bytes_map_to_the_same_low_byte() {
        // "Primärer" in cp1252: ä = 0xE4; 0x99 ("™" in cp1252) stays U+0099 like the low byte in the original.
        assert_eq!(
            ansi_name(b"Prim\xE4rer Soundtreiber"),
            "Primärer Soundtreiber"
        );
        assert_eq!(ansi_name(b"X\x99").chars().nth(1), Some('\u{99}'));
    }

    #[test]
    fn guid_text_matches_the_wasapi_endpoint_id() {
        let key = guid_text(0x0123_4567_89AB_CDEF_0011_2233_4455_6677);
        assert_eq!(key, "01234567-89ab-cdef-0011-223344556677");
        if cfg!(windows) {
            assert!(device_matches(
                "{3.0.0.00000001}.{01234567-89AB-CDEF-0011-223344556677}",
                &key
            ));
            assert!(!device_matches(
                "{0.0.0.00000000}.{01234567-89ab-cdef-0011-223344556678}",
                &key
            ));
        }
    }
}
