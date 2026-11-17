//! Finding the game folder: first the search, on failure the folder dialog of the system (choosing again after "Yes"
//! to the retry prompt).
//!
//! The folder dialog opens only if the search finds nothing and no location was given explicitly (`--game-dir`,
//! non-empty `NML_GAME_DIR`); a wrong explicit location shows only the message window; the choice is remembered in
//! `game-dir.txt`, never in the installation.

use std::path::{Path, PathBuf};

use anyhow::Result;
use nml_client::settings::settings_path_for;
use nml_rda::{RdaError, SearchInput, Source, check_choice, search, write_remembered};

/// The windows the start needs when the game folder is missing (replaced in tests).
pub trait FolderPrompt {
    /// Asks for the folder; `None` on cancel.
    fn pick_folder(&mut self) -> Option<PathBuf>;
    /// Asks yes/no whether another folder should be chosen.
    fn ask_retry(&mut self, message: &str) -> bool;
    /// Shows an error.
    fn show_error(&mut self, message: &str);
}

/// The windows of the system (`rfd` on Windows, otherwise without effect).
pub struct SystemPrompt;

#[cfg(windows)]
const TITLE: &str = "OpenNML";

#[cfg(windows)]
impl FolderPrompt for SystemPrompt {
    fn pick_folder(&mut self) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title("Choose the folder of No Man's Land");
        if let Some(pf) = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from)
            && pf.is_dir()
        {
            dialog = dialog.set_directory(pf);
        }
        dialog.pick_folder()
    }

    fn ask_retry(&mut self, message: &str) -> bool {
        rfd::MessageDialog::new()
            .set_title(TITLE)
            .set_description(message)
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    }

    fn show_error(&mut self, message: &str) {
        rfd::MessageDialog::new()
            .set_title(TITLE)
            .set_description(message)
            .set_level(rfd::MessageLevel::Error)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}

#[cfg(not(windows))]
impl FolderPrompt for SystemPrompt {
    fn pick_folder(&mut self) -> Option<PathBuf> {
        None
    }

    fn ask_retry(&mut self, _message: &str) -> bool {
        false
    }

    fn show_error(&mut self, _message: &str) {}
}

/// Finds the game folder: the search, otherwise (only with `prompt`) the dialog. A valid choice is remembered in
/// `remembered_file` after `settings_path_for` has passed against it.
pub fn resolve_game_dir(
    input: &SearchInput,
    remembered_file: Option<&Path>,
    settings: Option<&Path>,
    prompt: Option<&mut dyn FolderPrompt>,
) -> Result<PathBuf> {
    let error = match search(input) {
        Ok(found) => return Ok(found.path),
        Err(e) => e,
    };
    let explicit = matches!(&error, RdaError::GameDirNotFound { explicit: true, .. });
    let Some(prompt) = prompt else {
        return Err(error.into());
    };
    if explicit {
        // A wrong explicit location is otherwise invisible on a double click; no folder dialog.
        prompt.show_error(&error.to_string());
        return Err(error.into());
    }
    loop {
        let Some(chosen) = prompt.pick_folder() else {
            prompt.show_error(&error.to_string());
            return Err(error.into());
        };
        match check_choice(&chosen) {
            Err(reason) => {
                let message = format!("{}: {reason}\n\nChoose another folder?", chosen.display());
                if !prompt.ask_retry(&message) {
                    prompt.show_error(&error.to_string());
                    return Err(error.into());
                }
            }
            Ok((dir, version)) => {
                if let Err(e) = settings_path_for(settings, &dir) {
                    prompt.show_error(&format!("{e:#}"));
                    return Err(e);
                }
                if let Some(file) = remembered_file
                    && let Err(e) = write_remembered(file, &dir)
                {
                    log::warn!("Game folder not remembered ({}): {e}", file.display());
                }
                log::info!(
                    "Game directory: {} ({}, found via {})",
                    dir.display(),
                    version.label(),
                    Source::Dialog
                );
                return Ok(dir);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_rda::read_remembered;
    use nml_rda::testutil::write_fake_install;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakePrompt {
        picks: VecDeque<Option<PathBuf>>,
        retries: VecDeque<bool>,
        errors: Vec<String>,
    }

    impl FolderPrompt for FakePrompt {
        fn pick_folder(&mut self) -> Option<PathBuf> {
            self.picks
                .pop_front()
                .expect("pick_folder called without a prepared answer")
        }
        fn ask_retry(&mut self, _message: &str) -> bool {
            self.retries
                .pop_front()
                .expect("ask_retry called without a prepared answer")
        }
        fn show_error(&mut self, message: &str) {
            self.errors.push(message.to_owned());
        }
    }

    fn valid_install(root: &Path) -> PathBuf {
        let dir = root.join("choice").join("No Man's Land");
        write_fake_install(&dir, 11, true);
        dir
    }

    #[test]
    fn without_prompt_no_dialog() {
        let err = resolve_game_dir(&SearchInput::default(), None, None, None).unwrap_err();
        assert!(err.to_string().contains("NML_GAME_DIR"), "{err}");
    }

    #[test]
    fn explicit_error_shows_error_but_never_picks() {
        let t = tempfile::tempdir().unwrap();
        let input = SearchInput {
            explicit: Some(t.path().join("absent")),
            ..SearchInput::default()
        };
        let mut fake = FakePrompt::default();
        assert!(resolve_game_dir(&input, None, None, Some(&mut fake)).is_err());
        assert_eq!(fake.errors.len(), 1);
        assert!(fake.errors[0].contains("absent"), "{}", fake.errors[0]);
        assert!(fake.picks.is_empty() && fake.retries.is_empty());
        assert!(resolve_game_dir(&input, None, None, None).is_err());
    }

    #[test]
    fn cancel_shows_error_and_fails() {
        let mut fake = FakePrompt::default();
        fake.picks.push_back(None);
        let err =
            resolve_game_dir(&SearchInput::default(), None, None, Some(&mut fake)).unwrap_err();
        assert_eq!(fake.errors.len(), 1);
        assert!(fake.errors[0].contains("--game-dir"), "{}", fake.errors[0]);
        assert!(err.to_string().contains("--game-dir"));
    }

    #[test]
    fn valid_choice_is_remembered() {
        let t = tempfile::tempdir().unwrap();
        let install = valid_install(t.path());
        let file = t.path().join("cfg").join("game-dir.txt");
        let mut fake = FakePrompt::default();
        fake.picks.push_back(Some(t.path().join("choice")));
        let dir = resolve_game_dir(
            &SearchInput::default(),
            Some(&file),
            Some(&t.path().join("cfg").join("Engine.ini")),
            Some(&mut fake),
        )
        .unwrap();
        assert_eq!(dir, install);
        assert_eq!(
            read_remembered(&file),
            Some(std::path::absolute(&install).unwrap())
        );
        assert!(fake.errors.is_empty());
    }

    #[test]
    fn invalid_choice_then_retry_then_valid() {
        let t = tempfile::tempdir().unwrap();
        let install = valid_install(t.path());
        let empty = t.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let mut fake = FakePrompt::default();
        fake.picks.push_back(Some(empty));
        fake.picks.push_back(Some(install.clone()));
        fake.retries.push_back(true);
        let dir = resolve_game_dir(
            &SearchInput::default(),
            None,
            Some(&t.path().join("Engine.ini")),
            Some(&mut fake),
        )
        .unwrap();
        assert_eq!(dir, install);
    }

    #[test]
    fn invalid_choice_without_retry_fails() {
        let t = tempfile::tempdir().unwrap();
        let empty = t.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let mut fake = FakePrompt::default();
        fake.picks.push_back(Some(empty));
        fake.retries.push_back(false);
        assert!(resolve_game_dir(&SearchInput::default(), None, None, Some(&mut fake)).is_err());
        assert_eq!(fake.errors.len(), 1);
    }

    #[test]
    fn settings_inside_choice_fails_and_writes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let install = valid_install(t.path());
        let file = t.path().join("cfg").join("game-dir.txt");
        let mut fake = FakePrompt::default();
        fake.picks.push_back(Some(install.clone()));
        let result = resolve_game_dir(
            &SearchInput::default(),
            Some(&file),
            Some(&install.join("Engine.ini")),
            Some(&mut fake),
        );
        assert!(result.is_err());
        assert!(!file.exists());
        assert!(!install.join("Engine.ini").exists());
        assert_eq!(fake.errors.len(), 1);
    }

    #[test]
    fn stale_remembered_dir_leads_to_dialog_and_is_replaced() {
        let t = tempfile::tempdir().unwrap();
        let install = valid_install(t.path());
        let gone = t.path().join("gone");
        let file = t.path().join("cfg").join("game-dir.txt");
        write_remembered(&file, &gone).unwrap();
        let input = SearchInput {
            remembered: Some(gone),
            ..SearchInput::default()
        };
        let mut fake = FakePrompt::default();
        fake.picks.push_back(Some(install.clone()));
        let dir = resolve_game_dir(
            &input,
            Some(&file),
            Some(&t.path().join("cfg").join("Engine.ini")),
            Some(&mut fake),
        )
        .unwrap();
        assert_eq!(dir, install);
        assert_eq!(
            read_remembered(&file),
            Some(std::path::absolute(&install).unwrap())
        );
    }
}
