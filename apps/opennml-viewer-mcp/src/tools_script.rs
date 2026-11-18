//! Tool `viewer_script` (protocol 33): parameters and execution of the script commands
//! `script_state` (`state`, `log`), `script_events` (`events`) and `script_command` (`skip_dialog`) as well as the
//! restart of the viewer with a campaign mission (`start`); see `docs/viewer-control.md`, sections "Scripts" and
//! "Presentation (protocol 33)". The tool method is in `tools.rs` right after `viewer_ki`.

use opennml_control::{CAMPAIGN_DIFFICULTY_RANGE, Command, ScriptAction, ScriptBlockRef};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::launch::{Profile, Program};
use crate::session::Session;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptToolAction {
    /// Script state (script_state): containers with blocks (state, wait reason), namespaces, speech state,
    /// object names (names filters), mission objectives, countdown, script commands, game result and UI (ui:
    /// lock, cinemascope, selection mirror, last button); with block_container and block_index the conditions
    /// of this block with current value. Without block log; read-only
    State,
    /// only the block log of the last steps steps (default 50, at most 1000; fired, waiting, deleted,
    /// script commands with result); read-only
    Log,
    /// Presentation events of the last steps steps (script_events: camera, fade, cinemascope, portraits,
    /// lines, tips, info windows, light, weather, music, fog ...) with presentation state, UI,
    /// profile variables and script camera; read-only
    Events,
    /// skip the running dialog (script_command skip_dialog): ends in the next tick, appears in the
    /// command log
    SkipDialog,
    /// Restart the viewer with a campaign mission: first viewer_stop, then viewer_start with
    /// --mission <mission> --campaign-difficulty <difficulty> --mute --window-mode windowed (script and AI on)
    Start,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScriptParams {
    pub action: ScriptToolAction,
    /// state: container of the block whose conditions come along (level 1, dialog -1); together with block_index
    pub block_container: Option<i32>,
    /// state: index of the block in the script file; together with block_container
    pub block_index: Option<u32>,
    /// state: only object names whose name or value contains this text
    pub names: Option<String>,
    /// log, events: steps 0-1000 (default 50)
    pub steps: Option<u32>,
    /// start: file stem of the mission from campaign.ini (required), e.g. kamp1-1
    pub mission: Option<String>,
    /// start: difficulty 0-2 (viewer default: 1)
    pub difficulty: Option<i32>,
    /// start: build profile release (default) or debug
    pub profile: Option<Profile>,
    /// start: start with the clock stopped (default true)
    pub paused: Option<bool>,
}

impl ScriptParams {
    /// Command of the control channel; missing required fields and limits (`Command::validate`) are errors. `start` is
    /// not a command of the control channel ([`Self::start_args`]).
    pub fn command(&self) -> Result<Command, String> {
        let command = match self.action {
            ScriptToolAction::State => Command::ScriptState {
                block: self.block()?,
                names: self.names.clone(),
                log: Some(0),
            },
            ScriptToolAction::Log => Command::ScriptState {
                block: None,
                names: None,
                log: Some(
                    self.steps
                        .unwrap_or(opennml_control::DEFAULT_SCRIPT_LOG_STEPS),
                ),
            },
            ScriptToolAction::Events => Command::ScriptEvents { steps: self.steps },
            ScriptToolAction::SkipDialog => Command::ScriptCommand {
                action: ScriptAction::SkipDialog,
            },
            ScriptToolAction::Start => {
                return Err("start restarts the viewer and is not a control command".into());
            }
        };
        command.validate()?;
        Ok(command)
    }

    /// Block from `block_container` and `block_index`: both or neither.
    fn block(&self) -> Result<Option<ScriptBlockRef>, String> {
        match (self.block_container, self.block_index) {
            (None, None) => Ok(None),
            (Some(container), Some(index)) => Ok(Some(ScriptBlockRef { container, index })),
            (Some(_), None) => Err("block_container needs block_index".into()),
            (None, Some(_)) => Err("block_index needs block_container".into()),
        }
    }

    /// Arguments of the viewer for `start`: the mission with difficulty, muted in the window (`viewer_args` adds
    /// the control channel and `--paused`). The mission name may consist only of letters, digits, `-` and `_`.
    pub fn start_args(&self) -> Result<Vec<String>, String> {
        let mission = self.mission.as_deref().ok_or("start needs mission")?;
        if mission.is_empty()
            || !mission
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "mission = \"{mission}\" invalid: allowed are letters, digits, - and _ (file stem from campaign.ini, e.g. kamp1-1)"
            ));
        }
        let mut args = vec!["--mission".to_string(), mission.to_string()];
        if let Some(difficulty) = self.difficulty {
            if !CAMPAIGN_DIFFICULTY_RANGE.contains(&difficulty) {
                return Err(format!("difficulty = {difficulty} invalid: allowed 0 to 2"));
            }
            args.extend(["--campaign-difficulty".to_string(), difficulty.to_string()]);
        }
        args.extend(
            ["--mute", "--window-mode", "windowed"]
                .iter()
                .map(ToString::to_string),
        );
        Ok(args)
    }
}

/// Executes `viewer_script`. `state`/`events`: the response of the viewer as indented JSON text; `log`: only the
/// block log (`log` field of `script_state`); `skip_dialog`: `{step}`; `start`: first `viewer_stop` (a
/// running or connected viewer is ended, no viewer is not an error), then the start with the arguments
/// of [`ScriptParams::start_args`].
pub async fn execute(session: &mut Session, params: &ScriptParams) -> Result<String, String> {
    if params.action == ScriptToolAction::Start {
        let args = params.start_args()?;
        // A restart: a running viewer is ended; if none is running, that is not an error.
        let _ = session.stop().await;
        return session
            .start(
                args,
                Program::Viewer,
                params.profile.unwrap_or_default(),
                params.paused.unwrap_or(true),
                false,
            )
            .await;
    }
    let command = params.command()?;
    let value = session.command(command).await?;
    let value = match params.action {
        ScriptToolAction::Log => value.get("log").cloned().unwrap_or(serde_json::Value::Null),
        _ => value,
    };
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(value: serde_json::Value) -> ScriptParams {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn mcp_viewer_script_maps_actions_to_commands() {
        // state: script state without block log, optionally with block and name filter.
        assert_eq!(
            params(json!({"action": "state"})).command(),
            Ok(Command::ScriptState {
                block: None,
                names: None,
                log: Some(0)
            })
        );
        assert_eq!(
            params(
                json!({"action": "state", "block_container": 1, "block_index": 4, "names": "hero"})
            )
            .command(),
            Ok(Command::ScriptState {
                block: Some(ScriptBlockRef {
                    container: 1,
                    index: 4
                }),
                names: Some("hero".into()),
                log: Some(0)
            })
        );
        // log: only the log, default 50 steps.
        assert_eq!(
            params(json!({"action": "log"})).command(),
            Ok(Command::ScriptState {
                block: None,
                names: None,
                log: Some(50)
            })
        );
        assert_eq!(
            params(json!({"action": "log", "steps": 1000})).command(),
            Ok(Command::ScriptState {
                block: None,
                names: None,
                log: Some(1000)
            })
        );
        // events: events of the last steps.
        assert_eq!(
            params(json!({"action": "events"})).command(),
            Ok(Command::ScriptEvents { steps: None })
        );
        assert_eq!(
            params(json!({"action": "events", "steps": 200})).command(),
            Ok(Command::ScriptEvents { steps: Some(200) })
        );
        // skip_dialog: command of the simulation via script_command.
        assert_eq!(
            params(json!({"action": "skip_dialog"})).command(),
            Ok(Command::ScriptCommand {
                action: ScriptAction::SkipDialog
            })
        );
        // start: not a control command; the arguments are mission and difficulty, muted in the window.
        let start = params(json!({"action": "start", "mission": "kamp1-1", "difficulty": 2}));
        assert!(
            start
                .command()
                .unwrap_err()
                .contains("not a control command")
        );
        assert_eq!(
            start.start_args().unwrap(),
            [
                "--mission",
                "kamp1-1",
                "--campaign-difficulty",
                "2",
                "--mute",
                "--window-mode",
                "windowed"
            ]
        );
        assert_eq!(
            params(json!({"action": "start", "mission": "kamp3-14"}))
                .start_args()
                .unwrap(),
            [
                "--mission",
                "kamp3-14",
                "--mute",
                "--window-mode",
                "windowed"
            ],
            "without difficulty the viewer default applies"
        );
        // Missing required fields and limits: error with the field name.
        for (bad, field) in [
            (
                json!({"action": "state", "block_container": 1}),
                "block_index",
            ),
            (
                json!({"action": "state", "block_index": 1}),
                "block_container",
            ),
            (json!({"action": "log", "steps": 1001}), "log"),
            (json!({"action": "events", "steps": 1001}), "steps"),
        ] {
            let error = params(bad.clone()).command().unwrap_err();
            assert!(error.contains(field), "{bad}: {error}");
        }
        for (bad, field) in [
            (json!({"action": "start"}), "mission"),
            (json!({"action": "start", "mission": ""}), "mission"),
            (json!({"action": "start", "mission": "../x"}), "mission"),
            (json!({"action": "start", "mission": "a b"}), "mission"),
            (
                json!({"action": "start", "mission": "kamp1-1", "difficulty": 3}),
                "difficulty",
            ),
            (
                json!({"action": "start", "mission": "kamp1-1", "difficulty": -1}),
                "difficulty",
            ),
        ] {
            let error = params(bad.clone()).start_args().unwrap_err();
            assert!(error.contains(field), "{bad}: {error}");
        }
        assert!(serde_json::from_value::<ScriptParams>(json!({"action": "run"})).is_err());
        assert!(serde_json::from_value::<ScriptParams>(json!({})).is_err());
    }

    #[test]
    fn start_defaults_are_a_paused_release_viewer() {
        let p = params(json!({"action": "start", "mission": "kamp1-1"}));
        assert_eq!(p.profile.unwrap_or_default(), Profile::Release);
        assert!(p.paused.unwrap_or(true));
        let p = params(
            json!({"action": "start", "mission": "kamp1-1", "profile": "debug", "paused": false}),
        );
        assert_eq!(p.profile, Some(Profile::Debug));
        assert_eq!(p.paused, Some(false));
    }
}
