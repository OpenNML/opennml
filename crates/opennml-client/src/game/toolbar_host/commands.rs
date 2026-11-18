//! Intents of the command bar -> commands of the simulation.
//!
//! The bar (`opennml_ui::hud::toolbar`) reports game actions as a [`ToolbarIntent`]; here they become commands,
//! selection changes and effects of the host ([`ToolbarEffects`]). Like all inputs, the commands go through the host
//! to the next tick boundary (deviation from the original).
//!
//! # Command forms (K-88, re/game-ui N.14.7)
//!
//! "List" is the GROUP_AND list of the `BUTTONMSG` (`Toolbar+0x2C`, filled by `FUN_00454950` before the click path):
//! with a match in [`ButtonSet::group_for`] that one, otherwise the whole selection (O-272). The bar already passes it
//! along with [`ToolbarIntent::Dispatch`]; for the other intents [`CommandContext::list`] looks it up here.
//!
//! | Form | Commands |
//! |---|---|
//! | head of the list, only for a non-empty list (`+0x34`) | `0x0E` MAKE (`FUN_0044B490`), `0x0F` cancel (`FUN_0044B4F0`), `0x14` occupant, `0x2C` headhunter (`FUN_0044B700`), `0x29`-`0x2B`, `0x2D`, `0x1F`, `0x22`-`0x25` (via [`unit_bar::apply`]) |
//! | one command with the list | `0x10` Die 408 (`FUN_0044BF90`), `0x12` stance (`Input_SubmitBehaviorCommand` `0x0044B930`), `0x67` demolition 530, `0x40`/`0x41` 487/488, `0x0B`, `0x0D`, `0x1B`-`0x1E`, `0x20`/`0x21`, `0x34`/`0x35`, `0x38`/`0x72`, `0x75` (via [`unit_bar::apply`]) |
//! | one command with the world selection | `0x44` formation, `0x45` trade (`FUN_0044AEA0`), 399 Delete (`Input_SubmitDeleteSelection` `0x0044BC70`), `0x01` build (`Input_SubmitPlaceBuildingCommand` `0x00452D80`), `0x18` wall (`FUN_00452C90`), defense point (`0x0044BD60`) |
//! | per object of the list | `0x15` 416 (`Toolbar_SubmitUnquarterAll` `0x0044B360`), `0x19`/`0x1A` 430/431, `0x36`, `0x4C`, `0x4F`, `0x60` (via [`unit_bar::apply`]) |
//!
//! The buttons the viewer's unit bar already knows ([`BarAction::from_button`]) run through [`unit_bar::apply`] with
//! the list as the selection; their filters (walk module, menu button) stay (K-88). The simulation checks every
//! command itself anyway.

use glam::Vec3;
use opennml_audio::engine::AudioEngine;
use opennml_rda::Vfs;
use opennml_sim::command::ability::TARGET_BUTTONS;
use opennml_sim::command::{BuildingPart, CommandKind, F32Bits, NO_SPREAD};
use opennml_sim::fx::Fx;
use opennml_sim::id::ObjectId;
use opennml_sim::module::building;
use opennml_sim::motion::NO_DIRECTION;
use opennml_sim::object::status;
use opennml_sim::targeting::button;
use opennml_ui::hud::toolbar::window::{MSG_DEFEND, STANCE_MODES};
use opennml_ui::hud::toolbar::{ButtonSet, ToolbarIntent, ToolbarOut, ToolbarSound};

use crate::game::economy::{queue_keys, with_queue_keys};
use crate::game::formation_order::FormationOrder;
use crate::game::selection::Selection;
use crate::game::sim_host::SimHost;
use crate::game::unit_bar::{self, BarAction, UnitBar};
use crate::game_session::GameSession;

#[cfg(test)]
mod tests;

/// Target buttons with a target point decal: only `Input_IssueTargetAction` (`0x0044D350`) sends `0xFA {3, x, y, z}`,
/// namely for the ground targets 427, 448, 530 and the object targets 529, 533 (re/game-ui N.5.8, O-237). The rally
/// point 447 is deliberately missing: case `0x1BF` only builds `0x27`, without decal, sound and message (D-262, test
/// `rally_flag::tests::flag_at_the_unrasterized_click_point_and_at_water_level`).
pub const TARGET_DECAL_BUTTONS: [i32; 5] = [button::ATTACK, button::GO_TO, 530, 529, 533];
/// Object targets of the decal: position with height (re/game-ui N.5.8).
pub const DECAL_OBJECT_BUTTONS: [i32; 2] = [529, 533];
/// First stance 420 (`STATUSBUTTONS` slot 0).
const MSG_STANCE_FIRST: i32 = 420;
/// Delete the selection (`Input_SubmitDeleteSelection`), without a button in the shipped data.
const MSG_DELETE: i32 = 399;
/// Die "really?" (submenu 407).
const MSG_DIE: i32 = 408;
/// Let out all occupants.
const MSG_UNQUARTER_ALL: i32 = 416;
/// Fire arrows on/off (`FUN_0044B910`/`FUN_0044B920`).
const MSG_FIRE_ARROWS: i32 = 487;
const MSG_NORMAL_ARROWS: i32 = 488;
/// Demolish building "really?" (submenu 529).
const MSG_DEMOLISH: i32 = 530;
/// Scatter.
const MSG_SCATTER: i32 = 519;
/// Formations 511 ... 518 (`FUN_0044ADE0`, 511 ... 515 in the shipped data).
const MSG_FORMATIONS: std::ops::RangeInclusive<i32> = 511..=518;

/// Target point decal of a target button (O-237): only the data; it is drawn for 300 ms of program time
/// (`GetTickCount`, also runs during the pause) with the size `rest * 0.00333 * 32 + 1` (re/game-ui N.5.8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetDecal {
    /// `BUTTONMSG` of the target button.
    pub button: i32,
    /// World point: ground targets with `y = 0` (terrain decal), object targets 529/533 with the height of the object.
    pub pos: [f32; 3],
}

/// Effects of an intent that are not commands (the host applies them).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolbarEffects {
    /// The selection changed through a selection or hero button: the host sends `{8}` and plays the selection sound
    /// ([`GameSession::selection_changed`]; in the original the full rebuild `FUN_00457980` at the end of the click
    /// path). The headhunter button changes the selection without `{8}` (O-270) and does not set this.
    pub selection_changed: bool,
    /// Camera to this world point `(x, z)` (`GameEngine_MoveCameraTo`, hero button on an already selected or quartered
    /// hero).
    pub camera_to: Option<[f32; 2]>,
    /// Target point decal (O-237), only for [`TARGET_DECAL_BUTTONS`].
    pub target_decal: Option<TargetDecal>,
    /// Sounds of the bar in output order (only from [`GameSession::toolbar_outputs`]); the host plays them.
    pub sounds: Vec<ToolbarSound>,
}

impl ToolbarEffects {
    /// Add the effects of a further intent (later ones win for camera and decal).
    pub fn merge(&mut self, other: ToolbarEffects) {
        self.selection_changed |= other.selection_changed;
        self.camera_to = other.camera_to.or(self.camera_to);
        self.target_decal = other.target_decal.or(self.target_decal);
        self.sounds.extend(other.sounds);
    }
}

/// What executing an intent needs (testable without a GPU): simulation, selection, bar state, the GROUP_AND lists of
/// the bar and the held keys Shift/Ctrl (`KD_SHIFT_BUTTON`, `KD_CONTROL_BUTTON`; they act only on the queue of the
/// commands).
pub struct CommandContext<'a> {
    pub sim: &'a mut SimHost,
    pub selection: &'a mut Selection,
    pub bar: &'a mut UnitBar,
    pub groups: &'a ButtonSet,
    pub shift: bool,
    pub ctrl: bool,
}

impl CommandContext<'_> {
    /// List `Toolbar+0x2C` for the `BUTTONMSG` `msg` (`FUN_00454950`): GROUP_AND match, otherwise the whole selection.
    pub fn list(&self, msg: i32) -> Vec<ObjectId> {
        self.groups.group_for(msg).map_or_else(
            || self.selection.ids.clone(),
            |l| l.iter().map(|&h| ObjectId(h)).collect(),
        )
    }

    /// Head of the list (`*(Toolbar+0x2C) + 8`), only for a non-empty list (`+0x34 != 0`).
    fn head(&self, msg: i32) -> Option<ObjectId> {
        self.list(msg).first().copied()
    }

    /// Selected formation of the world for the walk commands, without dragging.
    fn order(&self) -> FormationOrder {
        FormationOrder::new(self.bar, None)
    }

    /// World point of an object (`GameObject+0x08`).
    fn object_pos(&self, id: ObjectId) -> Option<[f32; 3]> {
        self.sim.state.object(id, false).map(|o| o.world_pos)
    }

    fn submit_nonempty(
        &mut self,
        units: Vec<ObjectId>,
        kind: impl FnOnce(Vec<ObjectId>) -> CommandKind,
    ) {
        if !units.is_empty() {
            self.sim.submit_own(kind(units));
        }
    }
}

/// Fixed-point tile of a world coordinate `Fixed_FromFloat(v / tile)` (`0x00547980`).
fn fixed_tile(v: f32, cell: i32) -> i32 {
    Fx::from_f32(v / cell as f32).0
}

/// Execute an intent of the bar (without host effects on sound and camera; [`execute`] applies those).
pub fn execute_in(intent: ToolbarIntent, cx: &mut CommandContext<'_>) -> ToolbarEffects {
    let mut fx = ToolbarEffects::default();
    match intent {
        // `FUN_0044B490`: `{head, guid, slot}`, `count` times; the UPGRADE flag does not go into the command.
        ToolbarIntent::Make { guid, count, .. } => {
            if let Some(building) = cx.head(guid as i32) {
                for _ in 0..count {
                    cx.sim
                        .submit_own(CommandKind::EnqueueProduction { building, guid });
                }
            }
        }
        // `FUN_0044B4F0`: `{head, kind & 0xFF, slot, guid}`.
        ToolbarIntent::Cancel { index, guid, .. } => {
            if let Some(building) = cx.head(guid as i32) {
                cx.sim.submit_own(CommandKind::CancelProduction {
                    building,
                    index,
                    arg: guid as i32,
                });
            }
        }
        ToolbarIntent::Dispatch { msg, objects } => {
            let ids: Vec<ObjectId> = objects.into_iter().map(ObjectId).collect();
            dispatch(cx, msg, ids);
        }
        // `Input_SubmitBehaviorCommand` (`0x0044B930`): one `0x12` with the list of the stance number.
        ToolbarIntent::Stance(mode) => {
            let msg = STANCE_MODES
                .iter()
                .position(|&m| m == mode)
                .map_or(MSG_DEFEND, |i| MSG_STANCE_FIRST + i as i32);
            let units = cx.list(msg);
            cx.submit_nonempty(units, |units| CommandKind::SetBehavior { units, mode });
        }
        // `ProductionBar_SubmitUnquarterUnit`: `0x14 {head, occupant}`.
        ToolbarIntent::Unquarter(hid) => {
            if let Some(container) = cx.head(hid as i32) {
                cx.sim.submit_own(CommandKind::Unquarter {
                    container,
                    unit: ObjectId(hid),
                });
            }
        }
        // `Toolbar_SubmitUnquarterAll` (`0x0044B360`): one `0x15` per object of the list.
        ToolbarIntent::UnquarterAll => {
            for container in cx.list(MSG_UNQUARTER_ALL) {
                cx.sim.submit_own(CommandKind::UnquarterAll { container });
            }
        }
        ToolbarIntent::ToFront(hid) => {
            cx.selection.to_front(ObjectId(hid));
            fx.selection_changed = true;
        }
        ToolbarIntent::SelectOnly(hid) => {
            select_only(cx, ObjectId(hid));
            fx.selection_changed = true;
        }
        ToolbarIntent::Deselect(hid) => {
            cx.selection.deselect(ObjectId(hid));
            fx.selection_changed = true;
        }
        // `FUN_00458130` kind `0x14`: camera if it was already selected or is quartered (`+0x1AC & 0x100`); then select
        // alone (`0x0047A130`, `0x00488F60`), camera there, full rebuild.
        ToolbarIntent::Hero { hid, camera } => {
            let id = ObjectId(hid);
            let Some(o) = cx.sim.state.object(id, false) else {
                return fx;
            };
            let camera = camera || o.status & status::QUARTERED != 0;
            let pos = o.world_pos;
            select_only(cx, id);
            fx.selection_changed = true;
            if camera {
                fx.camera_to = Some([pos[0], pos[2]]);
            }
        }
        // `FUN_0048B910`: clear the selection, add the HQ, without `{8}` (O-270).
        ToolbarIntent::HeadhunterHq { hq, .. } => select_only(cx, ObjectId(hq)),
        // `FUN_0044B770`: `0x2F {guid, Kopf}`.
        ToolbarIntent::CounterUpgrade { guid, source } => {
            cx.sim.submit_own(CommandKind::CounterUpgrade {
                guid,
                source: ObjectId(source),
            });
        }
        // `FUN_0044B700`: `0x2C {kind, target slot, GUID, head}`; without a target player -1.
        ToolbarIntent::Hire { art, target, guid } => {
            if let Some(building) = cx.head(guid as i32) {
                cx.sim.submit_own(CommandKind::HireHeadhunter {
                    art: i32::from(art),
                    target_player: if target == u8::MAX {
                        -1
                    } else {
                        i32::from(target)
                    },
                    target_guid: guid as i32,
                    building,
                });
            }
        }
        // `FUN_0044AEA0`: `0x45 {ware, target slot}` with the world selection.
        ToolbarIntent::Trade { kind, slot } => {
            let units = cx.selection.ids.clone();
            cx.submit_nonempty(units, |units| CommandKind::TradeWithPlayer {
                ware: kind,
                target: i32::from(slot),
                units,
            });
        }
        ToolbarIntent::Scatter => {
            let list = cx.list(MSG_SCATTER);
            unit_bar::apply(cx.bar, cx.sim, &list, BarAction::Scatter);
        }
        // `Input_SubmitPlaceBuildingCommand` (`0x00452D80`): `{guid, rotation, x, y, z}` with the world selection as
        // workers through `Input_SubmitCommandWithQueueKeys` (Shift/Ctrl).
        ToolbarIntent::PlaceBuilding {
            guid,
            rotation,
            pos,
        } => {
            let kind = CommandKind::PlaceBuilding {
                guid,
                rotation: F32Bits::new(rotation),
                x: F32Bits::new(pos[0]),
                y: F32Bits::new(pos[1]),
                z: F32Bits::new(pos[2]),
                workers: cx.selection.ids.clone(),
                border: None,
            };
            let queue = queue_keys(cx.shift, cx.ctrl);
            cx.sim.submit_own(with_queue_keys(kind, queue));
        }
        // `FUN_00453490`: `0x2E {unit, x/T, z/T}`.
        ToolbarIntent::PlaceTent { unit, pos } => {
            let cell = cx.sim.rules.map.cell;
            cx.sim.submit_own(CommandKind::UnpackTent {
                unit: ObjectId(unit),
                x: fixed_tile(pos[0], cell[0]),
                z: fixed_tile(pos[1], cell[2]),
            });
        }
        // `FUN_00452C90`: `0x18 {n, parts..., world selection}`, without queue keys.
        ToolbarIntent::PlaceWall { parts } => {
            let parts = parts
                .into_iter()
                .map(|p| BuildingPart {
                    guid: p.guid,
                    rotation: F32Bits::new(p.rotation),
                    x: F32Bits::new(p.pos[0]),
                    y: F32Bits::new(p.pos[1]),
                    z: F32Bits::new(p.pos[2]),
                })
                .collect();
            cx.sim.submit_own(CommandKind::PlaceBuildings {
                parts,
                workers: cx.selection.ids.clone(),
            });
        }
        ToolbarIntent::TargetChosen {
            msg,
            object,
            ground,
        } => fx.target_decal = target_chosen(cx, msg, object.map(ObjectId), ground),
        ToolbarIntent::DefensePoint { x, z } => defend_area(cx, x, z),
        // The session applies the minimap GUIDs with full access ([`GameSession::toolbar_outputs`] ->
        // [`GameSession::minimap_button`]); the list gets no command.
        ToolbarIntent::Minimap { msg, .. } => {
            log::debug!("Command bar: minimap button {msg} without a session");
        }
    }
    fx
}

/// `0x0047A130` + `0x00488F60` with the fog check of the host.
fn select_only(cx: &mut CommandContext<'_>, id: ObjectId) {
    let sim = &*cx.sim;
    cx.selection
        .select_only(&sim.state, &sim.rules, |o| sim.fog_selectable(o), id);
}

/// Branches of the dispatcher `Toolbar_DispatchButtonCommand` (`0x0044C040`) with a command to the list `ids`.
fn dispatch(cx: &mut CommandContext<'_>, msg: i32, ids: Vec<ObjectId>) {
    match msg {
        // `FUN_0044BF90` -> `Input_SubmitSelectionCommand(0x10)`.
        MSG_DIE => cx.submit_nonempty(ids, |units| CommandKind::Die { units }),
        // `Input_SubmitDeleteSelection`: only with an own first object; building `0x67` with the list, otherwise `0x10`
        // with the world selection.
        MSG_DELETE => {
            let Some(first) = cx
                .selection
                .ids
                .first()
                .and_then(|&id| cx.sim.state.object(id, false))
            else {
                return;
            };
            if first.owner() != cx.sim.active_player {
                return;
            }
            if first.has_module(building::INDEX, false) {
                cx.submit_nonempty(ids, |buildings| CommandKind::SellBuildings { buildings });
            } else {
                let units = cx.selection.ids.clone();
                cx.submit_nonempty(units, |units| CommandKind::Die { units });
            }
        }
        // `Input_SubmitSelectionCommand(0x67)`: without confirmation (the confirmation is submenu 529).
        MSG_DEMOLISH => {
            cx.submit_nonempty(ids, |buildings| CommandKind::SellBuildings { buildings })
        }
        // `FUN_0044B910`/`FUN_0044B920`: `0x40`/`0x41` with the list (the simulation rejects player commands, C-224).
        MSG_FIRE_ARROWS | MSG_NORMAL_ARROWS => {
            cx.submit_nonempty(ids, |units| CommandKind::SetArrowType {
                units,
                fire: msg == MSG_FIRE_ARROWS,
            })
        }
        // `FUN_0044ADE0(n)`: formation n (0 ... 7, outside 0 ... 4 counts as 0) and `0x44` to the whole selection.
        m if MSG_FORMATIONS.contains(&m) => {
            unit_bar::apply(cx.bar, cx.sim, &ids, BarAction::Formation(m - 511));
        }
        _ => match BarAction::from_button(msg) {
            // Target buttons and unpacking a tent never come as a dispatcher branch (cursor of the bar).
            Some(BarAction::Target(_) | BarAction::UnpackTent) | None => {
                log::debug!("Command bar: button {msg} has no command in the reimplementation");
            }
            Some(action) => unit_bar::apply(cx.bar, cx.sim, &ids, action),
        },
    }
}

/// Targeting mode chosen (`0x16` -> `0x00457C50` -> `Input_IssueTargetAction` with the `BUTTONMSG`): 427 through the
/// attack mode, the ability targets through [`unit_bar::control_unit_bar`] (first selected own object), all others
/// through [`SimHost::issue_button_action`]. Returns the target point decal (O-237), only if a command went out: for a
/// selection without an own object the original returns before `Input_IssueTargetAction` and then sends no `0xFA`
/// either.
fn target_chosen(
    cx: &mut CommandContext<'_>,
    msg: i32,
    object: Option<ObjectId>,
    ground: Option<[f32; 2]>,
) -> Option<TargetDecal> {
    let object_pos = object.and_then(|o| cx.object_pos(o));
    let point = ground
        .map(|[x, z]| Vec3::new(x, 0.0, z))
        .or_else(|| object_pos.map(Vec3::from_array))?;
    let selection = cx.selection.ids.clone();
    let queue = queue_keys(cx.shift, cx.ctrl);
    let order = cx.order();
    let issued = if msg == button::ATTACK {
        !cx.sim
            .issue_attack_action(&selection, object, point, queue, order)
            .is_empty()
    } else if TARGET_BUTTONS.contains(&msg) {
        let ground = Some([point.x, point.z]);
        match unit_bar::control_unit_bar(
            cx.bar,
            cx.sim,
            &selection,
            msg,
            object.map(|o| o.0),
            ground,
        ) {
            Ok(()) => true,
            Err(e) => {
                log::debug!("Command bar: target button {msg}: {e}");
                false
            }
        }
    } else {
        let kind = cx
            .sim
            .issue_button_action(msg, &selection, object, point, queue, cx.ctrl, order);
        if kind.is_none() {
            log::debug!("Command bar: target button {msg} without a command");
        }
        kind.is_some()
    };
    (issued && TARGET_DECAL_BUTTONS.contains(&msg)).then(|| {
        let pos = match object_pos {
            Some(p) if DECAL_OBJECT_BUTTONS.contains(&msg) => p,
            _ => [point.x, 0.0, point.z],
        };
        TargetDecal { button: msg, pos }
    })
}

/// Defense point (`0x0044BD60`, stance 421 on units): only with a selection; `0x12` mode 0 to the list, then the walk
/// of the world selection in the chosen formation to the point (`MoveCommand_Construct(n, x, z, -1, formation, no
/// direction, -1, 0)`), after it a wrapped `0x7B {0x12 mode 4}` to the world selection. `x`, `z` are world
/// coordinates; the cursor sends them as `Fixed_FromFloat(v / tile)` (`0x004502A3`...`0x004502E6`).
fn defend_area(cx: &mut CommandContext<'_>, x: f32, z: f32) {
    let units = cx.selection.ids.clone();
    if units.is_empty() {
        return;
    }
    let behaviour = cx.list(MSG_DEFEND);
    cx.submit_nonempty(behaviour, |units| CommandKind::SetBehavior {
        units,
        mode: 0,
    });
    let cell = cx.sim.rules.map.cell;
    cx.sim.submit_own(CommandKind::Move {
        units: units.clone(),
        x: fixed_tile(x, cell[0]),
        z: fixed_tile(z, cell[2]),
        target: -1,
        formation: cx.bar.formation(),
        direction: NO_DIRECTION,
        spread: NO_SPREAD,
        flags: 0,
    });
    cx.sim.submit_own(CommandKind::Queue {
        cyclic: false,
        inner: Box::new(CommandKind::SetBehavior { units, mode: 4 }),
    });
}

/// Execute an intent of the bar in the match ([`execute_in`]) and apply the effects of the host: selection change with
/// `{8}` and selection sound ([`GameSession::selection_changed`]), camera to the hero. The decal is drawn from the
/// return value. `groups` are the GROUP_AND lists of the bar (`Toolbar::set`). Nothing without a simulation.
pub fn execute(
    intent: ToolbarIntent,
    session: &mut GameSession,
    groups: &ButtonSet,
    vfs: &Vfs,
    audio: Option<&mut AudioEngine>,
) -> ToolbarEffects {
    let shift = session.pointer.shift();
    let ctrl = session.pointer.ctrl();
    let Some(sim) = session.map.sim.as_mut() else {
        return ToolbarEffects::default();
    };
    let fx = execute_in(
        intent,
        &mut CommandContext {
            sim,
            selection: &mut session.selection,
            bar: &mut session.toolbar_host.bar,
            groups,
            shift,
            ctrl,
        },
    );
    if fx.selection_changed {
        session.selection_changed(vfs, audio);
    }
    if let Some([x, z]) = fx.camera_to {
        let terrain = &session.map.scene.terrain;
        session.map.camera.move_to(x, z, terrain);
    }
    fx
}

impl GameSession {
    /// The outputs of the bar of one event in sequence: `ButtonPressed` -> [`GameSession::button_pressed`] (always
    /// first, as the click path reports them), minimap buttons -> [`GameSession::minimap_button`] (it plays their
    /// selection sounds itself, the effects are added), the other intents -> [`execute`], sounds in
    /// [`ToolbarEffects::sounds`].
    pub fn toolbar_outputs(
        &mut self,
        outs: Vec<ToolbarOut>,
        groups: &ButtonSet,
        vfs: &Vfs,
        mut audio: Option<&mut AudioEngine>,
    ) -> ToolbarEffects {
        let mut fx = ToolbarEffects::default();
        for out in outs {
            match out {
                ToolbarOut::ButtonPressed(value) => self.button_pressed(value),
                ToolbarOut::Intent(ToolbarIntent::Minimap { msg, double }) => {
                    fx.merge(self.minimap_button(msg, double, vfs, audio.as_deref_mut()));
                }
                ToolbarOut::Intent(intent) => {
                    fx.merge(execute(intent, self, groups, vfs, audio.as_deref_mut()));
                }
                ToolbarOut::Sound(sound) => fx.sounds.push(sound),
            }
        }
        fx
    }
}
