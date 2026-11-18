//! Set building of the command bar: which buttons the bar shows for the selection.
//!
//! Basis: re/game-ui N.2, N.5.2, N.5.5, N.14.4, N.14.7 and Ghidra (read only):
//! - `Toolbar_BuildButtonSet` `0x004557C0` fills the 14 normal slots (`+0x20C`) from a `toolbar.ini` menu;
//! - the full build `0x00457980` chooses the menu and calls it per selected object;
//! - `0x00457710` creates the buttons: normal slots (`0x00453D70`), production (`0x004573C0`), multi-selection
//!   (`0x00453F20`), stance (`0x00454520`), formation (`0x00457250`/`0x00457360`);
//! - occupants `0x004548E0`/`0x00454310`, dynamic buttons `0x0044B9A0`/`0x0044A8C0`/`0x0044AA10`, heroes and
//!   headhunters `0x004550E0`/`0x00454760`/`0x00454B20`/`0x004549B0`.
//!
//! Positions: normal, production, selection, stance and formation buttons are relative to the bar
//! (`(0, H - 176)`, `OffsetRect` with the window origin). Heroes and headhunters count a negative x from the right
//! edge of the screen (`W + x`) and then also lie relative to the bar, so outside its rectangle.
//!
//! Deviations (all without effect in the shipped data): where the original aborts or reads past the end for an
//! invalid icon sheet, a slot outside 0...13 or a position list that is too short, the reimplementation leaves the
//! button out.

use std::collections::BTreeMap;

use opennml_formats::gifcon::{ToolbarButton, button_kind as kinds};

use crate::coords::{Screen, toolbar_rect};
use crate::hud::HudEventKind;

use super::button::{
    Button, ButtonSpec, KIND_HERO, KIND_PRODUCTION, KIND_SELECTION, KIND_STANCE, STANCE_SELECTED,
    SheetRef,
};
use super::data::ToolbarData;
use super::model::ToolbarModel;

/// Kind of the occupant buttons (`0xB000000`, `FUN_00454310`).
pub const KIND_OCCUPANT: u32 = 0xB00_0000;
/// Kind of the target player buttons (`0xC000000`, `FUN_0044A8C0`).
pub const KIND_TARGET_PLAYER: u32 = 0xC00_0000;
/// Kind of the target GUID buttons (`0xE000000`, `FUN_0044AA10`).
pub const KIND_TARGET_GUID: u32 = 0xE00_0000;
/// Kind of the headhunter buttons (`0x16000000`, `FUN_004549B0`).
pub const KIND_HEADHUNTER: u32 = 0x1600_0000;

/// Number of normal slots (`+0x20C`, 14 words).
pub const SLOT_COUNT: usize = 14;
/// The only slot of a construction site (O-229) and the slot of the back buttons of the dynamic levels.
pub const SITE_SLOT: usize = 13;
/// First slot of the queue in `SELECTIONBUTTONS` (O-37); the running order is in slot 0.
pub const QUEUE_START: usize = 4;

/// Occupant view (`0x19F`).
pub const MSG_OCCUPANTS: i32 = 415;
/// Unquarter all (`0x1A0`).
pub const MSG_UNQUARTER_ALL: i32 = 416;
/// Cancel the occupant view (`0x1A1`).
pub const MSG_CANCEL_OCCUPANTS: i32 = 417;
/// Back from the headhunter levels (`0x199`).
pub const MSG_BACK: i32 = 409;
/// Back from the trade menu (`0x1EE`).
pub const MSG_TRADE_BACK: i32 = 494;
/// Trade with slot p: `520 + p` (`0x208 + p`).
pub const MSG_TRADE_FIRST: i32 = 520;
/// Target player: `470 + color` (`0x1D6 + color`).
pub const MSG_TARGET_PLAYER_FIRST: i32 = 470;
/// Stance slot i: `420 + i` (`0x1A4 + i`).
pub const MSG_STANCE_FIRST: i32 = 420;
/// Hide formation (`0x1FD`).
pub const MSG_FORMATION_HIDE: i32 = 509;
/// Current formation, collapsed (`0x1FE`).
pub const MSG_FORMATION_CURRENT: i32 = 510;
/// Scatter (`0x207`).
pub const MSG_SCATTER: i32 = 519;

/// The three heroes and the headquarters per nation 0 ... 5 for the target GUID level of the headhunter, fixed in the
/// code of `FUN_0044AA10` (`switch (Player+0x238)`; re/heroes-resources 2.2). Program logic, not a game data table.
pub const HUNT_TARGET_TYPES: [[u32; 4]; 6] = [
    [1410, 1411, 1412, 1300],
    [1714, 1715, 1716, 1600],
    [1112, 1113, 1115, 1000],
    [2614, 2615, 2616, 2500],
    [2013, 2014, 2015, 1900],
    [2313, 2314, 2315, 2200],
];

/// The four target GUIDs of a nation (three heroes, then the HQ); `None` outside 0 ... 5 (the original then leaves
/// the list unset).
pub fn hunt_target_types(nation: i32) -> Option<[u32; 4]> {
    usize::try_from(nation)
        .ok()
        .and_then(|n| HUNT_TARGET_TYPES.get(n))
        .copied()
}

/// Which menu the full build shows (`0x00457980`, N.5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuChoice {
    /// Empty selection: no buttons, key context 1.
    Empty,
    /// First object foreign: no buttons.
    Foreign,
    /// Unit or finished building alone: the set of the type GUID.
    Menu(i32),
    /// Construction site: only slot 13 of its menu (O-229).
    SiteOnly13(i32),
    /// Multi-selection: the set of the first object, the others add to it (O-228).
    Multi(i32),
}

impl MenuChoice {
    /// The menu of the choice (type GUID of the first object); `None` without buttons.
    pub fn menu(self) -> Option<i32> {
        match self {
            Self::Empty | Self::Foreign => None,
            Self::Menu(m) | Self::SiteOnly13(m) | Self::Multi(m) => Some(m),
        }
    }
}

/// The buttons of the bar from a set building. Heroes and headhunters do not belong here but to the bar window
/// ([`ObjectRow`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ButtonSet {
    /// The `toolbar.ini` lines of the 14 normal slots (`+0x20C`), including occupant and substitute lines.
    pub slots: [Option<ToolbarButton>; SLOT_COUNT],
    /// The buttons of the normal slots (`0x00453D70`); without an icon, an occupied slot stays empty.
    pub normal: [Option<Button>; SLOT_COUNT],
    /// Production row: running order, then the waiting ones.
    pub production: Vec<Button>,
    /// Multi-selection in selection order.
    pub selection: Vec<Button>,
    /// Stance.
    pub stance: Vec<Button>,
    /// Formation.
    pub formation: Vec<Button>,
    /// `+0x5CC`: the GUID of the shown menu; 0 after the addition of a multi-selection (D-234).
    pub shown_menu: i32,
    /// Filter lists of `GROUP_AND` (`Bar+0x5D0`, `FUN_00455330`): **the key is the type GUID** of the menu in whose set
    /// building the HID was added (O-272).
    pub groups: BTreeMap<u32, Vec<u32>>,
    /// The automatic occupant view (`FUN_00454310(0)` from `FUN_004548E0`) has been absorbed into this set; it sets
    /// the flag "full occupant view" of the bar (`+0x44`) to 0.
    pub occupants_auto: bool,
}

impl ButtonSet {
    /// Filter list of a button (`FUN_00454950`, called from `0x0045834C` with `ev[1]` = BUTTONMSG): the search uses
    /// the **BUTTONMSG**, the storage is under the object type ([`ButtonSet::groups`]). A match only arises if the
    /// command number happens to equal the type GUID of a set building; otherwise `None`, and the command goes to the
    /// whole selection (`FUN_0044CF50` with argument 1, O-272).
    pub fn group_for(&self, msg: i32) -> Option<&[u32]> {
        self.groups.get(&(msg as u32)).map(Vec::as_slice)
    }

    /// All buttons in child order of the original (`0x00457710`): normal slots, production, multi-selection, stance,
    /// formation.
    pub fn buttons(&self) -> impl Iterator<Item = &Button> {
        self.normal
            .iter()
            .flatten()
            .chain(&self.production)
            .chain(&self.selection)
            .chain(&self.stance)
            .chain(&self.formation)
    }

    /// Like [`ButtonSet::buttons`], mutable.
    pub fn buttons_mut(&mut self) -> impl Iterator<Item = &mut Button> {
        self.normal
            .iter_mut()
            .flatten()
            .chain(&mut self.production)
            .chain(&mut self.selection)
            .chain(&mut self.stance)
            .chain(&mut self.formation)
    }

    /// Replays the mouse movement to all buttons, as the button constructor does (`0x0044F15C...0x0044F184`). For
    /// buttons that arose without a mouse position ([`occupant_view`]).
    pub fn replay_mouse(&mut self, at: [i32; 2]) {
        replay_mouse(self.buttons_mut(), at);
    }
}

/// Mouse movement to `at` for every button (constructor replay `0x0044F15C`), for example for the buttons from
/// [`dynamic_buttons`].
pub fn replay_mouse<'a>(buttons: impl IntoIterator<Item = &'a mut Button>, at: [i32; 2]) {
    let mut sink = Vec::new();
    for b in buttons {
        b.mouse(HudEventKind::MouseMove, at, &mut sink);
    }
}

/// Menu choice of the full build (`0x00457980`, N.5.2): empty, foreign (owner != local), multi-selection,
/// construction site (module 3 without class `0x403`) or the set of the type GUID.
pub fn choose_menu(model: &ToolbarModel) -> MenuChoice {
    let Some(first) = model.selection.first() else {
        return MenuChoice::Empty;
    };
    if !first.own {
        return MenuChoice::Foreign;
    }
    let menu = first.guid as i32;
    if model.selection.len() > 1 {
        MenuChoice::Multi(menu)
    } else if first.site {
        MenuChoice::SiteOnly13(menu)
    } else {
        MenuChoice::Menu(menu)
    }
}

/// Set building: fill the normal slots from `menu` and create all buttons (`0x00457710`). The buttons replay `mouse`.
///
/// - `main = true`: **main set of the selection** like the full build `0x00457980` (steps 3-5); `menu` is the type
///   GUID of the first object from [`choose_menu`].
///   - First object own: construction site -> `BuildButtonSet(menu, 1, only13, 0)` (O-229, without a filter list);
///     otherwise `BuildButtonSet(menu, 1, 0, HID)` and then the automatic occupant view (`0x004548E0`) for a building
///     or for exactly one object.
///   - First object foreign and alone: no buttons (`0x00457B3A` returns before `0x00457710`).
///   - Every further object: `BuildButtonSet(type, 0, 0, HID)` - adds buttons without `GROUP_AND` on free slots and
///     sets `+0x5CC = 0` (O-228). As in the original, also when the first object is foreign.
/// - `main = false`: **submenu** (`FUN_00458130`: `BuildButtonSet(menu, 1, 0, 0)`): no addition, no filter list;
///   `shown_menu = menu`.
///
/// The rows are derived from the model. The original remembers in the full build which ones exist (`+0x56C`, list
/// `+0x554`, stance bits `+0x5AC`, formation `+0x18C`) and recreates them on every set building: production for a
/// single own building, multi-selection from two objects, stance for bits != 0, formation for module 0.
pub fn build_set(
    model: &ToolbarModel,
    menu: i32,
    main: bool,
    data: &ToolbarData,
    screen: Screen,
    mouse: Option<[i32; 2]>,
) -> ButtonSet {
    let mut set = ButtonSet::default();
    if main {
        let Some(first) = model.selection.first() else {
            return set;
        };
        let count = model.selection.len();
        if first.own {
            if first.site {
                fill(&mut set, model, menu, true, true, None);
            } else {
                fill(&mut set, model, menu, true, false, Some(first.hid));
                if first.building || count == 1 {
                    occupant_slots(&mut set, model, false);
                }
            }
        } else if count == 1 {
            return set;
        }
        for o in &model.selection[1..] {
            fill(&mut set, model, o.guid as i32, false, false, Some(o.hid));
        }
    } else {
        fill(&mut set, model, menu, true, false, None);
    }
    create(&mut set, model, data, screen, mouse);
    set
}

/// Silent rebuild `{0x10, 0}` (`0x00457DB0`, D-234): the shown menu `shown_menu` (`+0x5CC`) anew without an object
/// (`BuildButtonSet(+0x5CC, 1, 0, 0)`), for exactly one object with the automatic occupant view. An open submenu
/// stays open. `None` if the original does the full build instead: no selection, `shown_menu = 0` (multi-selection)
/// or the first object is a construction site.
pub fn rebuild_shown(
    model: &ToolbarModel,
    shown_menu: i32,
    data: &ToolbarData,
    screen: Screen,
    mouse: Option<[i32; 2]>,
) -> Option<ButtonSet> {
    let first = model.selection.first()?;
    if shown_menu == 0 || first.site {
        return None;
    }
    let mut set = ButtonSet::default();
    fill(&mut set, model, shown_menu, true, false, None);
    if model.selection.len() == 1 {
        occupant_slots(&mut set, model, false);
    }
    create(&mut set, model, data, screen, mouse);
    Some(set)
}

/// Only the production row anew (`{0x10, 3}`: `FUN_00453BF0`, `FUN_00454FD0`, `FUN_004573C0(1)`), with mouse replay;
/// conditions as in the set building (exactly one own building with production).
pub fn production_buttons(
    model: &ToolbarModel,
    data: &ToolbarData,
    screen: Screen,
    mouse: Option<[i32; 2]>,
) -> Vec<Button> {
    production_row(model, data, origin(screen), mouse)
}

/// Occupant view (O-266) on a built set; afterwards it recreates all buttons (`0x00457710`), without mouse replay
/// (use [`ButtonSet::replay_mouse`] for that).
///
/// - `full = true` (415 pressed, `FUN_00454310(1)`): clear all slots and filter lists, the occupants of the first
///   object in slot 0, 1, ..., then 416 in slot 12 and 417 in slot 13.
/// - `full = false` (`FUN_004548E0`): only if 415 is in a slot and at least 5 of the slots 0-6 are free; without
///   clearing, 415 becomes 416, the occupants go to the free slots 0...13 in ascending order, **no 417**.
///
/// Occupants are the response to `0x1817 {12}` to the first selected object (at most 12), line
/// `{slot, HID, 0xB000000}`.
pub fn occupant_view(
    set: &mut ButtonSet,
    model: &ToolbarModel,
    data: &ToolbarData,
    screen: Screen,
    full: bool,
) {
    if occupant_slots(set, model, full) {
        create(set, model, data, screen, None);
    }
}

/// Formation buttons (`FORMATIONBUTTONS`, 25 px, `resourcen_icons.tga`, scale 1.0, kind DO; N.5.5).
///
/// - collapsed with formation 0 ... 4 (`FUN_00457250(0)`): slot 0 = 510 with icon `9 + formation`;
/// - collapsed otherwise (`FUN_00457360`, single selection with -1, O-232): slot 0 = 519 with icon 14;
/// - expanded (`FUN_00457250(1)`): 509 with icon `9 + formation`, slots 1 ... 5 = 511 ... 515 (icons 9 ... 13),
///   slot 6 = 519 (icon 14).
///
/// If a position is missing, its button is omitted (the original aborts at slot 0 or 6).
pub fn formation_buttons(
    formation: i32,
    expanded: bool,
    data: &ToolbarData,
    screen: Screen,
    mouse: Option<[i32; 2]>,
) -> Vec<Button> {
    let o = origin(screen);
    let make = |slot: usize, msg: i32, icon: i32| {
        data.formation
            .get(slot)
            .map(|&pos| Button::new(resource_spec(o, pos, msg, kinds::DO, icon), mouse))
    };
    let mut out = Vec::new();
    if !expanded {
        if (0..5).contains(&formation) {
            out.extend(make(
                0,
                MSG_FORMATION_CURRENT,
                formation + FORMATION_ICON_BASE,
            ));
        } else {
            out.extend(make(0, MSG_SCATTER, SCATTER_ICON));
        }
    } else {
        out.extend(make(0, MSG_FORMATION_HIDE, formation + FORMATION_ICON_BASE));
        for i in 1..=5 {
            out.extend(make(
                i,
                MSG_FORMATION_CURRENT + i as i32,
                FORMATION_ICON_BASE - 1 + i as i32,
            ));
        }
        out.extend(make(6, MSG_SCATTER, SCATTER_ICON));
    }
    out
}

/// Level of the dynamic buttons (message `0x186A4`, after `0x186A3` "buttons gone").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicKind {
    /// Trade menu of kind 490 ... 493 (`FUN_0044B9A0`; the bar remembers the kind for the click).
    Trade(i32),
    /// Target player of headhunter kind 0 ... 2 (`FUN_0044A8C0`).
    TargetPlayer(u8),
    /// Target GUID for the kind and the target slot (`FUN_0044AA10`; the slot comes from the color, `FUN_00478DE0`).
    TargetGuid { art: u8, slot: u8 },
}

/// Dynamic buttons on the normal slots (N.14.7; 50 px, icon of the GUID like a normal button, without mouse replay):
///
/// - **Trade** (O-268): for each slot p = 0 ... **8** other than the local one with a trading building, a DO button
///   `520 + p`, consecutive from slot 0; 494 in slot 13.
/// - **Target player:** for each enemy (in slot order) with a non-locked `470 + color`, a button of kind `0xC000000`,
///   consecutive; 409 in slot 13.
/// - **Target GUID** (O-267): the three heroes of the target nation ([`HUNT_TARGET_TYPES`]) on **slot = index**, only
///   with an object counter (gaps stay); if there is none and a hunter of the kind exists, the HQ in slot 3 if the
///   target player has one; 409 in slot 13. Without a target player (`GameEngine_GetPlayer = 0`) nothing at all. For a
///   nation outside 0 ... 5 the original leaves the list unset; the reimplementation then shows only 409.
pub fn dynamic_buttons(
    kind: DynamicKind,
    model: &ToolbarModel,
    data: &ToolbarData,
    screen: Screen,
) -> [Option<Button>; SLOT_COUNT] {
    let mut out: [Option<Button>; SLOT_COUNT] = Default::default();
    let mut rows = Vec::new();
    match kind {
        DynamicKind::Trade(_) => {
            let mut n = 0;
            for p in 0..=8u8 {
                if p != model.local_slot && model.trade_partners.contains(&p) {
                    rows.push(row(n, MSG_TRADE_FIRST + i32::from(p), kinds::DO));
                    n += 1;
                }
            }
            rows.push(row(SITE_SLOT, MSG_TRADE_BACK, kinds::DO));
        }
        DynamicKind::TargetPlayer(_) => {
            let mut n = 0;
            for &(_, color) in &model.enemy_players {
                let msg = MSG_TARGET_PLAYER_FIRST + i32::from(color);
                if !model.is_disabled(msg) {
                    rows.push(row(n, msg, KIND_TARGET_PLAYER));
                    n += 1;
                }
            }
            rows.push(row(SITE_SLOT, MSG_BACK, kinds::DO));
        }
        DynamicKind::TargetGuid { art, slot } => {
            let Some(target) = model.hunt_targets.get(&slot) else {
                return out;
            };
            if let Some(types) = hunt_target_types(target.nation) {
                let mut shown = false;
                for (i, (&guid, &stock)) in types[..3].iter().zip(&target.stock).enumerate() {
                    if stock != 0 {
                        rows.push(row(i, guid as i32, KIND_TARGET_GUID));
                        shown = true;
                    }
                }
                let hunter = model
                    .hunters_present
                    .get(usize::from(art))
                    .copied()
                    .unwrap_or(false);
                if !shown && hunter && target.stock[3] != 0 {
                    rows.push(row(3, types[3] as i32, KIND_TARGET_GUID));
                }
            }
            rows.push(row(SITE_SLOT, MSG_BACK, kinds::DO));
        }
    }
    let o = origin(screen);
    for r in rows {
        let slot = r.id as usize;
        if slot < SLOT_COUNT {
            out[slot] = normal_button(slot, &r, model, data, o, None);
        }
    }
    out
}

/// A slot of the hero or headhunter row (16 bytes in the original: position, button, HID or GUID).
#[derive(Debug, Clone, PartialEq)]
pub struct RowSlot {
    /// Position from `toolbar.cfg`; a negative x becomes `W + x` for the first button and is **written back** (O-35).
    /// `None` for the hunter slots that the copy with the hero count does not reach (O-36).
    pub pos: Option<[i32; 2]>,
    /// HID (hero) or GUID (headhunter) of the slot.
    pub id: u32,
    /// The button; `None` = free.
    pub button: Option<Button>,
}

/// Hero or headhunter row of the bar window (`+0x8C`/`+0x98` or `+0x90`/`+0x94`). It only changes through the events
/// `{0x23, 1, id}` ([`ObjectRow::add`]), `0x24` ([`ObjectRow::remove`]) and `{0x25}` ([`ObjectRow::clear`]) or those
/// of the hunters; the bar window keeps it across the rebuilds of the set.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectRow {
    /// [`KIND_HERO`] or [`KIND_HEADHUNTER`].
    kind: u32,
    pub slots: Vec<RowSlot>,
}

impl ObjectRow {
    /// Empty hero row: as many slots as `HEROBUTTONS`.
    pub fn heroes(data: &ToolbarData) -> Self {
        Self {
            kind: KIND_HERO,
            slots: data
                .heroes
                .iter()
                .map(|&p| RowSlot {
                    pos: Some(p),
                    id: 0,
                    button: None,
                })
                .collect(),
        }
    }

    /// Empty headhunter row: as many slots as `HEADHUNTERBUTTONS` (`+0x94`), but the positions only for as many as
    /// there are hero slots (`0x0045696C` copies with `+0x98`, O-36; both 5 in the shipped data). The original leaves
    /// the remaining slots unset, the reimplementation without a button.
    pub fn headhunters(data: &ToolbarData) -> Self {
        Self {
            kind: KIND_HEADHUNTER,
            slots: (0..data.headhunters.len())
                .map(|i| RowSlot {
                    pos: (i < data.heroes.len()).then(|| data.headhunters[i]),
                    id: 0,
                    button: None,
                })
                .collect(),
        }
    }

    /// All buttons gone (`{0x25}` or `0x32`, `FUN_00453160`); the positions stay as they were written back.
    pub fn clear(&mut self) {
        for s in &mut self.slots {
            s.button = None;
        }
    }

    /// Remove the button with this ID (`FUN_00453060`); the slot becomes free.
    pub fn remove(&mut self, id: u32) {
        if let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.button.is_some() && s.id == id)
        {
            s.button = None;
        }
    }

    /// `{0x23, 1, id}` (`0x004550E0`/`0x00454B20`): if a button already has this ID, nothing (no duplicate). Otherwise
    /// create a button in the first free slot (`0x00454760`/`0x004549B0`); no free slot (sixth hero): nothing.
    ///
    /// - Hero: the object is looked up by HID ([`ToolbarModel::heroes`]), the icon comes from the type GUID;
    ///   `BUTTONMSG` and object are the HID, kind `0x14000000`.
    /// - Headhunter: `id` is the GUID (4100 ... 4102), at the same time icon and `BUTTONMSG`, kind `0x16000000`.
    ///
    /// Position: a negative x counts from the right edge of the screen and is written back (O-35); then relative to
    /// the bar. If object or icon is missing, the slot stays free. The icon does not clamp (`FUN_00453CC0` without
    /// clamping); an invalid sheet would abort, the reimplementation leaves the button out (deviation).
    pub fn add(
        &mut self,
        id: u32,
        model: &ToolbarModel,
        data: &ToolbarData,
        screen: Screen,
        mouse: Option<[i32; 2]>,
    ) {
        if self.slots.iter().any(|s| s.button.is_some() && s.id == id) {
            return;
        }
        let kind = self.kind;
        let Some(slot) = self.slots.iter_mut().find(|s| s.button.is_none()) else {
            return;
        };
        slot.id = id;
        slot.button = row_button(kind, slot, model, data, screen, mouse);
    }

    /// Like [`ObjectRow::clear`] and then [`ObjectRow::add`] for each ID of the list.
    pub fn rebuild(
        &mut self,
        ids: impl IntoIterator<Item = u32>,
        model: &ToolbarModel,
        data: &ToolbarData,
        screen: Screen,
        mouse: Option<[i32; 2]>,
    ) {
        self.clear();
        for id in ids {
            self.add(id, model, data, screen, mouse);
        }
    }

    /// The buttons in slot order.
    pub fn buttons(&self) -> impl Iterator<Item = &Button> {
        self.slots.iter().filter_map(|s| s.button.as_ref())
    }

    /// Like [`ObjectRow::buttons`], mutable.
    pub fn buttons_mut(&mut self) -> impl Iterator<Item = &mut Button> {
        self.slots.iter_mut().filter_map(|s| s.button.as_mut())
    }
}

/// New hero row from [`ToolbarModel::heroes`] (`{0x25}`, then `{0x23, 1, HID}` per hero).
pub fn hero_buttons(
    model: &ToolbarModel,
    data: &ToolbarData,
    screen: Screen,
    mouse: Option<[i32; 2]>,
) -> ObjectRow {
    let mut row = ObjectRow::heroes(data);
    row.rebuild(model.heroes.iter().map(|h| h.0), model, data, screen, mouse);
    row
}

/// New headhunter row from [`ToolbarModel::headhunters`].
pub fn headhunter_buttons(
    model: &ToolbarModel,
    data: &ToolbarData,
    screen: Screen,
    mouse: Option<[i32; 2]>,
) -> ObjectRow {
    let mut row = ObjectRow::headhunters(data);
    row.rebuild(
        model.headhunters.iter().copied(),
        model,
        data,
        screen,
        mouse,
    );
    row
}

/// First icon index of formation 0 (`9 + +0x18C`).
const FORMATION_ICON_BASE: i32 = 9;
/// Icon of scatter (519).
const SCATTER_ICON: i32 = 14;
/// Icon of stance slot i: tile `i + 4` of `resourcen_icons.tga`.
const STANCE_ICON_BASE: i32 = 4;
/// Base icon of stance and formation: tile 15 of `resourcen_icons.tga`.
const RESOURCE_BASE_TILE: i32 = 15;
/// Stance bit per slot 0 ... 4 (`0x00457866`): 420 -> mode 3, 421 -> 4, 422 -> 1, 423 -> 2, 424 -> 0.
const STANCE_BITS: [u32; 5] = [8, 0x10, 2, 4, 1];
/// Edge length of the large buttons.
const LARGE: i32 = 50;
/// Edge length of the small buttons (multi-selection, queue, stance, formation).
const SMALL: i32 = 25;
/// `0x1817 {12}` queries at most this many occupants.
const OCCUPANT_MAX: usize = 12;
/// Free slots among 0-6 from which the occupant view opens by itself (`> 4`, `0x004548E0`).
const AUTO_OCCUPANT_FREE: usize = 5;

/// Origin of the bar (`OffsetRect` with `+8`/`+0xC` of the window): `(0, H - 176)`.
fn origin(screen: Screen) -> [i32; 2] {
    let r = toolbar_rect(screen);
    [r[0], r[1]]
}

/// A synthetic `toolbar.ini` line (occupants, substitute buttons, dynamic buttons).
fn row(slot: usize, msg: i32, flags: u32) -> ToolbarButton {
    ToolbarButton {
        id: slot as i32,
        msg,
        flags,
        ..ToolbarButton::default()
    }
}

/// Hidden lines (`0x004557C0`, order as there): script lock or value set; once-only kinds with an order or object
/// counter; TARGET/TARGET_DO without a `targets.ini` entry; COUNTERUPGRADE with a `REQUIRED_GUID` with object counter 0
/// (all five slots, zeros skipped).
fn hidden(model: &ToolbarModel, row: &ToolbarButton) -> bool {
    if model.is_disabled(row.msg) {
        return true;
    }
    match row.kind() {
        kinds::ONLYONCE | kinds::UPGRADE | kinds::MAKEONLY_ONCE => {
            model.pending_of(row.msg) != 0 || model.stock_of(row.msg) != 0
        }
        kinds::TARGET | kinds::TARGET_DO => !model.targets.contains(&row.msg),
        kinds::COUNTERUPGRADE => row
            .required
            .iter()
            .any(|&g| g != 0 && model.stock_of(g) == 0),
        _ => false,
    }
}

/// `Toolbar_BuildButtonSet(menu, primary, only13, object)` (`0x004557C0`):
///
/// 1. `+0x5CC = primary ? menu : 0`.
/// 2. For each line of the menu in file order that is not [`hidden`]: first entry per slot (`BUTTONID`), with
///    `only13` only slot 13 (O-227, O-229). The reimplementation skips slots outside 0...13 (the original would
///    write past the end).
/// 3. For each slot 0...13 with the line `new` of the menu:
///    - free, `primary`: take over `new`; with `GROUP_AND` and an object, remember the HID;
///    - free, otherwise: take over only `new` without `GROUP_AND` and then remember the HID;
///    - occupied with `GROUP_AND` and `new` with the same BUTTONMSG: remember the HID.
///
/// Remembered under `menu` as the key (`FUN_00455330`, O-272), without a duplicate.
fn fill(
    set: &mut ButtonSet,
    model: &ToolbarModel,
    menu: i32,
    primary: bool,
    only13: bool,
    object: Option<u32>,
) {
    set.shown_menu = if primary { menu } else { 0 };
    let mut local: [Option<&ToolbarButton>; SLOT_COUNT] = [None; SLOT_COUNT];
    for r in model.menus.menus.get(&menu).into_iter().flatten() {
        if hidden(model, r) {
            continue;
        }
        let Some(slot) = usize::try_from(r.id).ok().filter(|&s| s < SLOT_COUNT) else {
            continue;
        };
        if (slot == SITE_SLOT || !only13) && local[slot].is_none() {
            local[slot] = Some(r);
        }
    }
    for (i, new) in local.into_iter().enumerate() {
        let remember = match &set.slots[i] {
            None if primary => {
                set.slots[i] = new.cloned();
                new.is_some_and(|n| n.flags & kinds::GROUP_AND != 0)
            }
            None => match new {
                Some(n) if n.flags & kinds::GROUP_AND == 0 => {
                    set.slots[i] = Some(n.clone());
                    true
                }
                _ => false,
            },
            Some(cur) => cur.flags & kinds::GROUP_AND != 0 && new.is_some_and(|n| n.msg == cur.msg),
        };
        if remember && let Some(hid) = object {
            let list = set.groups.entry(menu as u32).or_default();
            if !list.contains(&hid) {
                list.push(hid);
            }
        }
    }
}

/// Occupant slots according to [`occupant_view`]; `true` if the slots have changed.
fn occupant_slots(set: &mut ButtonSet, model: &ToolbarModel, full: bool) -> bool {
    if !full {
        let has_view = set.slots.iter().flatten().any(|r| r.msg == MSG_OCCUPANTS);
        let free = set.slots[..7].iter().filter(|s| s.is_none()).count();
        if !has_view || free < AUTO_OCCUPANT_FREE {
            return false;
        }
    }
    let Some(first) = model.selection.first() else {
        return false;
    };
    if full {
        // `FUN_00454120`: clear slots and filter lists.
        set.slots = Default::default();
        set.groups.clear();
    }
    let mut occupants = first.occupants.iter().take(OCCUPANT_MAX);
    for (i, slot) in set.slots.iter_mut().enumerate() {
        match slot {
            None => {
                if let Some(&(hid, _)) = occupants.next() {
                    *slot = Some(row(i, hid as i32, KIND_OCCUPANT));
                }
            }
            Some(r) if r.msg == MSG_OCCUPANTS => {
                *slot = Some(row(i, MSG_UNQUARTER_ALL, kinds::DO));
            }
            Some(_) => {}
        }
    }
    if full {
        set.slots[12] = Some(row(12, MSG_UNQUARTER_ALL, kinds::DO));
        set.slots[13] = Some(row(13, MSG_CANCEL_OCCUPANTS, kinds::DO));
    }
    set.occupants_auto = !full;
    true
}

/// `0x00457710`: create all buttons from the slots and the model.
fn create(
    set: &mut ButtonSet,
    model: &ToolbarModel,
    data: &ToolbarData,
    screen: Screen,
    mouse: Option<[i32; 2]>,
) {
    let o = origin(screen);
    set.normal = std::array::from_fn(|i| {
        set.slots[i]
            .as_ref()
            .and_then(|r| normal_button(i, r, model, data, o, mouse))
    });
    set.production = production_row(model, data, o, mouse);
    set.selection = if model.selection.len() > 1 {
        selection_row(model, data, o, mouse)
    } else {
        Vec::new()
    };
    set.stance = stance_row(model, data, o, mouse);
    // `0x004578D3`: with a selection only if the first object has module 0. The full build `0x00457980` sets `+0x18C`
    // to -1 and takes over the formation of the world (`FUN_00477950`) only for a multi-selection (O-232); until the
    // next full build this also applies to submenus.
    let formation = if model.selection.len() > 1 {
        model.formation
    } else {
        -1
    };
    set.formation = if model.selection.first().is_none_or(|f| f.has_unit_module) {
        formation_buttons(formation, false, data, screen, mouse)
    } else {
        Vec::new()
    };
}

/// Check the icon sheet: `None` if the sheet is not among the `ICONS` sheets (the original aborts).
fn checked(data: &ToolbarData, icon: Option<(usize, i32)>) -> Option<(usize, i32)> {
    icon.filter(|&(sheet, _)| data.icon_sheet(sheet).is_some())
}

/// Button from `icons.ini` sheets: base icon sheet 0 tile 0, icon `icon`.
#[allow(clippy::too_many_arguments)]
fn icon_spec(
    rect: [i32; 4],
    msg: i32,
    flags: u32,
    required: [i32; 5],
    icon: (usize, i32),
    scale: f32,
    progress: f32,
    object: Option<u32>,
) -> ButtonSpec {
    ButtonSpec {
        msg,
        flags,
        required,
        rect,
        base: (SheetRef::Icons(0), 0),
        icon: (SheetRef::Icons(icon.0), icon.1),
        scale,
        progress,
        object,
    }
}

/// Normal slot (`0x00453D70`, 50 px at `NORMALBUTTONS[slot]`). Occupants (`0xB000000`): `BUTTONMSG` is the HID, the
/// icon is that of the type GUID without clamping (the reimplementation is strict like [`ToolbarData::icon_of`]);
/// otherwise the icon of the BUTTONMSG with negative values clamped to 0. Without an icon or position no button.
fn normal_button(
    slot: usize,
    r: &ToolbarButton,
    model: &ToolbarModel,
    data: &ToolbarData,
    o: [i32; 2],
    mouse: Option<[i32; 2]>,
) -> Option<Button> {
    let [x, y] = *data.normal.get(slot)?;
    let (icon, object) = if r.kind() == KIND_OCCUPANT {
        let hid = r.msg as u32;
        let first = model.selection.first()?;
        let &(_, guid) = first.occupants.iter().find(|(h, _)| *h == hid)?;
        (data.icon_of(guid as i32), Some(hid))
    } else {
        (data.icon_clamped(r.msg), None)
    };
    let icon = checked(data, icon)?;
    let rect = [o[0] + x, o[1] + y, o[0] + x + LARGE, o[1] + y + LARGE];
    let spec = icon_spec(rect, r.msg, r.flags, r.required, icon, 1.0, 1.0, object);
    Some(Button::new(spec, mouse))
}

#[allow(clippy::too_many_arguments)]
/// Small button of the multi-selection or queue (`0x00453F20`): 25 px, scale 0.5, kind bits `kind | index`, strict
/// icon of the GUID (gap for a missing or negative icon, O-263).
fn small_button(
    pos: [i32; 2],
    flags: u32,
    msg: i32,
    icon_guid: u32,
    object: Option<u32>,
    data: &ToolbarData,
    o: [i32; 2],
    mouse: Option<[i32; 2]>,
) -> Option<Button> {
    let icon = checked(data, data.icon_of(icon_guid as i32))?;
    let [x, y] = pos;
    let rect = [o[0] + x, o[1] + y, o[0] + x + SMALL, o[1] + y + SMALL];
    let spec = icon_spec(rect, msg, flags, [0; 5], icon, 0.5, 1.0, object);
    Some(Button::new(spec, mouse))
}

/// Production row (`0x004573C0`, O-37) for exactly one own building (`FUN_00454FD0` sets `+0x56C | 4` only then):
/// slot 0 = running order (`0x00454180`, 50 px, scale 1, progress), slots 1-3 empty, from slot 4 a 25 px button per
/// waiting order with index `slot - 3` in the kind bits, until the positions end (26 for 30 positions; the simulation
/// holds at most 26 waiting orders, the original would run into an endless loop for more, `0x00457456`).
fn production_row(
    model: &ToolbarModel,
    data: &ToolbarData,
    o: [i32; 2],
    mouse: Option<[i32; 2]>,
) -> Vec<Button> {
    let mut out = Vec::new();
    let [first] = model.selection.as_slice() else {
        return out;
    };
    let (true, true, Some(p), Some(&[x, y])) = (
        first.own,
        first.building,
        first.production.as_ref(),
        data.selection.first(),
    ) else {
        return out;
    };
    if let Some((guid, progress)) = p.running
        && let Some(icon) = checked(data, data.icon_of(guid as i32))
    {
        let rect = [o[0] + x, o[1] + y, o[0] + x + LARGE, o[1] + y + LARGE];
        let spec = icon_spec(
            rect,
            guid as i32,
            KIND_PRODUCTION,
            [0; 5],
            icon,
            1.0,
            progress,
            None,
        );
        out.push(Button::new(spec, mouse));
    }
    for (k, &guid) in (QUEUE_START..data.selection.len()).zip(&p.queue) {
        let flags = KIND_PRODUCTION | (k - 3) as u32;
        out.extend(small_button(
            data.selection[k],
            flags,
            guid as i32,
            guid,
            None,
            data,
            o,
            mouse,
        ));
    }
    out
}

/// Multi-selection (`0x00457710`, O-258): for each object in selection order on `SELECTIONBUTTONS[i]` (column by
/// column) a button with `BUTTONMSG` = HID, object = HID, kind `0x5000000 | i`, icon of the type GUID; a gap without an
/// icon; none from the 31st object.
fn selection_row(
    model: &ToolbarModel,
    data: &ToolbarData,
    o: [i32; 2],
    mouse: Option<[i32; 2]>,
) -> Vec<Button> {
    model
        .selection
        .iter()
        .zip(&data.selection)
        .enumerate()
        .filter_map(|(i, (obj, &pos))| {
            small_button(
                pos,
                KIND_SELECTION | i as u32,
                obj.hid as i32,
                obj.guid,
                Some(obj.hid),
                data,
                o,
                mouse,
            )
        })
        .collect()
}

/// Button from `resourcen_icons.tga` (stance `0x00454520`, formation `0x00454630`): 25 px, base icon tile 15, scale
/// 1.0 (O-257).
fn resource_spec(o: [i32; 2], pos: [i32; 2], msg: i32, flags: u32, icon: i32) -> ButtonSpec {
    let [x, y] = pos;
    ButtonSpec {
        msg,
        flags,
        required: [0; 5],
        rect: [o[0] + x, o[1] + y, o[0] + x + SMALL, o[1] + y + SMALL],
        base: (SheetRef::Resource, RESOURCE_BASE_TILE),
        icon: (SheetRef::Resource, icon),
        scale: 1.0,
        progress: 1.0,
        object: None,
    }
}

/// Stance (`0x00457710`, `0x00454520`): only for bits != 0 (union over the selection, `FUN_00452FF0`); for each
/// position `i` a button `420 + i`, icon `i + 4`; selected (`0xA000004`) when the bit is set (8, `0x10`, 2, 4, 1 for
/// 0 ... 4). With a mixed selection several are selected.
fn stance_row(
    model: &ToolbarModel,
    data: &ToolbarData,
    o: [i32; 2],
    mouse: Option<[i32; 2]>,
) -> Vec<Button> {
    let bits = model.selection.iter().fold(0, |a, s| a | s.stance_bits);
    if bits == 0 {
        return Vec::new();
    }
    data.stance
        .iter()
        .enumerate()
        .map(|(i, &pos)| {
            let selected = STANCE_BITS.get(i).is_some_and(|b| bits & b != 0);
            let flags = KIND_STANCE | if selected { STANCE_SELECTED } else { 0 };
            let msg = MSG_STANCE_FIRST + i as i32;
            let icon = STANCE_ICON_BASE + i as i32;
            Button::new(resource_spec(o, pos, msg, flags, icon), mouse)
        })
        .collect()
}

/// Button of a hero or hunter slot ([`ObjectRow::add`]).
fn row_button(
    kind: u32,
    slot: &mut RowSlot,
    model: &ToolbarModel,
    data: &ToolbarData,
    screen: Screen,
    mouse: Option<[i32; 2]>,
) -> Option<Button> {
    let (guid, object) = if kind == KIND_HERO {
        let &(_, guid) = model.heroes.iter().find(|h| h.0 == slot.id)?;
        (guid, Some(slot.id))
    } else {
        (slot.id, None)
    };
    let icon = data.icon_of(guid as i32)?;
    let pos = slot.pos.as_mut()?;
    if pos[0] < 0 {
        pos[0] += screen.width;
    }
    let icon = checked(data, Some(icon))?;
    let o = origin(screen);
    let [x, y] = *pos;
    let rect = [o[0] + x, o[1] + y, o[0] + x + LARGE, o[1] + y + LARGE];
    let spec = icon_spec(rect, slot.id as i32, kind, [0; 5], icon, 1.0, 1.0, object);
    Some(Button::new(spec, mouse))
}

#[cfg(test)]
mod tests;
