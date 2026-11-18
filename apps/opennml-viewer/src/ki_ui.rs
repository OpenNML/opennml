//! Match against the computer in the viewer: the "Match" window (from the "Simulation" window; per lobby slot kind,
//! nation, team, color, AI kind or handicap, plus AI on/off and campaign difficulty; "Restart" via the same path as
//! `sim_setup`), the "AI" window (key K; threads per engine in execution order, `m_iValues` with search, rejected
//! commands, command statistics; pause, continue and single step as commands `KiPause`/`KiStep`) and the AI overlay
//! (key L; frames around the objects with the `ai_marks` bit set of an AI slot, like `ToggleShowKISelection` of the
//! original). The lines are produced without egui (testable); the windows only read
//! (`SimHost::control_ki_state`), interventions go into the simulation as commands via
//! `SimHost::control_ki_command`.

use std::collections::HashMap;

use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind};
use glam::{Mat4, Vec3};
use opennml_control::{KiEngineInfo, KiStateResult, KiThreadInfo, SlotSetup, SlotSetupKind};
use opennml_formats::sound::NATIONS;
use opennml_sim::id::ObjectId;
use opennml_sim::setup::{MatchSetup, SlotKind, ki_kind_handicap};
use opennml_sim::state::{Control, SimState};

use crate::app::SimSetupArgs;
use crate::config::{Config, Mission};
use opennml_client::game::grid::project;
use opennml_client::game::sim_host::SimHost;

// --- Window "Match" ------------------------------------------------------------------------------

/// Draft of the "Match" window: one complete entry per lobby slot, plus AI on/off, the campaign difficulty and
/// script on/off with the speaking time per character. "Restart" passes it as [`SimSetupArgs`] on the same path as
/// `sim_setup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchDraft {
    pub slots: Vec<SlotSetup>,
    pub ki_enabled: bool,
    pub campaign_difficulty: i32,
    pub script_enabled: bool,
    pub time_per_char: i32,
    /// Chosen mission (index in the list of `campaign_missions`).
    pub mission: usize,
    /// Result of the last restart (an error appears as the message).
    pub status: Option<String>,
}

impl MatchDraft {
    /// Draft from the running match (`setup.lobby`) and the settings: the AI kind of an AI slot comes from
    /// `config.slots` (the lobby knows it only as a handicap, like the original).
    pub fn new(config: &Config, setup: &MatchSetup) -> Self {
        let slots = setup
            .lobby
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let kind = match e.kind {
                    SlotKind::Closed => SlotSetupKind::Closed,
                    SlotKind::Human => SlotSetupKind::Human,
                    SlotKind::Ai => SlotSetupKind::Ai,
                };
                let ki_kind = config
                    .slots
                    .get(i)
                    .filter(|s| s.kind == SlotSetupKind::Ai && kind == SlotSetupKind::Ai)
                    .and_then(|s| s.ki_kind);
                SlotSetup {
                    kind,
                    nation: Some(e.nation),
                    team: Some(e.team),
                    color: Some(e.color),
                    handicap: ki_kind.is_none().then_some(e.handicap),
                    ki_kind,
                }
            })
            .collect();
        Self {
            slots,
            ki_enabled: config.ki_enabled,
            campaign_difficulty: config.campaign_difficulty,
            script_enabled: setup.script.enabled,
            time_per_char: setup.script.time_per_char,
            mission: 0,
            status: None,
        }
    }

    /// New draft from the match after "Start" or "Restart"; the mission choice `mission` of the old draft stays
    /// (whoever wants to restart the same mission or pick the next one does not have to search again).
    pub fn renewed(mission: usize, config: &Config, setup: &MatchSetup) -> Self {
        Self {
            mission,
            ..Self::new(config, setup)
        }
    }

    /// Values for `sim_setup` (`SimSetupArgs::apply`); slots only with a lobby (the campaign has none).
    pub fn args(&self) -> SimSetupArgs {
        SimSetupArgs {
            slots: (!self.slots.is_empty()).then(|| self.slots.clone()),
            ki_enabled: Some(self.ki_enabled),
            campaign_difficulty: Some(self.campaign_difficulty),
            script_enabled: Some(self.script_enabled),
            time_per_char: Some(self.time_per_char),
            ..SimSetupArgs::default()
        }
    }
}

/// Buttons of the "Match" window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchAction {
    /// New match with the draft.
    Restart,
    /// Read the draft anew from the running match.
    Reload,
    /// Start campaign mission `index` (list of `campaign_missions`) with the difficulty of the draft: new level,
    /// game type -1, AI and level script on.
    StartMission(usize),
}

fn kind_name(kind: SlotSetupKind) -> &'static str {
    match kind {
        SlotSetupKind::Closed => "Closed",
        SlotSetupKind::Human => "Human",
        SlotSetupKind::Ai => "AI",
    }
}

/// Name of the nation 0-5 (`WID` ... `PAT`), otherwise the number.
fn nation_name(nation: i32) -> String {
    usize::try_from(nation)
        .ok()
        .and_then(|n| NATIONS.get(n))
        .map_or_else(|| nation.to_string(), |n| (*n).to_string())
}

/// Text of the choice "AI kind or handicap".
fn ki_choice_name(ki_kind: Option<u8>) -> String {
    match ki_kind.and_then(|k| Some((k, ki_kind_handicap(k)?))) {
        Some((k, h)) => format!("AI kind {k} (Handicap {h})"),
        None => "Free handicap".into(),
    }
}

/// Cell "AI kind or handicap": AI slots choose AI kind 1-4 or a free handicap 0-5, humans only the handicap;
/// closed slots nothing. Keeps the entry valid (AI kind only for AI, never together with `handicap`).
fn handicap_cell(ui: &mut egui::Ui, index: usize, slot: &mut SlotSetup) {
    if slot.kind != SlotSetupKind::Ai && slot.ki_kind.take().is_some() {
        slot.handicap = Some(0);
    }
    match slot.kind {
        SlotSetupKind::Closed => {
            ui.label("–");
        }
        SlotSetupKind::Human => {
            let mut h = slot.handicap.unwrap_or(0);
            ui.add(
                egui::DragValue::new(&mut h)
                    .range(0..=5)
                    .prefix("Handicap "),
            );
            slot.handicap = Some(h);
        }
        SlotSetupKind::Ai => {
            ui.horizontal(|ui| {
                let mut choice = slot.ki_kind;
                egui::ComboBox::from_id_salt(("ki_kind", index))
                    .selected_text(ki_choice_name(choice))
                    .show_ui(ui, |ui| {
                        for kind in opennml_control::KI_KIND_RANGE {
                            ui.selectable_value(
                                &mut choice,
                                Some(kind),
                                ki_choice_name(Some(kind)),
                            );
                        }
                        ui.selectable_value(&mut choice, None, ki_choice_name(None));
                    });
                slot.ki_kind = choice;
                if choice.is_some() {
                    slot.handicap = None;
                } else {
                    let mut h = slot.handicap.unwrap_or(0);
                    ui.add(
                        egui::DragValue::new(&mut h).range(0..=opennml_control::MAX_SLOT_HANDICAP),
                    );
                    slot.handicap = Some(h);
                }
            });
        }
    }
}

/// "Match" window: per lobby slot kind, nation, team, color and AI kind or handicap; AI on/off, campaign
/// difficulty, script on/off and speaking time per character; buttons "Restart" and "Reset" ([`MatchAction`]);
/// below them the mission list of the campaigns with "Start".
pub fn build_match(
    ctx: &egui::Context,
    open: &mut bool,
    draft: &mut MatchDraft,
    missions: &[Mission],
    actions: &mut Vec<MatchAction>,
) {
    egui::Window::new("Match")
        .open(open)
        .default_pos([490.0, 220.0])
        .default_width(560.0)
        .show(ctx, |ui| {
            if draft.slots.is_empty() {
                ui.label("Campaign map or game type -1: no lobby (only AI and difficulty)");
            } else {
                egui::Grid::new("match").striped(true).show(ui, |ui| {
                    for c in [
                        "Slot",
                        "Kind",
                        "Nation",
                        "Team",
                        "Color",
                        "AI kind / Handicap",
                    ] {
                        ui.strong(c);
                    }
                    ui.end_row();
                    for (i, slot) in draft.slots.iter_mut().enumerate() {
                        ui.label(i.to_string());
                        egui::ComboBox::from_id_salt(("kind", i))
                            .selected_text(kind_name(slot.kind))
                            .show_ui(ui, |ui| {
                                for kind in [
                                    SlotSetupKind::Closed,
                                    SlotSetupKind::Human,
                                    SlotSetupKind::Ai,
                                ] {
                                    ui.selectable_value(&mut slot.kind, kind, kind_name(kind));
                                }
                            });
                        let mut nation = slot.nation.unwrap_or(0);
                        egui::ComboBox::from_id_salt(("nation", i))
                            .selected_text(nation_name(nation))
                            .show_ui(ui, |ui| {
                                for (n, name) in (0..).zip(NATIONS) {
                                    ui.selectable_value(&mut nation, n, name);
                                }
                            });
                        slot.nation = Some(nation);
                        let index = i32::try_from(i).unwrap_or(0);
                        let mut team = slot.team.unwrap_or(index);
                        ui.add(egui::DragValue::new(&mut team).range(0..=7));
                        slot.team = Some(team);
                        let mut color = slot.color.unwrap_or(index);
                        ui.add(egui::DragValue::new(&mut color).range(0..=8));
                        slot.color = Some(color);
                        handicap_cell(ui, i, slot);
                        ui.end_row();
                    }
                });
            }
            ui.separator();
            ui.checkbox(
                &mut draft.ki_enabled,
                "Start AI (engines, setschwierigkeitsgrad, multi_startplayer)",
            );
            ui.horizontal(|ui| {
                ui.label("Campaign difficulty");
                ui.add(
                    egui::DragValue::new(&mut draft.campaign_difficulty)
                        .range(opennml_control::CAMPAIGN_DIFFICULTY_RANGE),
                );
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut draft.script_enabled, "Start level script (.abl)");
                ui.label("Speaking time per character");
                ui.add(
                    egui::DragValue::new(&mut draft.time_per_char)
                        .range(opennml_control::TIME_PER_CHAR_RANGE)
                        .suffix(" ms"),
                );
            });
            ui.horizontal(|ui| {
                if ui.button("Restart").clicked() {
                    actions.push(MatchAction::Restart);
                }
                if ui.button("Reset").clicked() {
                    actions.push(MatchAction::Reload);
                }
            });
            if !missions.is_empty() {
                ui.separator();
                draft.mission = draft.mission.min(missions.len() - 1);
                ui.horizontal(|ui| {
                    ui.label("Mission");
                    egui::ComboBox::from_id_salt("match_mission")
                        .selected_text(missions[draft.mission].label())
                        .show_ui(ui, |ui| {
                            for (i, m) in missions.iter().enumerate() {
                                ui.selectable_value(&mut draft.mission, i, m.label());
                            }
                        });
                    if ui
                        .button("Start")
                        .on_hover_text(
                            "Load level: game type -1, AI and level script on, difficulty as above",
                        )
                        .clicked()
                    {
                        actions.push(MatchAction::StartMission(draft.mission));
                    }
                });
            }
            if let Some(status) = &draft.status {
                ui.label(status);
            }
        });
}

// --- Window "AI" ---------------------------------------------------------------------------------

/// State of the "AI" window (key K).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiWindow {
    pub open: bool,
    /// Search in `m_iValues` (key or value contains the text).
    pub search: String,
    /// Slot and passes of the single step.
    pub step_slot: u8,
    pub passes: u32,
    /// Result of the last intervention (an error of the pre-check appears as the message).
    pub status: Option<String>,
}

impl Default for KiWindow {
    fn default() -> Self {
        Self {
            open: false,
            search: String::new(),
            step_slot: 0,
            passes: 1,
            status: None,
        }
    }
}

/// Buttons of the "AI" window; the host passes them as `ki_command` (`KiPause`, `KiStep`) into the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KiUiAction {
    /// Pause the whole AI (`true`) or let it continue running.
    Pause(bool),
    /// With the AI paused, `passes` passes of the engine of `slot`.
    Step { slot: u8, passes: u32 },
}

/// Columns of the thread table.
pub const THREAD_COLUMNS: [&str; 8] = [
    "ID",
    "Module",
    "Name",
    "Counter",
    "Status",
    "Pause ms",
    "Semaphore",
    "Stack",
];

/// One row of the thread table; `depth` > 0 for a running `CALLM` child (below the caller, shown in the window
/// as "Child <ID>").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRow {
    pub depth: usize,
    pub cells: [String; 8],
}

/// Status of a thread (`m_lStatus`, bits `opennml_sim::ki::vm::status`): 0x10 waits for the child (with 1 as 0x11,
/// when the caller is running), 4 done, 1 running; other values additionally as a number.
pub fn thread_status_name(status: u32) -> String {
    use opennml_sim::ki::vm::status::{CHILD, DONE, RUNNING};
    let name = if status & CHILD != 0 {
        "waiting for child"
    } else if status & DONE != 0 {
        "done"
    } else if status & RUNNING != 0 {
        "running"
    } else {
        return format!("{status:#x}");
    };
    if [RUNNING, DONE, CHILD].contains(&status) {
        name.into()
    } else {
        format!("{name} ({status:#x})")
    }
}

fn push_thread(rows: &mut Vec<ThreadRow>, t: &KiThreadInfo, depth: usize) {
    let top: Vec<String> = t.stack_top.iter().map(i32::to_string).collect();
    let more = if t.stack_depth > t.stack_top.len() {
        "… "
    } else {
        ""
    };
    rows.push(ThreadRow {
        depth,
        cells: [
            t.id.to_string(),
            t.module.to_string(),
            if t.name.is_empty() {
                "?".into()
            } else {
                t.name.clone()
            },
            t.pc.to_string(),
            thread_status_name(t.status),
            t.pause_ms.to_string(),
            if t.wait_semaphore < 0 {
                "–".into()
            } else {
                t.wait_semaphore.to_string()
            },
            format!("{}: {more}{}", t.stack_depth, top.join(" ")),
        ],
    });
    if let Some(child) = &t.child {
        push_thread(rows, child, depth + 1);
    }
}

/// Threads of an engine in execution order (order of `ki_state`, MFC order of the thread table), each running
/// `CALLM` child directly below its caller.
pub fn thread_rows(engine: &KiEngineInfo) -> Vec<ThreadRow> {
    let mut rows = Vec::new();
    for t in &engine.threads {
        push_thread(&mut rows, t, 0);
    }
    rows
}

/// `m_iValues` of an engine whose key or value contains `search` (empty: all), ascending.
pub fn value_rows(engine: &KiEngineInfo, search: &str) -> Vec<(i32, i32)> {
    let search = search.trim();
    engine
        .values
        .iter()
        .copied()
        .filter(|(k, v)| {
            search.is_empty() || k.to_string().contains(search) || v.to_string().contains(search)
        })
        .collect()
}

/// Result of an AI command as display text.
fn outcome_name(outcome: &str) -> &str {
    match outcome {
        "accepted" => "accepted",
        "partial" => "partial",
        "rejected" => "rejected",
        other => other,
    }
}

/// Rejected AI commands: slot, network command, reason, count.
pub fn reject_rows(state: &KiStateResult) -> Vec<[String; 4]> {
    state
        .rejected
        .iter()
        .map(|r| {
            [
                r.slot.to_string(),
                format!("{:#04x}", r.code),
                r.reason.clone(),
                r.count.to_string(),
            ]
        })
        .collect()
}

/// Command statistics: slot, network command, result, count.
pub fn command_rows(state: &KiStateResult) -> Vec<[String; 4]> {
    state
        .commands
        .iter()
        .map(|c| {
            [
                c.slot.to_string(),
                format!("{:#04x}", c.code),
                outcome_name(&c.outcome).to_string(),
                c.count.to_string(),
            ]
        })
        .collect()
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

/// "AI" window (key K): only reads `state`; the buttons end up in `actions`.
pub fn build_ki(
    ctx: &egui::Context,
    window: &mut KiWindow,
    state: &KiStateResult,
    actions: &mut Vec<KiUiAction>,
) {
    let mut open = window.open;
    egui::Window::new("AI")
        .open(&mut open)
        .default_pos([490.0, 40.0])
        .default_width(620.0)
        .default_height(520.0)
        .vscroll(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(if state.paused {
                    "AI paused"
                } else {
                    "AI is running"
                });
                if ui
                    .button(if state.paused { "Continue" } else { "Pause" })
                    .clicked()
                {
                    actions.push(KiUiAction::Pause(!state.paused));
                }
                ui.separator();
                let slots: Vec<u8> = state.engines.iter().map(|e| e.slot).collect();
                if !slots.contains(&window.step_slot)
                    && let Some(&first) = slots.first()
                {
                    window.step_slot = first;
                }
                egui::ComboBox::from_id_salt("ki_step_slot")
                    .selected_text(format!("Slot {}", window.step_slot))
                    .show_ui(ui, |ui| {
                        for s in slots {
                            ui.selectable_value(&mut window.step_slot, s, format!("Slot {s}"));
                        }
                    });
                ui.add(
                    egui::DragValue::new(&mut window.passes)
                        .range(1..=opennml_control::MAX_KI_STEP_PASSES)
                        .prefix("Passes "),
                );
                if ui
                    .add_enabled(state.paused, egui::Button::new("Step"))
                    .clicked()
                {
                    actions.push(KiUiAction::Step {
                        slot: window.step_slot,
                        passes: window.passes,
                    });
                }
            });
            if let Some(status) = &window.status {
                ui.label(status);
            }
            ui.horizontal(|ui| {
                ui.label("Search in m_iValues");
                ui.text_edit_singleline(&mut window.search);
            });
            if state.engines.is_empty() {
                ui.label("no engine (AI off or match without occupied slots)");
            }
            for engine in &state.engines {
                let rows = thread_rows(engine);
                egui::CollapsingHeader::new(format!(
                    "Slot {}: {} threads, next ID {}",
                    engine.slot,
                    engine.threads.len(),
                    engine.next_thread_id
                ))
                .id_salt(("ki_engine", engine.slot))
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new(("ki_threads", engine.slot))
                        .striped(true)
                        .show(ui, |ui| {
                            for c in THREAD_COLUMNS {
                                ui.strong(c);
                            }
                            ui.end_row();
                            for row in &rows {
                                for (i, cell) in row.cells.iter().enumerate() {
                                    if i == 0 && row.depth > 0 {
                                        ui.label(format!(
                                            "{}Child {cell}",
                                            "  ".repeat(row.depth - 1)
                                        ));
                                    } else {
                                        ui.label(cell);
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    let values = value_rows(engine, &window.search);
                    egui::CollapsingHeader::new(format!(
                        "m_iValues ({} of {})",
                        values.len(),
                        engine.values.len()
                    ))
                    .id_salt(("ki_values", engine.slot))
                    .show(ui, |ui| {
                        let rows: Vec<[String; 2]> = values
                            .iter()
                            .map(|(k, v)| [k.to_string(), v.to_string()])
                            .collect();
                        grid(
                            ui,
                            &format!("ki_values_{}", engine.slot),
                            ["Key", "Value"],
                            &rows,
                        );
                    });
                    if !engine.semaphores.is_empty() {
                        let rows: Vec<[String; 2]> = engine
                            .semaphores
                            .iter()
                            .map(|(k, v)| [k.to_string(), v.to_string()])
                            .collect();
                        egui::CollapsingHeader::new(format!("Semaphores ({})", rows.len()))
                            .id_salt(("ki_semaphores", engine.slot))
                            .show(ui, |ui| {
                                grid(
                                    ui,
                                    &format!("ki_semaphores_{}", engine.slot),
                                    ["Key", "held"],
                                    &rows,
                                );
                            });
                    }
                });
            }
            egui::CollapsingHeader::new(format!(
                "Rejected commands ({})",
                state.rejected.iter().map(|r| r.count).sum::<u64>()
            ))
            .default_open(true)
            .show(ui, |ui| {
                grid(
                    ui,
                    "ki_rejected",
                    ["Slot", "Command", "Reason", "Count"],
                    &reject_rows(state),
                );
            });
            egui::CollapsingHeader::new(format!(
                "Command statistics ({})",
                state.commands.iter().map(|c| c.count).sum::<u64>()
            ))
            .default_open(true)
            .show(ui, |ui| {
                grid(
                    ui,
                    "ki_commands",
                    ["Slot", "Command", "Result", "Count"],
                    &command_rows(state),
                );
            });
        });
    window.open = open;
}

// --- AI overlay ----------------------------------------------------------------------------------

/// Key L: next slot of the AI overlay - off -> the AI slots with an engine ascending -> off.
pub fn next_overlay_slot(state: &SimState, current: Option<u8>) -> Option<u8> {
    let mut slots = (0..8u8).filter(|&p| {
        state.ki.engines[usize::from(p)].is_some()
            && state.players[usize::from(p)].control == Control::Ai
    });
    match current {
        None => slots.next(),
        Some(c) => slots.find(|&p| p > c),
    }
}

/// Objects with the `ai_marks` bit of `slot` set (slot 0-7), in the order of the object table.
pub fn marked_objects(state: &SimState, slot: u8) -> Vec<ObjectId> {
    if slot >= 8 {
        return Vec::new();
    }
    state
        .objects()
        .filter(|o| o.ai_marks >> slot & 1 == 1)
        .map(|o| o.id)
        .collect()
}

/// Half the minimum width of a frame (world units).
const MARK_MIN_HALF: f32 = 20.0;
/// Margin above the hull (world units).
const MARK_LIFT: f32 = 8.0;

/// Frames of the overlay (pixels, `[top left, bottom right]`): for each marked object with a drawn model (`tops` =
/// top edge of the hull) a rectangle turned toward the camera from the foot point to above the hull, half as wide as
/// high (at least 40 world units); objects behind the camera are omitted.
pub fn mark_frames(
    sim: &SimHost,
    slot: u8,
    tops: &HashMap<ObjectId, Vec3>,
    view_proj: Mat4,
    right: Vec3,
    size: (u32, u32),
) -> Vec<[[f32; 2]; 2]> {
    let mut frames = Vec::new();
    for id in marked_objects(&sim.state, slot) {
        let (Some(&top), Some(o)) = (tops.get(&id), sim.state.object(id, false)) else {
            continue;
        };
        let foot = sim.display(o).0;
        let top = top + Vec3::Y * MARK_LIFT;
        let half = ((top.y - foot.y) * 0.5).max(MARK_MIN_HALF);
        let corners = [
            foot - right * half,
            foot + right * half,
            top - right * half,
            top + right * half,
        ]
        .map(|p| project(view_proj, p, size));
        let Some(points) = corners.into_iter().collect::<Option<Vec<_>>>() else {
            continue;
        };
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in points {
            for axis in 0..2 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        frames.push([min, max]);
    }
    frames
}

/// Color of the frames (magenta, stands out from selection rings and health bars).
const MARK_COLOR: Color32 = Color32::from_rgb(255, 60, 220);

/// Draws the frames and a line with the slot and the number of marked objects (`marked`) with the background
/// painter of egui (below the windows).
pub fn paint_marks(ctx: &egui::Context, slot: u8, marked: usize, frames: &[[[f32; 2]; 2]]) {
    let ppp = ctx.pixels_per_point();
    let point = |p: [f32; 2]| Pos2::new(p[0] / ppp, p[1] / ppp);
    let painter = ctx.layer_painter(egui::LayerId::background());
    for frame in frames {
        painter.rect_stroke(
            Rect::from_min_max(point(frame[0]), point(frame[1])),
            2.0,
            Stroke::new(2.0, MARK_COLOR),
            StrokeKind::Outside,
        );
    }
    let screen = ctx.content_rect();
    painter.text(
        Pos2::new(screen.center().x, screen.top() + 8.0),
        Align2::CENTER_TOP,
        format!(
            "AI overlay slot {slot}: {marked} marked, {} in view (L: next)",
            frames.len()
        ),
        FontId::proportional(15.0),
        MARK_COLOR,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testworld_viewer::{ki_host, sim_setup};
    use glam::Vec4;
    use opennml_client::game::testworld::{KI_MARK, ai_slot, human_slot, spawn};
    use opennml_control::{KiAction, KiCommandCount, KiRejectCount};

    fn thread(id: i32, module: u32, name: &str, child: Option<KiThreadInfo>) -> KiThreadInfo {
        KiThreadInfo {
            id,
            module,
            name: name.into(),
            pc: 12,
            status: if child.is_some() { 0x10 } else { 1 },
            pause_ms: 40,
            wait_semaphore: -1,
            stack_depth: 10,
            stack_top: vec![1, 2, 3, 4, 5, 6, 7, 8],
            child: child.map(Box::new),
        }
    }

    #[test]
    fn ki_window_lists_threads_in_execution_order() {
        // Execution order is the order of `ki_state` (MFC), not that of the IDs; the child stands below its
        // caller.
        let engine = KiEngineInfo {
            slot: 3,
            next_thread_id: 10,
            threads: vec![
                thread(7, 195_235, "multi_startplayer", None),
                thread(2, 300, "", Some(thread(2, 301, "child", None))),
                thread(5, 302, "third", None),
            ],
            values: vec![(302, 1000), (500, 1), (1302, 7)],
            semaphores: vec![(4, 1)],
        };
        let rows = thread_rows(&engine);
        let ids: Vec<(&str, usize)> = rows
            .iter()
            .map(|r| (r.cells[0].as_str(), r.depth))
            .collect();
        assert_eq!(ids, [("7", 0), ("2", 0), ("2", 1), ("5", 0)]);
        assert_eq!(
            rows[0].cells,
            [
                "7",
                "195235",
                "multi_startplayer",
                "12",
                "running",
                "40",
                "–",
                "10: … 1 2 3 4 5 6 7 8"
            ]
            .map(String::from)
        );
        assert_eq!(
            (rows[1].cells[2].as_str(), rows[1].cells[4].as_str()),
            ("?", "waiting for child")
        );
        assert_eq!(rows[2].cells[1], "301");
        assert_eq!(thread_status_name(4), "done");
        assert_eq!(thread_status_name(0x11), "waiting for child (0x11)");
        assert_eq!(thread_status_name(2), "0x2");
        // Search in m_iValues: key or value contains the text.
        assert_eq!(value_rows(&engine, ""), engine.values);
        assert_eq!(value_rows(&engine, " 302 "), [(302, 1000), (1302, 7)]);
        assert_eq!(value_rows(&engine, "7"), [(1302, 7)]);
        assert!(value_rows(&engine, "999").is_empty());
        let state = KiStateResult {
            paused: true,
            engines: vec![engine],
            rejected: vec![KiRejectCount {
                slot: 3,
                code: 0x01,
                reason: "Construction: no space".into(),
                count: 2,
            }],
            commands: vec![
                KiCommandCount {
                    slot: 3,
                    code: 0x01,
                    outcome: "rejected".into(),
                    count: 2,
                },
                KiCommandCount {
                    slot: 3,
                    code: 0x0E,
                    outcome: "partial".into(),
                    count: 1,
                },
            ],
        };
        assert_eq!(
            reject_rows(&state),
            [["3", "0x01", "Construction: no space", "2"].map(String::from)]
        );
        assert_eq!(
            command_rows(&state),
            [
                ["3", "0x01", "rejected", "2"].map(String::from),
                ["3", "0x0e", "partial", "1"].map(String::from)
            ]
        );
        // The real match: rows in the order of the thread table of the engine, name from the catalog.
        let (mut host, _) = ki_host();
        host.control_ki_command(opennml_client::game::sim_control::KiCommandRequest {
            action: KiAction::Start,
            slot: Some(1),
            module: Some(KI_MARK),
            args: None,
            sync: None,
            paused: None,
            passes: None,
        })
        .unwrap();
        host.step_once();
        let before = (host.checksum(), host.pending_commands().len());
        let live = host.control_ki_state(None, true).unwrap();
        let engine = live.engines.iter().find(|e| e.slot == 1).unwrap();
        let rows = thread_rows(engine);
        let order: Vec<String> = host.state.ki.engines[1]
            .as_ref()
            .unwrap()
            .threads
            .iter()
            .map(|(_, t)| t.id.to_string())
            .collect();
        assert!(!order.is_empty());
        assert_eq!(
            rows.iter().map(|r| r.cells[0].clone()).collect::<Vec<_>>(),
            order
        );
        assert_eq!(rows[0].cells[2], "multi_startplayer");
        // The window builds without a window system and leaves the simulation untouched.
        let ctx = egui::Context::default();
        let mut window = KiWindow {
            open: true,
            ..KiWindow::default()
        };
        let mut actions = Vec::new();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            build_ki(ui.ctx(), &mut window, &live, &mut actions)
        });
        assert!(!output.shapes.is_empty());
        output.drop_without_applying_deltas();
        assert!(actions.is_empty() && window.open);
        assert_eq!((host.checksum(), host.pending_commands().len()), before);
    }

    #[test]
    fn overlay_marks_follow_the_selected_slot() {
        let (mut host, mut config) = ki_host();
        let args = SimSetupArgs {
            slots: Some(vec![
                human_slot(),
                ai_slot(1, 3),
                human_slot(),
                ai_slot(3, 2),
            ]),
            ..SimSetupArgs::default()
        };
        sim_setup(&mut host, &mut config, args).unwrap();
        // L: off -> AI slots with an engine (1, 3; the human slots 0 and 2 also have engines) -> off.
        let state = &host.state;
        assert!(state.ki.engines[0].is_some() && state.ki.engines[2].is_some());
        assert_eq!(next_overlay_slot(state, None), Some(1));
        assert_eq!(next_overlay_slot(state, Some(1)), Some(3));
        assert_eq!(next_overlay_slot(state, Some(3)), None);
        // Without AI no engine: L stays off.
        let mut off = config.clone();
        sim_setup(
            &mut host,
            &mut off,
            SimSetupArgs {
                ki_enabled: Some(false),
                ..SimSetupArgs::default()
            },
        )
        .unwrap();
        assert_eq!(next_overlay_slot(&host.state, None), None);
        sim_setup(
            &mut host,
            &mut config,
            SimSetupArgs {
                ki_enabled: Some(true),
                ..SimSetupArgs::default()
            },
        )
        .unwrap();
        let a = spawn(&mut host, 1100, 1, (4, 4));
        let b = spawn(&mut host, 1100, 3, (8, 4));
        let c = spawn(&mut host, 1100, 0, (12, 4));
        let d = spawn(&mut host, 1100, 0, (16, 4));
        for (id, marks) in [(a, 1 << 1), (b, 1 << 3), (c, 1 << 1 | 1 << 3), (d, 1)] {
            host.state.object_mut(id, false).unwrap().ai_marks = marks;
        }
        let sorted = |mut v: Vec<ObjectId>| {
            v.sort_by_key(|id| id.0);
            v
        };
        let before = host.checksum();
        assert_eq!(sorted(marked_objects(&host.state, 1)), sorted(vec![a, c]));
        assert_eq!(sorted(marked_objects(&host.state, 3)), sorted(vec![b, c]));
        assert_eq!(marked_objects(&host.state, 0), [d]);
        assert!(marked_objects(&host.state, 8).is_empty());
        // Frames only for objects with a drawn model (top edge known), from the foot to above the
        // hull.
        let foot = host.display(host.state.object(a, false).unwrap()).0;
        let tops = HashMap::from([(a, foot + Vec3::Y * 100.0), (b, foot + Vec3::Y * 100.0)]);
        let view_proj = Mat4::from_scale(Vec3::splat(1.0e-4)) * Mat4::from_translation(-foot);
        let frames = mark_frames(&host, 1, &tops, view_proj, Vec3::X, (1000, 1000));
        assert_eq!(frames.len(), 1, "c without a model is omitted");
        let [min, max] = frames[0];
        assert!(min[0] < 500.0 && max[0] > 500.0, "{frames:?}");
        assert!(min[1] < max[1] && max[1] >= 500.0, "{frames:?}");
        assert_eq!(
            mark_frames(&host, 3, &tops, view_proj, Vec3::X, (1000, 1000)).len(),
            1
        );
        // Behind the camera the frame is omitted.
        let behind = Mat4::from_cols(Vec4::X, Vec4::Y, Vec4::Z, Vec4::new(0.0, 0.0, 0.0, -1.0));
        assert!(mark_frames(&host, 1, &tops, behind, Vec3::X, (1000, 1000)).is_empty());
        assert_eq!(host.checksum(), before, "the overlay only reads");
    }

    #[test]
    fn renewed_draft_keeps_the_mission_choice() {
        let (host, config) = ki_host();
        let mut old = MatchDraft::new(&config, &host.setup);
        assert_eq!(old.mission, 0);
        old.mission = 5;
        old.status = Some("old".into());
        let renewed = MatchDraft::renewed(old.mission, &config, &host.setup);
        assert_eq!(renewed.mission, 5, "the choice stays after \"Start\"");
        assert_eq!(renewed.status, None, "the rest comes anew from the match");
        assert_eq!(renewed.slots, MatchDraft::new(&config, &host.setup).slots);
    }

    #[test]
    fn match_draft_restarts_the_same_lobby() {
        // "Match" window: draft from the running match, "Restart" via `sim_setup` gives the same lobby; the AI
        // kind stays (the lobby knows only the handicap).
        let (mut host, mut config) = ki_host();
        let draft = MatchDraft::new(&config, &host.setup);
        assert_eq!(draft.slots.len(), 4);
        assert_eq!(
            (draft.slots[1].ki_kind, draft.slots[1].handicap),
            (Some(3), None)
        );
        assert_eq!(draft.slots[0].handicap, Some(0));
        opennml_control::check_slots(&draft.slots).unwrap();
        let lobby = host.setup.lobby.clone();
        sim_setup(&mut host, &mut config, draft.args()).unwrap();
        assert_eq!(host.setup.lobby, lobby);
        assert!(host.setup.ki.enabled);
        // Changed draft: slot 2 AI with free handicap 4, AI off, difficulty 2.
        let mut draft = MatchDraft::new(&config, &host.setup);
        draft.slots[2].kind = SlotSetupKind::Ai;
        draft.slots[2].handicap = Some(4);
        draft.ki_enabled = false;
        draft.campaign_difficulty = 2;
        assert!(!draft.script_enabled, "viewer default: script off");
        draft.script_enabled = true;
        draft.time_per_char = 90;
        sim_setup(&mut host, &mut config, draft.args()).unwrap();
        assert_eq!(
            host.setup.script,
            opennml_sim::script::ScriptSetup {
                enabled: true,
                time_per_char: 90,
                ..Default::default()
            }
        );
        assert_eq!(MatchDraft::new(&config, &host.setup).time_per_char, 90);
        assert_eq!(
            (host.setup.lobby[2].kind, host.setup.lobby[2].handicap),
            (SlotKind::Ai, 4)
        );
        assert_eq!(
            (host.setup.ki.enabled, host.setup.ki.campaign_difficulty),
            (false, 2)
        );
        // The window builds without a window system.
        let ctx = egui::Context::default();
        let (mut open, mut actions) = (true, Vec::new());
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            build_match(ui.ctx(), &mut open, &mut draft, &[], &mut actions)
        });
        assert!(!output.shapes.is_empty());
        output.drop_without_applying_deltas();
        assert!(actions.is_empty());
        // With a mission list: the index is clamped to the list, without a click no action.
        let missions = [Mission {
            campaign: 1,
            name: "kamp1-1".into(),
            path: "data/levels/kampagne1/kamp1-1/kamp1-1.lvl".into(),
        }];
        draft.mission = 7;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            build_match(ui.ctx(), &mut open, &mut draft, &missions, &mut actions)
        });
        output.drop_without_applying_deltas();
        assert_eq!(draft.mission, 0);
        assert_eq!(missions[0].label(), "Campaign 1: kamp1-1");
        assert!(actions.is_empty());
    }
}
