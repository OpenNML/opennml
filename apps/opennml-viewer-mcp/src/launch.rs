//! Starting the viewer or `opennml-game`: repo root, path of the exe, log from stderr and the port line of the
//! control channel.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use schemars::JsonSchema;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::oneshot;

/// Number of log lines kept.
pub const LOG_LINES: usize = 2000;

/// Build profile of the viewer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    #[default]
    Release,
    Debug,
}

impl Profile {
    fn dir(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Debug => "debug",
        }
    }

    /// Command that builds `program` in this profile (the MCP does not build by itself).
    pub fn build_command(self, program: Program) -> String {
        match self {
            Self::Release => format!("cargo build --release -p {}", program.package()),
            Self::Debug => format!("cargo build -p {}", program.package()),
        }
    }
}

/// Program that `viewer_start` starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Program {
    /// `opennml-viewer`: maps, models, dialogs, simulation
    #[default]
    Viewer,
    /// `opennml-game`: the game, starts in the main menu; control channel with a subset of the commands, `state`/`step`
    /// return the state of the UI
    Game,
}

impl Program {
    /// Name of the cargo package (`cargo build -p`).
    pub fn package(self) -> &'static str {
        match self {
            Self::Viewer => "opennml-viewer",
            Self::Game => "opennml-game",
        }
    }

    /// File stem of the exe; the game is built as `OpenNML.exe`, not under its package name.
    pub fn exe_stem(self) -> &'static str {
        match self {
            Self::Viewer => "opennml-viewer",
            Self::Game => "OpenNML",
        }
    }
}

/// Repo root: `NML_REPO`, otherwise from the path of the own exe
/// (`<repo>/target/<profile>/opennml-viewer-mcp.exe` -> `<repo>`).
pub fn repo_root(env: Option<OsString>, exe: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = env.filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    match exe.parent().and_then(Path::parent) {
        Some(target) if target.file_name().is_some_and(|n| n == "target") => target
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("{} is not inside a repo", exe.display())),
        _ => Err(format!(
            "cannot determine the repo root from {} (expected <repo>/target/<profile>/); set NML_REPO",
            exe.display()
        )),
    }
}

/// Path of the built program (`<repo>/target/<profile>/opennml-viewer.exe` or `OpenNML.exe`).
pub fn program_exe(repo: &Path, program: Program, profile: Profile) -> PathBuf {
    repo.join("target").join(profile.dir()).join(format!(
        "{}{}",
        program.exe_stem(),
        std::env::consts::EXE_SUFFIX
    ))
}

/// Arguments of the viewer or of `opennml-game` (both understand the three switches): control channel on a free
/// port, with `paused` the stopped clock (`--paused`, unless the caller already gave it), without `audio`
/// the muted output (`--mute`, unless the caller already gave it), then the arguments of the caller.
pub fn viewer_args(args: &[String], paused: bool, audio: bool) -> Vec<String> {
    let mut all = vec!["--control".to_string(), "127.0.0.1:0".to_string()];
    if paused && !args.iter().any(|a| a == "--paused") {
        all.push("--paused".to_string());
    }
    if !audio && !args.iter().any(|a| a == "--mute") {
        all.push("--mute".to_string());
    }
    all.extend_from_slice(args);
    all
}

/// Port from the log line `… Steuerkanal: 127.0.0.1:PORT`.
pub fn parse_port_line(line: &str) -> Option<u16> {
    let (_, addr) = line.split_once("Steuerkanal: ")?;
    addr.trim().parse::<SocketAddr>().ok().map(|a| a.port())
}

/// The last [`LOG_LINES`] lines of stderr of the started viewer.
#[derive(Debug, Clone, Default)]
pub struct LogBuffer(Arc<Mutex<VecDeque<String>>>);

impl LogBuffer {
    fn lines(&self) -> std::sync::MutexGuard<'_, VecDeque<String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn push(&self, line: String) {
        let mut lines = self.lines();
        if lines.len() == LOG_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The last `n` lines (oldest first).
    pub fn last(&self, n: usize) -> Vec<String> {
        let lines = self.lines();
        lines
            .iter()
            .skip(lines.len().saturating_sub(n))
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.lines().clear();
    }
}

/// Reads stderr line by line into `log` and reports through `port` the port of the first
/// control channel line. Ends with the stream (process exit); unreported until then means: the
/// viewer terminated beforehand.
pub async fn read_log(stderr: impl AsyncRead + Unpin, log: LogBuffer, port: oneshot::Sender<u16>) {
    let mut reader = BufReader::new(stderr);
    let mut port = Some(port);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                if let Some(found) = parse_port_line(&line)
                    && let Some(tx) = port.take()
                {
                    let _ = tx.send(found);
                }
                log.push(line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_root_comes_from_the_environment_or_the_exe_path() {
        let exe = Path::new("E:/nml/rust/target/release/opennml-viewer-mcp.exe");
        assert_eq!(repo_root(None, exe).unwrap(), Path::new("E:/nml/rust"));
        assert_eq!(
            repo_root(Some("E:/other".into()), exe).unwrap(),
            Path::new("E:/other")
        );
        // An empty variable does not count.
        assert_eq!(
            repo_root(Some("".into()), exe).unwrap(),
            Path::new("E:/nml/rust")
        );
        let error = repo_root(None, Path::new("C:/Programs/opennml-viewer-mcp.exe")).unwrap_err();
        assert!(error.contains("NML_REPO"), "{error}");
    }

    #[test]
    fn viewer_args_start_paused_unless_told_otherwise() {
        let extra = vec!["--window-mode".to_string(), "windowed".to_string()];
        assert_eq!(
            viewer_args(&extra, true, false),
            [
                "--control",
                "127.0.0.1:0",
                "--paused",
                "--mute",
                "--window-mode",
                "windowed"
            ]
        );
        assert_eq!(
            viewer_args(&extra, false, false),
            [
                "--control",
                "127.0.0.1:0",
                "--mute",
                "--window-mode",
                "windowed"
            ]
        );
        // Already given: not twice (clap rejects repeated switches).
        let given = vec!["--paused".to_string()];
        assert_eq!(
            viewer_args(&given, true, false),
            ["--control", "127.0.0.1:0", "--mute", "--paused"]
        );
        assert_eq!(
            viewer_args(&[], true, false),
            ["--control", "127.0.0.1:0", "--paused", "--mute"]
        );
    }

    #[test]
    fn viewer_args_are_mute_unless_audio_is_requested_or_given() {
        // `audio: true`: no --mute, the sound is audible.
        assert_eq!(
            viewer_args(&[], true, true),
            ["--control", "127.0.0.1:0", "--paused"]
        );
        // The caller gives --mute itself: not twice.
        let given = vec!["--mute".to_string()];
        assert_eq!(
            viewer_args(&given, true, false),
            ["--control", "127.0.0.1:0", "--paused", "--mute"]
        );
        // Even with audio: true an explicit --mute of the caller stays.
        assert_eq!(
            viewer_args(&given, false, true),
            ["--control", "127.0.0.1:0", "--mute"]
        );
    }

    #[test]
    fn game_exe_lies_in_the_profile_directory() {
        let repo = Path::new("E:/nml/rust");
        assert_eq!(
            program_exe(repo, Program::Game, Profile::Release),
            repo.join("target")
                .join("release")
                .join(format!("OpenNML{}", std::env::consts::EXE_SUFFIX))
        );
        assert_eq!(Program::default(), Program::Viewer);
        assert_eq!(
            serde_json::from_str::<Program>(r#""game""#).unwrap(),
            Program::Game
        );
        // Build target per program and profile (the MCP does not build, it names the command).
        assert_eq!(
            Profile::Release.build_command(Program::Game),
            "cargo build --release -p opennml-game"
        );
        assert_eq!(
            Profile::Debug.build_command(Program::Game),
            "cargo build -p opennml-game"
        );
        assert_eq!(
            Profile::Release.build_command(Program::Viewer),
            "cargo build --release -p opennml-viewer"
        );
    }

    #[test]
    fn viewer_exe_lies_in_the_profile_directory() {
        let exe = program_exe(Path::new("E:/nml/rust"), Program::Viewer, Profile::Debug);
        assert!(exe.starts_with("E:/nml/rust/target/debug"));
        assert!(
            exe.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("opennml-viewer")
        );
        assert_eq!(Profile::default(), Profile::Release);
        assert_eq!(Program::Viewer.exe_stem(), Program::Viewer.package());
    }

    #[test]
    fn port_line_is_found_in_env_logger_output() {
        assert_eq!(
            parse_port_line(
                "[2026-10-10T13:51:05Z INFO  opennml_viewer] Steuerkanal: 127.0.0.1:60036"
            ),
            Some(60036)
        );
        assert_eq!(parse_port_line("Steuerkanal: [::1]:4711"), Some(4711));
        assert_eq!(
            parse_port_line("[... INFO  opennml_viewer] Settings: ..."),
            None
        );
        assert_eq!(parse_port_line("Steuerkanal: broken"), None);
    }

    #[tokio::test]
    async fn stderr_is_logged_and_the_port_reported() {
        let stderr: &[u8] = b"first line\r\n[x INFO  opennml_viewer] Steuerkanal: 127.0.0.1:4711\nSteuerkanal: 127.0.0.1:1\n\xFFend";
        let log = LogBuffer::default();
        let (tx, rx) = oneshot::channel();
        read_log(stderr, log.clone(), tx).await;
        assert_eq!(rx.await.unwrap(), 4711, "only the first port line");
        assert_eq!(
            log.last(10),
            [
                "first line",
                "[x INFO  opennml_viewer] Steuerkanal: 127.0.0.1:4711",
                "Steuerkanal: 127.0.0.1:1",
                "\u{FFFD}end"
            ]
        );
        // The stream ends without a port line: the receiver learns of it.
        let (tx, rx) = oneshot::channel();
        read_log(&b"Error: level is missing\n"[..], LogBuffer::default(), tx).await;
        assert!(rx.await.is_err());
    }

    #[test]
    fn the_log_keeps_the_last_2000_lines() {
        let log = LogBuffer::default();
        for i in 0..2500 {
            log.push(format!("Line {i}"));
        }
        let all = log.last(usize::MAX);
        assert_eq!(all.len(), LOG_LINES);
        assert_eq!(all[0], "Line 500");
        assert_eq!(log.last(2), ["Line 2498", "Line 2499"]);
        log.clear();
        assert!(log.last(5).is_empty());
    }
}
