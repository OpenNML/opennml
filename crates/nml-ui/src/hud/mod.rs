//! Game UI without a GPU: window tree with layers and dispatch, message bar with action stack and game-time clock,
//! status bar, command bar ([`toolbar`]), portrait with values ([`portrait`]), tooltip window ([`tooltip`]), the
//! lettering of the pause ([`pause_text`]), cinemascope area ([`cinemascope`]), scene fade ([`scene_fade`]), info
//! window ([`info_frame`]), countdown ([`countdown`]) and screen texts ([`screen_text`]), plus the selection box of
//! the root ([`selection_rect`]). The module does not know `nml-sim`; values arrive as its own data.
//!
//! Basis: re/game-ui 1.1-1.4 and N.1, N.7, N.13; re/campaign-ui D6.1-D6.17.

pub mod cinemascope;
pub mod countdown;
pub mod info_frame;
pub mod message_bar;
pub mod pause_text;
pub mod portrait;
pub mod scene_fade;
pub mod screen_text;
pub mod selection_rect;
pub mod status_bar;
pub mod tile_grids;
pub mod toolbar;
pub mod tooltip;
pub mod tree;

/// Font of the texts in the game UI: portrait subtitle, counter-upgrade time, tooltip (string `0x00619F74`).
pub const TEXT_FONT: &str = "TextFont";
/// Font of the value lines next to the portrait (string `0x00619F80`, `0x004570C0`).
pub const PANEL_INFO_FONT: &str = "PanelInfo";
/// Font of the dialog lines (text slots 0/1) of the cinemascope area **only in the fallback without `DIALOGAREA`**
/// (`0x004484F0`, re/campaign-ui D6.17); with an area, `DialogArea::text_font` applies unchanged (an empty name
/// yields the substitute font).
pub const DIALOG_FONT: &str = "DialogText";
/// Font of the hints (text slots 2/3) of the cinemascope area (re/campaign-ui D6.3).
pub const INFO_TEXT_FONT: &str = "ShowInfoText";
/// Font of the info windows (re/campaign-ui D6.5).
pub const INFO_FRAME_FONT: &str = "InfoFrame";
/// Font of the countdown (re/campaign-ui D6.9).
pub const COUNTDOWN_FONT: &str = "CountDown";
/// Font of the screen texts (`this+4` of the message bar, `FUN_00494940`; re/campaign-ui D6.12).
pub const SCREEN_TEXT_FONT: &str = "InfoMessages";

/// Kind of a UI event (first word of the event; re/game-ui 1.4 and N.1). The number is the code of the original. Raw
/// mouse kinds and UI events share the same number space.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudEventKind {
    /// 1: mouse move.
    MouseMove = 1,
    /// 2: left button released.
    LeftUp = 2,
    /// 3: left button pressed.
    LeftDown = 3,
    /// 4: right button released.
    RightUp = 4,
    /// 5: right button pressed.
    RightDown = 5,
    /// 6: once per frame (`0x004BB16D`).
    Frame = 6,
    /// 8: selection changed (bar refresh).
    SelectionChanged = 8,
    /// 9: button left (GUID, kind, icon).
    Button = 9,
    /// `0x0A`: all cursors gone.
    CursorsOff = 0x0A,
    /// `0x0B`: place building (GUID, rotation, position).
    PlaceBuilding = 0x0B,
    /// `0x0C`: set tooltip.
    TooltipSet = 0x0C,
    /// `0x0D`: clear tooltip.
    TooltipClear = 0x0D,
    /// `0x10`: bar refresh (subkind 0-3).
    Rebuild = 0x10,
    /// `0x11`: bar off/on.
    ToolbarOnOff = 0x11,
    /// `0x12`: double right-click.
    DoubleRight = 0x12,
    /// `0x13`: double left-click.
    DoubleLeft = 0x13,
    /// `0x14`: place wall (n, 5 words each).
    Wall = 0x14,
    /// `0x15`: wall cursor per frame.
    WallFrame = 0x15,
    /// `0x16`: target chosen.
    Target = 0x16,
    /// `0x17`: targeting mode aborted.
    TargetCancel = 0x17,
    /// `0x19`: input lock on/off.
    Lock = 0x19,
    /// `0x1A`: network command `0x5000002E`; no sender in the original (dead).
    Dead1A = 0x1A,
    /// `0x1B`: cinemascope bars on/off.
    Cinemascope = 0x1B,
    /// `0x1C`: status bar off/on.
    StatusBarOnOff = 0x1C,
    /// `0x1D`: minimap off/on.
    MinimapOnOff = 0x1D,
    /// `0x1E`: show cinemascope portrait (`0x00446F00`, slot 0...3; re/campaign-ui D6.1).
    PortraitShow = 0x1E,
    /// `0x1F`: close cinemascope portrait (`0x00446F60`).
    PortraitHide = 0x1F,
    /// `0x20`: show cinemascope text slot (`0x00446FC0`).
    TextShow = 0x20,
    /// `0x21`: close cinemascope text slot (`0x00447020`).
    TextHide = 0x21,
    /// `0x22`: minimap buttons refresh.
    MinimapButtons = 0x22,
    /// `0x23`: hero button on.
    HeroOn = 0x23,
    /// `0x24`: remove one hero button (no sender).
    HeroRemove = 0x24,
    /// `0x25`: remove all hero buttons.
    HeroClear = 0x25,
    /// `0x26`: button right.
    ButtonRight = 0x26,
    /// `0x27`: button left double.
    ButtonDouble = 0x27,
    /// `0x28`: button right double (not handled).
    ButtonRightDouble = 0x28,
    /// `0x29`: object under the mouse (hover).
    Hover = 0x29,
    /// `0x2D`: wheel up.
    WheelUp = 0x2D,
    /// `0x2E`: wheel down.
    WheelDown = 0x2E,
    /// `0x2F`: defense point.
    DefensePoint = 0x2F,
    /// `0x30`: cancel defense radius.
    DefenseCancel = 0x30,
    /// `0x31`: headhunter button on.
    HeadhunterOn = 0x31,
    /// `0x32`: remove all headhunter buttons.
    HeadhunterClear = 0x32,
}

impl HudEventKind {
    /// All known kinds in code order.
    pub const ALL: [HudEventKind; 43] = [
        Self::MouseMove,
        Self::LeftUp,
        Self::LeftDown,
        Self::RightUp,
        Self::RightDown,
        Self::Frame,
        Self::SelectionChanged,
        Self::Button,
        Self::CursorsOff,
        Self::PlaceBuilding,
        Self::TooltipSet,
        Self::TooltipClear,
        Self::Rebuild,
        Self::ToolbarOnOff,
        Self::DoubleRight,
        Self::DoubleLeft,
        Self::Wall,
        Self::WallFrame,
        Self::Target,
        Self::TargetCancel,
        Self::Lock,
        Self::Dead1A,
        Self::Cinemascope,
        Self::StatusBarOnOff,
        Self::MinimapOnOff,
        Self::PortraitShow,
        Self::PortraitHide,
        Self::TextShow,
        Self::TextHide,
        Self::MinimapButtons,
        Self::HeroOn,
        Self::HeroRemove,
        Self::HeroClear,
        Self::ButtonRight,
        Self::ButtonDouble,
        Self::ButtonRightDouble,
        Self::Hover,
        Self::WheelUp,
        Self::WheelDown,
        Self::DefensePoint,
        Self::DefenseCancel,
        Self::HeadhunterOn,
        Self::HeadhunterClear,
    ];

    /// Kind for a code of the original; `None` for unknown codes (dispatch then returns false).
    pub fn from_code(code: u32) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.code() == code)
    }

    /// Code of the original.
    pub fn code(self) -> u32 {
        self as u32
    }
}

/// A UI event: kind and the further words (re/game-ui N.1).
///
/// For the raw mouse kinds (1-5, `0x12`, `0x13`, wheel), `args = [x, y]` in **UI pixels** (like `ev[1]`/`ev[2]` in the
/// original, `0x004584A0`); the host converts window pixels beforehand. [`tree::mouse_point`] reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudEvent {
    pub kind: HudEventKind,
    pub args: Vec<i32>,
}

impl HudEvent {
    /// Event without further words.
    pub fn new(kind: HudEventKind) -> Self {
        Self {
            kind,
            args: Vec::new(),
        }
    }

    /// Event with further words.
    pub fn with_args(kind: HudEventKind, args: &[i32]) -> Self {
        Self {
            kind,
            args: args.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_codes_round_trip() {
        // Every kind has its code of the original and maps back through it.
        let all = HudEventKind::ALL;
        for k in all {
            assert_eq!(HudEventKind::from_code(k.code()), Some(k), "{k:?}");
        }
        // Unique codes.
        let mut codes: Vec<u32> = all.iter().map(|k| k.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
        // Samples from N.1.
        assert_eq!(HudEventKind::MouseMove.code(), 1);
        assert_eq!(HudEventKind::Frame.code(), 6);
        assert_eq!(HudEventKind::Button.code(), 9);
        assert_eq!(HudEventKind::Rebuild.code(), 0x10);
        assert_eq!(HudEventKind::Dead1A.code(), 0x1A);
        assert_eq!(HudEventKind::StatusBarOnOff.code(), 0x1C);
        assert_eq!(HudEventKind::MinimapButtons.code(), 0x22);
        assert_eq!(HudEventKind::Hover.code(), 0x29);
        assert_eq!(HudEventKind::HeadhunterClear.code(), 0x32);
        // D6.1: show/close portrait and show/close text slot.
        assert_eq!(HudEventKind::PortraitShow.code(), 0x1E);
        assert_eq!(HudEventKind::PortraitHide.code(), 0x1F);
        assert_eq!(HudEventKind::TextShow.code(), 0x20);
        assert_eq!(HudEventKind::TextHide.code(), 0x21);
        // Gaps in the original (0, 7, 0x18, 0x2A...0x2C, 0x33) are not kinds.
        for code in [0, 7, 0x18, 0x2A, 0x2B, 0x2C, 0x33, u32::MAX] {
            assert_eq!(HudEventKind::from_code(code), None, "{code:#x}");
        }
    }
}
