//! Finds the original installation.
//!
//! Order: `--game-dir` (used exclusively), `NML_GAME_DIR` (used exclusively), remembered folder, next to
//! the exe and its parent folders (in each folder also `No Man's Land` and `original/No Man's Land`
//! inside it), working directory, Steam, uninstall entries, usual program folders; the first valid
//! location wins. Description: `docs/programs.md`, section "Game directory".

use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::{GameVersion, RdaError};

mod check;
mod remembered;
mod steam;
mod system;
pub use check::{Rejection, Strictness, check_candidate, check_choice};
pub use remembered::{
    REMEMBERED_FILE, read_remembered, remembered_file, remembered_file_from, write_remembered,
};

/// Where a candidate comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    GameDirArg,
    EnvVar,
    Remembered,
    ExeDir,
    WorkingDir,
    Steam,
    Uninstall,
    ProgramFiles,
    Dialog,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Source::GameDirArg => "--game-dir",
            Source::EnvVar => "NML_GAME_DIR",
            Source::Remembered => "remembered folder",
            Source::ExeDir => "next to the program",
            Source::WorkingDir => "working directory",
            Source::Steam => "Steam",
            Source::Uninstall => "uninstall entry",
            Source::ProgramFiles => "program folder",
            Source::Dialog => "dialog selection",
        })
    }
}

/// A location to check, together with its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub source: Source,
}

/// A location that was checked and rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tried {
    pub candidate: Candidate,
    pub reason: Rejection,
}

/// The installation that was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub source: Source,
    /// `None`: edition unknown (only for an explicitly given location).
    pub version: Option<GameVersion>,
}

/// Everything the search needs from the machine; tests fill it in by hand.
#[derive(Debug, Clone, Default)]
pub struct SearchInput {
    pub explicit: Option<PathBuf>,
    pub env: Option<OsString>,
    /// The remembered game folder (not the path of the file that stores it).
    pub remembered: Option<PathBuf>,
    pub exe: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    /// Locations from Steam, uninstall entries, and program folders, in search order.
    pub system: Vec<Candidate>,
}

impl SearchInput {
    /// Reads the environment variable, exe path, and working directory of the running process.
    pub fn from_environment(explicit: Option<&Path>, remembered: Option<PathBuf>) -> Self {
        let env = std::env::var_os("NML_GAME_DIR");
        let given = explicit.is_some() || env.as_ref().is_some_and(|v| !v.is_empty());
        SearchInput {
            explicit: explicit.map(Path::to_path_buf),
            env,
            remembered,
            exe: std::env::current_exe().ok(),
            cwd: std::env::current_dir().ok(),
            system: if given {
                Vec::new()
            } else {
                system::system_candidates()
            },
        }
    }
}

const GAME_FOLDER: &str = "No Man's Land";
const INSTALL_SUBDIR: &str = "original/No Man's Land";

/// Key for comparing paths: absolute, `..` resolved, case-insensitive on Windows.
fn comparison_key(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    let text = out.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// The candidate locations (in search order, without duplicates): remembered folder,
/// next to the exe, working directory, then `system`.
pub fn candidates(input: &SearchInput) -> Vec<Candidate> {
    let mut list: Vec<Candidate> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut add = |path: PathBuf, source: Source| {
        let key = comparison_key(&path);
        if !seen.contains(&key) {
            seen.push(key);
            list.push(Candidate { path, source });
        }
    };
    if let Some(dir) = &input.remembered {
        add(dir.clone(), Source::Remembered);
    }
    if let Some(parent) = input.exe.as_deref().and_then(Path::parent) {
        for dir in parent.ancestors() {
            if dir.as_os_str().is_empty() {
                continue;
            }
            add(dir.to_path_buf(), Source::ExeDir);
            add(dir.join(GAME_FOLDER), Source::ExeDir);
            add(dir.join(INSTALL_SUBDIR), Source::ExeDir);
        }
    }
    if let Some(cwd) = &input.cwd {
        add(cwd.join("..").join(INSTALL_SUBDIR), Source::WorkingDir);
        add(cwd.join(INSTALL_SUBDIR), Source::WorkingDir);
    }
    for candidate in &input.system {
        add(candidate.path.clone(), candidate.source);
    }
    list
}

/// Searches for the installation: an explicitly given location (`--game-dir`, non-empty
/// `NML_GAME_DIR`) is used exclusively and is never silently replaced; otherwise the first valid candidate
/// wins.
pub fn search(input: &SearchInput) -> Result<Found, RdaError> {
    let given = input
        .explicit
        .as_ref()
        .map(|p| Candidate {
            path: p.clone(),
            source: Source::GameDirArg,
        })
        .or_else(|| {
            input
                .env
                .as_ref()
                .filter(|v| !v.is_empty())
                .map(|v| Candidate {
                    path: PathBuf::from(v),
                    source: Source::EnvVar,
                })
        });
    if let Some(candidate) = given {
        return match check_candidate(&candidate.path, Strictness::Explicit) {
            Ok(version) => Ok(announce(candidate, version)),
            Err(reason) => Err(RdaError::GameDirNotFound {
                tried: vec![Tried { candidate, reason }],
                explicit: true,
            }),
        };
    }
    let mut tried = Vec::new();
    for candidate in candidates(input) {
        match check_candidate(&candidate.path, Strictness::Found) {
            Ok(version) => return Ok(announce(candidate, version)),
            Err(reason) => {
                if reason == Rejection::NoArchives && candidate.source == Source::ExeDir {
                    // The ancestors of the exe always exist; that is not worth an info message.
                    log::debug!(
                        "Game directory rejected: {} ({}): {reason}",
                        candidate.path.display(),
                        candidate.source
                    );
                } else if reason != Rejection::Missing {
                    log::info!(
                        "Game directory rejected: {} ({}): {reason}",
                        candidate.path.display(),
                        candidate.source
                    );
                }
                tried.push(Tried { candidate, reason });
            }
        }
    }
    Err(RdaError::GameDirNotFound {
        tried,
        explicit: false,
    })
}

fn announce(candidate: Candidate, version: Option<GameVersion>) -> Found {
    log::info!(
        "Game directory: {} ({}, found via {})",
        candidate.path.display(),
        version.map_or("edition unknown", GameVersion::label),
        candidate.source
    );
    Found {
        path: candidate.path,
        source: candidate.source,
        version,
    }
}

/// Searches for the installation of the running process (see [`search`]).
pub fn find_game_dir(
    explicit: Option<&Path>,
    remembered: Option<PathBuf>,
) -> Result<Found, RdaError> {
    search(&SearchInput::from_environment(explicit, remembered))
}

/// The text of the error message "game directory not found".
pub(crate) fn not_found_message(tried: &[Tried], explicit: bool) -> String {
    let line = |t: &Tried| {
        format!(
            "{} ({}): {}",
            t.candidate.path.display(),
            t.candidate.source,
            t.reason
        )
    };
    let help = if explicit {
        "To fix: correct the location, or omit --game-dir/NML_GAME_DIR so that the program searches on its own."
    } else {
        "To fix: specify --game-dir <folder> or set the environment variable NML_GAME_DIR."
    };
    if explicit {
        let first = tried.first().map(line).unwrap_or_default();
        format!("The specified game directory is invalid: {first}\n{help}")
    } else {
        let mut text = String::from("No valid installation of No Man's Land found. Checked:");
        for t in tried {
            text.push_str("\n  ");
            text.push_str(&line(t));
        }
        text.push('\n');
        text.push_str(help);
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write_fake_install;

    fn input() -> SearchInput {
        SearchInput::default()
    }

    /// Creates `<base>/original/No Man's Land`.
    fn install_under(base: &Path, german: bool) {
        write_fake_install(&base.join(INSTALL_SUBDIR), 11, german);
    }

    #[test]
    fn explicit_missing_is_error_without_fallback() {
        let t = tempfile::tempdir().unwrap();
        install_under(&t.path().join("cwd"), false);
        let mut i = input();
        i.explicit = Some(t.path().join("missing"));
        i.cwd = Some(t.path().join("cwd"));
        let err = search(&i).unwrap_err();
        match &err {
            RdaError::GameDirNotFound { tried, explicit } => {
                assert!(*explicit);
                assert_eq!(tried.len(), 1);
                assert_eq!(tried[0].candidate.source, Source::GameDirArg);
                assert_eq!(tried[0].reason, Rejection::Missing);
            }
            other => panic!("unexpected: {other:?}"),
        }
        let text = err.to_string();
        assert!(text.contains("missing"));
        assert!(text.contains("--game-dir"));
        assert!(text.contains("NML_GAME_DIR"));
    }

    #[test]
    fn explicit_unknown_version_is_used() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("demo"), 3, false);
        let mut i = input();
        i.explicit = Some(t.path().join("demo"));
        let found = search(&i).unwrap();
        assert_eq!(found.version, None);
        assert_eq!(found.source, Source::GameDirArg);
        assert_eq!(found.path, t.path().join("demo"));
    }

    #[test]
    fn invalid_env_is_error_without_fallback() {
        let t = tempfile::tempdir().unwrap();
        install_under(t.path(), false);
        let mut i = input();
        i.env = Some("Z:/does/not/exist".into());
        i.cwd = Some(t.path().to_path_buf());
        match search(&i).unwrap_err() {
            RdaError::GameDirNotFound { tried, explicit } => {
                assert!(explicit);
                assert_eq!(tried[0].candidate.source, Source::EnvVar);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn empty_env_counts_as_unset() {
        let t = tempfile::tempdir().unwrap();
        install_under(t.path(), false);
        let mut i = input();
        i.env = Some("".into());
        i.cwd = Some(t.path().to_path_buf());
        assert_eq!(search(&i).unwrap().source, Source::WorkingDir);
    }

    #[test]
    fn first_valid_in_order_wins() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("de"), 11, true);
        install_under(&t.path().join("a"), false);
        let mut i = input();
        i.remembered = Some(t.path().join("de"));
        i.exe = Some(t.path().join("a/nml-game.exe"));
        let found = search(&i).unwrap();
        assert_eq!(found.source, Source::Remembered);
        assert_eq!(found.version, Some(GameVersion::German));
        i.remembered = None;
        let found = search(&i).unwrap();
        assert_eq!(found.source, Source::ExeDir);
        assert_eq!(found.version, Some(GameVersion::English));
    }

    #[test]
    fn exe_relative_finds_original_beside_tree() {
        let t = tempfile::tempdir().unwrap();
        install_under(&t.path().join("a"), false);
        let mut i = input();
        i.exe = Some(t.path().join("a/target/release/nml-game.exe"));
        let found = search(&i).unwrap();
        assert_eq!(found.source, Source::ExeDir);
        assert_eq!(found.path, t.path().join("a").join(INSTALL_SUBDIR));
    }

    #[test]
    fn exe_relative_finds_original_beside_program_folder() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("Games/No Man's Land"), 11, false);
        let mut i = input();
        i.exe = Some(t.path().join("Games/OpenNML/nml-game.exe"));
        let found = search(&i).unwrap();
        assert_eq!(found.source, Source::ExeDir);
        assert_eq!(found.path, t.path().join("Games/No Man's Land"));
    }

    #[test]
    fn explicit_beats_env() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("demo"), 3, false);
        let mut i = input();
        i.explicit = Some(t.path().join("demo"));
        i.env = Some("Z:/missing".into());
        assert_eq!(search(&i).unwrap().source, Source::GameDirArg);
    }

    #[test]
    fn explicit_missing_beats_valid_env() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("demo"), 3, false);
        let mut i = input();
        i.explicit = Some(t.path().join("missing"));
        i.env = Some(t.path().join("demo").into_os_string());
        match search(&i).unwrap_err() {
            RdaError::GameDirNotFound { tried, explicit } => {
                assert!(explicit);
                assert_eq!(tried[0].candidate.source, Source::GameDirArg);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn help_text_depends_on_explicit() {
        let t = tempfile::tempdir().unwrap();
        let mut i = input();
        i.explicit = Some(t.path().join("missing"));
        let text = search(&i).unwrap_err().to_string();
        assert!(
            text.contains("omit --game-dir/NML_GAME_DIR so that the program searches on its own"),
            "{text}"
        );
        let text = search(&input()).unwrap_err().to_string();
        assert!(
            text.contains("or set the environment variable NML_GAME_DIR"),
            "{text}"
        );
    }

    #[test]
    fn exe_without_parent_yields_no_candidates() {
        let mut i = input();
        i.exe = Some("nml-game.exe".into());
        assert!(candidates(&i).is_empty());
        match search(&i).unwrap_err() {
            RdaError::GameDirNotFound { tried, explicit } => {
                assert!(!explicit);
                assert!(tried.is_empty());
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn duplicates_are_checked_once() {
        let t = tempfile::tempdir().unwrap();
        let mut i = input();
        i.cwd = Some(t.path().join("x"));
        i.exe = Some(t.path().join("x/nml-game.exe"));
        let wanted = comparison_key(&t.path().join(INSTALL_SUBDIR));
        let count = candidates(&i)
            .iter()
            .filter(|c| comparison_key(&c.path) == wanted)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn failure_lists_every_candidate_with_reason() {
        let t = tempfile::tempdir().unwrap();
        write_fake_install(&t.path().join("demo"), 3, false);
        std::fs::create_dir(t.path().join("empty")).unwrap();
        let mut i = input();
        i.remembered = Some(t.path().join("demo"));
        i.system = vec![Candidate {
            path: t.path().join("empty"),
            source: Source::ProgramFiles,
        }];
        let err = search(&i).unwrap_err();
        match &err {
            RdaError::GameDirNotFound { tried, explicit } => {
                assert!(!explicit);
                assert!(
                    tried
                        .iter()
                        .any(|x| x.candidate.source == Source::Remembered
                            && matches!(x.reason, Rejection::UnknownVersion(_)))
                );
                assert!(
                    tried
                        .iter()
                        .any(|x| x.candidate.source == Source::ProgramFiles
                            && x.reason == Rejection::NoArchives)
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
        let text = err.to_string();
        assert!(text.starts_with("No valid installation"));
        assert!(text.contains("demo"));
        assert!(text.contains("remembered folder"));
        assert!(text.contains("empty"));
        assert!(text.contains("program folder"));
    }
}
