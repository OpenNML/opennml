//! The log of the program: every line goes to stderr and, once the user folder is known, to
//! `logs\OpenNML.log` in it. The release build has no console, so the file is the only log a user can send in.
//!
//! The logger starts before the user folder is known (it depends on the game folder search and the settings file),
//! so [`LogWriter`] keeps the lines in memory until [`LogHandle::open_in`] opens the file. A failure to open or
//! rotate the file never stops the program: the target is switched off and stderr keeps working.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use opennml_rda::user_dir_from;

/// Folder of the log files inside the user folder.
pub const LOG_DIR: &str = "logs";
/// Log of the current run.
pub const LOG_FILE: &str = "OpenNML.log";
/// Log of the previous run.
pub const PREVIOUS_LOG_FILE: &str = "OpenNML.1.log";

/// Where the lines go besides stderr.
enum Target {
    /// The user folder is not known yet: lines are kept.
    Buffer(Vec<u8>),
    /// The log file is open.
    File(File, PathBuf),
    /// Opening or writing failed: stderr only.
    Off,
}

type Shared = Arc<Mutex<Target>>;

fn lock(shared: &Shared) -> MutexGuard<'_, Target> {
    // A panic while logging must not silence the log of the panic.
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The writer behind `env_logger`: each line goes to stderr and to the shared target.
pub struct LogWriter {
    stderr: Box<dyn Write + Send>,
    shared: Shared,
}

/// Second owner of the shared target of a [`LogWriter`].
#[derive(Clone)]
pub struct LogHandle {
    shared: Shared,
}

impl LogWriter {
    /// A writer with the given stderr replacement and its handle.
    pub fn new(stderr: Box<dyn Write + Send>) -> (LogWriter, LogHandle) {
        let shared: Shared = Arc::new(Mutex::new(Target::Buffer(Vec::new())));
        (
            LogWriter {
                stderr,
                shared: Arc::clone(&shared),
            },
            LogHandle { shared },
        )
    }
}

impl Write for LogWriter {
    /// Always succeeds: a failing stderr (no console) or log file must not turn into a logger error.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = self.stderr.write_all(buf);
        let failed = {
            let mut target = lock(&self.shared);
            let failed = match &mut *target {
                Target::Buffer(lines) => {
                    lines.extend_from_slice(buf);
                    None
                }
                Target::File(file, path) => file
                    .write_all(buf)
                    .and_then(|()| file.flush())
                    .err()
                    .map(|error| (error, path.clone())),
                Target::Off => None,
            };
            if failed.is_some() {
                *target = Target::Off;
            }
            failed
        };
        // The lock is released before the one-time warning goes to stderr.
        if let Some((error, path)) = failed {
            let _ = writeln!(
                self.stderr,
                "[WARN  opennml_game::log_file] writing {} failed, the log file is off: {error}",
                path.display()
            );
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.stderr.flush();
        Ok(())
    }
}

impl LogHandle {
    /// Rotates the logs in `<user_dir>\logs`, writes the kept lines to the new file and switches to it. Returns the
    /// path of the file. On failure: a warning, the target is off, `None`.
    pub fn open_in(&self, user_dir: &Path) -> Option<PathBuf> {
        let result = {
            let mut target = lock(&self.shared);
            let opened = rotate(&user_dir.join(LOG_DIR)).and_then(|(mut file, path)| {
                if let Target::Buffer(lines) = &*target {
                    file.write_all(lines)?;
                    file.flush()?;
                }
                Ok((file, path))
            });
            match opened {
                Ok((file, path)) => {
                    *target = Target::File(file, path.clone());
                    Ok(path)
                }
                Err(error) => {
                    *target = Target::Off;
                    Err(error)
                }
            }
        };
        // The lock is released: the warning goes through the writer again.
        match result {
            Ok(path) => Some(path),
            Err(error) => {
                log::warn!(
                    "Cannot open the log file in {}: {error}",
                    user_dir.join(LOG_DIR).display()
                );
                None
            }
        }
    }

    /// Path of the open log file.
    pub fn path(&self) -> Option<PathBuf> {
        match &*lock(&self.shared) {
            Target::File(_, path) => Some(path.clone()),
            _ => None,
        }
    }
}

/// Installs `env_logger` (default level `info`, no colours) writing through a [`LogWriter`] to the real stderr.
pub fn init() -> LogHandle {
    let (writer, handle) = LogWriter::new(Box::new(io::stderr()));
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .write_style(env_logger::WriteStyle::Never)
        .target(env_logger::Target::Pipe(Box::new(writer)))
        .init();
    handle
}

/// Moves the log of the previous run to `OpenNML.1.log` (replacing an older one) and creates a new `OpenNML.log`.
///
/// Rust opens files on Windows with `FILE_SHARE_DELETE`, so this also works while a second program writes to the
/// current file; that program then continues in `OpenNML.1.log`.
fn rotate(dir: &Path) -> io::Result<(File, PathBuf)> {
    fs::create_dir_all(dir)?;
    let current = dir.join(LOG_FILE);
    let previous = dir.join(PREVIOUS_LOG_FILE);
    if let Err(error) = fs::remove_file(&previous)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    if let Err(error) = fs::rename(&current, &previous)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    Ok((File::create(&current)?, current))
}

/// Folder for the log when the start fails before the settings file is checked: the default user folder, only
/// without `--settings` and without a non-empty `NML_SETTINGS` (such a path may lie in the installation).
pub fn fallback_log_dir(
    settings: Option<&Path>,
    settings_env: Option<OsString>,
    local_app_data: Option<OsString>,
) -> Option<PathBuf> {
    if settings.is_some() || settings_env.is_some_and(|value| !value.is_empty()) {
        return None;
    }
    user_dir_from(None, None, local_app_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stderr replacement that can be read back.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Capture {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    fn writer() -> (LogWriter, LogHandle, Capture) {
        let capture = Capture::default();
        let (writer, handle) = LogWriter::new(Box::new(capture.clone()));
        (writer, handle, capture)
    }

    #[test]
    fn writer_buffers_until_opened_then_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        let (mut writer, handle, stderr) = writer();
        writer.write_all(b"one\n").unwrap();
        writer.write_all(b"two\n").unwrap();
        assert_eq!(handle.path(), None);
        let path = handle.open_in(dir.path()).unwrap();
        writer.write_all(b"three\n").unwrap();
        assert_eq!(path, dir.path().join(LOG_DIR).join(LOG_FILE));
        assert_eq!(handle.path(), Some(path.clone()));
        assert_eq!(fs::read_to_string(path).unwrap(), "one\ntwo\nthree\n");
        assert_eq!(stderr.text(), "one\ntwo\nthree\n");
    }

    #[test]
    fn rotate_keeps_one_previous_log() {
        let base = tempfile::tempdir().unwrap();
        let user = base.path().join("\u{e4} b");
        for run in 1..=3 {
            let (mut writer, handle, _) = writer();
            writer.write_all(format!("run {run}\n").as_bytes()).unwrap();
            assert!(handle.open_in(&user).is_some());
        }
        let logs = user.join(LOG_DIR);
        assert_eq!(fs::read_to_string(logs.join(LOG_FILE)).unwrap(), "run 3\n");
        assert_eq!(
            fs::read_to_string(logs.join(PREVIOUS_LOG_FILE)).unwrap(),
            "run 2\n"
        );
        for entry in fs::read_dir(&logs).unwrap() {
            let text = fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains("run 1"), "run 1 must be gone: {text}");
        }
    }

    #[test]
    fn second_open_in_same_dir_keeps_both_writers() {
        let dir = tempfile::tempdir().unwrap();
        let (mut first, first_handle, _) = writer();
        let (mut second, second_handle, _) = writer();
        assert!(first_handle.open_in(dir.path()).is_some());
        assert!(second_handle.open_in(dir.path()).is_some());
        first.write_all(b"first\n").unwrap();
        second.write_all(b"second\n").unwrap();
        let logs = dir.path().join(LOG_DIR);
        assert_eq!(
            fs::read_to_string(logs.join(PREVIOUS_LOG_FILE)).unwrap(),
            "first\n"
        );
        assert_eq!(fs::read_to_string(logs.join(LOG_FILE)).unwrap(), "second\n");
    }

    #[test]
    fn open_failure_does_not_abort() {
        let base = tempfile::tempdir().unwrap();
        let not_a_dir = base.path().join("file");
        fs::write(&not_a_dir, b"x").unwrap();
        let (mut writer, handle, stderr) = writer();
        writer.write_all(b"before\n").unwrap();
        assert_eq!(handle.open_in(&not_a_dir), None);
        assert_eq!(handle.path(), None);
        writer.write_all(b"after\n").unwrap();
        assert!(stderr.text().ends_with("before\nafter\n"));
    }

    #[test]
    fn fallback_log_dir_only_without_explicit_settings() {
        let local = || Some(OsString::from("C:/L"));
        assert_eq!(
            fallback_log_dir(None, None, local()),
            Some(PathBuf::from("C:/L").join("OpenNML"))
        );
        assert_eq!(
            fallback_log_dir(Some(Path::new("x/Engine.ini")), None, local()),
            None
        );
        assert_eq!(
            fallback_log_dir(None, Some(OsString::from("E:/x.ini")), local()),
            None
        );
        assert_eq!(
            fallback_log_dir(None, Some(OsString::new()), local()),
            Some(PathBuf::from("C:/L").join("OpenNML"))
        );
    }
}
