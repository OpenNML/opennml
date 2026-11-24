//! The user interface language of Windows, the fallback for the language of the own texts when an installation has
//! no detected edition ([`crate::own_texts::language`]).

use opennml_rda::GameVersion;

/// Primary language identifier of German (`LANG_GERMAN`).
#[cfg(windows)]
const LANG_GERMAN: u16 = 0x07;

/// German if the Windows UI language is German, otherwise English (also without Windows).
pub fn system_language() -> GameVersion {
    #[cfg(windows)]
    {
        // SAFETY: no arguments, no pointers.
        let langid = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        if langid & 0x3FF == LANG_GERMAN {
            return GameVersion::German;
        }
    }
    GameVersion::English
}
