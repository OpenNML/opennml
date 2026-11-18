//! Tool `viewer_ui`: parameters and execution of the UI commands `ui_state`,
//! `ui_click`, `ui_key`, `ui_open`, `ui_video`, `ui_present` (`docs/viewer-control.md`, section "UI (`ui_*`)").
//! The tool method is in `tools.rs` right after `viewer_state`.

use opennml_control::{Command, MouseButtonName};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::session::Session;
use crate::tools::ButtonParam;

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UiAction {
    /// State of the UI: dialogs, named elements (rectangle, visible, locked, value), focus, images,
    /// last effects, with opennml-game and a match also game (map, game menu state, steps, checksum,
    /// loading progress; minimap {visible, mode, center, theta, pings, idle_workers, fog_swaps},
    /// diplomacy_open and chat_lines) (ui_state)
    State,
    /// Click on the center of element in dialog in the next frame, like the mouse by hand (ui_click)
    Click,
    /// Key event chord (key names of the original) with the character char in the next frame (ui_key)
    Key,
    /// Queue a dialog for opening like command 5 (ui_open)
    Open,
    /// Play video name like videostart in the menu campaignvideo: open menus closed, PlayVideo,
    /// WaitVideo, AfterVideo, menus opened again (ui_video)
    Video,
    /// opennml-game only: simulate the loss of the window focus - held keys released, the match pauses like the
    /// original, also with a control channel (ui_focus_lost)
    FocusLost,
    /// Trigger a presentation event (parameter event) by hand, presentation only like an event of the scripts: bars,
    /// portrait, text, fade; the checksum stays, game speed and fog mode of the host change as well (ui_present, only
    /// with a
    /// match)
    Present,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UiParams {
    pub action: UiAction,
    /// click, open: dialog name as in state (case-insensitive), e.g. start, options, credits
    pub dialog: Option<String>,
    /// click: element name as in state, e.g. options, ok. With dialog GAMEUI (opennml-game with a match only) the elements
    /// of the game UI: command bar (normal:<0..13>, guid:<msg>, queue:<i>, selection:<i>, stance:<i>, formation:<i>,
    /// hero:<i>, headhunter:<i>, portrait, value:<i>, counterupgrade:<i>, toolbar), statusbar and the minimap: minimap
    /// (center of the diamond, camera jump), minimap_mode (mode button), minimap_idle (576), minimap_signal (571),
    /// minimap_idle_all (577), minimap_menu (579, game menu), minimap_diplomacy (580, diplomacy dialog; not in the
    /// campaign). Two clicks on minimap_idle without a step in between are always a double click when the clock is
    /// stopped (camera move); in between a step with more time than DoubleClickTimeout. minimap_menu and
    /// minimap_diplomacy open a menu immediately, a second click is blocked by it.
    pub element: Option<String>,
    /// click: mouse button (default left)
    pub button: Option<ButtonParam>,
    /// key: key names of the original in press order, e.g. ESCAPE, RETURN, "CONTROL H" (case-insensitive).
    /// opennml-game with a match: RETURN, "SHIFT RETURN" and "CONTROL RETURN" open chat window 1
    /// (all), 2 (friends) and 3 (chat mask); the text comes character by character, one call per character with
    /// chord A and the character as char; RETURN sends. "SHIFT D" toggles the diplomacy dialog
    pub chord: Option<String>,
    /// key: character (cp1252), optional
    pub char: Option<u16>,
    /// video: name of the .bik without path and extension (data/videos/<name>.bik), e.g. eng
    pub name: Option<String>,
    /// present: event in the JSON form of the presentation, an object with exactly one key, e.g.
    /// {"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}}, {"Fade": {"into": false, "ms": 500}},
    /// {"Cinemascope": {"on": true}}, {"Speech": {"text": 7, "slot": 0, "ms": 1000}}; unit variants as a
    /// string, e.g. "CameraBackup"
    pub event: Option<serde_json::Value>,
}

fn required(value: &Option<String>, action: &str, field: &str) -> Result<String, String> {
    value
        .clone()
        .ok_or_else(|| format!("{action} needs {field}"))
}

impl UiParams {
    /// Command of the control channel; missing required fields and empty names (`Command::validate`) are errors.
    pub fn command(&self) -> Result<Command, String> {
        let command = match self.action {
            UiAction::State => Command::UiState,
            UiAction::Click => Command::UiClick {
                dialog: required(&self.dialog, "click", "dialog")?,
                element: required(&self.element, "click", "element")?,
                button: self.button.map(|b| match b {
                    ButtonParam::Left => MouseButtonName::Left,
                    ButtonParam::Right => MouseButtonName::Right,
                    ButtonParam::Middle => MouseButtonName::Middle,
                }),
            },
            UiAction::Key => Command::UiKey {
                chord: required(&self.chord, "key", "chord")?,
                char: self.char,
            },
            UiAction::Open => Command::UiOpen {
                dialog: required(&self.dialog, "open", "dialog")?,
            },
            UiAction::Video => Command::UiVideo {
                name: required(&self.name, "video", "name")?,
            },
            UiAction::FocusLost => Command::UiFocusLost,
            UiAction::Present => Command::UiPresent {
                event: self
                    .event
                    .clone()
                    .ok_or_else(|| "present needs event".to_string())?,
            },
        };
        command.validate()?;
        Ok(command)
    }
}

/// Executes `viewer_ui`; result: the response of the program as indented JSON text (`state`: state of the
/// UI, `click`: `{"x", "y"}`, `key`: `{"vk"}`, `open`, `video`, `focus_lost` and `present`: `null`).
pub async fn execute(session: &mut Session, params: &UiParams) -> Result<String, String> {
    let command = params.command()?;
    let value = session.command(command).await?;
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::Program;
    use crate::session::tests::{fake_game, ui_example};
    use opennml_control::UiStateData;
    use serde_json::json;

    fn params(value: serde_json::Value) -> UiParams {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn ui_params_map_video_to_ui_video() {
        assert_eq!(
            params(json!({"action": "video", "name": "eng"}))
                .command()
                .unwrap(),
            Command::UiVideo { name: "eng".into() }
        );
        // Without a name (also with the field of another command) and with an empty name: error, nothing sent.
        for missing in [
            json!({"action": "video"}),
            json!({"action": "video", "dialog": "eng"}),
        ] {
            let error = params(missing.clone()).command().unwrap_err();
            assert!(error.contains("name"), "{missing}: {error}");
        }
        let error = params(json!({"action": "video", "name": " "}))
            .command()
            .unwrap_err();
        assert!(error.contains("name is empty"), "{error}");
    }

    #[test]
    fn ui_params_map_present_to_ui_present() {
        let event = json!({"Portrait": {"slot": 1, "portrait": 1000, "frame": 101}});
        assert_eq!(
            params(json!({"action": "present", "event": event}))
                .command()
                .unwrap(),
            Command::UiPresent { event }
        );
        assert_eq!(
            params(json!({"action": "present", "event": "CameraBackup"}))
                .command()
                .unwrap(),
            Command::UiPresent {
                event: json!("CameraBackup")
            }
        );
        // Without event and with the wrong form: error, nothing sent.
        let error = params(json!({"action": "present"})).command().unwrap_err();
        assert!(error.contains("event"), "{error}");
        let error = params(json!({"action": "present", "event": 3}))
            .command()
            .unwrap_err();
        assert!(error.contains("event must be an object"), "{error}");
    }

    #[test]
    fn ui_params_map_to_commands() {
        assert_eq!(
            params(json!({"action": "state"})).command().unwrap(),
            Command::UiState
        );
        assert_eq!(
            params(json!({"action": "click", "dialog": "start", "element": "options"}))
                .command()
                .unwrap(),
            Command::UiClick {
                dialog: "start".into(),
                element: "options".into(),
                button: None,
            }
        );
        assert_eq!(
            params(json!({"action": "click", "dialog": "start", "element": "options", "button": "right"}))
                .command()
                .unwrap(),
            Command::UiClick {
                dialog: "start".into(),
                element: "options".into(),
                button: Some(MouseButtonName::Right),
            }
        );
        assert_eq!(
            params(json!({"action": "key", "chord": "CONTROL H"}))
                .command()
                .unwrap(),
            Command::UiKey {
                chord: "CONTROL H".into(),
                char: None,
            }
        );
        assert_eq!(
            params(json!({"action": "key", "chord": "a", "char": 228}))
                .command()
                .unwrap(),
            Command::UiKey {
                chord: "a".into(),
                char: Some(228),
            }
        );
        assert_eq!(
            params(json!({"action": "open", "dialog": "credits"}))
                .command()
                .unwrap(),
            Command::UiOpen {
                dialog: "credits".into(),
            }
        );
        // Focus loss (opennml-game only).
        assert_eq!(
            params(json!({"action": "focus_lost"})).command().unwrap(),
            Command::UiFocusLost
        );
        // Missing required fields: error with the field name.
        for (missing, field) in [
            (json!({"action": "click", "element": "options"}), "dialog"),
            (json!({"action": "click", "dialog": "start"}), "element"),
            (json!({"action": "key"}), "chord"),
            (json!({"action": "key", "char": 65}), "chord"),
            (json!({"action": "open"}), "dialog"),
        ] {
            let error = params(missing.clone()).command().unwrap_err();
            assert!(error.contains(field), "{missing}: {error}");
        }
        // The protocol already checks empty names (`Command::validate`).
        let error = params(json!({"action": "open", "dialog": " "}))
            .command()
            .unwrap_err();
        assert!(error.contains("empty"), "{error}");
        let error = params(json!({"action": "key", "chord": ""}))
            .command()
            .unwrap_err();
        assert!(error.contains("empty key combination"), "{error}");
        // Unknown action or mouse button: reading already fails; the protocol rejects the middle one (quirk O-331).
        assert!(serde_json::from_value::<UiParams>(json!({"action": "hover"})).is_err());
        assert!(
            serde_json::from_value::<UiParams>(
                json!({"action": "click", "dialog": "a", "element": "b", "button": "x1"})
            )
            .is_err()
        );
        let error =
            params(json!({"action": "click", "dialog": "a", "element": "b", "button": "middle"}))
                .command()
                .unwrap_err();
        assert!(error.contains("middle button"), "{error}");
    }

    #[tokio::test]
    async fn the_ui_tool_sends_the_command_and_returns_the_answer() {
        let (port, rx) = fake_game();
        let mut session = Session::new(Err("no repo".into()));
        session.attach_as(port, Program::Game).await.unwrap();
        let text = execute(
            &mut session,
            &params(json!({"action": "click", "dialog": "start", "element": "options"})),
        )
        .await
        .unwrap();
        let point: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(point, json!({"x": 204, "y": 377}));
        let text = execute(&mut session, &params(json!({"action": "state"})))
            .await
            .unwrap();
        let state: UiStateData = serde_json::from_str(&text).unwrap();
        assert_eq!(state, ui_example());
        // Invalid parameters: nothing sent.
        assert!(
            execute(&mut session, &params(json!({"action": "key"})))
                .await
                .is_err()
        );
        let sent: Vec<Command> = rx.try_iter().collect();
        assert_eq!(
            sent,
            [
                Command::Hello,
                Command::UiClick {
                    dialog: "start".into(),
                    element: "options".into(),
                    button: None,
                },
                Command::UiState,
            ]
        );
    }
}
