//! The remembered game folder: a text file `game-dir.txt` next to the settings file.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// File name of the remembered choice.
pub const REMEMBERED_FILE: &str = "game-dir.txt";

/// Location of `game-dir.txt` from the inputs: the folder of `settings`, otherwise the folder of the
/// non-empty `settings_env`, otherwise `<local_app_data>\OpenNML`. An empty parent folder counts as `.`.
pub fn remembered_file_from(
    settings: Option<&Path>,
    settings_env: Option<OsString>,
    local_app_data: Option<OsString>,
) -> Option<PathBuf> {
    let settings_file = settings.map(Path::to_path_buf).or_else(|| {
        settings_env
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    });
    if let Some(file) = settings_file {
        let parent = match file.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        return Some(parent.join(REMEMBERED_FILE));
    }
    local_app_data
        .filter(|value| !value.is_empty())
        .map(|base| PathBuf::from(base).join("OpenNML").join(REMEMBERED_FILE))
}

/// Location of `game-dir.txt` using `NML_SETTINGS` (only if `honour_settings_env` is set) and
/// `LOCALAPPDATA` from the environment.
pub fn remembered_file(settings: Option<&Path>, honour_settings_env: bool) -> Option<PathBuf> {
    let settings_env = if honour_settings_env {
        std::env::var_os("NML_SETTINGS")
    } else {
        None
    };
    remembered_file_from(settings, settings_env, std::env::var_os("LOCALAPPDATA"))
}

/// Reads the remembered folder. If the file is missing, the result is `None` without a log entry; an unreadable
/// file or non-UTF-8 content yields a warning and `None`. A BOM and whitespace at the edges are dropped, an empty
/// line yields `None`; only the first non-empty line counts.
pub fn read_remembered(file: &Path) -> Option<PathBuf> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!(
                "Remembered game folder: {} not readable: {error}",
                file.display()
            );
            return None;
        }
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            log::warn!(
                "Remembered game folder: {} is not valid UTF-8",
                file.display()
            );
            return None;
        }
    };
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

/// Writes the absolute path of `dir` as one line to `file` and creates the parent folder.
pub fn write_remembered(file: &Path, dir: &Path) -> std::io::Result<()> {
    if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let absolute = std::path::absolute(dir)?;
    let text = absolute.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "game folder is not valid UTF-8",
        )
    })?;
    std::fs::write(file, format!("{text}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_sits_next_to_settings() {
        assert_eq!(
            remembered_file_from(
                Some(Path::new("D:/profile/x/Engine.ini")),
                Some("E:/y/Engine.ini".into()),
                Some("C:/L".into())
            ),
            Some(PathBuf::from("D:/profile/x/game-dir.txt"))
        );
        assert_eq!(
            remembered_file_from(None, Some("E:/y/Engine.ini".into()), Some("C:/L".into())),
            Some(PathBuf::from("E:/y/game-dir.txt"))
        );
        assert_eq!(
            remembered_file_from(None, Some("".into()), Some("C:/L".into())),
            Some(PathBuf::from("C:/L/OpenNML/game-dir.txt"))
        );
        assert_eq!(
            remembered_file_from(Some(Path::new("Engine.ini")), None, None),
            Some(PathBuf::from("./game-dir.txt"))
        );
        assert_eq!(remembered_file_from(None, None, None), None);
    }

    #[test]
    fn remembered_roundtrip_keeps_unicode_and_apostrophe() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sub").join(REMEMBERED_FILE);
        let dir = temp
            .path()
            .join("Games")
            .join("Ärger")
            .join("No Man's Land");
        write_remembered(&file, &dir).unwrap();
        assert_eq!(
            read_remembered(&file),
            Some(std::path::absolute(&dir).unwrap())
        );
    }

    #[test]
    fn remembered_read_strips_bom_and_whitespace() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(REMEMBERED_FILE);
        std::fs::write(&file, "\u{feff}  D:/X/No Man's Land\r\n\r\n").unwrap();
        assert_eq!(
            read_remembered(&file),
            Some(PathBuf::from("D:/X/No Man's Land"))
        );
    }

    #[test]
    fn remembered_read_takes_first_nonempty_line_only() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(REMEMBERED_FILE);
        std::fs::write(&file, "D:/X/No Man's Land\r\njunk\r\n").unwrap();
        assert_eq!(
            read_remembered(&file),
            Some(PathBuf::from("D:/X/No Man's Land"))
        );
    }

    #[test]
    fn remembered_missing_empty_or_binary_is_none() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(REMEMBERED_FILE);
        assert_eq!(read_remembered(&file), None);
        std::fs::write(&file, "\r\n").unwrap();
        assert_eq!(read_remembered(&file), None);
        std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read_remembered(&file), None);
    }
}
