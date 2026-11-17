//! egui window "Economy" (key T) and - only as a viewer aid with the path grid overlay (G) - the
//! footprint of the building site (green valid, red invalid, cross at the snapped position). The
//! rally point flag is drawn like in the original by the model `flag.rdo` in the world
//! ([`nml_client::game::rally_flag`]); the window names its coordinates. The build preview itself
//! is, as in the original, the tinted model (`MapView::set_build_preview`). Content and commands come
//! from [`nml_client::game::economy`]; here only display and clicks. Below the account are the
//! running counter-upgrades with remaining time (HUD `0x00455C00`).

use std::collections::BTreeMap;

use egui::{Color32, Pos2, Rect, Stroke};
use glam::{Mat4, Vec3};
use nml_formats::gifcon::button_kind;
use nml_sim::rules::Rules;

use nml_client::game::economy::{
    BuildingPanel, HirePanel, MenuEntry, PanelAction, PanelModel, Placement, PlacementPreview,
    cost_text, placement_line,
};
use nml_client::game::grid::project;
use nml_client::game::sim_host::SimHost;

/// Colors of the footprint.
const VALID: Color32 = Color32::from_rgb(40, 230, 60);
const INVALID: Color32 = Color32::from_rgb(240, 40, 40);
/// Color of the debug buttons (no game function): red with a dashed border so that they stand out from
/// the game buttons.
const DEBUG: Color32 = Color32::from_rgb(235, 50, 50);
/// Background of a debug button under the mouse or pressed.
const DEBUG_HOVER: Color32 = Color32::from_rgba_premultiplied(60, 12, 12, 60);
/// Tooltip of the debug button "complete immediately".
const DEBUG_COMPLETE_TIP: &str = "Debug: complete order immediately (not a game feature)";

/// What the window and the overlays show per frame (screen points in physical pixels).
pub struct EconomyView<'a> {
    pub rules: &'a Rules,
    /// Window content; `None` when the window is closed.
    pub model: Option<&'a PanelModel>,
    pub placement: Option<&'a Placement>,
    pub preview: Option<&'a PlacementPreview>,
    /// Selling waits for the second click.
    pub sell_armed: bool,
    /// Outline of the building site and screen point of the snapped position (empty or `None` without
    /// the path grid overlay).
    pub footprint: Vec<[f32; 2]>,
    pub anchor: Option<[f32; 2]>,
    /// Running counter-upgrades ([`counter_upgrade_rows`]).
    pub counter_upgrades: Vec<CounterUpgradeRow>,
    /// Label "Back" (text 409) of the headhunter menu ([`nml_client::game::unit_bar::back_label`]).
    pub hire_back: String,
}

/// Row of a running counter-upgrade: "<name> m:ss" in the color of the source slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterUpgradeRow {
    pub text: String,
    pub color: [u8; 3],
}

/// Running counter-upgrades of all players in list order (HUD `0x00455C00` over the list
/// `GameWorld+0x990`): remaining time `"%d:%02d"` with `rest / 60000` and `(rest / 1000) % 60` (truncated)
/// from the state, name from `texts` (`guids.deu`, otherwise "Counter-upgrade <GUID>"), color of the slot
/// of the stored source (`(hid >> 11) & 0x1F`, `Player_GetColor`). Deviation: the original shows the icon
/// of the button instead of the name (50 x 50, `FUN_00453CC0`), the time in white with a shadow and frames
/// the icon in the player color only for enemies of the local player. Read only.
pub fn counter_upgrade_rows(
    host: &SimHost,
    texts: &BTreeMap<i32, String>,
) -> Vec<CounterUpgradeRow> {
    host.state
        .counter_upgrades
        .iter()
        .map(|c| {
            let name = i32::try_from(c.guid)
                .ok()
                .and_then(|g| texts.get(&g))
                .filter(|t| !t.is_empty())
                .cloned()
                .unwrap_or_else(|| nml_client::game::unit_bar::counter_upgrade_fallback(c.guid));
            let slot = usize::from(c.source.slot());
            let color = host.state.players.get(slot).map_or(8, |p| p.color);
            CounterUpgradeRow {
                text: format!(
                    "{name} {}:{:02}",
                    c.rest_ms / 60_000,
                    (c.rest_ms / 1000) % 60
                ),
                color: nml_world::markers::player_rgb(color),
            }
        })
        .collect()
}

/// Outline of the building site in screen points: the tile border of the checked rectangle over the
/// terrain (every tile corner at its height), without a rectangle the hull on the ground.
pub fn footprint_outline(
    rules: &Rules,
    preview: &PlacementPreview,
    view_proj: Mat4,
    size: (u32, u32),
) -> Vec<[f32; 2]> {
    let (sx, sz) = (rules.map.cell[0] as f32, rules.map.cell[2] as f32);
    let at = |x: f32, z: f32| {
        project(
            view_proj,
            Vec3::new(x, rules.ground_height(x, z) + 4.0, z),
            size,
        )
    };
    let points: Vec<(f32, f32)> = match preview.rect {
        Some([x0, z0, x1, z1]) => {
            let (x1, z1) = (x1 + 1, z1 + 1);
            let mut ring = Vec::new();
            ring.extend((x0..x1).map(|x| (x, z0)));
            ring.extend((z0..z1).map(|z| (x1, z)));
            ring.extend((x0 + 1..=x1).rev().map(|x| (x, z1)));
            ring.extend((z0 + 1..=z1).rev().map(|z| (x0, z)));
            ring.into_iter()
                .map(|(x, z)| (x as f32 * sx, z as f32 * sz))
                .collect()
        }
        None => preview.hull.iter().map(|c| (c[0], c[1])).collect(),
    };
    points.into_iter().filter_map(|(x, z)| at(x, z)).collect()
}

/// Screen point of the snapped position.
pub fn anchor_point(
    rules: &Rules,
    preview: &PlacementPreview,
    view_proj: Mat4,
    size: (u32, u32),
) -> Option<[f32; 2]> {
    let [x, _, z] = preview.pos;
    project(view_proj, Vec3::new(x, rules.ground_height(x, z), z), size)
}

/// Text of a list row: GUID, name and for research (`UPGRADE`) the suffix "(research)".
/// Counter-upgrades are not in the production list (bar buttons 3370-3377).
fn entry_line(e: &MenuEntry) -> String {
    let mut text = format!("{} {}", e.guid, e.name);
    if e.kind == button_kind::UPGRADE {
        text.push_str(" (research)");
    }
    text
}

/// A list (build or production) as a grid: button, cost, reason for the lock.
fn entry_grid(
    ui: &mut egui::Ui,
    id: &str,
    entries: &[MenuEntry],
    actions: &mut Vec<PanelAction>,
    action: impl Fn(u32) -> PanelAction,
) {
    egui::Grid::new(id).striped(true).show(ui, |ui| {
        for e in entries {
            let button = egui::Button::new(entry_line(e));
            if ui.add_enabled(e.blocked.is_none(), button).clicked() {
                actions.push(action(e.guid));
            }
            let cost = cost_text(e.costs);
            if e.affordable {
                ui.label(cost);
            } else {
                ui.colored_label(INVALID, cost);
            }
            ui.weak(e.blocked.as_deref().unwrap_or(""));
            ui.end_row();
        }
    });
}

/// Debug button: red text, dashed red border. Border and size stay the same in all states (no enlargement
/// and no border change on hovering, so that the button does not jump); under the mouse or pressed only a
/// dark red background. Disabled gray.
fn debug_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let color = if enabled { DEBUG } else { Color32::GRAY };
    let response = ui
        .scope(|ui| {
            let widgets = &mut ui.visuals_mut().widgets;
            for (visuals, fill) in [
                (&mut widgets.inactive, Color32::TRANSPARENT),
                (&mut widgets.noninteractive, Color32::TRANSPARENT),
                (&mut widgets.hovered, DEBUG_HOVER),
                (&mut widgets.active, DEBUG_HOVER),
            ] {
                visuals.expansion = 0.0;
                visuals.bg_stroke = Stroke::NONE;
                visuals.weak_bg_fill = fill;
                visuals.bg_fill = fill;
            }
            let text = egui::RichText::new(label).small().color(color);
            ui.add_enabled(enabled, egui::Button::new(text))
        })
        .inner;
    let r = response.rect.shrink(0.5);
    let corners = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    ui.painter().extend(egui::Shape::dashed_line(
        &corners,
        Stroke::new(1.0, color),
        3.0,
        2.0,
    ));
    response
}

/// Queue of a building: per order name, progress or "waiting", "x" (cancel `0x0F`, game function) and - set
/// apart by spacing and separator, dashed red outline - the debug button "complete immediately"
/// (`DebugCompleteOrder`, not a game feature). The debug button is enabled only for living (not sleeping)
/// buildings of the active player ([`BuildingPanel::debug_complete`]), otherwise disabled with a tooltip.
/// Clicks go into `actions`; returns per row the areas of "x" and the debug button.
fn queue_rows(
    ui: &mut egui::Ui,
    b: &BuildingPanel,
    actions: &mut Vec<PanelAction>,
) -> Vec<(Rect, Rect)> {
    let mut rects = Vec::new();
    if !b.queue.is_empty() && !b.debug_complete {
        ui.weak("Debug \"complete immediately\" only for living (not sleeping) own buildings");
    }
    egui::Grid::new("queue").show(ui, |ui| {
        for q in &b.queue {
            ui.label(format!("{} {}", q.guid, q.name));
            match q.progress {
                Some(p) => ui.add(
                    egui::ProgressBar::new(p)
                        .desired_width(140.0)
                        .text(format!("{:.0} %", p * 100.0)),
                ),
                None => ui.weak(format!("waiting ({})", q.index)),
            };
            let row = ui.horizontal(|ui| {
                let cancel = ui.small_button("×").on_hover_text("Cancel order");
                if cancel.clicked() {
                    actions.push(PanelAction::Cancel(q.index));
                }
                ui.add_space(8.0);
                ui.separator();
                let debug = debug_button(ui, "DBG now", b.debug_complete)
                    .on_hover_text(DEBUG_COMPLETE_TIP)
                    .on_disabled_hover_text(format!(
                        "{DEBUG_COMPLETE_TIP} - disabled: only for living (not sleeping) own buildings"
                    ));
                if debug.clicked() {
                    actions.push(PanelAction::DebugComplete {
                        index: q.index,
                        guid: q.guid,
                    });
                }
                (cancel.rect, debug.rect)
            });
            rects.push(row.inner);
            ui.end_row();
        }
    });
    rects
}

/// Builds window and overlays of a frame; `open` = window visible. Clicks go into
/// `actions` (in click order).
pub fn build(
    ctx: &egui::Context,
    open: &mut bool,
    view: &EconomyView,
    actions: &mut Vec<PanelAction>,
) {
    let ppp = ctx.pixels_per_point();
    let point = |p: [f32; 2]| Pos2::new(p[0] / ppp, p[1] / ppp);
    let painter = ctx.layer_painter(egui::LayerId::background());
    if view.footprint.len() >= 3 {
        let color = if view.preview.is_some_and(|p| p.valid) {
            VALID
        } else {
            INVALID
        };
        painter.add(egui::Shape::closed_line(
            view.footprint.iter().map(|&p| point(p)).collect(),
            Stroke::new(2.0, color),
        ));
        if let Some(a) = view.anchor {
            let c = point(a);
            let d = 6.0;
            for (from, to) in [((-d, 0.0), (d, 0.0)), ((0.0, -d), (0.0, d))] {
                painter.line_segment(
                    [c + egui::vec2(from.0, from.1), c + egui::vec2(to.0, to.1)],
                    Stroke::new(2.0, color),
                );
            }
        }
    }
    let Some(model) = view.model else {
        return;
    };
    let width = ctx.content_rect().width();
    egui::Window::new("Economy")
        .open(open)
        .default_pos([(width - 440.0).max(16.0), 60.0])
        .default_width(420.0)
        .default_height(520.0)
        .vscroll(true)
        .show(ctx, |ui| {
            ui.label(&model.account);
            for row in &view.counter_upgrades {
                let [r, g, b] = row.color;
                ui.colored_label(Color32::from_rgb(r, g, b), &row.text);
            }
            if let Some(p) = view.placement {
                let color = if view.preview.is_some_and(|p| p.valid) {
                    VALID
                } else {
                    INVALID
                };
                ui.colored_label(color, placement_line(view.rules, p, view.preview));
            }
            if !model.build.is_empty() {
                egui::CollapsingHeader::new(format!("Build ({} workers)", model.workers.len()))
                    .default_open(true)
                    .show(ui, |ui| {
                        entry_grid(ui, "build_list", &model.build, actions, PanelAction::Build);
                        ui.weak("Left click places, Shift+left click places more, R rotates, Esc/right click ends; walls only as single pieces");
                    });
            }
            if let Some(b) = &model.building {
                ui.separator();
                ui.strong(format!(
                    "{} {}{}",
                    b.name,
                    if b.finished { "" } else { "(construction site)" },
                    if b.dead { " (destroyed)" } else { "" }
                ));
                if !b.production.is_empty() {
                    egui::CollapsingHeader::new("Production")
                        .default_open(true)
                        .show(ui, |ui| {
                            entry_grid(ui, "production", &b.production, actions, PanelAction::Produce);
                        });
                }
                if let Some(h) = &b.hire {
                    egui::CollapsingHeader::new("Headhunter")
                        .default_open(true)
                        .show(ui, |ui| hire_rows(ui, h, &view.hire_back, actions));
                }
                if b.is_building {
                    egui::CollapsingHeader::new(format!("Queue ({})", b.queue.len()))
                        .default_open(true)
                        .show(ui, |ui| {
                            if b.queue.is_empty() {
                                ui.weak("empty");
                            }
                            queue_rows(ui, b, actions);
                        });
                }
                // Buildings and mobile containers (ship, wagon) with a quarters module.
                if let Some((cur, max)) = b.quarter {
                    egui::CollapsingHeader::new(format!("Quarters {cur}/{max}"))
                        .default_open(true)
                        .show(ui, |ui| {
                            if ui
                                .add_enabled(!b.occupants.is_empty(), egui::Button::new("Let out all"))
                                .clicked()
                            {
                                actions.push(PanelAction::UnquarterAll);
                            }
                            for (unit, name) in &b.occupants {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{name} ({:#010x})", unit.0));
                                    if ui.small_button("Let out").clicked() {
                                        actions.push(PanelAction::Unquarter(*unit));
                                    }
                                });
                            }
                        });
                }
                if b.is_building {
                    ui.label(format!(
                        "Rally point ({:.2}, {:.2}) tiles - right click on the ground sets it",
                        b.meeting_point[0], b.meeting_point[1]
                    ));
                    let sell = if view.sell_armed {
                        "Really sell? (click again)"
                    } else {
                        "Sell"
                    };
                    if ui.add_enabled(!b.dead, egui::Button::new(sell)).clicked() {
                        actions.push(PanelAction::Sell);
                    }
                }
            }
            if model.build.is_empty() && model.building.is_none() {
                ui.weak("Select own workers or an own building (left click, selection box)");
            }
        });
}

/// Headhunter menu (re/headhunter-robbing 2.5): level 1 one button per kind (gray when locked) with price
/// and the lines target and client (colored); after a chosen kind one button per target player in their
/// color, after a chosen target player one button per target GUID, on both "Back".
fn hire_rows(ui: &mut egui::Ui, h: &HirePanel, back: &str, actions: &mut Vec<PanelAction>) {
    for a in &h.arts {
        ui.horizontal(|ui| {
            let chosen = h.art == Some(a.art);
            let label = if chosen {
                format!("▶ {}", a.name)
            } else {
                a.name.clone()
            };
            if ui
                .add_enabled(a.enabled, egui::Button::new(label))
                .clicked()
            {
                actions.push(PanelAction::HireArt(a.art));
            }
            ui.label(format!("{} Gold", a.price));
        });
        // Target and client: two lines without a label in the player color (original: tooltips
        // `FUN_0042F7B0`/`FUN_0042F8B0`; deviation: place in the window).
        for (text, color) in a.target.iter().chain(a.customer.iter()) {
            let [r, g, b] = nml_world::markers::player_rgb(*color);
            ui.label(egui::RichText::new(text).color(Color32::from_rgb(r, g, b)));
        }
    }
    if h.art.is_none() {
        return;
    }
    ui.separator();
    if h.target_player.is_none() {
        ui.label("Target player:");
        if h.players.is_empty() {
            ui.weak("none");
        }
        for (slot, name, color) in &h.players {
            let [r, g, b] = nml_world::markers::player_rgb(*color);
            let text = egui::RichText::new(name).color(Color32::from_rgb(r, g, b));
            if ui.button(text).clicked() {
                actions.push(PanelAction::HirePlayer(*slot));
            }
        }
    } else {
        ui.label("Target:");
        if h.targets.is_empty() {
            ui.weak("none");
        }
        for (guid, name) in &h.targets {
            if ui.button(name).clicked() {
                actions.push(PanelAction::HireTarget(*guid));
            }
        }
    }
    if ui.button(back).clicked() {
        actions.push(PanelAction::HireBack);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_client::game::economy::testkit::{COLONIST, HQ, host, spawn};

    /// A click (press and release) in the same frame works in egui like across two frames: the control
    /// channel sends both before the next step.
    #[test]
    fn a_press_and_release_in_one_frame_is_a_click() {
        let ctx = egui::Context::default();
        let mut rect = egui::Rect::NOTHING;
        let mut clicks = 0;
        let frame = |events: Vec<egui::Event>, rect: &mut egui::Rect, clicks: &mut i32| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| {
                egui::Area::new(egui::Id::new("a"))
                    .fixed_pos([20.0, 20.0])
                    .show(ui.ctx(), |ui| {
                        let r = ui.button("Button");
                        *rect = r.rect;
                        if r.clicked() {
                            *clicks += 1;
                        }
                    });
            });
            out.drop_without_applying_deltas();
        };
        // A new area is invisible in the first frame (size determination).
        frame(Vec::new(), &mut rect, &mut clicks);
        frame(Vec::new(), &mut rect, &mut clicks);
        let pos = rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            vec![egui::Event::PointerMoved(pos), button(true), button(false)],
            &mut rect,
            &mut clicks,
        );
        assert_eq!(clicks, 1);
    }

    /// The viewer draws no egui line to the rally point (the flag is the model in the world);
    /// the coordinates stay in the "Economy" window.
    #[test]
    fn the_viewer_draws_no_rally_line() {
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let model = host.panel(&[hq]);
        let view = |model| EconomyView {
            rules: &host.rules,
            model,
            placement: None,
            preview: None,
            sell_armed: false,
            footprint: Vec::new(),
            anchor: None,
            counter_upgrades: Vec::new(),
            hire_back: String::new(),
        };
        let ctx = egui::Context::default();
        let mut open = true;
        let mut actions = Vec::new();
        // Window closed, HQ selected: no overlay.
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            build(ui.ctx(), &mut open, &view(None), &mut actions)
        });
        assert!(out.shapes.is_empty(), "{:?}", out.shapes.len());
        out.drop_without_applying_deltas();
        // Window open: the row with the coordinates (a new area is invisible in the first frame).
        let mut texts = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                build(ui.ctx(), &mut open, &view(Some(&model)), &mut actions)
            });
            texts = out
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            out.drop_without_applying_deltas();
        }
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with("Rally point (30.50, 27.50) tiles")),
            "{texts:?}"
        );
    }

    #[test]
    fn the_window_and_overlays_build_without_a_gpu() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let model = host.panel(&[worker]);
        let preview = host
            .placement_preview(1001, Vec3::new(1000.0, 0.0, 1000.0), 0, &[worker])
            .unwrap();
        // Top view: map 0...8192 onto screen space -1...1 (x -> x, z -> -y).
        let view_proj = Mat4::from_cols(
            glam::Vec4::new(1.0 / 4096.0, 0.0, 0.0, 0.0),
            glam::Vec4::ZERO,
            glam::Vec4::new(0.0, -1.0 / 4096.0, 0.0, 0.0),
            glam::Vec4::new(-1.0, 1.0, 0.5, 1.0),
        );
        let footprint = footprint_outline(&host.rules, &preview, view_proj, (800, 600));
        // Rectangle 7..=8 x 7..=8: two tiles per side, eight border points.
        assert_eq!(footprint.len(), 8);
        let placement = Placement {
            guid: 1001,
            steps: 0,
        };
        let view = EconomyView {
            rules: &host.rules,
            model: Some(&model),
            placement: Some(&placement),
            preview: Some(&preview),
            sell_armed: false,
            footprint,
            anchor: anchor_point(&host.rules, &preview, view_proj, (800, 600)),
            counter_upgrades: vec![CounterUpgradeRow {
                text: "Plague 1:59".into(),
                color: [255, 0, 0],
            }],
            hire_back: "Back".into(),
        };
        let ctx = egui::Context::default();
        let mut open = true;
        let mut actions = Vec::new();
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            build(ui.ctx(), &mut open, &view, &mut actions)
        });
        assert!(!out.shapes.is_empty());
        out.drop_without_applying_deltas();
        assert!(actions.is_empty());
    }

    #[test]
    fn running_counter_upgrades_show_their_rest_time() {
        use nml_client::game::unit_bar::testkit::{MISSIONARY, host, spawn};
        use nml_sim::command::CommandKind;
        let mut host = host();
        let missionary = spawn(&mut host, MISSIONARY, 0, (10, 10));
        host.state.add_guid_count(0, 3354, 1);
        host.state.players[0].color = 3;
        let texts: BTreeMap<i32, String> = [(3374, "Execute chastity belt".to_string())]
            .into_iter()
            .collect();
        assert!(counter_upgrade_rows(&host, &texts).is_empty());
        host.submit_own(CommandKind::CounterUpgrade {
            guid: 3374,
            source: missionary,
        });
        // Ten phase 0 ticks (deduction from the tick of the command): 120 s - 1 s. The phase 0 block
        // runs in the `step` that lowers the phase from 1 to 0.
        let mut phase0 = 0;
        while phase0 < 10 {
            if host.state.phase == 1 {
                phase0 += 1;
            }
            host.step_once();
        }
        assert_eq!(host.state.counter_upgrades[0].rest_ms, 119_000);
        let before = nml_sim::save::checksum(&host.state);
        let rows = counter_upgrade_rows(&host, &texts);
        assert_eq!(nml_sim::save::checksum(&host.state), before);
        assert_eq!(
            rows,
            [CounterUpgradeRow {
                text: "Execute chastity belt 1:59".into(),
                color: nml_world::markers::player_rgb(3),
            }]
        );
        // `%d:%02d` with `rest / 60000` and `(rest / 1000) % 60` (`0x00455C00`), name as a substitute.
        host.state.counter_upgrades[0].rest_ms = 9_999;
        assert_eq!(
            counter_upgrade_rows(&host, &BTreeMap::new())[0].text,
            "Counter-upgrade 3374 0:09"
        );
    }

    /// A frame with the rows of the queue of `building` (as in the window, here in a fixed area): clicks
    /// and per row the areas of "x" and debug button.
    fn queue_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        panel: &BuildingPanel,
    ) -> (Vec<(egui::Rect, egui::Rect)>, Vec<PanelAction>) {
        let mut actions = Vec::new();
        let mut rects = Vec::new();
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| {
            egui::Area::new(egui::Id::new("q"))
                .fixed_pos([20.0, 20.0])
                .show(ui.ctx(), |ui| {
                    rects = queue_rows(ui, panel, &mut actions);
                });
        });
        out.drop_without_applying_deltas();
        (rects, actions)
    }

    /// Click (move, press, release in one frame) on `pos`.
    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    #[test]
    fn the_debug_complete_button_sits_apart_next_to_cancel_and_sends_the_debug_command() {
        use nml_client::game::economy::EconomyUi;
        use nml_sim::command::CommandKind;
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let foreign = spawn(&mut host, HQ, 1, (50, 50));
        for _ in 0..2 {
            host.submit_own(CommandKind::EnqueueProduction {
                building: hq,
                guid: COLONIST,
            });
        }
        host.step_once();
        let panel = host.panel(&[hq]).building.expect("building part");
        assert!(panel.debug_complete);
        let ctx = egui::Context::default();
        queue_frame(&ctx, Vec::new(), &panel);
        let (rects, _) = queue_frame(&ctx, Vec::new(), &panel);
        assert_eq!(rects.len(), 2);
        // Per row right of "x", at the same height, with a clear gap (separator).
        for (cancel, debug) in &rects {
            assert!(
                debug.left() >= cancel.right() + 10.0,
                "{cancel:?} {debug:?}"
            );
            assert!((debug.center().y - cancel.center().y).abs() < 2.0);
        }
        // "x" still cancels; the debug button of the waiting order (index 1) completes it.
        let (_, actions) = queue_frame(&ctx, click_at(rects[1].0.center()), &panel);
        assert_eq!(actions, [PanelAction::Cancel(1)]);
        let (_, actions) = queue_frame(&ctx, click_at(rects[1].1.center()), &panel);
        assert_eq!(
            actions,
            [PanelAction::DebugComplete {
                index: 1,
                guid: COLONIST
            }]
        );
        let mut ui = EconomyUi::default();
        ui.apply(&mut host, &[hq], actions[0]);
        assert_eq!(
            host.pending_commands()[0].kind,
            CommandKind::DebugCompleteOrder {
                building: hq,
                index: 1,
                guid: COLONIST,
            }
        );
        host.step_once();
        let players = host.query(None, None, None, 1).players;
        assert_eq!(players[0].counts.get(&COLONIST), Some(&1));
        assert_eq!(host.production_queue(hq).len(), 1);
        // Foreign building: no building part (so no button); a click stays without a command.
        host.active_player = 1;
        host.submit_own(CommandKind::EnqueueProduction {
            building: foreign,
            guid: COLONIST,
        });
        host.step_once();
        host.active_player = 0;
        assert!(host.panel(&[foreign]).building.is_none());
        let action = PanelAction::DebugComplete {
            index: 0,
            guid: COLONIST,
        };
        ui.apply(&mut host, &[foreign], action);
        assert!(host.pending_commands().is_empty());
        // Own but sleeping building (state `DIED`, like A8 `validate::object_is_dead`): button disabled, a click
        // stays without a command.
        host.state.object_mut(hq, false).unwrap().status |= nml_sim::object::status::DIED;
        let panel = host.panel(&[hq]).building.expect("building part");
        assert!(!panel.debug_complete);
        queue_frame(&ctx, Vec::new(), &panel);
        let (rects, _) = queue_frame(&ctx, Vec::new(), &panel);
        assert_eq!(rects.len(), 1);
        let (_, actions) = queue_frame(&ctx, click_at(rects[0].1.center()), &panel);
        assert!(actions.is_empty());
        ui.apply(&mut host, &[hq], action);
        assert!(host.pending_commands().is_empty());
    }

    /// The headhunter menu draws all three levels: kinds with price, then target players, then targets;
    /// without a click without an action.
    #[test]
    fn the_hire_menu_draws_all_three_levels() {
        use nml_client::game::economy::{EconomyUi, HireMenu};
        use nml_client::game::testworld::{HQ_SPA, UNDERTAKER, hunter_host};
        let mut host = hunter_host();
        let hq = nml_client::game::testworld::spawn(&mut host, HQ_SPA, 0, (20, 20));
        nml_client::game::testworld::spawn(&mut host, UNDERTAKER, 1, (46, 30));
        let mut ui = EconomyUi::default();
        let ctx = egui::Context::default();
        for action in [
            None,
            Some(PanelAction::HireArt(2)),
            Some(PanelAction::HirePlayer(1)),
        ] {
            if let Some(action) = action {
                ui.apply(&mut host, &[hq], action);
            }
            let hire = host
                .panel_with_hire(&[hq], &ui.hire)
                .building
                .and_then(|b| b.hire)
                .expect("headhunter menu");
            // A new area does not draw until the second frame (size determination in the first).
            let mut actions = Vec::new();
            let mut shapes = 0;
            for _ in 0..2 {
                let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    egui::Area::new(egui::Id::new("h"))
                        .fixed_pos([20.0, 20.0])
                        .show(ui.ctx(), |ui| hire_rows(ui, &hire, "Back", &mut actions));
                });
                shapes = out.shapes.len();
                out.drop_without_applying_deltas();
            }
            assert!(shapes > 0);
            assert!(actions.is_empty());
        }
        assert_ne!(ui.hire, HireMenu::default());
    }
}
