//! What the user sees when the game stops on an error or a panic: the log line, and on a plain start a message box
//! that names the log file. The release build has no console, so without the box nothing would be visible.

use std::any::Any;
use std::backtrace::Backtrace;
use std::fmt;
use std::path::Path;

use crate::game_dir::FolderPrompt;

/// Marks an error the user has already seen in a message box (the game folder search shows its errors itself).
#[derive(Debug)]
pub struct Reported;

impl fmt::Display for Reported {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("already shown to the user")
    }
}

/// Logs every panic with a backtrace, then calls the previous hook (the message on stderr).
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("Panic: {info}\n{}", Backtrace::force_capture());
        previous(info);
    }));
}

/// Text of the message box.
pub fn failure_message(error: &str, log_file: Option<&Path>) -> String {
    match log_file {
        Some(path) => format!(
            "OpenNML stopped because of an error.\n\n{error}\n\nLog file: {}",
            path.display()
        ),
        None => {
            format!("OpenNML stopped because of an error.\n\n{error}\n\nNo log file was written.")
        }
    }
}

/// Shows the message box, but only with `window` (a plain start) and for an error not yet shown.
pub fn report_failure(
    window: bool,
    error: &anyhow::Error,
    log_file: Option<&Path>,
    prompt: &mut dyn FolderPrompt,
) {
    if window && error.downcast_ref::<Reported>().is_none() {
        prompt.show_error(&failure_message(&format!("{error:#}"), log_file));
    }
}

/// Message of a panic payload.
pub fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::path::PathBuf;

    #[derive(Default)]
    struct Counting {
        shown: Vec<String>,
    }

    impl FolderPrompt for Counting {
        fn pick_folder(&mut self) -> Option<PathBuf> {
            None
        }
        fn ask_retry(&mut self, _message: &str) -> bool {
            false
        }
        fn show_error(&mut self, message: &str) {
            self.shown.push(message.to_owned());
        }
    }

    #[test]
    fn failure_message_names_log_file() {
        assert_eq!(
            failure_message("boom", Some(Path::new(r"C:\Users\Jörg Ä\x.log"))),
            "OpenNML stopped because of an error.\n\nboom\n\nLog file: C:\\Users\\Jörg Ä\\x.log"
        );
        assert_eq!(
            failure_message("boom", None),
            "OpenNML stopped because of an error.\n\nboom\n\nNo log file was written."
        );
    }

    #[test]
    fn reported_errors_are_not_shown_twice() {
        let mut prompt = Counting::default();
        report_failure(true, &anyhow!("x").context(Reported), None, &mut prompt);
        assert_eq!(prompt.shown.len(), 0);
        report_failure(true, &anyhow!("x"), None, &mut prompt);
        assert_eq!(prompt.shown.len(), 1);
        assert!(prompt.shown[0].contains("\n\nx\n\n"), "{}", prompt.shown[0]);
        report_failure(false, &anyhow!("x"), None, &mut prompt);
        assert_eq!(prompt.shown.len(), 1);
    }

    #[test]
    fn panic_text_reads_str_and_string() {
        assert_eq!(panic_text(&"plain"), "plain");
        assert_eq!(panic_text(&String::from("formatted")), "formatted");
        assert_eq!(panic_text(&42_u32), "unknown panic");
    }
}
