//! Corner buttons of the minimap (`0x0044A040`; re/game-ui 2.3, D5.1; O-292).
//!
//! The buttons are children of the minimap window (`0x0044A710`), kind `0x13000000`, 25 x 25, sheet `map_icons.tga`
//! (tile 30 x 30, base tile 15, scale 1.0). They use the same button object as the bar (`0x0044F080`, [`Button`]),
//! but without a color list: the constructor then takes its fixed colors ([`CONSTRUCTOR_COLORS`]).
//!
//! | Child | GUID | Icon | Position (left, top) |
//! |---|---|---|---|
//! | 1 | mode m: 574 (m = 0), 575 (1), 573 (2) | m | (W - 177, H - 173) |
//! | 2 | 576 | 3 | (W - 33, H - 173) |
//! | 3 | 571 | 4 | (W - 177, H - 27) |
//! | 4 | 577 | 5 | (W - 33, H - 27) |
//! | 5 | 579 | 6 | (W - 32, 0) |
//! | 6 | 580, only for game type `Engine+0x108 >= 0` | 7 | (W - 62, 0) |
//!
//! The mode button carries the GUID of the **next** mode and, as its icon, the tile of the **current** one - on the
//! sheet, tile m shows the image of the next mode (O-292).
//!
//! As in the original, a button event 9/`0x26`/`0x27` goes to slot 3 of the bar as an event (`0x00458510`): only a
//! visible bar takes it ([`crate::hud::toolbar::Toolbar::event`]); without a bar the click is consumed but has no
//! effect (O-292). The button itself plays the click sound (`0x0044F860`), so also without a bar.

use crate::dialog::UiContext;
use crate::draw::DrawCmd;
use crate::hud::HudEventKind;
use crate::hud::tile_grids::TileGrids;
use crate::hud::toolbar::button::{
    Button, ButtonLocks, ButtonOut, ButtonSpec, SheetRef, TooltipRequest,
};
use crate::hud::toolbar::data::{COLOR_COUNT, Sheet, ToolbarData};
use crate::hud::toolbar::window::KIND_MINIMAP;

/// Icon sheet of the corner buttons (`0x0044A7AD`, string `0x00619A98`), loaded via the texture cache `0x00445F40`.
pub const MAP_ICONS_PATH: &str = "data/icons/map_icons.tga";
/// Tile of [`MAP_ICONS_PATH`] (`PUSH 0x1E, 0x1E` at `0x0044A790`).
pub const MAP_ICON_TILE: i32 = 30;
/// Base tile of the corner buttons (empty, `0x0044A040`: `PUSH 0xF`).
pub const BASE_TILE: i32 = 15;
/// Edge length of a corner button.
pub const BUTTON_SIZE: i32 = 25;
/// Colors of the button constructor without a color list (`0x0044F080`, `param_13 == 0`): icon white on mouse
/// over/pressed, `B4B4B4` when normal and locked; likewise for the selected stance; veil per state 0 ... 3.
pub const CONSTRUCTOR_COLORS: [u32; COLOR_COUNT] = [
    0xFFFF_FFFF,
    0xFFB4_B4B4,
    0xFFB4_B4B4,
    0xFFFF_FFFF,
    0xFFB4_B4B4,
    0xFFB4_B4B4,
    0xFFFF_FFFF,
    0xFFB4_B4B4,
    0xFFB4_B4B4,
    0xFFFF_FFFF,
];

/// 571 "Give signal".
pub const MSG_SIGNAL: i32 = 571;
/// 572 "Diplomacy menu" (key only, without a game type check).
pub const MSG_DIPLOMACY_KEY: i32 = 572;
/// 573 "Normal map" (mode 0).
pub const MSG_MODE_NORMAL: i32 = 573;
/// 574 "Economy map" (mode 1).
pub const MSG_MODE_ECONOMY: i32 = 574;
/// 575 "Military map" (mode 2).
pub const MSG_MODE_MILITARY: i32 = 575;
/// 576 "Select idle work units".
pub const MSG_IDLE_ONE: i32 = 576;
/// 577 "Select all idle work units".
pub const MSG_IDLE_ALL: i32 = 577;
/// 579 "Menu (ESC)".
pub const MSG_GAME_MENU: i32 = 579;
/// 580 "Diplomacy" (button, only outside the campaign).
pub const MSG_DIPLOMACY_BUTTON: i32 = 580;

/// GUID of the mode button in mode `mode` (`FUN_004785C0`; 2 -> -1, then + 574): the **next** mode.
pub fn mode_guid(mode: u8) -> i32 {
    if mode == 2 {
        MSG_MODE_NORMAL
    } else {
        MSG_MODE_ECONOMY + i32::from(mode)
    }
}

/// Mode that a click on `msg` sets (cases `0xE`/`0xF`/`0x10` of the dispatcher, `FUN_004785D0`): 573 -> 0,
/// 574 -> 1, 575 -> 2.
pub fn mode_of(msg: i32) -> Option<u8> {
    (MSG_MODE_NORMAL..=MSG_MODE_MILITARY)
        .contains(&msg)
        .then(|| (msg - MSG_MODE_NORMAL) as u8)
}

/// Setup of the corner buttons in child order (`0x0044A040`) for the screen `W x H`, the map mode `mode` and the game
/// type `game_type`.
pub fn layout(width: i32, height: i32, mode: u8, game_type: i32) -> Vec<ButtonSpec> {
    let mut rows = vec![
        (
            mode_guid(mode),
            i32::from(mode),
            [width - 177, height - 173],
        ),
        (MSG_IDLE_ONE, 3, [width - 33, height - 173]),
        (MSG_SIGNAL, 4, [width - 177, height - 27]),
        (MSG_IDLE_ALL, 5, [width - 33, height - 27]),
        (MSG_GAME_MENU, 6, [width - 32, 0]),
    ];
    if game_type >= 0 {
        rows.push((MSG_DIPLOMACY_BUTTON, 7, [width - 62, 0]));
    }
    rows.into_iter()
        .map(|(msg, icon, [x, y])| ButtonSpec {
            msg,
            flags: KIND_MINIMAP,
            required: [0; 5],
            rect: [x, y, x + BUTTON_SIZE, y + BUTTON_SIZE],
            base: (SheetRef::MapIcons, BASE_TILE),
            icon: (SheetRef::MapIcons, icon),
            scale: 1.0,
            progress: 1.0,
            object: None,
        })
        .collect()
}

/// The corner buttons of the minimap: children of the window, empty while they are taken away (lock `{0x19, 1}`, map
/// off `{0x1D, 0}`).
#[derive(Debug, Clone, PartialEq)]
pub struct MinimapButtons {
    /// Buttons in child order (see module header).
    pub buttons: Vec<Button>,
    /// Sheet and colors of the buttons for [`Button::draw`]: only [`ToolbarData::map_icons`] and
    /// [`CONSTRUCTOR_COLORS`].
    pub data: ToolbarData,
    /// Last pointer position (UI pixels), from which new buttons take over their mouse state (`0x0044F15C`, position
    /// from `FUN_004CCB00`); `None` before the first mouse movement.
    pub mouse: Option<[i32; 2]>,
}

impl MinimapButtons {
    /// Without buttons; the sheet takes its grid from `grids` (O-280: the minimap constructor loads it after the
    /// status bar and before the command bar).
    pub fn new(grids: &mut TileGrids) -> Self {
        let tile = grids.register(MAP_ICONS_PATH, [MAP_ICON_TILE; 2]);
        Self {
            buttons: Vec::new(),
            data: ToolbarData {
                colors: CONSTRUCTOR_COLORS,
                map_icons: Some(Sheet::new(MAP_ICONS_PATH, tile)),
                ..ToolbarData::default()
            },
            mouse: None,
        }
    }

    /// Create the buttons anew (`0x0044A040`: first `FUN_00449F60` removes all, then new ones according to
    /// [`layout`]); each new button takes over the mouse state of the last pointer position.
    pub fn build(&mut self, width: i32, height: i32, mode: u8, game_type: i32) {
        let mouse = self.mouse;
        self.buttons = layout(width, height, mode, game_type)
            .into_iter()
            .map(|spec| Button::new(spec, mouse))
            .collect();
    }

    /// All buttons gone (`FUN_00449F60`).
    pub fn remove(&mut self) {
        self.buttons.clear();
    }

    /// Created?
    pub fn is_built(&self) -> bool {
        !self.buttons.is_empty()
    }

    /// Raw mouse to the children in child order ([`Button::mouse`]); the first true ends the distribution. The button
    /// events go to `out`; the host passes them to slot 3 of the bar.
    pub fn mouse(&mut self, raw: HudEventKind, at: [i32; 2], out: &mut Vec<ButtonOut>) -> bool {
        self.buttons.iter_mut().any(|b| b.mouse(raw, at, out))
    }

    /// Frame event 6 of the buttons (without locks, progress and bars): tooltip request with the mouse over a button,
    /// the last one wins (re/game-ui N.14.6). The tooltip kind `0x13000000` shows the name of the GUID.
    pub fn update(&mut self, tooltip: &mut Option<TooltipRequest>) {
        for b in &mut self.buttons {
            b.update(&ButtonLocks::default(), None, None, tooltip);
        }
    }

    /// Draws the buttons ([`Button::draw`]), read-only after [`Self::update`].
    pub fn draw(&self, ctx: &mut UiContext<'_>, out: &mut Vec<DrawCmd>) {
        for b in &self.buttons {
            b.draw(ctx, &self.data, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::toolbar::button::{ButtonEvent, ButtonState};

    /// The mode button shows the GUID and tooltip of the next mode (0 -> 574, 1 -> 575, 2 -> 573) and, as its icon, the
    /// tile of the current mode (tile m shows the image of the next one, O-292); a click on 573/574/575 sets mode
    /// 0/1/2.
    // QUIRK O-292: kept
    #[test]
    fn mode_button_shows_the_next_mode_m10() {
        for (mode, guid) in [(0, 574), (1, 575), (2, 573)] {
            let specs = layout(1024, 768, mode, -1);
            let b = &specs[0];
            assert_eq!(b.msg, guid, "Modus {mode}");
            assert_eq!(b.icon, (SheetRef::MapIcons, i32::from(mode)));
            assert_eq!(b.base, (SheetRef::MapIcons, BASE_TILE));
            assert_eq!(b.rect, [847, 595, 872, 620]);
            assert_eq!(b.flags, KIND_MINIMAP);
            assert_eq!(mode_of(guid), Some((mode + 1) % 3), "click advances");
        }
        assert_eq!(mode_of(570), None);
        assert_eq!(mode_of(576), None);
        // Tooltip of the mouse over the mode button: GUID of the next mode, kind `0x13000000`.
        let mut buttons = MinimapButtons::new(&mut TileGrids::new());
        buttons.mouse = Some([850, 600]);
        buttons.build(1024, 768, 1, -1);
        assert_eq!(buttons.buttons[0].state, ButtonState::Hover);
        let mut tooltip = None;
        buttons.update(&mut tooltip);
        let t = tooltip.expect("tooltip over the mode button");
        assert_eq!((t.guid, t.kind), (575, KIND_MINIMAP));
    }

    /// Positions, icons and condition of 580 (re/game-ui D5.1, at 1024 x 768 and 800 x 600); colors of the
    /// constructor without a list.
    // QUIRK O-292, O-388: kept
    #[test]
    fn corner_buttons_follow_d5_1() {
        let specs = layout(1024, 768, 0, 0);
        let rows: Vec<_> = specs.iter().map(|s| (s.msg, s.icon.1, s.rect)).collect();
        assert_eq!(
            rows,
            [
                (574, 0, [847, 595, 872, 620]),
                (576, 3, [991, 595, 1016, 620]),
                (571, 4, [847, 741, 872, 766]),
                (577, 5, [991, 741, 1016, 766]),
                (579, 6, [992, 0, 1017, 25]),
                (580, 7, [962, 0, 987, 25]),
            ]
        );
        let small: Vec<_> = layout(800, 600, 0, -1).iter().map(|s| s.rect).collect();
        assert_eq!(small[0], [623, 427, 648, 452]);
        assert_eq!(small.len(), 5, "580 only for game type >= 0");
        assert_eq!(layout(800, 600, 0, -2).len(), 5);
        let b = MinimapButtons::new(&mut TileGrids::new());
        assert_eq!(b.data.colors, CONSTRUCTOR_COLORS);
        assert_eq!(b.data.map_icons, Some(Sheet::new(MAP_ICONS_PATH, [30, 30])));
    }

    /// Click on a corner button: sound, then event 9 or, on a double-click, 9, 9, `0x27` (O-260); a new setup takes
    /// over the mouse state.
    #[test]
    fn corner_buttons_click_like_toolbar_buttons() {
        let mut b = MinimapButtons::new(&mut TileGrids::new());
        b.build(1024, 768, 0, -1);
        let at = [1003, 607];
        let mut out = Vec::new();
        assert!(!b.mouse(HudEventKind::MouseMove, at, &mut out));
        for raw in [
            HudEventKind::LeftDown,
            HudEventKind::LeftUp,
            HudEventKind::LeftDown,
            HudEventKind::LeftUp,
            HudEventKind::DoubleLeft,
        ] {
            assert!(b.mouse(raw, at, &mut out), "{raw:?}");
        }
        let events: Vec<_> = out
            .iter()
            .filter_map(|o| match o {
                ButtonOut::Event { msg, ev, .. } => Some((*msg, *ev)),
                ButtonOut::Sound(_) => None,
            })
            .collect();
        assert_eq!(
            events,
            [
                (576, ButtonEvent::Left),
                (576, ButtonEvent::Left),
                (576, ButtonEvent::Double)
            ]
        );
        assert_eq!(out.len(), 6, "one click sound per event");
        b.mouse = Some(at);
        b.build(1024, 768, 0, -1);
        assert_eq!(b.buttons[1].state, ButtonState::Hover);
        b.remove();
        assert!(!b.is_built());
        assert!(!b.mouse(HudEventKind::LeftDown, at, &mut out));
    }
}
