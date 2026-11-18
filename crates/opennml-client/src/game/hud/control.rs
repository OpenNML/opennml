//! Control channel of the game UI: the elements of `ui_click` on `GAMEUI` and the report
//! `ui_state.game.hud.toolbar`. Both read only the state of the last frame: the rectangles of the buttons from the
//! bar (frame step), the rectangles of portrait, values and counter-upgrades from the tooltip areas of the last
//! drawing ([`Hud::areas`]), the box of the tooltip from [`Hud::tooltip_box`].
//!
//! # Element names
//!
//! | Name | Rectangle |
//! |---|---|
//! | `normal:<0..13>` | normal slot of the bar |
//! | `guid:<msg>` | first normal button with this `BUTTONMSG` |
//! | `queue:<i>` | production; 0 = running order, 1 ... waiting (the kind bits `& 0xFF`, like the cancel command) |
//! | `selection:<i>` | multi-selection, `i` = index of the object in the selection (kind bits `& 0xFF`) |
//! | `stance:<i>` | stance 420 + i |
//! | `formation:<i>` | formation button in the order of the bar |
//! | `hero:<i>`, `headhunter:<i>` | slot `i` of the hero row or the headhunter row |
//! | `portrait` | area of the portrait |
//! | `value:<i>` | `i`-th value row of the portrait |
//! | `counterupgrade:<i>` | `i`-th counter-upgrade |
//! | `statusbar` | status bar |
//! | `toolbar` | free bar area `(630, H - 86)` |
//! | `minimap` | center of the diamond of the minimap `(W - 89, H - 89)` |
//! | `minimap_mode` | mode button (573/574/575) |
//! | `minimap_idle` | button 576 (one idle worker) |
//! | `minimap_signal` | button 571 (signal) |
//! | `minimap_idle_all` | button 577 (all idle workers) |
//! | `minimap_menu` | button 579 (game menu) |
//! | `minimap_diplomacy` | button 580 (diplomacy; only outside the campaign) |
//!
//! The names of the minimap apply before those of the bar: they hang on the node of the minimap, not on the command
//! bar, and also work without a bar and with a hidden bar (the buttons then take the click but have no effect,
//! O-292).

use opennml_control::{UiPortraitData, UiToolbarButton, UiToolbarData, UiTooltipData};
use opennml_ui::assets::Texts;
use opennml_ui::hud::portrait::{
    GROUP_TEXT, NATION_SYMBOL_BASE, PortraitModel, VALUE_ORDER, value_text,
};
use opennml_ui::hud::toolbar::button::{Button, ButtonState};
use opennml_ui::hud::toolbar::minimap_buttons::{
    MSG_DIPLOMACY_BUTTON, MSG_GAME_MENU, MSG_IDLE_ALL, MSG_IDLE_ONE, MSG_SIGNAL, mode_of,
};
use opennml_ui::hud::toolbar::set::MSG_STANCE_FIRST;
use opennml_ui::hud::toolbar::{Cursor, Toolbar};
use opennml_ui::hud::tree::WindowKind;
use opennml_ui::text::{GameText, TextContext};

use super::Hud;
use crate::game::minimap::draw::CENTER_INSET;

/// Error message for a name that `GAMEUI` does not know.
fn unknown(element: &str) -> String {
    format!("element {element} does not exist in GAMEUI")
}

/// Area of the free bar for the element `toolbar`: `x` fixed, `y` = screen height - 86 (center of the bar).
const FREE_BAR_X: i32 = 630;
const FREE_BAR_FROM_BOTTOM: i32 = 86;

/// Center of a rectangle `[l, t, r, b]` (integer, rounded down).
fn center([l, t, r, b]: [i32; 4]) -> [i32; 2] {
    [l + (r - l) / 2, t + (b - t) / 2]
}

/// All buttons with their element names in child order: normal slots, production, multi-selection, stance,
/// formation, heroes, headhunters.
fn named_buttons(toolbar: &Toolbar) -> Vec<(String, &Button)> {
    let set = &toolbar.set;
    let mut out = Vec::new();
    for (i, b) in set.normal.iter().enumerate() {
        if let Some(b) = b {
            out.push((format!("normal:{i}"), b));
        }
    }
    for b in &set.production {
        out.push((format!("queue:{}", b.spec.flags & 0xFF), b));
    }
    for b in &set.selection {
        out.push((format!("selection:{}", b.spec.flags & 0xFF), b));
    }
    for b in &set.stance {
        out.push((format!("stance:{}", b.spec.msg - MSG_STANCE_FIRST), b));
    }
    for (i, b) in set.formation.iter().enumerate() {
        out.push((format!("formation:{i}"), b));
    }
    for (i, slot) in toolbar.heroes.slots.iter().enumerate() {
        if let Some(b) = &slot.button {
            out.push((format!("hero:{i}"), b));
        }
    }
    for (i, slot) in toolbar.headhunters.slots.iter().enumerate() {
        if let Some(b) = &slot.button {
            out.push((format!("headhunter:{i}"), b));
        }
    }
    out
}

/// State `+0x90` as a number 0 ... 3.
fn state_number(state: ButtonState) -> u8 {
    match state {
        ButtonState::Normal => 0,
        ButtonState::Hover => 1,
        ButtonState::Pressed => 2,
        ButtonState::Locked => 3,
    }
}

/// Name of the open cursor in the report.
fn cursor_name(cursor: &Cursor) -> &'static str {
    match cursor {
        Cursor::Build(b) if b.tent.is_some() => "tent",
        Cursor::Build(_) => "build",
        Cursor::Fort(_) => "wall",
        Cursor::Target(_) => "target",
        Cursor::Defense(_) => "defense",
    }
}

/// Remaining time of a counter-upgrade as drawn (`m:ss`, `draw_counter_row`).
fn rest_text(rest_ms: i32) -> String {
    format!("{}:{:02}", rest_ms / 60_000, (rest_ms / 1000) % 60)
}

impl Hud {
    /// Center of the element `element` (names in the module header) in UI pixels, from the state of the last frame.
    /// Error message: unknown name, element not present or hidden bar.
    pub fn element_center(&self, element: &str) -> Result<[i32; 2], String> {
        if let Some(point) = self.minimap_element_center(element) {
            return point.ok_or_else(|| unknown(element));
        }
        if element == "statusbar" {
            let window = self
                .windows
                .find(WindowKind::StatusBar)
                .ok_or_else(|| unknown(element))?;
            return Ok(center(self.windows.rect(window)));
        }
        let toolbar = self.toolbar.as_ref().ok_or_else(|| unknown(element))?;
        let (name, number) = match element.split_once(':') {
            Some((name, number)) => (
                name,
                Some(number.parse::<i64>().map_err(|_| unknown(element))?),
            ),
            None => (element, None),
        };
        let index = || {
            number
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| unknown(element))
        };
        let rect = match (name, number) {
            ("toolbar", None) => {
                if !toolbar.shown {
                    return Err("command bar is hidden".to_string());
                }
                return Ok([FREE_BAR_X, self.screen.height - FREE_BAR_FROM_BOTTOM]);
            }
            ("guid", Some(msg)) => toolbar
                .set
                .normal
                .iter()
                .flatten()
                .find(|b| i64::from(b.spec.msg) == msg)
                .map(|b| b.spec.rect),
            ("portrait", None) => self.areas.borrow().portrait_areas().first().map(|a| a.0),
            ("value", _) => {
                let i = index()?;
                self.areas.borrow().portrait_areas().get(i + 1).map(|a| a.0)
            }
            ("counterupgrade", _) => {
                let i = index()?;
                self.areas.borrow().counter_areas().get(i).map(|a| a.0)
            }
            (_, Some(_)) => {
                let wanted = format!("{name}:{}", index()?);
                named_buttons(toolbar)
                    .into_iter()
                    .find(|(n, _)| *n == wanted)
                    .map(|(_, b)| b.spec.rect)
            }
            _ => None,
        };
        let rect = rect.ok_or_else(|| unknown(element))?;
        if !toolbar.shown {
            return Err("command bar is hidden".to_string());
        }
        Ok(center(rect))
    }

    /// Center of the diamond of the minimap in UI pixels: the position of the last frame, before the first frame
    /// from the screen size `(W - 89, H - 89)` (`0x0044A710`, re/game-ui D5.1). Only with a minimap node and the
    /// minimap of the host.
    pub fn minimap_center(&self) -> Option<[i32; 2]> {
        self.windows.find(WindowKind::Minimap)?;
        let map = self.minimap.as_ref()?;
        Some(match map.last_layout() {
            // The center is an integer in `f32` (`W - 89`).
            Some(layout) => layout.center.map(|v| v.round() as i32),
            None => [
                self.screen.width - CENTER_INSET as i32,
                self.screen.height - CENTER_INSET as i32,
            ],
        })
    }

    /// Center of the minimap elements (`minimap`, `minimap_mode`, `minimap_idle`, `minimap_signal`,
    /// `minimap_idle_all`, `minimap_menu`, `minimap_diplomacy`, module header); `None` if `element` is none of these
    /// names, `Some(None)` for a name without an element (map or button not created: map without `IsMap`, buttons
    /// removed by a lock or a hidden map, button 580 in the campaign).
    fn minimap_element_center(&self, element: &str) -> Option<Option<[i32; 2]>> {
        let pick: fn(i32) -> bool = match element {
            "minimap" => return Some(self.minimap_center()),
            "minimap_mode" => |msg| mode_of(msg).is_some(),
            "minimap_idle" => |msg| msg == MSG_IDLE_ONE,
            "minimap_signal" => |msg| msg == MSG_SIGNAL,
            "minimap_idle_all" => |msg| msg == MSG_IDLE_ALL,
            "minimap_menu" => |msg| msg == MSG_GAME_MENU,
            "minimap_diplomacy" => |msg| msg == MSG_DIPLOMACY_BUTTON,
            _ => return None,
        };
        Some(
            self.minimap_buttons
                .buttons
                .iter()
                .find(|b| pick(b.spec.msg))
                .map(|b| center(b.spec.rect)),
        )
    }

    /// Report of the command bar for `ui_state.game.hud.toolbar` ([`UiToolbarData`]); without a bar `None`.
    /// `portrait`, `game` and `keys` are the snapshots of drawing ([`super::BarFrame`]; `GameSession::bar_texts`
    /// builds them for drawing and report together), `texts` supplies the subtitle of the portrait, `last_button`
    /// comes from the session. Read only.
    pub fn toolbar_report(
        &self,
        portrait: &PortraitModel,
        game: &dyn GameText,
        keys: &TextContext,
        texts: &Texts,
        last_button: Option<i32>,
    ) -> Option<UiToolbarData> {
        let toolbar = self.toolbar.as_ref()?;
        let buttons = named_buttons(toolbar)
            .into_iter()
            .map(|(element, b)| UiToolbarButton {
                element,
                msg: b.spec.msg,
                kind: b.spec.flags,
                state: state_number(b.state),
                rect: b.spec.rect,
                // `f32` -> whole percent, so that the report stays `Eq` and can be compared bit-identically.
                progress: (b.progress() * 100.0).round() as i32,
            })
            .collect();
        let tooltip = self.tooltip.request.map(|r| UiTooltipData {
            guid: r.guid,
            kind: r.kind,
            text: self
                .tooltip
                .text(game, keys, self.enhanced_tooltips)
                .unwrap_or_default(),
            rect: self.tooltip_box.get().unwrap_or([0; 4]),
        });
        Some(UiToolbarData {
            shown_menu: toolbar.shown_menu(),
            selected: toolbar.selected,
            hover: toolbar.hover,
            buttons,
            portrait: portrait_report(toolbar, portrait, texts),
            tooltip,
            counter_upgrades: portrait
                .counter_upgrades
                .iter()
                .map(|c| (c.guid as i32, rest_text(c.rest_ms)))
                .collect(),
            cursor: toolbar.cursor.as_ref().map(|c| cursor_name(c).to_string()),
            last_button,
        })
    }
}

/// Portrait as `draw_portrait` draws it (`0x00456B70`): nothing for selection and mouse 0; the group with text 500;
/// otherwise the object with value rows on the slots 2 and following of `PORTRAITPOSITION`. If the portrait entry
/// of the type is missing, everything is dropped.
fn portrait_report(
    toolbar: &Toolbar,
    model: &PortraitModel,
    texts: &Texts,
) -> Option<UiPortraitData> {
    let (selected, hover, data) = (toolbar.selected, toolbar.hover, &toolbar.data);
    if (selected == 0 && hover == 0) || data.portrait.is_empty() {
        return None;
    }
    let subtitle_slot = data.portrait.get(1).is_some();
    let subtitle = |guid: i32| {
        if subtitle_slot {
            texts.display(guid)
        } else {
            String::new()
        }
    };
    if selected == -1 && hover == 0 {
        if let Some(nation) = model.group_nation {
            data.portrait_of(NATION_SYMBOL_BASE.wrapping_add(nation))?;
        }
        return Some(UiPortraitData {
            guid: GROUP_TEXT,
            subtitle: subtitle(GROUP_TEXT),
            values: Vec::new(),
        });
    }
    let object = model.object.as_ref()?;
    let guid = object.guid as i32;
    data.portrait_of(guid)?;
    let mut places = data.portrait.len().saturating_sub(2);
    let mut values = Vec::new();
    if subtitle_slot {
        for value in &object.values {
            if !VALUE_ORDER.iter().any(|k| k.attribute == value.attribute) {
                continue;
            }
            if places == 0 {
                break;
            }
            places -= 1;
            values.push((value.attribute, value_text(value)));
        }
    }
    Some(UiPortraitData {
        guid,
        subtitle: subtitle(guid),
        values,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use opennml_formats::gifcon::button_kind as kinds;
    use opennml_formats::messages::parse_messages;
    use opennml_ui::coords::Screen;
    use opennml_ui::hud::HudEventKind;
    use opennml_ui::hud::portrait::CounterRow;
    use opennml_ui::hud::toolbar::button::{
        ButtonSpec, KIND_HERO, KIND_PRODUCTION, KIND_SELECTION, KIND_STANCE, SheetRef,
        TooltipRequest,
    };
    use opennml_ui::hud::toolbar::cursor::{BuildCursor, DefenseCursor, FortCursor, TargetCursor};
    use opennml_ui::hud::toolbar::data::ToolbarData;
    use opennml_ui::hud::toolbar::set::{KIND_HEADHUNTER, RowSlot};

    use super::*;
    use crate::game::economy::testkit::host;
    use crate::game::hud::{HudOptions, SimGameText};

    const SCREEN: Screen = Screen {
        width: 1024,
        height: 768,
    };

    fn texts() -> Texts {
        let vfs = opennml_rda::Vfs::from_archives(Vec::new());
        Texts::load(
            &vfs,
            &opennml_formats::localisation::Localisation::default(),
        )
    }

    fn hud_with(toolbar: Toolbar) -> Hud {
        let mut hud = Hud::new(
            &parse_messages(""),
            &BTreeMap::new(),
            &HudOptions::default(),
            SCREEN,
        );
        hud.toolbar = Some(toolbar);
        hud
    }

    fn empty_toolbar() -> Toolbar {
        Toolbar::new(ToolbarData::from_texts("", ""), SCREEN)
    }

    fn spec(msg: i32, flags: u32, rect: [i32; 4], progress: f32) -> ButtonSpec {
        ButtonSpec {
            msg,
            flags,
            required: [0; 5],
            rect,
            base: (SheetRef::Resource, 0),
            icon: (SheetRef::Resource, 0),
            scale: 1.0,
            progress,
            object: None,
        }
    }

    fn button(msg: i32, flags: u32, rect: [i32; 4], progress: f32) -> Button {
        Button::new(spec(msg, flags, rect, progress), None)
    }

    fn report(hud: &Hud, portrait: &PortraitModel) -> UiToolbarData {
        let (host, texts, keys) = (host(), texts(), TextContext::default());
        let game = SimGameText::new(&host, &texts);
        hud.toolbar_report(portrait, &game, &keys, &texts, Some(534))
            .expect("report")
    }

    /// Every family of buttons carries its name and its fields: normal slots (`normal:<slot>`), production and
    /// multi-selection by the kind bits `& 0xFF`, stance by `msg - 420`, formation by the order, heroes and
    /// headhunters by the slot; plus state 0 ... 3 and the progress in whole percent.
    #[test]
    fn every_button_family_is_named_and_reported_exactly() {
        let mut tb = empty_toolbar();
        let (r1, r2, r3) = (
            [10, 600, 60, 650],
            [70, 600, 120, 650],
            [130, 600, 180, 650],
        );
        tb.set.normal[3] = Some(button(534, kinds::MAKE, r1, 1.0));
        // Created locked (BUTTONMSG < 0, state 3).
        tb.set.normal[5] = Some(button(-7, kinds::DO, r2, 1.0));
        // Mouse over it (1), then pressed (2).
        let mut pressed = Button::new(spec(409, kinds::DO, r3, 1.0), Some([140, 610]));
        assert!(pressed.mouse(HudEventKind::LeftDown, [140, 610], &mut Vec::new()));
        tb.set.normal[13] = Some(pressed);
        tb.set.production = vec![
            button(1410, KIND_PRODUCTION, [1, 1, 51, 51], 0.42),
            button(1411, KIND_PRODUCTION | 3, [2, 2, 27, 27], 1.0),
        ];
        tb.set.selection = vec![
            button(65537, KIND_SELECTION, [3, 3, 28, 28], 1.0),
            button(65538, KIND_SELECTION | 5, [4, 4, 29, 29], 1.0),
        ];
        tb.set.stance = vec![
            button(420, KIND_STANCE, [5, 5, 30, 30], 1.0),
            button(423, KIND_STANCE | 4, [6, 6, 31, 31], 1.0),
        ];
        tb.set.formation = vec![
            button(510, kinds::DO, [7, 7, 32, 32], 1.0),
            button(519, kinds::DO, [8, 8, 33, 33], 1.0),
        ];
        let slot = |id, button| RowSlot {
            pos: None,
            id,
            button,
        };
        tb.heroes.slots = vec![
            slot(0, None),
            slot(77, Some(button(77, KIND_HERO, [9, 9, 59, 59], 1.0))),
        ];
        tb.headhunters.slots = vec![slot(
            4100,
            Some(button(4100, KIND_HEADHUNTER, [10, 10, 60, 60], 1.0)),
        )];
        let hud = hud_with(tb);
        let got: Vec<_> = report(&hud, &PortraitModel::default())
            .buttons
            .into_iter()
            .map(|b| (b.element, b.msg, b.kind, b.state, b.rect, b.progress))
            .collect();
        let want = [
            ("normal:3", 534, kinds::MAKE, 0, r1, 100),
            ("normal:5", -7, kinds::DO, 3, r2, 100),
            ("normal:13", 409, kinds::DO, 2, r3, 100),
            ("queue:0", 1410, KIND_PRODUCTION, 0, [1, 1, 51, 51], 42),
            ("queue:3", 1411, KIND_PRODUCTION | 3, 0, [2, 2, 27, 27], 100),
            ("selection:0", 65537, KIND_SELECTION, 0, [3, 3, 28, 28], 100),
            (
                "selection:5",
                65538,
                KIND_SELECTION | 5,
                0,
                [4, 4, 29, 29],
                100,
            ),
            ("stance:0", 420, KIND_STANCE, 0, [5, 5, 30, 30], 100),
            ("stance:3", 423, KIND_STANCE | 4, 0, [6, 6, 31, 31], 100),
            ("formation:0", 510, kinds::DO, 0, [7, 7, 32, 32], 100),
            ("formation:1", 519, kinds::DO, 0, [8, 8, 33, 33], 100),
            ("hero:1", 77, KIND_HERO, 0, [9, 9, 59, 59], 100),
            (
                "headhunter:0",
                4100,
                KIND_HEADHUNTER,
                0,
                [10, 10, 60, 60],
                100,
            ),
        ]
        .map(|(e, m, k, s, r, p)| (e.to_string(), m, k, s, r, p));
        assert_eq!(got, want);
        // Every name leads to the center of its rectangle; `guid:` finds normal buttons (also locked ones).
        for (name, _, _, _, rect, _) in &want {
            assert_eq!(hud.element_center(name), Ok(center(*rect)), "{name}");
        }
        assert_eq!(hud.element_center("guid:534"), Ok(center(r1)));
        assert_eq!(hud.element_center("guid:-7"), Ok(center(r2)));
        for bad in [
            "hero:0",
            "queue:1",
            "selection:1",
            "stance:1",
            "normal:4",
            "guid:1",
        ] {
            assert_eq!(
                hud.element_center(bad),
                Err(format!("element {bad} does not exist in GAMEUI")),
                "{bad}"
            );
        }
    }

    /// Counter-upgrades as `m:ss`, the names of the cursors, `hover` and the last button.
    #[test]
    fn counter_upgrades_cursor_hover_and_last_button_are_reported() {
        assert_eq!(rest_text(0), "0:00");
        assert_eq!(rest_text(999), "0:00");
        assert_eq!(rest_text(65_000), "1:05");
        assert_eq!(rest_text(3_599_000), "59:59");
        let row = |guid, rest_ms| CounterRow {
            guid,
            rest_ms,
            source_color: None,
            enemy: false,
        };
        let model = PortraitModel {
            counter_upgrades: vec![row(602, 65_000), row(700, 3_599_000), row(701, 999)],
            ..PortraitModel::default()
        };
        let mut tb = empty_toolbar();
        tb.hover = 77;
        let mut hud = hud_with(tb.clone());
        let r = report(&hud, &model);
        assert_eq!(
            r.counter_upgrades,
            [
                (602, "1:05".to_string()),
                (700, "59:59".to_string()),
                (701, "0:00".to_string())
            ]
        );
        assert_eq!((r.hover, r.selected, r.shown_menu), (77, 0, 0));
        assert_eq!((r.cursor.as_deref(), r.last_button), (None, Some(534)));
        let cursors = [
            (Cursor::Build(BuildCursor::new(1)), "build"),
            (Cursor::Build(BuildCursor::tent(2, 3)), "tent"),
            (Cursor::Fort(FortCursor::new(5)), "wall"),
            (Cursor::Target(TargetCursor::new(403, 0)), "target"),
            (Cursor::Defense(DefenseCursor::default()), "defense"),
        ];
        for (cursor, name) in cursors {
            tb.cursor = Some(cursor);
            hud.toolbar = Some(tb.clone());
            assert_eq!(report(&hud, &model).cursor.as_deref(), Some(name));
        }
    }

    /// Portrait of the group (text 500, no values), nothing for an empty selection, subtitle only with slot 1.
    #[test]
    fn group_portrait_has_the_group_text_and_no_values() {
        let mut tb = empty_toolbar();
        tb.data.portrait = vec![[0, 0], [0, 80]];
        let mut hud = hud_with(tb.clone());
        assert_eq!(report(&hud, &PortraitModel::default()).portrait, None);
        tb.selected = -1;
        hud.toolbar = Some(tb.clone());
        let r = report(&hud, &PortraitModel::default());
        assert_eq!(r.selected, -1);
        assert_eq!(
            r.portrait,
            Some(UiPortraitData {
                guid: GROUP_TEXT,
                subtitle: texts().display(GROUP_TEXT),
                values: Vec::new(),
            })
        );
        // Without slot 1 there is no subtitle (the original aborts there).
        tb.data.portrait = vec![[0, 0]];
        hud.toolbar = Some(tb.clone());
        assert_eq!(
            report(&hud, &PortraitModel::default()).portrait,
            Some(UiPortraitData {
                guid: GROUP_TEXT,
                subtitle: String::new(),
                values: Vec::new(),
            })
        );
        // Without `PORTRAITPOSITION` the original draws nothing.
        tb.data.portrait.clear();
        hud.toolbar = Some(tb);
        assert_eq!(report(&hud, &PortraitModel::default()).portrait, None);
    }

    /// The tooltip carries GUID and kind of the request, the text from the game values and keys of drawing, and the
    /// box of the last drawing.
    #[test]
    fn tooltip_reports_request_text_and_box() {
        let mut hud = hud_with(empty_toolbar());
        assert_eq!(report(&hud, &PortraitModel::default()).tooltip, None);
        hud.tooltip.set(TooltipRequest {
            guid: 534,
            kind: kinds::MAKE,
            value: 1.0,
            required: [0; 5],
        });
        // Missing texts appear as a number (O-241): name "534", extra line text 4 as "4".
        let tip = report(&hud, &PortraitModel::default()).tooltip.unwrap();
        assert_eq!(
            tip,
            UiTooltipData {
                guid: 534,
                kind: kinds::MAKE,
                text: "534\n4".to_string(),
                rect: [0; 4],
            }
        );
        hud.tooltip_box.set(Some([0, 10, 100, 568]));
        let tip = report(&hud, &PortraitModel::default()).tooltip.unwrap();
        assert_eq!(tip.rect, [0, 10, 100, 568]);
    }
}
