//! "Script" window (key J, button "Script ..." in the "Simulation" window): per container (level, dialog)
//! countdown, namespaces and blocks with state, action index and wait reason; the conditions of the chosen block
//! with their current value; speech state; object names with search; the block log of the last steps; the mission
//! objectives with the countdown display; the presentation of the scripts as text and their events (running line
//! with text from the text table, tips, info windows, portraits, highlight, lock, cinemascope, fade), plus the
//! cinemascope bars and the game-end display in the map view. The lines are produced without egui (testable); the
//! window reads only the same data as `script_state` and `script_events` (`SimHost::control_script_state`,
//! [`script_events`]; the checksum stays the same).

use nml_client::game::script_camera::ScriptCamera;
use nml_client::game::sim_host::SimHost;
use nml_control::{
    DEFAULT_SCRIPT_LOG_STEPS, MAX_SCRIPT_LOG_STEPS, ScriptBlockRef, ScriptCommandsInfo,
    ScriptConditionInfo, ScriptContainerInfo, ScriptEventsResult, ScriptLogEntry, ScriptQuestInfo,
    ScriptSpeechInfo, ScriptStateResult,
};
use nml_formats::abl::catalog;
use nml_sim::game_end::{DEFEAT, VICTORY};
use nml_sim::script::{
    DIALOG_CONTAINER_ID, InfoFrameMemory, Presentation, PresentationMemory, ScriptTrace,
};

/// State of the "Script" window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptWindow {
    pub open: bool,
    /// Block whose conditions the window shows (click on a block row).
    pub selected: Option<ScriptBlockRef>,
    /// Search in the object names (name or value contains the text).
    pub names_search: String,
    /// Steps of the block log (1-1000).
    pub log_steps: u32,
}

impl Default for ScriptWindow {
    fn default() -> Self {
        Self {
            open: false,
            selected: None,
            names_search: String::new(),
            log_steps: DEFAULT_SCRIPT_LOG_STEPS,
        }
    }
}

impl ScriptWindow {
    /// Request of the window to `control_script_state`: chosen block, name search, log steps.
    pub fn request(&self) -> (Option<ScriptBlockRef>, Option<&str>, Option<u32>) {
        (
            self.selected,
            Some(self.names_search.as_str()),
            Some(self.log_steps.min(MAX_SCRIPT_LOG_STEPS)),
        )
    }
}

/// Name of a container: "Dialog" (-1) or "Level <ID>".
pub fn container_name(id: i32) -> String {
    if id == DIALOG_CONTAINER_ID {
        "Dialog".into()
    } else {
        format!("Level {id}")
    }
}

/// Countdown of a container (`m_iCounter` in ms, -1 = off).
pub fn countdown_text(counter: i32) -> String {
    if counter == -1 {
        "Countdown off".into()
    } else {
        format!("Countdown {counter} ms")
    }
}

/// Columns of the block table.
pub const BLOCK_COLUMNS: [&str; 5] = ["Block", "Name", "State", "Action", "Wait reason"];

/// Blocks of a container in file order: index, name, state, action index, wait reason.
pub fn block_rows(container: &ScriptContainerInfo) -> Vec<[String; 5]> {
    container
        .blocks
        .iter()
        .map(|b| {
            [
                b.index.to_string(),
                b.name.clone(),
                b.state.clone(),
                b.cur_action.to_string(),
                b.wait.clone().unwrap_or_else(|| "–".into()),
            ]
        })
        .collect()
}

/// Columns of the condition table.
pub const CONDITION_COLUMNS: [&str; 4] = ["Link", "Condition", "Parameter", "Value"];

/// Conditions of the chosen block: link (the first has no effect), number with catalog text, parameters, value
/// ("true", "false", "not yet implemented").
pub fn condition_rows(conditions: &[ScriptConditionInfo]) -> Vec<[String; 4]> {
    conditions
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let link = match (i, k.or) {
                (0, _) => "–",
                (_, true) => "or",
                (_, false) => "and",
            };
            let name = catalog::condition(k.number)
                .map(|s| s.name)
                .filter(|n| !n.is_empty());
            [
                link.into(),
                match name {
                    Some(n) => format!("{} {n}", k.number),
                    None => format!("{} (no case)", k.number),
                },
                k.params.join(";"),
                match k.value {
                    Some(true) => "true".into(),
                    Some(false) => "false".into(),
                    None => "not yet implemented".into(),
                },
            ]
        })
        .collect()
}

/// Namespaces of a container: kind, name, value (switches, random switches, timers in ms, variables).
pub fn namespace_rows(container: &ScriptContainerInfo) -> Vec<[String; 3]> {
    let mut rows = Vec::new();
    for (kind, list) in [
        ("Switch", &container.switches),
        ("Random switch", &container.random_switches),
        ("Timer", &container.timers),
        ("Variable", &container.variables),
    ] {
        for (name, value) in list {
            rows.push([kind.into(), name.clone(), value.to_string()]);
        }
    }
    rows
}

/// Speech state as lines: running dialog line, tips of slots 0/1, text events, info history.
pub fn speech_lines(speech: &ScriptSpeechInfo) -> Vec<String> {
    let list = |v: &[i32]| {
        if v.is_empty() {
            "–".to_string()
        } else {
            v.iter().map(i32::to_string).collect::<Vec<_>>().join(", ")
        }
    };
    let mut lines = vec![match speech.line {
        Some(l) => format!(
            "Dialog line: text {} on slot {}, {} ms remaining",
            l.text, l.slot, l.remaining_ms
        ),
        None => "Dialog line: none".into(),
    }];
    for (i, tip) in speech.tips.iter().enumerate() {
        lines.push(format!(
            "Tip {i}: text {}, {} ms remaining",
            tip.text, tip.remaining_ms
        ));
    }
    lines.push(format!("Text events: {}", list(&speech.events)));
    lines.push(format!("Info history: {}", list(&speech.info_history)));
    lines
}

/// Status of a mission objective as text (re/campaign-ui 4.5): 1 present (actions 198/199), 3 set (27/182),
/// 6 completed (184/185), 10 failed (186/187), 0 empty.
pub fn quest_status_text(status: i32) -> String {
    match status {
        0 => "empty".into(),
        1 => "present".into(),
        3 => "set".into(),
        6 => "completed".into(),
        10 => "failed".into(),
        other => format!("Status {other}"),
    }
}

/// Columns of the mission objective table.
pub const QUEST_COLUMNS: [&str; 4] = ["Kind", "No.", "Status", "Text ID"];

/// Mission objectives: first the main, then the sub-objectives; only entries with a status or text (number
/// 1-based as in the script), the text as text ID or "–".
pub fn quest_rows(main: &[ScriptQuestInfo], sub: &[ScriptQuestInfo]) -> Vec<[String; 4]> {
    let mut rows = Vec::new();
    for (kind, list) in [("Main objective", main), ("Sub-objective", sub)] {
        for (i, q) in list.iter().enumerate() {
            if q.status == 0 && q.text == 0 {
                continue;
            }
            rows.push([
                kind.into(),
                (i + 1).to_string(),
                quest_status_text(q.status),
                if q.text == 0 {
                    "–".into()
                } else {
                    q.text.to_string()
                },
            ]);
        }
    }
    rows
}

/// Lines below the mission objectives: the countdown display (the registered container and the seconds), the
/// task status of action 200 and the areas changed by scripts.
pub fn mission_lines(state: &ScriptStateResult) -> Vec<String> {
    let mut lines = vec![if state.countdown_container == 0 {
        "Countdown display: none".to_string()
    } else {
        format!(
            "Countdown display: {}, {} s",
            container_name(state.countdown_container),
            state.countdown_s
        )
    }];
    if !state.task_status.is_empty() {
        let tasks: Vec<String> = state
            .task_status
            .iter()
            .map(|(task, status)| format!("{task} = {status}"))
            .collect();
        lines.push(format!("Task status: {}", tasks.join(", ")));
    }
    if !state.area_overrides.is_empty() {
        let areas: Vec<String> = state
            .area_overrides
            .iter()
            .map(|(name, r)| format!("{name} = [{}, {}, {}, {}]", r[0], r[1], r[2], r[3]))
            .collect();
        lines.push(format!("Changed areas: {}", areas.join(", ")));
    }
    lines
}

/// A message of the block log as text (JSON form of [`ScriptTrace`]; unknown as JSON).
pub fn trace_text(trace: &serde_json::Value) -> String {
    let Ok(trace) = serde_json::from_value::<ScriptTrace>(trace.clone()) else {
        return trace.to_string();
    };
    let at = |c: i32, b: u32| format!("{} Block {b}", container_name(c));
    match trace {
        ScriptTrace::Fired { container, block } => format!("{}: fired", at(container, block)),
        ScriptTrace::Deleted { container, block } => format!("{}: deleted", at(container, block)),
        ScriptTrace::Suspended {
            container,
            block,
            action,
        } => format!("{}: waits at action {action}", at(container, block)),
        ScriptTrace::Pending {
            container,
            block,
            action,
        } => format!(
            "{}: action {action} has no effect yet",
            at(container, block)
        ),
        ScriptTrace::ConditionPending {
            container,
            block,
            condition,
        } => format!(
            "{}: condition {condition} not yet implemented",
            at(container, block)
        ),
        ScriptTrace::DialogStarted { path } => format!("Dialog started: {path}"),
        ScriptTrace::DialogEnded => "Dialog ended".into(),
        ScriptTrace::Guarded {
            container,
            block,
            number,
            why,
        } => format!(
            "{}: number {number} intercepted: {why}",
            at(container, block)
        ),
        ScriptTrace::Command {
            container,
            block,
            action,
            code,
            outcome,
        } => {
            let verdict = match outcome {
                nml_sim::command::Outcome::Accepted => "accepted".to_owned(),
                nml_sim::command::Outcome::Partial(why) => format!("partial: {why}"),
                nml_sim::command::Outcome::Rejected(why) => format!("rejected: {why}"),
            };
            format!(
                "{}: action {action} sends command {code:#04X}: {verdict}",
                at(container, block)
            )
        }
        ScriptTrace::Limit { what } => format!("Limit reached: {what}"),
    }
}

/// Block log, newest message first: step, message.
pub fn log_rows(log: &[ScriptLogEntry]) -> Vec<[String; 2]> {
    log.iter()
        .rev()
        .map(|e| [e.step.to_string(), trace_text(&e.trace)])
        .collect()
}

/// Columns of the table of script commands per action.
pub const COMMAND_COLUMNS: [&str; 4] = ["Action", "accepted", "partial", "rejected"];

/// Script commands per action (ascending by number): number with catalog text, accepted, partial, rejected.
pub fn command_rows(commands: &ScriptCommandsInfo) -> Vec<[String; 4]> {
    commands
        .actions
        .iter()
        .map(|a| {
            let name = catalog::action(a.action)
                .map(|s| s.name)
                .filter(|n| !n.is_empty());
            [
                match name {
                    Some(n) => format!("{} {n}", a.action),
                    None => a.action.to_string(),
                },
                a.accepted.to_string(),
                a.partial.to_string(),
                a.rejected.to_string(),
            ]
        })
        .collect()
}

/// Columns of the table of rejected script commands.
pub const REJECTION_COLUMNS: [&str; 5] = ["Step", "Block", "Action", "Command", "Reason"];

/// The last rejections, newest first: step, block, action, command type of the original, reason.
pub fn rejection_rows(commands: &ScriptCommandsInfo) -> Vec<[String; 5]> {
    commands
        .rejections
        .iter()
        .rev()
        .map(|r| {
            [
                r.step.to_string(),
                format!("{} Block {}", container_name(r.container), r.block),
                r.action.to_string(),
                format!("{:#04X}", r.code),
                r.reason.clone(),
            ]
        })
        .collect()
}

/// Headline of the section "Script commands" with the totals.
pub fn commands_title(commands: &ScriptCommandsInfo) -> String {
    let (done, rejected) = commands.actions.iter().fold((0, 0), |(d, r), a| {
        (d + a.accepted + a.partial, r + a.rejected)
    });
    if done + rejected == 0 {
        "Script commands (none)".into()
    } else {
        format!("Script commands ({done} accepted or partial, {rejected} rejected)")
    }
}

/// Game result: one row per passed slot (1 lost, 2 won, anything else as "Result n"). `script_state` returns only
/// the decided slots (result 1 or 2); playing (0) and unoccupied slots (3) are missing.
pub fn result_rows(results: &[(u8, i32)]) -> Vec<[String; 2]> {
    results
        .iter()
        .map(|&(slot, result)| {
            [
                format!("Slot {slot}"),
                match result {
                    1 => "lost".into(),
                    2 => "won".into(),
                    other => format!("Result {other}"),
                },
            ]
        })
        .collect()
}

/// Number of rows of the event table in the window (the last 50 messages, newest first).
pub const EVENT_ROWS: usize = 50;

/// Response of `script_events` for the viewer: the part of the `SimHost` ([`SimHost::control_script_events`]) with
/// the scripts' camera from the camera state of the session (it lives in `ScriptPresentation`, not in the
/// `SimHost`). Reads only.
pub fn script_events(
    sim: &SimHost,
    camera: &ScriptCamera,
    steps: Option<u32>,
) -> Result<ScriptEventsResult, String> {
    let mut result = sim.control_script_events(steps)?;
    result.camera = serde_json::to_value(camera).unwrap_or(serde_json::Value::Null);
    Ok(result)
}

/// Text for a text ID from the text table (`texts`); if it is missing, the number.
pub fn text_of(id: i32, texts: &dyn Fn(i32) -> Option<String>) -> String {
    match texts(id) {
        Some(text) => format!("\"{text}\""),
        None => format!("Text {id} (not in the table)"),
    }
}

/// Lines of the section "Presentation": running line with its text, the tips, info windows, portraits, highlight,
/// input lock, cinemascope, fade ("faded out"), light/weather/music, the selection mirror with the last button and
/// the script camera. The presentation state comes from `events.presentation`, line and tips from the speech
/// state of `state`.
pub fn presentation_lines(
    state: &ScriptStateResult,
    events: &ScriptEventsResult,
    texts: &dyn Fn(i32) -> Option<String>,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(match state.speech.line {
        Some(l) => format!(
            "Line on slot {}, {} ms remaining: {}",
            l.slot,
            l.remaining_ms,
            text_of(l.text, texts)
        ),
        None => "Line: none".into(),
    });
    for (i, tip) in state.speech.tips.iter().enumerate() {
        lines.push(if tip.remaining_ms > 0 {
            format!(
                "Tip {i}, {} ms remaining: {}",
                tip.remaining_ms,
                text_of(tip.text, texts)
            )
        } else {
            format!("Tip {i}: none")
        });
    }
    let Ok(memory) = serde_json::from_value::<PresentationMemory>(events.presentation.clone())
    else {
        lines.push("Presentation state: not readable".into());
        return lines;
    };
    if memory.info_frames.is_empty() {
        lines.push("Info windows: none".into());
    }
    for (text, frame) in &memory.info_frames {
        lines.push(match frame {
            InfoFrameMemory::Window { x, y, style } => format!(
                "Info window at ({x}, {y}), frame {style}: {}",
                text_of(*text, texts)
            ),
            InfoFrameMemory::Speech {
                slot,
                portrait,
                frame,
            } => format!(
                "Speech window on slot {slot}, portrait {portrait}, frame {frame}: {}",
                text_of(*text, texts)
            ),
        });
    }
    let portraits: Vec<String> = memory
        .portraits
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            p.map(|p| format!("Slot {i}: portrait {}, frame {}", p.portrait, p.frame))
        })
        .collect();
    lines.push(if portraits.is_empty() {
        "Portraits: none".into()
    } else {
        format!("Portraits: {}", portraits.join("; "))
    });
    lines.push(match memory.highlight {
        Some(id) => format!("Highlight: object {}", id.0),
        None => "Highlight: none".into(),
    });
    let on = |flag: bool| if flag { "on" } else { "off" };
    lines.push(format!("Input lock: {}", on(state.ui.ui_locked)));
    lines.push(format!("Cinemascope: {}", on(state.ui.cinemascope)));
    lines.push(if memory.faded_out {
        "Screen: faded out".into()
    } else {
        "Screen: visible".into()
    });
    lines.push(format!(
        "Light: {}, weather: {}, music: {}",
        memory.light.as_deref().unwrap_or("Level"),
        memory.weather,
        memory
            .music
            .map_or_else(|| "none".to_string(), |id| id.to_string())
    ));
    let selection: Vec<String> = state.ui.selection.iter().map(u32::to_string).collect();
    lines.push(format!(
        "Selection mirror: {}, last button: {}",
        if selection.is_empty() {
            "empty".to_string()
        } else {
            selection.join(", ")
        },
        state.ui.last_button
    ));
    lines.push(
        match serde_json::from_value::<ScriptCamera>(events.camera.clone()) {
            Ok(camera) => format!(
                "Script camera: {}, {} pending moves, binding: {}",
                if camera.running() { "running" } else { "idle" },
                camera.moves.len(),
                camera
                    .bind
                    .map_or_else(|| "none".to_string(), |id| format!("object {}", id.0))
            ),
            Err(_) => "Script camera: unknown".into(),
        },
    );
    lines
}

/// A presentation event as text (text IDs with their text from the table).
pub fn event_text(event: &Presentation, texts: &dyn Fn(i32) -> Option<String>) -> String {
    let on = |flag: &bool| if *flag { "on" } else { "off" };
    let text = |id: &i32| text_of(*id, texts);
    match event {
        Presentation::CameraCenter { tile_x, tile_z } => {
            format!("Camera to tile ({tile_x}, {tile_z})")
        }
        Presentation::CameraToObject { id } => format!("Camera to object {}", id.0),
        Presentation::CameraCut { camera } => format!("Camera: cut to \"{camera}\""),
        Presentation::CameraMove { camera, ms } => {
            format!("Camera: move to \"{camera}\" in {ms} ms")
        }
        Presentation::CameraBackup => "Camera: save".into(),
        Presentation::CameraRestoreBackup => "Camera: restore the saved one".into(),
        Presentation::CameraMoveToBackup { ms } => {
            format!("Camera: move to the saved one in {ms} ms")
        }
        Presentation::CameraToGame { ms } => {
            format!("Camera: back to the game camera in {ms} ms")
        }
        Presentation::CameraBind { id: Some(id) } => format!("Camera: bind to object {}", id.0),
        Presentation::CameraBind { id: None } => "Camera: release the binding".into(),
        Presentation::CameraRotate { degrees, ms } => {
            format!("Camera: rotate by {degrees} degrees in {ms} ms")
        }
        Presentation::Fade { into, ms } => {
            format!("Fade {} over {ms} ms", if *into { "in" } else { "out" })
        }
        Presentation::Cinemascope { on: flag } => format!("Cinemascope {}", on(flag)),
        Presentation::UiLock { on: flag } => format!("Input lock {}", on(flag)),
        Presentation::Portrait {
            slot,
            portrait,
            frame,
        } => format!("Portrait {portrait}, frame {frame} on slot {slot}"),
        Presentation::PortraitHide { slot } => format!("Hide portrait on slot {slot}"),
        Presentation::Highlight { id: Some(id) } => format!("Highlight on object {}", id.0),
        Presentation::Highlight { id: None } => "Clear highlight".into(),
        Presentation::Speech { text: id, slot, ms } => {
            format!("Line on slot {slot} for {ms} ms: {}", text(id))
        }
        Presentation::SpeechEnd { text: id, slot } => {
            format!("Line on slot {slot} ended: {}", text(id))
        }
        Presentation::Tip { text: id, slot, ms } => {
            format!("Tip {slot} for {ms} ms: {}", text(id))
        }
        Presentation::TipEnd { text: id, slot } => format!("Tip {slot} ended: {}", text(id)),
        Presentation::InfoFrame {
            text: id,
            x,
            y,
            style,
        } => format!("Info window at ({x}, {y}), frame {style}: {}", text(id)),
        Presentation::InfoSpeechFrame {
            text: id,
            slot,
            portrait,
            frame,
        } => format!(
            "Speech window on slot {slot}, portrait {portrait}, frame {frame}: {}",
            text(id)
        ),
        Presentation::InfoFrameClose { text: id } => {
            format!("Close info window: {}", text(id))
        }
        Presentation::ScreenText { text: id, seconds } => {
            format!("Screen text for {seconds} s: {}", text(id))
        }
        Presentation::MinimapPing {
            player,
            tile_x,
            tile_z,
        } => format!("Minimap ping for player {player} at ({tile_x}, {tile_z})"),
        Presentation::Music { id } => format!("Music {id}"),
        Presentation::AttackMusic { id } => format!("Combat music {id}"),
        Presentation::AttackThreshold { points } => format!("Combat music from {points} points"),
        Presentation::Sound2d { id } => format!("2D sound {id}"),
        Presentation::Light { preset } => format!("Light \"{preset}\""),
        Presentation::LightRestoreLevel => "Object light of the level".into(),
        Presentation::Weather { kind } => format!("Weather {kind}"),
        Presentation::DistanceFog { on: flag } => format!("Distance fog {}", on(flag)),
        Presentation::FogValues {
            color,
            start,
            end,
            ms,
            swap_red_blue,
        } => format!(
            "Fog values: color {color:#010X}, start {start}, end {end}, {ms} ms{}",
            if *swap_red_blue {
                " (red and blue swapped)"
            } else {
                ""
            }
        ),
        Presentation::FogShowAll { on: flag } => format!("Fog view mode {}", on(flag)),
        Presentation::QuickSave => "Quicksave".into(),
        Presentation::ProfileVar { name, value } => format!("Profile variable {name} = {value}"),
    }
}

/// Columns of the event table.
pub const EVENT_COLUMNS: [&str; 2] = ["Step", "Event"];

/// The presentation events, newest first, at most [`EVENT_ROWS`]: step, event as text (unreadable as
/// JSON).
pub fn event_rows(
    events: &ScriptEventsResult,
    texts: &dyn Fn(i32) -> Option<String>,
) -> Vec<[String; 2]> {
    events
        .events
        .iter()
        .rev()
        .take(EVENT_ROWS)
        .map(|e| {
            [
                e.step.to_string(),
                match serde_json::from_value::<Presentation>(e.event.clone()) {
                    Ok(event) => event_text(&event, texts),
                    Err(_) => e.event.to_string(),
                },
            ]
        })
        .collect()
}

/// Result of the active slot at the game end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameEnd {
    Victory,
    Defeat,
}

impl GameEnd {
    /// Display in the map view.
    pub fn text(self) -> &'static str {
        match self {
            Self::Victory => "Victory",
            Self::Defeat => "Defeat",
        }
    }
}

/// Has the active slot won or lost (`SimEvent::PlayerResult` sets `Player::result`)? Then the viewer shows the
/// display and sends no more steps to the simulation; a restart, load or `sim_setup` resets the result and the match
/// runs again.
pub fn game_end(sim: &SimHost) -> Option<GameEnd> {
    match sim
        .state
        .players
        .get(usize::from(sim.active_player))?
        .result
    {
        VICTORY => Some(GameEnd::Victory),
        DEFEAT => Some(GameEnd::Defeat),
        _ => None,
    }
}

/// Does the viewer send steps to the simulation? No, as soon as the active slot is decided ([`game_end`]); without
/// a simulation (model browser, map without rules) there is nothing to pause.
pub fn viewer_steps(sim: Option<&SimHost>) -> bool {
    sim.is_none_or(|s| game_end(s).is_none())
}

/// Height of a cinemascope bar at a window height of `height`: `BARSIZE` 170 of 1024 virtual rows
/// (re/campaign-ui 4.2).
pub fn cinemascope_bar_height(height: f32) -> f32 {
    height * 170.0 / 1024.0
}

/// Does the viewer draw the egui bars of the cinemascope? Only without `--hud` (deviation from the original): with
/// the game UI the original area draws the bars (only in the campaign, O-350).
pub fn egui_bars(cinemascope: bool, hud: bool) -> bool {
    cinemascope && !hud
}

/// Draws the cinemascope bars (top and bottom, black) and, at the game end, the display "Victory"/"Defeat" in the
/// center. With the background painter of egui: above the map, below the windows.
pub fn paint_presentation(ctx: &egui::Context, cinemascope: bool, end: Option<GameEnd>) {
    if cinemascope {
        let screen = ctx.content_rect();
        let bar = cinemascope_bar_height(screen.height());
        let painter = ctx.layer_painter(egui::LayerId::background());
        for rect in [
            egui::Rect::from_min_size(screen.min, egui::vec2(screen.width(), bar)),
            egui::Rect::from_min_size(
                egui::pos2(screen.min.x, screen.max.y - bar),
                egui::vec2(screen.width(), bar),
            ),
        ] {
            painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
        }
    }
    if let Some(end) = end {
        let color = match end {
            GameEnd::Victory => egui::Color32::from_rgb(120, 220, 120),
            GameEnd::Defeat => egui::Color32::from_rgb(230, 90, 90),
        };
        egui::Area::new(egui::Id::new("script_game_end"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(egui::RichText::new(end.text()).size(48.0).color(color));
                });
            });
    }
}

fn grid<const N: usize>(ui: &mut egui::Ui, id: &str, columns: [&str; N], rows: &[[String; N]]) {
    egui::Grid::new(id).striped(true).show(ui, |ui| {
        for c in columns {
            ui.strong(c);
        }
        ui.end_row();
        for row in rows {
            for cell in row {
                ui.label(cell);
            }
            ui.end_row();
        }
    });
}

/// "Script" window (key J): only reads `state` and `events`; a click on a block row chooses the block whose
/// conditions are shown (again: clear the choice). `texts` is the text table for the lines, tips and info windows of
/// the presentation; without `events` the sections "Presentation" and "Events" are omitted.
pub fn build_script(
    ctx: &egui::Context,
    window: &mut ScriptWindow,
    state: &ScriptStateResult,
    events: Option<&ScriptEventsResult>,
    texts: &dyn Fn(i32) -> Option<String>,
) {
    let mut open = window.open;
    egui::Window::new("Script")
        .open(&mut open)
        .default_pos([490.0, 60.0])
        .default_width(640.0)
        .default_height(560.0)
        .vscroll(true)
        .show(ctx, |ui| {
            ui.label(format!(
                "Script {} | Dialog {}",
                if state.enabled { "on" } else { "off" },
                if state.dialog_running {
                    "running (AI and level script paused)"
                } else {
                    "not running"
                }
            ));
            if state.containers.is_empty() {
                ui.label(
                    "no container (script off or map without a level script; switch on in the \"Match\" window or with sim_setup script_enabled)",
                );
            }
            for c in &state.containers {
                egui::CollapsingHeader::new(format!(
                    "{}: {} | {} | {} blocks",
                    container_name(c.id),
                    c.path,
                    countdown_text(c.counter),
                    c.blocks.len()
                ))
                .id_salt(("script_container", c.id))
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new(("script_blocks", c.id))
                        .striped(true)
                        .show(ui, |ui| {
                            for col in BLOCK_COLUMNS {
                                ui.strong(col);
                            }
                            ui.end_row();
                            for (b, row) in c.blocks.iter().zip(block_rows(c)) {
                                let this = ScriptBlockRef {
                                    container: c.id,
                                    index: b.index,
                                };
                                let chosen = window.selected == Some(this);
                                if ui.selectable_label(chosen, &row[0]).clicked() {
                                    window.selected = (!chosen).then_some(this);
                                }
                                for cell in &row[1..] {
                                    ui.label(cell);
                                }
                                ui.end_row();
                            }
                        });
                    let rows = namespace_rows(c);
                    egui::CollapsingHeader::new(format!("Namespaces ({})", rows.len()))
                        .id_salt(("script_namespaces", c.id))
                        .show(ui, |ui| {
                            grid(
                                ui,
                                &format!("script_names_{}", c.id),
                                ["Kind", "Name", "Value"],
                                &rows,
                            );
                        });
                });
            }
            if let (Some(r), Some(conditions)) = (window.selected, &state.conditions) {
                egui::CollapsingHeader::new(format!(
                    "Conditions {} Block {} ({})",
                    container_name(r.container),
                    r.index,
                    conditions.len()
                ))
                .id_salt("script_conditions")
                .default_open(true)
                .show(ui, |ui| {
                    if conditions.is_empty() {
                        ui.label("no conditions (always true)");
                    }
                    grid(
                        ui,
                        "script_conditions_table",
                        CONDITION_COLUMNS,
                        &condition_rows(conditions),
                    );
                });
            }
            if let Some(events) = events {
                egui::CollapsingHeader::new("Presentation")
                    .default_open(true)
                    .show(ui, |ui| {
                        for line in presentation_lines(state, events, texts) {
                            ui.label(line);
                        }
                    });
            }
            egui::CollapsingHeader::new("Mission objectives")
                .default_open(true)
                .show(ui, |ui| {
                    let rows = quest_rows(&state.main_quests, &state.sub_quests);
                    if rows.is_empty() {
                        ui.label("no mission objectives set");
                    } else {
                        grid(ui, "script_mission_objectives", QUEST_COLUMNS, &rows);
                    }
                    for line in mission_lines(state) {
                        ui.label(line);
                    }
                });
            egui::CollapsingHeader::new("Speech state")
                .default_open(true)
                .show(ui, |ui| {
                    for line in speech_lines(&state.speech) {
                        ui.label(line);
                    }
                });
            egui::CollapsingHeader::new(format!("Object names ({})", state.names.len())).show(
                ui,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Search");
                        ui.text_edit_singleline(&mut window.names_search);
                    });
                    let rows: Vec<[String; 2]> = state
                        .names
                        .iter()
                        .map(|(k, v)| [k.clone(), v.clone()])
                        .collect();
                    grid(ui, "script_object_names", ["Name", "Value"], &rows);
                },
            );
            egui::CollapsingHeader::new(commands_title(&state.commands))
                .id_salt("script_commands")
                .default_open(true)
                .show(ui, |ui| {
                    let rows = command_rows(&state.commands);
                    if !rows.is_empty() {
                        grid(ui, "script_commands_per_action", COMMAND_COLUMNS, &rows);
                    }
                    let rows = rejection_rows(&state.commands);
                    if !rows.is_empty() {
                        ui.label(format!("The last {} rejections:", rows.len()));
                        grid(ui, "script_rejections", REJECTION_COLUMNS, &rows);
                    }
                });
            egui::CollapsingHeader::new("Game result")
                .default_open(true)
                .show(ui, |ui| {
                    let rows = result_rows(&state.results);
                    if rows.is_empty() {
                        ui.label("no slot decided yet");
                    } else {
                        grid(ui, "script_game_result", ["Slot", "Result"], &rows);
                    }
                });
            egui::CollapsingHeader::new(format!("Block log ({})", state.log.len()))
                .default_open(true)
                .show(ui, |ui| {
                    ui.add(
                        egui::DragValue::new(&mut window.log_steps)
                            .range(1..=MAX_SCRIPT_LOG_STEPS)
                            .prefix("last ")
                            .suffix(" steps"),
                    );
                    grid(ui, "script_log", ["Step", "Message"], &log_rows(&state.log));
                });
            if let Some(events) = events {
                egui::CollapsingHeader::new(format!("Events ({})", events.events.len()))
                    .default_open(true)
                    .show(ui, |ui| {
                        let rows = event_rows(events, texts);
                        if rows.is_empty() {
                            ui.label("no presentation events in the last steps");
                        } else {
                            grid(ui, "script_events", EVENT_COLUMNS, &rows);
                        }
                    });
            }
        });
    window.open = open;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testworld_viewer::script_host;
    use nml_client::game::testworld::{COLONIST, abl, spawn};
    use nml_control::{
        ScriptActionCount, ScriptBlockInfo, ScriptCommandsInfo, ScriptLineInfo, ScriptQuestInfo,
        ScriptRejection, ScriptTipInfo,
    };
    use serde_json::json;

    fn container() -> ScriptContainerInfo {
        ScriptContainerInfo {
            id: 1,
            path: "synthetisch.abl".into(),
            counter: 12_000,
            switches: vec![("on".into(), 1)],
            timers: vec![("clock".into(), 2900)],
            variables: vec![("v".into(), -3)],
            random_switches: vec![("throw".into(), 7)],
            blocks: vec![
                ScriptBlockInfo {
                    index: 0,
                    name: "start]".into(),
                    state: "bereit".into(),
                    wait: None,
                    cur_action: 0,
                },
                ScriptBlockInfo {
                    index: 4,
                    name: String::new(),
                    state: "wartet".into(),
                    wait: Some("Action 2 (w40;1000): 900 ms remaining".into()),
                    cur_action: 2,
                },
            ],
        }
    }

    #[test]
    fn script_window_rows_show_blocks_conditions_and_log() {
        let c = container();
        assert_eq!(
            block_rows(&c),
            [
                ["0", "start]", "bereit", "0", "–"].map(String::from),
                [
                    "4",
                    "",
                    "wartet",
                    "2",
                    "Action 2 (w40;1000): 900 ms remaining"
                ]
                .map(String::from)
            ]
        );
        assert_eq!(
            namespace_rows(&c),
            [
                ["Switch", "on", "1"].map(String::from),
                ["Random switch", "throw", "7"].map(String::from),
                ["Timer", "clock", "2900"].map(String::from),
                ["Variable", "v", "-3"].map(String::from),
            ]
        );
        assert_eq!(
            (countdown_text(12_000), countdown_text(-1)),
            ("Countdown 12000 ms".into(), "Countdown off".into())
        );
        assert_eq!(
            (container_name(1), container_name(-1)),
            ("Level 1".into(), "Dialog".into())
        );
        let condition = |number, or, params: &[&str], value| ScriptConditionInfo {
            number,
            or,
            params: params.iter().map(|p| p.to_string()).collect(),
            value,
        };
        assert_eq!(
            condition_rows(&[
                condition(19, true, &["5"], Some(true)),
                condition(2, false, &["0", "0", "100"], None),
                condition(15, true, &[], Some(false)),
            ]),
            [
                ["–", "19 Text ended", "5", "true"].map(String::from),
                [
                    "and",
                    "2 Resource at least",
                    "0;0;100",
                    "not yet implemented"
                ]
                .map(String::from),
                ["or", "15 (no case)", "", "false"].map(String::from),
            ]
        );
        let speech = ScriptSpeechInfo {
            line: Some(ScriptLineInfo {
                text: 4711,
                remaining_ms: 1200,
                slot: 4,
            }),
            tips: vec![
                ScriptTipInfo {
                    remaining_ms: 0,
                    text: 0,
                },
                ScriptTipInfo {
                    remaining_ms: 300,
                    text: 12,
                },
            ],
            events: vec![5, 5],
            info_history: Vec::new(),
        };
        assert_eq!(
            speech_lines(&speech),
            [
                "Dialog line: text 4711 on slot 4, 1200 ms remaining",
                "Tip 0: text 0, 0 ms remaining",
                "Tip 1: text 12, 300 ms remaining",
                "Text events: 5, 5",
                "Info history: –"
            ]
        );
        // Log: newest first, every message kind as text, unknown as JSON.
        let entry = |step, trace| ScriptLogEntry { step, trace };
        let log = [
            entry(1, json!({"Fired": {"container": 1, "block": 3}})),
            entry(
                1,
                json!({"Suspended": {"container": 1, "block": 3, "action": 2}}),
            ),
            entry(
                2,
                json!({"Pending": {"container": -1, "block": 0, "action": 151}}),
            ),
            entry(
                2,
                json!({"ConditionPending": {"container": 1, "block": 1, "condition": 2}}),
            ),
            entry(3, json!({"DialogStarted": {"path": "data/x.abl"}})),
            entry(3, json!("DialogEnded")),
            entry(4, json!({"Deleted": {"container": 1, "block": 3}})),
            entry(
                4,
                json!({"Guarded": {"container": 1, "block": 1, "number": 40, "why": "parameter missing"}}),
            ),
            entry(5, json!({"Limit": {"what": "text events"}})),
            entry(5, json!({"New": 1})),
        ];
        let rows = log_rows(&log);
        let texts: Vec<&str> = rows.iter().map(|r| r[1].as_str()).collect();
        assert_eq!(
            texts,
            [
                "{\"New\":1}",
                "Limit reached: text events",
                "Level 1 Block 1: number 40 intercepted: parameter missing",
                "Level 1 Block 3: deleted",
                "Dialog ended",
                "Dialog started: data/x.abl",
                "Level 1 Block 1: condition 2 not yet implemented",
                "Dialog Block 0: action 151 has no effect yet",
                "Level 1 Block 3: waits at action 2",
                "Level 1 Block 3: fired",
            ]
        );
        assert_eq!(rows[0][0], "5");
    }

    #[test]
    fn mission_rows_name_the_status_and_skip_empty_goals() {
        let quest = |status, text| ScriptQuestInfo { status, text };
        let mut main = vec![quest(0, 0); 10];
        main[0] = quest(3, 105);
        main[2] = quest(6, 106);
        main[3] = quest(10, 0);
        let mut sub = vec![quest(0, 0); 10];
        sub[9] = quest(1, 0);
        sub[4] = quest(0, 77);
        // Status 0 (empty) shows the table only if the entry has a text.
        assert_eq!(quest_status_text(0), "empty");
        assert_eq!(
            [1, 3, 6, 10, 4].map(quest_status_text),
            ["present", "set", "completed", "failed", "Status 4"]
        );
        assert_eq!(
            quest_rows(&main, &sub),
            [
                ["Main objective", "1", "set", "105"].map(String::from),
                ["Main objective", "3", "completed", "106"].map(String::from),
                ["Main objective", "4", "failed", "–"].map(String::from),
                ["Sub-objective", "5", "empty", "77"].map(String::from),
                ["Sub-objective", "10", "present", "–"].map(String::from),
            ]
        );
        assert!(quest_rows(&[], &[]).is_empty());
        let mut state = empty_state();
        assert_eq!(mission_lines(&state), ["Countdown display: none"]);
        state.countdown_container = 1;
        state.countdown_s = 89;
        state.task_status = vec![(2, 1), (5, 0)];
        state.area_overrides = vec![("meadow".into(), [2, 1, 9, 8])];
        assert_eq!(
            mission_lines(&state),
            [
                "Countdown display: Level 1, 89 s",
                "Task status: 2 = 1, 5 = 0",
                "Changed areas: meadow = [2, 1, 9, 8]",
            ]
        );
    }

    fn empty_state() -> ScriptStateResult {
        ScriptStateResult {
            enabled: true,
            dialog_running: false,
            containers: Vec::new(),
            conditions: None,
            speech: ScriptSpeechInfo {
                line: None,
                tips: Vec::new(),
                events: Vec::new(),
                info_history: Vec::new(),
            },
            names: Vec::new(),
            log: Vec::new(),
            main_quests: Vec::new(),
            sub_quests: Vec::new(),
            countdown_container: 0,
            countdown_s: 0,
            task_status: Vec::new(),
            area_overrides: Vec::new(),
            commands: ScriptCommandsInfo::default(),
            results: Vec::new(),
            ui: nml_control::ScriptUiInfo::default(),
        }
    }

    #[test]
    fn script_window_builds_from_the_live_state_and_only_reads() {
        let (mut host, _) = script_host(&abl(&[
            (&["|19;5", "&2;0;0;100"], &["n12;text"]),
            (&[], &["n12;on", "w40;1000"]),
        ]));
        host.step_once();
        let mut window = ScriptWindow {
            open: true,
            selected: Some(ScriptBlockRef {
                container: nml_sim::script::LEVEL_CONTAINER_ID,
                index: 0,
            }),
            ..ScriptWindow::default()
        };
        let before = (host.checksum(), nml_sim::save::checksum(&host.state));
        let (block, names, log) = window.request();
        let state = host.control_script_state(block, names, log).unwrap();
        assert_eq!(state.conditions.as_ref().map(Vec::len), Some(2));
        let rows = block_rows(&state.containers[0]);
        assert_eq!(rows[1][2], "wartet");
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            build_script(ui.ctx(), &mut window, &state, None, &|_| None)
        });
        assert!(!output.shapes.is_empty());
        output.drop_without_applying_deltas();
        assert!(window.open);
        assert_eq!(
            (host.checksum(), nml_sim::save::checksum(&host.state)),
            before
        );
        // Defaults: closed, no choice, 50 steps of log.
        let fresh = ScriptWindow::default();
        assert_eq!(fresh.request(), (None, Some(""), Some(50)));
    }

    // --- Script commands and game result -----------------------------------------------------------

    #[test]
    fn command_and_result_rows_name_actions_blocks_and_reasons() {
        let commands = ScriptCommandsInfo {
            actions: vec![
                ScriptActionCount {
                    action: 191,
                    accepted: 5,
                    partial: 0,
                    rejected: 2,
                },
                ScriptActionCount {
                    action: 153,
                    accepted: 1,
                    partial: 3,
                    rejected: 0,
                },
                // Number without a catalog row: only the number.
                ScriptActionCount {
                    action: 9999,
                    accepted: 0,
                    partial: 0,
                    rejected: 1,
                },
            ],
            rejections: vec![
                ScriptRejection {
                    step: 7,
                    container: 1,
                    block: 4,
                    action: 191,
                    code: 0x3C,
                    reason: "Object 65537 missing".into(),
                },
                ScriptRejection {
                    step: 9,
                    container: -1,
                    block: 0,
                    action: 9999,
                    code: 0x04,
                    reason: "Target invalid".into(),
                },
            ],
        };
        assert_eq!(
            command_rows(&commands),
            [
                ["191 Halt unit", "5", "0", "2"].map(String::from),
                ["153 Move unit", "1", "3", "0"].map(String::from),
                ["9999", "0", "0", "1"].map(String::from),
            ]
        );
        // Newest rejection first.
        assert_eq!(
            rejection_rows(&commands),
            [
                ["9", "Dialog Block 0", "9999", "0x04", "Target invalid"].map(String::from),
                [
                    "7",
                    "Level 1 Block 4",
                    "191",
                    "0x3C",
                    "Object 65537 missing"
                ]
                .map(String::from),
            ]
        );
        assert_eq!(
            commands_title(&commands),
            "Script commands (9 accepted or partial, 3 rejected)"
        );
        assert_eq!(
            commands_title(&ScriptCommandsInfo::default()),
            "Script commands (none)"
        );
        assert_eq!(
            result_rows(&[(0, 2), (3, 1), (5, 7)]),
            [
                ["Slot 0", "won"].map(String::from),
                ["Slot 3", "lost"].map(String::from),
                ["Slot 5", "Result 7"].map(String::from),
            ]
        );
        assert!(result_rows(&[]).is_empty());
    }

    #[test]
    fn script_window_counts_rejections() {
        // A repeatable block sends a halt to two units in every step, of which the first is stale: one rejection and
        // one acceptance per step; the window shows the last 20 reasons.
        let (mut host, _) = script_host(&abl(&[(&[], &["n10", "n191;enemy"])]));
        let live = spawn(&mut host, COLONIST, 3, (10, 10));
        let gone = spawn(&mut host, COLONIST, 3, (12, 10));
        {
            let rules = host.rules.clone();
            let mut sim = nml_sim::sim::Sim::new(&mut host.state, &rules);
            nml_sim::world::destroy_object(&mut sim, gone);
        }
        host.state
            .names
            .set("enemy", format!("{};{}", gone.as_i32(), live.as_i32()));
        for _ in 0..25 {
            host.step_once();
        }
        let before = (host.checksum(), nml_sim::save::checksum(&host.state));
        let mut window = ScriptWindow {
            open: true,
            ..ScriptWindow::default()
        };
        let (block, names, log) = window.request();
        let state = host.control_script_state(block, names, log).unwrap();
        assert_eq!(
            state.commands.actions,
            [ScriptActionCount {
                action: 191,
                accepted: 25,
                partial: 0,
                rejected: 25
            }]
        );
        // Only the last 20 rejections, the oldest first; the window shows them newest first.
        let steps: Vec<u64> = state.commands.rejections.iter().map(|r| r.step).collect();
        assert_eq!(steps, (6..=25).collect::<Vec<u64>>());
        let rows = rejection_rows(&state.commands);
        assert_eq!(rows.len(), 20);
        assert_eq!(rows[0][0], "25");
        assert_eq!(rows[19][0], "6");
        assert!(
            rows.iter()
                .all(|r| r[2] == "191" && r[3] == "0x3C" && !r[4].is_empty())
        );
        assert_eq!(
            commands_title(&state.commands),
            "Script commands (25 accepted or partial, 25 rejected)"
        );
        // The window builds with counters and only reads.
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            build_script(ui.ctx(), &mut window, &state, None, &|_| None)
        });
        assert!(!output.shapes.is_empty());
        output.drop_without_applying_deltas();
        assert_eq!(
            (host.checksum(), nml_sim::save::checksum(&host.state)),
            before
        );
    }

    // --- Presentation --------------------------------------------------------------------------

    /// State and events of a cutscene: running line, a tip, two info windows, a portrait, highlight, lock, cinemascope,
    /// fade, light/weather/music, selection mirror and a running camera move.
    fn scene() -> (ScriptStateResult, ScriptEventsResult) {
        use nml_control::ScriptUiInfo;
        use nml_sim::id::ObjectId;
        use nml_sim::script::PortraitMemory;
        let mut state = empty_state();
        state.speech.line = Some(ScriptLineInfo {
            text: 4711,
            remaining_ms: 1200,
            slot: 2,
        });
        state.speech.tips = vec![
            ScriptTipInfo {
                remaining_ms: 0,
                text: 0,
            },
            ScriptTipInfo {
                remaining_ms: 300,
                text: 12,
            },
        ];
        state.ui = ScriptUiInfo {
            ui_locked: true,
            cinemascope: true,
            selection: vec![65_537],
            last_button: 511,
        };
        let mut memory = PresentationMemory {
            light: Some("night".into()),
            weather: 1,
            music: Some(5),
            faded_out: true,
            highlight: Some(ObjectId(65_538)),
            ..PresentationMemory::default()
        };
        memory.portraits[1] = Some(PortraitMemory {
            portrait: 11,
            frame: 22,
        });
        memory.info_frames.insert(
            300,
            InfoFrameMemory::Window {
                x: 10,
                y: 20,
                style: 1,
            },
        );
        memory.info_frames.insert(
            301,
            InfoFrameMemory::Speech {
                slot: 5,
                portrait: 3,
                frame: 4,
            },
        );
        let camera = ScriptCamera {
            rest_ms: 500.0,
            bind: Some(ObjectId(7)),
            ..ScriptCamera::default()
        };
        let entry = |step, event: Presentation| nml_control::ScriptEventEntry {
            step,
            event: serde_json::to_value(event).unwrap(),
        };
        let events = ScriptEventsResult {
            events: vec![
                entry(3, Presentation::Cinemascope { on: true }),
                entry(
                    3,
                    Presentation::Speech {
                        text: 4711,
                        slot: 2,
                        ms: 900,
                    },
                ),
                entry(
                    5,
                    Presentation::Fade {
                        into: false,
                        ms: 500,
                    },
                ),
            ],
            presentation: serde_json::to_value(&memory).unwrap(),
            ui: state.ui.clone(),
            profile_vars: Vec::new(),
            camera: serde_json::to_value(&camera).unwrap(),
        };
        (state, events)
    }

    fn texts(id: i32) -> Option<String> {
        match id {
            4711 => Some("Hello, hero!".into()),
            12 => Some("Tip".into()),
            300 => Some("Info text".into()),
            _ => None,
        }
    }

    #[test]
    fn script_window_shows_the_running_line_tips_and_frames() {
        let (state, events) = scene();
        assert_eq!(
            presentation_lines(&state, &events, &texts),
            [
                "Line on slot 2, 1200 ms remaining: \"Hello, hero!\"",
                "Tip 0: none",
                "Tip 1, 300 ms remaining: \"Tip\"",
                "Info window at (10, 20), frame 1: \"Info text\"",
                "Speech window on slot 5, portrait 3, frame 4: Text 301 (not in the table)",
                "Portraits: Slot 1: portrait 11, frame 22",
                "Highlight: object 65538",
                "Input lock: on",
                "Cinemascope: on",
                "Screen: faded out",
                "Light: night, weather: 1, music: 5",
                "Selection mirror: 65537, last button: 511",
                "Script camera: running, 0 pending moves, binding: object 7",
            ]
        );
        // Events: newest first, with text from the table; unreadable as JSON.
        let mut shown = events.clone();
        shown.events.push(nml_control::ScriptEventEntry {
            step: 6,
            event: serde_json::json!({"Unknown": 1}),
        });
        assert_eq!(
            event_rows(&shown, &texts),
            [
                ["6", r#"{"Unknown":1}"#].map(String::from),
                ["5", "Fade out over 500 ms"].map(String::from),
                ["3", "Line on slot 2 for 900 ms: \"Hello, hero!\""].map(String::from),
                ["3", "Cinemascope on"].map(String::from),
            ]
        );
        // At most the last 50 messages.
        let many = ScriptEventsResult {
            events: (1..=60)
                .map(|step| nml_control::ScriptEventEntry {
                    step,
                    event: serde_json::to_value(Presentation::QuickSave).unwrap(),
                })
                .collect(),
            ..events.clone()
        };
        let rows = event_rows(&many, &texts);
        assert_eq!(
            (rows.len(), rows[0][0].as_str(), rows[49][0].as_str()),
            (50, "60", "11")
        );
        // Without a readable state and without a camera: says so instead of being missing.
        let broken = ScriptEventsResult {
            presentation: serde_json::json!({"weather": "x"}),
            ..events.clone()
        };
        assert_eq!(
            presentation_lines(&state, &broken, &texts).last().unwrap(),
            "Presentation state: not readable"
        );
        let no_camera = ScriptEventsResult {
            camera: serde_json::Value::Null,
            ..events.clone()
        };
        assert_eq!(
            presentation_lines(&state, &no_camera, &texts)
                .last()
                .unwrap(),
            "Script camera: unknown"
        );
        // The window builds with both sections.
        let mut window = ScriptWindow {
            open: true,
            ..ScriptWindow::default()
        };
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            build_script(ui.ctx(), &mut window, &state, Some(&events), &texts)
        });
        assert!(!output.shapes.is_empty());
        output.drop_without_applying_deltas();
    }

    #[test]
    fn script_events_of_the_viewer_carry_the_camera_and_leave_the_checksum() {
        let (mut host, _) = script_host(&abl(&[(&[], &["n147", "n209;1"])]));
        host.step_once();
        host.step_once();
        let before = (host.checksum(), nml_sim::save::checksum(&host.state));
        let camera = ScriptCamera {
            rest_ms: 250.0,
            ..ScriptCamera::default()
        };
        let result = script_events(&host, &camera, None).unwrap();
        assert!(
            result
                .events
                .iter()
                .any(|e| e.event == serde_json::json!({"Cinemascope": {"on": true}}))
        );
        assert_eq!(
            serde_json::from_value::<ScriptCamera>(result.camera).unwrap(),
            camera
        );
        assert!(script_events(&host, &camera, Some(1001)).is_err());
        assert_eq!(
            (host.checksum(), nml_sim::save::checksum(&host.state)),
            before
        );
    }

    // QUIRK O-350: kept
    #[test]
    fn viewer_without_hud_keeps_egui_bars() {
        // Deviation from the original: without `--hud` the egui bars stay in cinemascope; with `--hud` the original
        // area draws (only in the campaign, O-350), egui nothing.
        assert!(egui_bars(true, false));
        assert!(!egui_bars(true, true));
        assert!(!egui_bars(false, false));
        assert!(!egui_bars(false, true));
    }

    #[test]
    fn cinemascope_bars_are_170_of_1024_and_the_game_end_is_shown() {
        assert_eq!(cinemascope_bar_height(1024.0), 170.0);
        assert!((cinemascope_bar_height(600.0) - 99.609_375).abs() < 1e-4);
        let run = |cinemascope, end| {
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 1024.0),
                )),
                ..egui::RawInput::default()
            };
            let output = ctx.run_ui(input, |ui| paint_presentation(ui.ctx(), cinemascope, end));
            let shapes: Vec<_> = output.shapes.iter().map(|s| s.shape.clone()).collect();
            output.drop_without_applying_deltas();
            shapes
        };
        assert!(
            run(false, None).is_empty(),
            "nothing without cinemascope and end"
        );
        // Two black bars: top and bottom, each 170 high, across the whole width.
        let bars: Vec<egui::Rect> = run(true, None)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Rect(r) if r.fill == egui::Color32::BLACK => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(
            bars,
            [
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 170.0)),
                egui::Rect::from_min_max(egui::pos2(0.0, 854.0), egui::pos2(800.0, 1024.0)),
            ]
        );
        // The display at the game end is added.
        assert!(run(false, Some(GameEnd::Victory)).len() > run(false, None).len());
        assert_eq!(
            (GameEnd::Victory.text(), GameEnd::Defeat.text()),
            ("Victory", "Defeat")
        );
    }

    #[test]
    fn viewer_stops_stepping_after_the_game_end() {
        let (mut host, _) = script_host(&abl(&[(&[], &["n10"])]));
        // The test world has no playing slots (result 3 = unoccupied): slots 0 and 1 play.
        host.state.players[0].result = 0;
        host.state.players[1].result = 0;
        assert_eq!(game_end(&host), None);
        assert!(viewer_steps(Some(&host)));
        // The viewer tick: a frame sends steps only as long as `viewer_steps` allows it.
        let frames = |host: &mut SimHost, n: u32| {
            for _ in 0..n {
                if viewer_steps(Some(host)) {
                    host.advance(50.0);
                }
            }
        };
        frames(&mut host, 4);
        assert_eq!(host.state.steps, 4);
        // Slot 0 wins (command 0x56 in the next tick), then the viewer stops.
        host.control_result(0, 2);
        frames(&mut host, 1);
        assert_eq!(game_end(&host), Some(GameEnd::Victory));
        assert!(!viewer_steps(Some(&host)));
        let at = (host.state.steps, host.checksum());
        frames(&mut host, 20);
        assert_eq!((host.state.steps, host.checksum()), at, "no more steps");
        // The active slot counts: slot 1 lost in the process.
        assert_eq!(host.state.players[1].result, 1);
        host.active_player = 1;
        assert_eq!(game_end(&host), Some(GameEnd::Defeat));
        assert!(!viewer_steps(Some(&host)));
        // A restart resets the result: the match runs again.
        host.restart(host.setup.clone()).unwrap();
        host.state.players[0].result = 0;
        host.active_player = 0;
        assert!(viewer_steps(Some(&host)));
        // Without a simulation the viewer keeps running (model browser, map without rules).
        assert!(viewer_steps(None));
    }
}
