//! Bar window of the command bar: events, mouse path, click paths and drawing.
//!
//! Basis: re/game-ui N.1 (event table), N.3 (mouse path), N.5.3/N.5.4 (click paths, dispatcher), N.14.2 (background
//! and order), N.14.7, N.14.9 and Ghidra (read only): constructor `0x00456630`, background `0x004574B0`, mouse
//! (slot 2) `0x00458460` with `FUN_00453790`/`0x004544E0`, event (slot 3) `0x00458510`, right-click `0x00457C50`,
//! full build `0x00457980`, click path `0x00458130`, kinds `0x0044CF50`, dispatcher `Toolbar_DispatchButtonCommand`
//! `0x0044C040`, second interface `0x00457E80` (`0x186A1` ...).
//!
//! # Distribution in the window tree
//!
//! The node `Toolbar` from [`crate::hud::tree`] calls [`Toolbar::handle`]: slot 3 -> [`Toolbar::event`], slot 2 ->
//! [`Toolbar::mouse`]. `mouse` first hands the raw mouse to the children (only for a visible bar: normal slots,
//! production, multi-selection, stance, formation, heroes, headhunters, then the cursor) and afterwards to slot 2 of
//! the bar. The first true ends the distribution.
//!
//! | Raw kind | Slot 3 (`event`) | Children (`mouse`, visible only) | Slot 2 (`mouse`) |
//! |---|---|---|---|
//! | 1 movement | - | all buttons (state 0/1, pressed: consumed inside) | consumed inside, also hidden; position kept |
//! | 3 / 2 left press / release | - | button inside consumes; release -> 9 | visible: consumed inside |
//! | `0x13` left double | - | button -> `0x27` | visible: consumed in the rectangle |
//! | 5 right press | - | button in the rectangle consumes | in the rectangle: set `+0x5BD`, consumed, also hidden |
//! | 4 right release | no cursor: right press began in the bar and pointer inside -> base menu, consumed; otherwise not | button -> `0x26`; cursor cancels | - |
//! | `0x12` right double | - | button -> `0x28` (sound only, O-223) | clear flag, not consumed (world) |
//! | `0x2D`/`0x2E` wheel | - | - | - |
//!
//! The position of the raw mouse is in `HudEvent::args = [x, y]` (UI pixels). Mouse movements and right press outside
//! the rectangle belong to the world: the original searches the target action there in slot 2
//! (`Input_PickTargetAction`, `FUN_00453790`); the reimplementation leaves it to `GameSession`.
//!
//! # Events (slot 3, `0x00458510`)
//!
//! Always, also hidden: 4 (see above), `{0x10, n}` (0 silent, 1 occupants, 2 full, 3 production, from 4 nothing),
//! `{0x11, w}` (w != 0 hides, w = 0 shows), `0x2F`/`0x30` (defense cursor closed). Only visible: 6 (no slot 3
//! content, the per-frame work is done by [`Toolbar::update`]), 8 (full build without its own sound), 9/`0x26`/
//! `0x27` (click path), `0x0A`, `0x19`, `0x23`-`0x25`, `0x29`, `0x31`/`0x32`. The cursor events `0x0B`, `0x14`,
//! `0x16`, `0x17` are replaced by intents of the cursor.
//!
//! # Frame: first `update`, then draw
//!
//! Each frame the host calls [`Toolbar::update`] (frame event 6: locks, progress, bars, tooltip) and afterwards the
//! read-only [`Toolbar::draw_background`] and [`Toolbar::draw_children`]. Between the two, `Hud` puts portrait,
//! counter-upgrades and tooltip areas (N.14.2).

use opennml_formats::gifcon::button_kind as kinds;

use crate::bkg_paint::image_quad;
use crate::coords::{Screen, toolbar_rect};
use crate::dialog::UiContext;
use crate::draw::{DrawCmd, Filter};
use crate::geom::{contains, offset};
use crate::hud::tree::{Slot, mouse_point};
use crate::hud::{HudEvent, HudEventKind};
use crate::keys::KeyContext;

use super::button::{
    Button, ButtonEvent, ButtonOut, KIND_HERO, KIND_PRODUCTION, KIND_SELECTION, KIND_STANCE,
    ToolbarOut, ToolbarSound, TooltipRequest,
};
use super::cursor::{
    BuildCursor, Cursor, CursorEnd, DefenseCursor, FortCursor, TargetCursor, WallPart,
};
use super::data::ToolbarData;
use super::model::ToolbarModel;
use super::set::{
    self, ButtonSet, DynamicKind, KIND_HEADHUNTER, KIND_OCCUPANT, KIND_TARGET_GUID,
    KIND_TARGET_PLAYER, MSG_BACK, MSG_CANCEL_OCCUPANTS, MSG_FORMATION_CURRENT, MSG_FORMATION_HIDE,
    MSG_OCCUPANTS, MSG_SCATTER, MSG_STANCE_FIRST, MSG_TARGET_PLAYER_FIRST, MSG_TRADE_BACK,
    MSG_TRADE_FIRST, MSG_UNQUARTER_ALL, ObjectRow,
};

/// Kind of the minimap buttons (`0x13000000`, 570-580).
pub const KIND_MINIMAP: u32 = 0x1300_0000;
/// First minimap GUID 570 "Center on action" (`0x0047F1E0`); up to 580 without 578 ([`ToolbarIntent::Minimap`]).
pub const MSG_MINIMAP_FIRST: i32 = 570;
/// Walls and palisades: BUILD opens the wall cursor for them (`FUN_00453530`).
pub const WALL_GUIDS: [u32; 5] = [0x9D0, 0x8A4, 0x51F, 0x3F3, 0x778];
/// Stance 420 ... 424 -> mode of command `0x12` (`Input_SubmitBehaviorCommand` 3, 4, 1, 2, 0).
pub const STANCE_MODES: [i32; 5] = [3, 4, 1, 2, 0];
/// Stance 421 "DEFEND AREA": the defense radius cursor for non-buildings.
pub const MSG_DEFEND: i32 = 421;
/// Pitch tent (travois): opens the tent cursor (`FUN_0044ACA0`).
pub const MSG_UNPACK_TENT: i32 = 453;
/// First formation 511 (`0x1FF`), up to 518.
pub const MSG_FORMATION_FIRST: i32 = 511;
/// First trade menu 490 (`0x1EA`), up to 493.
pub const MSG_TRADE_MENU_FIRST: i32 = 490;
/// Headhunter kind 0 (`0x1004`), up to 4102.
pub const MSG_HEADHUNTER_FIRST: i32 = 4100;

/// White: color of the background tiles (`0x004574B0`).
const WHITE: u32 = 0xFFFF_FFFF;

/// A button event to the click path `FUN_00458130`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolbarEvent {
    /// `{9 | 0x26 | 0x27, BUTTONMSG, kind}` of a button (or from outside via [`Toolbar::event`]).
    Click {
        /// `BUTTONMSG`; for hero, selection and occupant buttons the HID.
        msg: i32,
        /// Full kind word (the lower bits carry the index for selection and queue).
        kind: u32,
        /// HID for object buttons.
        object: Option<u32>,
        ev: ButtonEvent,
    },
    /// Key shortcut `0x186AA` -> `{9, msg, kind}` (N.5.7). Only the key handling creates it; the path is
    /// that of a left-click.
    Shortcut { msg: i32, kind: u32 },
}

/// A game action of the bar; the host converts it into commands according to K-88. "List" means: the GROUP_AND list of
/// the `BUTTONMSG` ([`ButtonSet::group_for`]), without a match the whole selection (O-272); for the "head only"
/// commands the host takes its first object.
///
/// The cursor intents carry world coordinates in `f32`; therefore there is no `Eq`.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolbarIntent {
    /// `0x0E` (`FUN_0044B490`): `count` orders, `upgrade` = flag 1 of the UPGRADE buttons.
    Make { guid: u32, count: u8, upgrade: bool },
    /// `0x0F` (`FUN_0044B4F0`): cancel; `index` 0 = running order, 1 ... waiting ones in list order (kind bits
    /// `& 0xFF`), `slot` = local slot.
    Cancel { index: i32, slot: i32, guid: u32 },
    /// A branch of the dispatcher `0x0044C040` with a command to the list (or, for 511 ... 518, the whole selection).
    Dispatch { msg: i32, objects: Vec<u32> },
    /// `0x12` to the whole selection with this mode (420 ... 424 -> 3, 4, 1, 2, 0).
    Stance(i32),
    /// `0x14` let an occupant (HID) out.
    Unquarter(u32),
    /// 416: `0x15` per object of the selection.
    UnquarterAll,
    /// Selection button left/double: object to the top of the selection (`0x0047FAB0`); the `{8}` of the host triggers
    /// the full build.
    ToFront(u32),
    /// Selection button right: select only this object (`0x0047A130` + `0x004891B0`).
    SelectOnly(u32),
    /// Selection button right with `KD_ADD_TO_GROUP`: deselect (`0x004801B0`).
    Deselect(u32),
    /// Hero button: select the hero alone; `camera` if it was already selected. The host adds the flag
    /// `+0x1AC & 0x100` of the object.
    Hero { hid: u32, camera: bool },
    /// Headhunter button (`FUN_0048B910`): select the headquarters `hq` alone (without `{8}`, O-270). Afterwards the
    /// click path runs a second time with kind SELECTHEADHUNTER (O-269).
    HeadhunterHq { art: u8, hq: u32 },
    /// Minimap button or key 570 ... 577, 579, 580 (branches `0x0E` ... of the dispatcher `0x0044C040`): the effect
    /// needs action stack, minimap, camera and flow, so the host implements it. `double` only for a double-click of a
    /// button of kind `0x13000000` (`*ev == 0x27`, `0x0044CAC5`), never for a key.
    Minimap { msg: i32, double: bool },
    /// `0x2F {guid, head}` (`FUN_0044B770`); `source` is the head of the list.
    CounterUpgrade { guid: u32, source: u32 },
    /// `0x2C {kind, target slot, GUID}` to the head of the list (`FUN_0044B700`), afterwards base menu.
    Hire { art: u8, target: u8, guid: u32 },
    /// `0x45` to the whole selection (`FUN_0044AEA0`): kind 0 ... 3 (trade menu 490 ... 493) and target slot.
    Trade { kind: i32, slot: u8 },
    /// `0x75` scatter (519; right-click on 509/510/519).
    Scatter,
    /// Build cursor, left click with a valid position and without dragging (`0x0044FBE0` -> event `0x0B`,
    /// `Input_SubmitPlaceBuildingCommand`): GUID, rotation in radians and world point `(x, y, z)`. The host reads
    /// Shift/Ctrl when executing (they only affect the queue of the command); the bar has then closed all cursors,
    /// without `{0x0A}` and without a rebuild (Ghidra `FUN_00452C10`).
    PlaceBuilding {
        guid: u32,
        rotation: f32,
        pos: [f32; 3],
    },
    /// Tent cursor (button 453, kind `0x1A`): pitch the tent of the unit `unit` (first selected object) at `(x, z)`
    /// (`FUN_00453490`, network command `0x5000002E`); the rotation is not used.
    PlaceTent { unit: u32, pos: [f32; 2] },
    /// Wall cursor, third click with `|dx| > 3`, `|dz| > 3` and a valid ring (`0x00450650` -> event `0x14`, network
    /// command `0x50000018`): all parts of the ring in ring order, part 0 is a corner.
    PlaceWall { parts: Vec<WallPart> },
    /// Targeting mode, left click with a valid target (`0x00451720` -> event `0x16`, `Input_IssueTargetAction`): the
    /// `BUTTONMSG`, the object under the pointer or otherwise the ground point `(x, z)`. The bar then rebuilds in full.
    TargetChosen {
        msg: i32,
        object: Option<u32>,
        ground: Option<[f32; 2]>,
    },
    /// Defense radius cursor, left click (`0x00450295` -> event `0x2F`, `0x0044BD60`): world point `(x, z)` under the
    /// pointer. The bar has closed the cursor.
    DefensePoint { x: f32, z: f32 },
}

/// The command bar (`0x00456630`).
#[derive(Debug, Clone)]
pub struct Toolbar {
    /// Positions and sheets.
    pub data: ToolbarData,
    /// Screen; the bar rectangle is `(0, H - 176, 640, H)`.
    pub screen: Screen,
    /// The buttons of the set building and `+0x5CC` (`shown_menu`).
    pub set: ButtonSet,
    /// Hero row (`+0x8C`); only changes through `0x23`-`0x25`.
    pub heroes: ObjectRow,
    /// Headhunter row (`+0x90`); only through `0x31`/`0x32`.
    pub headhunters: ObjectRow,
    /// `+0x524`: HID of the single selection, -1 for a multi-selection, 0 without a selection.
    pub selected: i32,
    /// `+0x48`: object under the mouse from `{0x29}` (only taken over for no or a multi-selection).
    pub hover: u32,
    /// `+0x1C`: visible (event `0x11`). The host mirrors it in `GameWindows::set_visible`.
    pub shown: bool,
    /// `+0x4C`: last mouse position (every movement and every right press, also outside).
    pub mouse_pos: [i32; 2],
    /// `+0x5BD`: the right press began on a free bar area.
    pub right_down_in_bar: bool,
    /// Open cursor (child behind the buttons). The original keeps three slots (`+0x51C`, `+0x520`, `+0x40`); the
    /// reimplementation one.
    pub cursor: Option<Cursor>,
    /// The cursor is in the child list **before** the buttons (O-265): a silent rebuild (`{0x10, 0}`), the rebuild of
    /// the production row (`{0x10, 3}`) and newly created buttons hang their children behind it, so it sees clicks on
    /// buttons first. Falls with the cursor.
    pub cursor_before_buttons: bool,
    /// `+0x18C`: formation of the buttons; -1 for a single selection, otherwise that of the world or the last chosen.
    pub formation: i32,
    /// `+0x44`: "full occupant view". The full build sets it to 1, the automatic view to 0, 415 to 1; `{0x10, 1}` and
    /// 415 open the full view with it, otherwise they rebuild in full (O-277).
    pub occupants_full: bool,
    /// `+0x1C` of the dispatcher: headhunter kind of the target player level.
    pub hunt_art: u8,
    /// `+0x20` of the dispatcher: slot of the chosen target player (`FUN_00478DE0`), `u8::MAX` without.
    pub hunt_target: u8,
    /// `+0x18` of the dispatcher: open trade menu 490 ... 493.
    pub trade_kind: i32,
    /// Type of the keyboard context (`Keyboard+0x90`), set in the full build ([`Toolbar::key_context`]).
    key_kind: i32,
    /// Selection GUID of the keyboard context (`Keyboard+0x98`), set in every set building ([`Toolbar::key_context`]).
    key_selection: i32,
}

impl Toolbar {
    /// Empty, visible bar (constructor `0x00456630`); hero and hunter row with their slots.
    pub fn new(data: ToolbarData, screen: Screen) -> Self {
        Self {
            heroes: ObjectRow::heroes(&data),
            headhunters: ObjectRow::headhunters(&data),
            data,
            screen,
            set: ButtonSet::default(),
            selected: 0,
            hover: 0,
            shown: true,
            mouse_pos: [0, 0],
            right_down_in_bar: false,
            cursor: None,
            cursor_before_buttons: false,
            formation: -1,
            occupants_full: false,
            hunt_art: 0,
            hunt_target: u8::MAX,
            trade_kind: MSG_TRADE_MENU_FIRST,
            key_kind: KeyContext::KIND_NOTHING,
            key_selection: 0,
        }
    }

    /// Keyboard context for `keys.ini` (re/game-ui D5.3) with the nation `nation` (nation of the local player + 1; the
    /// original sets that when setting the local player, `0x00430140`, not in the set building). Type and selection
    /// GUID are fixed by the set buildings:
    /// - Full build `0x00457980`: empty selection -> `0x004540E0` -> `FUN_0044B110(0)` (type 1, selection 0); first
    ///   object own -> `FUN_0044B110(first)` (type 3 with module 3, otherwise 2), then `Toolbar_BuildButtonSet` for the
    ///   first and every further object, and each call sets the selection GUID (`0x00455824` -> `0x0049CF60`): the
    ///   GUID of the **last** object applies (O-303). First object foreign: no `FUN_0044B110`, the type stays; alone
    ///   also no set building (old context), in a multi-selection the set buildings of the further objects run
    ///   anyway.
    /// - Submenu (`BuildButtonSet(msg, 1)`) and silent rebuild (`BuildButtonSet(+0x5CC, 1)`): selection = GUID of the
    ///   built menu; the type stays.
    /// - Button 415 (occupants) in the dispatcher: `Toolbar_DispatchButtonCommand` sets the selection directly to 415
    ///   (`0x0044C1FD`...`0x0044C208`, `0x0049CF60(0x19F)`) and only then sends `0x186A2`; if the bar falls back to
    ///   the full build there, that overwrites the GUID. `{0x10, 1}` does not run through the dispatcher and changes
    ///   nothing.
    /// - The occupant view itself, the production row and the dynamic levels build no set.
    pub fn key_context(&self, nation: i32) -> KeyContext {
        KeyContext {
            kind: self.key_kind,
            nation,
            selection: self.key_selection,
        }
    }

    /// Context of the full build `0x00457980` (see [`Toolbar::key_context`]).
    fn note_full_build(&mut self, model: &ToolbarModel) {
        let Some(first) = model.selection.first() else {
            self.key_kind = KeyContext::KIND_NOTHING;
            self.key_selection = 0;
            return;
        };
        if first.own {
            self.key_kind = if first.building {
                KeyContext::KIND_BUILDING
            } else {
                KeyContext::KIND_UNIT
            };
        }
        if (first.own || model.selection.len() > 1)
            && let Some(last) = model.selection.last()
        {
            self.key_selection = last.guid as i32;
        }
    }

    /// `+0x5CC`: GUID of the shown menu (0 after the addition of a multi-selection, D-234).
    pub fn shown_menu(&self) -> i32 {
        self.set.shown_menu
    }

    /// Bar rectangle `(0, H - 176, 640, H)` (O-34).
    pub fn rect(&self) -> [i32; 4] {
        toolbar_rect(self.screen)
    }

    /// New screen size: all buttons move with the top edge of the bar (positions are bar-relative). The written-back x
    /// of the heroes and hunters (O-35) stay. Same size: nothing (the host calls it every frame).
    pub fn resize(&mut self, screen: Screen) {
        if screen == self.screen {
            return;
        }
        let dy = toolbar_rect(screen)[1] - toolbar_rect(self.screen)[1];
        for b in self.children_mut() {
            b.spec.rect = offset(b.spec.rect, [0, dy]);
        }
        self.screen = screen;
    }

    /// All buttons in child order: set ([`ButtonSet::buttons`]), heroes, headhunters.
    pub fn buttons(&self) -> impl Iterator<Item = &Button> {
        self.set
            .buttons()
            .chain(self.heroes.buttons())
            .chain(self.headhunters.buttons())
    }

    fn children_mut(&mut self) -> impl Iterator<Item = &mut Button> {
        self.set
            .buttons_mut()
            .chain(self.heroes.buttons_mut())
            .chain(self.headhunters.buttons_mut())
    }

    /// Content of the node in the window tree: slot 3 -> [`Toolbar::event`], slot 2 -> [`Toolbar::mouse`] for raw mouse
    /// with a position ([`mouse_point`]); slot 2 does not take other events.
    pub fn handle(
        &mut self,
        slot: Slot,
        ev: &HudEvent,
        model: &ToolbarModel,
        out: &mut Vec<ToolbarOut>,
    ) -> bool {
        match slot {
            Slot::Event => self.event(ev, model, out),
            Slot::Mouse => mouse_point(ev).is_some_and(|at| self.mouse(ev.kind, at, model, out)),
        }
    }

    /// Slot 3 (`0x00458510`) according to the table in the module header. Returns whether the bar consumes the event.
    ///
    /// Nobody sets the operation lock `+0xCC` (4 and the left clicks consumed without effect) to 1 in the original
    /// (N.3); the reimplementation omits it.
    pub fn event(
        &mut self,
        ev: &HudEvent,
        model: &ToolbarModel,
        out: &mut Vec<ToolbarOut>,
    ) -> bool {
        let arg = |i: usize| ev.args.get(i).copied().unwrap_or(0);
        match ev.kind {
            HudEventKind::RightUp => self.cursor.is_none() && self.right_release(model, out),
            HudEventKind::Rebuild => {
                self.rebuild(arg(0), model, out);
                true
            }
            // `FUN_00452F90`: w != 0 -> `+0x1C = 0`, w = 0 -> `+0x1C = 1`.
            HudEventKind::ToolbarOnOff => {
                self.shown = arg(0) == 0;
                true
            }
            // `0x2F`: `FUN_00453140` (defense point, the intent comes from the cursor) and close the cursor `+0x40`;
            // `0x30`: only close.
            HudEventKind::DefensePoint | HudEventKind::DefenseCancel => {
                if matches!(self.cursor, Some(Cursor::Defense(_))) {
                    self.close_cursor();
                }
                true
            }
            _ if !self.shown => false,
            HudEventKind::SelectionChanged => {
                self.full_rebuild(model, false, out);
                false
            }
            HudEventKind::Button | HudEventKind::ButtonRight | HudEventKind::ButtonDouble => {
                let e = match ev.kind {
                    HudEventKind::Button => ButtonEvent::Left,
                    HudEventKind::ButtonRight => ButtonEvent::Right,
                    _ => ButtonEvent::Double,
                };
                let click = ToolbarEvent::Click {
                    msg: arg(0),
                    kind: arg(1) as u32,
                    object: None,
                    ev: e,
                };
                self.press(click, model, out);
                true
            }
            HudEventKind::CursorsOff => {
                self.close_cursor();
                true
            }
            // w != 0: all cursors gone, selection empty (`0x0047A130`, the host clears it), full build; not consumed.
            HudEventKind::Lock => {
                if arg(0) != 0 {
                    self.close_cursor();
                    let mut empty = model.clone();
                    empty.selection.clear();
                    self.full_rebuild(&empty, false, out);
                }
                false
            }
            HudEventKind::HeroOn => {
                let mouse = Some(self.mouse_pos);
                self.heroes
                    .add(arg(1) as u32, model, &self.data, self.screen, mouse);
                false
            }
            HudEventKind::HeroRemove => {
                self.heroes.remove(arg(1) as u32);
                false
            }
            HudEventKind::HeroClear => {
                self.heroes.clear();
                false
            }
            // `FUN_00453120`: only for `+0x524` 0 or -1.
            HudEventKind::Hover => {
                if self.selected == 0 || self.selected == -1 {
                    self.hover = arg(0) as u32;
                }
                false
            }
            HudEventKind::HeadhunterOn => {
                let mouse = Some(self.mouse_pos);
                self.headhunters
                    .add(arg(1) as u32, model, &self.data, self.screen, mouse);
                false
            }
            HudEventKind::HeadhunterClear => {
                self.headhunters.clear();
                false
            }
            _ => false,
        }
    }

    /// Children and slot 2 (`0x00458460`) according to the table in the module header; `at` in UI pixels. The buttons
    /// run in child order, the first true ends the distribution; their events 9/`0x26`/`0x27` go immediately (like the
    /// synchronous `0x004529F0`) into the click path [`Toolbar::press`], `0x28` does not (O-223).
    pub fn mouse(
        &mut self,
        raw: HudEventKind,
        at: [i32; 2],
        model: &ToolbarModel,
        out: &mut Vec<ToolbarOut>,
    ) -> bool {
        if matches!(raw, HudEventKind::MouseMove | HudEventKind::RightDown) {
            self.mouse_pos = at;
        }
        if self.shown {
            // O-265: if the cursor is in front of the buttons, it sees the mouse first.
            if self.cursor_before_buttons && self.cursor_mouse(raw, model, out) {
                return true;
            }
            let mut events = Vec::new();
            let taken = self.children_mut().any(|b| b.mouse(raw, at, &mut events));
            for e in events {
                match e {
                    ButtonOut::Sound(s) => out.push(ToolbarOut::Sound(s)),
                    ButtonOut::Event {
                        ev: ButtonEvent::RightDouble,
                        ..
                    } => {}
                    ButtonOut::Event {
                        msg,
                        flags,
                        object,
                        ev,
                    } => self.press(
                        ToolbarEvent::Click {
                            msg,
                            kind: flags,
                            object,
                            ev,
                        },
                        model,
                        out,
                    ),
                }
            }
            // The cursor normally hangs behind the buttons and gets the raw mouse afterwards.
            if taken || (!self.cursor_before_buttons && self.cursor_mouse(raw, model, out)) {
                return true;
            }
        }
        let inside = contains(self.rect(), at[0], at[1]);
        match raw {
            HudEventKind::MouseMove => inside,
            HudEventKind::RightDown => {
                if inside {
                    self.right_down_in_bar = true;
                }
                inside
            }
            // `0x004584C0`: `+0x5BC = 1`, `+0x5BD = 0`, then the target action of the world.
            HudEventKind::DoubleRight => {
                self.right_down_in_bar = false;
                false
            }
            HudEventKind::LeftDown | HudEventKind::LeftUp | HudEventKind::DoubleLeft => {
                self.shown && inside
            }
            _ => false,
        }
    }

    /// Right release without a cursor (`0x00457C50`): if the right press began in the bar and the pointer is in the
    /// rectangle, base menu and consumed; the flag falls in any case. A right press in the world belongs to it.
    fn right_release(&mut self, model: &ToolbarModel, out: &mut Vec<ToolbarOut>) -> bool {
        let in_bar = std::mem::take(&mut self.right_down_in_bar);
        if in_bar && contains(self.rect(), self.mouse_pos[0], self.mouse_pos[1]) {
            self.full_rebuild(model, true, out);
            true
        } else {
            false
        }
    }

    /// `{0x10, n}` (`0x00458510`): 0 silent (`0x00457DB0`, without sound, cursors stay, D-234; without a shown menu,
    /// for a multi-selection or construction site full build), 1 full occupant view for `+0x44`, otherwise full build,
    /// 2 full build, 3 only the production row, from 4 nothing.
    fn rebuild(&mut self, n: i32, model: &ToolbarModel, out: &mut Vec<ToolbarOut>) {
        match n {
            0 => {
                let mouse = Some(self.mouse_pos);
                match set::rebuild_shown(model, self.set.shown_menu, &self.data, self.screen, mouse)
                {
                    Some(s) => {
                        if s.occupants_auto {
                            self.occupants_full = false;
                        }
                        self.key_selection = self.set.shown_menu;
                        self.set = s;
                        self.buttons_rebuilt();
                    }
                    None => self.full_rebuild(model, true, out),
                }
            }
            1 => self.occupants(model, out),
            2 => self.full_rebuild(model, true, out),
            3 => {
                let mouse = Some(self.mouse_pos);
                self.set.production =
                    set::production_buttons(model, &self.data, self.screen, mouse);
                self.buttons_rebuilt();
            }
            _ => {}
        }
    }

    /// Full build `0x00457980`: cursors gone, `+0x44 = 1`, selection sound for a single selection of an own or neutral
    /// object, set building with automatic occupant view, `+0x524`, formation.
    ///
    /// `sound = false` only for `{8}`: there the host already plays the selection sound in `selection_changed`. All
    /// other full builds of the bar (`{0x10, 2}`, fallbacks from `{0x10, 0/1}`, base menu via 409/417/494, right-click
    /// and trade/hire) report it like the original (`0x00457A4D`, `0x00457A95`; O-275).
    fn full_rebuild(&mut self, model: &ToolbarModel, sound: bool, out: &mut Vec<ToolbarOut>) {
        self.close_cursor();
        self.occupants_full = true;
        if let [one] = model.selection.as_slice()
            && sound
            && (one.own || one.neutral)
        {
            out.push(ToolbarOut::Sound(ToolbarSound::Select));
        }
        let menu = set::choose_menu(model).menu().unwrap_or(0);
        let mouse = Some(self.mouse_pos);
        self.set = set::build_set(model, menu, true, &self.data, self.screen, mouse);
        self.note_full_build(model);
        if self.set.occupants_auto {
            self.occupants_full = false;
        }
        self.selected = match model.selection.as_slice() {
            [] => 0,
            [one] => one.hid as i32,
            _ => -1,
        };
        self.formation = if model.selection.len() > 1 {
            model.formation
        } else {
            -1
        };
    }

    /// Submenu `msg` (`FUN_00454120`, `BuildButtonSet(msg, 1)`, `FUN_00457710`): without a stack, `+0x5CC = msg`.
    fn submenu(&mut self, model: &ToolbarModel, msg: i32) {
        let mouse = Some(self.mouse_pos);
        self.set = set::build_set(model, msg, false, &self.data, self.screen, mouse);
        self.key_selection = msg;
        self.buttons_rebuilt();
    }

    /// `{0x10, 1}` and `0x186A2` (415): with `+0x44` the full occupant view (`FUN_00454310(1)`, `FUN_00457710`),
    /// otherwise full build (`0x00457EDA`).
    fn occupants(&mut self, model: &ToolbarModel, out: &mut Vec<ToolbarOut>) {
        if self.occupants_full {
            set::occupant_view(&mut self.set, model, &self.data, self.screen, true);
            self.set.replay_mouse(self.mouse_pos);
            self.buttons_rebuilt();
        } else {
            self.full_rebuild(model, true, out);
        }
    }

    /// `0x186A3` (`0x00457F7C`) and the dynamic buttons `0x186A4`. `0x186A3` clears two button lists of the window:
    /// `+0xE0` (`FUN_00453450`) with the normal **and the stance buttons** (`AddTail` in `0x00453EF8` and
    /// `0x0045460F`) and `+0xFC` (`FUN_00453BF0`) with the production **and the multi-selection** (`0x004542EF`,
    /// `0x00454077`). Only the formation (`+0x118`, `0x00454739`) stays; `0x186A4` then only creates normal buttons
    /// (`FUN_00453D70`). Evidence from the original: target player level without stance.
    fn dynamic(&mut self, model: &ToolbarModel, kind: DynamicKind) {
        let mut buttons = set::dynamic_buttons(kind, model, &self.data, self.screen);
        set::replay_mouse(buttons.iter_mut().flatten(), self.mouse_pos);
        self.set.normal = buttons;
        self.set.stance.clear();
        self.set.production.clear();
        self.set.selection.clear();
        self.buttons_rebuilt();
    }

    /// List of a command (`FUN_00454950` and `FUN_0044CF50`): the GROUP_AND list of the BUTTONMSG, otherwise the whole
    /// selection.
    fn list(&self, model: &ToolbarModel, msg: i32) -> Vec<u32> {
        self.set
            .group_for(msg)
            .map_or_else(|| whole(model), <[u32]>::to_vec)
    }

    /// Click path `FUN_00458130` (N.5.3):
    /// 1. First always `ButtonPressed(value)` (`0x0042ED10`, O-246; value = BUTTONMSG, for object buttons the HID).
    /// 2. Then per kind:
    ///    - BUILD/ONLYONCE: nothing in the visible pause (O-231), left build or wall cursor;
    ///    - multi-selection: right deselect alone or with `KD_ADD_TO_GROUP`, otherwise to the top;
    ///    - TARGET: left submenu of the BUTTONMSG and targeting mode;
    ///    - hero: left, if the hero is still alive ([`ToolbarModel::heroes`]);
    ///    - headhunter: with an own HQ select it and go the path again with kind SELECTHEADHUNTER (second
    ///      `ButtonPressed`, O-269);
    ///    - otherwise the kinds of `FUN_0044CF50` ([`Toolbar::kind_path`]); if none consumes, left opens the submenu of
    ///      the BUTTONMSG.
    pub fn press(&mut self, e: ToolbarEvent, model: &ToolbarModel, out: &mut Vec<ToolbarOut>) {
        let (msg, flags, object, ev) = match e {
            ToolbarEvent::Click {
                msg,
                kind,
                object,
                ev,
            } => (msg, kind, object, ev),
            ToolbarEvent::Shortcut { msg, kind } => (msg, kind, None, ButtonEvent::Left),
        };
        let value = object.map_or(msg, |hid| hid as i32);
        out.push(ToolbarOut::ButtonPressed(value));
        let left = ev == ButtonEvent::Left;
        match flags & kinds::KIND_MASK {
            kinds::BUILD | kinds::ONLYONCE => {
                if model.visible_pause || !left {
                    return;
                }
                let guid = msg as u32;
                self.open_cursor(if WALL_GUIDS.contains(&guid) {
                    Cursor::Fort(FortCursor::new(guid))
                } else {
                    Cursor::Build(BuildCursor::new(guid))
                });
            }
            KIND_SELECTION => {
                let hid = value as u32;
                let intent = match ev {
                    ButtonEvent::Right if model.keys.add_to_group => ToolbarIntent::Deselect(hid),
                    ButtonEvent::Right => ToolbarIntent::SelectOnly(hid),
                    _ => ToolbarIntent::ToFront(hid),
                };
                out.push(ToolbarOut::Intent(intent));
            }
            kinds::TARGET => {
                if left {
                    self.submenu(model, msg);
                    let icon = model.target_icons.get(&msg).copied().unwrap_or(0);
                    self.open_cursor(Cursor::Target(TargetCursor::new(msg, icon)));
                }
            }
            KIND_HERO => {
                let hid = value as u32;
                if left && model.heroes.iter().any(|h| h.0 == hid) {
                    let camera = model.selection.iter().any(|s| s.hid == hid);
                    out.push(ToolbarOut::Intent(ToolbarIntent::Hero { hid, camera }));
                }
            }
            KIND_HEADHUNTER => {
                if let Some(hq) = model.headquarters {
                    let art = msg.wrapping_sub(MSG_HEADHUNTER_FIRST) as u8;
                    out.push(ToolbarOut::Intent(ToolbarIntent::HeadhunterHq { art, hq }));
                    let again = ToolbarEvent::Click {
                        msg,
                        kind: kinds::SELECTHEADHUNTER,
                        object: None,
                        ev: ButtonEvent::Left,
                    };
                    self.press(again, model, out);
                }
            }
            _ => {
                if !self.kind_path(msg, flags, ev, model, out) && left {
                    self.submenu(model, msg);
                }
            }
        }
    }

    /// The kinds of `FUN_0044CF50`; `true` if one consumes the click.
    fn kind_path(
        &mut self,
        msg: i32,
        flags: u32,
        ev: ButtonEvent,
        model: &ToolbarModel,
        out: &mut Vec<ToolbarOut>,
    ) -> bool {
        let guid = msg as u32;
        match flags & kinds::KIND_MASK {
            // `FUN_0044AA10(color)`: target slot from the color, then the target GUID level.
            KIND_TARGET_PLAYER => {
                let color = msg - MSG_TARGET_PLAYER_FIRST;
                self.hunt_target = model
                    .enemy_players
                    .iter()
                    .find(|p| i32::from(p.1) == color)
                    .map_or(u8::MAX, |p| p.0);
                let kind = DynamicKind::TargetGuid {
                    art: self.hunt_art,
                    slot: self.hunt_target,
                };
                self.dynamic(model, kind);
            }
            KIND_PRODUCTION => intent(
                out,
                ToolbarIntent::Cancel {
                    index: (flags & 0xFF) as i32,
                    slot: i32::from(model.local_slot),
                    guid,
                },
            ),
            // `0x0044D03E`: 1x; double another 1x; otherwise with Shift/Ctrl/Alt another 4x.
            kinds::MAKE => {
                let k = model.keys;
                let count = if ev == ButtonEvent::Double {
                    2
                } else if k.shift || k.control || k.alt {
                    5
                } else {
                    1
                };
                intent(
                    out,
                    ToolbarIntent::Make {
                        guid,
                        count,
                        upgrade: false,
                    },
                );
            }
            kinds::UPGRADE | kinds::MAKEONLY_ONCE => intent(
                out,
                ToolbarIntent::Make {
                    guid,
                    count: 1,
                    upgrade: flags & kinds::KIND_MASK == kinds::UPGRADE,
                },
            ),
            // Stance (and minimap): the dispatcher for every click kind (`0x0044D0F0`, O-278).
            KIND_STANCE => return self.dispatch(msg, false, model, out),
            // `FUN_0044CF50`: the dispatcher for every click kind; the double-click (`0x27`) only works for 576/577.
            KIND_MINIMAP => {
                self.dispatch(msg, ev == ButtonEvent::Double, model, out);
            }
            KIND_OCCUPANT => intent(out, ToolbarIntent::Unquarter(guid)),
            // `FUN_0044B1F0`: unused in the shipped data (N.5.3).
            kinds::SHORTCUT => {}
            KIND_TARGET_GUID => {
                if !self.list(model, msg).is_empty() {
                    let hire = ToolbarIntent::Hire {
                        art: self.hunt_art,
                        target: self.hunt_target,
                        guid,
                    };
                    out.push(ToolbarOut::Intent(hire));
                    self.full_rebuild(model, true, out);
                }
            }
            kinds::COUNTERUPGRADE => {
                if let Some(&source) = self.list(model, msg).first() {
                    intent(out, ToolbarIntent::CounterUpgrade { guid, source });
                }
            }
            kinds::DO | kinds::TARGET_DO | kinds::SELECTHEADHUNTER => match ev {
                ButtonEvent::Right => {
                    if matches!(
                        msg,
                        MSG_FORMATION_HIDE | MSG_FORMATION_CURRENT | MSG_SCATTER
                    ) {
                        intent(out, ToolbarIntent::Scatter);
                    }
                }
                ButtonEvent::Left => return self.dispatch(msg, false, model, out),
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    /// `Toolbar_DispatchButtonCommand` `0x0044C040`: the branches of the bar itself and otherwise a command to the
    /// list, if the dispatcher knows the number ([`dispatcher_handles`]); `false` for unknown numbers. `double`:
    /// double-click of a minimap button ([`ToolbarIntent::Minimap`]).
    fn dispatch(
        &mut self,
        msg: i32,
        double: bool,
        model: &ToolbarModel,
        out: &mut Vec<ToolbarOut>,
    ) -> bool {
        let mouse = Some(self.mouse_pos);
        match msg {
            // `0x186A1`: base menu, without a stack (O-226).
            MSG_BACK | MSG_CANCEL_OCCUPANTS | MSG_TRADE_BACK => self.full_rebuild(model, true, out),
            // `0x0044C1FD`: first the selection of the keyboard context to 415 (`0x0049CF60(0x19F)`), then `0x186A2`.
            MSG_OCCUPANTS => {
                self.key_selection = MSG_OCCUPANTS;
                self.occupants(model, out);
            }
            MSG_UNQUARTER_ALL => intent(out, ToolbarIntent::UnquarterAll),
            420..=424 => {
                let i = (msg - MSG_STANCE_FIRST) as usize;
                if msg == MSG_DEFEND {
                    // `0x0044C1B9`: building mode 4, otherwise `0x186AC` defense radius cursor.
                    match model.selection.first() {
                        Some(f) if f.building => {
                            intent(out, ToolbarIntent::Stance(STANCE_MODES[i]))
                        }
                        Some(_) => self.open_cursor(Cursor::Defense(DefenseCursor::default())),
                        None => {}
                    }
                } else {
                    intent(out, ToolbarIntent::Stance(STANCE_MODES[i]));
                }
            }
            490..=493 => {
                self.trade_kind = msg;
                self.dynamic(model, DynamicKind::Trade(msg));
            }
            // `0x186A7`: formation closed or open (`FUN_00457250`).
            MSG_FORMATION_HIDE | MSG_FORMATION_CURRENT => {
                let open = msg == MSG_FORMATION_CURRENT;
                self.set.formation =
                    set::formation_buttons(self.formation, open, &self.data, self.screen, mouse);
            }
            // `FUN_0044ADE0`: `0x186A8` (select and collapse, `FUN_00457930`), then `0x44` to the whole selection.
            511..=518 => {
                self.formation = msg - MSG_FORMATION_FIRST;
                self.set.formation =
                    set::formation_buttons(self.formation, false, &self.data, self.screen, mouse);
                let objects = whole(model);
                intent(out, ToolbarIntent::Dispatch { msg, objects });
            }
            MSG_SCATTER => intent(out, ToolbarIntent::Scatter),
            // `FUN_0044AEA0`: base menu, then `0x45` to the whole selection.
            520..=528 => {
                self.full_rebuild(model, true, out);
                let trade = ToolbarIntent::Trade {
                    kind: self.trade_kind - MSG_TRADE_MENU_FIRST,
                    slot: (msg - MSG_TRADE_FIRST) as u8,
                };
                intent(out, trade);
            }
            // `FUN_0044A8C0(kind)`: target player level.
            4100..=4102 => {
                self.hunt_art = (msg - MSG_HEADHUNTER_FIRST) as u8;
                self.dynamic(model, DynamicKind::TargetPlayer(self.hunt_art));
            }
            // `FUN_0044ACA0` (button 453, pitch tent): first selected object, its tent (`0x423`) and no pause, then all
            // cursors closed (`0x186A5`) and the build cursor of kind `0x1A` (`0x186A6`, `FUN_004533C0`) with the GUID
            // of the tent. Without a tent or during the pause nothing happens (pause: `+0x24E4`, like O-231 as
            // `visible_pause`).
            MSG_UNPACK_TENT => {
                if let (Some(first), Some(tent)) = (model.selection.first(), model.tent_guid)
                    && !model.visible_pause
                {
                    self.open_cursor(Cursor::Build(BuildCursor::tent(tent, first.hid)));
                }
            }
            // Minimap (`0x0044CAxx`: 570 action, 571 signal, 572/580 diplomacy, 573 ... 575 mode, 576/577 idle workers,
            // 579 game menu): to the host.
            MSG_MINIMAP_FIRST..=577 | 579 | 580 => {
                intent(out, ToolbarIntent::Minimap { msg, double });
            }
            _ if dispatcher_handles(msg) => {
                let objects = self.list(model, msg);
                intent(out, ToolbarIntent::Dispatch { msg, objects });
            }
            _ => return false,
        }
        true
    }

    /// Frame event 6: per button in child order [`Button::update`] with locks, progress and bars of the frame
    /// (`0x0044F250` steps 1 and 5, `FUN_00452E40`); the tooltip request overrides the previous one in `tooltip` (the
    /// last one wins, N.14.6). The host first sets the request of the tooltip areas and calls drawing only afterwards.
    /// When hidden, the original does not draw the children and so does not calculate anything either.
    pub fn update(&mut self, model: &ToolbarModel, tooltip: &mut Option<TooltipRequest>) {
        if !self.shown {
            return;
        }
        for b in self.children_mut() {
            let locks = model.locks_for(&b.spec);
            let progress = model.progress_for(&b.spec);
            let bars = model.bars_for(&b.spec);
            b.update(&locks, progress, bars, tooltip);
        }
        // The cursor is a child of the bar and does its per-frame work here: in the original the build cursor on frame
        // event 6 (`0x00450020`), wall (`0x004513B0`), target (`0x00451890`) and defense cursor on the pointer movement
        // (event 1; `0x15` is only the drawing for the wall cursor). Position, rotation and validity.
        if let (Some(cursor), Some(frame)) = (&mut self.cursor, &model.cursor) {
            cursor.update(frame);
        }
    }

    /// World point `(x, z)` for the defense ring: only while the defense radius cursor is open, the bar is visible and
    /// the point under the pointer is on explored terrain (`0x004501AF`: otherwise `Engine+0x30D = 0`). In the original
    /// the ring is a terrain decal of the engine (`defenceradius.tga`, `0x0042F750`, radius tile width * 6); the cursor
    /// itself draws nothing.
    pub fn defense_ring(&self) -> Option<[f32; 2]> {
        match &self.cursor {
            Some(Cursor::Defense(d)) if self.shown && d.explored => d.point,
            _ => None,
        }
    }

    /// Verdict of the host for the position that the cursor assumed in the [`Toolbar::update`] that just ran:
    /// building/tent `valid` = placement and sight check of the new position and rotation, wall `ring_valid` = all
    /// parts of the new ring valid (`BuildCursor_CheckWallSegments` `0x00450870`), target `valid` = target valid,
    /// defense `valid` = point explored. The original checks in the same step in which it sets the position
    /// (`0x0044FE72` ... `0x0044FFC8`, `0x004513E6`); so a click between two frames always sees the verdict for the
    /// position it places at, even if the position changes every frame (edge scrolling). Without a cursor nothing.
    pub fn set_cursor_validity(&mut self, valid: bool, ring_valid: Option<bool>) {
        match &mut self.cursor {
            Some(Cursor::Build(b)) => b.valid = valid,
            Some(Cursor::Fort(f)) => f.ring_valid = ring_valid,
            Some(Cursor::Target(t)) => t.valid = valid,
            Some(Cursor::Defense(d)) => d.explored = valid,
            None => {}
        }
    }

    /// Open a cursor: all others are closed (`FUN_00452C10`), the new one hangs behind all buttons (`AddTail`).
    fn open_cursor(&mut self, cursor: Cursor) {
        self.cursor = Some(cursor);
        self.cursor_before_buttons = false;
    }

    /// Close all cursors (`FUN_00452C10`), without a rebuild.
    fn close_cursor(&mut self) {
        self.cursor = None;
        self.cursor_before_buttons = false;
    }

    /// Buttons were created anew (silent rebuild, production row, submenu, dynamic buttons): an open cursor now lies
    /// in front of them in the child list (O-265).
    fn buttons_rebuilt(&mut self) {
        if self.cursor.is_some() {
            self.cursor_before_buttons = true;
        }
    }

    /// Raw mouse to the cursor; `true` if it consumes it. Intents go to `out`, the end of the cursor is handled by this
    /// function: close (placing, cancel) or rebuild in full (target selected/canceled, `0x16`, `0x17`).
    fn cursor_mouse(
        &mut self,
        raw: HudEventKind,
        model: &ToolbarModel,
        out: &mut Vec<ToolbarOut>,
    ) -> bool {
        let Some(cursor) = &mut self.cursor else {
            return false;
        };
        match cursor.mouse(raw, out) {
            None => false,
            Some(CursorEnd::Keep) => true,
            Some(CursorEnd::Close) => {
                self.close_cursor();
                true
            }
            Some(CursorEnd::Rebuild) => {
                self.full_rebuild(model, true, out);
                true
            }
        }
    }

    /// Background (`0x004574B0`): for each `ICON x y sheet tile` of tag `TOOLBAR` in file order the tile at
    /// `(left + x, top + y)`, white, scale 1, POINT, **without clip** (O-262): `status_a_00` protrudes 128 px above the
    /// bar, the row `status_b` below the screen edge. A sheet outside the list aborts in the original (`FUN_005E86BF`);
    /// the reimplementation omits the tile. Nothing when hidden.
    pub fn draw_background(&self, ctx: &mut UiContext<'_>, out: &mut Vec<DrawCmd>) {
        if !self.shown {
            return;
        }
        let [left, top, ..] = self.rect();
        for icon in &self.data.background {
            let Some(sheet) = usize::try_from(icon.sheet)
                .ok()
                .and_then(|i| self.data.sheets.get(i))
            else {
                continue;
            };
            let Some(texture) = sheet.texture(ctx) else {
                continue;
            };
            let [x, y] = [left + icon.pos[0], top + icon.pos[1]];
            let dst = [x, y, x + sheet.tile[0], y + sheet.tile[1]];
            image_quad(
                ctx.assets,
                texture,
                sheet.tile,
                icon.tile,
                dst,
                dst,
                WHITE,
                Filter::Point,
                out,
            );
        }
    }

    /// Children in drawing order: the buttons ([`Toolbar::buttons`], read-only after [`Toolbar::update`]); the cursors
    /// draw nothing of their own (Draw = RET). The model stays for the cursors. Nothing when hidden.
    pub fn draw_children(
        &self,
        ctx: &mut UiContext<'_>,
        _model: &ToolbarModel,
        out: &mut Vec<DrawCmd>,
    ) {
        if !self.shown {
            return;
        }
        for b in self.buttons() {
            b.draw(ctx, &self.data, out);
        }
    }
}

/// Output an intent.
fn intent(out: &mut Vec<ToolbarOut>, i: ToolbarIntent) {
    out.push(ToolbarOut::Intent(i));
}

/// All HIDs of the selection in selection order.
fn whole(model: &ToolbarModel) -> Vec<u32> {
    model.selection.iter().map(|s| s.hid).collect()
}

/// Numbers with their own branch in `Toolbar_DispatchButtonCommand` (`0x0044C040`), without those that the bar itself
/// handles (409, 415-417, 420-424, 490-494, 509-528, 4100-4102) or passes to the host (570-577, 579, 580 as
/// [`ToolbarIntent::Minimap`]). All others yield 0 and, on a left click, open the submenu of their number, for
/// example 529, 541, 546, 548, 551, 556, 565-569 and 578 (N.5.4).
pub fn dispatcher_handles(msg: i32) -> bool {
    matches!(
        msg,
        399 | 405
            | 406
            | 408
            | 430..=439
            | 441..=443
            | 449..=453
            | 458
            | 459
            | 482
            | 485..=488
            | 530
            | 537..=540
            | 542..=545
            | 547
            | 549
            | 550
            | 552..=555
            | 557..=564
            | 620
            | 621
    )
}

#[cfg(test)]
mod tests;
