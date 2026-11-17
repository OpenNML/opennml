//! Unit bar (formation 511-515, scatter 519, dismount 405, disembark 406, dive 432, surface 433, train
//! forward/backward/stop 449-451) and formation dragging on right-click: model, texts, actions - without drawing
//! (`apps/nml-viewer/src/unit_bar_ui.rs` draws). Based on re/groups-formations 2.2 and 12.6; re/riding 2.5 (405/406),
//! re/swim-dive 7 (432/433); re/train-multipart 2.2 (449-451); arm 485/486
//! (re/arming-gates 2.A), abilities 543 ff. with command `0x36` kind 4
//! (re/temp-upgrades 15 no. 8) and counter-upgrades 3370-3377 (re/temp-upgrades 1.5, 3.5);
//! pack tent 452 and unpack tent 453 with the unpack cursor (re/tents 2.5, 3.4); open gate 430 and close gate 431
//! (re/arming-gates 2.C, 7); camouflage 434/435, dig in 437/438, healing 458/459/554/564 and self-healing 555
//! (re/masking 2.4, re/healing 2.4); evoke ghost 436, banish 439/547/550/553/558/562 and evoke animal 441-443
//! (re/ghosts-banish 2.5/2.6, re/evoke-aura 2.4); spells 480-482, 484, 541 and special attacks
//! 454-457, 542, 546, 548, 551, 556 with the targeting mode of the target buttons (re/spells 2.5,
//! re/special-attacks 2.4); robbing 540 (re/headhunter-robbing 2.4, 3.7).
//!
//! The buttons 485/486, the abilities and the counter-upgrades come from the menus of the `toolbar.ini`
//! of the selected own types including submenus ([`menu_buttons`], the same rule as the
//! command check); the counter-upgrades appear directly as buttons, without the submenu 601.
//! Locked buttons (`Toolbar_IsButtonLocked` `0x0044AF60`,
//! [`button_locked`]) are shown grayed out in the bar.
//!
//! The selected formation is viewer state (deviation from the original): in the original it lives in
//! `GameWorld+0x20B8` (setter `0x00477960`, call `0x00457930`); the simulation never reads it, only
//! the UI writes it into the walk command. The reimplementation therefore stores it neither in the
//! savegame nor in the checksum.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use glam::Vec3;
use nml_formats::gifcon::{ToolbarButton, button_kind};
use nml_formats::messages::{decode_text_file, parse_text_table};
use nml_sim::command::ability::{
    AbilityTarget, ROB_BUTTON, SCOUT_BUTTONS, TARGET_BUTTONS, WHIRLWIND_BUTTON, button_state,
    target_command,
};
use nml_sim::command::headhunter::BACK_BUTTON;
pub use nml_sim::command::upgrade::ABILITY_BUTTONS;
use nml_sim::command::validate::{button_locked, menu_buttons, object_is_dead};
use nml_sim::command::{CommandKind, TrainOp};
use nml_sim::fx::Fx;
use nml_sim::id::ObjectId;
use nml_sim::module::diving::INDEX as DIVING_MODULE;
use nml_sim::module::train::INDEX as TRAIN_MODULE;
use nml_sim::module::{arming, building, spell};
use nml_sim::object::{GameObject, status};
use nml_sim::rules::Rules;
use nml_sim::sim::Sim;

use nml_sim::module::ModuleKind;
use nml_sim::module::tent::{INDEX as TENT_MODULE, transport};

use crate::game::economy::PlacementPreview;
use crate::game::formation_order::FormationOrder;
use crate::game::sim_host::SimHost;
use crate::game::texts::{TEXTS_PATH, clean_text};

/// Button numbers of the bar (`toolbar.ini`; 516-518 without an entry).
pub const FORMATION_BUTTONS: [i32; 5] = [511, 512, 513, 514, 515];
pub const SCATTER_BUTTON: i32 = 519;
/// Dismount (`targets.ini`: status `ISMOUNTED`; command `0x0B`), disembark (status `ISMOUNTEDONBOAT`;
/// `0x0D`), dive and surface (module 24; `0x1B`, `0x1C`).
pub const DISMOUNT_BUTTON: i32 = 405;
pub const LEAVE_BOAT_BUTTON: i32 = 406;
pub const DIVE_BUTTON: i32 = 432;
pub const SURFACE_BUTTON: i32 = 433;
/// Train forward, backward, stop (`toolbar.ini`, menu of the locomotive 2613, each `TARGET_DO_MSG`; commands
/// `0x29`, `0x2A`, `0x2B`).
pub const TRAIN_FORWARD_BUTTON: i32 = 449;
pub const TRAIN_BACKWARD_BUTTON: i32 = 450;
pub const TRAIN_STOP_BUTTON: i32 = 451;
/// Arm and disarm (worker bars, `TARGET_DO_MSG`; commands `0x38`, `0x72`;
/// re/arming-gates 2.A): 485 with `REQUIRED_GUID` = arming upgrade of the nation.
pub const ARM_BUTTON: i32 = nml_sim::command::upgrade::ARM_BUTTON;
pub const DISARM_BUTTON: i32 = 486;
/// Pack tent (tents, `GROUP_AND_MSG TARGET_DO_MSG`; command `0x2D`) and unpack (travois
/// 1717; unpack cursor, command `0x2E`), re/tents 2.5.
pub const PACK_TENT_BUTTON: i32 = nml_sim::command::tent::PACK_BUTTON;
pub const UNPACK_TENT_BUTTON: i32 = nml_sim::command::tent::UNPACK_BUTTON;
/// Bar button "Open gate" (430, `targets.ini` active with `TORCLOSE`).
pub const OPEN_GATE_BUTTON: i32 = nml_sim::command::gate::OPEN_BUTTON;
/// Bar button "Close gate" (431, `targets.ini` active with `TOROPEN`).
pub const CLOSE_GATE_BUTTON: i32 = nml_sim::command::gate::CLOSE_BUTTON;
/// Camouflage (434, command `0x1D`) and uncamouflage (435, `0x1E`), dig in (437, `0x20`) and dig out
/// (438, `0x21`), start healing (458 medic, 554 pilgrim, `0x34`) and stop (459, 564,
/// `0x35`), self-healing (555, `0x60`); re/masking 2.4,
/// re/healing 2.4.
pub const MASK_BUTTON: i32 = nml_sim::command::ability::MASK_BUTTON;
pub const UNMASK_BUTTON: i32 = nml_sim::command::ability::UNMASK_BUTTON;
pub const DIG_IN_BUTTON: i32 = nml_sim::command::ability::DIG_IN_BUTTON;
pub const DIG_OUT_BUTTON: i32 = nml_sim::command::ability::DIG_OUT_BUTTON;
pub const START_HEAL_BUTTONS: [i32; 2] = nml_sim::command::ability::START_HEAL_BUTTONS;
pub const STOP_HEAL_BUTTONS: [i32; 2] = nml_sim::command::ability::STOP_HEAL_BUTTONS;
pub const SELF_HEAL_BUTTON: i32 = nml_sim::command::ability::SELF_HEAL_BUTTON;
/// The nine buttons of the camouflage, dig-in and healing abilities in bar order (ascending) with
/// the fallback label when `guids.deu` has no text ([`ability_states`]).
const STEALTH_HEAL_BUTTONS: [(i32, &str); 9] = [
    (MASK_BUTTON, "Camouflage"),
    (UNMASK_BUTTON, "Uncamouflage"),
    (DIG_IN_BUTTON, "Dig in"),
    (DIG_OUT_BUTTON, "Dig out"),
    (458, "Start healing"),
    (459, "Stop healing"),
    (554, "Start healing"),
    (SELF_HEAL_BUTTON, "Self-healing"),
    (564, "Stop healing"),
];
/// Evoke ghost (436, command `0x1F`), evoke animal (441 wolf, 442 bear, 443 cat; commands
/// `0x22`-`0x24`) and the banish buttons (command `0x25`).
pub const EVOKE_GHOST_BUTTON: i32 = nml_sim::command::ability::EVOKE_GHOST_BUTTON;
pub const EVOKE_ANIMAL_BUTTONS: [i32; 3] = nml_sim::command::ability::EVOKE_ANIMAL_BUTTONS;
pub const BANISH_BUTTONS: [i32; 6] = nml_sim::command::ability::BANISH_BUTTONS;
/// The ten evocation buttons in bar order (ascending) with the fallback label,
/// when `guids.deu` has no text ([`evocation_states`]).
const EVOCATION_BUTTONS: [(i32, &str); 10] = [
    (EVOKE_GHOST_BUTTON, "Evoke ghost"),
    (439, "Banish"),
    (441, "Evoke wolf"),
    (442, "Evoke bear"),
    (443, "Evoke wildcat"),
    (547, "Banish ghosts"),
    (550, "Banish ghosts"),
    (553, "Banish ghosts"),
    (558, "Banish ghosts"),
    (562, "Banish ghosts"),
];
/// Reincarnation (482, `DO_MSG`, command `0x36 {3, 0, 0, 482}` per object, re/spells 2.5).
pub const REINCARNATE_BUTTON: i32 = 482;
/// The 14 buttons of the spells and special attacks in bar order with the fallback label
/// when `guids.deu` has no text ([`spell_special_states`]; re/spells 2.5,
/// re/special-attacks 2.4): the spells 480-482, 484, 541, the whirlwind 542, then the demolition expert,
/// killer and smith buttons and the scouting of the trapper. 482 and 542 are `DO` buttons, the
/// others target buttons ([`TARGET_BUTTONS`]).
const SPELL_SPECIAL_BUTTONS: [(i32, &str); 14] = [
    (480, "Explosion spell"),
    (481, "Storm spell"),
    (REINCARNATE_BUTTON, "Reincarnation"),
    (484, "Scout"),
    (541, "Drain mana"),
    (WHIRLWIND_BUTTON, "Whirlwind"),
    (454, "Dynamite at building"),
    (455, "Dynamite at ship"),
    (456, "Kill unit"),
    (457, "Eliminate unit"),
    (546, "Snipe"),
    (548, "Eliminate unit"),
    (551, "Damage building"),
    (556, "Scout map"),
];
/// Rob (540, `GROUP_AND_MSG`, `DO_MSG`, command `0x4C` per object; re/headhunter-robbing 2.4, 3.7)
/// with the fallback label when `guids.deu` has no text ([`rob_states`]).
const ROB_BUTTONS: [(i32, &str); 1] = [(ROB_BUTTON, "Rob")];
/// Execute counter-upgrades (`COUNTERUPGRADE_MSG`, submenu 601; command `0x2F`).
pub const COUNTER_UPGRADE_BUTTONS: RangeInclusive<i32> = 3370..=3377;
/// Header line "Current formation: [FORMATION]".
pub const HEADER_TEXT: i32 = 510;
/// Drag length in world units that must be exceeded (`0x00453991`, `FCOM` with 16.0 from
/// `0x00619E2C`).
const MIN_DRAG_WORLD: f32 = 16.0;

/// What the selection yields for the bar ([`summarize`]): the number of own units with walk and whether at
/// least one of them is mounted, in a boat, able to dive or dived, plus the locomotive for the train buttons
/// and the buttons from the menus of the selected types ([`SelectionSummary::extra`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionSummary {
    /// Own units of the selection with a walk module (addressees of the bar).
    pub own_units: usize,
    /// At least one is mounted (`ISMOUNTED`, `0x2000`): button 405.
    pub mounted: bool,
    /// At least one is in a boat (`ISMOUNTEDONBOAT`, `0x8000`): button 406.
    pub in_boat: bool,
    /// At least one has module 24 and is not dived: button 432.
    pub can_dive: bool,
    /// At least one is dived (`DIVING`): button 433.
    pub diving: bool,
    /// The first own object of the selection with train module 33 whose type lists the train buttons in
    /// `toolbar.ini` (in the shipped data only the locomotive 2613), and whether it is moving (status bit `MOVE`).
    /// The bar sends the train commands with `len = 1`: a multi-selection only affects this one
    /// object (K-88, list head `0x0044B640`).
    pub train: Option<(ObjectId, bool)>,
    /// Arm/disarm, abilities, counter-upgrades and the tent buttons 452/453 in this order, already with the
    /// lock ([`menu_extras`]); the label is the fallback,
    /// [`buttons`] inserts the text.
    pub extra: Vec<BarButton>,
}

/// State of the bar: the selected formation 0 (none) ... 4 (pincer), the running unpack cursor
/// (button 453) and the targeting mode of a target button. All viewer state, not in the savegame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnitBar {
    pub formation: i32,
    pub tent_cursor: Option<TentCursor>,
    pub target_mode: Option<TargetMode>,
}

/// Targeting mode of a target button: button `button`
/// from [`TARGET_BUTTONS`] for the first selected own object `unit`; the next left click on
/// an object or the ground gives the command ([`click_target`]). In the original the TARGET
/// button sets the targeting mode, which ends in `Input_IssueTargetAction` (`0x0044D350`) with
/// `BUTTONMSG` = button (re/spells 2.5). No dedicated mouse cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetMode {
    pub unit: ObjectId,
    pub button: i32,
}

/// Unpack cursor (button 453, re/tents 3.4): the ordinary build cursor with the GUID of the
/// packed tent (hypothesis of the cursor GUID, re/tents 3.4) and its rotation
/// (`rotation[1]`). The rotation is stored as `f32` bits so that [`UnitBar`] stays `Eq`;
/// it cannot be changed, since it would not enter the command anyway. `unit`
/// is the travois that was first in the selection at the start. The cursor does not hang on the
/// "Economy" window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TentCursor {
    pub unit: ObjectId,
    pub tent_guid: u32,
    pub rotation_bits: u32,
}

impl TentCursor {
    /// Rotation of the tent around y (radians).
    pub fn rotation(&self) -> f32 {
        f32::from_bits(self.rotation_bits)
    }
}

impl UnitBar {
    /// Selected formation limited to 0...4 (`Input_IssueTargetAction` `0x0044D350`: outside -> 0).
    pub fn formation(&self) -> i32 {
        if (0..=4).contains(&self.formation) {
            self.formation
        } else {
            0
        }
    }
}

impl FormationOrder {
    /// Command of the viewer with the formation of the bar (`bar`) and the drag `drag`.
    pub fn new(bar: &UnitBar, drag: Option<(i32, i32)>) -> Self {
        Self {
            formation: bar.formation(),
            drag,
        }
    }
}

/// A button of the bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarButton {
    pub id: i32,
    pub label: String,
    /// The selected formation (only 511-515).
    pub active: bool,
    /// Clickable; locked buttons (`REQUIRED_GUID` with object counter 0, running counter-upgrade) are shown
    /// grayed out in the bar.
    pub enabled: bool,
}

/// What a click on the bar does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarAction {
    /// Select formation 0...4 (button 511 + n).
    Formation(i32),
    /// Scatter (button 519).
    Scatter,
    /// Dismount (button 405, command `0x0B`).
    Dismount,
    /// Disembark (button 406, command `0x0D`).
    LeaveBoat,
    /// Dive (button 432, command `0x1B`).
    Dive,
    /// Surface (button 433, command `0x1C`).
    Surface,
    /// Train forward (button 449, command `0x29`).
    TrainForward,
    /// Train backward (button 450, command `0x2A`).
    TrainBackward,
    /// Train stop (button 451, command `0x2B`).
    TrainStop,
    /// Arm (button 485, command `0x38`).
    Arm,
    /// Disarm (button 486, command `0x72`).
    Disarm,
    /// Ability (button from [`ABILITY_BUTTONS`], command `0x36` kind 4 per caster).
    Ability(i32),
    /// Execute counter-upgrade (button = GUID 3370-3377, command `0x2F`).
    CounterUpgrade(u32),
    /// Pack tent (button 452, command `0x2D`).
    PackTent,
    /// Unpack tent (button 453): starts the unpack cursor, the click gives `0x2E`.
    UnpackTent,
    /// Open gate (button 430, command `0x19`).
    OpenGate,
    /// Close gate (button 431, command `0x1A`).
    CloseGate,
    /// Camouflage (button 434, command `0x1D`).
    Mask,
    /// Uncamouflage (button 435, command `0x1E`).
    Unmask,
    /// Dig in (button 437, command `0x20`).
    DigIn,
    /// Dig out (button 438, command `0x21`).
    DigOut,
    /// Start healing (button 458 or 554, command `0x34`).
    StartHealing,
    /// Stop healing (button 459 or 564, command `0x35`).
    StopHealing,
    /// Self-healing (button 555, command `0x60` per object).
    SelfHeal,
    /// Evoke ghost (button 436, command `0x1F` for the first selected object).
    EvokeGhost,
    /// Evoke animal (kind 0 wolf 441, 1 bear 442, 2 cat 443; commands `0x22`-`0x24` for the first
    /// selected object).
    EvokeAnimal(u8),
    /// Banish (button from [`BANISH_BUTTONS`], command `0x25` for the first selected object).
    Banish(i32),
    /// Reincarnation (button 482, command `0x36` kind 3 per object with the button).
    Reincarnate,
    /// Whirlwind (button 542, command `0x4F` per object with the button).
    Whirlwind,
    /// Target button from [`TARGET_BUTTONS`]: starts the targeting mode ([`TargetMode`]).
    Target(i32),
    /// Rob (button 540, command `0x4C` per selected own object with a free button).
    Rob,
}

impl BarAction {
    /// Action for a button number of the bar.
    pub fn from_button(button: i32) -> Option<Self> {
        if ABILITY_BUTTONS.iter().any(|b| b.0 == button) {
            return Some(Self::Ability(button));
        }
        if COUNTER_UPGRADE_BUTTONS.contains(&button) {
            return Some(Self::CounterUpgrade(button as u32));
        }
        match button {
            511..=515 => Some(Self::Formation(button - 511)),
            SCATTER_BUTTON => Some(Self::Scatter),
            DISMOUNT_BUTTON => Some(Self::Dismount),
            LEAVE_BOAT_BUTTON => Some(Self::LeaveBoat),
            DIVE_BUTTON => Some(Self::Dive),
            SURFACE_BUTTON => Some(Self::Surface),
            TRAIN_FORWARD_BUTTON => Some(Self::TrainForward),
            TRAIN_BACKWARD_BUTTON => Some(Self::TrainBackward),
            TRAIN_STOP_BUTTON => Some(Self::TrainStop),
            ARM_BUTTON => Some(Self::Arm),
            DISARM_BUTTON => Some(Self::Disarm),
            PACK_TENT_BUTTON => Some(Self::PackTent),
            UNPACK_TENT_BUTTON => Some(Self::UnpackTent),
            OPEN_GATE_BUTTON => Some(Self::OpenGate),
            CLOSE_GATE_BUTTON => Some(Self::CloseGate),
            MASK_BUTTON => Some(Self::Mask),
            UNMASK_BUTTON => Some(Self::Unmask),
            DIG_IN_BUTTON => Some(Self::DigIn),
            DIG_OUT_BUTTON => Some(Self::DigOut),
            b if START_HEAL_BUTTONS.contains(&b) => Some(Self::StartHealing),
            b if STOP_HEAL_BUTTONS.contains(&b) => Some(Self::StopHealing),
            SELF_HEAL_BUTTON => Some(Self::SelfHeal),
            EVOKE_GHOST_BUTTON => Some(Self::EvokeGhost),
            b if EVOKE_ANIMAL_BUTTONS.contains(&b) => EVOKE_ANIMAL_BUTTONS
                .iter()
                .position(|&a| a == b)
                .map(|art| Self::EvokeAnimal(art as u8)),
            b if BANISH_BUTTONS.contains(&b) => Some(Self::Banish(b)),
            REINCARNATE_BUTTON => Some(Self::Reincarnate),
            ROB_BUTTON => Some(Self::Rob),
            WHIRLWIND_BUTTON => Some(Self::Whirlwind),
            b if TARGET_BUTTONS.contains(&b) => Some(Self::Target(b)),
            _ => None,
        }
    }
}

/// Reads the texts 509-519, 405, 406, 430-435, 437, 438, 449-453, 458, 459, 485, 486, 554, 555, 564,
/// the ability buttons ([`ABILITY_BUTTONS`]), the evocation buttons ([`EVOCATION_BUTTONS`]: 436,
/// 439, 441-443, 547, 550, 553, 558, 562), the spells and special attacks
/// ([`SPELL_SPECIAL_BUTTONS`]), robbing 540, "Back" 409 (headhunter menu,
/// [`back_label`]) and 3370-3377 from `guids.deu`, cleaned
/// ([`clean_text`]); if the file is missing, the table stays empty (warning).
pub fn load_texts(vfs: &nml_rda::Vfs) -> BTreeMap<i32, String> {
    match vfs.read(TEXTS_PATH) {
        Ok(bytes) => parse_text_table(&decode_text_file(&bytes))
            .into_iter()
            .filter(|(k, _)| {
                (509..=519).contains(k)
                    || COUNTER_UPGRADE_BUTTONS.contains(k)
                    || ABILITY_BUTTONS.iter().any(|b| b.0 == *k)
                    || [
                        DISMOUNT_BUTTON,
                        LEAVE_BOAT_BUTTON,
                        DIVE_BUTTON,
                        SURFACE_BUTTON,
                        TRAIN_FORWARD_BUTTON,
                        TRAIN_BACKWARD_BUTTON,
                        TRAIN_STOP_BUTTON,
                        PACK_TENT_BUTTON,
                        UNPACK_TENT_BUTTON,
                        OPEN_GATE_BUTTON,
                        CLOSE_GATE_BUTTON,
                        ARM_BUTTON,
                        DISARM_BUTTON,
                    ]
                    .contains(k)
                    || STEALTH_HEAL_BUTTONS.iter().any(|b| b.0 == *k)
                    || EVOCATION_BUTTONS.iter().any(|b| b.0 == *k)
                    || SPELL_SPECIAL_BUTTONS.iter().any(|b| b.0 == *k)
                    || ROB_BUTTONS.iter().any(|b| b.0 == *k)
                    || *k == BACK_BUTTON as i32
            })
            .map(|(k, v)| (k, clean_text(&v)))
            .collect(),
        Err(e) => {
            log::warn!("Unit bar: {TEXTS_PATH} not read: {e:#}");
            BTreeMap::new()
        }
    }
}

/// Own units of the selection that can walk (walk module): addressees of the bar.
pub fn own_units(host: &SimHost, selection: &[ObjectId]) -> Vec<ObjectId> {
    host.own_objects(selection)
        .into_iter()
        .filter(|&id| {
            host.state
                .object(id, false)
                .is_some_and(|o| o.has_module(0, false))
        })
        .collect()
}

/// Does the menu of the type list one of the train buttons 449-451 (`BUTTONMSG`)?
fn train_button_msg(buttons: &[ToolbarButton]) -> bool {
    buttons
        .iter()
        .any(|b| (TRAIN_FORWARD_BUTTON..=TRAIN_STOP_BUTTON).contains(&b.msg))
}

/// The first own object of the selection with train module 33 whose type lists the train buttons in `toolbar.ini`
/// (tender and wagon have none, re/train-multipart 2.2), and its driving state (`MOVE`).
fn selected_train(host: &SimHost, selection: &[ObjectId]) -> Option<(ObjectId, bool)> {
    host.own_objects(selection).into_iter().find_map(|id| {
        let o = host.state.object(id, false)?;
        let menu = host.rules.toolbar.menus.get(&i32::try_from(o.guid).ok()?)?;
        (o.has_module(TRAIN_MODULE, false) && train_button_msg(menu))
            .then_some((id, o.status & status::MOVE != 0))
    })
}

/// `ISARMED` on the arming module (module 39, also switched off; like [`arming::is_armed`]).
fn is_armed(o: &GameObject) -> bool {
    o.module(arming::INDEX)
        .is_some_and(|m| m.base.flags & arming::ISARMED != 0)
}

/// The button `id` is in the menu of the type `guid` including submenus.
fn in_menu(rules: &Rules, guid: u32, id: i32) -> bool {
    menu_buttons(rules, guid).iter().any(|b| b.msg == id)
}

/// Own selected objects with module 39 and the button 485 (`arm`, unarmed) or 486
/// (armed) in the menu of their type: addressees of 485/486.
fn arming_units(host: &SimHost, selection: &[ObjectId], arm: bool) -> Vec<ObjectId> {
    let id = if arm { ARM_BUTTON } else { DISARM_BUTTON };
    host.own_objects(selection)
        .into_iter()
        .filter(|&u| {
            host.state.object(u, false).is_some_and(|o| {
                o.module(arming::INDEX).is_some()
                    && is_armed(o) != arm
                    && in_menu(&host.rules, o.guid, id)
            })
        })
        .collect()
}

/// Own selected casters (module 38) with the ability button `button` in the menu of their type.
fn casters(host: &SimHost, selection: &[ObjectId], button: i32) -> Vec<ObjectId> {
    host.own_objects(selection)
        .into_iter()
        .filter(|&u| {
            host.state.object(u, false).is_some_and(|o| {
                o.module(spell::INDEX).is_some() && in_menu(&host.rules, o.guid, button)
            })
        })
        .collect()
}

/// The first own selected object whose menu (including submenus) has `COUNTERUPGRADE` buttons:
/// the source of the counter-upgrades.
fn counter_source(host: &SimHost, selection: &[ObjectId]) -> Option<ObjectId> {
    host.own_objects(selection).into_iter().find(|&u| {
        host.state.object(u, false).is_some_and(|o| {
            menu_buttons(&host.rules, o.guid)
                .iter()
                .any(|b| b.kind() == button_kind::COUNTERUPGRADE)
        })
    })
}

/// The first selected object if it belongs to the active player: the tent buttons follow the first
/// of the selection (re/tents 2.5; the bar sends `len = 1`).
fn own_leader<'h>(host: &'h SimHost, selection: &[ObjectId]) -> Option<&'h GameObject> {
    let o = host.state.object(*selection.first()?, false)?;
    (o.owner() == host.active_player).then_some(o)
}

/// Tent of button 452: the first selected object if it is own and selectable,
/// has module 42 and its type lists 452 in the menu (`targets.ini` `MY_MM MM_CHECKPROPERTY PROP_TENT`).
/// The storage camp 1603 lists 452 without module 42 and gets no button (W-262). A packed
/// tent is unselectable but stays selected until the fade-out ends: no button whose command
/// would be rejected anyway (W-247).
fn pack_tent_source(host: &SimHost, selection: &[ObjectId]) -> Option<ObjectId> {
    let o = own_leader(host, selection)?;
    (o.selectable
        && o.has_module(TENT_MODULE, false)
        && in_menu(&host.rules, o.guid, PACK_TENT_BUTTON))
    .then_some(o.id)
}

/// Travois and tent of button 453: the first selected object if it is own, has module 35 with
/// `TRANSPORTINGTENT` (`targets.ini` 453), its type lists 453 in the menu and the carried tent still
/// exists. Without a tent (destroyed, the flag stays, W-261) the cursor would have neither GUID nor rotation.
fn unpack_tent_source<'h>(
    host: &'h SimHost,
    selection: &[ObjectId],
) -> Option<(ObjectId, &'h GameObject)> {
    let o = own_leader(host, selection)?;
    let m = o.module(transport::INDEX)?;
    if m.base.flags & transport::TRANSPORTING_TENT == 0
        || !in_menu(&host.rules, o.guid, UNPACK_TENT_BUTTON)
    {
        return None;
    }
    let ModuleKind::TransportTent(t) = &m.kind else {
        return None;
    };
    Some((o.id, host.state.object(t.tent, false)?))
}

/// Gate button of the first selected object (re/arming-gates 2.C and 7): 430 with
/// `TORCLOSE`, 431 with `TOROPEN`, if the object is own, alive (`object_is_dead` like
/// [`nml_sim::command::gate::check_gate`]) and a building with `TOR` and the button is in the menu
/// of its type. A construction site does not carry `TOR` (`StartBuild` clears it until
/// completion) and gets no button; nor does a gate without `TOROPEN` and `TORCLOSE`.
/// Returns the gate and the button number. Only reads.
fn gate_source(host: &SimHost, selection: &[ObjectId]) -> Option<(ObjectId, i32)> {
    let o = own_leader(host, selection)?;
    gate_button(&host.rules, o).map(|button| (o.id, button))
}

/// Gate button of an object (430 with `TORCLOSE`, 431 with `TOROPEN`) by the rules of [`gate_source`], without the
/// ownership check: alive, building with `TOR`, button in the menu of the type.
fn gate_button(rules: &Rules, o: &GameObject) -> Option<i32> {
    let m = o.module(building::INDEX)?;
    if object_is_dead(o) || !m.base.has_flags(building::flags::TOR) {
        return None;
    }
    let button = if m.base.has_flags(building::flags::TORCLOSE) {
        OPEN_GATE_BUTTON
    } else if m.base.has_flags(building::flags::TOROPEN) {
        CLOSE_GATE_BUTTON
    } else {
        return None;
    };
    in_menu(rules, o.guid, button).then_some(button)
}

/// Fallback label of a counter-upgrade without a text.
pub fn counter_upgrade_fallback(guid: u32) -> String {
    format!("Counter-upgrade {guid}")
}

/// Add button `id` with the lock state `enabled`; one that is already present becomes free as soon as one of the
/// types has it free.
fn push_extra(out: &mut Vec<BarButton>, id: i32, label: String, enabled: bool) {
    match out.iter_mut().find(|b| b.id == id) {
        Some(b) => b.enabled |= enabled,
        None => out.push(BarButton {
            id,
            label,
            active: false,
            enabled,
        }),
    }
}

/// The buttons camouflage, dig in and healing for the own selected objects `own` of the slot
/// `player` (re/masking 2.4, re/healing 2.4, W-277). For each button from
/// [`STEALTH_HEAL_BUTTONS`], [`button_state`] asks every object: hidden (`None`) with a missing
/// menu entry or an unmet MY filter, otherwise clickable or gray (`REQUIRED_GUID`). The button
/// appears if at least one object shows it, and is gray if it is locked for all
/// ([`push_extra`]). The filters read the state (`MASKED`, `WAITING4KILLING`, `HEAL`): 434 stays visible
/// even during the camouflage animation, because `MASKED` is not set until the end of the animation
/// (W-277). Only reads.
fn ability_states(sim: &mut Sim, player: u8, own: &[ObjectId]) -> Vec<BarButton> {
    let mut out = Vec::new();
    for (button, fallback) in STEALTH_HEAL_BUTTONS {
        for &id in own {
            if let Some(free) = button_state(sim, player, id, button) {
                push_extra(&mut out, button, fallback.into(), free);
            }
        }
    }
    out
}

/// The buttons evoke ghost, banish and evoke animal for the first selected own object
/// `leader` of the slot `player` (re/ghosts-banish 2.5/2.6, re/evoke-aura 2.4). For each button
/// from [`EVOCATION_BUTTONS`], [`button_state`] asks this one object: hidden (`None`) with a
/// missing menu entry or an unmet MY filter (436 and 441-443: property and not
/// `SWIMMING`), otherwise clickable or gray (`REQUIRED_GUID`). The bar sends `len = 1`: the buttons
/// follow only the first selected object, like the tent buttons. Only reads.
fn evocation_states(sim: &mut Sim, player: u8, leader: Option<ObjectId>) -> Vec<BarButton> {
    let mut out = Vec::new();
    let Some(id) = leader else { return out };
    for (button, fallback) in EVOCATION_BUTTONS {
        if let Some(free) = button_state(sim, player, id, button) {
            push_extra(&mut out, button, fallback.into(), free);
        }
    }
    out
}

/// The buttons of the spells and special attacks for the first selected own object `leader` of the slot
/// `player` (re/spells 2.5, re/special-attacks 2.4). For each button from
/// [`SPELL_SPECIAL_BUTTONS`], [`button_state`] asks this one object: hidden (`None`) with a
/// missing menu entry or an unmet MY filter (457/546 not `SWIMMING`, 454 not
/// mounted), otherwise clickable or gray (`REQUIRED_GUID`). Only reads.
fn spell_special_states(sim: &mut Sim, player: u8, leader: Option<ObjectId>) -> Vec<BarButton> {
    let mut out = Vec::new();
    let Some(id) = leader else { return out };
    for (button, fallback) in SPELL_SPECIAL_BUTTONS {
        if let Some(free) = button_state(sim, player, id, button) {
            push_extra(&mut out, button, fallback.into(), free);
        }
    }
    out
}

/// The button Rob 540 for the own selected objects `own` of the slot `player` (re/headhunter-robbing 2.4):
/// like [`ability_states`] over **all** objects - the
/// button appears if [`button_state`] returns a `Some` for one (menu of the type; 540 has no
/// `targets.ini` entry), and is gray if it is locked for all (`REQUIRED_GUID 3326`).
/// Only reads.
fn rob_states(sim: &mut Sim, player: u8, own: &[ObjectId]) -> Vec<BarButton> {
    let mut out = Vec::new();
    for (button, fallback) in ROB_BUTTONS {
        for &id in own {
            if let Some(free) = button_state(sim, player, id, button) {
                push_extra(&mut out, button, fallback.into(), free);
            }
        }
    }
    out
}

/// Buttons from the menus of the selected own types, in this order:
/// - 485 Arm, if a unit with module 39 is unarmed and has 485 in its menu (free if a type has
///   it free: `REQUIRED_GUID` = arming upgrade); 486 Disarm for armed ones
///   (`ISARMED`, `targets.ini` 485/486);
/// - the abilities from [`ABILITY_BUTTONS`] in menu order that are in the menu of a type with
///   module 38;
/// - camouflage, dig in and healing (434, 435, 437, 438, 458, 459, 554, 555, 564) behind the
///   spell buttons ([`ability_states`]: appears if at least one own selected object has it
///   in its menu and the MY filter fits; gray if it is locked for all such objects);
/// - the `COUNTERUPGRADE` buttons of the first unit that has some, directly without submenu 601;
/// - evoke ghost, banish and evoke animal (436, 439, 441-443, 547, 550, 553, 558, 562)
///   behind the camouflage, dig-in and healing buttons, only for the first selected own object
///   ([`evocation_states`]);
/// - spells and special attacks (480-482, 484, 541, 542, 454-457, 546, 548, 551, 556) behind the
///   evocations, only for the first selected own object ([`spell_special_states`]). The target buttons
///   and the `DO` buttons of this group uniformly follow the first
///   object: the bar shows the menu of the first selected object; 543 ff. stay
///   as the union over all selected casters (**hypothesis**, re/spells 2.5). The click
///   on 482/542 nevertheless goes to every selected own object with the button ([`apply`], "each");
/// - Rob 540 behind the spells, over all selected own objects ([`rob_states`]);
/// - 452 Pack tent ([`pack_tent_source`]) and 453 Unpack tent ([`unpack_tent_source`]),
///   both only for the first selected object;
/// - 430 Open gate or 431 Close gate ([`gate_source`]), likewise for the first selected
///   object (the demolition button 529 does not enter the bar).
///
/// Locked per [`button_locked`] (including the run locks of the counter-upgrades). Only
/// reads; the simulation context is needed for the lock check.
pub fn menu_extras(host: &mut SimHost, selection: &[ObjectId]) -> Vec<BarButton> {
    let own = host.own_objects(selection);
    let source = counter_source(host, selection);
    let pack = pack_tent_source(host, selection);
    let unpack = unpack_tent_source(host, selection).map(|(unit, _)| unit);
    let gate = gate_source(host, selection);
    // The ID of the first object before the borrow of `host`.
    let leader = own_leader(host, selection).map(|o| o.id);
    let player = host.active_player;
    let slot = i32::from(player);
    let mut sim = host.sim_context();
    let rules = sim.rules;
    let mut arming_buttons = Vec::new();
    let mut abilities = Vec::new();
    let mut counters = Vec::new();
    for &id in &own {
        let Some(o) = sim.object(id) else { continue };
        let menu = menu_buttons(rules, o.guid);
        if o.module(arming::INDEX).is_some() {
            let (wanted, fallback) = if is_armed(o) {
                (DISARM_BUTTON, "Disarm")
            } else {
                (ARM_BUTTON, "Arm")
            };
            if let Some(b) = menu.iter().find(|b| b.msg == wanted) {
                let free = !button_locked(&sim, slot, b);
                push_extra(&mut arming_buttons, wanted, fallback.into(), free);
            }
        }
        if o.module(spell::INDEX).is_some() {
            for b in menu
                .iter()
                .filter(|b| ABILITY_BUTTONS.iter().any(|a| a.0 == b.msg))
            {
                let free = !button_locked(&sim, slot, b);
                push_extra(&mut abilities, b.msg, format!("Ability {}", b.msg), free);
            }
        }
        if source == Some(id) {
            for b in menu
                .iter()
                .filter(|b| b.kind() == button_kind::COUNTERUPGRADE)
            {
                let free = !button_locked(&sim, slot, b);
                let label = counter_upgrade_fallback(b.msg as u32);
                push_extra(&mut counters, b.msg, label, free);
            }
        }
    }
    let mut tents = Vec::new();
    for (source, wanted, fallback) in [
        (pack, PACK_TENT_BUTTON, "Pack tent"),
        (unpack, UNPACK_TENT_BUTTON, "Unpack tent"),
    ] {
        let Some(o) = source.and_then(|id| sim.object(id)) else {
            continue;
        };
        if let Some(b) = menu_buttons(rules, o.guid).iter().find(|b| b.msg == wanted) {
            let free = !button_locked(&sim, slot, b);
            push_extra(&mut tents, wanted, fallback.into(), free);
        }
    }
    let mut gates = Vec::new();
    if let Some((id, wanted)) = gate
        && let Some(o) = sim.object(id)
        && let Some(b) = menu_buttons(rules, o.guid).iter().find(|b| b.msg == wanted)
    {
        let fallback = if wanted == OPEN_GATE_BUTTON {
            "Open gate"
        } else {
            "Close gate"
        };
        let free = !button_locked(&sim, slot, b);
        push_extra(&mut gates, wanted, fallback.into(), free);
    }
    let stealth_heal = ability_states(&mut sim, player, &own);
    let evocation = evocation_states(&mut sim, player, leader);
    let spells = spell_special_states(&mut sim, player, leader);
    let rob = rob_states(&mut sim, player, &own);
    arming_buttons.sort_by_key(|b| b.id);
    arming_buttons.extend(abilities);
    arming_buttons.extend(stealth_heal);
    arming_buttons.extend(evocation);
    arming_buttons.extend(spells);
    arming_buttons.extend(rob);
    arming_buttons.extend(counters);
    arming_buttons.extend(tents);
    arming_buttons.extend(gates);
    arming_buttons
}

/// Selection for the bar: own units with walk like [`own_units`]; the states (mounted, in a
/// boat, able to dive, dived) apply if they hold for at least one of them. Plus the locomotive
/// of the selection ([`selected_train`]) and the buttons from the menus ([`menu_extras`]). Only reads
/// (the checksum stays the same).
pub fn summarize(host: &mut SimHost, selection: &[ObjectId]) -> SelectionSummary {
    let units = own_units(host, selection);
    let mut out = SelectionSummary {
        own_units: units.len(),
        train: selected_train(host, selection),
        extra: menu_extras(host, selection),
        ..SelectionSummary::default()
    };
    for o in units.iter().filter_map(|&id| host.state.object(id, false)) {
        let diving = o.status & status::DIVING != 0;
        out.mounted |= o.status & status::ISMOUNTED != 0;
        out.in_boat |= o.status & status::ISMOUNTEDONBOAT != 0;
        out.diving |= diving;
        out.can_dive |= !diving && o.has_module(DIVING_MODULE, false);
    }
    out
}

/// Label of a formation button (fallback with the number if the text is missing).
fn formation_label(texts: &BTreeMap<i32, String>, id: i32) -> String {
    text_or(texts, id, &format!("Formation {id}"))
}

/// Label of a button with the text `id` from the table; if it is missing, `fallback` is used.
fn text_or(texts: &BTreeMap<i32, String>, id: i32, fallback: &str) -> String {
    texts
        .get(&id)
        .filter(|t| !t.is_empty())
        .cloned()
        .unwrap_or_else(|| fallback.into())
}

/// Buttons of the bar: without own units (with walk) and without a locomotive none; with own units
/// 511-515 and 519, plus depending on the state of the selection 405 Dismount (mounted), 406 Disembark (in a boat),
/// 432 Dive (able to dive) and 433 Surface (dived); with a locomotive in the selection 449
/// Forward and 450 Backward when standing, 451 Stop when moving (`targets.ini`: 449/450 only without, 451
/// only with `MOVE`). Then the buttons from the menus ([`SelectionSummary::extra`]).
pub fn buttons(
    bar: &UnitBar,
    texts: &BTreeMap<i32, String>,
    selection: &SelectionSummary,
) -> Vec<BarButton> {
    let units = selection.own_units > 0;
    let mut out: Vec<BarButton> = if units {
        FORMATION_BUTTONS
            .iter()
            .map(|&id| BarButton {
                id,
                label: formation_label(texts, id),
                active: id - 511 == bar.formation(),
                enabled: true,
            })
            .collect()
    } else {
        Vec::new()
    };
    let at_rest = selection.train.is_some_and(|(_, moving)| !moving);
    let moving = selection.train.is_some_and(|(_, moving)| moving);
    for (id, shown, fallback) in [
        (SCATTER_BUTTON, units, "Scatter"),
        (DISMOUNT_BUTTON, selection.mounted, "Dismount"),
        (LEAVE_BOAT_BUTTON, selection.in_boat, "Disembark"),
        (DIVE_BUTTON, selection.can_dive, "Dive"),
        (SURFACE_BUTTON, selection.diving, "Surface"),
        (TRAIN_FORWARD_BUTTON, at_rest, "Forward"),
        (TRAIN_BACKWARD_BUTTON, at_rest, "Backward"),
        (TRAIN_STOP_BUTTON, moving, "Stop"),
    ] {
        if shown {
            out.push(BarButton {
                id,
                label: text_or(texts, id, fallback),
                active: false,
                enabled: true,
            });
        }
    }
    // 485/486, abilities and counter-upgrades: text from `guids.deu`, otherwise the fallback.
    out.extend(selection.extra.iter().map(|b| BarButton {
        label: text_or(texts, b.id, &b.label),
        ..b.clone()
    }));
    out
}

/// Label of the button "Back" (409) in the headhunter menu (re/headhunter-robbing 2.5): text from
/// `guids.deu`, otherwise "Back".
pub fn back_label(texts: &BTreeMap<i32, String>) -> String {
    text_or(texts, BACK_BUTTON as i32, "Back")
}

/// Header line: text 510 with `[FORMATION]` -> name of the selected formation.
pub fn header(bar: &UnitBar, texts: &BTreeMap<i32, String>) -> String {
    let name = formation_label(texts, 511 + bar.formation());
    texts
        .get(&HEADER_TEXT)
        .filter(|t| !t.is_empty())
        .map_or_else(
            || format!("Current formation: {name}"),
            |t| t.replace("[FORMATION]", &name),
        )
}

/// Header line of the window for the selection: the formation line ([`header`]) only with own
/// units; a locomotive alone shows only its train buttons, without the formation header.
pub fn header_for(
    bar: &UnitBar,
    texts: &BTreeMap<i32, String>,
    selection: &SelectionSummary,
) -> String {
    if selection.own_units == 0 {
        return String::new();
    }
    header(bar, texts)
}

/// Click on the bar: `Formation(f)` remembers the formation and, with more than one own
/// unit in the selection, gives `SetFormation` (bar 511-515 = `Toolbar_SubmitFormation` `0x0044ADE0`);
/// `Scatter` gives `Scatter` (`0x0044BC30`). Dismount, disembark, dive and surface give
/// `Dismount`, `LeaveBoat`, `Dive` and `Surface` with the own units of the selection; whom the
/// command affects is decided by the simulation (re/riding 2.5, re/swim-dive 2.3). The train buttons
/// 449-451 give `Train` to the first locomotive of the selection.
///
/// 485/486 give `ToggleArming` with the own units with module 39 that are unarmed or
/// armed and have the button in their menu (`GameWorld_CmdToggleArming` `0x00475C10`); an
/// ability gives a `CastSpell` with kind 4 and the fixed `(anim, idx)` of the button for each own caster
/// with the button in the menu (`Toolbar_DispatchButtonCommand` `0x0044C040`, one command per
/// object); a counter-upgrade gives `CounterUpgrade` with the first unit that has counter-upgrades in its
/// menu. Without a matching unit no command; the simulation checks locks itself.
///
/// 452 gives `PackTent` only for the first selected object ([`pack_tent_source`], `len = 1`,
/// re/tents 2.4); 453 starts the unpack cursor ([`TentCursor`]) with GUID and rotation of the
/// carried tent, the command is not given until the click ([`click_unpack`]). 430/431 give for each own
/// living gate of the selection in the matching state an `OpenGate` or `CloseGate` ([`gate_button`],
/// per object like `FUN_0044B3F0`, K-88); the simulation checks again (`check_gate`).
///
/// 482 gives for each selected own object that shows the button ([`button_state`]) a `CastSpell`
/// with kind 3 and `(0, 0)`, 542 a `Whirlwind` for each such object (`Toolbar_DispatchButtonCommand`
/// `0x0044C040`, "each", re/spells 2.5). A target button starts the targeting mode ([`TargetMode`]) for the
/// first selected own object if the button is clickable there; the targeting mode ends the
/// unpack cursor (mutually exclusive), 453 ends the targeting mode. The command is not given until the
/// click ([`click_target`]).
///
/// 540 gives for each selected own object for which the button is free ([`button_state`] = `Some(true)`)
/// a `RobCorpses` (`GROUP_AND_MSG`, `DO_MSG`, re/headhunter-robbing 2.4/3.7).
pub fn apply(bar: &mut UnitBar, host: &mut SimHost, selection: &[ObjectId], action: BarAction) {
    let units = own_units(host, selection);
    match action {
        BarAction::Formation(f) => {
            bar.formation = if (0..=4).contains(&f) { f } else { 0 };
            if units.len() > 1 {
                host.submit_own(CommandKind::SetFormation {
                    units,
                    formation: bar.formation,
                });
            }
        }
        BarAction::Scatter => {
            if !units.is_empty() {
                host.submit_own(CommandKind::Scatter { units });
            }
        }
        BarAction::Dismount => submit_units(host, units, |units| CommandKind::Dismount { units }),
        BarAction::LeaveBoat => submit_units(host, units, |units| CommandKind::LeaveBoat { units }),
        BarAction::Dive => submit_units(host, units, |units| CommandKind::Dive { units }),
        BarAction::Surface => submit_units(host, units, |units| CommandKind::Surface { units }),
        BarAction::TrainForward => submit_train(host, selection, TrainOp::Forward),
        BarAction::TrainBackward => submit_train(host, selection, TrainOp::Backward),
        BarAction::TrainStop => submit_train(host, selection, TrainOp::Stop),
        BarAction::Arm | BarAction::Disarm => {
            let arm = action == BarAction::Arm;
            let units = arming_units(host, selection, arm);
            submit_units(host, units, |units| CommandKind::ToggleArming {
                units,
                arm,
            });
        }
        BarAction::Ability(button) => {
            let Some(&(_, anim, idx)) = ABILITY_BUTTONS.iter().find(|b| b.0 == button) else {
                return;
            };
            for caster in casters(host, selection, button) {
                host.submit_own(CommandKind::CastSpell {
                    caster,
                    art: spell::ART_EXEC_TEMP,
                    anim,
                    idx,
                    button,
                });
            }
        }
        BarAction::CounterUpgrade(guid) => {
            if let Some(source) = counter_source(host, selection) {
                host.submit_own(CommandKind::CounterUpgrade { guid, source });
            }
        }
        BarAction::PackTent => {
            if let Some(tent) = pack_tent_source(host, selection) {
                host.submit_own(CommandKind::PackTent { tent });
            }
        }
        BarAction::OpenGate | BarAction::CloseGate => {
            // One command per object (`FUN_0044B3F0`/`FUN_0044B440` over the list `+0x2C`, K-88): every own gate
            // of the selection in the matching state ([`gate_button`]); the simulation checks again (`check_gate`).
            let wanted = if action == BarAction::OpenGate {
                OPEN_GATE_BUTTON
            } else {
                CLOSE_GATE_BUTTON
            };
            let gates: Vec<ObjectId> = host
                .own_objects(selection)
                .into_iter()
                .filter(|&id| {
                    host.state
                        .object(id, false)
                        .and_then(|o| gate_button(&host.rules, o))
                        == Some(wanted)
                })
                .collect();
            for gate in gates {
                host.submit_own(if action == BarAction::OpenGate {
                    CommandKind::OpenGate { gate }
                } else {
                    CommandKind::CloseGate { gate }
                });
            }
        }
        BarAction::Mask => submit_own_objects(host, selection, |units| CommandKind::Mask { units }),
        BarAction::Unmask => {
            submit_own_objects(host, selection, |units| CommandKind::Unmask { units });
        }
        BarAction::DigIn => {
            submit_own_objects(host, selection, |units| CommandKind::DigIn { units })
        }
        BarAction::DigOut => {
            submit_own_objects(host, selection, |units| CommandKind::DigOut { units });
        }
        BarAction::StartHealing => {
            submit_own_objects(host, selection, |units| CommandKind::StartHealing { units });
        }
        BarAction::StopHealing => {
            submit_own_objects(host, selection, |units| CommandKind::StopHealing { units });
        }
        BarAction::SelfHeal => {
            // One command per object with the button 555 in the menu (`Toolbar_DispatchButtonCommand`
            // `0x0044C040`, re/healing 2.4); the simulation deducts the costs.
            let player = host.active_player;
            let own = host.own_objects(selection);
            let mut sim = host.sim_context();
            let carriers: Vec<ObjectId> = own
                .into_iter()
                .filter(|&u| button_state(&mut sim, player, u, SELF_HEAL_BUTTON).is_some())
                .collect();
            for unit in carriers {
                host.submit_own(CommandKind::SelfHeal {
                    unit,
                    button: SELF_HEAL_BUTTON,
                });
            }
        }
        BarAction::EvokeGhost | BarAction::EvokeAnimal(_) | BarAction::Banish(_) => {
            // One command for the first selected own object (`len = 1`), only if it shows the button
            // (menu, MY filter, re/ghosts-banish 2.5, re/evoke-aura 2.4); the simulation checks
            // again (`check_ability`) and deducts the costs.
            let player = host.active_player;
            let Some(unit) = own_leader(host, selection).map(|o| o.id) else {
                return;
            };
            let (button, kind) = match action {
                BarAction::EvokeGhost => (
                    EVOKE_GHOST_BUTTON,
                    CommandKind::EvokeGhost {
                        unit,
                        button: EVOKE_GHOST_BUTTON,
                    },
                ),
                BarAction::EvokeAnimal(art) => {
                    let Some(&button) = EVOKE_ANIMAL_BUTTONS.get(usize::from(art)) else {
                        return;
                    };
                    (button, CommandKind::EvokeAnimal { unit, art, button })
                }
                BarAction::Banish(button) => (button, CommandKind::Banish { unit, button }),
                _ => return,
            };
            let mut sim = host.sim_context();
            if button_state(&mut sim, player, unit, button).is_some() {
                host.submit_own(kind);
            }
        }
        BarAction::UnpackTent => {
            if let Some((unit, tent)) = unpack_tent_source(host, selection) {
                bar.tent_cursor = Some(TentCursor {
                    unit,
                    tent_guid: tent.guid,
                    rotation_bits: tent.rotation[1].to_bits(),
                });
                bar.target_mode = None;
            }
        }
        BarAction::Reincarnate | BarAction::Whirlwind => {
            // One command per selected own object that shows the button (menu, MY filter); the
            // simulation checks module, lock and mana again (`check_cast_spell`, `check_ability`).
            let button = if action == BarAction::Reincarnate {
                REINCARNATE_BUTTON
            } else {
                WHIRLWIND_BUTTON
            };
            let player = host.active_player;
            let own = host.own_objects(selection);
            let mut sim = host.sim_context();
            let carriers: Vec<ObjectId> = own
                .into_iter()
                .filter(|&u| button_state(&mut sim, player, u, button).is_some())
                .collect();
            for unit in carriers {
                host.submit_own(if action == BarAction::Reincarnate {
                    CommandKind::CastSpell {
                        caster: unit,
                        art: spell::ART_REINCARNATE,
                        anim: 0,
                        idx: 0,
                        button,
                    }
                } else {
                    CommandKind::Whirlwind { unit, button }
                });
            }
        }
        BarAction::Target(button) => {
            // Only the first selected object (`Input_IssueTargetAction` `0x0044D350`, "1." in
            // re/spells 2.5), and only if the button is clickable there.
            let player = host.active_player;
            let Some(unit) = own_leader(host, selection).map(|o| o.id) else {
                return;
            };
            if !TARGET_BUTTONS.contains(&button) {
                return;
            }
            let mut sim = host.sim_context();
            if button_state(&mut sim, player, unit, button) == Some(true) {
                bar.target_mode = Some(TargetMode { unit, button });
                bar.tent_cursor = None;
            }
        }
        BarAction::Rob => {
            // One command per object with the free button 540 (pattern `SelfHeal`); the simulation checks
            // module, lock and mana again (`ability::execute`, `0x4C`).
            let player = host.active_player;
            let own = host.own_objects(selection);
            let mut sim = host.sim_context();
            let robbers: Vec<ObjectId> = own
                .into_iter()
                .filter(|&u| button_state(&mut sim, player, u, ROB_BUTTON) == Some(true))
                .collect();
            for unit in robbers {
                host.submit_own(CommandKind::RobCorpses {
                    unit,
                    button: ROB_BUTTON,
                });
            }
        }
    }
}

/// Send the target command of the button `button` for `unit` at `target` (for the targeting mode and the
/// control channel): [`target_command`] with the active player checks target kind and target filter
/// (hypothesis re/spells 2.5/11) and builds the command, which then goes through the host
/// (`submit_own`, like the other bar commands). `Err` with the reason, without a command. Until
/// then it only reads (the checksum stays).
pub fn issue_target(
    host: &mut SimHost,
    unit: ObjectId,
    button: i32,
    target: AbilityTarget,
) -> Result<(), String> {
    let player = host.active_player;
    let kind = {
        let mut sim = host.sim_context();
        target_command(&mut sim, player, unit, button, target)?
    };
    host.submit_own(kind);
    Ok(())
}

/// Click in the targeting mode: [`issue_target`] with the object and button from
/// `bar.target_mode`; on `Ok` the targeting mode ends. `Err` (unsuitable target, no targeting mode): no
/// command, the targeting mode stays, the status line shows the message.
pub fn click_target(
    bar: &mut UnitBar,
    host: &mut SimHost,
    target: AbilityTarget,
) -> Result<(), String> {
    let mode = bar.target_mode.ok_or("no targeting mode")?;
    issue_target(host, mode.unit, mode.button, target)?;
    bar.target_mode = None;
    Ok(())
}

/// Button of the unit bar via the control channel (`sim_unit_bar`, protocol 21): the buttons without a
/// target as before ([`BarAction::from_button`] and
/// [`apply`]); a target button ([`TARGET_BUTTONS`]) gives the target command immediately for the first
/// selected own object ([`own_leader`], [`issue_target`]) with the object `target` or the ground point
/// `ground` (world coordinates x, z), without touching the targeting mode. 484/556 always take the ground
/// point, as in the targeting mode ([`ability_target`]). Without a target the error "Button {b} needs a target
/// (target or ground)"; `Err` also for an unknown button, without an own first object and for
/// an unsuitable target ([`target_command`]), each without a command.
pub fn control_unit_bar(
    bar: &mut UnitBar,
    host: &mut SimHost,
    selection: &[ObjectId],
    button: i32,
    target: Option<u32>,
    ground: Option<[f32; 2]>,
) -> Result<(), String> {
    let action = BarAction::from_button(button).ok_or_else(|| {
        format!(
            "Button {button} does not exist in the unit bar (511-515, 519, 405, 406, 430, \
             431, 432, 433, 449-453, 485, 486, abilities 543, 544, 545, 549, 552, 557, 559, \
             560, 561, 563, counter-upgrades 3370-3377, camouflage 434, 435, dig in 437, 438, \
             healing 458, 459, 554, 555, 564, ghost 436, banish 439, 547, 550, 553, 558, 562, \
             animals 441, 442, 443, spells 480, 481, 482, 484, 541, special attacks 454-457, 542, \
             546, 548, 551, 556, robbing 540)"
        )
    })?;
    let BarAction::Target(button) = action else {
        apply(bar, host, selection, action);
        return Ok(());
    };
    if target.is_none() && ground.is_none() {
        return Err(format!("Button {button} needs a target (target or ground)"));
    }
    let floor = ground.map(|[x, z]| Vec3::new(x, 0.0, z));
    let aim = ability_target(button, target.map(ObjectId), floor)
        .ok_or_else(|| format!("Button {button} needs a ground point (ground)"))?;
    let unit = own_leader(host, selection)
        .map(|o| o.id)
        .ok_or_else(|| format!("Button {button}: the first selected object is not own"))?;
    issue_target(host, unit, button, aim)
}

/// End the targeting mode (Esc, right-click).
pub fn cancel_target_mode(bar: &mut UnitBar) {
    bar.target_mode = None;
}

/// Per frame next to [`follow_tent_cursor`]: the targeting mode ends if its object is no longer the first
/// selected own object (every change of selection), is dead (`object_is_dead`) or
/// the button is no longer clickable there ([`button_state`]: menu, MY filter, lock). Only reads.
pub fn follow_target_mode(bar: &mut UnitBar, host: &mut SimHost, selection: &[ObjectId]) {
    let Some(mode) = bar.target_mode else {
        return;
    };
    let leader = own_leader(host, selection)
        .filter(|o| !object_is_dead(o))
        .map(|o| o.id);
    let player = host.active_player;
    if leader != Some(mode.unit)
        || button_state(&mut host.sim_context(), player, mode.unit, mode.button) != Some(true)
    {
        bar.target_mode = None;
    }
}

/// Target of a click in the targeting mode from the object `object` and the terrain point `ground` under the
/// pointer: 484/556 always take the ground (re/spells 2.5, right-click on the ground), the other target buttons
/// the object, otherwise the ground (which [`target_command`] then rejects). Without both `None`.
pub fn ability_target(
    button: i32,
    object: Option<ObjectId>,
    ground: Option<Vec3>,
) -> Option<AbilityTarget> {
    let floor = ground.map(|g| AbilityTarget::Ground { x: g.x, z: g.z });
    if SCOUT_BUTTONS.contains(&button) {
        return floor;
    }
    object.map(AbilityTarget::Object).or(floor)
}

/// Status line of the targeting mode: "Select target: <button text>" (text from `guids.deu`, otherwise the
/// fallback label from [`SPELL_SPECIAL_BUTTONS`]).
pub fn target_mode_line(mode: &TargetMode, texts: &BTreeMap<i32, String>) -> String {
    let fallback = SPELL_SPECIAL_BUTTONS
        .iter()
        .find(|b| b.0 == mode.button)
        .map_or_else(|| format!("Button {}", mode.button), |b| b.1.to_string());
    format!("Select target: {}", text_or(texts, mode.button, &fallback))
}

/// Preview of the unpack cursor at the terrain point `ground`: the build cursor
/// ([`SimHost::placement_preview_rot`]) with GUID and rotation of the tent. It snaps and checks the
/// site with layer and region of the first selected own object (the travois) and the sight
/// (re/tents 3.4). Only reads.
pub fn tent_preview(
    host: &mut SimHost,
    cursor: &TentCursor,
    ground: Vec3,
    selection: &[ObjectId],
) -> Option<PlacementPreview> {
    host.placement_preview_rot(cursor.tent_guid, ground, cursor.rotation(), selection)
}

/// Left click with the unpack cursor (`FUN_00453490`, re/tents 3.4): with a valid preview the cursor
/// ends (`clear_cursor`) and `UnpackTent` goes to the first selected object (W-263, not to
/// "the travois") with the snapped position in fixed-point tiles, `Fixed_FromFloat(x / cell_x)`
/// and `Fixed_FromFloat(z / cell_z)`. Invalid site: no command, the cursor stays (like build
/// placement). Returns the checked preview (`None` without a cursor or hull).
pub fn click_unpack(
    bar: &mut UnitBar,
    host: &mut SimHost,
    selection: &[ObjectId],
    ground: Vec3,
) -> Option<PlacementPreview> {
    let cursor = bar.tent_cursor?;
    let preview = tent_preview(host, &cursor, ground, selection)?;
    if preview.valid
        && let Some(&unit) = selection.first()
    {
        bar.tent_cursor = None;
        let cell = host.rules.map.cell;
        let x = Fx::from_f32(preview.pos[0] / cell[0] as f32).0;
        let z = Fx::from_f32(preview.pos[2] / cell[2] as f32).0;
        host.submit_own(CommandKind::UnpackTent { unit, x, z });
    }
    Some(preview)
}

/// After every change of selection and every tick: the unpack cursor ends if its
/// travois is no longer the first selected object, no longer carries a tent (flag gone, e.g. after
/// setting up) or the tent is missing.
pub fn follow_tent_cursor(bar: &mut UnitBar, host: &SimHost, selection: &[ObjectId]) {
    if let Some(c) = bar.tent_cursor
        && unpack_tent_source(host, selection).is_none_or(|(unit, _)| unit != c.unit)
    {
        bar.tent_cursor = None;
    }
}

/// Overlay line of the unpack cursor (check aid like [`crate::game::economy::placement_line`]).
pub fn tent_cursor_line(cursor: &TentCursor, preview: Option<&PlacementPreview>) -> String {
    let keys = "Esc/right-click: end";
    match preview {
        None => format!(
            "Unpack tent: {} | no ground point | {keys}",
            cursor.tent_guid
        ),
        Some(p) => format!(
            "Unpack tent: {}, position ({:.1}, {:.1}), {} | {keys}",
            cursor.tent_guid,
            p.pos[0],
            p.pos[2],
            if p.valid { "valid" } else { "invalid" }
        ),
    }
}

/// Train command to the locomotive of the selection ([`selected_train`]); only this one, also with a
/// multi-selection (the bar sends `len = 1`, K-88), without a target (the target tile comes only from scripts,
/// re/train-multipart 2.2). Without a locomotive in the selection no command.
fn submit_train(host: &mut SimHost, selection: &[ObjectId], op: TrainOp) {
    if let Some((train, _)) = selected_train(host, selection) {
        host.submit_own(CommandKind::Train {
            train,
            op,
            target: None,
        });
    }
}

/// Submit the command for `units` if there are any.
fn submit_units(
    host: &mut SimHost,
    units: Vec<ObjectId>,
    command: impl FnOnce(Vec<ObjectId>) -> CommandKind,
) {
    if !units.is_empty() {
        host.submit_own(command(units));
    }
}

/// A command with all own selected objects (not only the walking ones: camouflage also applies to the
/// alarm post); whom it affects is decided by the simulation per ID ([`check_ability`]).
///
/// [`check_ability`]: nml_sim::command::ability::check_ability
fn submit_own_objects(
    host: &mut SimHost,
    selection: &[ObjectId],
    command: impl FnOnce(Vec<ObjectId>) -> CommandKind,
) {
    let units = host.own_objects(selection);
    submit_units(host, units, command);
}

/// Viewing direction and spread from the right-button drag from the ground point `start` to `end`
/// (world coordinates), as the original computes it (re/groups-formations 12.6):
/// - Mouse-move handler `0x00453790` (`0x00453986`...`0x00453A1E`, only with "Go to" 448): length
///   `|end - start|` (3D, `Vector3_Length` `0x00575130`); only above 16 world units does it count
///   (`0x00453991`). Angle `atan2(dz, dx) - pi/2` (`FPATAN` `0x004539B5`, only x and z) into
///   `Input+0x59C`, spread `min(length / 256 + 1, 2)` (`0x004539C7`...`0x004539E4`) into `+0x5A0`.
/// - `Input_IssueTargetAction` (`0x0044D350`): `direction = Fixed_FromFloat((angle + pi/2) *
///   57.295776)`, `spread = Fixed_FromFloat(spread)`.
///
/// The -pi/2 and the +pi/2 cancel: the formation looks in the drag direction (formation coordinate
/// "front" = `(cos dir, sin dir)`, re/groups-formations 3.3). The camera does not factor in, only the ground points.
pub fn formation_drag(start: Vec3, end: Vec3) -> Option<(i32, i32)> {
    // Basic arithmetic in `f32` like the x87 with a 24-bit mantissa (re/sim-core 2); `FPATAN`
    // is not rounded by the precision control, so `atan2` in `f64` and only the difference in
    // `f32`. So +z gives exactly 90 degrees (computed in `f64` it would be 89.99999 degrees -> 368639).
    let d = end - start;
    let length = (d.z * d.z + d.y * d.y + d.x * d.x).sqrt();
    // `FCOM 16.0`: length <= 16 (or NaN) is not a drag.
    if length.is_nan() || length <= MIN_DRAG_WORLD {
        return None;
    }
    let half_pi = std::f32::consts::FRAC_PI_2;
    let angle = (f64::from(d.z).atan2(f64::from(d.x)) - f64::from(half_pi)) as f32;
    let spread = (length * (1.0 / 256.0) + 1.0).min(2.0);
    let direction = (angle + half_pi) * 57.295776;
    Some((Fx::from_f32(direction).0, Fx::from_f32(spread).0))
}

/// Dragging with the right button via the mouse movements: like the handler `0x00453790` the last
/// movement beyond 16 world units counts; if the mouse then moves closer to the press point again,
/// direction and spread (and the flag `0x10`) stay until the button is released.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormationDragTracker {
    start: Vec3,
    last: Option<(i32, i32)>,
}

impl FormationDragTracker {
    /// Right button pressed at the ground point `start` (`Input+0x590...+0x598`).
    pub fn new(start: Vec3) -> Self {
        Self { start, last: None }
    }

    /// Mouse over the ground point `p`.
    pub fn moved(&mut self, p: Vec3) {
        if let Some(drag) = formation_drag(self.start, p) {
            self.last = Some(drag);
        }
    }

    /// Direction and spread on release (`None` = not dragged).
    pub fn drag(&self) -> Option<(i32, i32)> {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::economy::testkit::{COLONIST, host, spawn};
    use crate::game::formation_order::with_formation;
    use nml_sim::command::NO_SPREAD;
    use nml_sim::motion::NO_DIRECTION;
    use nml_sim::object::status;

    #[test]
    fn texts_lose_their_key_marks() {
        assert_eq!(clean_text("Line [KEYID 512]"), "Line");
        assert_eq!(
            clean_text("Current formation:[CR][FORMATION]"),
            "Current formation: [FORMATION]"
        );
        // Unknown and incomplete marks stay.
        assert_eq!(clean_text("a [b] [c"), "a [b] [c");
    }

    /// Selection of `n` own units without a riding or diving state.
    fn plain(n: usize) -> SelectionSummary {
        SelectionSummary {
            own_units: n,
            ..SelectionSummary::default()
        }
    }

    fn ids(buttons: &[BarButton]) -> Vec<i32> {
        buttons.iter().map(|b| b.id).collect()
    }

    #[test]
    fn buttons_need_own_units_and_show_the_chosen_formation() {
        let texts: BTreeMap<i32, String> = [
            (511, "none"),
            (512, "Line"),
            (513, "Square"),
            (514, "Column"),
            (515, "Pincer"),
            (519, "Scatter"),
        ]
        .into_iter()
        .map(|(k, v)| (k, v.to_string()))
        .collect();
        assert!(
            buttons(
                &UnitBar {
                    formation: 0,
                    ..UnitBar::default()
                },
                &texts,
                &plain(0)
            )
            .is_empty()
        );
        let b = buttons(
            &UnitBar {
                formation: 2,
                ..UnitBar::default()
            },
            &texts,
            &plain(3),
        );
        assert_eq!(ids(&b), vec![511, 512, 513, 514, 515, 519]);
        assert!(b.iter().find(|b| b.id == 513).unwrap().active);
        assert_eq!(b.iter().filter(|b| b.active).count(), 1);
        // Missing texts -> fallback with the number.
        assert_eq!(
            buttons(&UnitBar::default(), &BTreeMap::new(), &plain(2))[1].label,
            "Formation 512"
        );
    }

    #[test]
    fn the_bar_offers_dismount_leave_dive_and_surface_by_selection() {
        use crate::game::sim_host::testkit::{DIVER, HORSE, RIDER, host, link, spawn, spawn_for};
        let mut host = host();
        let texts = BTreeMap::new();
        let bar = UnitBar::default();
        let rider = spawn(&mut host, RIDER, 640.0, 640.0);
        let horse = spawn(&mut host, HORSE, 704.0, 640.0);
        let diver = spawn(&mut host, DIVER, 1280.0, 640.0);
        let offered = |host: &mut SimHost, selection: &[ObjectId]| {
            ids(&buttons(&bar, &texts, &summarize(host, selection)))
        };
        let formation = [511, 512, 513, 514, 515, 519];
        let with = |extra: &[i32]| [formation.as_slice(), extra].concat();
        // Unmounted, not in a boat, not diving: only 511-515/519; the diver may dive.
        assert_eq!(offered(&mut host, &[rider]), formation);
        assert_eq!(offered(&mut host, &[diver]), with(&[432]));
        // Mounted (`ISMOUNTED`) -> Dismount 405; the mount itself gets no button.
        link(&mut host, rider, horse);
        assert_eq!(offered(&mut host, &[rider]), with(&[405]));
        assert_eq!(offered(&mut host, &[horse]), formation);
        // In a boat (`ISMOUNTEDONBOAT`) -> Disembark 406.
        let o = host.state.object_mut(rider, false).unwrap();
        o.set_status_flags(status::ISMOUNTED, false);
        o.set_status_flags(status::ISMOUNTEDONBOAT, true);
        assert_eq!(offered(&mut host, &[rider]), with(&[406]));
        // Dived -> Surface 433 instead of Dive 432.
        host.state
            .object_mut(diver, false)
            .unwrap()
            .set_status_flags(status::DIVING, true);
        assert_eq!(offered(&mut host, &[diver]), with(&[433]));
        // Mixed selection: one button if the condition holds for at least one unit.
        assert_eq!(offered(&mut host, &[rider, diver]), with(&[406, 433]));
        host.state
            .object_mut(diver, false)
            .unwrap()
            .set_status_flags(status::DIVING, false);
        assert_eq!(offered(&mut host, &[rider, diver]), with(&[406, 432]));
        // Foreign units do not count: only own ones, without an own unit no bar.
        let foreign = spawn_for(&mut host, DIVER, 1, 1920.0, 640.0);
        assert_eq!(offered(&mut host, &[rider, foreign]), with(&[406]));
        assert!(offered(&mut host, &[foreign]).is_empty());
        assert_eq!(
            summarize(&mut host, &[rider, foreign]),
            SelectionSummary {
                own_units: 1,
                mounted: false,
                in_boat: true,
                can_dive: false,
                diving: false,
                train: None,
                extra: Vec::new(),
            }
        );
    }

    #[test]
    fn the_new_buttons_submit_their_commands() {
        use crate::game::sim_host::testkit::{DIVER, RIDER, host, spawn, spawn_for};
        let mut host = host();
        let mut bar = UnitBar {
            formation: 3,
            ..UnitBar::default()
        };
        let rider = spawn(&mut host, RIDER, 640.0, 640.0);
        let diver = spawn(&mut host, DIVER, 1280.0, 640.0);
        let foreign = spawn_for(&mut host, RIDER, 1, 1920.0, 640.0);
        let selection = [rider, diver, foreign];
        let last = |host: &SimHost| host.pending_commands().last().unwrap().kind.clone();
        let own = vec![rider, diver];
        for (action, expected) in [
            (
                BarAction::Dismount,
                CommandKind::Dismount { units: own.clone() },
            ),
            (
                BarAction::LeaveBoat,
                CommandKind::LeaveBoat { units: own.clone() },
            ),
            (BarAction::Dive, CommandKind::Dive { units: own.clone() }),
            (
                BarAction::Surface,
                CommandKind::Surface { units: own.clone() },
            ),
        ] {
            let before = host.pending_commands().len();
            apply(&mut bar, &mut host, &selection, action);
            assert_eq!(host.pending_commands().len(), before + 1, "{action:?}");
            assert_eq!(last(&host), expected, "{action:?}");
        }
        // The selected formation stays untouched.
        assert_eq!(bar.formation, 3);
        // Without an own unit no command.
        let before = host.pending_commands().len();
        apply(&mut bar, &mut host, &[foreign], BarAction::Dive);
        assert_eq!(host.pending_commands().len(), before);
        // IDs of the buttons and actions.
        assert_eq!(
            [
                DISMOUNT_BUTTON,
                LEAVE_BOAT_BUTTON,
                DIVE_BUTTON,
                SURFACE_BUTTON
            ],
            [405, 406, 432, 433]
        );
        for (button, action) in [
            (DISMOUNT_BUTTON, BarAction::Dismount),
            (LEAVE_BOAT_BUTTON, BarAction::LeaveBoat),
            (DIVE_BUTTON, BarAction::Dive),
            (SURFACE_BUTTON, BarAction::Surface),
        ] {
            assert_eq!(BarAction::from_button(button), Some(action));
        }
    }

    #[test]
    fn the_new_buttons_have_texts_and_fallbacks() {
        let all = SelectionSummary {
            own_units: 1,
            mounted: true,
            in_boat: true,
            can_dive: true,
            diving: true,
            train: None,
            extra: Vec::new(),
        };
        let b = buttons(&UnitBar::default(), &BTreeMap::new(), &all);
        assert_eq!(ids(&b), [511, 512, 513, 514, 515, 519, 405, 406, 432, 433]);
        let labels: Vec<_> = b.iter().skip(6).map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Dismount", "Disembark", "Dive", "Surface"]);
        assert!(b.iter().skip(6).all(|b| !b.active));
        let texts: BTreeMap<i32, String> = [(405, "Down"), (433, "Up")]
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect();
        let b = buttons(
            &UnitBar::default(),
            &texts,
            &SelectionSummary {
                own_units: 1,
                mounted: true,
                diving: true,
                ..SelectionSummary::default()
            },
        );
        let labels: Vec<_> = b.iter().skip(6).map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Down", "Up"]);
    }

    // --- Train buttons 449-451 ---

    /// `toolbar.ini` of the test world: only the locomotive (2613) lists the train buttons (re/train-multipart 2.2).
    const TRAIN_TOOLBAR: &str = "GUID 2613
BUTTONID 0
BUTTONMSG 450
TARGET_DO_MSG
        BUTTONID 1
BUTTONMSG 451
TARGET_DO_MSG
BUTTONID 2
BUTTONMSG 449
TARGET_DO_MSG
ENDGUID
";

    /// Train host with the train bar in `toolbar.ini`; locomotive and tender stand free on the map.
    fn train_bar_host() -> SimHost {
        let mut host = crate::game::sim_host::testkit::train_host();
        host.rules.toolbar = nml_formats::gifcon::parse_toolbar(TRAIN_TOOLBAR);
        host
    }

    fn set_moving(host: &mut SimHost, id: ObjectId, moving: bool) {
        host.state
            .object_mut(id, false)
            .unwrap()
            .set_status_flags(status::MOVE, moving);
    }

    #[test]
    fn the_bar_offers_train_buttons_by_motion() {
        use crate::game::sim_host::testkit::{CART, LOCO, TENDER, spawn_dir};
        let mut host = train_bar_host();
        let texts = BTreeMap::new();
        let bar = UnitBar::default();
        let offered = |host: &mut SimHost, selection: &[ObjectId]| {
            ids(&buttons(&bar, &texts, &summarize(host, selection)))
        };
        let loco = spawn_dir(&mut host, LOCO, 0, (10, 10), 0);
        let loco2 = spawn_dir(&mut host, LOCO, 0, (10, 14), 0);
        let tender = spawn_dir(&mut host, TENDER, 0, (10, 18), 0);
        let foreign = spawn_dir(&mut host, LOCO, 1, (10, 22), 0);
        let cart = spawn_dir(&mut host, CART, 0, (10, 26), 0);
        // Locomotive standing: forward and backward, no formation buttons (no walk module).
        assert_eq!(offered(&mut host, &[loco]), [449, 450]);
        assert_eq!(summarize(&mut host, &[loco]).train, Some((loco, false)));
        assert_eq!(summarize(&mut host, &[loco]).own_units, 0);
        // Moving (status bit `MOVE`): only stop.
        set_moving(&mut host, loco, true);
        assert_eq!(offered(&mut host, &[loco]), [451]);
        assert_eq!(summarize(&mut host, &[loco]).train, Some((loco, true)));
        // Tender alone, foreign locomotive and locomotive without a train bar in `toolbar.ini`: no button.
        assert!(offered(&mut host, &[tender]).is_empty());
        assert_eq!(summarize(&mut host, &[tender]).train, None);
        assert!(offered(&mut host, &[foreign]).is_empty());
        host.rules.toolbar = Default::default();
        assert!(offered(&mut host, &[loco]).is_empty());
        host.rules.toolbar = nml_formats::gifcon::parse_toolbar(TRAIN_TOOLBAR);
        // Two locomotives selected: the first determines buttons and command (K-88: the bar sends `len = 1`).
        assert_eq!(offered(&mut host, &[loco, loco2]), [451]);
        assert_eq!(offered(&mut host, &[loco2, loco]), [449, 450]);
        assert_eq!(
            summarize(&mut host, &[loco2, loco]).train,
            Some((loco2, false))
        );
        // The locomotive before the tender and with a foot unit: formation buttons and train buttons together.
        set_moving(&mut host, loco, false);
        assert_eq!(
            offered(&mut host, &[tender, cart, loco]),
            [511, 512, 513, 514, 515, 519, 449, 450]
        );
    }

    #[test]
    fn train_buttons_submit_train_commands() {
        use crate::game::sim_host::testkit::{LOCO, TENDER, spawn_dir};
        use nml_sim::command::TrainOp;
        let mut host = train_bar_host();
        let mut bar = UnitBar {
            formation: 2,
            ..UnitBar::default()
        };
        let first = spawn_dir(&mut host, LOCO, 0, (10, 10), 0);
        let second = spawn_dir(&mut host, LOCO, 0, (10, 14), 0);
        let tender = spawn_dir(&mut host, TENDER, 0, (10, 18), 0);
        let last = |host: &SimHost| host.pending_commands().last().unwrap().kind.clone();
        for (action, op) in [
            (BarAction::TrainForward, TrainOp::Forward),
            (BarAction::TrainBackward, TrainOp::Backward),
            (BarAction::TrainStop, TrainOp::Stop),
        ] {
            let before = host.pending_commands().len();
            // K-88: multi-selection, the command goes only to the first locomotive.
            apply(&mut bar, &mut host, &[tender, first, second], action);
            assert_eq!(host.pending_commands().len(), before + 1, "{action:?}");
            assert_eq!(
                last(&host),
                CommandKind::Train {
                    train: first,
                    op,
                    target: None
                },
                "{action:?}"
            );
        }
        // Without a locomotive in the selection no command; the formation of the bar stays untouched.
        let before = host.pending_commands().len();
        apply(&mut bar, &mut host, &[tender], BarAction::TrainStop);
        assert_eq!(host.pending_commands().len(), before);
        assert_eq!(bar.formation, 2);
        // Button numbers and mapping.
        assert_eq!(
            [
                TRAIN_FORWARD_BUTTON,
                TRAIN_BACKWARD_BUTTON,
                TRAIN_STOP_BUTTON
            ],
            [449, 450, 451]
        );
        for (button, action) in [
            (449, BarAction::TrainForward),
            (450, BarAction::TrainBackward),
            (451, BarAction::TrainStop),
        ] {
            assert_eq!(BarAction::from_button(button), Some(action));
        }
        // 452 (pack tent).
        assert_eq!(BarAction::from_button(452), Some(BarAction::PackTent));
    }

    #[test]
    fn the_train_buttons_have_texts_and_fallbacks() {
        use crate::game::sim_host::testkit::LOCO;
        let id = ObjectId(LOCO);
        let at_rest = SelectionSummary {
            train: Some((id, false)),
            ..SelectionSummary::default()
        };
        let moving = SelectionSummary {
            train: Some((id, true)),
            ..SelectionSummary::default()
        };
        let labels = |texts: &BTreeMap<i32, String>, s: &SelectionSummary| -> Vec<String> {
            buttons(&UnitBar::default(), texts, s)
                .into_iter()
                .map(|b| b.label)
                .collect()
        };
        let none = BTreeMap::new();
        assert_eq!(labels(&none, &at_rest), ["Forward", "Backward"]);
        assert_eq!(labels(&none, &moving), ["Stop"]);
        let texts: BTreeMap<i32, String> = [(449, "Go"), (450, "Reverse"), (451, "Brake")]
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect();
        assert_eq!(labels(&texts, &at_rest), ["Go", "Reverse"]);
        assert_eq!(labels(&texts, &moving), ["Brake"]);
        // No formation, no unit: the train buttons are never "selected".
        assert!(
            buttons(&UnitBar::default(), &texts, &at_rest)
                .iter()
                .all(|b| !b.active)
        );
    }

    #[test]
    fn a_locomotive_alone_gets_no_formation_header() {
        let mut texts = BTreeMap::new();
        texts.insert(510, "Current formation: [FORMATION]".to_string());
        let bar = UnitBar::default();
        let train_only = SelectionSummary {
            train: Some((ObjectId(1), false)),
            ..SelectionSummary::default()
        };
        assert_eq!(header_for(&bar, &texts, &train_only), "");
        assert_eq!(
            header_for(&bar, &texts, &plain(2)),
            "Current formation: Formation 511"
        );
    }

    #[test]
    fn the_header_names_the_chosen_formation() {
        let mut texts = BTreeMap::new();
        texts.insert(510, "Current formation: [FORMATION]".to_string());
        texts.insert(513, "Square".to_string());
        assert_eq!(
            header(
                &UnitBar {
                    formation: 2,
                    ..UnitBar::default()
                },
                &texts
            ),
            "Current formation: Square"
        );
        // Outside 0...4, 0 applies.
        assert_eq!(
            header(
                &UnitBar {
                    formation: 9,
                    ..UnitBar::default()
                },
                &BTreeMap::new()
            ),
            "Current formation: Formation 511"
        );
    }

    #[test]
    fn a_drag_gives_its_own_direction_and_spread_from_world_units() {
        let at = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        // Dragged toward +x -> view 0 degrees; 102.4 world units -> 1 + 102.4/256 = 1.4 -> 5734.
        assert_eq!(
            formation_drag(at(100.0, 0.0, 100.0), at(202.4, 0.0, 100.0)),
            Some((0, 5734))
        );
        // Toward +z -> 90 degrees, toward -z -> -90 degrees (fixed point, not wrapped). Toward -x just below 180
        // degrees: pi - pi/2 rounds down in `f32`, the sum with pi/2 lies exactly between two `f32`
        // (even mantissa wins), `Fixed_FromFloat` truncates -> 737279 instead of 737280.
        let dir = |x: f32, z: f32| formation_drag(at(0.0, 0.0, 0.0), at(x, 0.0, z)).unwrap().0;
        assert_eq!(dir(0.0, 200.0), 90 << 12);
        assert_eq!(dir(-200.0, 0.0), (180 << 12) - 1);
        assert_eq!(dir(0.0, -200.0), -(90 << 12));
        // Spread at most 2.0 (from 256 world units).
        assert_eq!(
            formation_drag(at(0.0, 0.0, 0.0), at(1000.0, 0.0, 0.0)),
            Some((0, 8192))
        );
        // Length in 3D: the height difference counts, the angle does not.
        assert_eq!(
            formation_drag(at(0.0, 0.0, 0.0), at(10.0, 15.0, 0.0)).map(|d| d.0),
            Some(0)
        );
        // Up to 16 world units it is not a drag.
        assert_eq!(formation_drag(at(0.0, 0.0, 0.0), at(16.0, 0.0, 0.0)), None);
        assert_eq!(
            formation_drag(at(0.0, 0.0, 0.0), at(f32::NAN, 0.0, 0.0)),
            None
        );
    }

    #[test]
    fn the_last_drag_beyond_16_units_stays_until_release() {
        let mut t = FormationDragTracker::new(Vec3::ZERO);
        t.moved(Vec3::new(8.0, 0.0, 0.0));
        assert_eq!(t.drag(), None);
        t.moved(Vec3::new(0.0, 0.0, 128.0));
        let far = t.drag();
        assert_eq!(far, Some((90 << 12, 6144)));
        // Back below 16 world units: the last drag stays (`0x00453A79`, no clearing).
        t.moved(Vec3::new(4.0, 0.0, 0.0));
        assert_eq!(t.drag(), far);
    }

    /// Game camera over flat terrain (800 x 600): ground point under a pixel and projection
    /// back.
    struct GameView {
        view_proj: glam::Mat4,
        terrain: nml_world::terrain::Terrain,
    }

    impl GameView {
        fn new() -> Self {
            let (camera, terrain) = crate::game::input::testkit::setup();
            let projection = camera.projection(nml_world::camera::AspectMode::Correct, 800, 600);
            Self {
                view_proj: projection * camera.view(),
                terrain,
            }
        }

        fn ground(&self, px: (f32, f32)) -> Vec3 {
            let ndc = glam::Vec2::new(px.0 / 400.0 - 1.0, 1.0 - px.1 / 300.0);
            let (origin, dir) = nml_world::camera::ray_from_ndc(self.view_proj, ndc);
            self.terrain.ray_hit(origin, dir, 100_000.0).unwrap()
        }

        fn project(&self, p: Vec3) -> [f32; 2] {
            crate::game::grid::project(self.view_proj, p, (800, 600)).unwrap()
        }
    }

    #[test]
    fn a_drag_on_screen_faces_the_formation_along_the_drag_on_screen() {
        // `0x00453790`: angle from the ground points (world x/z), `0x0044D350` cancels the -pi/2
        // again -> the formation looks in the drag direction, also on the screen.
        let view = GameView::new();
        let from = (400.0, 300.0);
        for to in [
            (560.0, 300.0),
            (400.0, 160.0),
            (290.0, 410.0),
            (520.0, 200.0),
        ] {
            let (a, b) = (view.ground(from), view.ground(to));
            let (dir, _) = formation_drag(a, b).unwrap();
            let rad = (dir as f32 / 4096.0).to_radians();
            let ahead = b + glam::Vec3::new(rad.cos(), 0.0, rad.sin()) * 256.0;
            let (pb, pa) = (view.project(b), view.project(ahead));
            let seen = (pa[1] - pb[1]).atan2(pa[0] - pb[0]).to_degrees();
            let dragged = (to.1 - from.1).atan2(to.0 - from.0).to_degrees();
            let off = (seen - dragged + 540.0).rem_euclid(360.0) - 180.0;
            assert!(
                off.abs() < 1.0,
                "Drag {from:?} -> {to:?}: view {off} degrees off"
            );
        }
    }

    #[test]
    fn moves_get_the_chosen_formation_and_the_drag() {
        let mv = CommandKind::Move {
            units: vec![ObjectId(1), ObjectId(2)],
            x: 1,
            z: 2,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: NO_SPREAD,
            flags: 0,
        };
        let k = with_formation(mv.clone(), 3, Some((90 << 12, 5734)));
        assert!(matches!(
            k,
            CommandKind::Move {
                formation: 3,
                direction: 368640,
                spread: 5734,
                ..
            }
        ));
        // Without a drag, direction and spread stay "none".
        assert!(matches!(
            with_formation(mv, 3, None),
            CommandKind::Move {
                formation: 3,
                spread: NO_SPREAD,
                direction: NO_DIRECTION,
                ..
            }
        ));
        // Other commands stay unchanged.
        let h = CommandKind::Halt { units: vec![] };
        assert_eq!(with_formation(h.clone(), 3, None), h);
    }

    fn two_colonists() -> (SimHost, Vec<ObjectId>) {
        let mut host = host();
        let units = vec![
            spawn(&mut host, COLONIST, 0, (10, 10)),
            spawn(&mut host, COLONIST, 0, (12, 10)),
        ];
        (host, units)
    }

    #[test]
    fn the_unit_bar_sends_set_formation_and_scatter_for_the_selection() {
        let (mut host, units) = two_colonists();
        let mut bar = UnitBar::default();
        apply(&mut bar, &mut host, &units, BarAction::Formation(4));
        assert_eq!(bar.formation, 4);
        assert!(matches!(
            &host.pending_commands().last().unwrap().kind,
            CommandKind::SetFormation { formation: 4, units: u } if *u == units
        ));
        apply(&mut bar, &mut host, &units, BarAction::Scatter);
        assert!(matches!(
            &host.pending_commands().last().unwrap().kind,
            CommandKind::Scatter { .. }
        ));
        // A single unit remembers the formation but gets no command.
        let before = host.pending_commands().len();
        apply(&mut bar, &mut host, &units[..1], BarAction::Formation(1));
        assert_eq!((bar.formation, host.pending_commands().len()), (1, before));
        // Unknown formation -> 0.
        apply(&mut bar, &mut host, &units, BarAction::Formation(7));
        assert_eq!(bar.formation, 0);
    }

    #[test]
    fn buttons_map_to_actions() {
        assert_eq!(BarAction::from_button(511), Some(BarAction::Formation(0)));
        assert_eq!(BarAction::from_button(515), Some(BarAction::Formation(4)));
        assert_eq!(BarAction::from_button(519), Some(BarAction::Scatter));
        assert_eq!(BarAction::from_button(516), None);
        assert_eq!(BarAction::from_button(405), Some(BarAction::Dismount));
        assert_eq!(BarAction::from_button(433), Some(BarAction::Surface));
        assert_eq!(BarAction::from_button(404), None);
        // Gate buttons.
        assert_eq!(BarAction::from_button(430), Some(BarAction::OpenGate));
        assert_eq!(BarAction::from_button(431), Some(BarAction::CloseGate));
        assert_eq!([OPEN_GATE_BUTTON, CLOSE_GATE_BUTTON], [430, 431]);
        assert_eq!(BarAction::from_button(429), None);
    }

    // --- Open gate 430 and close gate 431 (re/arming-gates 2.C, 7) ---

    #[test]
    fn gate_buttons_follow_the_door_state() {
        use crate::game::testworld::{GATE, host, spawn, spawn_site};
        let mut host = host();
        let gate = spawn(&mut host, GATE, 0, (10, 10));
        let foreign = spawn(&mut host, GATE, 1, (30, 10));
        let site = spawn_site(&mut host, GATE, 0, (20, 30));
        let texts = BTreeMap::new();
        let bar = UnitBar::default();
        let offered = |host: &mut SimHost, selection: &[ObjectId]| {
            ids(&buttons(&bar, &texts, &summarize(host, selection)))
        };
        let flags = |host: &SimHost, id: ObjectId| {
            let m = host
                .state
                .object(id, false)
                .unwrap()
                .module(building::INDEX);
            let m = m.expect("building module");
            (
                m.base.has_flags(building::flags::TOROPEN),
                m.base.has_flags(building::flags::TORCLOSE),
            )
        };
        // A finished pre-placed gate is open (gate init): only 431, with the fallback label, free.
        assert_eq!(flags(&host, gate), (true, false));
        assert_eq!(offered(&mut host, &[gate]), [431]);
        let b = buttons(&bar, &texts, &summarize(&mut host, &[gate]));
        let close = button(&b, CLOSE_GATE_BUTTON).unwrap();
        assert_eq!((close.label.as_str(), close.enabled), ("Close gate", true));
        // Texts from `guids.deu` instead of the fallback label.
        let named: BTreeMap<i32, String> = [(431, "Shut".to_string())].into_iter().collect();
        let b = buttons(&bar, &named, &summarize(&mut host, &[gate]));
        assert_eq!(button(&b, 431).unwrap().label, "Shut");
        // The query leaves the checksum the same.
        let before = host.checksum();
        let state = nml_sim::save::checksum(&host.state);
        let _ = summarize(&mut host, &[gate]);
        assert_eq!(nml_sim::save::checksum(&host.state), state);
        assert_eq!(host.checksum(), before);
        // 431 sends `CloseGate` with the gate; the wrong counterpart (430) sends nothing.
        let mut bar = UnitBar::default();
        apply(&mut bar, &mut host, &[gate], BarAction::OpenGate);
        assert!(host.pending_commands().is_empty());
        apply(&mut bar, &mut host, &[gate], BarAction::CloseGate);
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(sent, [CommandKind::CloseGate { gate }]);
        assert_eq!(bar, UnitBar::default());
        // After one step the gate is closed: only 430.
        host.step_once();
        assert_eq!(flags(&host, gate), (false, true));
        assert_eq!(offered(&mut host, &[gate]), [430]);
        let b = buttons(&bar, &texts, &summarize(&mut host, &[gate]));
        let open = button(&b, OPEN_GATE_BUTTON).unwrap();
        assert_eq!((open.label.as_str(), open.enabled), ("Open gate", true));
        // 430 opens it again.
        apply(&mut bar, &mut host, &[gate], BarAction::OpenGate);
        host.step_once();
        assert_eq!(flags(&host, gate), (true, false));
        assert_eq!(offered(&mut host, &[gate]), [431]);
        // Gate under construction (no `TOR` until completion) and foreign gate: no button, no command.
        assert!(offered(&mut host, &[site]).is_empty());
        assert!(offered(&mut host, &[foreign]).is_empty());
        for target in [site, foreign] {
            apply(
                &mut UnitBar::default(),
                &mut host,
                &[target],
                BarAction::CloseGate,
            );
            apply(
                &mut UnitBar::default(),
                &mut host,
                &[target],
                BarAction::OpenGate,
            );
        }
        assert!(host.pending_commands().is_empty());
        // The first of the selection decides: a gate behind a foreign gate gives no button.
        assert!(offered(&mut host, &[foreign, gate]).is_empty());
        assert_eq!(offered(&mut host, &[gate, foreign]), [431]);
    }

    #[test]
    fn gates_open_and_close_per_object() {
        // K-88: 430/431 send one command per gate of the selection (`FUN_0044B3F0` over the list),
        // not only for the first; gates in the other state and foreign ones stay out.
        use crate::game::testworld::{GATE, host, spawn};
        let mut host = host();
        let a = spawn(&mut host, GATE, 0, (10, 10));
        let b = spawn(&mut host, GATE, 0, (20, 10));
        let foreign = spawn(&mut host, GATE, 1, (30, 10));
        apply(
            &mut UnitBar::default(),
            &mut host,
            &[a, foreign, b],
            BarAction::CloseGate,
        );
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(
            sent,
            [
                CommandKind::CloseGate { gate: a },
                CommandKind::CloseGate { gate: b }
            ]
        );
        host.step_once();
        // Now `a` and `b` are closed. "Open" with only `a` in the selection opens only `a`.
        apply(
            &mut UnitBar::default(),
            &mut host,
            &[a],
            BarAction::OpenGate,
        );
        host.step_once();
        // Only `b` is still closed: "open" for `[b, a]` affects only `b`, the already open `a` gets no command.
        apply(
            &mut UnitBar::default(),
            &mut host,
            &[b, a],
            BarAction::OpenGate,
        );
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(sent, [CommandKind::OpenGate { gate: b }]);
    }

    #[test]
    fn a_dead_gate_has_no_buttons() {
        use crate::game::testworld::{GATE, host, spawn};
        let mut host = host();
        let gate = spawn(&mut host, GATE, 0, (10, 10));
        let texts = BTreeMap::new();
        let bar = UnitBar::default();
        let offered = |host: &mut SimHost| ids(&buttons(&bar, &texts, &summarize(host, &[gate])));
        assert_eq!(offered(&mut host), [431]);
        host.state.object_mut(gate, false).unwrap().status |= status::DIED;
        assert!(offered(&mut host).is_empty());
        apply(
            &mut UnitBar::default(),
            &mut host,
            &[gate],
            BarAction::CloseGate,
        );
        assert!(host.pending_commands().is_empty());
    }

    // --- Camouflage, dig in, healing (re/masking 2.4, re/healing 2.4, W-277) ---

    #[test]
    fn from_button_maps_the_ability_buttons() {
        for (button, action) in [
            (434, BarAction::Mask),
            (435, BarAction::Unmask),
            (437, BarAction::DigIn),
            (438, BarAction::DigOut),
            (458, BarAction::StartHealing),
            (554, BarAction::StartHealing),
            (459, BarAction::StopHealing),
            (564, BarAction::StopHealing),
            (555, BarAction::SelfHeal),
        ] {
            assert_eq!(BarAction::from_button(button), Some(action), "{button}");
        }
        // Neighbors without these actions (556-563 are spells, 565 is dead, K-89); 436, 439 and 553
        // belong to the evocation group (`from_button_maps_the_evocation_buttons`).
        for button in [433, 457, 460, 556, 565] {
            assert!(
                !matches!(
                    BarAction::from_button(button),
                    Some(
                        BarAction::Mask
                            | BarAction::Unmask
                            | BarAction::DigIn
                            | BarAction::DigOut
                            | BarAction::StartHealing
                            | BarAction::StopHealing
                            | BarAction::SelfHeal
                    )
                ),
                "{button}"
            );
        }
    }

    /// The buttons 434 to 564 of the bar for `selection`: `(button, clickable)` in bar order.
    fn ability_bar(host: &mut SimHost, selection: &[ObjectId]) -> Vec<(i32, bool)> {
        let summary = summarize(host, selection);
        buttons(&UnitBar::default(), &BTreeMap::new(), &summary)
            .into_iter()
            .filter(|b| STEALTH_HEAL_BUTTONS.iter().any(|a| a.0 == b.id))
            .map(|b| (b.id, b.enabled))
            .collect()
    }

    #[test]
    fn mask_buttons_follow_the_mask_bit() {
        use crate::game::testworld::{ARCHER, MASK_UPGRADE, host, spawn, unlock};
        let mut host = host();
        // Without 2924 module 25 is off: 434 is shown gray in the bar, 435 is missing (MY filter).
        let plain = spawn(&mut host, ARCHER, 0, (10, 10));
        assert_eq!(ability_bar(&mut host, &[plain]), [(434, false)]);
        unlock(&mut host, 0, MASK_UPGRADE);
        let archer = spawn(&mut host, ARCHER, 0, (12, 10));
        assert_eq!(ability_bar(&mut host, &[archer]), [(434, true)]);
        // A selection of both: one is enough, the button is clickable.
        assert_eq!(ability_bar(&mut host, &[plain, archer]), [(434, true)]);
        // The button gives `Mask` with all own selected objects.
        let mut bar = UnitBar::default();
        apply(&mut bar, &mut host, &[plain, archer], BarAction::Mask);
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(
            sent,
            [CommandKind::Mask {
                units: vec![plain, archer]
            }]
        );
        // During the camouflage animation (time marker 1600 ms, `MASKED` does not come until its end) 434 stays
        // visible and 435 hidden (W-277); afterwards it flips to 435.
        let masked =
            |host: &SimHost| host.state.object(archer, false).unwrap().status & status::MASKED != 0;
        let mut during = 0;
        for _ in 0..80 {
            host.step_once();
            if masked(&host) {
                break;
            }
            assert_eq!(ability_bar(&mut host, &[archer]), [(434, true)]);
            during += 1;
        }
        assert!(masked(&host), "the archer camouflages itself");
        assert!(during >= 10, "camouflage animation takes: {during} steps");
        assert_eq!(ability_bar(&mut host, &[archer]), [(435, true)]);
        // Uncamouflage sends `Unmask`; afterwards 434 again.
        apply(&mut bar, &mut host, &[archer], BarAction::Unmask);
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(
            sent,
            [CommandKind::Unmask {
                units: vec![archer]
            }]
        );
        // 435 also stays visible during the uncamouflage animation (W-277: the bar checks only
        // `MASKED`, which is not cleared until its end).
        let mut unmasking = 0;
        for _ in 0..80 {
            host.step_once();
            if !masked(&host) {
                break;
            }
            assert_eq!(ability_bar(&mut host, &[archer]), [(435, true)]);
            unmasking += 1;
        }
        assert!(!masked(&host), "the archer is uncamouflaged");
        assert!(
            unmasking >= 2,
            "uncamouflage animation takes: {unmasking} steps"
        );
        assert_eq!(ability_bar(&mut host, &[archer]), [(434, true)]);
        // Swimming: no camouflage button (follow-up filter `SWIMMING`).
        host.state.object_mut(archer, false).unwrap().status |= status::SWIMMING;
        assert!(ability_bar(&mut host, &[archer]).is_empty());
    }

    #[test]
    fn dig_and_heal_buttons_follow_their_status() {
        use crate::game::testworld::{
            ASSASSIN, DIG_IN_UPGRADE, HEAL_UPGRADE, MEDIC, PILGRIM, host, spawn, unlock,
        };
        let mut host = host();
        // Dig in 437 gray without 2922; with 2922 clickable, after `WAITING4KILLING` it switches to 438.
        let plain = spawn(&mut host, ASSASSIN, 0, (10, 10));
        assert_eq!(ability_bar(&mut host, &[plain]), [(437, false)]);
        unlock(&mut host, 0, DIG_IN_UPGRADE);
        let assassin = spawn(&mut host, ASSASSIN, 0, (12, 10));
        assert_eq!(ability_bar(&mut host, &[assassin]), [(437, true)]);
        host.state.object_mut(assassin, false).unwrap().status |= status::WAITING4KILLING;
        assert_eq!(ability_bar(&mut host, &[assassin]), [(438, true)]);
        // A selection of a dug-in and a fresh assassin shows both buttons.
        let fresh = spawn(&mut host, ASSASSIN, 0, (14, 10));
        assert_eq!(
            ability_bar(&mut host, &[assassin, fresh]),
            [(437, true), (438, true)]
        );
        // Pilgrim: 554 gray without 3125, 555 free; in the trance 564 instead of 554.
        let pilgrim = spawn(&mut host, PILGRIM, 0, (16, 10));
        assert_eq!(
            ability_bar(&mut host, &[pilgrim]),
            [(554, false), (555, true)]
        );
        unlock(&mut host, 0, HEAL_UPGRADE);
        assert_eq!(
            ability_bar(&mut host, &[pilgrim]),
            [(554, true), (555, true)]
        );
        host.state.object_mut(pilgrim, false).unwrap().status |= status::HEAL;
        assert_eq!(
            ability_bar(&mut host, &[pilgrim]),
            [(555, true), (564, true)]
        );
        // Medic: 458/459 without a prerequisite, no 555.
        let medic = spawn(&mut host, MEDIC, 0, (18, 10));
        assert_eq!(ability_bar(&mut host, &[medic]), [(458, true)]);
        host.state.object_mut(medic, false).unwrap().status |= status::HEAL;
        assert_eq!(ability_bar(&mut host, &[medic]), [(459, true)]);
        // Order of the mixed selection: ascending by button number.
        let sel = [assassin, fresh, pilgrim, medic];
        assert_eq!(
            ability_bar(&mut host, &sel),
            [
                (437, true),
                (438, true),
                (459, true),
                (555, true),
                (564, true)
            ]
        );
        // Foreign objects contribute nothing.
        let foreign = spawn(&mut host, PILGRIM, 1, (30, 10));
        assert!(ability_bar(&mut host, &[foreign]).is_empty());
    }

    #[test]
    fn the_ability_buttons_send_one_command_for_all_own_objects() {
        use crate::game::testworld::{ARCHER, ASSASSIN, MEDIC, PILGRIM, host, spawn};
        let mut host = host();
        let a = spawn(&mut host, MEDIC, 0, (10, 10));
        let b = spawn(&mut host, PILGRIM, 0, (12, 10));
        let c = spawn(&mut host, ASSASSIN, 0, (14, 10));
        let foreign = spawn(&mut host, ARCHER, 1, (30, 10));
        let selection = [a, foreign, b, c];
        let sent = |host: &mut SimHost, action: BarAction| {
            let mut bar = UnitBar::default();
            apply(&mut bar, host, &selection, action);
            let out: Vec<CommandKind> = host
                .pending_commands()
                .iter()
                .map(|c| c.kind.clone())
                .collect();
            host.step_once();
            out
        };
        let units = vec![a, b, c];
        assert_eq!(
            sent(&mut host, BarAction::DigIn),
            [CommandKind::DigIn {
                units: units.clone()
            }]
        );
        assert_eq!(
            sent(&mut host, BarAction::DigOut),
            [CommandKind::DigOut {
                units: units.clone()
            }]
        );
        assert_eq!(
            sent(&mut host, BarAction::StartHealing),
            [CommandKind::StartHealing {
                units: units.clone()
            }]
        );
        assert_eq!(
            sent(&mut host, BarAction::StopHealing),
            [CommandKind::StopHealing {
                units: units.clone()
            }]
        );
        assert_eq!(
            sent(&mut host, BarAction::Mask),
            [CommandKind::Mask {
                units: units.clone()
            }]
        );
        assert_eq!(
            sent(&mut host, BarAction::Unmask),
            [CommandKind::Unmask { units }]
        );
        // Only foreign objects selected: no command.
        let mut bar = UnitBar::default();
        apply(&mut bar, &mut host, &[foreign], BarAction::Mask);
        assert!(host.pending_commands().is_empty());
    }

    #[test]
    fn self_heal_sends_one_command_per_object() {
        use crate::game::testworld::{ARCHER, MEDIC, PILGRIM, host, spawn};
        let mut host = host();
        let first = spawn(&mut host, PILGRIM, 0, (10, 10));
        let medic = spawn(&mut host, MEDIC, 0, (12, 10));
        let second = spawn(&mut host, PILGRIM, 0, (14, 10));
        let archer = spawn(&mut host, ARCHER, 0, (16, 10));
        let foreign = spawn(&mut host, PILGRIM, 1, (30, 10));
        let mut bar = UnitBar::default();
        apply(
            &mut bar,
            &mut host,
            &[first, medic, foreign, second, archer],
            BarAction::SelfHeal,
        );
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(
            sent,
            [
                CommandKind::SelfHeal {
                    unit: first,
                    button: 555
                },
                CommandKind::SelfHeal {
                    unit: second,
                    button: 555
                },
            ]
        );
        // The command takes effect: hit points full, mana reduced by the costs (50).
        let set = |host: &mut SimHost, id: ObjectId, attr: u32, value: i32| {
            let mut sim = host.sim_context();
            let o = sim.object(id).unwrap();
            let pct = sim.type_upgrade_percent(o.owner(), o.guid, attr);
            sim.object_mut(id)
                .unwrap()
                .attributes
                .set_current_value(attr, value, pct);
        };
        let get =
            |host: &mut SimHost, id: ObjectId, attr: u32| host.sim_context().attribute(id, attr);
        host.step_once();
        set(&mut host, first, 14, 10);
        set(&mut host, first, 15, 120);
        host.submit_own(CommandKind::SelfHeal {
            unit: first,
            button: SELF_HEAL_BUTTON,
        });
        host.step_once();
        assert_eq!(get(&mut host, first, 14), 1000);
        assert_eq!(get(&mut host, first, 15), 70);
    }

    #[test]
    fn the_ability_bar_leaves_the_checksum_unchanged() {
        use crate::game::testworld::{
            ARCHER, ASSASSIN, MASK_UPGRADE, MEDIC, PILGRIM, host, spawn, unlock,
        };
        let mut host = host();
        unlock(&mut host, 0, MASK_UPGRADE);
        let ids = [ARCHER, ASSASSIN, PILGRIM, MEDIC]
            .iter()
            .enumerate()
            .map(|(k, &g)| spawn(&mut host, g, 0, (10 + 2 * k as i32, 10)))
            .collect::<Vec<_>>();
        let before = host.checksum();
        let state = nml_sim::save::checksum(&host.state);
        let texts = BTreeMap::new();
        for _ in 0..3 {
            let summary = summarize(&mut host, &ids);
            assert!(!buttons(&UnitBar::default(), &texts, &summary).is_empty());
        }
        assert_eq!(nml_sim::save::checksum(&host.state), state);
        assert_eq!(host.checksum(), before);
        // The click changes the simulation only via the command (at the next tick), not at the click.
        let mut bar = UnitBar::default();
        apply(&mut bar, &mut host, &ids, BarAction::SelfHeal);
        assert_eq!(nml_sim::save::checksum(&host.state), state);
    }

    #[test]
    fn the_original_texts_name_the_ability_buttons() {
        let wanted = [434, 435, 437, 438, 458, 459, 554, 555, 564];
        for (id, _) in STEALTH_HEAL_BUTTONS {
            assert!(wanted.contains(&id), "{id}");
        }
        assert_eq!(STEALTH_HEAL_BUTTONS.len(), wanted.len());
        // Fallback labels if the text is missing; the text from `guids.deu` takes precedence.
        let mut host = crate::game::testworld::host();
        let medic =
            crate::game::testworld::spawn(&mut host, crate::game::testworld::MEDIC, 0, (10, 10));
        let summary = summarize(&mut host, &[medic]);
        let plain = buttons(&UnitBar::default(), &BTreeMap::new(), &summary);
        assert_eq!(button(&plain, 458).unwrap().label, "Start healing");
        let named: BTreeMap<i32, String> = [(458, "Heal".to_string())].into_iter().collect();
        let texts = buttons(&UnitBar::default(), &named, &summary);
        assert_eq!(button(&texts, 458).unwrap().label, "Heal");
    }

    // --- Evoke ghost, banish, evoke animal (re/ghosts-banish 2.5/2.6, re/evoke-aura 2.4) ---

    #[test]
    fn from_button_maps_the_evocation_buttons() {
        assert_eq!(BarAction::from_button(436), Some(BarAction::EvokeGhost));
        for (art, button) in [441, 442, 443].into_iter().enumerate() {
            assert_eq!(
                BarAction::from_button(button),
                Some(BarAction::EvokeAnimal(art as u8)),
                "{button}"
            );
        }
        for button in [439, 547, 550, 553, 558, 562] {
            assert_eq!(
                BarAction::from_button(button),
                Some(BarAction::Banish(button)),
                "{button}"
            );
        }
        // 440 is only the target filter of banishing, 546 is a target button; the abilities 543 ff.
        // stay `Ability`.
        assert_eq!(BarAction::from_button(440), None);
        assert_eq!(BarAction::from_button(546), Some(BarAction::Target(546)));
        assert_eq!(
            BarAction::from_button(549),
            Some(BarAction::Ability(549)),
            "549 stays an ability"
        );
        // The list of the bar covers exactly the buttons of the actions, ascending.
        let listed: Vec<i32> = EVOCATION_BUTTONS.iter().map(|b| b.0).collect();
        assert_eq!(listed, [436, 439, 441, 442, 443, 547, 550, 553, 558, 562]);
    }

    /// The buttons of the evocations for `selection`: `(button, clickable)` in bar order.
    fn evocation_bar(host: &mut SimHost, selection: &[ObjectId]) -> Vec<(i32, bool)> {
        let summary = summarize(host, selection);
        buttons(&UnitBar::default(), &BTreeMap::new(), &summary)
            .into_iter()
            .filter(|b| EVOCATION_BUTTONS.iter().any(|a| a.0 == b.id))
            .map(|b| (b.id, b.enabled))
            .collect()
    }

    #[test]
    fn evocation_buttons_follow_the_first_selected_object() {
        use crate::game::testworld::{
            ANIMAL_EVOKER, BANISH_GHOSTS_UPGRADE, EVOKE_BEAR_UPGRADE, EVOKE_CAT_UPGRADE,
            EVOKE_GHOST_UPGRADE, EVOKE_WOLF_UPGRADE, GHOST_EVOKER, MISSIONARY,
            MISSIONARY_BANISH_UPGRADE, PILGRIM, WOLF, host, spawn, unlock,
        };
        let mut host = host();
        let evoker = spawn(&mut host, GHOST_EVOKER, 0, (10, 10));
        let animals = spawn(&mut host, ANIMAL_EVOKER, 0, (12, 10));
        let foreign = spawn(&mut host, GHOST_EVOKER, 1, (30, 10));
        // Without the research the buttons are shown gray in the bar (`REQUIRED_GUID`).
        assert_eq!(
            evocation_bar(&mut host, &[evoker]),
            [(436, false), (547, false)]
        );
        // Without 2930 only 436 is gray, 547 needs 2931.
        unlock(&mut host, 0, BANISH_GHOSTS_UPGRADE);
        assert_eq!(
            evocation_bar(&mut host, &[evoker]),
            [(436, false), (547, true)]
        );
        unlock(&mut host, 0, EVOKE_GHOST_UPGRADE);
        assert_eq!(
            evocation_bar(&mut host, &[evoker]),
            [(436, true), (547, true)]
        );
        // The first of the selection decides: 1411 first -> 436 and 547, no animals; the other way round only 441-443.
        assert_eq!(
            evocation_bar(&mut host, &[evoker, animals]),
            [(436, true), (547, true)]
        );
        assert_eq!(
            evocation_bar(&mut host, &[animals, evoker]),
            [(441, false), (442, false), (443, false)]
        );
        for upgrade in [EVOKE_WOLF_UPGRADE, EVOKE_BEAR_UPGRADE, EVOKE_CAT_UPGRADE] {
            unlock(&mut host, 0, upgrade);
        }
        assert_eq!(
            evocation_bar(&mut host, &[animals, evoker]),
            [(441, true), (442, true), (443, true)]
        );
        // Foreign object in front: nothing, even if an own one stands behind it.
        assert!(evocation_bar(&mut host, &[foreign, evoker]).is_empty());
        // Swimming (MY filter `SWIMMING`): 436 and 441-443 hidden, 547 stays.
        host.state.object_mut(evoker, false).unwrap().status |= status::SWIMMING;
        host.state.object_mut(animals, false).unwrap().status |= status::SWIMMING;
        assert_eq!(evocation_bar(&mut host, &[evoker]), [(547, true)]);
        assert!(evocation_bar(&mut host, &[animals]).is_empty());
        // Missionary: banish 439 gray without 2827, then clickable; pilgrim and wolf have no button of the group
        // (the pilgrim of the test world has no banish, its healing-group behavior stays).
        let missionary = spawn(&mut host, MISSIONARY, 0, (14, 10));
        assert_eq!(evocation_bar(&mut host, &[missionary]), [(439, false)]);
        unlock(&mut host, 0, MISSIONARY_BANISH_UPGRADE);
        assert_eq!(evocation_bar(&mut host, &[missionary]), [(439, true)]);
        let pilgrim = spawn(&mut host, PILGRIM, 0, (16, 10));
        let wolf = spawn(&mut host, WOLF, 0, (18, 10));
        assert!(evocation_bar(&mut host, &[pilgrim]).is_empty());
        assert!(evocation_bar(&mut host, &[wolf]).is_empty());
        // The group stands behind the camouflage and healing buttons (554/555 of the pilgrim before 439).
        let summary = summarize(&mut host, &[pilgrim, missionary]);
        assert_eq!(
            ids(&buttons(&UnitBar::default(), &BTreeMap::new(), &summary))[6..],
            [554, 555]
        );
        let summary = summarize(&mut host, &[missionary, pilgrim]);
        let all = ids(&buttons(&UnitBar::default(), &BTreeMap::new(), &summary));
        assert_eq!(
            all[6..],
            [554, 555, 439],
            "evocation behind the camouflage and healing buttons"
        );
    }

    /// Commands that `host` expects (`pending_commands`).
    fn pending(host: &SimHost) -> Vec<CommandKind> {
        host.pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect()
    }

    #[test]
    fn evocation_sends_one_command_for_the_first_object() {
        use crate::game::testworld::{
            ANIMAL_EVOKER, BANISH_GHOSTS_UPGRADE, EVOKE_BEAR_UPGRADE, EVOKE_GHOST_UPGRADE,
            GHOST_EVOKER, MISSIONARY, MISSIONARY_BANISH_UPGRADE, host, spawn, unlock,
        };
        let mut host = host();
        for upgrade in [
            EVOKE_GHOST_UPGRADE,
            BANISH_GHOSTS_UPGRADE,
            EVOKE_BEAR_UPGRADE,
            MISSIONARY_BANISH_UPGRADE,
        ] {
            unlock(&mut host, 0, upgrade);
        }
        let first = spawn(&mut host, GHOST_EVOKER, 0, (10, 10));
        let second = spawn(&mut host, GHOST_EVOKER, 0, (12, 10));
        let animals = spawn(&mut host, ANIMAL_EVOKER, 0, (14, 10));
        let missionary = spawn(&mut host, MISSIONARY, 0, (16, 10));
        let foreign = spawn(&mut host, GHOST_EVOKER, 1, (30, 10));
        let mut bar = UnitBar::default();
        let mut send = |host: &mut SimHost, sel: &[ObjectId], action: BarAction| {
            apply(&mut bar, host, sel, action);
            let out = pending(host);
            host.step_once();
            out
        };
        // Two ghost evokers selected: one command, only for the first.
        assert_eq!(
            send(&mut host, &[first, second], BarAction::EvokeGhost),
            [CommandKind::EvokeGhost {
                unit: first,
                button: 436
            }]
        );
        // Banish at the ghost evoker (547) and at the missionary (439) carries exactly this button.
        assert_eq!(
            send(&mut host, &[second, first], BarAction::Banish(547)),
            [CommandKind::Banish {
                unit: second,
                button: 547
            }]
        );
        assert_eq!(
            send(&mut host, &[missionary], BarAction::Banish(439)),
            [CommandKind::Banish {
                unit: missionary,
                button: 439
            }]
        );
        // Evoke animal carries kind and button of the kind.
        assert_eq!(
            send(&mut host, &[animals, first], BarAction::EvokeAnimal(1)),
            [CommandKind::EvokeAnimal {
                unit: animals,
                art: 1,
                button: 442
            }]
        );
        // No command if the first object does not show the button: wrong type, foreign, other
        // banish button (553 is not in the menu), swimming (MY filter), kind outside 0-2.
        assert!(send(&mut host, &[animals, first], BarAction::EvokeGhost).is_empty());
        assert!(send(&mut host, &[first, animals], BarAction::EvokeAnimal(0)).is_empty());
        assert!(send(&mut host, &[foreign, first], BarAction::EvokeGhost).is_empty());
        assert!(send(&mut host, &[first], BarAction::Banish(553)).is_empty());
        assert!(send(&mut host, &[missionary], BarAction::Banish(547)).is_empty());
        assert!(send(&mut host, &[first], BarAction::EvokeAnimal(3)).is_empty());
        host.state.object_mut(first, false).unwrap().status |= status::SWIMMING;
        assert!(send(&mut host, &[first, second], BarAction::EvokeGhost).is_empty());
        // The commands take effect on the simulation: banish costs mana (200 -> 175 at the missionary).
        let mana = |host: &mut SimHost, id: ObjectId| host.sim_context().attribute(id, 15);
        let before = mana(&mut host, missionary);
        apply(&mut bar, &mut host, &[missionary], BarAction::Banish(439));
        host.step_once();
        host.step_once();
        assert_eq!(before - mana(&mut host, missionary), 25, "cost 25 paid");
    }

    #[test]
    fn the_evocation_bar_leaves_the_checksum_unchanged() {
        use crate::game::testworld::{
            ANIMAL_EVOKER, EVOKE_GHOST_UPGRADE, GHOST_EVOKER, MISSIONARY, host, spawn, unlock,
        };
        let mut host = host();
        unlock(&mut host, 0, EVOKE_GHOST_UPGRADE);
        let ids = [GHOST_EVOKER, ANIMAL_EVOKER, MISSIONARY]
            .iter()
            .enumerate()
            .map(|(k, &g)| spawn(&mut host, g, 0, (10 + 2 * k as i32, 10)))
            .collect::<Vec<_>>();
        let before = host.checksum();
        let state = nml_sim::save::checksum(&host.state);
        let texts = BTreeMap::new();
        for lead in 0..ids.len() {
            let mut sel = ids.clone();
            sel.rotate_left(lead);
            let summary = summarize(&mut host, &sel);
            assert!(
                buttons(&UnitBar::default(), &texts, &summary)
                    .iter()
                    .any(|b| EVOCATION_BUTTONS.iter().any(|a| a.0 == b.id))
            );
        }
        assert_eq!(nml_sim::save::checksum(&host.state), state);
        assert_eq!(host.checksum(), before);
        // The click changes the simulation only via the command (at the next tick), not at the click.
        let mut bar = UnitBar::default();
        apply(&mut bar, &mut host, &ids, BarAction::EvokeGhost);
        assert_eq!(nml_sim::save::checksum(&host.state), state);
        assert_eq!(pending(&host).len(), 1);
    }

    #[test]
    fn the_viewer_world_carries_the_evocation_data() {
        use crate::game::testworld::{
            GHOST, GHOST_EVOKER, MISSIONARY, WOLF, WOLF_AURA, host, spawn,
        };
        let mut host = host();
        let rules = host.rules.clone();
        // Filters (441-443 with MY filter) and aura.
        for button in [436, 440, 441, 442, 443] {
            assert!(rules.target_entry(button).is_some(), "entry {button}");
        }
        assert!(rules.temp_upgrades.contains_key(&WOLF_AURA));
        assert!(rules.upgrade_types[&WOLF_AURA].contains(&GHOST_EVOKER));
        // Missionary gets hit points and mana, otherwise it could never banish.
        let missionary = spawn(&mut host, MISSIONARY, 0, (10, 10));
        let sim = host.sim_context();
        assert_eq!(sim.attribute(missionary, 14), 1000);
        assert_eq!(sim.attribute(missionary, 15), 200);
        assert_eq!(sim.attribute(missionary, 18), 40);
        // Wolf: modules 0, 2, 16, 17, 18, 20, 21, 44 (without dying module 16 there would be no death), without
        // banish 31.
        let wolf = spawn(&mut host, WOLF, 0, (12, 10));
        let o = host.state.object(wolf, false).unwrap();
        for index in [0, 2, 16, 17, 18, 20, 21, 44] {
            assert!(o.has_module(index, false), "module {index} of the wolf");
        }
        assert!(!o.has_module(31, true));
        let ghost = spawn(&mut host, GHOST, 0, (14, 10));
        assert!(
            host.state
                .object(ghost, false)
                .unwrap()
                .has_module(27, false)
        );
    }

    // --- Spells and special attacks (re/spells 2.5, re/special-attacks 2.4) ---

    #[test]
    fn from_button_maps_the_spell_and_special_buttons() {
        assert_eq!(BarAction::from_button(482), Some(BarAction::Reincarnate));
        assert_eq!(BarAction::from_button(542), Some(BarAction::Whirlwind));
        for button in [454, 455, 456, 457, 480, 481, 484, 541, 546, 548, 551, 556] {
            assert_eq!(
                BarAction::from_button(button),
                Some(BarAction::Target(button)),
                "{button}"
            );
        }
        // 483 has no branch (re/spells 2.5), 543 ff. stay abilities.
        assert_eq!(BarAction::from_button(483), None);
        assert_eq!(BarAction::from_button(543), Some(BarAction::Ability(543)));
        let listed: Vec<i32> = SPELL_SPECIAL_BUTTONS.iter().map(|b| b.0).collect();
        assert_eq!(
            listed,
            [
                480, 481, 482, 484, 541, 542, 454, 455, 456, 457, 546, 548, 551, 556
            ]
        );
    }

    /// The buttons of the spells and special attacks for `selection`: `(button, clickable)` in
    /// bar order.
    fn spell_bar(host: &mut SimHost, selection: &[ObjectId]) -> Vec<(i32, bool)> {
        let summary = summarize(host, selection);
        buttons(&UnitBar::default(), &BTreeMap::new(), &summary)
            .into_iter()
            .filter(|b| SPELL_SPECIAL_BUTTONS.iter().any(|a| a.0 == b.id))
            .map(|b| (b.id, b.enabled))
            .collect()
    }

    #[test]
    fn spell_buttons_follow_the_first_selected_object() {
        use crate::game::testworld::{
            EXPLOSION_UPGRADE, MEDICINE_MAN, REINCARNATION_UPGRADE, SHARPSHOOTER, SMITH,
            SNIPE_UPGRADE, STORM_UPGRADE, WHIRLWIND_UPGRADE, host, spawn, unlock,
        };
        let mut host = host();
        let medicine = spawn(&mut host, MEDICINE_MAN, 0, (10, 10));
        let smith = spawn(&mut host, SMITH, 0, (12, 10));
        let foreign = spawn(&mut host, MEDICINE_MAN, 1, (30, 10));
        // Without research gray (`REQUIRED_GUID`).
        assert_eq!(
            spell_bar(&mut host, &[medicine, smith]),
            [(480, false), (481, false), (482, false)]
        );
        // Without 3024 480 stays gray.
        unlock(&mut host, 0, STORM_UPGRADE);
        unlock(&mut host, 0, REINCARNATION_UPGRADE);
        assert_eq!(
            spell_bar(&mut host, &[medicine, smith]),
            [(480, false), (481, true), (482, true)]
        );
        unlock(&mut host, 0, EXPLOSION_UPGRADE);
        assert_eq!(
            spell_bar(&mut host, &[medicine, smith]),
            [(480, true), (481, true), (482, true)],
            "1716 first: 480-482, not 542/551 of the smith"
        );
        // Smith first: only 542 and 551 (the DO buttons also follow the first object).
        unlock(&mut host, 0, WHIRLWIND_UPGRADE);
        assert_eq!(
            spell_bar(&mut host, &[smith, medicine]),
            [(542, true), (551, false)]
        );
        // Foreign object in front: nothing.
        assert!(spell_bar(&mut host, &[foreign, medicine]).is_empty());
        // Swimming sharpshooter: 457 hidden (MY filter not `SWIMMING`).
        unlock(&mut host, 0, SNIPE_UPGRADE);
        let sharpshooter = spawn(&mut host, SHARPSHOOTER, 0, (14, 10));
        assert_eq!(spell_bar(&mut host, &[sharpshooter]), [(457, true)]);
        host.state.object_mut(sharpshooter, false).unwrap().status |= status::SWIMMING;
        assert!(spell_bar(&mut host, &[sharpshooter]).is_empty());
        // The group stands behind the evocations (fallback label without `guids.deu`).
        let summary = summarize(&mut host, &[medicine]);
        let all = buttons(&UnitBar::default(), &BTreeMap::new(), &summary);
        assert_eq!(button(&all, 480).unwrap().label, "Explosion spell");
        assert_eq!(button(&all, 482).unwrap().label, "Reincarnation");
    }

    #[test]
    fn the_target_mode_sends_one_command_for_the_first_object() {
        use crate::game::testworld::{
            CHIEF, COLONIST, EXPLOSION_UPGRADE, MEDICINE_MAN, SCOUT_UPGRADE, host, spawn, unlock,
        };
        let mut host = host();
        unlock(&mut host, 0, EXPLOSION_UPGRADE);
        unlock(&mut host, 0, SCOUT_UPGRADE);
        let first = spawn(&mut host, MEDICINE_MAN, 0, (10, 10));
        let second = spawn(&mut host, MEDICINE_MAN, 0, (12, 10));
        let chief = spawn(&mut host, CHIEF, 0, (14, 10));
        let enemy = spawn(&mut host, COLONIST, 1, (20, 12));
        let mut bar = UnitBar::default();
        // Button 480: targeting mode for the first selected object, no command yet.
        apply(
            &mut bar,
            &mut host,
            &[first, second],
            BarAction::Target(480),
        );
        assert_eq!(
            bar.target_mode,
            Some(TargetMode {
                unit: first,
                button: 480
            })
        );
        assert!(pending(&host).is_empty());
        assert_eq!(
            target_mode_line(&bar.target_mode.unwrap(), &BTreeMap::new()),
            "Select target: Explosion spell"
        );
        // Click on the enemy colonist: exactly one `CastSpell` kind 1 for the first object with
        // the `f32` bits of the target position (tile center (20, 12)).
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        assert_eq!(
            click_target(&mut bar, &mut host, AbilityTarget::Object(enemy)),
            Ok(())
        );
        assert_eq!(
            pending(&host),
            [CommandKind::CastSpell {
                caster: first,
                art: 1,
                anim: at(20).to_bits() as i32,
                idx: at(12).to_bits() as i32,
                button: 480,
            }]
        );
        assert_eq!(bar.target_mode, None);
        host.step_once();
        // 484 at the chief, ground click -> `ScoutArea` with the tile (K-313: only the first object).
        apply(&mut bar, &mut host, &[chief, first], BarAction::Target(484));
        let ground = AbilityTarget::Ground {
            x: 20.0 * 128.0 + 10.0,
            z: 30.0 * 128.0 + 100.0,
        };
        assert_eq!(click_target(&mut bar, &mut host, ground), Ok(()));
        assert_eq!(
            pending(&host),
            [CommandKind::ScoutArea {
                unit: chief,
                x: 20,
                z: 30,
                size: 30,
                duration_ms: 5000,
                flag: 1,
                button: 484,
            }]
        );
        assert_eq!(bar.target_mode, None);
        host.step_once();
        // Gray button (481 without 3025) or foreign first: no targeting mode.
        apply(&mut bar, &mut host, &[first], BarAction::Target(481));
        apply(&mut bar, &mut host, &[enemy, first], BarAction::Target(480));
        assert_eq!(bar.target_mode, None);
        // Without a targeting mode no command.
        assert!(click_target(&mut bar, &mut host, AbilityTarget::Object(enemy)).is_err());
        assert!(pending(&host).is_empty());
        // Target from the click: 484/556 take the ground even over an object (re/spells 2.5,
        // right-click on the ground), the others the object under the pointer, otherwise the ground.
        let g = Vec3::new(100.0, 5.0, 200.0);
        let floor = AbilityTarget::Ground { x: 100.0, z: 200.0 };
        assert_eq!(ability_target(484, Some(enemy), Some(g)), Some(floor));
        assert_eq!(
            ability_target(480, Some(enemy), Some(g)),
            Some(AbilityTarget::Object(enemy))
        );
        assert_eq!(ability_target(480, None, Some(g)), Some(floor));
        assert_eq!(ability_target(556, Some(enemy), None), None);
    }

    #[test]
    fn the_target_mode_rejects_unfit_targets() {
        use crate::game::testworld::{
            COLONIST, EXPLOSION_UPGRADE, MEDICINE_MAN, TRAPPER, TRAPPER_KILL_UPGRADE, host, spawn,
            unlock,
        };
        let mut host = host();
        unlock(&mut host, 0, TRAPPER_KILL_UPGRADE);
        unlock(&mut host, 0, EXPLOSION_UPGRADE);
        let trapper = spawn(&mut host, TRAPPER, 0, (10, 10));
        let own = spawn(&mut host, COLONIST, 0, (12, 10));
        let medicine = spawn(&mut host, MEDICINE_MAN, 0, (10, 20));
        // Sight 52 -> range 17 tiles (`MM_METHODINDEX 3`).
        let far = spawn(&mut host, COLONIST, 1, (40, 20));
        let near = spawn(&mut host, COLONIST, 1, (20, 20));
        host.step_once();
        let before = host.checksum();
        let state = nml_sim::save::checksum(&host.state);
        let mut bar = UnitBar::default();
        // 456 at the trapper on the own unit: rejected, the targeting mode stays, no command.
        apply(&mut bar, &mut host, &[trapper], BarAction::Target(456));
        let mode = bar.target_mode;
        assert!(mode.is_some());
        let err = click_target(&mut bar, &mut host, AbilityTarget::Object(own)).unwrap_err();
        assert!(err.contains("target filter"), "{err}");
        // Ground point for an object button: also rejected.
        let ground = AbilityTarget::Ground {
            x: 1000.0,
            z: 1000.0,
        };
        assert!(click_target(&mut bar, &mut host, ground).is_err());
        assert_eq!(bar.target_mode, mode);
        // 480 at an enemy beyond sight/3 (30 tiles): rejected; within: accepted.
        apply(&mut bar, &mut host, &[medicine], BarAction::Target(480));
        assert!(click_target(&mut bar, &mut host, AbilityTarget::Object(far)).is_err());
        assert_eq!(
            bar.target_mode,
            Some(TargetMode {
                unit: medicine,
                button: 480
            })
        );
        assert!(pending(&host).is_empty());
        assert_eq!(nml_sim::save::checksum(&host.state), state);
        assert_eq!(host.checksum(), before);
        assert_eq!(
            click_target(&mut bar, &mut host, AbilityTarget::Object(near)),
            Ok(())
        );
        assert_eq!(pending(&host).len(), 1);
        // `issue_target` without a targeting mode (control channel): button without a target command.
        assert!(issue_target(&mut host, medicine, 482, AbilityTarget::Object(near)).is_err());
    }

    #[test]
    fn the_target_mode_ends_on_cancel() {
        use crate::game::testworld::{EXPLOSION_UPGRADE, MEDICINE_MAN, SMITH, host, spawn, unlock};
        let mut host = host();
        unlock(&mut host, 0, EXPLOSION_UPGRADE);
        let first = spawn(&mut host, MEDICINE_MAN, 0, (10, 10));
        let second = spawn(&mut host, MEDICINE_MAN, 0, (12, 10));
        let smith = spawn(&mut host, SMITH, 0, (14, 10));
        let mut bar = UnitBar::default();
        let start = |bar: &mut UnitBar, host: &mut SimHost| {
            apply(bar, host, &[first, second], BarAction::Target(480));
            assert!(bar.target_mode.is_some());
        };
        // Esc and right-click (viewer, `apps/nml-viewer/src/app.rs`) call `cancel_target_mode`.
        start(&mut bar, &mut host);
        cancel_target_mode(&mut bar);
        assert_eq!(bar.target_mode, None);
        // Per frame `follow_target_mode`: unchanged -> stays (also with another rest of the selection).
        start(&mut bar, &mut host);
        follow_target_mode(&mut bar, &mut host, &[first, second]);
        follow_target_mode(&mut bar, &mut host, &[first, smith]);
        assert!(bar.target_mode.is_some());
        // Selection changed (other first, empty) -> end.
        follow_target_mode(&mut bar, &mut host, &[second, first]);
        assert_eq!(bar.target_mode, None);
        start(&mut bar, &mut host);
        follow_target_mode(&mut bar, &mut host, &[]);
        assert_eq!(bar.target_mode, None);
        // Button locked (research gone) -> end.
        start(&mut bar, &mut host);
        host.state.add_guid_count(0, EXPLOSION_UPGRADE, -1);
        follow_target_mode(&mut bar, &mut host, &[first, second]);
        assert_eq!(bar.target_mode, None);
        host.state.add_guid_count(0, EXPLOSION_UPGRADE, 1);
        // Caster dead -> end.
        start(&mut bar, &mut host);
        host.state.object_mut(first, false).unwrap().status |= status::DIED;
        follow_target_mode(&mut bar, &mut host, &[first, second]);
        assert_eq!(bar.target_mode, None);
        // The targeting mode ends the unpack cursor; 453 ends the targeting mode.
        bar.tent_cursor = Some(TentCursor {
            unit: smith,
            tent_guid: 1601,
            rotation_bits: 0,
        });
        apply(&mut bar, &mut host, &[second], BarAction::Target(480));
        assert!(bar.target_mode.is_some());
        assert_eq!(bar.tent_cursor, None);
    }

    #[test]
    fn reincarnation_and_whirlwind_go_to_every_selected_object() {
        use crate::game::testworld::{
            COLONIST, MEDICINE_MAN, REINCARNATION_UPGRADE, SMITH, WHIRLWIND_UPGRADE, host, spawn,
            unlock,
        };
        let mut host = host();
        unlock(&mut host, 0, REINCARNATION_UPGRADE);
        unlock(&mut host, 0, WHIRLWIND_UPGRADE);
        let m1 = spawn(&mut host, MEDICINE_MAN, 0, (10, 10));
        let m2 = spawn(&mut host, MEDICINE_MAN, 0, (12, 10));
        let s1 = spawn(&mut host, SMITH, 0, (14, 10));
        let s2 = spawn(&mut host, SMITH, 0, (16, 10));
        let plain = spawn(&mut host, COLONIST, 0, (18, 10));
        let foreign = spawn(&mut host, MEDICINE_MAN, 1, (30, 10));
        let mut bar = UnitBar::default();
        let mut send = |host: &mut SimHost, sel: &[ObjectId], action: BarAction| {
            apply(&mut bar, host, sel, action);
            let out = pending(host);
            host.step_once();
            out
        };
        let reincarnate = |caster| CommandKind::CastSpell {
            caster,
            art: 3,
            anim: 0,
            idx: 0,
            button: 482,
        };
        let whirlwind = |unit| CommandKind::Whirlwind { unit, button: 542 };
        // One command per selected own object with the button; the unit without the module and the
        // foreign object get none.
        assert_eq!(
            send(
                &mut host,
                &[m1, plain, m2, foreign, s1],
                BarAction::Reincarnate
            ),
            [reincarnate(m1), reincarnate(m2)]
        );
        assert_eq!(
            send(&mut host, &[s1, plain, s2, m1], BarAction::Whirlwind),
            [whirlwind(s1), whirlwind(s2)]
        );
        assert!(send(&mut host, &[plain], BarAction::Reincarnate).is_empty());
        assert!(send(&mut host, &[m1], BarAction::Whirlwind).is_empty());
    }

    #[test]
    fn the_spell_bar_leaves_the_checksum_unchanged() {
        use crate::game::testworld::{
            CHIEF, COLONIST, EXPLOSION_UPGRADE, MEDICINE_MAN, PATRIOT, SHADOW_WARRIOR,
            SHARPSHOOTER, SMITH, TRAPPER, host, spawn, unlock,
        };
        let mut host = host();
        unlock(&mut host, 0, EXPLOSION_UPGRADE);
        let ids = [
            MEDICINE_MAN,
            CHIEF,
            TRAPPER,
            SHARPSHOOTER,
            PATRIOT,
            SHADOW_WARRIOR,
            SMITH,
        ]
        .iter()
        .enumerate()
        .map(|(k, &g)| spawn(&mut host, g, 0, (10 + 2 * k as i32, 10)))
        .collect::<Vec<_>>();
        let enemy = spawn(&mut host, COLONIST, 1, (12, 14));
        host.step_once();
        let before = host.checksum();
        let state = nml_sim::save::checksum(&host.state);
        let texts = BTreeMap::new();
        for lead in 0..ids.len() {
            let mut sel = ids.clone();
            sel.rotate_left(lead);
            let summary = summarize(&mut host, &sel);
            assert!(
                buttons(&UnitBar::default(), &texts, &summary)
                    .iter()
                    .any(|b| SPELL_SPECIAL_BUTTONS.iter().any(|a| a.0 == b.id)),
                "{lead}"
            );
        }
        // Targeting mode, target check and sync only read; the click changes the simulation only via
        // the command (at the next tick).
        let mut bar = UnitBar::default();
        apply(&mut bar, &mut host, &ids, BarAction::Target(480));
        follow_target_mode(&mut bar, &mut host, &ids);
        assert!(click_target(&mut bar, &mut host, AbilityTarget::Object(ids[1])).is_err());
        assert_eq!(nml_sim::save::checksum(&host.state), state);
        assert_eq!(host.checksum(), before);
        assert_eq!(
            click_target(&mut bar, &mut host, AbilityTarget::Object(enemy)),
            Ok(())
        );
        apply(&mut bar, &mut host, &ids, BarAction::Reincarnate);
        apply(&mut bar, &mut host, &ids, BarAction::Whirlwind);
        assert_eq!(nml_sim::save::checksum(&host.state), state);
        assert_eq!(pending(&host).len(), 3);
    }

    #[test]
    fn right_clicks_carry_the_formation_and_the_drag_inside_the_queue_wrapper() {
        use nml_sim::command::Command;
        let (mut host, units) = two_colonists();
        let bar = UnitBar {
            formation: 2,
            ..UnitBar::default()
        };
        let order = FormationOrder::new(&bar, Some((90 << 12, 5734)));
        let ground = glam::Vec3::new(20.0 * 128.0 + 64.0, 0.0, 20.0 * 128.0 + 64.0);
        let (_, kind) = host
            .issue_target_action_keys(&units, None, ground, Some(false), false, false, order)
            .expect("Go to");
        let CommandKind::Queue { inner, .. } = &kind else {
            panic!("queue wrapper expected: {kind:?}");
        };
        assert!(matches!(
            **inner,
            CommandKind::Move {
                formation: 2,
                direction: 368_640,
                spread: 5734,
                ..
            }
        ));
        // Without a drag, direction and spread stay "none", the formation is in the command anyway.
        let order = FormationOrder::new(&bar, None);
        let (_, kind) = host
            .issue_target_action_keys(&units, None, ground, None, false, false, order)
            .expect("Go to");
        assert!(matches!(
            kind,
            CommandKind::Move {
                formation: 2,
                direction: NO_DIRECTION,
                spread: NO_SPREAD,
                ..
            }
        ));
        // Attack march (427) carries it too, but no drag (`0x00453934`: flag `0x10` only with 448); the
        // stance before it stays unchanged.
        let dragged = FormationOrder::new(&bar, Some((90 << 12, 5734)));
        let kinds = host.issue_attack_action(&units, None, ground, None, dragged);
        assert!(matches!(kinds[0], CommandKind::SetBehavior { .. }));
        assert!(matches!(
            kinds[1],
            CommandKind::Move {
                formation: 2,
                flags: 4,
                direction: NO_DIRECTION,
                spread: NO_SPREAD,
                ..
            }
        ));
        // The control channel (`sim_move`) likewise; it is submitted for the active player.
        assert!(host.move_units(&units, ground, None, order));
        let last: &Command = host.pending_commands().last().unwrap();
        assert!(matches!(last.kind, CommandKind::Move { formation: 2, .. }));
    }

    // --- Arm 485/486, abilities 543 ff., counter-upgrades 3370-3377 ---

    fn button(buttons: &[BarButton], id: i32) -> Option<&BarButton> {
        buttons.iter().find(|b| b.id == id)
    }

    #[test]
    fn the_bar_offers_arming_by_state_and_upgrade() {
        use super::testkit::{COLONIST, host, set_armed, spawn};
        let mut host = host();
        let colonist = spawn(&mut host, COLONIST, 0, (10, 10));
        let texts = BTreeMap::new();
        let bar = UnitBar::default();
        let offered = |host: &mut SimHost| buttons(&bar, &texts, &summarize(host, &[colonist]));
        // Unarmed without 2806: 485 visible, but locked (`REQUIRED_GUID 2806`).
        let b = offered(&mut host);
        let arm = button(&b, ARM_BUTTON).expect("485 visible");
        assert!(!arm.enabled);
        assert_eq!(arm.label, "Arm");
        assert!(button(&b, DISARM_BUTTON).is_none());
        // With 2806 (object counter of the slot): free.
        host.state.add_guid_count(0, 2806, 1);
        assert!(button(&offered(&mut host), ARM_BUTTON).unwrap().enabled);
        // Armed (`ISARMED`): only 486.
        set_armed(&mut host, colonist, true);
        let b = offered(&mut host);
        assert!(button(&b, ARM_BUTTON).is_none());
        let disarm = button(&b, DISARM_BUTTON).expect("486 visible");
        assert!(disarm.enabled);
        assert_eq!(disarm.label, "Disarm");
        // The formation buttons stay free; foreign colonists give no button.
        assert!(
            b.iter()
                .filter(|b| b.id != DISARM_BUTTON)
                .all(|b| b.enabled)
        );
        let foreign = spawn(&mut host, COLONIST, 1, (20, 10));
        assert!(summarize(&mut host, &[foreign]).extra.is_empty());
        // The click: `ToggleArming` with the matching own units.
        let unarmed = spawn(&mut host, COLONIST, 0, (12, 10));
        let mut bar = UnitBar::default();
        let selection = [colonist, unarmed, foreign];
        apply(&mut bar, &mut host, &selection, BarAction::Disarm);
        apply(&mut bar, &mut host, &selection, BarAction::Arm);
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(
            sent,
            [
                CommandKind::ToggleArming {
                    units: vec![colonist],
                    arm: false
                },
                CommandKind::ToggleArming {
                    units: vec![unarmed],
                    arm: true
                },
            ]
        );
        assert_eq!(BarAction::from_button(485), Some(BarAction::Arm));
        assert_eq!(BarAction::from_button(486), Some(BarAction::Disarm));
    }

    #[test]
    fn ability_buttons_cast_one_spell_per_caster() {
        use super::testkit::{COLONIST, GOVERNOR, host, spawn};
        let mut host = host();
        let first = spawn(&mut host, GOVERNOR, 0, (10, 10));
        let second = spawn(&mut host, GOVERNOR, 0, (12, 10));
        let colonist = spawn(&mut host, COLONIST, 0, (14, 10));
        let selection = [first, second, colonist];
        // Buttons from the menu of the governor (order of `toolbar.ini`); 543 needs 2828.
        let b = summarize(&mut host, &selection).extra;
        let abilities: Vec<(i32, bool)> = b
            .iter()
            .filter(|b| (543..=563).contains(&b.id))
            .map(|b| (b.id, b.enabled))
            .collect();
        assert_eq!(abilities, [(544, true), (543, false), (545, true)]);
        host.state.add_guid_count(0, 2828, 1);
        assert!(
            button(&summarize(&mut host, &selection).extra, 543)
                .unwrap()
                .enabled
        );
        // Click on 543: one `CastSpell` per governor with kind 4 and (anim, idx) = (2, 1).
        let mut bar = UnitBar::default();
        apply(&mut bar, &mut host, &selection, BarAction::Ability(543));
        let cast = |caster| CommandKind::CastSpell {
            caster,
            art: 4,
            anim: 2,
            idx: 1,
            button: 543,
        };
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(sent, [cast(first), cast(second)]);
        // Every button from `ABILITY_BUTTONS` is an ability; 546 is a target button, not an ability
        // with kind 4.
        for &(id, _, _) in &ABILITY_BUTTONS {
            assert_eq!(BarAction::from_button(id), Some(BarAction::Ability(id)));
        }
        assert_eq!(BarAction::from_button(546), Some(BarAction::Target(546)));
    }

    #[test]
    fn counter_upgrade_buttons_lock_while_one_runs() {
        use super::testkit::{MISSIONARY, host, spawn};
        use nml_sim::upgrades::CounterUpgrade;
        let mut host = host();
        let missionary = spawn(&mut host, MISSIONARY, 0, (10, 10));
        let second = spawn(&mut host, MISSIONARY, 0, (12, 10));
        let counters = |host: &mut SimHost| -> Vec<(i32, bool)> {
            summarize(host, &[missionary, second])
                .extra
                .iter()
                .filter(|b| (3370..=3377).contains(&b.id))
                .map(|b| (b.id, b.enabled))
                .collect()
        };
        // Locked without 3354; directly as buttons, without submenu 601, only those of the
        // first unit.
        assert_eq!(counters(&mut host), [(3374, false), (3375, false)]);
        host.state.add_guid_count(0, 3354, 1);
        assert_eq!(counters(&mut host), [(3374, true), (3375, true)]);
        assert!(
            summarize(&mut host, &[missionary])
                .extra
                .iter()
                .all(|b| b.id != 601)
        );
        // While 3375 of the slot runs: both locked (`Toolbar_IsButtonLocked`, re/upgrades 2).
        host.state.counter_upgrades.push(CounterUpgrade {
            guid: 3375,
            rest_ms: 60_000,
            source: missionary,
        });
        assert_eq!(counters(&mut host), [(3374, false), (3375, false)]);
        // The click gives `CounterUpgrade` with the first unit as source.
        host.state.counter_upgrades.clear();
        let mut bar = UnitBar::default();
        apply(
            &mut bar,
            &mut host,
            &[missionary, second],
            BarAction::CounterUpgrade(3374),
        );
        assert_eq!(
            host.pending_commands().last().unwrap().kind,
            CommandKind::CounterUpgrade {
                guid: 3374,
                source: missionary
            }
        );
        for guid in 3370..=3377 {
            assert_eq!(
                BarAction::from_button(guid),
                Some(BarAction::CounterUpgrade(guid as u32))
            );
        }
        assert_eq!(BarAction::from_button(3378), None);
        // Texts from `guids.deu` instead of the fallback label.
        let texts: BTreeMap<i32, String> = [(3374, "Execute chastity belt".to_string())]
            .into_iter()
            .collect();
        let summary = summarize(&mut host, &[missionary]);
        let b = buttons(&UnitBar::default(), &texts, &summary);
        assert_eq!(button(&b, 3374).unwrap().label, "Execute chastity belt");
        assert_eq!(button(&b, 3375).unwrap().label, "Counter-upgrade 3375");
    }

    #[test]
    fn the_new_bar_buttons_leave_the_checksum_unchanged() {
        use super::testkit::{COLONIST, GOVERNOR, MISSIONARY, host, spawn};
        let mut host = host();
        let ids = [
            spawn(&mut host, COLONIST, 0, (10, 10)),
            spawn(&mut host, GOVERNOR, 0, (12, 10)),
            spawn(&mut host, MISSIONARY, 0, (14, 10)),
        ];
        host.step_once();
        let before = host.checksum();
        let state = nml_sim::save::checksum(&host.state);
        let summary = summarize(&mut host, &ids);
        assert!(!summary.extra.is_empty());
        assert_eq!(nml_sim::save::checksum(&host.state), state);
        assert_eq!(host.checksum(), before);
    }

    // --- Pack tent 452 and unpack tent 453 (re/tents 2.5, 3.4) ---

    #[test]
    fn tent_buttons_follow_the_leader() {
        use crate::game::testworld::{
            CHIEF_TENT, COLONIST, DWELLING_TENT, STORAGE_TENT, TRAVOIS, host, load_travois, spawn,
        };
        let mut host = host();
        let chief = spawn(&mut host, CHIEF_TENT, 0, (10, 10));
        let dwelling = spawn(&mut host, DWELLING_TENT, 0, (20, 10));
        let storage = spawn(&mut host, STORAGE_TENT, 0, (30, 10));
        let foreign = spawn(&mut host, DWELLING_TENT, 1, (40, 10));
        let travois = spawn(&mut host, TRAVOIS, 0, (10, 30));
        let colonist = spawn(&mut host, COLONIST, 0, (12, 30));
        let lost = spawn(&mut host, TRAVOIS, 0, (14, 30));
        let texts = BTreeMap::new();
        let bar = UnitBar::default();
        let offered = |host: &mut SimHost, selection: &[ObjectId]| {
            ids(&buttons(&bar, &texts, &summarize(host, selection)))
        };
        let formation = [511, 512, 513, 514, 515, 519];
        let with = |extra: &[i32]| [formation.as_slice(), extra].concat();
        // Tents with module 42 and 452 in the menu: only 452 (without a unit no formation row).
        assert_eq!(offered(&mut host, &[dwelling]), [452]);
        assert_eq!(offered(&mut host, &[chief]), [452]);
        // Storage camp 1603: 452 is in the menu, module 42 is missing -> no button (W-262; `targets.ini`
        // `MY_MM MM_CHECKPROPERTY PROP_TENT`).
        assert!(offered(&mut host, &[storage]).is_empty());
        // Foreign tent (look only): no button.
        assert!(offered(&mut host, &[foreign]).is_empty());
        // Hidden (unselectable, still selected) tent: no dead button.
        host.state.object_mut(dwelling, false).unwrap().selectable = false;
        assert!(offered(&mut host, &[dwelling]).is_empty());
        host.state.object_mut(dwelling, false).unwrap().selectable = true;
        // Travois without `TRANSPORTINGTENT`: only the formation row; with the flag and a tent 453.
        assert_eq!(offered(&mut host, &[travois]), formation);
        load_travois(&mut host, travois, dwelling);
        assert_eq!(offered(&mut host, &[travois]), with(&[453]));
        // The first of the selection decides: worker in front -> no 453.
        assert_eq!(offered(&mut host, &[travois, colonist]), with(&[453]));
        assert_eq!(offered(&mut host, &[colonist, travois]), formation);
        // Flag without a tent (destroyed, W-261: the flag stays): no 453.
        load_travois(&mut host, lost, ObjectId::NONE);
        assert_eq!(offered(&mut host, &[lost]), formation);
        // Fallback label and free.
        let b = buttons(&bar, &texts, &summarize(&mut host, &[travois]));
        let unpack = button(&b, UNPACK_TENT_BUTTON).unwrap();
        assert_eq!(
            (unpack.label.as_str(), unpack.enabled),
            ("Unpack tent", true)
        );
        let b = buttons(&bar, &texts, &summarize(&mut host, &[dwelling]));
        let pack = button(&b, PACK_TENT_BUTTON).unwrap();
        assert_eq!((pack.label.as_str(), pack.enabled), ("Pack tent", true));
    }

    #[test]
    fn the_pack_button_sends_pack_tent_for_the_first_object_only() {
        use crate::game::testworld::{CHIEF_TENT, DWELLING_TENT, STORAGE_TENT, host, spawn};
        let mut host = host();
        let first = spawn(&mut host, DWELLING_TENT, 0, (10, 10));
        let second = spawn(&mut host, CHIEF_TENT, 0, (20, 10));
        let storage = spawn(&mut host, STORAGE_TENT, 0, (30, 10));
        let foreign = spawn(&mut host, DWELLING_TENT, 1, (40, 10));
        let mut bar = UnitBar::default();
        // Two tents selected (unreachable via the UI, a multi-selection keeps only units): the bar
        // sends `len = 1`, only the first (re/tents 2.4).
        apply(&mut bar, &mut host, &[first, second], BarAction::PackTent);
        // Without a tent in front no command: storage camp without module 42, foreign tent.
        apply(&mut bar, &mut host, &[storage, second], BarAction::PackTent);
        apply(&mut bar, &mut host, &[foreign], BarAction::PackTent);
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(sent, [CommandKind::PackTent { tent: first }]);
        assert_eq!(bar, UnitBar::default());
        assert_eq!(BarAction::from_button(452), Some(BarAction::PackTent));
        assert_eq!(BarAction::from_button(453), Some(BarAction::UnpackTent));
        assert_eq!([PACK_TENT_BUTTON, UNPACK_TENT_BUTTON], [452, 453]);
    }

    #[test]
    fn the_unpack_cursor_keeps_the_tent_rotation_and_sends_grid_coordinates() {
        use crate::game::testworld::{
            COLONIST, DWELLING_TENT, TRAVOIS, host, load_travois, set_shape,
        };
        use std::f32::consts::FRAC_PI_2;
        let mut host = host();
        // Test hull of the dwelling tent: center 0, half extent 160 (x) x 64 (z). Rotated by pi/2
        // (`RotY`: x' = -z, z' = x) the minimum corner (-160, -64) becomes (64, -160); at the ground point
        // (1000, 2000) it lies at (1064, 1840) = tile (8.3125, 14.375). Snapped
        // (`GameWorld_SnapBoxMinCornerToGrid`) the position moves by 0.3125 or 0.375 tiles to
        // (960, 1952) = tile (7.5, 15.25) -> `Fixed_FromFloat` 30720 and 62464. Without the rotation
        // (928, 1984) would result (29696, 63488); the values 32000/64000 would be the raw ground point.
        set_shape(
            &mut host.rules,
            DWELLING_TENT,
            [0.0; 3],
            [160.0, 50.0, 64.0],
        );
        let tent = spawn(&mut host, DWELLING_TENT, 0, (40, 40));
        host.state.object_mut(tent, false).unwrap().rotation[1] = FRAC_PI_2;
        let travois = spawn(&mut host, TRAVOIS, 0, (4, 4));
        let colonist = spawn(&mut host, COLONIST, 0, (6, 4));
        load_travois(&mut host, travois, tent);
        let selection = [travois, colonist];
        let mut bar = UnitBar {
            formation: 2,
            ..UnitBar::default()
        };
        // 453 only starts the cursor, with GUID and rotation of the packed tent.
        apply(&mut bar, &mut host, &selection, BarAction::UnpackTent);
        let cursor = bar.tent_cursor.expect("unpack cursor");
        assert_eq!(
            cursor,
            TentCursor {
                unit: travois,
                tent_guid: DWELLING_TENT,
                rotation_bits: FRAC_PI_2.to_bits(),
            }
        );
        assert_eq!(cursor.rotation(), FRAC_PI_2);
        assert_eq!(bar.formation, 2);
        assert!(host.pending_commands().is_empty());
        // Preview like the build cursor, with the rotation of the tent; it leaves the checksum the same.
        let before = host.checksum();
        let ground = Vec3::new(1000.0, 0.0, 2000.0);
        let preview = tent_preview(&mut host, &cursor, ground, &selection).expect("preview");
        assert_eq!(host.checksum(), before);
        assert_eq!(preview.guid, DWELLING_TENT);
        assert_eq!(preview.rot, FRAC_PI_2);
        assert_eq!((preview.pos[0], preview.pos[2]), (960.0, 1952.0));
        assert!(preview.valid);
        assert_eq!(
            tent_cursor_line(&cursor, Some(&preview)),
            "Unpack tent: 1601, position (960.0, 1952.0), valid | Esc/right-click: end"
        );
        // Invalid site (at the map edge the hull sticks out): no command, the cursor stays.
        let edge = Vec3::new(20.0, 0.0, 20.0);
        let invalid = click_unpack(&mut bar, &mut host, &selection, edge).expect("preview");
        assert!(!invalid.valid);
        assert!(host.pending_commands().is_empty());
        assert_eq!(bar.tent_cursor, Some(cursor));
        // Valid click: `UnpackTent` to the first selected object with the snapped position in
        // fixed-point tiles (`FUN_00453490`), afterwards the cursor is over.
        click_unpack(&mut bar, &mut host, &selection, ground).expect("preview");
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(
            sent,
            [CommandKind::UnpackTent {
                unit: travois,
                x: 30720,
                z: 62464
            }]
        );
        assert_eq!(bar.tent_cursor, None);
        // The cursor ends if the travois is no longer in front or carries no tent any more.
        apply(&mut bar, &mut host, &selection, BarAction::UnpackTent);
        follow_tent_cursor(&mut bar, &host, &selection);
        assert_eq!(bar.tent_cursor, Some(cursor));
        follow_tent_cursor(&mut bar, &host, &[colonist, travois]);
        assert_eq!(bar.tent_cursor, None);
        apply(&mut bar, &mut host, &selection, BarAction::UnpackTent);
        host.state
            .object_mut(travois, false)
            .unwrap()
            .module_mut(nml_sim::module::tent::transport::INDEX)
            .unwrap()
            .base
            .flags &= !nml_sim::module::tent::transport::TRANSPORTING_TENT;
        follow_tent_cursor(&mut bar, &host, &selection);
        assert_eq!(bar.tent_cursor, None);
        // Without a travois in front 453 starts no cursor.
        apply(
            &mut bar,
            &mut host,
            &[colonist, travois],
            BarAction::UnpackTent,
        );
        assert_eq!(bar.tent_cursor, None);
    }

    #[test]
    fn the_original_texts_name_the_formation_buttons() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let texts = load_texts(&vfs);
        for id in [511, 512, 513, 514, 515, 519] {
            let text = texts
                .get(&id)
                .unwrap_or_else(|| panic!("text {id} is missing"));
            assert!(!text.is_empty() && !text.contains('['), "{id}: {text}");
        }
        let b = buttons(
            &UnitBar {
                formation: 1,
                ..UnitBar::default()
            },
            &texts,
            &plain(2),
        );
        assert_eq!(b.len(), 6);
        // Texts of the dismount and dive buttons (`guids.deu` 405, 406, 432, 433).
        for id in [405, 406, 432, 433, 449, 450, 451] {
            let text = texts
                .get(&id)
                .unwrap_or_else(|| panic!("text {id} is missing"));
            assert!(!text.is_empty() && !text.contains('['), "{id}: {text}");
        }
        // Arm, abilities with kind 4 and counter-upgrades, camouflage, dig in and healing, evoke ghost,
        // banish and evoke animal, spells and special attacks.
        // Plus pack and unpack tent (`icons.ini` 452/453) and open and close gate (430/431).
        let ids = [
            ARM_BUTTON,
            DISARM_BUTTON,
            PACK_TENT_BUTTON,
            UNPACK_TENT_BUTTON,
            OPEN_GATE_BUTTON,
            CLOSE_GATE_BUTTON,
        ]
        .into_iter()
        .chain(ABILITY_BUTTONS.iter().map(|b| b.0))
        .chain(STEALTH_HEAL_BUTTONS.iter().map(|b| b.0))
        .chain(EVOCATION_BUTTONS.iter().map(|b| b.0))
        .chain(SPELL_SPECIAL_BUTTONS.iter().map(|b| b.0))
        .chain(COUNTER_UPGRADE_BUTTONS);
        for id in ids {
            let text = texts
                .get(&id)
                .unwrap_or_else(|| panic!("text {id} is missing"));
            assert!(!text.is_empty() && !text.contains('['), "{id}: {text}");
        }
        assert!(!header(&UnitBar::default(), &texts).contains('['));
    }

    #[test]
    fn only_the_locomotive_menu_of_the_original_leads_the_train_buttons() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let text = decode_text_file(&vfs.read(nml_formats::gifcon::TOOLBAR_PATH).unwrap());
        let toolbar = nml_formats::gifcon::parse_toolbar(&text);
        let leading: Vec<i32> = toolbar
            .menus
            .iter()
            .filter(|(_, buttons)| train_button_msg(buttons))
            .map(|(&guid, _)| guid)
            .collect();
        assert_eq!(leading, [2613], "in the shipped data only the locomotive");
    }

    // --- Rob 540 (re/headhunter-robbing 2.4, 3.7) ---

    #[test]
    fn rob_goes_to_every_selected_robber() {
        use crate::game::testworld::{
            ROB_UPGRADE, SANDERS, SPA_COLONIST, UNDERTAKER, hunter_host, spawn, unlock,
        };
        assert_eq!(BarAction::from_button(540), Some(BarAction::Rob));
        assert_eq!(ROB_BUTTON, 540);
        let mut host = hunter_host();
        let u1 = spawn(&mut host, UNDERTAKER, 0, (10, 10));
        let u2 = spawn(&mut host, UNDERTAKER, 0, (12, 10));
        let plain = spawn(&mut host, SPA_COLONIST, 0, (14, 10));
        let foreign = spawn(&mut host, SANDERS, 1, (30, 10));
        let sel = [u1, plain, u2, foreign];
        let texts = BTreeMap::new();
        let bar = UnitBar::default();
        let rob = |host: &mut SimHost, texts: &BTreeMap<i32, String>, sel: &[ObjectId]| {
            button(&buttons(&bar, texts, &summarize(host, sel)), ROB_BUTTON).cloned()
        };
        // Without research 3326 540 is shown gray in the bar (fallback label); the click sends
        // nothing.
        let b = rob(&mut host, &texts, &sel).expect("button 540");
        assert_eq!((b.label.as_str(), b.enabled), ("Rob", false));
        apply(&mut UnitBar::default(), &mut host, &sel, BarAction::Rob);
        assert!(pending(&host).is_empty());
        // With 3326 free: one `RobCorpses` per selected own robber; the colonist and the foreign robber
        // get none.
        unlock(&mut host, 0, ROB_UPGRADE);
        assert!(rob(&mut host, &texts, &sel).expect("button 540").enabled);
        let before = nml_sim::save::checksum(&host.state);
        apply(&mut UnitBar::default(), &mut host, &sel, BarAction::Rob);
        assert_eq!(nml_sim::save::checksum(&host.state), before);
        assert_eq!(
            pending(&host),
            [
                CommandKind::RobCorpses {
                    unit: u1,
                    button: 540,
                },
                CommandKind::RobCorpses {
                    unit: u2,
                    button: 540,
                },
            ]
        );
        // Without an own robber in the selection no button; text from `guids.deu` instead of the fallback.
        assert!(rob(&mut host, &texts, &[plain, foreign]).is_none());
        let named: BTreeMap<i32, String> = [(540, "Rob corpses".to_string())].into_iter().collect();
        assert_eq!(
            rob(&mut host, &named, &sel).expect("button 540").label,
            "Rob corpses"
        );
    }
}

/// Viewer test world of the upgrade bar: `nml_sim::upgrades::testkit`
/// is not visible in the viewer. Colonist 1100 with arming (39), governor 1112 with spell (38)
/// and execution (44), missionary 1113 with the submenu 601; menus, values and records as there
/// (simplified: 3375 with `REQUIRED_GUID 3354`, synthetic effect paths).
#[cfg(any(test, feature = "testkit"))]
pub mod testkit {
    use super::*;
    use crate::game::options::DEFAULT_GAME_SPEED;
    use nml_formats::gifcon::parse_toolbar;
    use nml_formats::temp_upgrades::parse_temp_upgrades;
    use nml_sim::setup::MatchSetup;
    use nml_sim::sim::Sim;
    use nml_sim::state::Control;
    use nml_sim::world::{ObjectDesc, create_object, desc_flags};

    pub const COLONIST: u32 = 1100;
    pub const GOVERNOR: u32 = 1112;
    pub const MISSIONARY: u32 = 1113;

    /// Walk (land) and stand like the colonist of the economy world.
    const WALK: &str = concat!(
        "PROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
        "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\n",
        "PROPID 2\nVALUEID 30001 1 1 0\n",
    );

    /// Types: colonist with module 39, governor with 38 (animations 7 7 7) and 44 (execution list
    /// 4452 4450 4451), missionary without an additional module.
    fn types() -> String {
        format!(
            "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\n{WALK}\
             PROPID 39\nVALUEID 400001 6 1 0\n\
             GUID 1112\nNAME SPA_Gouverneur\nVALUEID 100 6 1 0\n{WALK}\
             PROPID 38\nVALUEID 390001 6 1 0\nVALUEID 390002 1 1 7 7 7\nVALUEID 390003 1 1 5 6 11\n\
             PROPID 44\nVALUEID 450001 1 1 4452 4450 4451\n\
             GUID 1113\nNAME SPA_Missionar\nVALUEID 100 6 1 0\n{WALK}"
        )
    }

    /// Menus like `toolbar.ini` (excerpt): colonist with 485/486, governor with 544, 543 (needs
    /// 2828) and 545, missionary with the submenu 601 (3374, 3375, both with `REQUIRED_GUID 3354`).
    const TOOLBAR: &str = concat!(
        "GUID 1100\n",
        "BUTTONID 12\nBUTTONMSG 485\nTARGET_DO_MSG\nREQUIRED_GUID 2806\n",
        "BUTTONID 12\nBUTTONMSG 486\nTARGET_DO_MSG\n",
        "ENDGUID\n",
        "GUID 1112\n",
        "BUTTONID 7\nBUTTONMSG 544\nDO_MSG\n",
        "BUTTONID 8\nBUTTONMSG 543\nDO_MSG\nREQUIRED_GUID 2828\n",
        "BUTTONID 9\nBUTTONMSG 545\nDO_MSG\n",
        "ENDGUID\n",
        "GUID 1113\nBUTTONID 10\nBUTTONMSG 601\nENDGUID\n",
        "GUID 601\n",
        "BUTTONID 7\nBUTTONMSG 3374\nCOUNTERUPGRADE_MSG\nREQUIRED_GUID 3354\n",
        "BUTTONID 8\nBUTTONMSG 3375\nCOUNTERUPGRADE_MSG\nREQUIRED_GUID 3354\n",
        "ENDGUID\n",
    );

    /// Records: 4450 and 4451 with an effect (4451 with height 30), 4461 temporary without an effect.
    const TEMP_UPGRADES: &str = concat!(
        "TEMPUPGRADE 4450\nTEMPHANDLELISTUPGRADE\nMSGTICK 57F 2 0 1000\n",
        "EFFECT 0 data/effects/test_steuern.pef\nEND\n",
        "TEMPUPGRADE 4451\nTEMPHANDLELISTUPGRADE\nEFFECT 30 data/effects/test_bauzeit.pef\n",
        "MSGSTART 601 1 300\nMSGEND 601 1 100\nEND\n",
        "TEMPUPGRADE 4461\nUPGRADETEMPORARY\nEND\n",
    );

    /// `default.dat` values (config) of the types, upgrades and ability buttons.
    const DEFAULTS: &[(u32, &[(u32, i32)])] = &[
        (COLONIST, &[(14, 160), (16, 145)]),
        (GOVERNOR, &[(14, 1400), (15, 100), (16, 145)]),
        (MISSIONARY, &[(14, 1000), (16, 145)]),
        (2806, &[(13, 10)]),
        (2828, &[(13, 10)]),
        (3354, &[(12, 200), (13, 20)]),
        (3374, &[(10, 150), (11, 150), (12, 150), (13, 120)]),
        (3375, &[(114, -40), (13, 60)]),
        (4450, &[(13, 60)]),
        (4451, &[(13, 60)]),
        (4461, &[(13, 60)]),
        (543, &[(15, 75)]),
        (544, &[(15, 50)]),
        (545, &[(15, 40)]),
    ];

    /// Rules on a 64 x 64 map.
    pub fn rules() -> Rules {
        let mut r = Rules::synthetic(&types(), 64);
        for &(guid, values) in DEFAULTS {
            r.defaults.insert(guid, values.iter().copied().collect());
        }
        r.upgrades.extend([2806, 2828, 3354, 3374, 3375]);
        for guid in [3374, 3375] {
            r.upgrade_types.insert(guid, vec![COLONIST]);
        }
        r.temp_upgrades = parse_temp_upgrades(TEMP_UPGRADES)
            .expect("synthetic tempupgrades.ini")
            .into_iter()
            .map(|t| (t.guid, t))
            .collect();
        r.toolbar = parse_toolbar(TOOLBAR);
        r
    }

    /// Host with player 0 (human, 1000 each of gold, wood and food), map revealed.
    pub fn host() -> SimHost {
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: nml_sim::ki::KiSetup::default(),
            script: nml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules(), setup, DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        host.state.players[0].control = Control::Human;
        host.state.players[0].resources_fx = [1000 << 12, 1000 << 12, 1000 << 12, 0, 50 << 12];
        nml_sim::fog::reveal_map(&mut host.state, -1);
        host
    }

    /// Finished object at the tile center `tile`.
    pub fn spawn(host: &mut SimHost, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
        let rules = host.rules.clone();
        let mut sim = Sim::new(&mut host.state, &rules);
        let at = |t: i32| t as f32 * 128.0 + 64.0;
        let mut desc = ObjectDesc::new(guid, slot, [at(tile.0), 0.0, at(tile.1)]);
        desc.flags |= desc_flags::PLACED;
        create_object(&mut sim, &mut desc).expect("object created")
    }

    /// Set or clear `ISARMED` on the arming module (without a variant change).
    pub fn set_armed(host: &mut SimHost, id: ObjectId, armed: bool) {
        let o = host.state.object_mut(id, false).unwrap();
        let m = o.module_mut(arming::INDEX).expect("module 39");
        if armed {
            m.base.flags |= arming::ISARMED;
        } else {
            m.base.flags &= !arming::ISARMED;
        }
    }
}
