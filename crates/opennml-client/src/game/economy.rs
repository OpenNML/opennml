//! Command UI of the economy: right-click according to `targets.ini`
//! ([`opennml_sim::targeting`]), build and production lists from `toolbar.ini` with the locks of the
//! command check ([`opennml_sim::command::validate`]), placement with build site check
//! ([`opennml_sim::placement`]), queue, sale and letting out. All without winit and GPU
//! (testable); drawing happens in the viewer
//! (`apps/opennml-viewer/src/economy_ui.rs`). The functions on [`SimHost`]
//! produce the same commands as the UI of the original and are also open to the control channel.
//!
//! The queries (target filter, build site, menus) need a mutable simulation context,
//! but do not change the state (test
//! `previews_and_target_queries_leave_the_checksum_unchanged`): What the UI queries per frame
//! is not in the command log and must not change replays.

use std::collections::BTreeMap;

use glam::Vec3;
use opennml_formats::gifcon::button_kind;
use opennml_render::pick::{PickDistances, PickFlags};
use opennml_sim::command::headhunter::{hire_players, hire_price, hire_targets};
use opennml_sim::command::validate::{
    self, button_locked, check_build, check_production, menu_buttons,
};
use opennml_sim::command::{Command, CommandKind, F32Bits};
use opennml_sim::economy::{find_unaffordable_resource, object_costs, resource};
use opennml_sim::fx::Fx;
use opennml_sim::id::ObjectId;
use opennml_sim::module::building::production::{progress_fraction, trade};
use opennml_sim::module::{ModuleKind, building, headhunter, quarter};
use opennml_sim::object::status;
use opennml_sim::placement::{
    CursorPlacement, PlacementBox, cursor_placement, placement_rect, rotation_locked,
};
use opennml_sim::rules::Rules;
use opennml_sim::setup::player_color;
use opennml_sim::sim::Sim;
use opennml_sim::targeting::{
    attack_actions, button, button_target_action, has_memory_action, has_object_action,
    memory_target_action, memory_target_entry, pick_target_action, pick_target_entry,
};

use crate::game::formation_order::FormationOrder;
use crate::game::grid::world_to_grid;
use crate::game::selection::{RightClickTarget, memory_pick_flags};
use crate::game::sim_host::SimHost;

/// Rotation step of the placement: `1.5707964f` (`BuildCursor_UpdatePlacement` `0x0044FDB0`,
/// `(dy/50 & 3) * 1.5707964f`).
pub const ROTATION_STEP: f32 = std::f32::consts::FRAC_PI_2;

/// Name of a right-click action (`BUTTONMSG` from `targets.ini`) for overlay and log.
pub fn action_name(button_msg: i32) -> &'static str {
    match button_msg {
        button::FISH => "Fish",
        button::CHOP_WOOD => "Chop wood",
        button::HARVEST => "Harvest",
        button::MINE_GOLD => "Mine gold",
        button::BUILD => "Build",
        button::REPAIR | button::REPAIR_2 => "Repair",
        button::QUARTER => "Quarter",
        button::MOUNT_HORSE => "Mount",
        button::BOARD_BOAT => "Board",
        button::ATTACK | button::ATTACK_UNIT => "Attack",
        button::GARRISON_ATTACK => "Garrison attack",
        button::GUT => "Gut",
        button::HUNT => "Hunt",
        button::FOLLOW => "Follow",
        button::DELIVER_WOOD => "Deliver wood",
        button::DELIVER_GOLD => "Deliver gold",
        button::DELIVER_FOOD => "Deliver food",
        button::RALLY_POINT => "Rally point",
        button::GO_TO => "Go to",
        _ => "?",
    }
}

/// Actions whose command `Input_SubmitCommandWithQueueKeys` (`0x0044D230`) wraps with Shift or
/// Ctrl in `0x7B`/`0x7C` (re/commands): 398, 400-402, 412/413/569, 444-446
/// and 448; Mount (403), Board (404), Quarter (414) and Rally point (447) always go
/// directly (W-195). Wrapped commands that `GameWorld_ExecuteQueuedCommand` does not queue per
/// unit (Fish `0x7D`) are executed by the simulation immediately, without clearing the queues
/// (quirk).
pub fn wraps_with_queue_keys(button_msg: i32) -> bool {
    matches!(
        button_msg,
        button::FISH
            | button::CHOP_WOOD
            | button::HARVEST
            | button::MINE_GOLD
            | button::BUILD
            | button::REPAIR
            | button::REPAIR_2
            | button::DELIVER_WOOD
            | button::DELIVER_GOLD
            | button::DELIVER_FOOD
            | button::GO_TO
    )
}

/// Command with the queue keys: `Some(false)` Shift (`0x7B`, append once),
/// `Some(true)` Ctrl (`0x7C`, cyclic), `None` direct.
pub fn with_queue_keys(kind: CommandKind, queue: Option<bool>) -> CommandKind {
    match queue {
        Some(cyclic) => CommandKind::Queue {
            cyclic,
            inner: Box::new(kind),
        },
        None => kind,
    }
}

/// Queue keys after `Input_SubmitCommandWithQueueKeys`: Shift takes precedence over Ctrl.
pub fn queue_keys(shift: bool, ctrl: bool) -> Option<bool> {
    if shift {
        Some(false)
    } else if ctrl {
        Some(true)
    } else {
        None
    }
}

/// Modifiers of the right-click (`Input_IssueTargetAction` `0x0044D350`, re/commands 1.5/1.7):
/// - `ctrl` sets modifier 4 on attack (411) (stubbornness 3, re/combat-attack 4.1):
///   `mods |= 4` in command `0x11`.
/// - `run` (right double-click, event `0x12` -> `0x00457C50`: `mods = 1`) sets on "Go to"
///   (448) flag 1 in command `0x04` (walk `0x100`, run) and on attack (411) modifier 1
///   (`0x100`).
///
/// All other actions and commands stay unchanged: They do not read `mods`.
pub fn with_modifiers(button_msg: i32, kind: CommandKind, ctrl: bool, run: bool) -> CommandKind {
    match kind {
        CommandKind::Attack {
            units,
            target,
            pos,
            mods,
        } if button_msg == button::ATTACK_UNIT => CommandKind::Attack {
            units,
            target,
            pos,
            mods: mods | i32::from(run) | if ctrl { 4 } else { 0 },
        },
        CommandKind::Move {
            units,
            x,
            z,
            target,
            formation,
            direction,
            spread,
            flags,
        } if button_msg == button::GO_TO => CommandKind::Move {
            units,
            x,
            z,
            target,
            formation,
            direction,
            spread,
            flags: flags | i32::from(run),
        },
        other => other,
    }
}

/// Display name of a type or trade (490-493).
pub fn type_name(rules: &Rules, guid: u32) -> String {
    guid_name(rules, &BTreeMap::new(), guid)
}

/// Display name of a GUID: trade (490-493), type name, text from `guids.deu` (`texts`, research
/// and counter-upgrades: [`crate::game::texts::upgrade_texts`]), otherwise "Type n".
pub fn guid_name(rules: &Rules, texts: &BTreeMap<u32, String>, guid: u32) -> String {
    match guid {
        trade::BUY_WOOD => "Buy wood".into(),
        trade::BUY_FOOD => "Buy food".into(),
        trade::SELL_WOOD => "Sell wood".into(),
        trade::SELL_FOOD => "Sell food".into(),
        _ => rules.types.get(&guid).map_or_else(
            || {
                texts
                    .get(&guid)
                    .cloned()
                    .unwrap_or_else(|| format!("Type {guid}"))
            },
            |t| t.name.clone(),
        ),
    }
}

/// Costs readable: gold, wood, food, population (values 0 are omitted); without a value set "-".
pub fn cost_text(costs: Option<[i32; 5]>) -> String {
    let Some(c) = costs else {
        return "-".into();
    };
    let parts: Vec<String> = [
        (resource::GOLD, "Gold"),
        (resource::WOOD, "Wood"),
        (resource::FOOD, "Food"),
        (resource::POPULATION, "Pop."),
    ]
    .iter()
    .filter(|(i, _)| c[*i] != 0)
    .map(|(i, name)| format!("{name} {}", c[*i]))
    .collect();
    if parts.is_empty() {
        "free".into()
    } else {
        parts.join(", ")
    }
}

/// Entry of a build or production list.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    pub guid: u32,
    pub name: String,
    /// Kind of the button ([`button_kind`]).
    pub kind: u32,
    /// Gold, wood, food, population, living space (`Player_GetObjectCosts`); `None` without a value set.
    pub costs: Option<[i32; 5]>,
    /// The balance suffices (`Player_FindUnaffordableResource`); otherwise the command reports the shortage.
    pub affordable: bool,
    /// Locked with a reason (command check); `None` = selectable.
    pub blocked: Option<String>,
}

/// Entry of the queue: `index` 0 = running order, `n` = n-th waiting one (index of
/// `0x1103`).
#[derive(Debug, Clone, PartialEq)]
pub struct QueueEntry {
    pub index: i32,
    pub guid: u32,
    pub name: String,
    /// Progress of the running order (`0xF03`, 0 before paying); `None` for waiting ones.
    pub progress: Option<f32>,
}

/// Part "Building" of the window (an own selected building).
#[derive(Debug, Clone, PartialEq)]
pub struct BuildingPanel {
    pub id: ObjectId,
    pub name: String,
    /// Building (with module 3); otherwise a mobile container, ship or wagon, only with quarters.
    pub is_building: bool,
    pub finished: bool,
    pub dead: bool,
    /// Debug "complete immediately" enabled ([`SimHost::debug_complete_allowed`]).
    pub debug_complete: bool,
    pub production: Vec<MenuEntry>,
    pub queue: Vec<QueueEntry>,
    /// Quarters: occupied/capacity and occupants with names; `None` without module 23.
    pub quarter: Option<(i32, i32)>,
    pub occupants: Vec<(ObjectId, String)>,
    /// Rally point in tiles (`m_MeetingPoint`).
    pub meeting_point: [f32; 2],
    /// Headhunter menu ([`HirePanel`]); `None` without `SELECTHEADHUNTER` buttons in the menu of the type
    /// and with an unfinished or dead building.
    pub hire: Option<HirePanel>,
}

/// Colored tooltip entry: text and color index of the player in whose color it is shown.
pub type HireTag = (String, i32);

/// Level 1 of the headhunter menu: a kind (`SELECTHEADHUNTER_MSG`, re/headhunter-robbing 2.5).
#[derive(Debug, Clone, PartialEq)]
pub struct HireArtEntry {
    /// Kind 0..2 (`BUTTONMSG - 4100`).
    pub art: usize,
    /// GUID of the headhunter (4100-4102).
    pub guid: u32,
    pub name: String,
    /// Tooltip price (`FUN_0047DEE0`, [`hire_price`]).
    pub price: i32,
    /// Button not locked ([`button_locked`], like the command check); locked is gray.
    pub enabled: bool,
    /// Type name of the target GUID and color of the target player (tooltip `FUN_0042F7B0`, without label);
    /// `None` without a headhunter of the kind (`headhunters[art]`) and with target GUID 0 or -1 or target
    /// player -1 (after a success).
    pub target: Option<HireTag>,
    /// Name of the customer and its color (tooltip `FUN_0042F8B0`, without label); `None` without a
    /// headhunter of the kind and with customer -1.
    pub customer: Option<HireTag>,
}

/// Headhunter menu of an own finished building (re/headhunter-robbing 2.5, three levels): level
/// 1 the kinds ([`HireArtEntry`]), level 2 after the chosen kind the target players ([`hire_players`]:
/// slot, name - empty "Player n" -, color), level 3 after the chosen target player the target GUIDs
/// ([`hire_targets`]) with type names. The menu state is held by [`HireMenu`].
#[derive(Debug, Clone, PartialEq)]
pub struct HirePanel {
    pub arts: Vec<HireArtEntry>,
    /// Chosen kind (levels 2 and 3).
    pub art: Option<usize>,
    pub players: Vec<(u8, String, i32)>,
    /// Chosen target player (level 3).
    pub target_player: Option<u8>,
    pub targets: Vec<(u32, String)>,
}

/// Menu state of the headhunter menu: the building in which it was opened, the chosen kind and
/// the chosen target player. Reset after hiring, with "Back" level by level, with Esc and
/// right-click ([`EconomyUi::cancel_hire`]), on selection change and on closing the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HireMenu {
    pub building: Option<ObjectId>,
    pub art: Option<usize>,
    pub target_player: Option<u8>,
}

/// Display name of a slot: its name, empty "Player n" (0-based like the messages).
fn player_label(sim: &Sim, slot: i32) -> String {
    usize::try_from(slot)
        .ok()
        .and_then(|s| sim.state.players.get(s))
        .map(|p| p.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Player {slot}"))
}

/// Target and customer of the headhunter of the kind `art` (tooltips `FUN_0042F7B0` = type name of the
/// target GUID in the color of the target player, `FUN_0042F8B0` = name of the customer in its color,
/// re/headhunter-robbing 2.5); each `None` if the headhunter does not exist ([`headhunter::current`]) or
/// the value is -1 (target GUID also 0), then the tooltip stays empty in the original.
fn hire_info(sim: &Sim, art: usize) -> (Option<HireTag>, Option<HireTag>) {
    let Some(id) = headhunter::current(sim.state, art) else {
        return (None, None);
    };
    let h = match sim.object(id).and_then(|o| o.module(headhunter::INDEX)) {
        Some(m) => match &m.kind {
            ModuleKind::HeadHunter(h) => h,
            _ => return (None, None),
        },
        None => return (None, None),
    };
    let target = (h.target_guid > 0 && h.target_player >= 0).then(|| {
        (
            type_name(sim.rules, h.target_guid as u32),
            player_color(sim, h.target_player),
        )
    });
    let customer =
        (h.customer >= 0).then(|| (player_label(sim, h.customer), player_color(sim, h.customer)));
    (target, customer)
}

/// Headhunter menu of the building `building` (type `guid`) for the slot `player` and the
/// menu state `menu`: level 1 from the `SELECTHEADHUNTER` buttons with `BUTTONMSG` 4100-4102 in the menu
/// of the type in menu order, gray when locked (same rule as the
/// command check); levels 2 and 3 only if the menu state applies to this building. `None` without such
/// buttons. Reads only.
fn hire_panel(
    sim: &Sim,
    player: u8,
    building: ObjectId,
    guid: u32,
    menu: &HireMenu,
) -> Option<HirePanel> {
    let arts: Vec<HireArtEntry> = menu_buttons(sim.rules, guid)
        .into_iter()
        .filter(|b| b.kind() == button_kind::SELECTHEADHUNTER)
        .filter_map(|b| {
            let art = headhunter::GUIDS.iter().position(|&g| g as i32 == b.msg)?;
            let hunter = headhunter::GUIDS[art];
            let (target, customer) = hire_info(sim, art);
            Some(HireArtEntry {
                art,
                guid: hunter,
                name: type_name(sim.rules, hunter),
                price: hire_price(sim, art),
                enabled: !button_locked(sim, i32::from(player), b),
                target,
                customer,
            })
        })
        .collect();
    if arts.is_empty() {
        return None;
    }
    let art = menu.art.filter(|_| menu.building == Some(building));
    let target_player = menu.target_player.filter(|_| art.is_some());
    let players = if art.is_some() {
        hire_players(sim, player)
            .into_iter()
            .map(|p| {
                let slot = i32::from(p);
                (p, player_label(sim, slot), player_color(sim, slot))
            })
            .collect()
    } else {
        Vec::new()
    };
    let targets = match (art, target_player) {
        (Some(a), Some(t)) => hire_targets(sim, a, t)
            .into_iter()
            .map(|g| (g, type_name(sim.rules, g)))
            .collect(),
        _ => Vec::new(),
    };
    Some(HirePanel {
        arts,
        art,
        players,
        target_player,
        targets,
    })
}

/// Content of the window "Economy".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PanelModel {
    /// Balance of the active player ([`SimHost::account_line`]).
    pub account: String,
    /// Own selected objects of the active player (workers of the build list).
    pub workers: Vec<ObjectId>,
    pub build: Vec<MenuEntry>,
    pub building: Option<BuildingPanel>,
}

/// Build site under the mouse pointer.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementPreview {
    pub guid: u32,
    /// Snapped position and rotation (as adjusted by the check), as the command receives them.
    pub pos: [f32; 3],
    pub rot: f32,
    pub valid: bool,
    /// Tile rectangle `[x0, z0, x1, z1]` (inclusive) of the check; `None` outside the map.
    pub rect: Option<[i32; 4]>,
    /// Corners of the hull on the ground (world x/z) in winding order.
    pub hull: [[f32; 2]; 4],
}

impl PlacementPreview {
    /// Preview for drawing only (type, position, rotation, verdict), without tile rectangle and hull: the parts of the
    /// wall ring and the build cursor of the command bar ([`crate::game::map::MapView::set_build_previews`]).
    pub fn model(guid: u32, pos: [f32; 3], rot: f32, valid: bool) -> Self {
        Self {
            guid,
            pos,
            rot,
            valid,
            rect: None,
            hull: [[pos[0], pos[2]]; 4],
        }
    }
}

/// Build kinds of the build menus.
const BUILD_KINDS: [u32; 2] = [button_kind::BUILD, button_kind::ONLYONCE];
/// Training, trade and research of the production menus (`COUNTERUPGRADE` is executed by a unit
/// and belongs in no building menu, the command check rejects it).
const PRODUCTION_KINDS: [u32; 3] = [
    button_kind::MAKE,
    button_kind::MAKEONLY_ONCE,
    button_kind::UPGRADE,
];

/// Entry of a list with costs, balance and lock for the slot `player`.
fn menu_entry(
    sim: &Sim,
    texts: &BTreeMap<u32, String>,
    player: u8,
    guid: u32,
    kind: u32,
    blocked: Result<(), String>,
) -> MenuEntry {
    MenuEntry {
        guid,
        name: guid_name(sim.rules, texts, guid),
        kind,
        costs: object_costs(sim, i32::from(player), guid),
        affordable: affordable(sim, player, guid),
        blocked: blocked.err(),
    }
}

/// Ground command of the right-click mapping: "Go to" and the rally point on the ground (on an object
/// `{-1, target}`).
pub(crate) fn is_ground_action(kind: &CommandKind, target: Option<ObjectId>) -> bool {
    match kind {
        CommandKind::Move { .. } => true,
        CommandKind::SetRallyPoint { x, z, .. } => {
            !(*x == -1 && target.is_some_and(|t| t.as_i32() == *z))
        }
        _ => false,
    }
}

/// The balance of the slot `player` suffices for the type `guid` (`Player_FindUnaffordableResource`;
/// always without a value set).
pub fn affordable(sim: &Sim, player: u8, guid: u32) -> bool {
    let slot = usize::from(player);
    object_costs(sim, slot as i32, guid).is_none_or(|c| {
        sim.state.players.get(slot).is_some_and(|p| {
            find_unaffordable_resource(p, sim.state.economy.max_living_space, &c) < 0
        })
    })
}

impl SimHost {
    /// Simulation context for queries and checks.
    pub(crate) fn sim_context(&mut self) -> Sim<'_> {
        Sim::new(&mut self.state, &self.rules)
    }

    /// Objects of the selection that belong to the active player (in selection order).
    pub fn own_objects(&self, selection: &[ObjectId]) -> Vec<ObjectId> {
        selection
            .iter()
            .copied()
            .filter(|&id| {
                self.state
                    .object(id, false)
                    .is_some_and(|o| o.owner() == self.active_player)
            })
            .collect()
    }

    /// The first selected object belongs to the active player (otherwise the UI gives no commands,
    /// `0x00457C50`).
    pub(crate) fn commands_selection(&self, selection: &[ObjectId]) -> bool {
        selection.first().is_some_and(|&id| {
            self.state
                .object(id, false)
                .is_some_and(|o| o.owner() == self.active_player)
        })
    }

    /// Right-click action for the selection on the object `target` (under the mouse) or the
    /// terrain point `ground` (world): `BUTTONMSG` and command to the whole selection
    /// ([`pick_target_action`]); `None` if the selection does not belong to the active player or no
    /// action matches. Ground actions (Go to, rally point on the ground) outside the map are discarded by
    /// `Input_IssueTargetAction` (`0x0044D3D0`...: `x < 0`, `x > Fixed(dim_x)`, `z` likewise).
    pub fn target_action(
        &mut self,
        selection: &[ObjectId],
        target: Option<ObjectId>,
        ground: Vec3,
    ) -> Option<(i32, CommandKind)> {
        if !self.commands_selection(selection) {
            return None;
        }
        let player = self.active_player;
        let map = self.rules.map;
        let ground = world_to_grid(ground, map.cell);
        let action =
            pick_target_action(&mut self.sim_context(), player, selection, target, ground)?;
        let inside = |v: i32, dim: i32| v >= 0 && i64::from(v) <= i64::from(dim) << 12;
        if is_ground_action(&action.1, target)
            && !(inside(ground.0, map.dim_x) && inside(ground.1, map.dim_z))
        {
            return None;
        }
        Some(action)
    }

    /// Right-click action on an active fog memory (`TargetActions_FindForFogMemory`
    /// `0x0047D770`, [`memory_target_action`]): filter on the snapshot of the memory of the
    /// object `memory`, command to the real ID. `None` without a memory or snapshot, if the
    /// selection does not belong to the active player or no action matches.
    pub fn memory_action(
        &mut self,
        selection: &[ObjectId],
        memory: ObjectId,
    ) -> Option<(i32, CommandKind)> {
        if !self.commands_selection(selection) {
            return None;
        }
        let player = self.active_player;
        let snap = self.memories.get(memory)?.snapshot.as_ref()?;
        memory_target_action(
            &mut Sim::new(&mut self.state, &self.rules),
            player,
            selection,
            snap,
        )
    }

    /// Right-click action for the right-click target `target` ([`RightClickTarget`]) or the
    /// terrain point `ground`: objects and ground via [`SimHost::target_action`],
    /// fog memories via [`SimHost::memory_action`]. For the hint "Right-click: ..." and the
    /// right-click.
    pub fn right_click_action(
        &mut self,
        selection: &[ObjectId],
        target: Option<RightClickTarget>,
        ground: Vec3,
    ) -> Option<(i32, CommandKind)> {
        match target {
            Some(RightClickTarget::Memory(id)) => self.memory_action(selection, id),
            Some(RightClickTarget::Object(id)) => self.target_action(selection, Some(id), ground),
            None => self.target_action(selection, None, ground),
        }
    }

    /// Line of `targets.ini` that the right-click chooses for the right-click target `target` (object, fog memory,
    /// the ground for `None`): [`pick_target_entry`] or [`memory_target_entry`] with the active slot, without a command
    /// and without an ownership check of the selection (`Input_PickTargetAction` `0x00480E80`; the check is made only
    /// by sending, [`SimHost::right_click_sends`]). Index into `rules.targets`. Changes nothing (the filters only need
    /// the simulation to be mutable).
    pub fn right_click_entry(
        &mut self,
        selection: &[ObjectId],
        target: Option<RightClickTarget>,
    ) -> Option<usize> {
        let player = self.active_player;
        match target {
            Some(RightClickTarget::Memory(id)) => {
                let snap = self.memories.get(id)?.snapshot.as_ref()?;
                memory_target_entry(
                    &mut Sim::new(&mut self.state, &self.rules),
                    player,
                    selection,
                    snap,
                )
            }
            Some(RightClickTarget::Object(id)) => {
                pick_target_entry(&mut self.sim_context(), player, selection, Some(id))
            }
            None => pick_target_entry(&mut self.sim_context(), player, selection, None),
        }
    }

    /// Pointer over the world (re/game-ui D5.2; mouse path of the bar `0x00453790`): `MAUSICONID` of the line
    /// that the right-click would choose for `target` ([`SimHost::right_click_entry`], active list, O-281),
    /// otherwise pointer 7. The ground action outside the map and a line without a command are not discarded by the
    /// pointer (it follows the line, not the command). Takes `&mut self` (the filters) and no ground point (the
    /// line choice does not need it). Changes nothing.
    pub fn hover_pointer(
        &mut self,
        selection: &[ObjectId],
        target: Option<RightClickTarget>,
    ) -> i32 {
        self.right_click_entry(selection, target)
            .map_or(opennml_formats::gifcon::POINTER_NORMAL, |i| {
                self.rules.targets[i].mouse_icon
            })
    }

    /// Does releasing the right button set the pointer to 7 (`Input_OnTargetButtonUp` `0x00457C50`, `0x00457D8F`)?
    /// Yes, if the selection is empty or its first object belongs to the active player and the right-click finds a
    /// line ([`SimHost::right_click_entry`], `+0x578 & 8`) - also a line without a command.
    pub fn right_click_sends(
        &mut self,
        selection: &[ObjectId],
        target: Option<RightClickTarget>,
    ) -> bool {
        (selection.is_empty() || self.commands_selection(selection))
            && self.right_click_entry(selection, target).is_some()
    }

    /// Right-click target of the selection like `Input_PickTargetAction` (`0x00480E80`,
    /// [`crate::game::selection::right_click_target`]) over the render list `objects` with the ray test
    /// `pick`, then the active fog memories ([`crate::game::fog_memory::FogMemories::active`])
    /// with the ray test `pick_memory` (ID of the memory): A rank whose last accepted
    /// hit has no action for the selection ([`has_object_action`] or [`has_memory_action`],
    /// filters only, slot of the active player) falls through.
    pub fn right_click_target<K: Copy>(
        &mut self,
        selection: &[ObjectId],
        objects: &[(ObjectId, K)],
        pick: impl FnMut(K, PickFlags, &mut PickDistances) -> bool,
        pick_memory: impl FnMut(ObjectId, PickFlags, &mut PickDistances) -> bool,
    ) -> Option<RightClickTarget> {
        let player = self.active_player;
        let rules = &self.rules;
        let memories = &self.memories;
        let view = opennml_sim::fog::FogView::new(&self.state);
        let list: Vec<_> = memories
            .active()
            .filter_map(|m| {
                let snap = m.snapshot.as_ref()?;
                Some((snap.id, memory_pick_flags(snap, &view)?))
            })
            .collect();
        crate::game::selection::right_click_target(
            &mut self.state,
            objects,
            &list,
            pick,
            pick_memory,
            |state, target| {
                let sim = &mut Sim::new(state, rules);
                match target {
                    RightClickTarget::Object(id) => has_object_action(sim, player, selection, id),
                    RightClickTarget::Memory(id) => memories
                        .get(id)
                        .and_then(|m| m.snapshot.as_ref())
                        .is_some_and(|snap| has_memory_action(sim, player, selection, snap)),
                }
            },
        )
    }

    /// Right-click (`Input_IssueTargetAction` `0x0044D350`): send the command of the action
    /// ([`SimHost::right_click_action`]: object, fog memory or ground for `None`),
    /// wrapped with the queue keys `queue` ([`queue_keys`]), as far as the original wraps the action
    /// ([`wraps_with_queue_keys`]). Returns `BUTTONMSG` and the sent command. Ctrl
    /// counts here only as `queue == Some(true)`; the UI passes it with
    /// [`SimHost::issue_target_action_keys`] independent of Shift.
    #[cfg(test)]
    pub fn issue_target_action(
        &mut self,
        selection: &[ObjectId],
        target: Option<RightClickTarget>,
        ground: Vec3,
        queue: Option<bool>,
    ) -> Option<(i32, CommandKind)> {
        self.issue_target_action_keys(
            selection,
            target,
            ground,
            queue,
            queue == Some(true),
            false,
            FormationOrder::default(),
        )
    }

    /// As [`SimHost::issue_target_action`], with the Ctrl key `ctrl` separate from `queue`
    /// (Shift takes precedence for the queues, Ctrl still sets modifier 4 on attack,
    /// [`with_modifiers`]); `run` is the right double-click (modifier 1: "Go to"
    /// and attack run, [`with_modifiers`]). `order` carries the selected formation into the
    /// move command and the dragging only for "Go to" 448 (`Input_IssueTargetAction` `0x0044D350`,
    /// re/groups-formations 12.6), before the queue wrapper.
    #[allow(clippy::too_many_arguments)]
    pub fn issue_target_action_keys(
        &mut self,
        selection: &[ObjectId],
        target: Option<RightClickTarget>,
        ground: Vec3,
        queue: Option<bool>,
        ctrl: bool,
        run: bool,
        order: FormationOrder,
    ) -> Option<(i32, CommandKind)> {
        let (button_msg, kind) = self.right_click_action(selection, target, ground)?;
        let kind = with_modifiers(button_msg, kind, ctrl, run);
        // Dragging only for "Go to" 448 (`0x00453790` sets the flag `0x10` only for
        // `Input+0x584 == 0x1C0`, `0x00453934`); all move commands carry the formation.
        let order = if button_msg == button::GO_TO {
            order
        } else {
            FormationOrder {
                drag: None,
                ..order
            }
        };
        let kind = order.apply(kind);
        let kind = if wraps_with_queue_keys(button_msg) {
            with_queue_keys(kind, queue)
        } else {
            kind
        };
        self.submit(Command {
            player: self.active_player,
            kind: kind.clone(),
        });
        Some((button_msg, kind))
    }

    /// Right-click of the UI: the action once (event 4,
    /// [`SimHost::issue_target_action_keys`]) and on a right double-click (`double`) **a second
    /// time** with modifier 1 (run, event `0x12` -> `0x00457C50`, re/commands 1.5): On the second
    /// release the original first issues the normal, then the running command
    /// (D-227). If the action yields nothing, there is no second output either. Returns: the
    /// sent actions in order.
    #[allow(clippy::too_many_arguments)]
    pub fn issue_right_click(
        &mut self,
        selection: &[ObjectId],
        target: Option<RightClickTarget>,
        ground: Vec3,
        queue: Option<bool>,
        ctrl: bool,
        order: FormationOrder,
        double: bool,
    ) -> Vec<(i32, CommandKind)> {
        let mut issued = Vec::new();
        for run in [false, true].into_iter().take(1 + usize::from(double)) {
            let Some(action) =
                self.issue_target_action_keys(selection, target, ground, queue, ctrl, run, order)
            else {
                break;
            };
            issued.push(action);
        }
        issued
    }

    /// Attack mode (key X, F7; `targeting::attack_actions`): an object target with entry 427
    /// gets `0x11` with modifier 4, otherwise stance 3 (`0x12`) and the march `0x04` with flag 4,
    /// which is wrapped like "Go to" (448) with the queue keys `queue`. All commands
    /// go out in this order and are returned. Empty if the selection does not belong to the
    /// active player or a march would lie outside the map (as with ground actions).
    /// The march carries the formation of `order` (427, re/groups-formations 12.6), but no dragging: The original
    /// sets the flag `0x10` only for "Go to" 448 (`0x00453934`).
    pub fn issue_attack_action(
        &mut self,
        selection: &[ObjectId],
        target: Option<ObjectId>,
        ground: Vec3,
        queue: Option<bool>,
        order: FormationOrder,
    ) -> Vec<CommandKind> {
        if !self.commands_selection(selection) {
            return Vec::new();
        }
        let player = self.active_player;
        let map = self.rules.map;
        let ground = world_to_grid(ground, map.cell);
        let kinds = attack_actions(&mut self.sim_context(), player, selection, target, ground);
        let inside = |v: i32, dim: i32| v >= 0 && i64::from(v) <= i64::from(dim) << 12;
        let marches = kinds.iter().any(|k| matches!(k, CommandKind::Move { .. }));
        if marches && !(inside(ground.0, map.dim_x) && inside(ground.1, map.dim_z)) {
            return Vec::new();
        }
        let kinds: Vec<CommandKind> = kinds
            .into_iter()
            .map(|k| match k {
                CommandKind::Move { .. } => with_queue_keys(
                    FormationOrder {
                        drag: None,
                        ..order
                    }
                    .apply(k),
                    queue,
                ),
                other => other,
            })
            .collect();
        for kind in &kinds {
            self.submit(Command {
                player,
                kind: kind.clone(),
            });
        }
        kinds
    }

    /// Command of the targeting mode of a bar button (TARGET 403, 413, 414, 447 ...), without
    /// sending it: target according to the entry of the `BUTTONMSG` `button_msg` ([`button_target_action`], target
    /// cursor `FUN_004515B0`), command to the whole selection. `None` if the selection does not belong to the active
    /// player, no target matches, no command is built or a ground action would lie outside the map (as
    /// [`SimHost::target_action`]). For the validity of the target cursor and [`SimHost::issue_button_action`].
    /// Reads only.
    pub fn button_action(
        &mut self,
        button_msg: i32,
        selection: &[ObjectId],
        target: Option<ObjectId>,
        ground: Vec3,
    ) -> Option<CommandKind> {
        if !self.commands_selection(selection) {
            return None;
        }
        let player = self.active_player;
        let map = self.rules.map;
        let grid = world_to_grid(ground, map.cell);
        let kind = button_target_action(
            &mut self.sim_context(),
            player,
            selection,
            button_msg,
            target,
            grid,
        )?;
        let inside = |v: i32, dim: i32| v >= 0 && i64::from(v) <= i64::from(dim) << 12;
        if is_ground_action(&kind, target)
            && !(inside(grid.0, map.dim_x) && inside(grid.1, map.dim_z))
        {
            return None;
        }
        Some(kind)
    }

    /// Trigger the targeting mode of a bar button (`0x16` -> `0x00457C50` -> `Input_IssueTargetAction` `0x0044D350`
    /// with the `BUTTONMSG` of the button): the command from [`SimHost::button_action`] with Ctrl
    /// ([`with_modifiers`]), the selected formation without dragging (`order`) and, as far as the original wraps
    /// the action ([`wraps_with_queue_keys`]), the queue keys `queue`. The attack 427 goes via
    /// [`SimHost::issue_attack_action`]. Returns the sent command.
    #[allow(clippy::too_many_arguments)]
    pub fn issue_button_action(
        &mut self,
        button_msg: i32,
        selection: &[ObjectId],
        target: Option<ObjectId>,
        ground: Vec3,
        queue: Option<bool>,
        ctrl: bool,
        order: FormationOrder,
    ) -> Option<CommandKind> {
        let kind = self.button_action(button_msg, selection, target, ground)?;
        let kind = with_modifiers(button_msg, kind, ctrl, false);
        let kind = FormationOrder {
            drag: None,
            ..order
        }
        .apply(kind);
        let kind = if wraps_with_queue_keys(button_msg) {
            with_queue_keys(kind, queue)
        } else {
            kind
        };
        self.submit_own(kind.clone());
        Some(kind)
    }

    /// Build list of the own selected workers: the `BUILD`/`ONLYONCE` buttons in the menu of their
    /// type and its submenus by the rule of the command check ([`menu_buttons`]: also the
    /// buttons without a kind of the female settler/pioneer open their submenu; once per GUID,
    /// workers in selection order), locked by [`check_build`] (prerequisite, once only).
    pub fn build_menu(&mut self, selection: &[ObjectId]) -> Vec<MenuEntry> {
        let workers = self.own_objects(selection);
        let player = self.active_player;
        let mut guids: Vec<(u32, u32)> = Vec::new();
        for &w in &workers {
            let Some(o) = self.state.object(w, false) else {
                continue;
            };
            for b in menu_buttons(&self.rules, o.guid) {
                let guid = b.msg as u32;
                if BUILD_KINDS.contains(&b.kind()) && guids.iter().all(|g| g.0 != guid) {
                    guids.push((guid, b.kind()));
                }
            }
        }
        let texts = &self.upgrade_texts;
        let sim = Sim::new(&mut self.state, &self.rules);
        guids
            .into_iter()
            .map(|(guid, kind)| {
                let blocked = check_build(&sim, player, guid, &workers);
                menu_entry(&sim, texts, player, guid, kind, blocked)
            })
            .collect()
    }

    /// Production list of a building: `MAKE`/`MAKEONLY_ONCE` (training, trade) and research
    /// (`UPGRADE`) from the menu of the building type and its submenus ([`menu_buttons`], the same
    /// rule as the command check), locked by [`check_production`] (training and research
    /// alike). GUIDs that the level data (config 1 = 0) or a script (action 207) lock
    /// (`Player_IsGuidDisabled`, [`opennml_sim::world::is_guid_disabled`]) are hidden by the original: they are
    /// not in the list.
    pub fn production_menu(&mut self, building_id: ObjectId) -> Vec<MenuEntry> {
        let Some(menu) = self.state.object(building_id, false).map(|o| o.guid) else {
            return Vec::new();
        };
        let player = self.active_player;
        let mut guids: Vec<(u32, u32)> = Vec::new();
        let sim = Sim::new(&mut self.state, &self.rules);
        for b in menu_buttons(&self.rules, menu) {
            let guid = b.msg as u32;
            if PRODUCTION_KINDS.contains(&b.kind())
                && !opennml_sim::world::is_guid_disabled(&sim, i32::from(player), guid)
                && guids.iter().all(|g| g.0 != guid)
            {
                guids.push((guid, b.kind()));
            }
        }
        let texts = &self.upgrade_texts;
        guids
            .into_iter()
            .map(|(guid, kind)| {
                let blocked = check_production(&sim, player, building_id, guid);
                menu_entry(&sim, texts, player, guid, kind, blocked)
            })
            .collect()
    }

    /// Queue of a building: running order (index 0, progress as `0xF03`) and the
    /// waiting ones (index 1 ...).
    pub fn production_queue(&self, building_id: ObjectId) -> Vec<QueueEntry> {
        let Some(ModuleKind::Building(b)) = self
            .state
            .object(building_id, false)
            .and_then(|o| o.module(building::INDEX))
            .map(|m| &m.kind)
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if b.make_guid != 0 {
            out.push(QueueEntry {
                index: 0,
                guid: b.make_guid,
                name: guid_name(&self.rules, &self.upgrade_texts, b.make_guid),
                progress: Some(if b.make_start == 0 {
                    0.0
                } else {
                    progress_fraction(b.make_time, b.make_start)
                }),
            });
        }
        for (i, &guid) in b.make_queue.iter().enumerate() {
            out.push(QueueEntry {
                index: i as i32 + 1,
                guid,
                name: guid_name(&self.rules, &self.upgrade_texts, guid),
                progress: None,
            });
        }
        out
    }

    /// The object has a building module (module 3).
    pub fn is_building(&self, id: ObjectId) -> bool {
        self.state
            .object(id, false)
            .is_some_and(|o| o.module(building::INDEX).is_some())
    }

    /// Debug "complete immediately" (`DebugCompleteOrder`) is enabled for `building`: same
    /// condition as the check of the simulation (A8) - building of the active player that is alive; a
    /// sleeping building (state `DIED`) also counts as dead ([`validate::object_is_dead`]).
    pub fn debug_complete_allowed(&self, building: ObjectId) -> bool {
        self.state
            .object(building, false)
            .is_some_and(|o| o.owner() == self.active_player && !validate::object_is_dead(o))
    }

    /// Building part of the window if the first selected object is an own building or an
    /// own container with a quarter module (building, ship, wagon): occupants with the buttons
    /// "Let out" and "Let out all" (`Unquarter`/`UnquarterAll`, re/ships 7). Vehicles have
    /// neither production nor queue nor rally point nor sale.
    ///
    /// The headhunter menu ([`HirePanel`]) exists only for an own, finished, living
    /// building; `hire` is the menu state of the UI.
    fn building_panel(&mut self, selection: &[ObjectId], hire: &HireMenu) -> Option<BuildingPanel> {
        let id = *selection.first()?;
        let o = self.state.object(id, false)?;
        if o.owner() != self.active_player {
            return None;
        }
        let building = match o.module(building::INDEX).map(|m| &m.kind) {
            Some(ModuleKind::Building(b)) => Some(b),
            _ => None,
        };
        if building.is_none() && o.module(quarter::INDEX).is_none() {
            return None;
        }
        let finished = building.is_none_or(|b| !b.is_build_building);
        let dead = building.map_or(o.status & status::DIED != 0, |b| b.is_dead);
        let meeting_point = building.map_or([0.0; 2], |b| {
            [b.meeting_point.x.to_f32(), b.meeting_point.z.to_f32()]
        });
        let is_building = building.is_some();
        let quarter_module = o.module(quarter::INDEX).and_then(|m| match &m.kind {
            ModuleKind::Quarter(q) => Some(q),
            _ => None,
        });
        let quarter = quarter_module.map(|q| (q.cur, q.max));
        let occupant_ids: Vec<ObjectId> =
            quarter_module.map_or_else(Vec::new, |q| q.occupants.clone());
        let name = type_name(&self.rules, o.guid);
        let guid = o.guid;
        let occupants = occupant_ids
            .into_iter()
            .map(|u| {
                let name = self
                    .state
                    .object(u, false)
                    .map_or_else(|| "?".into(), |o| type_name(&self.rules, o.guid));
                (u, name)
            })
            .collect();
        let hire = if is_building && finished && !dead {
            let player = self.active_player;
            let sim = Sim::new(&mut self.state, &self.rules);
            hire_panel(&sim, player, id, guid, hire)
        } else {
            None
        };
        Some(BuildingPanel {
            id,
            name,
            is_building,
            finished,
            dead,
            debug_complete: is_building && self.debug_complete_allowed(id),
            production: if is_building {
                self.production_menu(id)
            } else {
                Vec::new()
            },
            queue: self.production_queue(id),
            quarter,
            occupants,
            meeting_point,
            hire,
        })
    }

    /// Content of the window "Economy" for the selection (headhunter menu on level 1); the
    /// UI uses [`SimHost::panel_with_hire`] with its menu state.
    #[cfg(any(test, feature = "testkit"))]
    pub fn panel(&mut self, selection: &[ObjectId]) -> PanelModel {
        self.panel_with_hire(selection, &HireMenu::default())
    }

    /// Content of the window "Economy" for the selection with the menu state `hire` of the
    /// headhunter menu. Reads only.
    pub fn panel_with_hire(&mut self, selection: &[ObjectId], hire: &HireMenu) -> PanelModel {
        PanelModel {
            account: self.account_line(),
            workers: self.own_objects(selection),
            build: self.build_menu(selection),
            building: self.building_panel(selection, hire),
        }
    }

    /// Build site of the type `guid` at the terrain point `ground` with `steps` quarter turns like the
    /// build cursor ([`cursor_placement`]: minimum hull corner to the tile boundary, then the check
    /// with layer and region of the first selected own object; tower, field, gold mine and gate snap
    /// in the process, locked types have rotation 0). The position is exactly the one the command check
    /// accepts. `None` without a model hull. Snapping is done with `GameWorld_SnapBoxMinCornerToGrid`
    /// (`0x0047A930`); the UI uses `BuildCursor_GetSnappedPosition` (`0x004CD240`), the same
    /// calculation with the full rotation matrix of the cursor model and rounding down via `ftol`. Deviations:
    /// Every rotation with R snaps again at once (the original does not snap again while dragging, only
    /// the rotation changes - with non-square buildings the rotated position differs until the
    /// next placement). Valid only on a seen tile, also for wall segments (like
    /// `0x0044FDB0`); the command checks the sight itself without view switch (re/construction 5.7).
    pub fn placement_preview(
        &mut self,
        guid: u32,
        ground: Vec3,
        steps: u8,
        selection: &[ObjectId],
    ) -> Option<PlacementPreview> {
        let rot = f32::from(steps & 3) * ROTATION_STEP;
        self.placement_preview_rot(guid, ground, rot, selection)
    }

    /// [`SimHost::placement_preview`] with the fixed rotation `rot` (radians) instead of quarter turns:
    /// the unpack cursor shows the tent in its own rotation (re/tents 3.4).
    pub fn placement_preview_rot(
        &mut self,
        guid: u32,
        ground: Vec3,
        rot: f32,
        selection: &[ObjectId],
    ) -> Option<PlacementPreview> {
        let hull = PlacementBox::of(&self.rules, guid)?;
        let map = self.rules.map;
        let worker = self.own_objects(selection).first().copied();
        let player = i32::from(self.active_player);
        let CursorPlacement { pos, rot, valid } = cursor_placement(
            &mut self.sim_context(),
            guid,
            ground.to_array(),
            rot,
            player,
            worker,
        )?;
        // Fog (`BuildCursor_UpdatePlacement` `0x0044FDB0`, no exception for wall segments):
        // seen at the tile under the preview (`IsWorldPosVisibleToPlayer(position, -1, 0)`), with
        // view switch. The wall rule "seen or explored" applies only to the parts of the
        // wall ring (`BuildCursor_CheckWallSegments` `0x00450870`, in command `0x18`).
        let seen = self
            .fog
            .view(&self.state)
            .world_pos_visible(&map, pos, player, false);
        let valid = valid && seen;
        let m = opennml_sim::footprint::world_matrix(pos, [0.0, rot, 0.0], [1.0; 3]);
        let corner = |x: f32, z: f32| {
            let v = opennml_sim::footprint::transform_coord([x, 0.0, z], &m);
            [v[0], v[2]]
        };
        let (mn, mx) = (hull.mn, hull.mx);
        Some(PlacementPreview {
            guid,
            pos,
            rot,
            valid,
            rect: placement_rect(mn, mx, pos, rot, &map),
            hull: [
                corner(mn[0], mn[2]),
                corner(mx[0], mn[2]),
                corner(mx[0], mx[2]),
                corner(mn[0], mx[2]),
            ],
        })
    }

    /// Verdict of the build or tent cursor of the command bar for an already snapped position and rotation (the
    /// position after the step of the cursor, e.g. when rotating): the check from the position without
    /// renewed tile snapping (that is not idempotent, [`opennml_sim::placement::snapped_command_pose`]) must yield exactly
    /// this position, and the tile is seen (with view switch like [`SimHost::placement_preview_rot`]).
    /// Worker as there: the first own object of `selection`. Reads only.
    pub fn placement_pose_valid(
        &mut self,
        guid: u32,
        pos: [f32; 3],
        rot: f32,
        selection: &[ObjectId],
    ) -> bool {
        let worker = self.own_objects(selection).first().copied();
        let player = i32::from(self.active_player);
        let snapped = opennml_sim::placement::snapped_command_pose(
            &mut self.sim_context(),
            guid,
            pos,
            rot,
            player,
            worker,
        );
        let map = self.rules.map;
        snapped.is_some_and(|s| opennml_sim::placement::same_pose(s, (pos, rot)))
            && self
                .fog
                .view(&self.state)
                .world_pos_visible(&map, pos, player, false)
    }

    /// Verdict per part of the wall ring of the command bar (`BuildCursor_CheckWallSegments` `0x00450870`):
    /// `parts` as (GUID, position, rotation); if the ring is smaller than 4 x 4 (`small`), all parts are
    /// invalid. Reads only.
    pub fn wall_cursor_parts_valid(
        &mut self,
        parts: &[(u32, [f32; 3], f32)],
        small: bool,
    ) -> Vec<bool> {
        if small {
            return vec![false; parts.len()];
        }
        let player = i32::from(self.active_player);
        let mut sim = self.sim_context();
        parts
            .iter()
            .map(|&(guid, pos, rot)| {
                opennml_sim::command::economy::wall_cursor_part_valid(
                    &mut sim, guid, pos, rot, player,
                )
            })
            .collect()
    }

    /// Verdict of the target cursor of a bar button (`FUN_004515B0`): the selection commands and the target
    /// under the pointer matches the entry of the `BUTTONMSG` in `targets.ini`
    /// ([`opennml_sim::targeting::button_target_valid`]). Reads only.
    pub fn button_target_valid(
        &mut self,
        button_msg: i32,
        selection: &[ObjectId],
        target: Option<ObjectId>,
    ) -> bool {
        let player = self.active_player;
        self.commands_selection(selection)
            && opennml_sim::targeting::button_target_valid(
                &mut self.sim_context(),
                player,
                button_msg,
                target,
            )
    }

    /// Place a building (`0x01`, build bar): type, position and rotation of the preview with all own
    /// selected objects as workers, wrapped with the queue keys (Shift: the building arises at once, the
    /// workers queue the construction). `false` without own objects.
    pub fn place_building(
        &mut self,
        preview: &PlacementPreview,
        selection: &[ObjectId],
        queue: Option<bool>,
    ) -> bool {
        let workers = self.own_objects(selection);
        if workers.is_empty() {
            return false;
        }
        let [x, y, z] = preview.pos;
        let kind = CommandKind::PlaceBuilding {
            guid: preview.guid,
            rotation: F32Bits::new(preview.rot),
            x: F32Bits::new(x),
            y: F32Bits::new(y),
            z: F32Bits::new(z),
            workers,
            border: None,
        };
        self.submit(Command {
            player: self.active_player,
            kind: with_queue_keys(kind, queue),
        });
        true
    }

    /// Command of the active player for the next tick boundary.
    pub fn submit_own(&mut self, kind: CommandKind) {
        self.submit(Command {
            player: self.active_player,
            kind,
        });
    }
}

/// Click in the window "Economy".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelAction {
    /// Build list: placement mode for the type.
    Build(u32),
    /// Production list: enqueue (`0x0E`).
    Produce(u32),
    /// Queue: cancel (`0x0F`, index 0 = running).
    Cancel(i32),
    /// Queue, **debug** (no game feature): complete the order `index` of the shown type `guid`
    /// immediately (`DebugCompleteOrder`; the simulation checks ownership, index, type and costs).
    DebugComplete { index: i32, guid: u32 },
    /// Sell: the first click asks, the second sells (`0x67`).
    Sell,
    /// Let out all occupants (`0x15` per own selected object, K-88).
    UnquarterAll,
    /// Let out one occupant (`0x14`).
    Unquarter(ObjectId),
    /// Headhunter menu level 1: choose kind 0..2 (`FUN_0044A8C0`).
    HireArt(usize),
    /// Level 2: choose target player (color button `470 + color`, `FUN_0044AA10`).
    HirePlayer(u8),
    /// Level 3: choose target GUID, sends `0x2C` (`Toolbar_SubmitHireHeadhunter` `0x0044B700`).
    HireTarget(u32),
    /// "Back" (button 409): one level up.
    HireBack,
}

/// Placement mode: type and quarter turns (key R).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub guid: u32,
    pub steps: u8,
}

/// State of the command UI (without drawing): window open, placement mode, confirmation when
/// selling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EconomyUi {
    pub open: bool,
    pub placement: Option<Placement>,
    /// Sell was clicked once (for this building).
    pub sell_armed: Option<ObjectId>,
    /// State of the headhunter menu.
    pub hire: HireMenu,
}

impl EconomyUi {
    /// Window on/off; when closed, the placement mode also ends.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.placement = None;
            self.sell_armed = None;
            self.hire = HireMenu::default();
        }
    }

    /// Esc/right-click: reset the headhunter menu to level 1; `true` if a kind was chosen.
    pub fn cancel_hire(&mut self) -> bool {
        let open = self.hire.art.is_some();
        self.hire = HireMenu::default();
        open
    }

    /// Esc/right-click: end the placement mode; `true` if it was active.
    pub fn cancel_placement(&mut self) -> bool {
        self.placement.take().is_some()
    }

    /// Key R: one more quarter turn (original: dragging in 90 degree steps). Types with the
    /// building flags `0x10000000` ([`rotation_locked`]: tower, gate, field, gold mine, Spanish shipyard
    /// 1008) are never rotated by the build cursor (`BuildCursor_UpdatePlacement` `0x0044FDB0`): R has no effect.
    pub fn rotate(&mut self, rules: &Rules) {
        if let Some(p) = &mut self.placement
            && !rotation_locked(rules, p.guid)
        {
            p.steps = (p.steps + 1) & 3;
        }
    }

    /// Executes a click in the window. The commands apply to the first selected object (building)
    /// or to all own selected ones (construction, "Let out all" per object).
    pub fn apply(&mut self, host: &mut SimHost, selection: &[ObjectId], action: PanelAction) {
        let building = host.own_objects(selection).first().copied();
        if !matches!(action, PanelAction::Sell) {
            self.sell_armed = None;
        }
        match action {
            PanelAction::Build(guid) => {
                self.placement = Some(Placement { guid, steps: 0 });
            }
            PanelAction::Produce(guid) => {
                if let Some(building) = building {
                    host.submit_own(CommandKind::EnqueueProduction { building, guid });
                }
            }
            PanelAction::Cancel(index) => {
                if let Some(building) = building {
                    host.submit_own(CommandKind::CancelProduction {
                        building,
                        index,
                        arg: 0,
                    });
                }
            }
            PanelAction::DebugComplete { index, guid } => {
                if let Some(building) = building.filter(|&b| host.debug_complete_allowed(b)) {
                    host.submit_own(CommandKind::DebugCompleteOrder {
                        building,
                        index,
                        guid,
                    });
                }
            }
            PanelAction::Sell => {
                // Selling exists only for buildings (no ship, no wagon).
                let Some(building) = building.filter(|&b| host.is_building(b)) else {
                    return;
                };
                if self.sell_armed == Some(building) {
                    self.sell_armed = None;
                    host.submit_own(CommandKind::SellBuildings {
                        buildings: vec![building],
                    });
                } else {
                    self.sell_armed = Some(building);
                }
            }
            PanelAction::UnquarterAll => {
                // 416: one command per object (`Toolbar_SubmitUnquarterAll` `0x0044B360`, K-88): every own selected
                // object; the simulation checks per container.
                for container in host.own_objects(selection) {
                    host.submit_own(CommandKind::UnquarterAll { container });
                }
            }
            PanelAction::Unquarter(unit) => {
                if let Some(container) = building {
                    host.submit_own(CommandKind::Unquarter { container, unit });
                }
            }
            PanelAction::HireArt(art) => {
                if building.is_some() && art < headhunter::GUIDS.len() {
                    self.hire = HireMenu {
                        building,
                        art: Some(art),
                        target_player: None,
                    };
                }
            }
            PanelAction::HirePlayer(target) => {
                if self.hire.art.is_some() && self.hire.building == building {
                    self.hire.target_player = Some(target);
                }
            }
            PanelAction::HireTarget(guid) => {
                // `Toolbar_SubmitHireHeadhunter` (`0x0044B700`): kind, target player, target GUID and the
                // selected building, unwrapped via the host; the simulation checks menu, lock,
                // target player and target again (`command::headhunter::check`).
                let HireMenu {
                    building: Some(b),
                    art: Some(art),
                    target_player: Some(target),
                } = self.hire
                else {
                    return;
                };
                if building != Some(b) {
                    return;
                }
                host.submit_own(CommandKind::HireHeadhunter {
                    art: art as i32,
                    target_player: i32::from(target),
                    target_guid: guid as i32,
                    building: b,
                });
                self.hire = HireMenu::default();
            }
            PanelAction::HireBack => {
                if self.hire.target_player.is_some() {
                    self.hire.target_player = None;
                } else {
                    self.hire = HireMenu::default();
                }
            }
        }
    }

    /// After every selection change: the confirmation when selling and the headhunter menu apply only
    /// to the selected building, the placement mode only with own selected objects.
    pub fn follow_selection(&mut self, host: &SimHost, selection: &[ObjectId]) {
        if self.sell_armed.is_some() && self.sell_armed != selection.first().copied() {
            self.sell_armed = None;
        }
        if self.placement.is_some() && host.own_objects(selection).is_empty() {
            self.placement = None;
        }
        // The headhunter menu applies only to the building in which it was opened.
        if self.hire.building.is_some() && self.hire.building != selection.first().copied() {
            self.hire = HireMenu::default();
        }
    }

    /// Left click in the placement mode at the terrain point `ground`: with a valid site send `0x01`
    /// (Shift/Ctrl wrap); without Shift the mode ends afterwards. Returns the preview that was used
    /// for the check (`None` without mode or hull).
    pub fn click_place(
        &mut self,
        host: &mut SimHost,
        selection: &[ObjectId],
        ground: Vec3,
        shift: bool,
        ctrl: bool,
    ) -> Option<PlacementPreview> {
        let p = self.placement?;
        let preview = host.placement_preview(p.guid, ground, p.steps, selection)?;
        if preview.valid
            && host.place_building(&preview, selection, queue_keys(shift, ctrl))
            && !shift
        {
            self.placement = None;
        }
        Some(preview)
    }
}

/// Overlay line of the right-click action under the mouse (check aid).
pub fn action_line(action: Option<&(i32, CommandKind)>) -> String {
    match action {
        Some((b, kind)) => format!(
            "Right-click: {b} {} (command {:#04x})",
            action_name(*b),
            kind.type_code() & 0xFF
        ),
        None => "Right-click: no action".into(),
    }
}

/// Overlay line of the placement mode; locked types ([`rotation_locked`]) show "Rotation
/// locked" instead of the rotation and without the hint on R.
pub fn placement_line(
    rules: &Rules,
    placement: &Placement,
    preview: Option<&PlacementPreview>,
) -> String {
    let name = type_name(rules, placement.guid);
    let (rotation, keys) = if rotation_locked(rules, placement.guid) {
        ("Rotation locked".to_string(), "Esc/Right-click: end")
    } else {
        (
            format!("Rotation {} degrees", u32::from(placement.steps) * 90),
            "R: rotate, Esc/Right-click: end",
        )
    };
    match preview {
        None => format!(
            "Build: {} {name}, {rotation} | no terrain point | {keys}",
            placement.guid
        ),
        Some(p) => {
            let cell = rules.map.cell;
            let tile = (
                Fx::from_f32(p.pos[0] / cell[0] as f32).to_f32(),
                Fx::from_f32(p.pos[2] / cell[2] as f32).to_f32(),
            );
            format!(
                "Build: {} {name}, {rotation}, position ({:.1}, {:.1}) = tile ({:.2}, {:.2}), {} | {keys}",
                placement.guid,
                p.pos[0],
                p.pos[2],
                tile.0,
                tile.1,
                if p.valid { "valid" } else { "invalid" }
            )
        }
    }
}

/// Test world of the economy (colonist, building, tree, gold, ship on a 64 x 64 map), also for
/// the tests of the viewer.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit {
    use super::*;
    use crate::game::options::DEFAULT_GAME_SPEED;
    use opennml_formats::gifcon::parse_toolbar;
    use opennml_formats::targets::{parse_target_defines, parse_targets};
    use opennml_sim::rules::{BuildingShape, ModelBox};
    use opennml_sim::setup::MatchSetup;
    use opennml_sim::state::Control;
    use opennml_sim::world::{ObjectDesc, create_object, desc_flags};

    pub const COLONIST: u32 = 1100;
    pub const HQ: u32 = 1000;
    pub const HOUSE: u32 = 1001;
    pub const STORE: u32 = 1003;
    pub const MINE: u32 = 1004;
    pub const WALL: u32 = 1011;
    pub const TREE: u32 = 3602;
    pub const GOLD: u32 = 3700;
    /// Transport ship: mobile container with quarters for 12.
    pub const SHIP: u32 = 1108;

    /// Colonist with walk, build, stand and chopping wood; HQ with quarters, house, store, gold mine
    /// (with the rotation lock `0x10000000` of the shipped data), wall, deciduous tree (values like the command tests
    /// of the simulation) and gold deposit (resource, module 6).
    pub const TYPES: &str = concat!(
        "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 0\n",
        "PROPID 0\nVALUEID 10001 6 1 3\nVALUEID 10003 6 1 264448\nVALUEID 10004 6 1 512\n",
        "VALUEID 10005 6 1 1024\nVALUEID 10006 6 1 200\nVALUEID 10007 6 1 1\nVALUEID 10008 6 1 4\n",
        "PROPID 1\nVALUEID 20001 6 1 5\nVALUEID 20002 6 1 0\nVALUEID 20003 6 1 7\n",
        "PROPID 2\nVALUEID 30001 1 1 0\n",
        "PROPID 8\nVALUEID 90001 6 1 4\nVALUEID 90002 1 1 5\nVALUEID 90005 6 1 10\n",
        "VALUEID 90006 6 1 3\nVALUEID 90007 6 1 -1\n",
        "GUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 14336\n",
        "PROPID 23\n",
        "GUID 1001\nNAME SPA_Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\n",
        "GUID 1003\nNAME SPA_Lagerhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 14336\n",
        "GUID 1004\nNAME SPA_Goldgrube\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 268435712\n",
        "GUID 1011\nNAME SPA_Mauer\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 3145728\n",
        "GUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\nVALUEID 50003 6 1 0\n",
        "GUID 3700\nNAME Goldvorkommen\nVALUEID 100 6 1 0\nPROPID 6\nVALUEID 70002 6 1 0\n",
        "GUID 1108\nNAME SPA_Transportschiff\nVALUEID 100 6 1 0\nPROPID 0\nVALUEID 10001 6 1 1\nVALUEID 10003 6 1 139776\n",
        "VALUEID 10004 6 1 2100\nVALUEID 10005 6 1 4000\nVALUEID 10006 6 1 7\nVALUEID 10007 6 1 1\n",
        "PROPID 2\nVALUEID 30001 6 1 0\nVALUEID 30003 6 1 256\nPROPID 23\n",
    );

    /// Colonist -> build menu 110 (HQ once only, house, store, gold mine needs a store, wall);
    /// HQ -> colonist, research 2806 (no type, as in the shipped data) and the button 600 without a kind, whose
    /// submenu contains the research 3354 (pattern of the research in `toolbar.ini`).
    pub const TOOLBAR: &str = concat!(
        "GUID 1100\nBUTTONID 0\nBUTTONMSG 110\nTARGET_DO_MSG\nBUTTONID 2\nBUTTONMSG 413\nTARGET_MSG\nENDGUID\n",
        "GUID 110\nBUTTONID 0\nBUTTONMSG 1000\nBUILD_MSG\nGROUP_AND_MSG\nONLYONCE_MSG\n",
        "BUTTONID 1\nBUTTONMSG 1001\nBUILD_MSG\nGROUP_AND_MSG\n",
        "BUTTONID 3\nBUTTONMSG 1003\nBUILD_MSG\nGROUP_AND_MSG\n",
        "BUTTONID 4\nBUTTONMSG 1004\nBUILD_MSG\nGROUP_AND_MSG\nREQUIRED_GUID 1003\n",
        "BUTTONID 10\nBUTTONMSG 1011\nBUILD_MSG\nGROUP_AND_MSG\nENDGUID\n",
        "GUID 1000\nBUTTONID 0\nBUTTONMSG 1100\nMAKE_MSG\nREQUIRED_GUID 1000\n",
        "BUTTONID 7\nBUTTONMSG 2806\nUPGRADE_MSG\nREQUIRED_GUID 1000\n",
        "BUTTONID 8\nBUTTONMSG 600\nENDGUID\n",
        "GUID 600\nBUTTONID 7\nBUTTONMSG 3354\nUPGRADE_MSG\nENDGUID\n",
    );

    pub const DEFINES: &str = "PROP_PTTOPT 0\nPROP_CONSTRUCT 1\nPROP_BUILDING 3\nPROP_TREE 4\nPROP_LUMBERING 8\nMID_PROP_BUILDING_CONSTRUCTED_FINISHED 1027\nMID_PROP_PTTOPT_SADDLE 32768\nMID_PROP_PTTOPT_CANQUARTER 262144\nMID_PROP_QUARTER_FREEFORQUARTER 279\n";

    /// Entries of `targets.ini` in file order: 400 Chop wood, 412 Build, 414 Quarter,
    /// 413 Repair, 447 Rally point, 448 Go to.
    pub const TARGETS: &str = concat!(
        "BEGIN\nBUTTONMSG 400\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_LUMBERING\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_STATUS ISMOUNTEDONBOAT\nMM_MODE MM_HL_MYLIST\nMM_END\n",
        "TARGET_MM\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_PROPID PROP_TREE\nMM_END\nEND\n",
        "BEGIN\nBUTTONMSG 412\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_CONSTRUCT\nMM_END\n",
        "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_CONSTRUCTED_FINISHED\nMM_MODE MM_CHECKNOTHANDLETYPE\nMM_MODE MM_HL_MYLIST\nMM_END\nEND\n",
        "BEGIN\nBUTTONMSG 414\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKHANDLETYPE\nMM_MODE MM_HL_MYLIST\nMM_MATCHBITS MID_PROP_PTTOPT_CANQUARTER\nMM_END\n",
        "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_QUARTER_FREEFORQUARTER\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_ANDNEXTMATCH\nMM_NEXT\nMM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_BUILDING_CONSTRUCTED_FINISHED\nMM_END\nEND\n",
        "BEGIN\nBUTTONMSG 413\nMY_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_CONSTRUCT\nMM_MODE MM_CHECKNOTSTATUS\nMM_STATUS ISMOUNTED\nMM_END\n",
        "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_MODE MM_HL_MYLIST\nMM_END\nEND\n",
        "BEGIN\nBUTTONMSG 447\nMY_MM\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\nMM_END\nTARGET_MM\nMM_END\nEND\n",
        "BEGIN\nBUTTONMSG 448\nMY_MM\nMM_MODE MM_CHECKNOTHANDLETYPE\nMM_MODE MM_CHECKPROPERTY\nMM_MODE MM_ANDALL\nMM_PROPID PROP_PTTOPT\nMM_MATCHBITS MID_PROP_PTTOPT_SADDLE\nMM_END\nTARGET_MM\nMM_END\nEND\n",
    );

    /// Rules on a 64 x 64 map: buildings 200 x 200 (about 2 x 2 tiles), wall 100 x 100.
    pub fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 64);
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        r.defaults.insert(
            COLONIST,
            set(&[(10, 60), (13, 15), (16, 183), (17, 20), (22, 1), (32, 60)]),
        );
        r.defaults
            .insert(HQ, set(&[(11, 500), (13, 90), (14, 4000), (23, 5)]));
        r.defaults
            .insert(HOUSE, set(&[(11, 150), (13, 20), (14, 900), (27, 15)]));
        r.defaults
            .insert(STORE, set(&[(11, 200), (13, 30), (14, 1500)]));
        r.defaults
            .insert(MINE, set(&[(11, 250), (13, 30), (14, 1200)]));
        r.defaults
            .insert(WALL, set(&[(11, 5), (13, 6), (14, 1000)]));
        r.defaults.insert(TREE, set(&[(25, 200)]));
        r.defaults
            .insert(SHIP, set(&[(14, 1400), (16, 183), (23, 12)]));
        // Research 2806 (10 s) and 3354 (20 s): entered as upgrades, with duration (config 13).
        r.defaults.insert(2806, set(&[(13, 10)]));
        r.defaults.insert(3354, set(&[(13, 20)]));
        r.upgrades.extend([2806, 3354]);
        for guid in [HQ, HOUSE, STORE, MINE, WALL] {
            let half = if guid == WALL { 50.0 } else { 100.0 };
            r.building_shapes.insert(
                guid,
                BuildingShape {
                    center: [0.0; 3],
                    half: [half, 50.0, half],
                    free_nodes: Vec::new(),
                    helper_nodes: Default::default(),
                },
            );
            r.model_boxes.insert(
                guid,
                ModelBox {
                    center: [0.0; 3],
                    half: [half, 50.0, half],
                },
            );
        }
        r.toolbar = parse_toolbar(TOOLBAR);
        r.targets = parse_targets(TARGETS, &parse_target_defines(DEFINES)).entries;
        r
    }

    /// Host with player 0 (human, 1000 gold/wood/food, living space 50), map revealed for all
    /// (the right-click tests see every target).
    pub fn host() -> SimHost {
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules(), setup, DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        host.state.players[0].control = Control::Human;
        host.state.players[0].resources_fx = [1000 << 12, 1000 << 12, 1000 << 12, 0, 50 << 12];
        opennml_sim::fog::reveal_map(&mut host.state, -1);
        host
    }

    /// World coordinate of the tile center.
    pub fn world(t: i32) -> f32 {
        t as f32 * 128.0 + 64.0
    }

    /// Finished object at the tile center `tile`.
    pub fn spawn(host: &mut SimHost, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
        let rules = host.rules.clone();
        let mut sim = Sim::new(&mut host.state, &rules);
        let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
        desc.flags |= desc_flags::PLACED;
        create_object(&mut sim, &mut desc).expect("object created")
    }

    /// Construction site at the tile center `tile`.
    pub fn site(host: &mut SimHost, guid: u32, slot: i32, tile: (i32, i32)) -> ObjectId {
        let rules = host.rules.clone();
        let mut sim = Sim::new(&mut host.state, &rules);
        let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
        create_object(&mut sim, &mut desc).expect("construction site created")
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use crate::game::options::DEFAULT_GAME_SPEED;
    use crate::game::selection::RightClickTarget::Object;
    use opennml_sim::save::checksum;

    fn ground(tile: (i32, i32)) -> Vec3 {
        Vec3::new(world(tile.0), 0.0, world(tile.1))
    }

    /// Ray test without fog memories (none is hit).
    fn no_memory(_: ObjectId, _: PickFlags, _: &mut PickDistances) -> bool {
        false
    }

    /// Commands of the last tick (after a step, from the command log).
    fn logged(host: &mut SimHost) -> Vec<CommandKind> {
        let before = host.log.len();
        host.step_once();
        host.log[before..]
            .iter()
            .map(|c| c.command.kind.clone())
            .collect()
    }

    #[test]
    fn right_clicks_issue_the_action_of_the_target_table_into_the_command_log() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let house_site = site(&mut host, HOUSE, 0, (20, 10));
        let enemy = spawn(&mut host, HOUSE, 1, (40, 40));
        let sel = [worker];
        // Worker on tree -> 400 / chop wood `0x06` to the selection.
        let r = host.issue_target_action(&sel, Some(Object(tree)), ground((14, 10)), None);
        assert_eq!(r.as_ref().map(|r| r.0), Some(400));
        assert_eq!(
            logged(&mut host),
            [CommandKind::ChopWood {
                tree,
                units: vec![worker]
            }]
        );
        // Own construction site -> 412 Build `0x02`; Shift queues (`0x7B`), Ctrl cyclic.
        let r = host.issue_target_action(
            &sel,
            Some(Object(house_site)),
            ground((20, 10)),
            Some(false),
        );
        assert_eq!(r.map(|r| r.0), Some(412));
        let construct = CommandKind::ConstructAt {
            building: house_site,
            units: vec![worker],
        };
        assert_eq!(
            logged(&mut host),
            [CommandKind::Queue {
                cyclic: false,
                inner: Box::new(construct)
            }]
        );
        // HQ with free quarters -> 414; Quarter stays direct despite Shift.
        let r = host.issue_target_action(&sel, Some(Object(hq)), ground((30, 30)), Some(false));
        assert_eq!(
            r.map(|r| r.1),
            Some(CommandKind::Quarter {
                container: hq,
                units: vec![worker]
            })
        );
        host.step_once();
        // Foreign building and ground -> 448 "Go to" with the ground point in fixed-point tiles.
        let to = Vec3::new(7.0 * 128.0 + 64.0, 0.0, 9.0 * 128.0 + 64.0);
        for target in [Some(enemy), None] {
            let r = host.issue_target_action(&sel, target.map(Object), to, None);
            assert_eq!(r.map(|r| r.0), Some(448));
            let [CommandKind::Move { units, x, z, .. }] = &logged(&mut host)[..] else {
                panic!("move expected");
            };
            assert_eq!(
                (units.clone(), *x, *z),
                (vec![worker], 7 << 12 | 0x800, 9 << 12 | 0x800)
            );
        }
        // Building selected + ground -> 447 rally point `0x27`, never wrapped.
        let r = host.issue_target_action(&[hq], None, to, Some(false));
        assert_eq!(r.map(|r| r.0), Some(447));
        assert_eq!(
            logged(&mut host),
            [CommandKind::SetRallyPoint {
                x: 7 << 12 | 0x800,
                z: 9 << 12 | 0x800,
                buildings: vec![hq]
            }]
        );
        // Ground actions outside the map (64 x 64 tiles) are discarded by the UI; the
        // action on an object stays.
        let outside = Vec3::new(-10.0, 0.0, 300.0);
        let beyond = Vec3::new(300.0, 0.0, 64.0 * 128.0 + 10.0);
        for to in [outside, beyond] {
            assert_eq!(host.issue_target_action(&sel, None, to, None), None);
            assert_eq!(host.issue_target_action(&[hq], None, to, None), None);
        }
        assert_eq!(
            host.issue_target_action(&sel, Some(Object(tree)), outside, None)
                .map(|r| r.0),
            Some(400)
        );
        // The rally point over a foreign building is a ground action: discarded.
        assert_eq!(
            host.issue_target_action(&[hq], Some(Object(enemy)), outside, None),
            None
        );
        logged(&mut host);
        // At the edge (x = 64 tiles) the point still counts.
        let edge = Vec3::new(64.0 * 128.0, 0.0, 300.0);
        assert_eq!(
            host.issue_target_action(&sel, None, edge, None)
                .map(|r| r.0),
            Some(448)
        );
        logged(&mut host);
        // Foreign or empty selection: no action, no command.
        assert_eq!(host.issue_target_action(&[enemy], None, to, None), None);
        assert_eq!(
            host.issue_target_action(&[], Some(Object(tree)), to, None),
            None
        );
        assert!(logged(&mut host).is_empty());
        assert_eq!(
            action_line(host.target_action(&sel, Some(tree), to).as_ref()),
            "Right-click: 400 Chop wood (command 0x06)"
        );
        assert_eq!(action_line(None), "Right-click: no action");
    }

    /// Ray above the roof of the town hall: It misses the town hall and hits the
    /// tree behind it. The right-click target (`0x00480E80`) is the tree -> chop wood; the click target
    /// (`0x0047A210`) knows no trees -> no object under the mouse, the left click selects nothing.
    #[test]
    fn a_right_click_above_the_roof_chops_the_tree_behind_the_town_hall() {
        use crate::game::selection::{Selection, click_target};
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let hq = spawn(&mut host, HQ, 0, (14, 10));
        let tree = spawn(&mut host, TREE, 9, (16, 10));
        let sel = [worker];
        // Render list with "does the ray hit?" per object.
        let hit = |hit: bool, _, _: &mut opennml_render::pick::PickDistances| hit;
        let over_roof = [(worker, false), (hq, false), (tree, true)];
        let target = host.right_click_target(&sel, &over_roof, hit, no_memory);
        assert_eq!(target, Some(Object(tree)));
        assert_eq!(
            host.right_click_action(&sel, target, ground((16, 10)))
                .map(|r| r.0),
            Some(400)
        );
        assert_eq!(click_target(&host.state, &over_roof, hit), None);
        let mut selection = Selection::of(sel.to_vec());
        let rules = host.rules.clone();
        selection.click(&host.state, &rules, |_| true, 0, None, false);
        assert!(selection.ids.is_empty());
        // On the roof: both take the town hall (rank 4 before the tree), right-click Quarter;
        // by left click it is selectable.
        let on_roof = [(worker, false), (hq, true), (tree, true)];
        let target = host.right_click_target(&sel, &on_roof, hit, no_memory);
        assert_eq!(target, Some(Object(hq)));
        assert_eq!(
            host.right_click_action(&sel, target, ground((14, 10)))
                .map(|r| r.0),
            Some(414)
        );
        let clicked = click_target(&host.state, &on_roof, hit);
        assert_eq!(clicked, Some(hq));
        selection.click(&host.state, &rules, |_| true, 0, clicked, false);
        assert_eq!(selection.ids, [hq]);
    }

    /// Falling through of the right-click target (`Input_PickTargetAction` `0x00480E80`): If the last
    /// accepted hit of a rank has no action (`0x00480600`), the search continues with its distances in the
    /// next rank. A tree in front of a foreign store: rank 4 takes the store (no action),
    /// rank 5 the nearer tree -> chop wood. The store in front of the tree: the tree behind lies behind
    /// `d1` and is rejected -> ground "Go to".
    #[test]
    fn a_right_click_falls_through_a_building_without_action() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let enemy_store = spawn(&mut host, STORE, 1, (14, 10));
        let tree = spawn(&mut host, TREE, 9, (16, 10));
        let sel = [worker];
        // Triangle hit `(object, t)`: accepted only if `t < d1`.
        let triangle = |t: f32, _, d: &mut opennml_render::pick::PickDistances| {
            if d.d1 <= t {
                return false;
            }
            d.d1 = t;
            d.d2 = 0.0;
            true
        };
        let tree_in_front = [(worker, f32::INFINITY), (tree, 0.5), (enemy_store, 1.0)];
        let target = host.right_click_target(&sel, &tree_in_front, triangle, no_memory);
        assert_eq!(target, Some(Object(tree)));
        assert_eq!(
            host.right_click_action(&sel, target, ground((16, 10)))
                .map(|r| r.0),
            Some(400)
        );
        let store_in_front = [(worker, f32::INFINITY), (enemy_store, 0.5), (tree, 1.0)];
        let target = host.right_click_target(&sel, &store_in_front, triangle, no_memory);
        assert_eq!(target, None);
        assert_eq!(
            host.right_click_action(&sel, target, ground((14, 10)))
                .map(|r| r.0),
            Some(448)
        );
        // Without a selection no object has an action: all ranks fall through.
        assert_eq!(
            host.right_click_target(&[], &tree_in_front, triangle, no_memory),
            None
        );
    }

    #[test]
    fn queue_keys_wrap_only_the_actions_the_original_wraps_and_shift_wins() {
        assert_eq!(queue_keys(true, true), Some(false));
        assert_eq!(queue_keys(false, true), Some(true));
        assert_eq!(queue_keys(false, false), None);
        for b in [398, 400, 401, 402, 412, 413, 569, 444, 445, 446, 448] {
            assert!(wraps_with_queue_keys(b), "{b}");
        }
        for b in [414, 447, 427] {
            assert!(!wraps_with_queue_keys(b), "{b}");
        }
    }

    // QUIRK W-195: kept
    #[test]
    fn mount_and_board_ignore_the_queue_keys() {
        // W-195: Mount (403) and Board (404) go directly to the network like Quarter (414);
        // Shift and Ctrl do not queue (`Input_IssueTargetAction` `0x0044D350`).
        for b in [button::MOUNT_HORSE, button::BOARD_BOAT, button::QUARTER] {
            assert!(!wraps_with_queue_keys(b), "{b}");
        }
        assert_eq!((button::MOUNT_HORSE, button::BOARD_BOAT), (403, 404));
    }

    #[test]
    fn the_build_list_of_a_colonist_shows_costs_and_locks() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let list = host.build_menu(&[worker]);
        let guids: Vec<u32> = list.iter().map(|e| e.guid).collect();
        assert_eq!(guids, [HQ, HOUSE, STORE, MINE, WALL]);
        let get = |list: &[MenuEntry], g| list.iter().find(|e| e.guid == g).unwrap().clone();
        let house = get(&list, HOUSE);
        assert_eq!(house.name, "SPA_Wohnhaus");
        assert_eq!(house.kind, button_kind::BUILD);
        assert_eq!(house.costs.map(|c| c[resource::WOOD]), Some(150));
        assert_eq!(cost_text(house.costs), "Wood 150");
        assert!(house.affordable && house.blocked.is_none());
        // Gold mine needs a store.
        assert_eq!(
            get(&list, MINE).blocked.as_deref(),
            Some("prerequisite for 1004 missing")
        );
        assert_eq!(get(&list, HQ).kind, button_kind::ONLYONCE);
        assert!(get(&list, HQ).blocked.is_none());
        // With a store the gold mine is free, with an HQ the HQ is locked; 500 wood for the HQ are missing at 400.
        spawn(&mut host, STORE, 0, (30, 10));
        spawn(&mut host, HQ, 0, (30, 30));
        host.state.players[0].resources_fx[resource::WOOD] = 400 << 12;
        let list = host.build_menu(&[worker]);
        assert!(get(&list, MINE).blocked.is_none());
        let hq = get(&list, HQ);
        assert_eq!(hq.blocked.as_deref(), Some("1000 exists only once"));
        assert!(!hq.affordable);
        // Foreign or no workers: empty list.
        let foreign = spawn(&mut host, COLONIST, 1, (12, 12));
        assert!(host.build_menu(&[foreign]).is_empty());
        assert!(host.build_menu(&[]).is_empty());
    }

    /// Like the female settler (2602, buttons 260/261 without a kind): a worker whose build menu button has
    /// no kind shows the build list of this submenu - the same rule as the command check,
    /// which allows the construction (previously only `TARGET_DO`: the list was empty).
    #[test]
    fn a_worker_with_a_kindless_menu_button_shows_the_build_list() {
        let mut host = host();
        for b in host
            .rules
            .toolbar
            .menus
            .get_mut(&(COLONIST as i32))
            .unwrap()
        {
            if b.msg == 110 {
                b.flags &= !button_kind::KIND_MASK;
            }
        }
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let guids: Vec<u32> = host.build_menu(&[worker]).iter().map(|e| e.guid).collect();
        assert_eq!(guids, [HQ, HOUSE, STORE, MINE, WALL]);
        assert_eq!(
            check_build(&host.sim_context(), 0, HOUSE, &[worker]),
            Ok(())
        );
    }

    #[test]
    fn placement_snaps_checks_rotates_and_places_with_the_snapped_position() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        spawn(&mut host, HQ, 0, (30, 30));
        let sel = [worker];
        // Hull -100..100: minimum corner 900 -> tile boundary 896, position 996.
        let p = host
            .placement_preview(HOUSE, Vec3::new(1000.0, 0.0, 1000.0), 0, &sel)
            .unwrap();
        assert!(p.valid);
        assert_eq!((p.pos[0], p.pos[2], p.rot), (996.0, 996.0, 0.0));
        assert_eq!(p.rect, Some([7, 7, 8, 8]));
        assert_eq!(p.hull[0], [896.0, 896.0]);
        // Invalid on the HQ.
        let blocked = host
            .placement_preview(HOUSE, ground((30, 30)), 0, &sel)
            .unwrap();
        assert!(!blocked.valid);
        // Rotating in quarters.
        let mut ui = EconomyUi::default();
        ui.apply(&mut host, &sel, PanelAction::Build(HOUSE));
        assert_eq!(
            ui.placement,
            Some(Placement {
                guid: HOUSE,
                steps: 0
            })
        );
        for _ in 0..5 {
            ui.rotate(&host.rules);
        }
        assert_eq!(ui.placement.unwrap().steps, 1);
        let turned = host
            .placement_preview(HOUSE, Vec3::new(1000.0, 0.0, 1000.0), 1, &sel)
            .unwrap();
        assert_eq!(turned.rot, ROTATION_STEP);
        assert!(turned.valid);
        // Invalid click: nothing placed, the mode stays.
        let r = ui.click_place(&mut host, &sel, ground((30, 30)), false, false);
        assert!(!r.unwrap().valid);
        assert!(ui.placement.is_some());
        assert!(logged(&mut host).is_empty());
        // Shift+click: placed (queued), the mode stays; a click without Shift ends it.
        let r = ui
            .click_place(&mut host, &sel, Vec3::new(1000.0, 0.0, 1000.0), true, false)
            .unwrap();
        assert!(ui.placement.is_some());
        let expected = CommandKind::PlaceBuilding {
            guid: HOUSE,
            rotation: F32Bits::new(r.rot),
            x: F32Bits::new(r.pos[0]),
            y: F32Bits::new(r.pos[1]),
            z: F32Bits::new(r.pos[2]),
            workers: vec![worker],
            border: None,
        };
        assert_eq!(
            logged(&mut host),
            [CommandKind::Queue {
                cyclic: false,
                inner: Box::new(expected)
            }]
        );
        ui.click_place(&mut host, &sel, ground((20, 20)), false, false);
        assert_eq!(ui.placement, None);
        let [CommandKind::PlaceBuilding { guid, x, .. }] = &logged(&mut host)[..] else {
            panic!("place building expected");
        };
        // Rotated, the rotated minimum corner (100, -100) snaps: 2724 -> 2688, position 2588.
        assert_eq!((*guid, x.get()), (HOUSE, 2588.0));
        // Both construction sites stand (wood 1000 - 2 * 150).
        let sites = host
            .state
            .objects()
            .filter(|o| o.guid == HOUSE && o.owner() == 0)
            .count();
        assert_eq!(sites, 2);
        assert_eq!(host.state.resource(0, resource::WOOD), 700);
        // Esc ends the mode; without an own selection it ends as well.
        ui.apply(&mut host, &sel, PanelAction::Build(WALL));
        assert!(ui.cancel_placement());
        assert!(!ui.cancel_placement());
        ui.apply(&mut host, &sel, PanelAction::Build(WALL));
        ui.follow_selection(&host, &[]);
        assert_eq!(ui.placement, None);
        assert!(
            placement_line(
                &host.rules,
                &Placement {
                    guid: HOUSE,
                    steps: 1
                },
                Some(&turned)
            )
            .starts_with("Build: 1001 SPA_Wohnhaus, Rotation 90 degrees, position (")
        );
    }

    #[test]
    fn r_does_not_turn_rotation_locked_types_and_the_overlay_says_so() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let sel = [worker];
        let mut ui = EconomyUi::default();
        // Gold mine with building flag `0x10000000`: R does not change the rotation counter.
        ui.apply(&mut host, &sel, PanelAction::Build(MINE));
        ui.rotate(&host.rules);
        assert_eq!(
            ui.placement,
            Some(Placement {
                guid: MINE,
                steps: 0
            })
        );
        let preview = host.placement_preview(MINE, ground((12, 12)), 0, &sel);
        let line = placement_line(&host.rules, &ui.placement.unwrap(), preview.as_ref());
        assert!(line.contains("Rotation locked"), "{line}");
        assert!(!line.contains("R: rotate"), "{line}");
        // The house keeps rotating.
        ui.apply(&mut host, &sel, PanelAction::Build(HOUSE));
        ui.rotate(&host.rules);
        assert_eq!(ui.placement.unwrap().steps, 1);
        let line = placement_line(&host.rules, &ui.placement.unwrap(), None);
        assert!(
            line.contains("Rotation 90 degrees") && line.contains("R: rotate"),
            "{line}"
        );
    }

    #[test]
    fn research_names_come_from_the_guid_texts() {
        // Research is not a type. Fallback trade -> type name ->
        // `guids.deu` -> "Type n". Synthetic text table (no original data); the table takes
        // only upgrades and the buttons UPGRADE/COUNTERUPGRADE.
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let names = |host: &mut SimHost| -> Vec<String> {
            host.production_menu(hq)
                .into_iter()
                .map(|e| e.name)
                .collect()
        };
        assert_eq!(names(&mut host), ["SPA_Kolonist", "Type 2806", "Type 3354"]);
        let table = [
            (1100, "Colonist from guids"),
            (2806, "Arm[CR]research [KEYID 7]"),
            (3354, "Develop"),
            (9999, "foreign"),
        ]
        .into_iter()
        .map(|(k, v)| (k, v.to_string()))
        .collect();
        host.upgrade_texts = crate::game::texts::upgrade_texts(table, &host.rules);
        assert_eq!(
            host.upgrade_texts.keys().copied().collect::<Vec<_>>(),
            [2806, 3354],
            "only upgrades and research buttons"
        );
        assert_eq!(
            names(&mut host),
            ["SPA_Kolonist", "Arm research", "Develop"]
        );
        // Control channel `sim_menu` and queue (`sim_train` enqueues) show the same names.
        let menu = host.control_menu(None, Some(hq)).unwrap();
        let menu_names: Vec<&str> = menu.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(menu_names, ["SPA_Kolonist", "Arm research", "Develop"]);
        host.control_train(hq, 2806).unwrap();
        host.control_train(hq, 3354).unwrap();
        host.step_once();
        let queue: Vec<String> = host
            .production_queue(hq)
            .into_iter()
            .map(|q| q.name)
            .collect();
        assert_eq!(queue, ["Arm research", "Develop"]);
    }

    #[test]
    fn production_list_queue_cancel_sell_and_unquarter_issue_their_commands() {
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let sel = [hq];
        let model = host.panel(&sel);
        assert!(model.build.is_empty());
        assert!(model.account.starts_with("Balance player 0: Gold 1000"));
        let panel = model.building.expect("building part");
        assert_eq!(panel.name, "SPA_Hauptquartier");
        assert!(panel.finished && !panel.dead);
        assert_eq!(panel.quarter, Some((0, 5)));
        let guids: Vec<u32> = panel.production.iter().map(|e| e.guid).collect();
        assert_eq!(guids, [COLONIST, 2806, 3354]);
        assert!(panel.production.iter().all(|e| e.blocked.is_none()));
        let mut ui = EconomyUi::default();
        ui.apply(&mut host, &sel, PanelAction::Produce(COLONIST));
        ui.apply(&mut host, &sel, PanelAction::Produce(COLONIST));
        assert_eq!(
            logged(&mut host),
            vec![
                CommandKind::EnqueueProduction {
                    building: hq,
                    guid: COLONIST
                };
                2
            ]
        );
        // Running (index 0) and one waiting (index 1).
        for _ in 0..4 {
            host.step_once();
        }
        let queue = host.production_queue(hq);
        assert_eq!(
            queue.iter().map(|q| (q.index, q.guid)).collect::<Vec<_>>(),
            [(0, COLONIST), (1, COLONIST)]
        );
        assert!(queue[0].progress.is_some() && queue[1].progress.is_none());
        ui.apply(&mut host, &sel, PanelAction::Cancel(1));
        assert_eq!(
            logged(&mut host),
            [CommandKind::CancelProduction {
                building: hq,
                index: 1,
                arg: 0
            }]
        );
        assert_eq!(host.production_queue(hq).len(), 1);
        // Letting out.
        ui.apply(&mut host, &sel, PanelAction::UnquarterAll);
        let unit = ObjectId(0x10_0001);
        ui.apply(&mut host, &sel, PanelAction::Unquarter(unit));
        assert_eq!(
            logged(&mut host),
            [
                CommandKind::UnquarterAll { container: hq },
                CommandKind::Unquarter {
                    container: hq,
                    unit
                }
            ]
        );
        // Selling only on the second click; another click in between asks anew.
        ui.apply(&mut host, &sel, PanelAction::Sell);
        assert_eq!(ui.sell_armed, Some(hq));
        assert!(logged(&mut host).is_empty());
        ui.apply(&mut host, &sel, PanelAction::Cancel(0));
        assert_eq!(ui.sell_armed, None);
        host.step_once();
        ui.apply(&mut host, &sel, PanelAction::Sell);
        ui.apply(&mut host, &sel, PanelAction::Sell);
        assert_eq!(
            logged(&mut host),
            [CommandKind::SellBuildings {
                buildings: vec![hq]
            }]
        );
        // Foreign building: no building part, no commands.
        let enemy = spawn(&mut host, HQ, 1, (50, 50));
        assert!(host.panel(&[enemy]).building.is_none());
        ui.apply(&mut host, &[enemy], PanelAction::Produce(COLONIST));
        assert!(logged(&mut host).is_empty());
    }

    // QUIRK W-223: deviation
    #[test]
    fn the_hq_lists_research_of_submenu_600() {
        let mut host = host();
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        let sel = [hq];
        // 3354 is in the submenu of the button 600 (without a kind, one level deep) and goes
        // through the command check of the training like 2806.
        let list = host.production_menu(hq);
        let kinds: Vec<(u32, u32)> = list.iter().map(|e| (e.guid, e.kind)).collect();
        assert_eq!(
            kinds,
            [
                (COLONIST, button_kind::MAKE),
                (2806, button_kind::UPGRADE),
                (3354, button_kind::UPGRADE)
            ]
        );
        assert!(list.iter().all(|e| e.blocked.is_none()));
        let mut ui = EconomyUi::default();
        ui.apply(&mut host, &sel, PanelAction::Produce(3354));
        assert_eq!(
            logged(&mut host),
            [CommandKind::EnqueueProduction {
                building: hq,
                guid: 3354
            }]
        );
        // A missing prerequisite locks the row with the reason of the command check.
        host.rules.toolbar.menus.get_mut(&600).unwrap()[0].required[0] = 4711;
        let list = host.production_menu(hq);
        assert_eq!(
            list.iter().find(|e| e.guid == 3354).unwrap().blocked,
            Some("prerequisite for 3354 missing".to_string())
        );
        // A lock of the level data (config 1 = 0) hides the row.
        host.rules
            .level
            .slot_configs
            .entry(0)
            .or_default()
            .insert(3354, [(1, 0)].into_iter().collect());
        let guids: Vec<u32> = host.production_menu(hq).iter().map(|e| e.guid).collect();
        assert_eq!(guids, [COLONIST, 2806]);
        // The command check rejects it anyway, with the reason.
        let sim = host.sim_context();
        assert_eq!(
            check_production(&sim, 0, hq, 3354),
            Err("3354 is locked on this map".to_string())
        );
    }

    #[test]
    fn ship_occupants_are_listed_and_can_leave() {
        let mut host = host();
        let ship = spawn(&mut host, SHIP, 0, (30, 30));
        let first = spawn(&mut host, COLONIST, 0, (10, 10));
        let second = spawn(&mut host, COLONIST, 0, (12, 10));
        {
            let o = host.state.object_mut(ship, false).unwrap();
            let Some(ModuleKind::Quarter(q)) = o.module_mut(quarter::INDEX).map(|m| &mut m.kind)
            else {
                panic!("quarter expected");
            };
            q.occupants = vec![first, second];
            q.cur = 2;
        }
        // Own ship selected: panel with both occupants, without production, queue, sale.
        let sel = [ship];
        let panel = host.panel(&sel).building.expect("container part");
        assert_eq!(panel.id, ship);
        assert_eq!(panel.name, "SPA_Transportschiff");
        assert!(!panel.is_building && !panel.dead);
        assert_eq!(panel.quarter, Some((2, 12)));
        assert_eq!(
            panel.occupants,
            [
                (first, "SPA_Kolonist".to_string()),
                (second, "SPA_Kolonist".to_string())
            ]
        );
        assert!(panel.production.is_empty() && panel.queue.is_empty());
        // "Let out all" and "Let out" send the commands to the ship.
        let mut ui = EconomyUi::default();
        ui.apply(&mut host, &sel, PanelAction::UnquarterAll);
        ui.apply(&mut host, &sel, PanelAction::Unquarter(second));
        assert_eq!(
            logged(&mut host),
            [
                CommandKind::UnquarterAll { container: ship },
                CommandKind::Unquarter {
                    container: ship,
                    unit: second
                }
            ]
        );
        // Selling exists only for buildings.
        ui.apply(&mut host, &sel, PanelAction::Sell);
        ui.apply(&mut host, &sel, PanelAction::Sell);
        assert_eq!(ui.sell_armed, None);
        assert!(logged(&mut host).is_empty());
        // Foreign ship: no panel and no commands; neither for an object without quarters and without
        // building module.
        let enemy = spawn(&mut host, SHIP, 1, (40, 40));
        assert!(host.panel(&[enemy]).building.is_none());
        ui.apply(&mut host, &[enemy], PanelAction::UnquarterAll);
        assert!(logged(&mut host).is_empty());
        assert!(host.panel(&[first]).building.is_none());
        // Display and commands leave the checksum unchanged.
        let sum = checksum(&host.state);
        host.panel(&sel);
        assert_eq!(checksum(&host.state), sum);
    }

    #[test]
    fn previews_and_target_queries_leave_the_checksum_unchanged() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let hq = spawn(&mut host, HQ, 0, (30, 30));
        host.step_once();
        let before = checksum(&host.state);
        let bytes = host.save().to_bytes();
        for sel in [vec![worker], vec![hq]] {
            for target in [Some(tree), Some(hq), Some(worker), None] {
                host.target_action(&sel, target, ground((5, 5)));
            }
            host.panel(&sel);
            for steps in 0..4 {
                host.placement_preview(HOUSE, ground((12, 12)), steps, &sel);
                host.placement_preview(MINE, ground((30, 30)), steps, &sel);
            }
        }
        assert_eq!(checksum(&host.state), before);
        assert!(host.state.events.is_empty(), "{:?}", host.state.events);
        assert!(host.save().to_bytes() == bytes, "savegame changed");
    }

    /// On a real map with the real `targets.ini`/`toolbar.ini` (`[2players]-tintebariver`:
    /// harbor, fishing baskets, gold, trees, walls of the build list): every own object of every player
    /// as a selection against one object of each type and owner as target, the ground inside and
    /// outside the map, window and build previews of all types of the build list at the positions of these
    /// objects in all rotations. The filters send queries (`0x3100`, `0x329`, `0x1C17`,
    /// `0x603`, `0x1311`, `0x2C03`), the build site check looks for walls, deposits and shores: the
    /// savegame stays byte-identical, and no query leaves an event (which would otherwise trigger e.g. a
    /// message in the next tick).
    #[test]
    fn previews_and_target_queries_leave_a_real_match_unchanged() {
        use std::collections::BTreeSet;
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let lvl = "data/levels/multiplayer/[2players]-tintebariver.lvl";
        let scene = opennml_world::level::load_level(&vfs, lvl).unwrap();
        let mut host = crate::game::sim_host::start_sim(
            &vfs,
            &scene,
            DEFAULT_GAME_SPEED,
            crate::game::sim_host::testkit::default_setup,
        )
        .unwrap()
        .unwrap();
        for _ in 0..4 {
            host.step_once();
        }
        let before = host.save().to_bytes();
        let mut seen = BTreeSet::new();
        let reps: Vec<(ObjectId, u8, Vec3)> = host
            .state
            .objects()
            .filter(|o| seen.insert((o.guid, o.owner())))
            .map(|o| (o.id, o.owner(), Vec3::from(o.world_pos)))
            .collect();
        let outside = Vec3::new(-500.0, 0.0, -500.0);
        let active = host.active_player;
        let (mut actions, mut previews) = (0, 0);
        for player in 0..8u8 {
            host.active_player = player;
            for &(sel, _, at) in reps.iter().filter(|r| r.1 == player) {
                let sel = [sel];
                for &(target, _, pos) in &reps {
                    actions += usize::from(host.target_action(&sel, Some(target), pos).is_some());
                }
                for ground in [at + Vec3::new(300.0, 0.0, 0.0), outside] {
                    actions += usize::from(host.target_action(&sel, None, ground).is_some());
                }
                host.panel(&sel);
                for entry in host.build_menu(&sel) {
                    for &(_, _, pos) in &reps {
                        for steps in 0..4 {
                            let p = host.placement_preview(entry.guid, pos, steps, &sel);
                            previews += usize::from(p.is_some());
                        }
                    }
                }
            }
        }
        host.active_player = active;
        eprintln!(
            "tintebariver: {} targets, {actions} actions, {previews} previews",
            reps.len()
        );
        assert!(actions > 0 && previews > 0, "{actions} {previews}");
        assert!(host.state.events.is_empty(), "{:?}", host.state.events);
        assert!(host.save().to_bytes() == before, "savegame changed");
    }

    #[test]
    fn fog_off_frees_the_view_but_not_the_right_click() {
        let mut host = host();
        host.state.fog = opennml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        let w = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (40, 40));
        for _ in 0..3 {
            host.step_once();
        }
        let ground = Vec3::new(40.5 * 128.0, 0.0, 40.5 * 128.0);
        host.fog.show_mode = crate::game::fog_view::FOG_OFF;
        let o = host.state.object(tree, false).unwrap().clone();
        assert!(host.fog_selectable(&host.state.object(w, false).unwrap().clone()));
        assert_ne!(
            host.entry_look(tree, 0).unwrap().bits,
            0,
            "fog off: tree visible"
        );
        // Target choice and right-click still see the real fog.
        let (b, _) = host.target_action(&[w], Some(o.id), ground).unwrap();
        assert_eq!(b, opennml_sim::targeting::button::GO_TO);
    }

    /// Right-click at the last rank on an active fog memory (`0x00480E80` ->
    /// `0x0047D770`): the tree under the fog is not hit (render bits), its memory is; the
    /// action comes from the snapshot, the command goes to the real ID. Queries leave
    /// the checksum unchanged.
    #[test]
    fn a_right_click_on_a_memory_chops_the_remembered_tree() {
        let mut host = host();
        host.state.fog = opennml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        let w = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (40, 40));
        host.step_once();
        // Seen once (snapshot), then only explored: the memory is active.
        host.state.fog.seen.or(40, 40, 1);
        host.update_memories();
        host.state.fog.seen.clear();
        host.state.fog.explored.or(40, 40, 1);
        host.update_memories();
        assert!(host.memories.get(tree).unwrap().active);
        let before = checksum(&host.state);
        let sel = [w];
        let objects = [(tree, ())];
        let target =
            host.right_click_target(&sel, &objects, |_, _, _| false, |id, _, _| id == tree);
        assert_eq!(target, Some(RightClickTarget::Memory(tree)));
        assert_eq!(
            host.right_click_action(&sel, target, ground((40, 40)))
                .map(|r| r.0),
            Some(button::CHOP_WOOD)
        );
        // Without a selection no action: the memory falls through, the ground remains.
        assert_eq!(
            host.right_click_target(&[], &objects, |_, _, _| false, |_, _, _| true),
            None
        );
        // The real object offers nothing under fog (`MM_CHECKFOG`): ground "Go to".
        assert_eq!(
            host.target_action(&sel, Some(tree), ground((40, 40)))
                .map(|r| r.0),
            Some(button::GO_TO)
        );
        assert_eq!(checksum(&host.state), before, "queries change nothing");
        let r = host.issue_target_action(&sel, target, ground((40, 40)), None);
        assert_eq!(r.map(|r| r.0), Some(button::CHOP_WOOD));
        assert_eq!(
            logged(&mut host),
            [CommandKind::ChopWood {
                tree,
                units: vec![w]
            }]
        );
    }

    /// `0x0047D770` checks only the filter: A memory whose first matching action has (as yet) no
    /// command is the right-click target; the click then stays without a command instead of falling through to
    /// the ground ("Go to") - like with the visible object.
    #[test]
    fn a_memory_with_an_action_without_command_stays_the_target() {
        let mut host = host();
        // Before chopping wood an entry with the same filter that produces no command.
        let mut entry = host.rules.targets[0].clone();
        assert_eq!(entry.button_msg, button::CHOP_WOOD);
        // 0x1FF: a number without a command (0x19B is now the action 411 "Attack").
        entry.button_msg = 0x1FF;
        host.rules.targets.insert(0, entry);
        host.state.fog = opennml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        let w = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (40, 40));
        host.step_once();
        host.state.fog.seen.or(40, 40, 1);
        host.update_memories();
        host.state.fog.seen.clear();
        host.state.fog.explored.or(40, 40, 1);
        host.update_memories();
        let before = checksum(&host.state);
        let sel = [w];
        let target =
            host.right_click_target(&sel, &[(tree, ())], |_, _, _| false, |id, _, _| id == tree);
        assert_eq!(target, Some(RightClickTarget::Memory(tree)));
        assert_eq!(
            host.right_click_action(&sel, target, ground((40, 40))),
            None
        );
        assert_eq!(checksum(&host.state), before);
        assert_eq!(
            host.issue_target_action(&sel, target, ground((40, 40)), None),
            None
        );
        assert!(logged(&mut host).is_empty(), "no \"Go to\"");
    }

    /// Click and box select only what the fog check (`0x00467D70`) passes; "Fog off"
    /// frees the selection.
    #[test]
    fn units_in_the_fog_cannot_be_selected() {
        use crate::game::selection::Selection;
        let mut host = host();
        host.state.fog = opennml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        let w = spawn(&mut host, COLONIST, 0, (10, 10));
        let enemy = spawn(&mut host, COLONIST, 1, (40, 40));
        let rules = host.rules.clone();
        let mut sel = Selection::default();
        let fog = |o: &opennml_sim::object::GameObject| host.fog_selectable(o);
        sel.click(&host.state, &rules, fog, 0, Some(enemy), false);
        sel.rect(&host.state, &rules, fog, 0, |_| true, false);
        assert!(sel.ids.is_empty());
        host.state.fog.seen.or(10, 10, 1);
        host.state.fog.seen.or(40, 40, 1);
        let fog = |o: &opennml_sim::object::GameObject| host.fog_selectable(o);
        sel.rect(&host.state, &rules, fog, 0, |_| true, false);
        assert_eq!(sel.ids, [w]);
        sel.click(&host.state, &rules, fog, 0, Some(enemy), false);
        assert_eq!(sel.ids, [enemy]);
        host.state.fog.seen.clear();
        host.fog.show_mode = crate::game::fog_view::FOG_OFF;
        let fog = |o: &opennml_sim::object::GameObject| host.fog_selectable(o);
        sel.click(&host.state, &rules, fog, 0, Some(w), false);
        assert_eq!(sel.ids, [w], "fog off: selectable");
    }

    /// The preview is valid only on seen tiles (`0x0044FDB0`); "Fog off" frees it,
    /// but the command checks the sight of the sender itself without view switch and rejects.
    #[test]
    fn the_preview_is_valid_only_on_seen_tiles() {
        let mut host = host();
        host.state.fog = opennml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        // Sight range 32 (attribute 18) as in the fog tests of `opennml-sim`: 224 tiles seen.
        host.rules
            .defaults
            .get_mut(&COLONIST)
            .unwrap()
            .insert(18, 32);
        let w = spawn(&mut host, COLONIST, 0, (10, 10));
        for _ in 0..3 {
            host.step_once();
        }
        let at = |t: f32| Vec3::new(t * 128.0, 0.0, 10.5 * 128.0);
        let near = host.placement_preview(HOUSE, at(14.0), 0, &[w]).unwrap();
        let far = host.placement_preview(HOUSE, at(40.0), 0, &[w]).unwrap();
        assert!(near.valid && !far.valid);
        host.fog.show_mode = crate::game::fog_view::FOG_OFF;
        let far = host.placement_preview(HOUSE, at(40.0), 0, &[w]).unwrap();
        assert!(far.valid, "fog off: the preview is free");
        // The command checks the sight of the sender without view switch and rejects.
        assert!(host.place_building(&far, &[w], None));
        host.step_once();
        assert!(host.state.objects().all(|o| o.guid != HOUSE));
    }

    /// The preview of a wall segment requires the seen tile like every build preview
    /// (`BuildCursor_UpdatePlacement` `0x0044FDB0` has no wall exception); explored is not enough.
    /// With "Fog off" it counts as seen.
    #[test]
    fn a_wall_segment_preview_needs_a_seen_tile() {
        let mut host = host();
        host.state.fog = opennml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        let w = spawn(&mut host, COLONIST, 0, (10, 10));
        let wall = |host: &mut SimHost| {
            host.placement_preview(WALL, ground((30, 30)), 0, &[w])
                .unwrap()
                .valid
        };
        assert!(!wall(&mut host), "unexplored");
        for z in 28..=32 {
            for x in 28..=32 {
                host.state.fog.explored.or(x, z, 1);
            }
        }
        assert!(!wall(&mut host), "explored, but not seen");
        host.fog.show_mode = crate::game::fog_view::FOG_OFF;
        assert!(wall(&mut host), "fog off: seen");
        host.fog.show_mode = 0;
        opennml_sim::fog::reveal_map(&mut host.state, 0);
        assert!(wall(&mut host), "seen");
    }

    #[test]
    fn combat_actions_have_names() {
        assert_eq!(action_name(button::GARRISON_ATTACK), "Garrison attack");
        assert_eq!(action_name(button::ATTACK_UNIT), "Attack");
        assert_eq!(action_name(button::ATTACK), "Attack");
    }

    #[test]
    fn ctrl_sets_the_attack_modifier_4_only_on_the_attack_action_411() {
        let attack = |mods| CommandKind::Attack {
            units: vec![ObjectId(1)],
            target: ObjectId(2),
            pos: None,
            mods,
        };
        // Ctrl (re/commands 1.5/1.7): modifier 4, an existing one stays.
        assert_eq!(
            with_modifiers(button::ATTACK_UNIT, attack(0), true, false),
            attack(4)
        );
        assert_eq!(
            with_modifiers(button::ATTACK_UNIT, attack(1), true, false),
            attack(5)
        );
        assert_eq!(
            with_modifiers(button::ATTACK_UNIT, attack(0), false, false),
            attack(0)
        );
        // Other actions and commands stay untouched.
        assert_eq!(
            with_modifiers(button::GARRISON_ATTACK, attack(0), true, false),
            attack(0)
        );
        let chop = CommandKind::ChopWood {
            tree: ObjectId(2),
            units: vec![ObjectId(1)],
        };
        assert_eq!(
            with_modifiers(button::CHOP_WOOD, chop.clone(), true, false),
            chop
        );
    }

    // QUIRK D-227: kept
    #[test]
    fn the_run_modifier_reaches_go_to_and_attack_only() {
        let go = |flags| CommandKind::Move {
            units: vec![ObjectId(1)],
            x: 0,
            z: 0,
            target: -1,
            formation: 0,
            direction: opennml_sim::motion::NO_DIRECTION,
            spread: opennml_sim::command::NO_SPREAD,
            flags,
        };
        let attack = |mods| CommandKind::Attack {
            units: vec![ObjectId(1)],
            target: ObjectId(2),
            pos: None,
            mods,
        };
        // Right double-click (`0x12`, `Input_IssueTargetAction`): modifier 1 -> "Go to" 448
        // gets flag 1 (walk `0x100`), attack 411 modifier 1 (`0x100`); Ctrl stays 4.
        assert_eq!(
            with_modifiers(button::GO_TO, go(0), false, true),
            go(1),
            "Go to runs"
        );
        assert_eq!(with_modifiers(button::GO_TO, go(0), false, false), go(0));
        assert_eq!(
            with_modifiers(button::ATTACK_UNIT, attack(0), false, true),
            attack(1)
        );
        assert_eq!(
            with_modifiers(button::ATTACK_UNIT, attack(0), true, true),
            attack(5)
        );
        // Running applies only to the actions that read `mods`.
        assert_eq!(
            with_modifiers(button::GARRISON_ATTACK, attack(0), false, true),
            attack(0)
        );
        let chop = CommandKind::ChopWood {
            tree: ObjectId(2),
            units: vec![ObjectId(1)],
        };
        assert_eq!(
            with_modifiers(button::CHOP_WOOD, chop.clone(), false, true),
            chop
        );
    }

    // QUIRK D-227: kept
    #[test]
    fn a_right_double_click_issues_the_action_twice_and_the_second_runs() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        // An attack action 411 on trees (filter of the wood action), so that it can be tested without an enemy type.
        let mut entry = host.rules.targets[0].clone();
        entry.button_msg = button::ATTACK_UNIT;
        host.rules.targets.insert(0, entry);
        let sel = [worker];
        let to = ground((7, 9));
        let go = |flags| CommandKind::Move {
            units: vec![worker],
            x: 7 << 12 | 0x800,
            z: 9 << 12 | 0x800,
            target: -1,
            formation: 0,
            direction: opennml_sim::motion::NO_DIRECTION,
            spread: opennml_sim::command::NO_SPREAD,
            flags,
        };
        let attack = |mods| CommandKind::Attack {
            units: vec![worker],
            target: tree,
            pos: None,
            mods,
        };
        let click = |host: &mut SimHost, target, queue, ctrl, double| {
            let issued = host.issue_right_click(
                &sel,
                target,
                to,
                queue,
                ctrl,
                crate::game::formation_order::FormationOrder::default(),
                double,
            );
            assert_eq!(issued.len(), 1 + usize::from(double), "actions per click");
            let logged = logged(host);
            assert_eq!(
                logged,
                issued.into_iter().map(|(_, k)| k).collect::<Vec<_>>(),
                "command log = issued commands"
            );
            logged
        };
        // Single click and two slow clicks: one command, walking.
        assert_eq!(click(&mut host, None, None, false, false), [go(0)]);
        // Double click: first the normal action (event 4), then the same one running (`0x12`).
        assert_eq!(
            click(&mut host, None, None, false, true),
            [go(0), go(1)],
            "Go to"
        );
        let on_tree = Some(Object(tree));
        assert_eq!(
            click(&mut host, on_tree, None, false, true),
            [attack(0), attack(1)],
            "Attack"
        );
        assert_eq!(
            click(&mut host, on_tree, None, true, true),
            [attack(4), attack(5)],
            "Attack with Ctrl"
        );
        // Shift wraps both move commands in `0x7B`.
        let queued = |inner| CommandKind::Queue {
            cyclic: false,
            inner: Box::new(inner),
        };
        assert_eq!(
            click(&mut host, None, Some(false), false, true),
            [queued(go(0)), queued(go(1))]
        );
        // Empty selection: even the double click outputs nothing.
        assert!(
            host.issue_right_click(
                &[],
                None,
                to,
                None,
                false,
                crate::game::formation_order::FormationOrder::default(),
                true
            )
            .is_empty()
        );
    }

    #[test]
    fn the_attack_mode_on_the_ground_sets_stance_3_and_marches_with_flag_4() {
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let enemy = spawn(&mut host, COLONIST, 1, (40, 10));
        let to = ground((7, 9));
        let march = |flags| CommandKind::Move {
            units: vec![worker],
            x: 7 << 12 | 0x800,
            z: 9 << 12 | 0x800,
            target: -1,
            formation: 0,
            direction: opennml_sim::motion::NO_DIRECTION,
            spread: opennml_sim::command::NO_SPREAD,
            flags,
        };
        let stance = CommandKind::SetBehavior {
            units: vec![worker],
            mode: 3,
        };
        // Without a key: stance 3, then the march (`0x04` with flag 4); both go into the log.
        let issued = host.issue_attack_action(&[worker], None, to, None, FormationOrder::default());
        assert_eq!(issued, [stance.clone(), march(4)]);
        assert_eq!(logged(&mut host), [stance.clone(), march(4)]);
        // Shift queues the march like 448; the stance goes directly.
        let issued =
            host.issue_attack_action(&[worker], None, to, Some(false), FormationOrder::default());
        assert_eq!(
            issued,
            [
                stance.clone(),
                CommandKind::Queue {
                    cyclic: false,
                    inner: Box::new(march(4))
                }
            ]
        );
        logged(&mut host);
        // Without a hit of entry 427 (the test table has none) also on an object: march.
        let issued =
            host.issue_attack_action(&[worker], Some(enemy), to, None, FormationOrder::default());
        assert_eq!(issued, [stance, march(4)]);
        logged(&mut host);
        // Outside the map, foreign or empty selection: nothing.
        let outside = Vec3::new(-10.0, 0.0, 300.0);
        assert!(
            host.issue_attack_action(&[worker], None, outside, None, FormationOrder::default())
                .is_empty()
        );
        assert!(
            host.issue_attack_action(&[enemy], None, to, None, FormationOrder::default())
                .is_empty()
        );
        assert!(
            host.issue_attack_action(&[], None, to, None, FormationOrder::default())
                .is_empty()
        );
        assert!(logged(&mut host).is_empty());
    }

    // --- Headhunter menu (re/headhunter-robbing 2.5) ------------------------------------------
    //
    // Headhunter world from `crate::game::testworld`: slot 0 SPA, slot 1 SID, both play; never
    // `tests::HQ`/`tests::host` of this module.

    /// Headhunter menu of the first selected object with the menu state of `ui`.
    fn hire_menu(host: &mut SimHost, sel: &[ObjectId], ui: &EconomyUi) -> HirePanel {
        host.panel_with_hire(sel, &ui.hire)
            .building
            .expect("building part")
            .hire
            .expect("headhunter menu")
    }

    // QUIRK D-239: kept
    #[test]
    fn the_hire_menu_walks_three_levels() {
        use crate::game::testworld::{HQ_SID, HQ_SPA, SID_SHERIFF, UNDERTAKER, hunter_host, spawn};
        let mut host = hunter_host();
        host.state.players[0].resources_fx[0] = 1000 << 12;
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        spawn(&mut host, HQ_SID, 1, (40, 40));
        spawn(&mut host, SID_SHERIFF, 1, (44, 30));
        spawn(&mut host, UNDERTAKER, 1, (46, 30));
        let sel = [hq];
        let mut ui = EconomyUi::default();
        // Level 1: the three kinds in menu order, price 300 (config 12), free, without a headhunter.
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!(
            h.arts
                .iter()
                .map(|a| (a.art, a.guid, a.name.as_str(), a.price, a.enabled))
                .collect::<Vec<_>>(),
            [
                (0, 4100, "Kopfgeldjaeger_SPA", 300, true),
                (1, 4101, "Kopfgeldjaeger_PRI", 300, true),
                (2, 4102, "Kopfgeldjaeger_SID", 300, true),
            ]
        );
        assert!(
            h.arts
                .iter()
                .all(|a| a.target.is_none() && a.customer.is_none())
        );
        assert_eq!((h.art, h.target_player), (None, None));
        assert!(h.players.is_empty() && h.targets.is_empty());
        // Level 2: only the enemy slot 1 (plays, enemy), name "Player 1", color 1.
        ui.apply(&mut host, &sel, PanelAction::HireArt(2));
        assert_eq!(
            ui.hire,
            HireMenu {
                building: Some(hq),
                art: Some(2),
                target_player: None,
            }
        );
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!(h.art, Some(2));
        assert_eq!(h.players, [(1, "Player 1".to_string(), 1)]);
        assert!(h.targets.is_empty());
        // Level 3: the elites of the SID slot that it owns (not 2616), in table order.
        ui.apply(&mut host, &sel, PanelAction::HirePlayer(1));
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!(h.target_player, Some(1));
        assert_eq!(
            h.targets,
            [
                (2614, "SID_Sheriff".to_string()),
                (2615, "SID_Leichenbestatter".to_string()),
            ]
        );
        assert!(host.pending_commands().is_empty());
        // Target button: exactly one `0x2C`, unwrapped, then level 1.
        ui.apply(&mut host, &sel, PanelAction::HireTarget(2615));
        let sent: Vec<CommandKind> = host
            .pending_commands()
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(
            sent,
            [CommandKind::HireHeadhunter {
                art: 2,
                target_player: 1,
                target_guid: 2615,
                building: hq,
            }]
        );
        assert_eq!(ui.hire, HireMenu::default());
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!((h.art, h.target_player), (None, None));
        // The simulation accepts the command: the headhunter arises, 300 gold are gone.
        host.step_once();
        assert!(host.state.headhunters[2].is_some());
        assert_eq!(host.state.players[0].resources_fx[0] >> 12, 700);
    }

    // QUIRK D-239, C-228: kept
    #[test]
    fn the_hire_menu_offers_the_hq_only_with_a_hunter() {
        use crate::game::testworld::{HQ_SPA, HUNTER_SID, SID_SHERIFF, hunter_host, spawn};
        let mut host = hunter_host();
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        let sel = [hq];
        let mut ui = EconomyUi::default();
        ui.apply(&mut host, &sel, PanelAction::HireArt(2));
        ui.apply(&mut host, &sel, PanelAction::HirePlayer(1));
        // Target player without an elite and without a headhunter of the kind: empty (C-228).
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!(h.players.len(), 1);
        assert!(h.targets.is_empty());
        // With a headhunter of kind 2: the HQ of nation 3 (SID).
        let hunter = spawn(&mut host, HUNTER_SID, 8, (10, 10));
        assert_eq!(host.state.headhunters[2], Some(hunter));
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!(h.targets, [(2500, "SID_Rathaus".to_string())]);
        // The headhunter counts only for its kind: kind 0 stays without a target.
        ui.apply(&mut host, &sel, PanelAction::HireArt(0));
        ui.apply(&mut host, &sel, PanelAction::HirePlayer(1));
        assert!(hire_menu(&mut host, &sel, &ui).targets.is_empty());
        // With an elite only the elite, no HQ.
        spawn(&mut host, SID_SHERIFF, 1, (44, 30));
        ui.apply(&mut host, &sel, PanelAction::HireArt(2));
        ui.apply(&mut host, &sel, PanelAction::HirePlayer(1));
        assert_eq!(
            hire_menu(&mut host, &sel, &ui).targets,
            [(2614, "SID_Sheriff".to_string())]
        );
    }

    #[test]
    fn the_hire_menu_shows_price_target_and_customer() {
        use crate::game::testworld::{HQ_SPA, UNDERTAKER, hunter_host, spawn};
        use opennml_sim::command::headhunter::hire_price_in;
        let mut host = hunter_host();
        host.state.players[0].resources_fx[0] = 1000 << 12;
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        spawn(&mut host, UNDERTAKER, 1, (46, 30));
        let sel = [hq];
        let mut ui = EconomyUi::default();
        for action in [
            PanelAction::HireArt(2),
            PanelAction::HirePlayer(1),
            PanelAction::HireTarget(2615),
        ] {
            ui.apply(&mut host, &sel, action);
        }
        // The command takes effect in the first step, `0x122` (price 350, target, customer) in the first object tick
        // of the headhunter (see `the_hire_reaches_the_new_hunter_before_its_first_tick`).
        // Measured: 1 step. The headhunter goes into the shorter tick group; in this
        // world that is the group of the command step, so its first object tick falls into the
        // command step (in the sim test world only into the second step).
        let mut steps = 0;
        while hire_price_in(&host.state, &host.rules, 2) != 350 {
            assert!(steps < 6, "price not 350 after {steps} steps");
            host.step_once();
            steps += 1;
        }
        assert_eq!(steps, 1);
        assert_eq!(host.state.players[0].resources_fx[0] >> 12, 700);
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!(
            h.arts.iter().map(|a| a.price).collect::<Vec<_>>(),
            [300, 300, 350]
        );
        // Two separate entries without a label: type name of the target in the color of the target player,
        // name of the customer in its color (tooltips `FUN_0042F7B0`/`FUN_0042F8B0`).
        let (color0, color1) = (host.state.players[0].color, host.state.players[1].color);
        assert_ne!(color0, color1);
        assert_eq!(
            h.arts[2].target,
            Some(("SID_Leichenbestatter".to_string(), color1))
        );
        assert_eq!(h.arts[2].customer, Some(("Player 0".to_string(), color0)));
        assert!(
            h.arts[0].target.is_none()
                && h.arts[0].customer.is_none()
                && h.arts[1].target.is_none()
                && h.arts[1].customer.is_none()
        );
        // With names instead of "Player n".
        host.state.players[0].name = "Ana".into();
        host.state.players[1].name = "Bo".into();
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!(h.arts[2].customer, Some(("Ana".to_string(), color0)));
        assert_eq!(
            h.arts[2].target,
            Some(("SID_Leichenbestatter".to_string(), color1))
        );
        // After a success (price 500, target and customer -1): both tooltips empty.
        let hunter = host.state.headhunters[2].unwrap();
        {
            let o = host.state.object_mut(hunter, false).unwrap();
            let Some(ModuleKind::HeadHunter(hh)) =
                o.module_mut(headhunter::INDEX).map(|m| &mut m.kind)
            else {
                panic!("headhunter module expected");
            };
            (hh.cur_reward, hh.target_guid, hh.target_player, hh.customer) = (500, -1, -1, -1);
        }
        let h = hire_menu(&mut host, &sel, &ui);
        assert_eq!(
            (h.arts[2].price, &h.arts[2].target, &h.arts[2].customer),
            (500, &None, &None)
        );
    }

    #[test]
    fn the_hire_menu_resets_on_back_cancel_and_selection() {
        use crate::game::testworld::{HQ_SID, HQ_SPA, UNDERTAKER, hunter_host, spawn};
        let mut host = hunter_host();
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        let hq2 = spawn(&mut host, HQ_SID, 0, (30, 20));
        spawn(&mut host, UNDERTAKER, 1, (46, 30));
        let mut ui = EconomyUi {
            open: true,
            ..EconomyUi::default()
        };
        let level3 = |ui: &mut EconomyUi, host: &mut SimHost| {
            ui.apply(host, &[hq], PanelAction::HireArt(1));
            ui.apply(host, &[hq], PanelAction::HirePlayer(1));
        };
        let at = |art, target_player| HireMenu {
            building: Some(hq),
            art,
            target_player,
        };
        // "Back" (409): level 3 -> 2 -> 1; nothing on level 1.
        level3(&mut ui, &mut host);
        assert_eq!(ui.hire, at(Some(1), Some(1)));
        ui.apply(&mut host, &[hq], PanelAction::HireBack);
        assert_eq!(ui.hire, at(Some(1), None));
        ui.apply(&mut host, &[hq], PanelAction::HireBack);
        assert_eq!(ui.hire, HireMenu::default());
        ui.apply(&mut host, &[hq], PanelAction::HireBack);
        assert_eq!(ui.hire, HireMenu::default());
        // Esc and right-click: the viewer (`apps/opennml-viewer/src/app.rs`) calls `cancel_hire` in both places.
        level3(&mut ui, &mut host);
        assert!(ui.cancel_hire());
        assert_eq!(ui.hire, HireMenu::default());
        assert!(!ui.cancel_hire());
        // Selection change: the same building keeps the menu, a second own HQ and the empty
        // selection reset it.
        level3(&mut ui, &mut host);
        ui.follow_selection(&host, &[hq]);
        assert_eq!(ui.hire, at(Some(1), Some(1)));
        ui.follow_selection(&host, &[hq2]);
        assert_eq!(ui.hire, HireMenu::default());
        level3(&mut ui, &mut host);
        ui.follow_selection(&host, &[]);
        assert_eq!(ui.hire, HireMenu::default());
        // The second HQ does not show the menu of the first and takes no clicks of levels 2/3.
        ui.apply(&mut host, &[hq], PanelAction::HireArt(1));
        let h = hire_menu(&mut host, &[hq2], &ui);
        assert_eq!((h.art, h.players.len()), (None, 0));
        ui.apply(&mut host, &[hq2], PanelAction::HirePlayer(1));
        assert_eq!(ui.hire, at(Some(1), None));
        // Window closed.
        level3(&mut ui, &mut host);
        ui.toggle();
        assert!(!ui.open);
        assert_eq!(ui.hire, HireMenu::default());
        // Without level 3 the target button sends nothing; none of this issues a command.
        ui.apply(&mut host, &[hq], PanelAction::HireTarget(2615));
        assert!(host.pending_commands().is_empty());
    }

    #[test]
    fn the_hire_menu_leaves_the_checksum_unchanged() {
        use crate::game::testworld::{
            HQ_SPA, HUNTER_SID, SID_SHERIFF, UNDERTAKER, hunter_host, spawn,
        };
        let mut host = hunter_host();
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        spawn(&mut host, SID_SHERIFF, 1, (44, 30));
        spawn(&mut host, UNDERTAKER, 1, (46, 30));
        spawn(&mut host, HUNTER_SID, 8, (10, 10));
        host.step_once();
        let before = checksum(&host.state);
        let bytes = host.save().to_bytes();
        let mut ui = EconomyUi::default();
        for action in [
            PanelAction::HireArt(2),
            PanelAction::HirePlayer(1),
            PanelAction::HireBack,
            PanelAction::HirePlayer(1),
        ] {
            ui.apply(&mut host, &[hq], action);
            let h = hire_menu(&mut host, &[hq], &ui);
            // The deployed headhunter has neither a target nor a customer: both tooltips empty.
            assert!(h.arts[2].target.is_none() && h.arts[2].customer.is_none());
        }
        assert_eq!(hire_menu(&mut host, &[hq], &ui).targets.len(), 2);
        host.panel(&[hq]);
        assert_eq!(checksum(&host.state), before);
        assert!(host.state.events.is_empty(), "{:?}", host.state.events);
        assert!(host.save().to_bytes() == bytes, "savegame changed");
        assert!(host.pending_commands().is_empty());
    }
}
