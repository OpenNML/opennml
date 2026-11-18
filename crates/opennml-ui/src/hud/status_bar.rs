//! Status bar of the game UI: resources, population and player name in a semi-transparent bar at the top edge.
//! Without simulation: the values arrive as [`StatusValues`].
//!
//! Basis: re/game-ui 2.2 and N.4; drawing `StatusBar_Render` `0x00451B40`, event `0x004518E0`, quirk O-221. The bar
//! has no mouse handling (clicks go to the world), and its texts run **without text layout and without shadow**
//! (`FUN_004287C0`/`FUN_00428390`), hence without tags: `[COLOR ...]` in the name appears literally.

use opennml_formats::statusbar::StatusBarConfig;

use crate::bkg_paint::{fill_rect, tile_quad};
use crate::coords::status_rect;
use crate::dialog::UiContext;
use crate::draw::{DrawCmd, Filter};
use crate::hud::{HudEvent, HudEventKind};
use crate::text::{atlas_codes, draw_string};

/// Font of the status bar (`StatusFont`, pointer `+0x4C`; `FUN_00451F80` `0x004526BA`).
pub const STATUS_FONT: &str = "StatusFont";
/// Red of the population above the living space (`0x00451B40`: `0xFFFF0000`).
pub const OVERFULL_COLOR: u32 = 0xFFFF_0000;
/// Diffuse color of the icons (`FUN_004020E0(..., 0xFFFFFFFF, 1.0, 1.0, 0)`).
const ICON_COLOR: u32 = 0xFFFF_FFFF;

/// Values the bar shows: resources 0...4 (gold, wood, food, population, living space; `Player_GetResource`) and the
/// name of the local player (`Player+0xC`, empty = draw nothing).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusValues {
    pub resources: [i32; 5],
    pub name: String,
}

impl StatusValues {
    /// The rule for red (`0x00451B40`): population (resource 3) **greater than** living space (resource 4).
    pub fn overfull(&self) -> bool {
        self.resources[4] < self.resources[3]
    }
}

/// Status bar (`0x00619CD0`): configuration from `statusbar.ini` and the switch `+0x7C`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusBar {
    pub config: StatusBarConfig,
    /// `+0x7C`: bar drawn (event `0x1C`).
    pub shown: bool,
}

impl StatusBar {
    /// Bar with the configuration; visible (the constructor `0x00452720` sets `+0x7C` to 1).
    pub fn new(config: StatusBarConfig) -> Self {
        Self {
            config,
            shown: true,
        }
    }

    /// Event `0x004518E0`: only `0x1C` (bar off/on): `shown = (w != 0)`, return true; all other kinds are not handled
    /// (false). The visibility of the window stays untouched.
    pub fn event(&mut self, ev: &HudEvent) -> bool {
        if ev.kind != HudEventKind::StatusBarOnOff {
            return false;
        }
        self.shown = ev.args.first().copied().unwrap_or(0) != 0;
        true
    }

    /// Text and color of an entry per `0x00451B40`; `None` if nothing is drawn.
    /// - Index 3: `"%d/%d"` from resources 3 and 4, red only if population **>** living space;
    /// - Index >= 0 otherwise: `"%d"` from `Player_GetResource` (outside 0...4 it returns 0) in `STATUSFONTCOLOR`;
    /// - Index -1: the name, only if not empty (the width computed afterwards stays unused);
    /// - any other index: nothing.
    fn entry_text(&self, index: i32, v: &StatusValues) -> Option<(String, u32)> {
        let font_color = self.config.font_color;
        match index {
            3 => {
                let (population, living) = (v.resources[3], v.resources[4]);
                let color = if v.overfull() {
                    OVERFULL_COLOR
                } else {
                    font_color
                };
                Some((format!("{population}/{living}"), color))
            }
            0.. => {
                let value = usize::try_from(index)
                    .ok()
                    .and_then(|i| v.resources.get(i))
                    .copied()
                    .unwrap_or(0);
                Some((value.to_string(), font_color))
            }
            -1 if !v.name.is_empty() => Some((v.name.clone(), font_color)),
            _ => None,
        }
    }

    /// Is the population display drawn red? True if the bar has an entry with index 3 and [`StatusValues::overfull`]
    /// holds - regardless of the font color of the file. Source of `ui_state.game.hud.status.red`.
    pub fn shows_overfull(&self, v: &StatusValues) -> bool {
        v.overfull() && self.config.entries.iter().any(|e| e.index == 3)
    }

    /// The texts of the bar in file order of the entries: `(index, text, color)`. A name is only drawn if it is not
    /// empty. Also the source of `ui_state.game.hud.status` (`opennml_client::game::hud::Hud::state`).
    pub fn texts(&self, v: &StatusValues) -> Vec<(i32, String, u32)> {
        self.config
            .entries
            .iter()
            .filter_map(|e| {
                self.entry_text(e.index, v)
                    .map(|(text, color)| (e.index, text, color))
            })
            .collect()
    }

    /// Drawing `0x00451B40` (only when `shown`):
    /// 1. Rectangle `(0, 0, W, STATUSSIZE)` in `STATUSCOLOR` (`alpharect1`, quad without texture);
    /// 2. all icons with index >= 0 (cell `i` of the sheet in the grid `TexW / tw x TexH / th`, size `ICONSIZE`,
    ///    position `icon`, `FUN_004020E0` like the icons in the text layout; if the sheet is missing, they are
    ///    omitted);
    /// 3. all texts in `StatusFont` at `text`, without text layout and without shadow.
    pub fn draw(&self, ctx: &mut UiContext<'_>, v: &StatusValues, out: &mut Vec<DrawCmd>) {
        if !self.shown {
            return;
        }
        let config = &self.config;
        fill_rect(status_rect(ctx.screen, config.size), config.color, out);
        if let Some(sheet) = ctx.assets.texture(ctx.vfs, &config.icon_file) {
            let image = ctx.assets.image(sheet);
            let tex_size = [image.width(), image.height()];
            for entry in config.entries.iter().filter(|e| e.index >= 0) {
                let [x, y] = entry.icon;
                let [kb, kh] = config.icon_size;
                let dst = [x, y, x.wrapping_add(kb), y.wrapping_add(kh)];
                tile_quad(
                    sheet,
                    tex_size,
                    config.icon_size,
                    entry.index,
                    dst,
                    dst,
                    ICON_COLOR,
                    Filter::Point,
                    out,
                );
            }
        }
        let font = ctx.fonts.by_name(STATUS_FONT);
        for entry in &config.entries {
            if let Some((text, color)) = self.entry_text(entry.index, v) {
                draw_string(&atlas_codes(&text), entry.text, color, font, ctx.fonts, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use opennml_formats::localisation::{Localisation, parse_localisation};
    use opennml_formats::statusbar::parse_statusbar;

    use super::*;
    use crate::assets::{Texts, UiAssets};
    use crate::coords::Screen;
    use crate::draw::Blend;
    use crate::fonts::FontBook;
    use crate::testutil::{fnt_text, test_vfs, tga_bottom_up};

    /// Lines as in the shipped data (re/game-ui 2.2): resources 0...3 at icon-x 1/120/240/360, name at 480.
    const INI: &str = "STATUSSIZE 26\r\nSTATUSCOLOR 2 22 35\r\nSTATUSFONTCOLOR 255 255 255\r\n\
ICONSIZE 32 32\r\nICONFILE data/icons/resourcen_icons.tga\r\n\
RESOURCE 0 1 1 33 5\r\nRESOURCE 1 120 1 153 5\r\nRESOURCE 2 240 1 273 5\r\nRESOURCE 3 360 1 393 5\r\n\
PLAYERNAME 480 1 480 5\r\n";

    fn bar() -> StatusBar {
        StatusBar::new(parse_statusbar(INI))
    }

    fn values(res: [i32; 5], name: &str) -> StatusValues {
        StatusValues {
            resources: res,
            name: name.into(),
        }
    }

    /// Font `StatusFont`: every character 10 px wide, line height 15 (cell 16).
    fn fonts() -> &'static FontBook {
        static FONTS: OnceLock<FontBook> = OnceLock::new();
        FONTS.get_or_init(|| {
            let vfs = test_vfs(&[
                (
                    "data/fonts/Test_15_0_0_0_0.fnt",
                    fnt_text(|_| 10, |_| 15).into_bytes(),
                ),
                (
                    "data/fonts/Test_15_0_0_0_0.tga",
                    tga_bottom_up(256, 256, |_, _| [255, 255, 255, 255]),
                ),
            ]);
            let mut assets = UiAssets::new();
            let loc =
                parse_localisation("Font StatusFont\nFontSize 15\nTextureFont\nFontName Test\n");
            FontBook::load(&vfs, &loc, 1024, &mut assets, None)
        })
    }

    /// Draws the bar onto `screen`; the icon sheet (128 x 32, four cells of 32 x 32) lies in the test VFS if
    /// `with_sheet` is set.
    fn draw(bar: &StatusBar, v: &StatusValues, screen: Screen, with_sheet: bool) -> Vec<DrawCmd> {
        let files: Vec<(&str, Vec<u8>)> = if with_sheet {
            vec![(
                "data/icons/resourcen_icons.tga",
                tga_bottom_up(128, 32, |_, _| [255, 255, 255, 255]),
            )]
        } else {
            Vec::new()
        };
        let vfs = test_vfs(&files);
        let mut assets = UiAssets::new();
        let texts = Texts::load(&vfs, &Localisation::default());
        let mut ctx = UiContext {
            vfs: &vfs,
            assets: &mut assets,
            fonts: fonts(),
            texts: &texts,
            screen,
        };
        let mut out = Vec::new();
        bar.draw(&mut ctx, v, &mut out);
        out
    }

    fn screen(width: i32, height: i32) -> Screen {
        Screen { width, height }
    }

    /// Color of the text quads (those with a texture and not the white-neutral icons): quads with color `color` and
    /// font atlas, in order `(x, y)`.
    fn text_quads(cmds: &[DrawCmd], color: u32) -> Vec<(i32, i32)> {
        cmds.iter()
            .filter_map(|c| match *c {
                DrawCmd::Quad {
                    texture: Some(_),
                    dst,
                    color: c,
                    ..
                } if c == color && dst[2] - dst[0] == 16 => Some((dst[0], dst[1])),
                _ => None,
            })
            .collect()
    }

    // QUIRK O-221: kept
    #[test]
    fn population_turns_red_only_above_living_space() {
        let bar = bar();
        let text = |pop, living| {
            let t = bar.texts(&values([0, 0, 0, pop, living], ""));
            t.into_iter().find(|(i, _, _)| *i == 3).unwrap()
        };
        // Tie: normal; one above: red; below: normal.
        assert_eq!(text(10, 10), (3, "10/10".to_string(), 0xFFFF_FFFF));
        assert_eq!(text(11, 10), (3, "11/10".to_string(), OVERFULL_COLOR));
        assert_eq!(text(9, 10), (3, "9/10".to_string(), 0xFFFF_FFFF));
        // Drawn in the same color.
        let cmds = draw(
            &bar,
            &values([0, 0, 0, 11, 10], ""),
            screen(1024, 768),
            true,
        );
        assert_eq!(text_quads(&cmds, OVERFULL_COLOR).len(), "11/10".len());
    }

    #[test]
    fn red_flag_follows_the_rule_not_the_font_color() {
        // With a purely red font color the population is not "red" as long as it does not exceed the living space.
        let mut red_font = bar();
        red_font.config.font_color = OVERFULL_COLOR;
        assert!(!red_font.shows_overfull(&values([0, 0, 0, 10, 10], "")));
        assert!(!red_font.shows_overfull(&values([0, 0, 0, 9, 10], "")));
        assert!(red_font.shows_overfull(&values([0, 0, 0, 11, 10], "")));
        // Without an entry for the population there is nothing red.
        let mut no_population = bar();
        no_population.config.entries.retain(|e| e.index != 3);
        assert!(!no_population.shows_overfull(&values([0, 0, 0, 11, 10], "")));
    }

    // QUIRK O-221: kept
    #[test]
    fn status_bar_formats_any_value() {
        // `%d` without a limit: negative and above 10^6.
        let bar = bar();
        let t = bar.texts(&values([-5, 2_000_000, i32::MIN, -1, i32::MAX], ""));
        let texts: Vec<_> = t.iter().map(|(_, s, _)| s.as_str()).collect();
        assert_eq!(texts, ["-5", "2000000", "-2147483648", "-1/2147483647"]);
        // Negative population below the living space stays white.
        assert_eq!(t[3].2, 0xFFFF_FFFF);
        // An index outside 0...4 reads `Player_GetResource` as 0; index < -1 draws nothing.
        let mut odd = bar.clone();
        odd.config.entries[0].index = 7;
        odd.config.entries[1].index = -2;
        let t = odd.texts(&values([9; 5], ""));
        assert_eq!(t.len(), 3);
        assert_eq!((t[0].0, t[0].1.as_str()), (7, "0"));
    }

    // QUIRK O-221: kept
    #[test]
    fn empty_name_draws_no_text() {
        let bar = bar();
        let empty = bar.texts(&values([1, 2, 3, 4, 5], ""));
        assert!(empty.iter().all(|(i, _, _)| *i >= 0));
        assert_eq!(empty.len(), 4);
        let named = bar.texts(&values([1, 2, 3, 4, 5], "Anna"));
        assert_eq!(
            named.last().unwrap(),
            &(-1, "Anna".to_string(), 0xFFFF_FFFF)
        );
        // Drawing: without a name no text quad at the position of the name (480, 5).
        let cmds = draw(&bar, &values([1, 2, 3, 4, 5], ""), screen(1024, 768), true);
        assert!(text_quads(&cmds, 0xFFFF_FFFF).iter().all(|&p| p.0 != 480));
        let cmds = draw(
            &bar,
            &values([1, 2, 3, 4, 5], "Anna"),
            screen(1024, 768),
            true,
        );
        let at_name = text_quads(&cmds, 0xFFFF_FFFF)
            .into_iter()
            .filter(|p| p.1 == 5 && p.0 >= 480)
            .count();
        assert_eq!(at_name, 4);
    }

    // QUIRK O-221: kept
    #[test]
    fn event_1c_hides_and_shows() {
        let mut bar = bar();
        let v = values([1, 2, 3, 4, 5], "Anna");
        assert!(bar.shown);
        assert!(!draw(&bar, &v, screen(1024, 768), true).is_empty());
        // w = 0 hides, w != 0 shows; true both times.
        assert!(bar.event(&HudEvent::with_args(HudEventKind::StatusBarOnOff, &[0])));
        assert!(!bar.shown);
        assert!(draw(&bar, &v, screen(1024, 768), true).is_empty());
        assert!(bar.event(&HudEvent::with_args(HudEventKind::StatusBarOnOff, &[-3])));
        assert!(bar.shown);
        // All other kinds: not handled, state stays.
        for kind in HudEventKind::ALL {
            if kind == HudEventKind::StatusBarOnOff {
                continue;
            }
            assert!(!bar.event(&HudEvent::with_args(kind, &[0])), "{kind:?}");
            assert!(bar.shown, "{kind:?}");
        }
    }

    // QUIRK O-221: kept
    #[test]
    fn status_bar_has_no_shadow_and_no_tags() {
        // The name passes through literally: no text layout, so no `[COLOR ...]` tag and no shadow.
        let bar = bar();
        let name = "[COLOR FF0000FF]Bob";
        let cmds = draw(&bar, &values([0; 5], name), screen(1024, 768), true);
        // One quad per character of the name, all in the font color, nothing in shadow color/another color.
        let at_name = text_quads(&cmds, 0xFFFF_FFFF)
            .into_iter()
            .filter(|p| p.1 == 5 && p.0 >= 480)
            .collect::<Vec<_>>();
        assert_eq!(at_name.len(), name.chars().count());
        // Advance 10 from (480, 5): no offset from a shadow.
        for (i, &(x, y)) in at_name.iter().enumerate() {
            assert_eq!((x, y), (480 + 10 * i as i32, 5));
        }
        let quads_with_texture = cmds
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    DrawCmd::Quad {
                        texture: Some(_),
                        ..
                    }
                )
            })
            .count();
        // 4 icons + texts "0", "0", "0", "0/0" and the name, one quad per character each.
        assert_eq!(quads_with_texture, 4 + 1 + 1 + 1 + 3 + name.chars().count());
        assert!(bar.texts(&values([0; 5], name)).last().unwrap().1 == name);
    }

    // QUIRK O-221: kept
    #[test]
    fn status_layout_at_1024x768() {
        let bar = bar();
        let cmds = draw(&bar, &values([5, 6, 7, 8, 9], "X"), screen(1024, 768), true);
        // 1. Rectangle without texture across the whole width in `STATUSCOLOR`.
        assert_eq!(
            cmds[0],
            DrawCmd::Quad {
                texture: None,
                dst: [0, 0, 1024, 26],
                uv: [0.0; 4],
                color: 0x8023_1602,
                filter: Filter::Point,
                blend: Blend::Alpha,
            }
        );
        // 2. Four icons (index 0...3) at the positions from the file, cell i of the 128 x 32 sheet, white, POINT.
        let icons: Vec<_> = cmds[1..5].to_vec();
        for (i, cmd) in icons.iter().enumerate() {
            let DrawCmd::Quad {
                texture,
                dst,
                uv,
                color,
                filter,
                blend: Blend::Alpha,
            } = *cmd
            else {
                unreachable!()
            };
            assert!(texture.is_some(), "icon {i}");
            let x = [1, 120, 240, 360][i];
            assert_eq!(dst, [x, 1, x + 32, 33], "icon {i}");
            assert_eq!((color, filter), (0xFFFF_FFFF, Filter::Point));
            let (zu, half) = (32.0 * i as f32 / 128.0, 0.5 / 128.0);
            assert!((uv[0] - (zu + half)).abs() < 1e-6, "icon {i} u0 {uv:?}");
            assert!(
                (uv[2] - (zu + 31.0 / 128.0 - half)).abs() < 1e-6,
                "icon {i} u1"
            );
            assert!((uv[1] - half).abs() < 1e-6 && (uv[3] - (31.0 / 32.0 - half)).abs() < 1e-6);
        }
        // 3. Texts: first character of "5" at (33, 5), "6" at (153, 5), "7" at (273, 5), "8/9" at (393, 5),
        // the name at (480, 5); cell 16 x 16, advance 10.
        let firsts: Vec<_> = cmds[5..]
            .iter()
            .map(|c| match *c {
                DrawCmd::Quad { dst, .. } => (dst[0], dst[1]),
                DrawCmd::Quad4 { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(
            firsts,
            [
                (33, 5),
                (153, 5),
                (273, 5),
                (393, 5),
                (403, 5),
                (413, 5),
                (480, 5)
            ]
        );
    }

    #[test]
    fn rectangle_follows_the_screen_width_and_missing_sheet_skips_icons() {
        let bar = bar();
        let cmds = draw(&bar, &values([0; 5], ""), screen(1280, 1024), false);
        let DrawCmd::Quad { dst, texture, .. } = cmds[0] else {
            unreachable!()
        };
        assert_eq!((dst, texture), ([0, 0, 1280, 26], None));
        // Without a sheet no icons: rectangle + 4 texts of 1 character "0" or 3 characters "0/0".
        assert_eq!(cmds.len(), 1 + 1 + 1 + 1 + 3);
    }
}
