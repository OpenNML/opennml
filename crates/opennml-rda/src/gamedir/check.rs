//! Checks whether a folder is a usable installation.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::{GameVersion, RdaError, Vfs, detect_version};

/// Why a folder does not count as an installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The folder does not exist.
    Missing,
    /// The folder contains no archives.
    NoArchives,
    /// The archives cannot be opened.
    Unreadable(String),
    /// The edition is unknown (message from `detect_version`).
    UnknownVersion(String),
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejection::Missing => write!(f, "not present"),
            Rejection::NoArchives => {
                write!(f, "no No Man's Land game data (NML0.RDA missing)")
            }
            Rejection::Unreadable(e) => write!(f, "game data not readable: {e}"),
            Rejection::UnknownVersion(m) => write!(f, "{m}"),
        }
    }
}

/// How strictly to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// Explicitly given location: an unknown edition is only reported.
    Explicit,
    /// Found location: an unknown edition rejects the candidate.
    Found,
}

/// Checks `dir`; `Ok(None)` means: usable, edition unknown (only for `Explicit`).
pub fn check_candidate(
    dir: &Path,
    strictness: Strictness,
) -> Result<Option<GameVersion>, Rejection> {
    if !dir.is_dir() {
        return Err(Rejection::Missing);
    }
    let vfs = Vfs::open_game_dir(dir).map_err(|e| match e {
        RdaError::NoArchives(_) => Rejection::NoArchives,
        other => Rejection::Unreadable(other.to_string()),
    })?;
    match detect_version(&vfs) {
        Ok(version) => Ok(Some(version)),
        Err(message) => match strictness {
            Strictness::Found => Err(Rejection::UnknownVersion(message)),
            Strictness::Explicit => {
                log::warn!("{}: {message}; continuing anyway", dir.display());
                Ok(None)
            }
        },
    }
}

/// Checks a folder chosen by the user: `dir` itself, otherwise `dir/No Man's Land`.
/// If both fail, the reason for `dir` is returned.
pub fn check_choice(dir: &Path) -> Result<(PathBuf, GameVersion), Rejection> {
    let first = match check_candidate(dir, Strictness::Found) {
        Ok(Some(version)) => return Ok((dir.to_path_buf(), version)),
        Ok(None) => Rejection::UnknownVersion("edition unknown".to_string()),
        Err(reason) => reason,
    };
    let sub = dir.join("No Man's Land");
    match check_candidate(&sub, Strictness::Found) {
        Ok(Some(version)) => Ok((sub, version)),
        _ => Err(first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write_fake_install;

    #[test]
    fn found_requires_known_version() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("de"), 11, true);
        write_fake_install(&t.path().join("en"), 11, false);
        write_fake_install(&t.path().join("demo"), 3, false);
        assert_eq!(
            check_candidate(&t.path().join("de"), Strictness::Found),
            Ok(Some(GameVersion::German))
        );
        assert_eq!(
            check_candidate(&t.path().join("en"), Strictness::Found),
            Ok(Some(GameVersion::English))
        );
        assert!(matches!(
            check_candidate(&t.path().join("demo"), Strictness::Found),
            Err(Rejection::UnknownVersion(m))
                if m == "incomplete or unknown edition (3 of 11 game files NML*.RDA)"
        ));
        write_fake_install(&t.path().join("more"), 13, false);
        assert!(matches!(
            check_candidate(&t.path().join("more"), Strictness::Found),
            Err(Rejection::UnknownVersion(m))
                if m == "unknown edition (13 game files NML*.RDA instead of 11)"
        ));
    }

    #[test]
    fn rejection_texts_are_plain() {
        assert_eq!(Rejection::Missing.to_string(), "not present");
        assert_eq!(
            Rejection::NoArchives.to_string(),
            "no No Man's Land game data (NML0.RDA missing)"
        );
        assert_eq!(
            Rejection::Unreadable("broken".into()).to_string(),
            "game data not readable: broken"
        );
    }

    #[test]
    fn explicit_accepts_unknown_version() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("demo"), 3, false);
        assert_eq!(
            check_candidate(&t.path().join("demo"), Strictness::Explicit),
            Ok(None)
        );
    }

    #[test]
    fn missing_and_empty_dirs_are_rejected() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(
            check_candidate(&t.path().join("absent"), Strictness::Found),
            Err(Rejection::Missing)
        );
        std::fs::create_dir(t.path().join("empty")).unwrap();
        assert_eq!(
            check_candidate(&t.path().join("empty"), Strictness::Found),
            Err(Rejection::NoArchives)
        );
        std::fs::create_dir(t.path().join("junk")).unwrap();
        std::fs::write(t.path().join("junk/NML0.RDA"), b"broken").unwrap();
        assert!(matches!(
            check_candidate(&t.path().join("junk"), Strictness::Found),
            Err(Rejection::Unreadable(_))
        ));
    }

    #[test]
    fn choice_accepts_parent_folder() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("No Man's Land"), 11, true);
        assert_eq!(
            check_choice(t.path()),
            Ok((t.path().join("No Man's Land"), GameVersion::German))
        );
        std::fs::create_dir(t.path().join("empty")).unwrap();
        assert_eq!(
            check_choice(&t.path().join("empty")),
            Err(Rejection::NoArchives)
        );
        assert_eq!(
            check_choice(&t.path().join("absent")),
            Err(Rejection::Missing)
        );
    }

    #[test]
    fn corpus_install_is_recognised() {
        let Some(dir) = crate::testutil::corpus_game_dir() else {
            return;
        };
        let vfs = crate::Vfs::open_game_dir(&dir).unwrap();
        assert_eq!(
            check_candidate(&dir, Strictness::Found).unwrap(),
            Some(crate::testutil::game_version(&vfs).unwrap())
        );
    }
}
