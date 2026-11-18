//! Language edition of an installation.

use crate::Vfs;

/// Language edition of the original installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameVersion {
    /// English edition (without the German patch).
    English,
    /// German edition (reference: installation with the German patch, which replaces `NML4.RDA`).
    German,
}

impl GameVersion {
    /// Name of the edition for messages.
    pub fn label(self) -> &'static str {
        match self {
            GameVersion::English => "English",
            GameVersion::German => "German",
        }
    }
}

/// Detects the edition from an entry that only the German `NML4.RDA` contains: the hero voice line
/// `wid_umak_select_06.wav` is missing in the English edition. The size of the archives is not used
/// as a criterion because the corpus tests check it themselves. An installation with a different
/// number of archives counts as unknown (`Err` with a message) instead of silently assuming an
/// edition.
pub fn detect_version(vfs: &Vfs) -> Result<GameVersion, String> {
    let count = vfs.archives().len();
    if count < 11 {
        return Err(format!(
            "incomplete or unknown edition ({count} of 11 game files NML*.RDA)"
        ));
    }
    if count > 11 {
        return Err(format!(
            "unknown edition ({count} game files NML*.RDA instead of 11)"
        ));
    }
    Ok(
        if vfs.exists("data/sprache/helden/wid_umak_select_06.wav") {
            GameVersion::German
        } else {
            GameVersion::English
        },
    )
}
