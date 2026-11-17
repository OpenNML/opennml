//! egui window "Unit": the unit bar (formation 511-515, scatter 519, train 449-451,
//! arm 485/486, abilities, counter-upgrades 3370-3377). Content and actions come from
//! [`nml_client::game::unit_bar`]; this file only handles drawing and clicks. Locked buttons (`enabled = false`)
//! are grayed out and take no click.

use nml_client::game::unit_bar::{BarAction, BarButton};

/// What the window shows per frame.
pub struct BarView {
    /// Header line ("Current formation: ...").
    pub header: String,
    /// Buttons; empty = no window.
    pub buttons: Vec<BarButton>,
}

/// Builds the window; clicks end up in `actions` (in click order).
pub fn build(ctx: &egui::Context, view: &BarView, actions: &mut Vec<BarAction>) {
    if view.buttons.is_empty() {
        return;
    }
    egui::Window::new("Unit")
        .anchor(egui::Align2::LEFT_BOTTOM, [16.0, -16.0])
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            // A locomotive alone has no formation: without a header line only the train buttons.
            if !view.header.is_empty() {
                ui.label(&view.header);
            }
            // Wrap instead of running past the screen edge: the missionary carries formation, scatter and
            // eight counter-upgrades.
            ui.horizontal_wrapped(|ui| {
                for button in &view.buttons {
                    let response = ui.add_enabled(
                        button.enabled,
                        egui::Button::new(&button.label).selected(button.active),
                    );
                    if response.clicked()
                        && let Some(action) = BarAction::from_button(button.id)
                    {
                        actions.push(action);
                    }
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_client::game::unit_bar::{SelectionSummary, UnitBar, buttons, header};
    use std::collections::BTreeMap;

    fn texts() -> BTreeMap<i32, String> {
        [
            (510, "Current formation: [FORMATION]"),
            (511, "none"),
            (512, "Line"),
            (513, "Square"),
            (514, "Column"),
            (515, "Pincer"),
            (519, "Scatter"),
        ]
        .into_iter()
        .map(|(k, v)| (k, v.to_string()))
        .collect()
    }

    /// One frame with the events `events`; returns the actions and the area of each button.
    fn frame(
        ctx: &egui::Context,
        view: &BarView,
        events: Vec<egui::Event>,
    ) -> (Vec<BarAction>, egui::FullOutput) {
        let mut actions = Vec::new();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| build(ui.ctx(), view, &mut actions));
        (actions, out)
    }

    #[test]
    fn the_window_builds_without_a_gpu_and_a_click_on_a_button_gives_its_action() {
        let texts = texts();
        let bar = UnitBar {
            formation: 1,
            ..UnitBar::default()
        };
        let view = BarView {
            header: header(&bar, &texts),
            buttons: buttons(
                &bar,
                &texts,
                &SelectionSummary {
                    own_units: 2,
                    ..SelectionSummary::default()
                },
            ),
        };
        let ctx = egui::Context::default();
        // A new area is invisible in the first frame (size determination).
        frame(&ctx, &view, Vec::new())
            .1
            .drop_without_applying_deltas();
        let (actions, out) = frame(&ctx, &view, Vec::new());
        assert!(actions.is_empty() && !out.shapes.is_empty());
        // The area of the button "Column" from the drawing: its text.
        let target = out.shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(t) if t.galley.text() == "Column" => {
                Some(t.visual_bounding_rect().center())
            }
            _ => None,
        });
        out.drop_without_applying_deltas();
        let pos = target.expect("button \"Column\" drawn");
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let (actions, out) = frame(
            &ctx,
            &view,
            vec![egui::Event::PointerMoved(pos), button(true), button(false)],
        );
        out.drop_without_applying_deltas();
        assert_eq!(actions, [BarAction::Formation(3)]);
    }

    #[test]
    fn a_locked_button_takes_no_click() {
        use nml_client::game::unit_bar::BarButton;
        let button = |id, label: &str, enabled| BarButton {
            id,
            label: label.into(),
            active: false,
            enabled,
        };
        let view = BarView {
            header: String::new(),
            buttons: vec![
                button(485, "Arm", false),
                button(3374, "Chastity belt", true),
            ],
        };
        let ctx = egui::Context::default();
        frame(&ctx, &view, Vec::new())
            .1
            .drop_without_applying_deltas();
        let (_, out) = frame(&ctx, &view, Vec::new());
        let at = |text: &str| {
            out.shapes.iter().find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) if t.galley.text() == text => {
                    Some(t.visual_bounding_rect().center())
                }
                _ => None,
            })
        };
        let (locked, free) = (at("Arm").unwrap(), at("Chastity belt").unwrap());
        out.drop_without_applying_deltas();
        let click = |pos| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            vec![egui::Event::PointerMoved(pos), button(true), button(false)]
        };
        let (actions, out) = frame(&ctx, &view, click(locked));
        out.drop_without_applying_deltas();
        assert!(actions.is_empty(), "locked: {actions:?}");
        let (actions, out) = frame(&ctx, &view, click(free));
        out.drop_without_applying_deltas();
        assert_eq!(actions, [BarAction::CounterUpgrade(3374)]);
    }

    #[test]
    fn a_long_bar_wraps_inside_the_screen() {
        // Missionary with formation, scatter and the eight counter-upgrades 3370-3377: the buttons wrap into
        // the next line instead of running past the screen edge.
        use nml_client::game::unit_bar::BarButton;
        let labels = [
            "none",
            "Line",
            "Square",
            "Column",
            "Pincer",
            "Scatter",
            "Execute plague",
            "Execute greed",
            "Execute bark beetle",
            "Execute firewater",
            "Execute chastity belt",
            "Execute weakness",
            "Execute termites",
            "Execute throw off",
        ];
        let view = BarView {
            header: "Current formation: none".into(),
            buttons: labels
                .iter()
                .enumerate()
                .map(|(i, label)| BarButton {
                    id: 3364 + i as i32,
                    label: (*label).into(),
                    active: false,
                    enabled: true,
                })
                .collect(),
        };
        let ctx = egui::Context::default();
        frame(&ctx, &view, Vec::new())
            .1
            .drop_without_applying_deltas();
        let (_, out) = frame(&ctx, &view, Vec::new());
        let rects: Vec<(String, egui::Rect)> = out
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) if labels.contains(&t.galley.text()) => {
                    Some((t.galley.text().to_string(), t.visual_bounding_rect()))
                }
                _ => None,
            })
            .collect();
        out.drop_without_applying_deltas();
        assert_eq!(rects.len(), labels.len(), "all buttons drawn");
        for (label, rect) in &rects {
            assert!(
                rect.max.x <= 800.0,
                "\"{label}\" extends past the screen edge: {rect:?}"
            );
        }
    }

    #[test]
    fn without_buttons_there_is_no_window() {
        let view = BarView {
            header: String::new(),
            buttons: Vec::new(),
        };
        let ctx = egui::Context::default();
        let (actions, out) = frame(&ctx, &view, Vec::new());
        assert!(actions.is_empty() && out.shapes.is_empty());
        out.drop_without_applying_deltas();
    }
}
