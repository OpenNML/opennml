//! Snapshot of the selection per frame: everything the command bar needs from the simulation for set building,
//! buttons and locks. The client builds it from `SimHost`, the selection and the object under the mouse; the bar does
//! not know `nml-sim`.
//!
//! The original queries these values directly from the player and the objects while building the set
//! (`Toolbar_BuildButtonSet` `0x004557C0`, full build `0x00457980`) and per frame in the button (`0x0044F250`). Here
//! they are data that the client fills before the frame. All collections are ordered (`BTreeMap`/`BTreeSet`, no
//! `HashMap` iteration).

use std::collections::{BTreeMap, BTreeSet};

use nml_formats::gifcon::{Toolbar, button_kind as kinds};

use super::button::{
    ButtonBars, ButtonLocks, ButtonSpec, KIND_HERO, KIND_PRODUCTION, KIND_SELECTION,
};
use super::cursor::CursorFrame;

/// Kinds whose prerequisites the bar checks (`Toolbar_IsButtonLocked` `0x0044AF60`, drawing `0x0044F2C9`): BUILD,
/// MAKE, UPGRADE, DO, TARGET, TARGET_DO, ONLYONCE, COUNTERUPGRADE, MAKEONLY_ONCE. The simulation keeps the same list
/// as `CHECKED_KINDS` (`nml_sim::command::validate`); `nml-ui` does not know `nml-sim` and keeps its own copy. The
/// corpus test of the locks in `nml-client` checks that both lists are equal.
pub const CHECKED_KINDS: [u32; 9] = [
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

/// Production of a selected building (`FUN_00454FD0`: message `0xF03` for the running order, `0x1003` for the
/// queue).
#[derive(Debug, Clone, PartialEq)]
pub struct Production {
    /// Running order: GUID and progress (`0xF03`, field `+0x574` of the bar); `None` without an order (the original
    /// then creates a button for GUID 0, which is omitted for lack of an icon).
    pub running: Option<(u32, f32)>,
    /// Waiting orders in list order (`0x1003`).
    pub queue: Vec<u32>,
}

/// A selected object.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedObject {
    /// HID (`+0x58`).
    pub hid: u32,
    /// Type GUID (`+0x54`) = menu in `toolbar.ini`.
    pub guid: u32,
    /// Belongs to the local player.
    pub own: bool,
    /// Belongs to nobody (owner slot 8): the full build then also plays the selection sound for a single selection
    /// (`0x00457A7C`).
    pub neutral: bool,
    /// Has module 3 (building).
    pub building: bool,
    /// Building under construction: module 3 without class `0x403` (`GameObject_MatchesClass(0x403)`).
    pub site: bool,
    /// Has module 0 (unit): only then are there formation buttons (`0x004578D3`).
    pub has_unit_module: bool,
    /// Production for buildings whose query `0xF03` answers; otherwise `None`.
    pub production: Option<Production>,
    /// Occupants in the response order of `0x1817 {12}`: HID and type GUID.
    pub occupants: Vec<(u32, u32)>,
    /// Stance bit `1 << mode` from `0x711` or, for rally point buildings (module `0x17` and class `0x217`, or button
    /// 447 in the menu), `0x3603` (`FUN_00452EB0`), 0 without a stance. The client chooses the source.
    pub stance_bits: u32,
}

/// Target of a player in the target GUID level of the headhunter (`FUN_0044AA10`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HuntTarget {
    /// Nation of the player (`Player+0x238`); outside 0...5 there are no target buttons.
    pub nation: i32,
    /// Object counters of the player (`Player_GetGuidCount`) for the four GUIDs from
    /// [`super::set::hunt_target_types`] of its nation: three heroes, then the headquarters.
    pub stock: [i32; 4],
}

/// Keys that the click paths of the bar read (`FUN_004143A0` with the names of the key layout). The client sets the
/// state before distributing the input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolbarKeys {
    /// `KD_ADD_TO_GROUP` (LSHIFT): right-click on a selection button deselects instead of selecting alone
    /// (`0x004583B6`).
    pub add_to_group: bool,
    /// `KD_SHIFT_BUTTON`: train x5 (`0x0044D049`).
    pub shift: bool,
    /// `KD_CONTROL_BUTTON`: train x5.
    pub control: bool,
    /// `KD_ALT_BUTTON`: train x5.
    pub alt: bool,
}

/// Snapshot of the selection and the player values for a frame.
#[derive(Debug, Clone)]
pub struct ToolbarModel<'a> {
    /// Selected objects in selection order (`GameWorld+0x370`).
    pub selection: Vec<SelectedObject>,
    /// `toolbar.ini` (`Rules.toolbar`); not the bar window `hud::toolbar::Toolbar`.
    pub menus: &'a Toolbar,
    /// `Player_IsGuidDisabled(local, guid)` (script lock `+0x21E0` or value set config 1 = 0) for all shown
    /// `BUTTONMSG` and `470 + color`.
    pub disabled: BTreeSet<u32>,
    /// Object counters of the local player (`Player_GetGuidCount`, with groups) for `REQUIRED_GUID`, the `BUTTONMSG`
    /// of the once-only kinds and the shown buttons. If a GUID is missing, 0 applies.
    pub stock: BTreeMap<u32, i32>,
    /// Orders of the local player (`Player_GetPendingGuidCount`) for the same GUIDs. If a GUID is missing, 0 applies.
    pub pending: BTreeMap<u32, i32>,
    /// GUIDs whose counter-upgrade is running somewhere (`0x00412620`).
    pub counter_busy: BTreeSet<u32>,
    /// The local slot itself has a running counter-upgrade (`0x00479690`).
    pub slot_counter_busy: bool,
    /// Bars per HID for selection and hero buttons (re/game-ui N.14.3 step 5).
    pub bars: BTreeMap<u32, ButtonBars>,
    /// Active target list `GameWorld+0x354`, in which `0x0047B0A0` searches: the `BUTTONMSG` of the active
    /// `targets.ini` entries whose MY filter a selected object satisfies (`GameWorld_RebuildTargetActions`
    /// `0x0047D6D0`, O-281), not `targets.ini` itself. The host sets it each frame
    /// (`toolbar_host::active_target_buttons`); `toolbar_model` leaves it empty.
    pub targets: BTreeSet<i32>,
    /// Heroes of the local slot in object table order (D-232): HID and type GUID.
    pub heroes: Vec<(u32, u32)>,
    /// Headhunter GUIDs (4100 ... 4102) of the hunter buttons.
    pub headhunters: Vec<u32>,
    /// Formation of the world (`GameWorld+0x20B8`). The bar turns it into `+0x18C`: this value for a multi-selection,
    /// otherwise -1 (`0x00457980`, O-232); the host therefore does not have to set single selections to -1 itself.
    pub formation: i32,
    /// Slots 0 ... 8 other than the local one with a trading building (filter `{2, 0, 0x8000003}`, `FUN_00477B30`).
    pub trade_partners: Vec<u8>,
    /// Players 0 ... 7 who are still playing and are enemies of the local one: slot and color, in slot order.
    pub enemy_players: Vec<(u8, u8)>,
    /// Targets of the target GUID level per slot (`FUN_0044AA10`).
    pub hunt_targets: BTreeMap<u8, HuntTarget>,
    /// Per headhunter kind 0 ... 2: a hunter of this kind exists (`GameWorld+0x4DC[kind]`, `FUN_00477100`).
    pub hunters_present: [bool; 3],
    /// Visible pause (`+0x24E5`): building has no effect (O-231).
    pub visible_pause: bool,
    /// Local slot.
    pub local_slot: u8,
    /// Key state for the click paths ([`ToolbarKeys`]).
    pub keys: ToolbarKeys,
    /// First own headquarters of the nation in object table order (`FUN_0048B910` searches for the type GUID 1300,
    /// 1600, 1000, 2500, 1900 or 2200 in `+0x234[slot]`); `None` without. The headhunter button only works with it.
    pub headquarters: Option<u32>,
    /// `BUTTONMSG` -> `MAUSICONID` from `targets.ini` (`TargetEntry.mouse_icon`) for the targeting mode. The pointer
    /// itself comes later; the cursor only remembers the number.
    pub target_icons: BTreeMap<i32, i32>,
    /// Type GUID of the tent that the first selected object carries (response to `0x423`, `FUN_0044ACA0`); `None`
    /// without a tent. Button 453 uses it to open the tent cursor.
    pub tent_guid: Option<u32>,
    /// State of the world queries for the open cursor, supplied by the client each frame (frame event 6).
    pub cursor: Option<CursorFrame>,
}

impl<'a> ToolbarModel<'a> {
    /// Empty snapshot over a `toolbar.ini`: no selection, no object counters, formation -1.
    pub fn new(menus: &'a Toolbar) -> Self {
        Self {
            selection: Vec::new(),
            menus,
            disabled: BTreeSet::new(),
            stock: BTreeMap::new(),
            pending: BTreeMap::new(),
            counter_busy: BTreeSet::new(),
            slot_counter_busy: false,
            bars: BTreeMap::new(),
            targets: BTreeSet::new(),
            heroes: Vec::new(),
            headhunters: Vec::new(),
            formation: -1,
            trade_partners: Vec::new(),
            enemy_players: Vec::new(),
            hunt_targets: BTreeMap::new(),
            hunters_present: [false; 3],
            visible_pause: false,
            local_slot: 0,
            keys: ToolbarKeys::default(),
            headquarters: None,
            target_icons: BTreeMap::new(),
            tent_guid: None,
            cursor: None,
        }
    }

    /// Object counter of a GUID (0 if it is missing).
    pub fn stock_of(&self, guid: i32) -> i32 {
        self.stock.get(&(guid as u32)).copied().unwrap_or(0)
    }

    /// Orders of a GUID (0 if it is missing).
    pub fn pending_of(&self, guid: i32) -> i32 {
        self.pending.get(&(guid as u32)).copied().unwrap_or(0)
    }

    /// `Player_IsGuidDisabled(local, guid)`.
    pub fn is_disabled(&self, guid: i32) -> bool {
        self.disabled.contains(&(guid as u32))
    }

    /// A `REQUIRED_GUID` (up to the first 0) has object counter 0.
    fn missing(&self, required: &[i32; 5]) -> bool {
        required
            .iter()
            .take_while(|g| **g != 0)
            .any(|&g| self.stock_of(g) == 0)
    }

    /// Locks of a button for this frame (`0x0044F250` step 1): `missing_required` only for the checked kinds
    /// ([`CHECKED_KINDS`]), `counter_running` only for COUNTERUPGRADE. This is the calculation of the simulation
    /// (`validate::missing_required_in` behind the kind check and `validate::counter_blocked_in`) on the object
    /// counters of the snapshot; together they yield `validate::button_locked_in` without its nation stub. The original
    /// checks `REQUIRED_GUID_NATION` only in `Toolbar_IsButtonLocked` (`0x0044AF60`, key shortcuts), not when drawing;
    /// the button does not carry the list (O-230, empty in the shipped data). Equality with the simulation for all
    /// buttons of a match: corpus test `toolbar_locks_match_the_sim_for_every_corpus_button` (`nml-client`).
    pub fn locks_for(&self, spec: &ButtonSpec) -> ButtonLocks {
        let kind = spec.kind();
        ButtonLocks {
            missing_required: CHECKED_KINDS.contains(&kind) && self.missing(&spec.required),
            counter_running: kind == kinds::COUNTERUPGRADE
                && (self.counter_busy.contains(&(spec.msg as u32)) || self.slot_counter_busy),
        }
    }

    /// Bars of a selection or hero button: the values of the HID from [`ToolbarModel::bars`]; no bars for other
    /// buttons.
    pub fn bars_for(&self, spec: &ButtonSpec) -> Option<&ButtonBars> {
        let kind = spec.kind();
        if kind != KIND_SELECTION && kind != KIND_HERO {
            return None;
        }
        self.bars.get(&spec.object?)
    }

    /// Progress of the running order for the button at slot 0 of the production row (event `0xF`, each frame);
    /// otherwise `None`. Waiting orders (index >= 1 in the lower kind bits) stay at 1.0.
    pub fn progress_for(&self, spec: &ButtonSpec) -> Option<f32> {
        if spec.kind() != KIND_PRODUCTION || spec.flags & 0xFF != 0 {
            return None;
        }
        let first = self.selection.first()?;
        let (guid, progress) = first.production.as_ref()?.running?;
        (guid as i32 == spec.msg).then_some(progress)
    }
}
