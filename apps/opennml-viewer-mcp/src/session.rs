//! At most one viewer connection: started by itself (with log) or attached to a running viewer.
//! Independent of rmcp (testable).

use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use opennml_control::{
    Command, HelloInfo, PROTOCOL_VERSION, ScreenshotData, StateSnapshot, UiStateData,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::process::Child;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::connection::{Connection, RequestError};
use crate::launch::{LogBuffer, Profile, Program, program_exe, read_log, viewer_args};

/// Maximum duration until the log line `Steuerkanal: …` after the start.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Wait time for the process to end after `quit`.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// Log lines in error messages.
const ERROR_LOG_LINES: usize = 20;

struct Viewer {
    connection: Connection,
    /// `None` with `viewer_attach`.
    child: Option<Child>,
    /// Started program; determines the shape of the response of `state`.
    program: Program,
}

/// State after `viewer_state`/`viewer_input`/`viewer_time`: the state snapshot of the viewer or, with `opennml-game`, the
/// state of the UI (there `state` and `step` answer with [`UiStateData`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Snapshot {
    Viewer(Box<StateSnapshot>),
    Game(Box<UiStateData>),
}

pub struct Session {
    repo: Result<PathBuf, String>,
    log: LogBuffer,
    /// Reads stderr of the started viewer until the process ends.
    log_reader: Option<JoinHandle<()>>,
    viewer: Option<Viewer>,
    /// Message after the viewer has terminated (until the next start).
    ended: Option<String>,
}

fn exit_text(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "exit code unknown".into(), |c| format!("exit code {c}"))
}

impl Session {
    pub fn new(repo: Result<PathBuf, String>) -> Self {
        Self {
            repo,
            log: LogBuffer::default(),
            log_reader: None,
            viewer: None,
            ended: None,
        }
    }

    /// Message when no viewer is connected (with the last log lines, if it was running).
    fn not_running(&self) -> String {
        match &self.ended {
            Some(message) => {
                let lines = self.log.last(ERROR_LOG_LINES);
                if lines.is_empty() {
                    message.clone()
                } else {
                    format!("{message}\nLast log lines:\n{}", lines.join("\n"))
                }
            }
            None => "No viewer connected (run viewer_start or viewer_attach first)".into(),
        }
    }

    /// Waits at most `wait` for `child` to end (otherwise it is killed) and reads the rest of
    /// stderr into the log; `None` if the process had to be killed.
    async fn end_child(&mut self, mut child: Child, wait: Duration) -> Option<ExitStatus> {
        let status = match timeout(wait, child.wait()).await {
            Ok(Ok(status)) => Some(status),
            _ => {
                let _ = child.kill().await;
                None
            }
        };
        if let Some(reader) = self.log_reader.take() {
            let _ = timeout(Duration::from_secs(1), reader).await;
        }
        status
    }

    /// Connection lost: wait for the process to end and remember it.
    async fn lost(&mut self, reason: &str) -> String {
        if let Some(viewer) = self.viewer.take() {
            let status = match viewer.child {
                Some(child) => self
                    .end_child(child, Duration::from_secs(2))
                    .await
                    .map_or_else(|| "not responding, process killed".into(), exit_text),
                None => reason.to_string(),
            };
            self.ended = Some(format!("Viewer is not running ({status})"));
        }
        self.not_running()
    }

    /// One command to the viewer.
    pub async fn command(&mut self, command: Command) -> Result<serde_json::Value, String> {
        let Some(viewer) = &mut self.viewer else {
            return Err(self.not_running());
        };
        match viewer.connection.request(command).await {
            Ok(value) => Ok(value),
            Err(RequestError::Viewer(message)) => Err(message),
            Err(RequestError::Lost(reason)) => Err(self.lost(&reason).await),
        }
    }

    async fn command_as<T: DeserializeOwned>(&mut self, command: Command) -> Result<T, String> {
        let value = self.command(command).await?;
        serde_json::from_value(value).map_err(|e| format!("response has an unexpected shape: {e}"))
    }

    /// Connects to `127.0.0.1:<port>` and calls `hello`; checks the protocol version.
    async fn connect(port: u16) -> Result<(Connection, HelloInfo), String> {
        let mut connection = Connection::connect(port)
            .await
            .map_err(|e| format!("Control channel 127.0.0.1:{port} not reachable: {e}"))?;
        let hello: HelloInfo = connection
            .request_as(Command::Hello)
            .await
            .map_err(|e| e.to_string())?;
        if hello.protocol != PROTOCOL_VERSION {
            return Err(format!(
                "Viewer speaks protocol {}, expected {PROTOCOL_VERSION} (rebuild viewer and MCP server together)",
                hello.protocol
            ));
        }
        Ok((connection, hello))
    }

    /// Is a viewer (still) running? A process that has ended in the meantime is cleaned up.
    fn running(&mut self) -> bool {
        let exited = match &mut self.viewer {
            None => return false,
            Some(Viewer {
                child: Some(child), ..
            }) => child.try_wait().ok().flatten(),
            Some(_) => None,
        };
        if let Some(status) = exited {
            self.viewer = None;
            self.ended = Some(format!("Viewer is not running ({})", exit_text(status)));
            return false;
        }
        true
    }

    /// Starts `target/<profile>/opennml-viewer` (or `opennml-game`) `--control 127.0.0.1:0 [--paused] [--mute] <args>`
    /// in the repo root (`--mute` as long as `audio` is not set). Does not build: if the exe is missing, the
    /// message names the build command.
    pub async fn start(
        &mut self,
        args: Vec<String>,
        program: Program,
        profile: Profile,
        paused: bool,
        audio: bool,
    ) -> Result<String, String> {
        if self.running() {
            return Err("A viewer is already running (run viewer_stop first)".into());
        }
        let repo = self.repo.clone()?;
        let exe = program_exe(&repo, program, profile);
        if !exe.is_file() {
            return Err(format!(
                "{} is missing - build first: {}",
                exe.display(),
                profile.build_command(program)
            ));
        }
        self.log.clear();
        self.ended = None;
        let mut child = tokio::process::Command::new(&exe)
            .args(viewer_args(&args, paused, audio))
            .current_dir(&repo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("{} cannot be started: {e}", exe.display()))?;
        let stderr = child.stderr.take().expect("stderr is redirected");
        let (port_tx, port_rx) = oneshot::channel();
        self.log_reader = Some(tokio::spawn(read_log(stderr, self.log.clone(), port_tx)));
        let port = match timeout(START_TIMEOUT, port_rx).await {
            Ok(Ok(port)) => port,
            Ok(Err(_)) => {
                let status = self
                    .end_child(child, Duration::from_secs(2))
                    .await
                    .map_or_else(|| "process killed".into(), exit_text);
                self.ended = Some(format!(
                    "Viewer terminated before the control channel ({status})"
                ));
                return Err(self.not_running());
            }
            Err(_) => {
                self.end_child(child, Duration::ZERO).await;
                self.ended = Some(format!(
                    "no log line \"Steuerkanal: ...\" within {} s, process killed",
                    START_TIMEOUT.as_secs()
                ));
                return Err(self.not_running());
            }
        };
        // If the viewer ends right after binding (e.g. level not loadable), exit code and reason
        // are in the log.
        let (connection, hello) = match Self::connect(port).await {
            Ok(connected) => connected,
            Err(message) => {
                let status = self
                    .end_child(child, Duration::from_secs(2))
                    .await
                    .map_or_else(|| "process killed".into(), exit_text);
                self.ended = Some(format!("Viewer is not running ({status}): {message}"));
                return Err(self.not_running());
            }
        };
        self.viewer = Some(Viewer {
            connection,
            child: Some(child),
            program,
        });
        Ok(format!(
            "{} started: PID {}, control channel 127.0.0.1:{port}, protocol {}",
            program.package(),
            hello.pid,
            hello.protocol
        ))
    }

    /// Connects to a running viewer (`--control 127.0.0.1:<port>`).
    pub async fn attach(&mut self, port: u16) -> Result<String, String> {
        self.attach_as(port, Program::Viewer).await
    }

    /// Connects to a running `program` (`viewer_attach` always assumes the viewer: `hello` does not
    /// distinguish the programs).
    pub async fn attach_as(&mut self, port: u16, program: Program) -> Result<String, String> {
        if self.running() {
            return Err("A viewer is already connected (run viewer_stop first)".into());
        }
        let (connection, hello) = Self::connect(port).await?;
        self.log.clear();
        self.ended = None;
        self.viewer = Some(Viewer {
            connection,
            child: None,
            program,
        });
        Ok(format!(
            "Connected to viewer PID {} on 127.0.0.1:{port} (log only with viewer_start)",
            hello.pid
        ))
    }

    /// `quit`; if necessary kills a viewer started by itself after 5 s.
    pub async fn stop(&mut self) -> Result<String, String> {
        let Some(mut viewer) = self.viewer.take() else {
            return Err(self.not_running());
        };
        let quit = viewer.connection.request(Command::Quit).await;
        self.ended = None;
        let Some(child) = viewer.child else {
            return match quit {
                Ok(_) => Ok("Viewer terminated (quit)".into()),
                Err(e) => Err(format!("quit not confirmed: {e}")),
            };
        };
        let status = self.end_child(child, STOP_TIMEOUT).await.map_or_else(
            || {
                format!(
                    "not ended after {} s, process killed",
                    STOP_TIMEOUT.as_secs()
                )
            },
            exit_text,
        );
        Ok(format!("Viewer terminated ({status})"))
    }

    pub async fn state(&mut self) -> Result<StateSnapshot, String> {
        self.command_as(Command::State).await
    }

    /// `state` in the form of the connected program.
    pub async fn snapshot(&mut self) -> Result<Snapshot, String> {
        match self.viewer.as_ref().map(|v| v.program) {
            Some(Program::Game) => self
                .command_as(Command::State)
                .await
                .map(|s| Snapshot::Game(Box::new(s))),
            _ => self.state().await.map(|s| Snapshot::Viewer(Box::new(s))),
        }
    }

    /// Executes the commands in order; result: the state afterwards ([`Self::snapshot`]). All commands are
    /// checked beforehand: an invalid one leaves the whole sequence unexecuted (otherwise a key, for
    /// example, would stay pressed).
    pub async fn input(&mut self, commands: Vec<Command>) -> Result<Snapshot, String> {
        for (i, command) in commands.iter().enumerate() {
            command
                .validate()
                .map_err(|e| format!("Action {}: {e} (nothing executed)", i + 1))?;
        }
        for (i, command) in commands.into_iter().enumerate() {
            self.command(command)
                .await
                .map_err(|e| format!("Action {}: {e}", i + 1))?;
        }
        self.snapshot().await
    }

    pub async fn screenshot(
        &mut self,
        overlay: bool,
        max_size: Option<u32>,
    ) -> Result<ScreenshotData, String> {
        self.command_as(Command::Screenshot {
            overlay,
            max_size,
            pointer: false,
        })
        .await
    }

    /// The last `lines` log lines of the viewer started by itself.
    pub fn log(&self, lines: usize) -> String {
        let lines = self.log.last(lines);
        if lines.is_empty() {
            "(no log: only a viewer started with viewer_start writes here)".into()
        } else {
            lines.join("\n")
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use opennml_control::{Response, read_line, write_line};
    use std::io::BufReader;
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// Fake viewer on a free port: answers `hello`, `state`/`step` with the example state
    /// snapshot, `screenshot` with a PNG, `lights` with two names, `key` with code
    /// "Insert" with an error, everything else with `null`. After `close_after` requests it closes the connection.
    /// Returns the port and the received commands.
    pub(crate) fn fake_viewer(close_after: usize) -> (u16, mpsc::Receiver<Command>) {
        fake_server(close_after, false)
    }

    /// Fake `opennml-game`: `state`/`step`/`ui_state` with [`ui_example`], `ui_click` with the click point,
    /// `ui_key` with the key code, commands outside the subset with an error, everything else with `null`.
    pub(crate) fn fake_game() -> (u16, mpsc::Receiver<Command>) {
        fake_server(usize::MAX, true)
    }

    /// Small UI state of `opennml-game` (main menu).
    pub(crate) fn ui_example() -> UiStateData {
        UiStateData {
            now_ms: 1008,
            screen: [1024, 768],
            dialogs: vec![opennml_control::UiDialogData {
                name: "start".into(),
                rect: [0, 0, 1024, 768],
                animating: false,
                modal: false,
            }],
            focus: None,
            hover: None,
            elements: Vec::new(),
            images: Vec::new(),
            busy: false,
            queued: 0,
            effects: Vec::new(),
            video: None,
            pointer_hidden: false,
            game: None,
            menu_level: None,
            accounting: None,
            play_time: None,
            pointer_model: None,
        }
    }

    fn fake_server(close_after: usize, game: bool) -> (u16, mpsc::Receiver<Command>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            for _ in 0..close_after {
                let Ok(opennml_control::LineRead::Line(line)) = read_line(&mut reader, 1 << 20)
                else {
                    return;
                };
                let request = opennml_control::parse_request(&line).unwrap();
                let id = request.id;
                let response = match &request.command {
                    Command::Hello => Response::ok(
                        id,
                        HelloInfo {
                            protocol: PROTOCOL_VERSION,
                            pid: 4242,
                            backend: None,
                            adapter: None,
                        },
                    ),
                    Command::State | Command::Step { .. } | Command::UiState if game => {
                        Response::ok(id, ui_example())
                    }
                    Command::UiClick { .. } if game => {
                        Response::ok(id, serde_json::json!({"x": 204, "y": 377}))
                    }
                    Command::UiKey { .. } if game => {
                        Response::ok(id, serde_json::json!({"vk": [27]}))
                    }
                    Command::Lights | Command::Key { .. } if game => {
                        Response::error(id, "Command not available in opennml-game")
                    }
                    Command::State | Command::Step { .. } => {
                        Response::ok(id, StateSnapshot::example())
                    }
                    Command::Screenshot { .. } => Response::ok(
                        id,
                        ScreenshotData {
                            width: 2,
                            height: 1,
                            rgba_hash: "0123456789abcdef".into(),
                            png_width: 2,
                            png_height: 1,
                            png_base64: "iVBORw0KGgo=".into(),
                        },
                    ),
                    Command::Lights => Response::ok(id, ["level", "Sommer Tag"]),
                    Command::Key { code, .. } if code == "Insert" => {
                        Response::error(id, "key \"Insert\" not bound here")
                    }
                    _ => Response::ok(id, ()),
                };
                let _ = tx.send(request.command);
                write_line(&mut writer, &response).unwrap();
            }
        });
        (port, rx)
    }

    fn received(rx: &mpsc::Receiver<Command>) -> Vec<Command> {
        rx.try_iter().collect()
    }

    fn session() -> Session {
        Session::new(Err("no repo in the test".into()))
    }

    #[tokio::test]
    async fn input_actions_become_commands_followed_by_a_state_query() {
        let (port, rx) = fake_viewer(usize::MAX);
        let mut session = session();
        let text = session.attach(port).await.unwrap();
        assert!(text.contains("PID 4242"), "{text}");
        let state = session
            .input(vec![
                Command::Key {
                    code: "F3".into(),
                    action: opennml_control::KeyAction::Tap,
                    repeat: 0,
                },
                Command::Step {
                    frames: 2,
                    frame_ms: 16.0,
                },
            ])
            .await
            .unwrap();
        assert_eq!(state, Snapshot::Viewer(Box::new(StateSnapshot::example())));
        assert_eq!(
            received(&rx),
            [
                Command::Hello,
                Command::Key {
                    code: "F3".into(),
                    action: opennml_control::KeyAction::Tap,
                    repeat: 0,
                },
                Command::Step {
                    frames: 2,
                    frame_ms: 16.0
                },
                Command::State,
            ]
        );
    }

    #[tokio::test]
    async fn an_invalid_action_sends_nothing() {
        let (port, rx) = fake_viewer(usize::MAX);
        let mut session = session();
        session.attach(port).await.unwrap();
        let error = session
            .input(vec![
                Command::Key {
                    code: "ArrowUp".into(),
                    action: opennml_control::KeyAction::Down,
                    repeat: 0,
                },
                Command::Step {
                    frames: 0,
                    frame_ms: 16.0,
                },
                Command::Key {
                    code: "ArrowUp".into(),
                    action: opennml_control::KeyAction::Up,
                    repeat: 0,
                },
            ])
            .await
            .unwrap_err();
        assert!(
            error.starts_with("Action 2: frames = 0 invalid")
                && error.ends_with("(nothing executed)"),
            "{error}"
        );
        // Only the `hello` from attaching; neither the key nor a `state` was sent.
        assert_eq!(received(&rx), [Command::Hello]);
        assert!(session.state().await.is_ok());
    }

    #[tokio::test]
    async fn viewer_errors_are_reported_and_the_connection_stays() {
        let (port, _rx) = fake_viewer(usize::MAX);
        let mut session = session();
        session.attach(port).await.unwrap();
        let error = session
            .input(vec![
                Command::Pause,
                Command::Key {
                    code: "Insert".into(),
                    action: opennml_control::KeyAction::Down,
                    repeat: 0,
                },
            ])
            .await
            .unwrap_err();
        assert_eq!(error, "Action 2: key \"Insert\" not bound here");
        assert!(session.state().await.is_ok());
        assert!(
            session
                .attach(port)
                .await
                .unwrap_err()
                .contains("already connected")
        );
    }

    #[tokio::test]
    async fn a_vanished_viewer_is_reported_on_every_call() {
        // Answers only `hello`, then the connection is gone.
        let (port, _rx) = fake_viewer(1);
        let mut session = session();
        session.attach(port).await.unwrap();
        let error = session.state().await.unwrap_err();
        assert!(error.starts_with("Viewer is not running"), "{error}");
        assert!(
            session
                .screenshot(false, None)
                .await
                .unwrap_err()
                .starts_with("Viewer is not running")
        );
        assert!(
            session
                .stop()
                .await
                .unwrap_err()
                .starts_with("Viewer is not running")
        );
        // Without ever having been connected:
        let error = Session::new(Err(String::new())).state().await.unwrap_err();
        assert!(error.contains("No viewer connected"), "{error}");
    }

    #[tokio::test]
    async fn stop_sends_quit_to_an_attached_viewer() {
        let (port, rx) = fake_viewer(usize::MAX);
        let mut session = session();
        session.attach(port).await.unwrap();
        assert_eq!(session.stop().await.unwrap(), "Viewer terminated (quit)");
        assert_eq!(received(&rx), [Command::Hello, Command::Quit]);
        assert!(
            session
                .state()
                .await
                .unwrap_err()
                .contains("No viewer connected")
        );
    }

    #[tokio::test]
    async fn start_needs_a_built_viewer() {
        let repo = std::env::temp_dir().join(format!("opennml-mcp-empty-{}", std::process::id()));
        let mut built = Session::new(Ok(repo));
        let error = built
            .start(vec![], Program::Viewer, Profile::Release, true, false)
            .await
            .unwrap_err();
        assert!(
            error.contains("cargo build --release -p opennml-viewer"),
            "{error}"
        );
        let error = session()
            .start(vec![], Program::Viewer, Profile::Debug, true, false)
            .await
            .unwrap_err();
        assert_eq!(error, "no repo in the test");
        assert!(built.log(50).contains("no log"));
    }

    #[tokio::test]
    async fn game_start_names_the_opennml_game_build() {
        // The MCP does not build; the message names the exe and build command of opennml-game.
        let repo =
            std::env::temp_dir().join(format!("opennml-mcp-empty-game-{}", std::process::id()));
        let mut session = Session::new(Ok(repo));
        let error = session
            .start(vec![], Program::Game, Profile::Release, true, false)
            .await
            .unwrap_err();
        assert!(
            error.contains(&format!("opennml-game{}", std::env::consts::EXE_SUFFIX))
                && error.contains("cargo build --release -p opennml-game"),
            "{error}"
        );
        let error = session
            .start(vec![], Program::Game, Profile::Debug, true, false)
            .await
            .unwrap_err();
        assert!(error.ends_with("cargo build -p opennml-game"), "{error}");
    }

    #[tokio::test]
    async fn game_state_and_input_read_the_ui_state() {
        // With opennml-game, `state`/`step` are the state of the UI.
        let (port, rx) = fake_game();
        let mut session = session();
        session.attach_as(port, Program::Game).await.unwrap();
        let expected = Snapshot::Game(Box::new(ui_example()));
        assert_eq!(session.snapshot().await.unwrap(), expected);
        let after = session
            .input(vec![Command::Step {
                frames: 2,
                frame_ms: 16.0,
            }])
            .await
            .unwrap();
        assert_eq!(after, expected);
        // Read as a state snapshot of the viewer, the response does not fit.
        let error = session.state().await.unwrap_err();
        assert!(error.contains("unexpected shape"), "{error}");
        assert_eq!(
            received(&rx),
            [
                Command::Hello,
                Command::State,
                Command::Step {
                    frames: 2,
                    frame_ms: 16.0
                },
                Command::State,
                Command::State,
            ]
        );
        // opennml-game itself reports commands outside the subset.
        let error = session.command(Command::Lights).await.unwrap_err();
        assert!(error.contains("not available"), "{error}");
    }
}
