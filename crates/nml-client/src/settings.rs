//! Settings file of the reimplementation in the format of `Engine.ini` (`docs/formats/engine-ini.md`; deviation from
//! the original): `NML_SETTINGS` or `--settings`, otherwise `%LOCALAPPDATA%\nml-rust\Engine.ini`, **never** in the
//! installation. The original reads its `Engine.ini` in the constructor (`EngineConfig_Construct` `0x00497010`:
//! defaults `FUN_00494FD0` including `defaultkeys.ini`, then `FUN_00496C00`) and writes it with `FUN_00496D40` at
//! `OPTIONS OK` and in the destructor `FUN_00496E50` (O-32).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nml_formats::engine_ini::{EngineConfig, parse_engine_ini_bytes_onto, write_engine_ini_bytes};
use nml_formats::profiles::{ProfilesFile, parse_profiles, write_profiles};
use nml_formats::text::decode_cp1252;
use nml_rda::Vfs;
use nml_ui::atlas_gen::{is_inside, user_path_from};
use nml_ui::screens::load_default_keys;

/// Environment variable for a custom location of the settings file.
pub const SETTINGS_ENV: &str = "NML_SETTINGS";
/// File name under `%LOCALAPPDATA%\nml-rust`.
pub const SETTINGS_FILE: &str = "Engine.ini";

/// `NML_SETTINGS`, otherwise `%LOCALAPPDATA%\nml-rust\Engine.ini`; `None` without both.
pub fn default_settings_path() -> Option<PathBuf> {
    user_path_from(
        std::env::var_os(SETTINGS_ENV),
        std::env::var_os("LOCALAPPDATA"),
        SETTINGS_FILE,
    )
}

/// Location of the settings file: `explicit` (`--settings`), otherwise [`default_settings_path`]. A location in the
/// installation `game_dir` (or the installation itself) is an error, as is the absence of any location.
pub fn settings_path_for(explicit: Option<&Path>, game_dir: &Path) -> Result<PathBuf> {
    resolve(explicit, default_settings_path(), game_dir)
}

/// Core of [`settings_path_for`] with a given default location.
fn resolve(explicit: Option<&Path>, default: Option<PathBuf>, game_dir: &Path) -> Result<PathBuf> {
    let Some(path) = explicit.map(Path::to_path_buf).or(default) else {
        bail!(
            "no location for the settings file: neither --settings nor {SETTINGS_ENV} nor LOCALAPPDATA is set"
        );
    };
    if is_inside(&path, game_dir) {
        bail!(
            "the settings file \"{}\" would lie in the installation \"{}\"; the reimplementation never writes there",
            path.display(),
            game_dir.display()
        );
    }
    Ok(path)
}

/// Like [`default_settings_path`] with given environment values (tests).
#[cfg(test)]
fn default_from(explicit: Option<OsString>, local_app_data: Option<OsString>) -> Option<PathBuf> {
    user_path_from(explicit, local_app_data, SETTINGS_FILE)
}

/// Defaults without a file: `EngineConfig::default` with the layout from `defaultkeys.ini` (`FUN_00494FD0`); the
/// viewer without `--settings` works this way too (in memory only).
pub fn default_config(vfs: &Vfs) -> EngineConfig {
    EngineConfig::default().with_default_keys(&load_default_keys(vfs))
}

/// Reads settings like `EngineConfig_Construct`: the base is the defaults (`EngineConfig::default`) with the key
/// layout from `defaultkeys.ini` (`FUN_00494FD0`), the file (cp1252) on top. If it is missing, the defaults apply (the
/// installation is not copied); if it is unreadable or contains errors, the defaults apply per key, and every warning
/// goes to the log. The file is overwritten only at the next saving.
pub fn load_settings(path: &Path, vfs: &Vfs) -> EngineConfig {
    let base = default_config(vfs);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::info!(
                "Settings file \"{}\" is missing: the defaults apply",
                path.display()
            );
            return base;
        }
        Err(e) => {
            log::warn!(
                "Settings file \"{}\" not readable ({e}): the defaults apply",
                path.display()
            );
            return base;
        }
    };
    let (cfg, warnings) = parse_engine_ini_bytes_onto(base, &bytes);
    for warning in warnings {
        log::warn!("Settings file \"{}\": {warning}", path.display());
    }
    cfg
}

/// Writes settings (`FUN_00496D40`, cp1252): create the directory, first write `<file>.tmp`, then rename it over the
/// file - an abort leaves the old file intact. If the renaming fails, the `.tmp` is deleted again. The location has
/// been checked with [`settings_path_for`].
pub fn save_settings(path: &Path, cfg: &EngineConfig) -> Result<()> {
    write_replacing(path, &write_engine_ini_bytes(cfg))
}

/// Create the folder, write `<file>.tmp` and rename it over the file; if the renaming fails, the `.tmp` is dropped
/// again.
fn write_replacing(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("folder \"{}\" cannot be created", dir.display()))?;
    }
    let mut tmp: OsString = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).with_context(|| format!("\"{}\" not writable", tmp.display()))?;
    std::fs::rename(&tmp, path).or_else(|e| {
        // Leave nothing behind: half the work does not belong next to the settings file.
        let _ = std::fs::remove_file(&tmp);
        Err(e).with_context(|| {
            format!(
                "\"{}\" cannot be renamed to \"{}\"",
                tmp.display(),
                path.display()
            )
        })
    })
}

/// File name of the profiles next to the settings file (deviation from the original).
pub const PROFILES_FILE: &str = "profiles.ini";
/// Folder of the savegames next to the settings file (deviation; in the original `savegame` in the game directory).
pub const SAVEGAME_DIR: &str = "savegame";

/// `profiles.ini` in the folder of the settings file `settings` (never in the installation; deviation from the
/// original).
pub fn profiles_path(settings: &Path) -> PathBuf {
    settings.with_file_name(PROFILES_FILE)
}

/// Folder `savegame` in the folder of the settings file `settings` (deviation from the original).
pub fn savegame_dir(settings: &Path) -> PathBuf {
    settings.with_file_name(SAVEGAME_DIR)
}

/// Suffix of the set-aside corrupted profile file (`profiles.ini.broken`).
pub const BROKEN_PROFILES_SUFFIX: &str = ".broken";

/// Reads `profiles.ini` (`FUN_0040E5C0`, cp1252). If it is missing, the defaults without profiles apply (the original
/// creates nothing); if it is unreadable or corrupted (`parse_profiles` rejects it, O-126), likewise, with a warning.
/// A corrupted file is renamed to `profiles.ini.broken` (an older backup is replaced) so that the next writing of the
/// profiles does not overwrite it (docs/formats/profiles-ini.md, section "Rejection").
pub fn load_profiles(path: &Path) -> ProfilesFile {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::info!(
                "Profile file \"{}\" is missing: no profiles",
                path.display()
            );
            return ProfilesFile::default();
        }
        Err(e) => {
            log::warn!(
                "Profile file \"{}\" not readable ({e}): no profiles",
                path.display()
            );
            return ProfilesFile::default();
        }
    };
    match parse_profiles(&decode_cp1252(&bytes)) {
        Ok(file) => file,
        Err(e) => {
            set_aside_broken(path, &e.to_string());
            ProfilesFile::default()
        }
    }
}

/// Rename a corrupted `profiles.ini` to `<file>.broken` (replacing an existing backup) and report it.
fn set_aside_broken(path: &Path, error: &str) {
    let mut backup = path.as_os_str().to_owned();
    backup.push(BROKEN_PROFILES_SUFFIX);
    let backup = PathBuf::from(backup);
    match std::fs::rename(path, &backup) {
        Ok(()) => log::warn!(
            "\"{}\": {error}; no profiles apply, the file is now kept next to it as \"{}\"",
            path.display(),
            backup.display()
        ),
        Err(e) => log::warn!(
            "\"{}\": {error}; no profiles apply. Renaming to \"{}\" failed ({e}): the next saving of the profiles overwrites the file",
            path.display(),
            backup.display()
        ),
    }
}

/// Writes `profiles.ini` (`FUN_0040E6C0`, cp1252) like [`save_settings`]: create the folder, via `<file>.tmp`.
pub fn save_profiles(path: &Path, file: &ProfilesFile) -> Result<()> {
    // The texts come from cp1252 (names are numbers, `m_VarMap` keys from the file that was read).
    let text = write_profiles(file);
    let (bytes, _, _) = encoding_rs::WINDOWS_1252.encode(&text);
    write_replacing(path, &bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nml_formats::keys::DEFAULT_KEYS_PATH;
    use nml_rda::testutil::{TestFile, build_rda};

    use crate::testutil::{capture_warnings, take_warnings};

    /// VFS with `defaultkeys.ini` (two entries) in a temporary directory.
    pub(crate) fn keys_vfs() -> (tempfile::TempDir, Vfs) {
        let keys = b"# Defaults\r\n0=\r\n7=CONTROL H\r\n8=F1\r\n".to_vec();
        let files = [TestFile {
            path: DEFAULT_KEYS_PATH,
            data: &keys,
            compress: true,
        }];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    #[test]
    fn settings_never_point_into_the_game_dir() {
        let game = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let g = game.path();
        // Explicitly or as default: every location in the installation is an error.
        for inside in [
            g.join("Engine.ini"),
            g.join("sub").join("Engine.ini"),
            g.join("..").join(g.file_name().unwrap()).join("x.ini"),
            g.to_path_buf(),
        ] {
            let err = resolve(Some(&inside), None, g).unwrap_err().to_string();
            assert!(err.contains("installation"), "{err}");
            assert!(resolve(None, Some(inside.clone()), g).is_err());
        }
        // Case does not count on Windows.
        if cfg!(windows) {
            let upper = PathBuf::from(g.to_string_lossy().to_uppercase()).join("Engine.ini");
            assert!(resolve(Some(&upper), None, g).is_err());
        }
        // A sibling folder with the same beginning does not lie inside it.
        let sibling = PathBuf::from(format!("{}2", g.display())).join("Engine.ini");
        assert_eq!(resolve(Some(&sibling), None, g).unwrap(), sibling);
        let outside = home.path().join("Engine.ini");
        assert_eq!(
            resolve(Some(&outside), Some(g.join("x")), g).unwrap(),
            outside
        );
        assert_eq!(resolve(None, Some(outside.clone()), g).unwrap(), outside);
        let err = resolve(None, None, g).unwrap_err().to_string();
        assert!(err.contains("NML_SETTINGS"), "{err}");
        // Default location: NML_SETTINGS, otherwise LOCALAPPDATA\nml-rust\Engine.ini; empty values do not count.
        let lad = || Some(OsString::from(r"C:\Home\x\AppData\Local"));
        assert_eq!(
            default_from(None, lad()),
            Some(
                PathBuf::from(r"C:\Home\x\AppData\Local")
                    .join("nml-rust")
                    .join("Engine.ini")
            )
        );
        assert_eq!(
            default_from(Some(OsString::from(r"E:\own.ini")), lad()),
            Some(PathBuf::from(r"E:\own.ini"))
        );
        assert_eq!(
            default_from(Some(OsString::new()), lad()),
            default_from(None, lad())
        );
        assert_eq!(default_from(None, Some(OsString::new())), None);
    }

    // QUIRK D-245: deviation
    #[test]
    fn missing_settings_use_defaults_and_default_keys() {
        let (_dir, vfs) = keys_vfs();
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("missing").join("Engine.ini");
        let cfg = load_settings(&path, &vfs);
        let want = EngineConfig::default().with_default_keys(&[
            (0, String::new()),
            (7, "CONTROL H".into()),
            (8, "F1".into()),
        ]);
        assert_eq!(cfg, want);
        assert!(!path.exists(), "reading creates nothing");
        // Broken file: defaults including keys, the file stays unchanged.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"<InitFile><ScreenXSize>").unwrap();
        assert_eq!(load_settings(&path, &vfs), want);
        assert_eq!(std::fs::read(&path).unwrap(), b"<InitFile><ScreenXSize>");
        // A directory instead of a file: defaults, no panic.
        assert_eq!(load_settings(home.path(), &vfs), want);
    }

    #[test]
    fn settings_round_trip_through_save() {
        let (_dir, vfs) = keys_vfs();
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("nml-rust").join("Engine.ini");
        let mut cfg = load_settings(&path, &vfs);
        cfg.screen_x_size = 1280;
        cfg.screen_y_size = 1024;
        cfg.gamma = 0.1;
        cfg.net_host_adr = "Gäste-Rechner".into();
        cfg.key_map[1].1 = "SHIFT N".into();
        cfg.sound.volumes = [1, 2, 3];
        save_settings(&path, &cfg).unwrap();
        assert!(!home.path().join("nml-rust").join("Engine.ini.tmp").exists());
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(2).any(|w| w == b"G\xE4"), "cp1252");
        assert_eq!(load_settings(&path, &vfs), cfg);
        // Overwriting an existing file.
        cfg.screen_x_size = 800;
        save_settings(&path, &cfg).unwrap();
        assert_eq!(load_settings(&path, &vfs).screen_x_size, 800);
    }

    #[test]
    fn failed_save_leaves_no_temporary_file() {
        let home = tempfile::tempdir().unwrap();
        // A folder at the location of the file: writing the `.tmp` succeeds, the renaming does not.
        let blocked = home.path().join("folder");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("old.txt"), b"old").unwrap();
        let err = save_settings(&blocked, &EngineConfig::default()).unwrap_err();
        assert!(format!("{err:#}").contains("cannot be renamed"), "{err:#}");
        let left: Vec<_> = std::fs::read_dir(home.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["folder"], "no .tmp left");
        assert_eq!(std::fs::read(blocked.join("old.txt")).unwrap(), b"old");
    }

    #[test]
    fn profiles_and_saves_lie_next_to_the_settings() {
        let settings = Path::new("C:/home/nml-rust/Engine.ini");
        assert_eq!(
            profiles_path(settings),
            Path::new("C:/home/nml-rust/profiles.ini")
        );
        assert_eq!(
            savegame_dir(settings),
            Path::new("C:/home/nml-rust/savegame")
        );
        assert_eq!(profiles_path(Path::new("x.ini")), Path::new("profiles.ini"));
    }

    // QUIRK O-126: deviation
    #[test]
    fn profiles_round_trip_and_bad_files_start_empty() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("nml-rust").join(PROFILES_FILE);
        // Missing: defaults.
        assert_eq!(load_profiles(&path), ProfilesFile::default());
        let mut file = ProfilesFile::default();
        let mut name = [0u16; 40];
        for (slot, u) in name.iter_mut().zip("Jürgen".encode_utf16()) {
            *slot = u;
        }
        file.profiles.push(nml_formats::profiles::ProfileRecord {
            name,
            guid: 3,
            levels: Vec::new(),
            vars: vec![("Gäste".into(), 2)],
        });
        file.next_guid = 4;
        file.current = 0;
        save_profiles(&path, &file).unwrap();
        assert!(!path.with_file_name("profiles.ini.tmp").exists());
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.windows(2).any(|w| w == b"G\xE4"), "cp1252");
        assert_eq!(load_profiles(&path), file);
        // Corrupted (GUID missing, O-126): without profiles; afterwards the file is kept next to it as
        // `profiles.ini.broken`, so that the next writing of the profiles does not overwrite it.
        let broken =
            "<m_CurProfileID>0</><m_NextProfileGUID>1</><m_Profiles><arraysize>1</><i0000></></>";
        std::fs::write(&path, broken).unwrap();
        capture_warnings();
        assert_eq!(load_profiles(&path), ProfilesFile::default());
        let backup = path.with_file_name("profiles.ini.broken");
        assert!(!path.exists(), "the corrupted file is set aside");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), broken);
        let warnings = take_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("profiles.ini.broken"), "{warnings:?}");
        // A new corrupted file replaces the old backup (only the last one stays).
        std::fs::write(&path, "<m_CurProfileID>5</>").unwrap();
        assert_eq!(load_profiles(&path), ProfilesFile::default());
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "<m_CurProfileID>5</>"
        );
        assert!(!path.exists());
        // Afterwards saving creates a new `profiles.ini`, the backup stays.
        save_profiles(&path, &file).unwrap();
        assert_eq!(load_profiles(&path), file);
        assert!(backup.exists());
    }
}
