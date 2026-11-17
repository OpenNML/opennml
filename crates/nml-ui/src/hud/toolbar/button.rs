//! Button object of the command bar: state, mouse handling, locks per frame, drawing of base icon, icon, veil and
//! bars, and the tooltip request.
//!
//! Basis: re/game-ui N.5.1 (button object `0x0044F080`, `0xCC` bytes) and N.14.1, N.14.3, N.14.6 (drawing
//! `0x0044F250`). The original draws and changes the state in one go (frame event 6). The reimplementation separates
//! the two so that drawing stays `&self`:
//! - [`Button::update`] is the frame event: take over progress (event `0xF`), locks per step 1, write back the state
//!   from the start of the frame, set "clickable" and make the tooltip request (step 5). The button remembers the
//!   lock of this frame only for drawing (`frame_lock`).
//! - [`Button::draw`] is read-only: base icon, icon, veil, bars (steps 2 to 4).
//!
//! Dimensions are calculated as in the original in `f32` without rounding (N.14.1); they enter the draw quad with
//! `as i32` (truncation like `ftol`). As a result, veils and bars are up to one pixel shorter than in the original,
//! which passes sub-pixels to D3D8 (a new `DrawCmd` would break the golden checksums).

use nml_formats::gifcon::button_kind as kinds;

use crate::bkg_paint::{fill_rect, image_quad};
use crate::dialog::UiContext;
use crate::draw::{DrawCmd, Filter};
use crate::geom::contains;
use crate::hud::HudEventKind;

use super::data::{Sheet, ToolbarData};
use super::window::ToolbarIntent;

/// Kind of the selection buttons (multi-selection), bars yes.
pub const KIND_SELECTION: u32 = 0x500_0000;
/// Kind of the production row (running and waiting orders), veil yes.
pub const KIND_PRODUCTION: u32 = 0x600_0000;
/// Tooltip kind "prerequisite missing" (`0x9000000`, N.5.1 step 5).
pub const KIND_MISSING_REQUIREMENT: u32 = 0x900_0000;
/// Kind of the stance buttons; with bit 4 (`0xA000004`) the selected stance.
pub const KIND_STANCE: u32 = 0xA00_0000;
/// Stance bit "selected" in the kind bits (`& 4`).
pub const STANCE_SELECTED: u32 = 4;
/// Kind of the hero buttons, bars yes.
pub const KIND_HERO: u32 = 0x1400_0000;
/// GUID of the tooltip "Counter-upgrade is currently running" (text 602).
pub const COUNTER_RUNNING_GUID: i32 = 602;

/// White: color of the base icon and the backgrounds.
const WHITE: u32 = 0xFFFF_FFFF;

/// Kinds whose buttons the drawing locks by prerequisites (`0x0044F2C9...0x0044F319`): BUILD, MAKE, UPGRADE,
/// DO, TARGET, TARGET_DO, ONLYONCE, COUNTERUPGRADE, MAKEONLY_ONCE.
const LOCKABLE_KINDS: [u32; 9] = [
    kinds::BUILD,
    kinds::MAKE,
    kinds::UPGRADE,
    kinds::DO,
    kinds::TARGET,
    kinds::TARGET_DO,
    kinds::ONLYONCE,
    kinds::COUNTERUPGRADE,
    kinds::MAKEONLY_ONCE,
];

/// Bars per attribute in the order of the original (`0x0044F5A1...0x0044F6B5`): attribute 14 hit points, 15 mana,
/// 28 stamina, 30 experience; foreground, background.
const BAR_COLORS: [(u32, u32); 4] = [
    (0xFF00_FF00, 0x80FF_0000),
    (0xFF88_DDFF, 0x8088_DDFF),
    (0xFFEE_BBAA, 0x80EE_BBAA),
    (0xFFFF_FF44, 0x80FF_FF44),
];

/// State of a button (`+0x90`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    /// 0: normal.
    Normal,
    /// 1: mouse over.
    Hover,
    /// 2: pressed.
    Pressed,
    /// 3: locked.
    Locked,
}

/// What the button reports to the bar from a mouse action: the event `9`, `0x26`, `0x27` or `0x28`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    /// 9: left released.
    Left,
    /// `0x26`: right released.
    Right,
    /// `0x27`: left double.
    Double,
    /// `0x28`: right double; the bar does not handle it (O-223).
    RightDouble,
}

impl ButtonEvent {
    /// Code of the event in the original.
    pub fn code(self) -> u32 {
        self.kind() as u32
    }

    /// Event kind in [`HudEventKind`].
    pub fn kind(self) -> HudEventKind {
        match self {
            Self::Left => HudEventKind::Button,
            Self::Right => HudEventKind::ButtonRight,
            Self::Double => HudEventKind::ButtonDouble,
            Self::RightDouble => HudEventKind::ButtonRightDouble,
        }
    }
}

/// Sound of the bar. The host maps it to the sound output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarSound {
    /// Click sound `SFX_KLICK`, flags `0x14`, category 1 (`0x0044F860` and siblings).
    Click,
    /// Selection sound of a unit (`Selection_PlaySelectSound`, re/audio, section "Acknowledgments").
    Select,
}

/// Output of the bar to the host (the internal button event [`ButtonOut::Event`] never arrives here).
#[derive(Debug, Clone, PartialEq)]
pub enum ToolbarOut {
    /// Sound (click of the button, selection sound of the full build).
    Sound(ToolbarSound),
    /// `GameEngine+0x4E0 = value` (`0x0042ED10`, first step of `FUN_00458130`, O-246): the host uses it to set the
    /// last pressed button for script condition 38. The value is the `BUTTONMSG`, for hero, multi-selection and
    /// occupant buttons the HID.
    ButtonPressed(i32),
    /// A game action that the host converts into commands.
    Intent(ToolbarIntent),
}

/// Intermediate output of a button that only the bar reads: sounds and the event
/// `{9, BUTTONMSG, kind, icon}` or `0x26`, `0x27`, `0x28`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ButtonOut {
    Sound(ToolbarSound),
    Event {
        /// `BUTTONMSG` of the button.
        msg: i32,
        /// Kind bits of the button (full word).
        flags: u32,
        /// HID for selection, hero and occupant buttons.
        object: Option<u32>,
        ev: ButtonEvent,
    },
}

/// Sheet from which the base icon or icon comes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetRef {
    /// An `ICONS` sheet of `icons.ini` (50 x 50), number as in the file.
    Icons(usize),
    /// `resourcen_icons.tga`: stance and formation (O-257); grid 24 x 24 or that of the status bar (O-280).
    Resource,
    /// `map_icons.tga` of the corner buttons of the minimap (30 x 30, [`ToolbarData::map_icons`]).
    MapIcons,
}

/// Setup of a button (constructor `0x0044F080`).
#[derive(Debug, Clone, PartialEq)]
pub struct ButtonSpec {
    /// `BUTTONMSG`; negative creates the button locked (state 3, O-261).
    pub msg: i32,
    /// Kind bits of the `toolbar.ini` line (full word); the kind is the top byte ([`ButtonSpec::kind`]).
    pub flags: u32,
    /// `REQUIRED_GUID` (up to the first 0).
    pub required: [i32; 5],
    /// Rectangle in screen pixels `[left, top, right, bottom]`.
    pub rect: [i32; 4],
    /// Base icon: sheet and tile (white, at `(left, top)`).
    pub base: (SheetRef, i32),
    /// Icon: sheet and tile.
    pub icon: (SheetRef, i32),
    /// Scale (`+0x98`): 0.5 for multi-selection and waiting orders, otherwise 1.0 (N.14.3).
    pub scale: f32,
    /// Initial value of the progress (`+0x9C`): 1.0 = without veil. The frame only overwrites it with `Some`.
    pub progress: f32,
    /// HID for selection, hero and occupant buttons; it replaces `msg` in the tooltip and the event of the host.
    pub object: Option<u32>,
}

impl ButtonSpec {
    /// Kind of the button: `flags & 0xFF000000` (like `ToolbarButton::kind`).
    pub fn kind(&self) -> u32 {
        self.flags & kinds::KIND_MASK
    }
}

/// Locks that the frame determines for a button (step 1). The host calculates them from object counters and model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ButtonLocks {
    /// A `REQUIRED_GUID` has object counter 0 for the local player.
    pub missing_required: bool,
    /// A counter-upgrade of this GUID is running somewhere or the local slot has one running itself
    /// (`0x00412620`, `0x00479690`); only applies to COUNTERUPGRADE.
    pub counter_running: bool,
}

/// Attribute shares of an object for the bars (step 5 of N.14.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ButtonBars {
    /// For the attributes 14, 15, 28, 30: `Some(share)` for a tracked attribute, already multiplied by `1e-5`;
    /// `None` for an untracked one (`AttributeSet+0xB8`), which gets neither a bar nor a gap.
    pub tracked: [Option<f32>; 4],
}

/// Request to the tooltip window: content of the event `{0xC, x, bottom, GUID, kind, value}` without `x` and `bottom`
/// (the original stores them but never reads them, O-264). The request applies for one frame (O-44).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooltipRequest {
    /// GUID of the tooltip text: `BUTTONMSG`, for selection, hero and occupant buttons the HID; 602 for the locked
    /// counter-upgrade.
    pub guid: i32,
    /// Kind without flags (`0xFF000000`) or [`KIND_MISSING_REQUIREMENT`].
    pub kind: u32,
    /// For production the progress (exactly 1.0 reports 0), otherwise 1; for a missing prerequisite 0.
    pub value: f32,
    /// The `REQUIRED_GUID` list (only set for [`KIND_MISSING_REQUIREMENT`]).
    pub required: [i32; 5],
}

/// A button of the command bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub spec: ButtonSpec,
    /// State `+0x90`; the state from the start of the frame also applies after [`Button::update`] (O-261).
    pub state: ButtonState,
    /// `+0xA0`: clickable, set in the frame (`state != 3` after the locks) and valid until the next frame.
    pub clickable: bool,
    /// Progress `+0x9C`.
    progress: f32,
    /// Bars of the frame (only selection and hero buttons with a found object).
    bars: Option<ButtonBars>,
    /// The frame has locked the button; drawing then shows state 3 even though `state` has been written back.
    frame_lock: bool,
}

impl Button {
    /// Creates a button (`0x0044F080`): locked for `BUTTONMSG < 0` (state 3). With a mouse position the constructor
    /// replays the movement there (`0x0044F15C...0x0044F184`, N.14.7): the button thus takes over the mouse state, and
    /// the initial lock is lost immediately. Without a position it stays until the first mouse event (O-261).
    pub fn new(spec: ButtonSpec, mouse: Option<[i32; 2]>) -> Self {
        let state = if spec.msg < 0 {
            ButtonState::Locked
        } else {
            ButtonState::Normal
        };
        let mut button = Self {
            progress: spec.progress,
            spec,
            state,
            clickable: state != ButtonState::Locked,
            bars: None,
            frame_lock: false,
        };
        if let Some(at) = mouse {
            button.mouse(HudEventKind::MouseMove, at, &mut Vec::new());
        }
        button
    }

    /// Current progress (`+0x9C`).
    pub fn progress(&self) -> f32 {
        self.progress
    }

    /// Value that tooltip and event carry as `BUTTONMSG`: the HID, otherwise the message.
    fn value_id(&self) -> i32 {
        self.spec.object.map_or(self.spec.msg, |hid| hid as i32)
    }

    fn is_lockable(&self) -> bool {
        LOCKABLE_KINDS.contains(&self.spec.kind())
    }

    /// Raw mouse (`0x0044EC60` movement, `0x0044EC00` press, `0x0044F860`/`0x0044F910` release,
    /// `0x0044F9B0`/`0x0044FA40` double-click). Returns whether the button consumes the event. Hit test like `PtInRect`
    /// (left and top inclusive, right and bottom exclusive).
    ///
    /// - Movement: outside state 0 (also from 3, O-261), not consumed; inside consumed in state 2, otherwise state 1,
    ///   not consumed.
    /// - Press (3, 5): inside always consumed; if the button is clickable and in state 1, it becomes 2.
    /// - Release (2, 4): inside always consumed; clickable and in state 2: state 1, click sound, event 9 or
    ///   `0x26`.
    /// - Double-click (`0x13`, `0x12`): inside consumed; clickable and in state 1: click sound, event `0x27` or
    ///   `0x28`. A double-click therefore yields 9, 9, `0x27` with three sounds (O-260).
    ///
    /// A button that is not clickable consumes press, release and double-click inside silently and without effect.
    pub fn mouse(&mut self, raw: HudEventKind, at: [i32; 2], out: &mut Vec<ButtonOut>) -> bool {
        let inside = contains(self.spec.rect, at[0], at[1]);
        match raw {
            HudEventKind::MouseMove => {
                if !inside {
                    self.state = ButtonState::Normal;
                    false
                } else if self.state == ButtonState::Pressed {
                    true
                } else {
                    self.state = ButtonState::Hover;
                    false
                }
            }
            HudEventKind::LeftDown | HudEventKind::RightDown => {
                if inside && self.clickable && self.state == ButtonState::Hover {
                    self.state = ButtonState::Pressed;
                }
                inside
            }
            HudEventKind::LeftUp | HudEventKind::RightUp => {
                if inside && self.clickable && self.state == ButtonState::Pressed {
                    self.state = ButtonState::Hover;
                    let ev = if raw == HudEventKind::LeftUp {
                        ButtonEvent::Left
                    } else {
                        ButtonEvent::Right
                    };
                    self.emit(ev, out);
                }
                inside
            }
            HudEventKind::DoubleLeft | HudEventKind::DoubleRight => {
                if inside && self.clickable && self.state == ButtonState::Hover {
                    let ev = if raw == HudEventKind::DoubleLeft {
                        ButtonEvent::Double
                    } else {
                        ButtonEvent::RightDouble
                    };
                    self.emit(ev, out);
                }
                inside
            }
            _ => false,
        }
    }

    /// Click sound, then event (order as in `0x0044F8EF`/`0x0044F8FC`).
    fn emit(&self, ev: ButtonEvent, out: &mut Vec<ButtonOut>) {
        out.push(ButtonOut::Sound(ToolbarSound::Click));
        out.push(ButtonOut::Event {
            msg: self.spec.msg,
            flags: self.spec.flags,
            object: self.spec.object,
            ev,
        });
    }

    /// Frame event 6: the state part of `0x0044F250` and the tooltip request (N.5.1, N.14.3, N.14.6).
    ///
    /// 1. `progress` (event `0xF`) replaces the progress.
    /// 2. Lock (only the lockable kinds, `LOCKABLE_KINDS`): the state from the start of the frame is remembered. If a
    ///    counter-upgrade is running (COUNTERUPGRADE only), the state becomes 3. If a prerequisite is missing, the
    ///    **current** state is remembered (so 3 after a counter-upgrade) and then 3 is set. `clickable` is
    ///    `state != 3`.
    /// 3. At the end of drawing (`0x0044F6CA`) the remembered state is written back: the lock only applies to this
    ///    frame, only `clickable` stays (O-261).
    /// 4. Tooltip for state 1/2 (after the write-back):
    ///    - if a prerequisite is missing: kind `0x9000000` with the list and value 0, state 2 becomes 1;
    ///    - otherwise GUID and kind of the button; value for production is the progress (exactly 1.0 reports 0),
    ///      otherwise 1.
    ///    - **Without a missing prerequisite** the button then still has state 3 (from the initial lock or because a
    ///      previous frame with a counter-upgrade **and** a missing prerequisite stored 3) and is COUNTERUPGRADE: GUID
    ///      602 without a mouse check, overrides any earlier request (`0x0044F756...0x0044F79A`, O-245). It holds
    ///      every frame until a mouse movement sets the state to 0/1. In the missing-prerequisite branch there is no
    ///      tooltip for state 3 (`0x0044F7A7`: only state 1/2, otherwise `JNZ 0x0044F821` returns).
    ///
    /// A request replaces the previous one in `tooltip` (the last one wins, N.14.6); without a cause it stays.
    pub fn update(
        &mut self,
        locks: &ButtonLocks,
        progress: Option<f32>,
        bars: Option<&ButtonBars>,
        tooltip: &mut Option<TooltipRequest>,
    ) {
        if let Some(p) = progress {
            self.progress = p;
        }
        self.bars = bars.copied();
        let kind = self.spec.kind();
        let lockable = self.is_lockable();
        let missing = lockable && locks.missing_required;
        let mut saved = self.state;
        if lockable && kind == kinds::COUNTERUPGRADE && locks.counter_running {
            self.state = ButtonState::Locked;
        }
        if missing {
            saved = self.state;
            self.state = ButtonState::Locked;
        }
        self.clickable = self.state != ButtonState::Locked;
        self.frame_lock = self.state == ButtonState::Locked;
        self.state = saved;
        let over = matches!(self.state, ButtonState::Hover | ButtonState::Pressed);
        if missing {
            if over {
                *tooltip = Some(TooltipRequest {
                    guid: self.spec.msg,
                    kind: KIND_MISSING_REQUIREMENT,
                    value: 0.0,
                    required: self.spec.required,
                });
                if self.state == ButtonState::Pressed {
                    self.state = ButtonState::Hover;
                }
            }
        } else if over {
            let value = if kind == KIND_PRODUCTION {
                if self.progress == 1.0 {
                    0.0
                } else {
                    self.progress
                }
            } else {
                1.0
            };
            *tooltip = Some(TooltipRequest {
                guid: self.value_id(),
                kind,
                value,
                required: [0; 5],
            });
        }
        // 602 only lies in the branch without a missing prerequisite (`0x0044F756`, reached via `0x0044F6D0` without a
        // jump to `0x0044F7A7`); the missing-prerequisite branch ends at state 3 with `JNZ 0x0044F821` (return).
        if !missing && self.state == ButtonState::Locked && kind == kinds::COUNTERUPGRADE {
            *tooltip = Some(TooltipRequest {
                guid: COUNTER_RUNNING_GUID,
                kind,
                value: 0.0,
                required: [0; 5],
            });
        }
    }

    /// State in which the frame draws: locked if [`Button::update`] has locked.
    fn draw_state(&self) -> ButtonState {
        if self.frame_lock {
            ButtonState::Locked
        } else {
            self.state
        }
    }

    /// Color of the icon (N.14.3 step 3): `[0]` mouse over or pressed, `[1]` normal, `[2]` locked; the selected stance
    /// (`0xA000004`) takes `[3]`, `[4]`, `[5]`.
    fn icon_color(&self, data: &ToolbarData, state: ButtonState) -> u32 {
        let selected_stance =
            self.spec.kind() == KIND_STANCE && self.spec.flags & STANCE_SELECTED != 0;
        let first = if selected_stance { 3 } else { 0 };
        let slot = match state {
            ButtonState::Hover | ButtonState::Pressed => first,
            ButtonState::Normal => first + 1,
            ButtonState::Locked => first + 2,
        };
        data.colors[slot]
    }

    fn sheet(r: SheetRef, data: &ToolbarData) -> Option<&Sheet> {
        match r {
            SheetRef::Icons(i) => data.icon_sheet(i),
            SheetRef::Resource => Some(&data.resource_icons),
            SheetRef::MapIcons => data.map_icons.as_ref(),
        }
    }

    /// Draws the base icon, the icon, the veil and the bars of the frame (`0x0044F250` steps 2 to 4); read-only. The
    /// state comes from the last [`Button::update`] (lock) and from the mouse events since.
    ///
    /// 1. **Base icon** white at `(left, top)`, scale `s`.
    /// 2. **Icon** at `(left + 2, top - 3)`, pressed `(left + 2, top - 1)`, fixed in pixels (not scaled with `s`,
    ///    O-254); size cell * `s`. Filter `Linear` only for `s != 1` (`FUN_004020E0` with `point = 0`), otherwise
    ///    `Point`.
    /// 3. **Veil** (`FUN_00402450`) for progress < 1 and kind production or UPGRADE: untextured from
    ///    `(pt.x + 2, pt.y)` to `(pt.x + (tw - 1)*s, pt.y + (1 - f)*(th - 1)*s)` with the icon point `pt` and the cell
    ///    of the icon sheet; color `[6 + state]` (O-255).
    /// 4. **Bars** (`FUN_0044ECD0`) for selection and hero buttons with attributes: `x = left + 3`, `y` from
    ///    `top - 3`, 2 px per bar, width `right - left - 2`; first the background, then the foreground with
    ///    `share * width` (O-256). An empty or reversed foreground is omitted.
    pub fn draw(&self, ctx: &mut UiContext<'_>, data: &ToolbarData, out: &mut Vec<DrawCmd>) {
        let [left, top, right, _] = self.spec.rect;
        let state = self.draw_state();
        let scale = self.spec.scale;
        let filter = if scale != 1.0 {
            Filter::Linear
        } else {
            Filter::Point
        };
        if let Some(sheet) = Self::sheet(self.spec.base.0, data) {
            tile_at(
                ctx,
                sheet,
                self.spec.base.1,
                [left, top],
                WHITE,
                scale,
                filter,
                out,
            );
        }
        let pt = [
            left + 2,
            top - if state == ButtonState::Pressed { 1 } else { 3 },
        ];
        let icon_sheet = Self::sheet(self.spec.icon.0, data);
        if let Some(sheet) = icon_sheet {
            let color = self.icon_color(data, state);
            tile_at(ctx, sheet, self.spec.icon.1, pt, color, scale, filter, out);
        }
        let kind = self.spec.kind();
        if let Some(sheet) = icon_sheet
            && self.progress < 1.0
            && (kind == KIND_PRODUCTION || kind == kinds::UPGRADE)
        {
            let [kb, kh] = sheet.tile;
            let x0 = pt[0] as f32 + 2.0;
            let x1 = (kb - 1) as f32 * scale + pt[0] as f32;
            let y1 = (1.0 - self.progress) * (kh - 1) as f32 * scale + pt[1] as f32;
            let color = data.colors[6 + state as usize];
            fill_rect([x0 as i32, pt[1], x1 as i32, y1 as i32], color, out);
        }
        if let Some(bars) = &self.bars
            && (kind == KIND_SELECTION || kind == KIND_HERO)
        {
            let x = left + 3;
            let width = (right - left - 2) as f32;
            let mut y = top - 3;
            for (fraction, (front, back)) in bars.tracked.iter().zip(BAR_COLORS) {
                let Some(fraction) = *fraction else { continue };
                let x1 = (x as f32 + width) as i32;
                fill_rect([x, y, x1, y + 2], back, out);
                let x1 = (x as f32 + fraction * width) as i32;
                if x1 > x {
                    fill_rect([x, y, x1, y + 2], front, out);
                }
                y += 2;
            }
        }
    }
}

/// A tile of a sheet at a point (`FUN_004020E0` with fill level 1): `x` to `pt.x + tw*s`, `y` to `pt.y + th*s`,
/// calculated in `f32` and truncated. Nothing without a texture or for an invalid tile.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tile_at(
    ctx: &mut UiContext<'_>,
    sheet: &Sheet,
    index: i32,
    pt: [i32; 2],
    color: u32,
    scale: f32,
    filter: Filter,
    out: &mut Vec<DrawCmd>,
) {
    let Some(texture) = sheet.texture(ctx) else {
        return;
    };
    let [kb, kh] = sheet.tile;
    let dst = [
        pt[0],
        pt[1],
        (pt[0] as f32 + kb as f32 * scale) as i32,
        (pt[1] as f32 + kh as f32 * scale) as i32,
    ];
    image_quad(
        ctx.assets, texture, sheet.tile, index, dst, dst, color, filter, out,
    );
}

#[cfg(test)]
mod tests {
    use nml_formats::localisation::Localisation;

    use super::*;
    use crate::assets::{Texts, UiAssets};
    use crate::coords::Screen;
    use crate::draw::{DrawCmd, Filter, TextureId};
    use crate::fonts::FontBook;
    use crate::hud::HudEventKind;
    use crate::hud::toolbar::data::Sheet;
    use crate::testutil::{TestVfs, test_vfs, tga_bottom_up};

    const MAKE: u32 = 0x200_0000;
    const BUILD: u32 = 0x100_0000;
    const UPGRADE: u32 = 0x300_0000;
    const SELECTION: u32 = 0x500_0000;
    const PRODUCTION: u32 = 0x600_0000;
    const STANCE: u32 = 0xA00_0000;
    const COUNTER: u32 = 0x1000_0000;
    const WHITE: u32 = 0xFFFF_FFFF;
    /// Inside and outside for the button at normal slot 0 `(245, 609)` at 1024 x 768.
    const INSIDE: [i32; 2] = [250, 620];
    const OUTSIDE: [i32; 2] = [100, 100];

    /// Test stand: image collection, fonts and bar data with distinguishable colors `0xFF000100 + i`.
    struct Rig {
        vfs: TestVfs,
        assets: UiAssets,
        texts: Texts,
        fonts: FontBook,
        data: ToolbarData,
    }

    fn rig() -> Rig {
        let vfs = test_vfs(&[
            (
                "data/icons/i0.tga",
                tga_bottom_up(200, 200, |_, _| [255; 4]),
            ),
            (
                "data/icons/resourcen_icons.tga",
                tga_bottom_up(128, 128, |_, _| [255; 4]),
            ),
        ]);
        let mut assets = UiAssets::new();
        let loc = Localisation::default();
        let texts = Texts::load(&vfs, &loc);
        let fonts = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        let data = ToolbarData {
            icon_sheets: vec![Sheet::new("data/icons/i0.tga", [50, 50])],
            colors: std::array::from_fn(|i| 0xFF00_0100 + i as u32),
            ..ToolbarData::default()
        };
        Rig {
            vfs,
            assets,
            texts,
            fonts,
            data,
        }
    }

    impl Rig {
        fn draw(&mut self, b: &Button) -> Vec<DrawCmd> {
            let mut ctx = UiContext {
                vfs: &self.vfs,
                assets: &mut self.assets,
                fonts: &self.fonts,
                texts: &self.texts,
                screen: Screen {
                    width: 1024,
                    height: 768,
                },
            };
            let mut out = Vec::new();
            b.draw(&mut ctx, &self.data, &mut out);
            out
        }

        fn tex(&self, path: &str) -> TextureId {
            self.assets.find(path).expect("image loaded")
        }
    }

    type Q = (Option<TextureId>, [i32; 4], u32, Filter);

    fn quads(cmds: &[DrawCmd]) -> Vec<Q> {
        cmds.iter()
            .map(|c| {
                let DrawCmd::Quad {
                    texture,
                    dst,
                    color,
                    filter,
                    ..
                } = *c
                else {
                    unreachable!()
                };
                (texture, dst, color, filter)
            })
            .collect()
    }

    /// Button at normal slot 0: `(245, 609)`, 50 x 50, base and icon from sheet 0 of the icons.
    fn spec(flags: u32) -> ButtonSpec {
        ButtonSpec {
            msg: 1000,
            flags,
            required: [0; 5],
            rect: [245, 609, 295, 659],
            base: (SheetRef::Icons(0), 0),
            icon: (SheetRef::Icons(0), 5),
            scale: 1.0,
            progress: 1.0,
            object: None,
        }
    }

    fn small(flags: u32) -> ButtonSpec {
        ButtonSpec {
            rect: [245, 609, 270, 634],
            scale: 0.5,
            ..spec(flags)
        }
    }

    fn locks(missing_required: bool, counter_running: bool) -> ButtonLocks {
        ButtonLocks {
            missing_required,
            counter_running,
        }
    }

    fn update(b: &mut Button, locks: ButtonLocks) -> Option<TooltipRequest> {
        let mut tip = None;
        b.update(&locks, None, None, &mut tip);
        tip
    }

    fn sound() -> ButtonOut {
        ButtonOut::Sound(ToolbarSound::Click)
    }

    fn event(flags: u32, ev: ButtonEvent) -> ButtonOut {
        ButtonOut::Event {
            msg: 1000,
            flags,
            object: None,
            ev,
        }
    }

    #[test]
    fn icon_sits_two_right_three_up_and_two_lower_when_pressed() {
        let mut rig = rig();
        let mut b = Button::new(spec(MAKE), Some(INSIDE));
        let q = quads(&rig.draw(&b));
        let tex = rig.tex("data/icons/i0.tga");
        // Base icon white at (left, top); icon in color [0] (mouse over) at (247, 606), size 50.
        assert_eq!(
            q[0],
            (Some(tex), [245, 609, 295, 659], WHITE, Filter::Point)
        );
        assert_eq!(
            q[1],
            (Some(tex), [247, 606, 297, 656], 0xFF00_0100, Filter::Point)
        );
        assert_eq!(q.len(), 2);
        // Normal: color [1]; pressed: [0] again, but two pixels lower, the base icon stays.
        let normal = Button::new(spec(MAKE), None);
        assert_eq!(quads(&rig.draw(&normal))[1].2, 0xFF00_0101);
        let mut out = Vec::new();
        assert!(b.mouse(HudEventKind::LeftDown, INSIDE, &mut out));
        assert_eq!(b.state, ButtonState::Pressed);
        let q = quads(&rig.draw(&b));
        assert_eq!(q[0].1, [245, 609, 295, 659]);
        assert_eq!(
            q[1],
            (Some(tex), [247, 608, 297, 658], 0xFF00_0100, Filter::Point)
        );
    }

    #[test]
    fn small_buttons_scale_the_icon_but_not_the_offset() {
        let mut rig = rig();
        let b = Button::new(small(SELECTION), None);
        let q = quads(&rig.draw(&b));
        let tex = rig.tex("data/icons/i0.tga");
        // 50 px tile at scale 0.5: 25 px, filter Linear; the offset (+2, -3) stays.
        assert_eq!(
            q[0],
            (Some(tex), [245, 609, 270, 634], WHITE, Filter::Linear)
        );
        assert_eq!(
            q[1],
            (Some(tex), [247, 606, 272, 631], 0xFF00_0101, Filter::Linear)
        );
    }

    #[test]
    fn stance_cells_follow_the_status_bar_grid_of_32() {
        // O-257, O-280: with a status bar `resourcen_icons.tga` has its 32 px grid (first loader); base icon and icon
        // are then 32 x 32 at scale 1.0 and protrude beyond the 25 px button.
        let mut rig = rig();
        rig.data.resource_icons = Sheet::new("data/icons/resourcen_icons.tga", [32, 32]);
        let stance = ButtonSpec {
            rect: [245, 609, 270, 634],
            base: (SheetRef::Resource, 15),
            icon: (SheetRef::Resource, 4),
            scale: 1.0,
            ..spec(STANCE)
        };
        let b = Button::new(stance, None);
        let cmds = rig.draw(&b);
        let tex = rig.tex("data/icons/resourcen_icons.tga");
        let q = quads(&cmds);
        assert_eq!(
            q[0],
            (Some(tex), [245, 609, 277, 641], WHITE, Filter::Point)
        );
        assert_eq!(
            q[1],
            (Some(tex), [247, 606, 279, 638], 0xFF00_0101, Filter::Point)
        );
        // Tile 4 in the 32 px grid of the 128 x 128 sheet: column 0, row 1 (instead of column 4 in the 24 px grid), UV
        // with the half-texel inset of `tile_quad`.
        let DrawCmd::Quad { uv, .. } = &cmds[1] else {
            unreachable!()
        };
        let t = |px: f32| px / 128.0;
        assert_eq!(*uv, [t(0.5), t(32.5), t(30.5), t(62.5)]);
    }

    #[test]
    fn stance_cells_are_24_px_at_scale_1_without_status_bar() {
        // O-257: without a status bar (`IsStatusBar 0`) stance and formation take the 24 px tiles of the bar at scale
        // 1.0 in the 25 px button.
        let mut rig = rig();
        let stance = ButtonSpec {
            rect: [245, 609, 270, 634],
            base: (SheetRef::Resource, 15),
            icon: (SheetRef::Resource, 4),
            scale: 1.0,
            ..spec(STANCE)
        };
        let b = Button::new(stance, None);
        let q = quads(&rig.draw(&b));
        let tex = rig.tex("data/icons/resourcen_icons.tga");
        assert_eq!(
            q[0],
            (Some(tex), [245, 609, 269, 633], WHITE, Filter::Point)
        );
        assert_eq!(
            q[1],
            (Some(tex), [247, 606, 271, 630], 0xFF00_0101, Filter::Point)
        );
    }

    #[test]
    fn selected_stance_takes_its_own_colors() {
        let mut rig = rig();
        let selected = ButtonSpec {
            base: (SheetRef::Resource, 15),
            icon: (SheetRef::Resource, 4),
            ..spec(STANCE | 4)
        };
        let color = |rig: &mut Rig, b: &Button| quads(&rig.draw(b))[1].2;
        let mut b = Button::new(selected.clone(), None);
        assert_eq!(color(&mut rig, &b), 0xFF00_0104);
        b.mouse(HudEventKind::MouseMove, INSIDE, &mut Vec::new());
        assert_eq!(color(&mut rig, &b), 0xFF00_0103);
        // STANCE is not one of the lockable kinds: color [3] stays.
        update(&mut b, locks(true, true));
        assert_eq!(color(&mut rig, &b), 0xFF00_0103);
        // An unselected stance stays at [0]/[1]/[2].
        let plain = Button::new(
            ButtonSpec {
                flags: STANCE,
                ..selected
            },
            Some(INSIDE),
        );
        assert_eq!(color(&mut rig, &plain), 0xFF00_0100);
    }

    #[test]
    fn veil_shrinks_from_the_top() {
        let mut rig = rig();
        let mut b = Button::new(
            ButtonSpec {
                progress: 0.25,
                ..spec(PRODUCTION)
            },
            None,
        );
        let mut q = quads(&rig.draw(&b));
        // Veil: x from pt.x + 2 = 249 to pt.x + 49 = 296, y from 606 to 606 + 0.75 * 49 = 642.75 -> 642;
        // color [6] (state 0), without a texture, after the icon.
        assert_eq!(q.len(), 3);
        assert_eq!(
            q[2],
            (None, [249, 606, 296, 642], 0xFF00_0106, Filter::Point)
        );
        // Mouse over: color [7]; pressed: the point moves two pixels lower, color [8].
        b.mouse(HudEventKind::MouseMove, INSIDE, &mut Vec::new());
        q = quads(&rig.draw(&b));
        assert_eq!(
            q[2],
            (None, [249, 606, 296, 642], 0xFF00_0107, Filter::Point)
        );
        b.mouse(HudEventKind::LeftDown, INSIDE, &mut Vec::new());
        q = quads(&rig.draw(&b));
        assert_eq!(
            q[2],
            (None, [249, 608, 296, 644], 0xFF00_0108, Filter::Point)
        );
        // The frame sets the progress: 0.75 -> height 0.25 * 49 = 12.25 -> 12.
        b.update(&ButtonLocks::default(), Some(0.75), None, &mut None);
        q = quads(&rig.draw(&b));
        assert_eq!(q[2].1, [249, 608, 296, 620]);
        // Without progress below 1.0 or with another kind there is no veil.
        let full = Button::new(spec(PRODUCTION), None);
        assert_eq!(quads(&rig.draw(&full)).len(), 2);
        let other = Button::new(
            ButtonSpec {
                progress: 0.25,
                ..spec(MAKE)
            },
            None,
        );
        assert_eq!(quads(&rig.draw(&other)).len(), 2);
        // UPGRADE also has the veil.
        let upgrade = Button::new(
            ButtonSpec {
                progress: 0.5,
                ..spec(UPGRADE)
            },
            None,
        );
        assert_eq!(quads(&rig.draw(&upgrade))[2].1, [249, 606, 296, 630]);
    }

    #[test]
    fn bars_follow_tracked_attributes_and_overhang_one_pixel() {
        let mut rig = rig();
        let mut b = Button::new(spec(SELECTION), None);
        let bars = ButtonBars {
            tracked: [Some(0.5), None, Some(1.0), Some(0.25)],
        };
        b.update(&ButtonLocks::default(), None, Some(&bars), &mut None);
        let q = quads(&rig.draw(&b));
        // Two quads for base and icon, then per bar background before foreground, 2 px high without gaps from
        // top - 3; width 50 - 2 = 48 from x = 248, so up to 296 (one pixel beyond the button, which ends at 295).
        let bar = |rect: [i32; 4], color| (None, rect, color, Filter::Point);
        assert_eq!(
            q[2..],
            [
                bar([248, 606, 296, 608], 0x80FF_0000),
                bar([248, 606, 272, 608], 0xFF00_FF00),
                bar([248, 608, 296, 610], 0x80EE_BBAA),
                bar([248, 608, 296, 610], 0xFFEE_BBAA),
                bar([248, 610, 296, 612], 0x80FF_FF44),
                bar([248, 610, 260, 612], 0xFFFF_FF44),
            ]
        );
        // Pressing does not move the bars.
        b.mouse(HudEventKind::MouseMove, INSIDE, &mut Vec::new());
        b.mouse(HudEventKind::LeftDown, INSIDE, &mut Vec::new());
        assert_eq!(quads(&rig.draw(&b))[2].1, [248, 606, 296, 608]);
        // 25 px buttons: width 23. Share 0 and negative draw no foreground.
        let mut s = Button::new(small(SELECTION), None);
        let bars = ButtonBars {
            tracked: [Some(0.0), Some(-0.5), None, Some(0.5)],
        };
        s.update(&ButtonLocks::default(), None, Some(&bars), &mut None);
        let q = quads(&rig.draw(&s));
        assert_eq!(
            q[2..],
            [
                bar([248, 606, 271, 608], 0x80FF_0000),
                bar([248, 608, 271, 610], 0x8088_DDFF),
                bar([248, 610, 271, 612], 0x80FF_FF44),
                bar([248, 610, 259, 612], 0xFFFF_FF44),
            ]
        );
        // Without a found object or with another kind no bars.
        s.update(&ButtonLocks::default(), None, None, &mut None);
        assert_eq!(quads(&rig.draw(&s)).len(), 2);
        let mut m = Button::new(spec(MAKE), None);
        m.update(&ButtonLocks::default(), None, Some(&bars), &mut None);
        assert_eq!(quads(&rig.draw(&m)).len(), 2);
        // Hero buttons (0x14000000) also have bars.
        let mut h = Button::new(spec(0x1400_0000), None);
        h.update(&ButtonLocks::default(), None, Some(&bars), &mut None);
        assert!(quads(&rig.draw(&h)).len() > 2);
    }

    #[test]
    fn click_sound_on_every_release_and_double_but_not_when_locked() {
        // Double-click: 3, 2, 3, 2, 0x13 -> 9, 9, 0x27 and three sounds (O-260); the sound comes before the event.
        let mut b = Button::new(spec(MAKE), Some(INSIDE));
        let mut out = Vec::new();
        for raw in [
            HudEventKind::LeftDown,
            HudEventKind::LeftUp,
            HudEventKind::LeftDown,
            HudEventKind::LeftUp,
            HudEventKind::DoubleLeft,
        ] {
            assert!(b.mouse(raw, INSIDE, &mut out));
        }
        assert_eq!(
            out,
            [
                sound(),
                event(MAKE, ButtonEvent::Left),
                sound(),
                event(MAKE, ButtonEvent::Left),
                sound(),
                event(MAKE, ButtonEvent::Double),
            ]
        );
        assert_eq!(
            [
                ButtonEvent::Left.code(),
                ButtonEvent::Right.code(),
                ButtonEvent::Double.code()
            ],
            [9, 0x26, 0x27]
        );
        // Right: 5, 4 -> sound and 0x26.
        out.clear();
        b.mouse(HudEventKind::RightDown, INSIDE, &mut out);
        b.mouse(HudEventKind::RightUp, INSIDE, &mut out);
        assert_eq!(out, [sound(), event(MAKE, ButtonEvent::Right)]);
        // Release without a previous press does nothing (consumed); outside it is not consumed.
        out.clear();
        assert!(b.mouse(HudEventKind::LeftUp, INSIDE, &mut out));
        assert!(!b.mouse(HudEventKind::LeftUp, OUTSIDE, &mut out));
        assert!(out.is_empty());
        // Locked: the events are consumed, but silently and without effect.
        update(&mut b, locks(true, false));
        assert!(!b.clickable);
        assert_eq!(b.state, ButtonState::Hover);
        for raw in [
            HudEventKind::LeftDown,
            HudEventKind::LeftUp,
            HudEventKind::RightDown,
            HudEventKind::RightUp,
            HudEventKind::DoubleLeft,
            HudEventKind::DoubleRight,
        ] {
            assert!(b.mouse(raw, INSIDE, &mut out), "{raw:?}");
        }
        assert!(out.is_empty());
        assert_eq!(b.state, ButtonState::Hover);
        // Outside, even a locked button consumes nothing.
        assert!(!b.mouse(HudEventKind::LeftDown, OUTSIDE, &mut out));
    }

    #[test]
    fn right_double_click_emits_0x28() {
        // O-223: the button sends 0x28 including the sound; the bar does not handle it.
        let mut b = Button::new(spec(MAKE), Some(INSIDE));
        let mut out = Vec::new();
        assert!(b.mouse(HudEventKind::DoubleRight, INSIDE, &mut out));
        assert_eq!(out, [sound(), event(MAKE, ButtonEvent::RightDouble)]);
        assert_eq!(ButtonEvent::RightDouble.code(), 0x28);
        assert_eq!(
            ButtonEvent::RightDouble.kind(),
            HudEventKind::ButtonRightDouble
        );
        assert_eq!(ButtonEvent::Left.kind(), HudEventKind::Button);
        assert_eq!(ButtonEvent::Right.kind(), HudEventKind::ButtonRight);
        assert_eq!(ButtonEvent::Double.kind(), HudEventKind::ButtonDouble);
        // A double-click only works in the state "mouse over".
        out.clear();
        b.mouse(HudEventKind::LeftDown, INSIDE, &mut out);
        b.mouse(HudEventKind::DoubleLeft, INSIDE, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn lock_holds_only_for_the_frame() {
        // O-261: the lock colors and locks only this frame; the state is written back.
        let mut rig = rig();
        let mut b = Button::new(spec(BUILD), Some(INSIDE));
        update(&mut b, locks(true, false));
        assert_eq!(b.state, ButtonState::Hover);
        assert!(!b.clickable);
        // Drawn locked: color [2].
        assert_eq!(quads(&rig.draw(&b))[1].2, 0xFF00_0102);
        // Without the lock in the next frame: clickable and color [0] again.
        update(&mut b, ButtonLocks::default());
        assert!(b.clickable);
        assert_eq!(quads(&rig.draw(&b))[1].2, 0xFF00_0100);
        // Pressed stays pressed if only a running counter-upgrade locks ...
        let mut counter = Button::new(spec(COUNTER), Some(INSIDE));
        counter.mouse(HudEventKind::LeftDown, INSIDE, &mut Vec::new());
        update(&mut counter, locks(false, true));
        assert_eq!(counter.state, ButtonState::Pressed);
        // ... with a missing prerequisite it falls back to "mouse over" (tooltip branch, N.5.1 step 5).
        update(&mut counter, locks(true, false));
        assert_eq!(counter.state, ButtonState::Hover);
        // Only counter-upgrade buttons lock through a running counter-upgrade.
        let mut other = Button::new(spec(BUILD), None);
        update(&mut other, locks(false, true));
        assert!(other.clickable);
        let mut counter = Button::new(spec(COUNTER), None);
        update(&mut counter, locks(false, true));
        assert!(!counter.clickable);
        assert_eq!(counter.state, ButtonState::Normal);
        // Kinds without a lock (selection) stay clickable.
        let mut sel = Button::new(spec(SELECTION), None);
        update(&mut sel, locks(true, true));
        assert!(sel.clickable);
    }

    #[test]
    fn initial_lock_is_lost_on_the_first_mouse_event() {
        // A button created with BUTTONMSG < 0 is locked until the first mouse event arrives (O-261).
        let mut rig = rig();
        let negative = ButtonSpec {
            msg: -1,
            ..spec(MAKE)
        };
        let mut b = Button::new(negative.clone(), None);
        assert_eq!(b.state, ButtonState::Locked);
        assert_eq!(quads(&rig.draw(&b))[1].2, 0xFF00_0102);
        let mut out = Vec::new();
        assert!(b.mouse(HudEventKind::LeftDown, INSIDE, &mut out));
        assert!(out.is_empty());
        assert_eq!(b.state, ButtonState::Locked);
        // Every mouse movement lifts the lock: outside 0, inside 1.
        b.mouse(HudEventKind::MouseMove, OUTSIDE, &mut out);
        assert_eq!(b.state, ButtonState::Normal);
        let mut b = Button::new(negative.clone(), None);
        b.mouse(HudEventKind::MouseMove, INSIDE, &mut out);
        assert_eq!(b.state, ButtonState::Hover);
        // With a mouse position the constructor replays the movement immediately (`0x0044F15C`).
        assert_eq!(
            Button::new(negative.clone(), Some(INSIDE)).state,
            ButtonState::Hover
        );
        assert_eq!(
            Button::new(negative, Some(OUTSIDE)).state,
            ButtonState::Normal
        );
        assert_eq!(
            Button::new(spec(MAKE), Some(INSIDE)).state,
            ButtonState::Hover
        );
    }

    #[test]
    fn mouse_states_follow_the_original_handlers() {
        let mut b = Button::new(spec(MAKE), None);
        let mut out = Vec::new();
        // Movement: inside 1 (not consumed), outside 0; the edge rule is PtInRect.
        assert!(!b.mouse(HudEventKind::MouseMove, [245, 609], &mut out));
        assert_eq!(b.state, ButtonState::Hover);
        assert!(!b.mouse(HudEventKind::MouseMove, [295, 620], &mut out));
        assert_eq!(b.state, ButtonState::Normal);
        assert!(!b.mouse(HudEventKind::MouseMove, [294, 658], &mut out));
        assert_eq!(b.state, ButtonState::Hover);
        assert!(!b.mouse(HudEventKind::MouseMove, [294, 659], &mut out));
        assert_eq!(b.state, ButtonState::Normal);
        // Pressed stays pressed as long as the mouse is inside (consumed), and falls to 0 outside.
        b.mouse(HudEventKind::MouseMove, INSIDE, &mut out);
        assert!(b.mouse(HudEventKind::LeftDown, INSIDE, &mut out));
        assert_eq!(b.state, ButtonState::Pressed);
        assert!(b.mouse(HudEventKind::MouseMove, INSIDE, &mut out));
        assert_eq!(b.state, ButtonState::Pressed);
        assert!(!b.mouse(HudEventKind::MouseMove, OUTSIDE, &mut out));
        assert_eq!(b.state, ButtonState::Normal);
        // Press without a previous movement (state 0) is consumed but changes nothing.
        assert!(b.mouse(HudEventKind::LeftDown, INSIDE, &mut out));
        assert_eq!(b.state, ButtonState::Normal);
        // Press outside is not consumed; foreign kinds likewise.
        assert!(!b.mouse(HudEventKind::LeftDown, OUTSIDE, &mut out));
        assert!(!b.mouse(HudEventKind::Frame, INSIDE, &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn tooltip_602_needs_running_and_missing_requirement() {
        // O-245. Only a running counter-upgrade: the button is locked but shows the ordinary tooltip.
        let counter = ButtonSpec {
            required: [7, 0, 0, 0, 0],
            ..spec(COUNTER)
        };
        let mut b = Button::new(counter.clone(), Some(INSIDE));
        let tip = update(&mut b, locks(false, true)).unwrap();
        assert_eq!((tip.guid, tip.kind, tip.value), (1000, COUNTER, 1.0));
        assert!(!b.clickable);
        assert_eq!(b.state, ButtonState::Hover);
        // Only a missing prerequisite: tooltip kind 0x9000000 with the list, value 0.
        let tip = update(&mut b, locks(true, false)).unwrap();
        assert_eq!(
            tip,
            TooltipRequest {
                guid: 1000,
                kind: 0x900_0000,
                value: 0.0,
                required: [7, 0, 0, 0, 0]
            }
        );
        // Previous frame: counter-upgrade running **and** a prerequisite missing. The frame stores state 3, the
        // missing-prerequisite branch sends no tooltip for state 3 (not even 602).
        let mut b = Button::new(counter.clone(), Some(INSIDE));
        assert_eq!(update(&mut b, locks(true, true)), None);
        assert_eq!(b.state, ButtonState::Locked);
        assert!(!b.clickable);
        // Following frame without a missing prerequisite (the prerequisite is there, the counter-upgrade is still
        // running or not): the state 3 from the start of the frame applies, 602 comes with the counter-upgrade kind,
        // without a mouse check.
        let tip = update(&mut b, ButtonLocks::default()).unwrap();
        assert_eq!((tip.guid, tip.kind), (602, COUNTER));
        assert_eq!(b.state, ButtonState::Locked);
        assert_eq!(update(&mut b, locks(false, true)).unwrap().guid, 602);
        // It holds until a mouse movement resets the state ...
        b.mouse(HudEventKind::MouseMove, OUTSIDE, &mut Vec::new());
        assert_eq!(b.state, ButtonState::Normal);
        assert_eq!(update(&mut b, ButtonLocks::default()), None);
        // ... and a movement inward yields the ordinary tooltip.
        let mut b = Button::new(counter.clone(), None);
        update(&mut b, locks(true, true));
        b.mouse(HudEventKind::MouseMove, INSIDE, &mut Vec::new());
        let tip = update(&mut b, ButtonLocks::default()).unwrap();
        assert_eq!((tip.guid, tip.kind), (1000, COUNTER));
        // The state 3 from the initial lock also triggers 602 for a counter-upgrade button.
        let mut c = Button::new(ButtonSpec { msg: -5, ..counter }, None);
        assert_eq!(update(&mut c, ButtonLocks::default()).unwrap().guid, 602);
        // Other kinds do not know 602.
        let mut m = Button::new(
            ButtonSpec {
                msg: -5,
                ..spec(MAKE)
            },
            None,
        );
        assert_eq!(update(&mut m, ButtonLocks::default()), None);
    }

    #[test]
    fn tooltip_follows_hover_and_overwrites_the_request() {
        let mut b = Button::new(spec(MAKE), None);
        assert_eq!(update(&mut b, ButtonLocks::default()), None);
        b.mouse(HudEventKind::MouseMove, INSIDE, &mut Vec::new());
        let mut tip = Some(TooltipRequest {
            guid: 1,
            kind: 1,
            value: 1.0,
            required: [0; 5],
        });
        b.update(&ButtonLocks::default(), None, None, &mut tip);
        assert_eq!(
            tip,
            Some(TooltipRequest {
                guid: 1000,
                kind: MAKE,
                value: 1.0,
                required: [0; 5]
            })
        );
        // With the mouse outside, the request of an earlier button stays untouched.
        b.mouse(HudEventKind::MouseMove, OUTSIDE, &mut Vec::new());
        let before = tip;
        b.update(&ButtonLocks::default(), None, None, &mut tip);
        assert_eq!(tip, before);
        // Selection buttons report the HID, the kind without the flags (GROUP_AND, stance bit).
        let sel = ButtonSpec {
            object: Some(77),
            ..spec(SELECTION | 1)
        };
        let mut b = Button::new(sel, Some(INSIDE));
        let t = update(&mut b, ButtonLocks::default()).unwrap();
        assert_eq!((t.guid, t.kind), (77, SELECTION));
        // Missing prerequisite and pressed: tooltip 0x9000000, the button falls back to "mouse over".
        let mut b = Button::new(
            ButtonSpec {
                required: [9, 0, 0, 0, 0],
                ..spec(BUILD)
            },
            Some(INSIDE),
        );
        b.mouse(HudEventKind::LeftDown, INSIDE, &mut Vec::new());
        assert_eq!(b.state, ButtonState::Pressed);
        let t = update(&mut b, locks(true, false)).unwrap();
        assert_eq!((t.kind, t.required[0]), (0x900_0000, 9));
        assert_eq!(b.state, ButtonState::Hover);
    }

    #[test]
    fn waiting_entry_reports_zero_progress() {
        // O-245: a waiting order (progress fixed 1.0) reports 0; the running one only for exactly 1.0.
        let mut waiting = Button::new(spec(PRODUCTION), Some(INSIDE));
        let t = update(&mut waiting, ButtonLocks::default()).unwrap();
        assert_eq!((t.kind, t.value), (PRODUCTION, 0.0));
        // The frame supplies the progress of the running order; every other value passes through unchanged.
        let mut running = Button::new(
            ButtonSpec {
                progress: 0.0,
                ..spec(PRODUCTION)
            },
            Some(INSIDE),
        );
        let mut tip = None;
        running.update(&ButtonLocks::default(), Some(0.4), None, &mut tip);
        assert_eq!(tip.unwrap().value, 0.4);
        running.update(&ButtonLocks::default(), Some(1.0), None, &mut tip);
        assert_eq!(tip.unwrap().value, 0.0);
        // The progress stays on the button if a frame supplies none.
        let mut tip = None;
        running.update(&ButtonLocks::default(), None, None, &mut tip);
        assert_eq!(tip.unwrap().value, 0.0);
        assert_eq!(running.progress(), 1.0);
        // Other kinds report a fixed 1.
        let mut up = Button::new(
            ButtonSpec {
                progress: 0.3,
                ..spec(UPGRADE)
            },
            Some(INSIDE),
        );
        assert_eq!(update(&mut up, ButtonLocks::default()).unwrap().value, 1.0);
    }

    #[test]
    fn missing_sheets_and_textures_draw_nothing() {
        let mut rig = rig();
        let b = Button::new(
            ButtonSpec {
                icon: (SheetRef::Icons(9), 0),
                base: (SheetRef::Icons(9), 0),
                ..spec(MAKE)
            },
            None,
        );
        assert!(rig.draw(&b).is_empty());
        rig.data.icon_sheets[0] = Sheet::new("data/icons/does-not-exist.tga", [50, 50]);
        assert!(rig.draw(&Button::new(spec(PRODUCTION), None)).is_empty());
    }
}
