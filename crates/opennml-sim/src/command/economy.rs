//! Economy commands (jump table `0x0048DB04`; re/garrison-and-commands, re/construction,
//! re/commands): handlers like the original - per unit first the message (`PostMessage` with abort or
//! `PostCommandMessage` with the feedback messages `0x87F`), then with `direct` acknowledge and empty the
//! command queue. Before that each command is checked against the sender ([`super::validate`], A8) and
//! writes a log line on rejection.
//!
//! The simulation reports the command acknowledgment (`Unit_PlayResponseSound(u, 0xC)`) at the same
//! points as [`crate::state::SimEvent::UnitResponse`]; filtering to the local player, counters and
//! sound are done by the presentation. Not reproduced: `GameObject_NotifyUiIfSelected` (UI). Hunting
//! (`0x16/0x17`) and following (`0x74`) do not acknowledge.

use opennml_formats::targets::MatchFilter;

use crate::collision::find_nearest_matching;
use crate::economy::{
    add_object_costs_percent, announce, can_afford_object, find_unaffordable_resource, guid_count,
    message, object_costs, pay_costs, pending_guid_count,
};
use crate::filter::{fog_visible, matches, matches_class};
use crate::fog::FogView;
use crate::fx::ftol;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{post_command, post_message, send, send_mut};
use crate::module::boarding::compute_boarding_points;
use crate::module::building::{flags as bflags, msg as bmsg, production};
use crate::module::gather::{self, harvest, lumber, mining};
use crate::module::walk::find_to_msg;
use crate::module::{construct, quarter, resource, tree};
use crate::motion::NO_DIRECTION;
use crate::placement::{
    PlacementArgs, PlacementBox, building_flags, check_building_placement, flags as pflags,
    placement_rect, pose_on_map, same_pose, snapped_command_pose, wall_corner_pair,
};
use crate::setup::player_color;
use crate::sim::Sim;
use crate::world::{ObjectDesc, create_object, desc_flags};

use super::validate::{self, own_building};
use super::{
    BuildingPart, Command, CommandKind, Origin, QueuedCommand, RESPONSE_ATTACK, RESPONSE_EXECUTE,
    enqueue, owns, pass, reject, respond, script_path, skip, skip_passed,
};

/// Headquarters that `0x01` locks after paying (**quirk:** SID town hall 2500 is missing).
pub const HEADQUARTERS: [u32; 5] = [1000, 1300, 1600, 1900, 2200];
/// Creation flags of `0x01`/`0x18` (`0xD3`: color, slot, rotation Y, position, type - not
/// finished, so a construction site).
pub const PLACE_FLAGS: u32 = desc_flags::COLOR
    | desc_flags::PLAYER
    | desc_flags::ROT_Y
    | desc_flags::POSITION
    | desc_flags::GUID;
/// Largest number of parts of `0x18` (A8): the fort cursor limits `|dx|`, `|dz|` to 4 ... 22
/// (`FUN_00451100`, `FUN_00450650`), the ring has `4 + 2|dx| + 2|dz|` <= 92 parts. The command
/// executor itself knows no limit.
pub const MAX_WALL_PARTS: usize = 92;
/// Fixed check flags of the wall segments (`BuildCursor_CheckWallSegments` `0x00450870`).
pub const WALL_SEGMENT_CHECK_FLAGS: u32 = 0x10_0003;
/// Largest occupancy border `0x3303 {n}` of the extra word of `0x01` (A8): the original takes any
/// value (sender presumably AI or script); a negative one inverts the rectangle, a large one would let
/// `MarkOccupancyRect` run over arbitrarily many tiles and the occupancy grow beyond the map. A mask
/// width keeps the rectangle within the limits the savegame accepts.
pub const MAX_BORDER: i32 = crate::module::building::MASK_SIDE;
/// Class "can be quartered" (`CANQUARTER`, walk flag).
const CAN_QUARTER: u32 = 0x4_0000;
/// Walk module (module index 0): a container with it is mobile (ship).
const VEHICLE_WALK: usize = 0;
/// Surcharge to the target radius when loading into mobile containers (`[0x00671384]`, a quarter tile).
const VEHICLE_APPROACH: i32 = 1024;
/// Hunting module (`0x16`) and fishing module (`0x37`), whose delivery codes `0x09` chooses for food.
const HUNTING: usize = 0x16;
/// Carcass/game animal module (`CPropertyEdibleAnimal`).
const EDIBLE: usize = 0x15;
/// `HuntOrGut` of hunting (`0x216 {animal}`).
const HUNT_OR_GUT: u32 = 0x216;
/// Walk "follow" (`0x3900 {target}`).
const FOLLOW: u32 = 0x3900;
const FISHING: usize = 0x37;
/// Feedback of the construction orders: report idle worker (`0x87F`).
const IDLE_WORKER: u32 = 0x87F;
/// Delivery code of hunting (`0xA16`, module 22).
const DELIVER_HUNTED: u32 = 0xA16;

/// Execution of an economy command (the wrappers `0x7B/0x7C` are in
/// [`super::execute`]).
pub(super) fn execute(sim: &mut Sim, player: u8, kind: &CommandKind, direct: bool) {
    match kind {
        CommandKind::PlaceBuilding { .. } => {
            place_single_building(sim, player, kind, direct, false);
        }
        CommandKind::PlaceBuildings { parts, workers } => {
            place_buildings(sim, player, parts, workers, direct);
        }
        CommandKind::ConstructAt { building, units } => {
            construct_at(sim, player, *building, units, direct);
        }
        CommandKind::MineGold { mine, units } => {
            if gather_target(sim, player, *mine, bflags::GOLDMINE, "Mine gold") {
                post_to_units(sim, player, units, mining::msg::START, *mine, direct);
            }
        }
        CommandKind::Harvest { farm, units } => {
            if gather_target(sim, player, *farm, bflags::FARM, "Harvest") {
                post_to_units(sim, player, units, harvest::msg::START, *farm, direct);
            }
        }
        CommandKind::ChopWood { tree, units } => chop_wood(sim, player, *tree, units),
        CommandKind::NextTree {
            tile_x,
            tile_z,
            unit,
        } => next_tree(sim, player, (*tile_x, *tile_z), *unit, direct),
        CommandKind::Deliver { building, units } => {
            deliver(sim, player, *building, units, direct);
        }
        CommandKind::Fish { basket, units } => {
            let ok = sim
                .object(*basket)
                .is_some_and(|o| o.guid == resource::FISH_BASKET);
            if ok {
                post_to_units(
                    sim,
                    player,
                    units,
                    gather::fishing::msg::START,
                    *basket,
                    direct,
                );
            } else {
                reject(
                    sim,
                    player,
                    "Fish",
                    format!("{} is not a fishing basket", basket.0),
                );
            }
        }
        CommandKind::EnqueueProduction { building, guid } => {
            match validate::check_production(sim, player, *building, *guid) {
                // Return value discarded (`GameWorld_CmdEnqueueProduction` `0x00475FC0`).
                Ok(()) => {
                    send(
                        sim,
                        *building,
                        &Message::new(bmsg::ENQUEUE, &[*guid as i32]),
                    );
                }
                Err(e) => reject(sim, player, "Production", e),
            }
        }
        CommandKind::CancelProduction {
            building,
            index,
            arg,
        } => match validate::check_cancel(sim, player, *building, *index) {
            Ok(()) => {
                send(sim, *building, &Message::new(bmsg::CANCEL, &[*index, *arg]));
            }
            Err(e) => reject(sim, player, "Cancel", e),
        },
        CommandKind::DebugCompleteOrder {
            building,
            index,
            guid,
        } => {
            let done = validate::check_debug_order(sim, player, *building, *index, *guid)
                .and_then(|()| production::complete_now(sim, *building, *index));
            if let Err(e) = done {
                reject(sim, player, "Complete immediately", e);
            }
        }
        CommandKind::Quarter { container, units } => {
            quarter_units(sim, player, *container, units, direct);
        }
        CommandKind::Unquarter { container, unit } => {
            if own_container(sim, player, *container) {
                let m = Message::new(quarter::msg::UNQUARTER_ONE, &[unit.as_i32()]);
                send(sim, *container, &m);
            }
        }
        CommandKind::UnquarterAll { container } => {
            if own_container(sim, player, *container) {
                send(
                    sim,
                    *container,
                    &Message::new(quarter::msg::UNQUARTER_ALL, &[]),
                );
            }
        }
        CommandKind::SetRallyPoint { x, z, buildings } => {
            // `GameWorld_CmdSetRallyPoint` (`0x00476E60`): synchronous per object, never empties.
            for &b in buildings {
                if own_building(sim, player, b) {
                    pass(sim);
                    send(sim, b, &Message::new(bmsg::SET_MEETING_POINT, &[*x, *z]));
                } else {
                    skip(sim, player, "Rally point", format!("{} not own", b.0));
                }
            }
        }
        CommandKind::SellBuildings { buildings } => {
            // `GameWorld_CmdSellBuildings` (`0x00478A90`).
            for &b in buildings {
                if own_building(sim, player, b) && !validate::is_dead(sim, b) {
                    pass(sim);
                    send(sim, b, &Message::new(bmsg::SELL, &[]));
                } else {
                    skip(sim, player, "Sale", format!("{} not own or dead", b.0));
                }
            }
        }
        CommandKind::Attack {
            units,
            target,
            pos,
            mods,
        } => {
            // `GameWorld_CmdAttack` (`0x0047D330`): modifier 1 -> `0x100`, 4 -> `0x400`.
            let mut flags = 0;
            if mods & 1 != 0 {
                flags |= 0x100;
            }
            if mods & 4 != 0 {
                flags |= 0x400;
            }
            let args: Vec<i32> = match pos {
                Some(p) => vec![target.as_i32(), flags, p[0], p[1], p[2]],
                None => vec![target.as_i32(), flags],
            };
            for u in own_units(sim, player, units) {
                post_message(sim, u, Message::new(0x111, &args), true);
                if direct {
                    respond(sim, u, RESPONSE_ATTACK);
                    clear_queue(sim, u);
                }
            }
        }
        CommandKind::SetBehavior { units, mode } => {
            // `GameWorld_CmdSetBehavior` (`0x004762E0`), per object: buildings report the mode via
            // `0x3503`, units get attack mode (`0x611`) and walk behavior (`0x2100`).
            for u in own_units(sim, player, units) {
                if sim.object(u).is_some_and(|o| o.has_module(3, false)) {
                    send(sim, u, &Message::new(bmsg::SET_ATTACK_MODE, &[*mode]));
                } else {
                    let msg = crate::module::attack::attack_mode_for(sim, u.slot(), *mode);
                    send(sim, u, &msg);
                    send(sim, u, &quarter::walk_behavior_msg(*mode, false));
                }
            }
        }
        CommandKind::GarrisonAttack { buildings, target } => {
            // `GameWorld_CmdQuarteredAttack` (`0x004784C0`).
            for b in own_units(sim, player, buildings) {
                post_message(
                    sim,
                    b,
                    Message::new(quarter::msg::GARRISON_ATTACK, &[target.as_i32()]),
                    true,
                );
            }
        }
        CommandKind::Hunt { animal, units, .. } => {
            // `GameWorld_CmdHuntOrGut` (`0x004764E0`): nothing without `direct` (K-81).
            if !direct {
                return;
            }
            // A8: an existing target must be a game animal or carcass (module 21).
            if sim
                .object(*animal)
                .is_some_and(|o| !o.has_module(EDIBLE, false))
            {
                reject(
                    sim,
                    player,
                    "Hunt",
                    format!("{} is not a huntable animal", animal.0),
                );
                return;
            }
            for u in own_units(sim, player, units) {
                post_message(sim, u, Message::new(HUNT_OR_GUT, &[animal.as_i32()]), true);
                clear_queue(sim, u);
            }
        }
        CommandKind::Follow { target, units } => {
            // `GameWorld_CmdFollow` (`0x00479300`).
            for u in own_units(sim, player, units) {
                post_message(sim, u, Message::new(FOLLOW, &[target.as_i32()]), true);
                if direct {
                    clear_queue(sim, u);
                }
            }
        }
        CommandKind::Move { .. }
        | CommandKind::Halt { .. }
        | CommandKind::SharedVision { .. }
        | CommandKind::Diplomacy { .. }
        | CommandKind::Queue { .. }
        | CommandKind::SetFormation { .. }
        | CommandKind::Scatter { .. }
        | CommandKind::Mount { .. }
        | CommandKind::Dismount { .. }
        | CommandKind::BoardBoat { .. }
        | CommandKind::LeaveBoat { .. }
        | CommandKind::Dive { .. }
        | CommandKind::Surface { .. }
        | CommandKind::Train { .. }
        | CommandKind::ApplyUpgrade { .. }
        | CommandKind::RemoveUpgrade { .. }
        | CommandKind::CounterUpgrade { .. }
        | CommandKind::CastSpell { .. }
        | CommandKind::ExecTempUpgrade { .. }
        | CommandKind::ToggleArming { .. }
        | CommandKind::SetArrowType { .. }
        | CommandKind::PackTent { .. }
        | CommandKind::UnpackTent { .. }
        | CommandKind::TradeWithPlayer { .. }
        | CommandKind::Tribute { .. }
        | CommandKind::OpenGate { .. }
        | CommandKind::CloseGate { .. }
        | CommandKind::Mask { .. }
        | CommandKind::Unmask { .. }
        | CommandKind::DigIn { .. }
        | CommandKind::DigOut { .. }
        | CommandKind::StartHealing { .. }
        | CommandKind::StopHealing { .. }
        | CommandKind::HealTarget { .. }
        | CommandKind::SelfHeal { .. }
        | CommandKind::EvokeGhost { .. }
        | CommandKind::EvokeAnimal { .. }
        | CommandKind::Banish { .. }
        | CommandKind::DrainMana { .. }
        | CommandKind::ScoutArea { .. }
        | CommandKind::DynamiteBuilding { .. }
        | CommandKind::DynamiteShip { .. }
        | CommandKind::KillUnit { .. }
        | CommandKind::SnipeUnit { .. }
        | CommandKind::Whirlwind { .. }
        | CommandKind::DamageBuilding { .. }
        | CommandKind::HireHeadhunter { .. }
        | CommandKind::RobCorpses { .. }
        | CommandKind::Die { .. }
        | CommandKind::OccupancyBorder { .. }
        | CommandKind::AiPlayerAttackMode { .. }
        | CommandKind::AiPlayerBehavior { .. }
        | CommandKind::AiPlayerWalkFlags { .. }
        | CommandKind::AiObjectAttackMode { .. }
        | CommandKind::AiObjectWalkFlags { .. }
        | CommandKind::AddResource { .. }
        | CommandKind::KiStart { .. }
        | CommandKind::KiPause { .. }
        | CommandKind::KiStep { .. }
        | CommandKind::SetPlayerResult { .. }
        | CommandKind::StartAnimationLock { .. }
        | CommandKind::StopAnimationLock { .. }
        | CommandKind::ScriptSkipDialog
        | CommandKind::DebugDestroy { .. }
        | CommandKind::ScriptSelection { .. }
        | CommandKind::ScriptButton { .. }
        | CommandKind::Signal { .. }
        | CommandKind::Chat { .. } => {}
    }
}

/// `GameObject_ClearCommandQueue` (`0x00469F20`): all entries free, the read pointer stays.
pub(super) fn clear_queue(sim: &mut Sim, unit: ObjectId) {
    if let Some(o) = sim.object_mut(unit) {
        o.commands.clear();
    }
}

/// What every handler does with `direct` (chopping wood always) after posting per unit:
/// `Unit_PlayResponseSound(u, 0xC)`, then `GameObject_ClearCommandQueue`.
pub(super) fn acknowledge(sim: &mut Sim, unit: ObjectId) {
    respond(sim, unit, RESPONSE_EXECUTE);
    clear_queue(sim, unit);
}

/// Existing units of the sender in command order.
pub(super) fn own_units(sim: &Sim, player: u8, units: &[ObjectId]) -> Vec<ObjectId> {
    units
        .iter()
        .copied()
        .filter(|&u| owns(sim, player, u))
        .collect()
}

/// Construction order `PC({0x101, 1, bld}, fault 0x87F, success 0x87F, HIDs −1/−1, abort 1)`.
fn post_construct(sim: &mut Sim, unit: ObjectId, building: ObjectId) {
    let idle = Message::new(IDLE_WORKER, &[]);
    let env = Envelope {
        msg: Message::new(construct::msg::CONSTRUCT, &[building.as_i32()]),
        success: Some(idle.clone()),
        fault: Some(idle),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    post_command(sim, unit, env, true);
}

/// `P({code, 1, target}, 1)` to every existing own unit, with `direct` followed by emptying
/// (`GameWorld_CmdMineGold` `0x0047C980`, `…Harvest` `0x0047D430`, `…Fish` `0x0047BD70`).
fn post_to_units(
    sim: &mut Sim,
    player: u8,
    units: &[ObjectId],
    code: u32,
    target: ObjectId,
    direct: bool,
) {
    let m = Message::new(code, &[target.as_i32()]);
    for &u in units {
        if !owns(sim, player, u) {
            continue;
        }
        post_message(sim, u, m.clone(), true);
        if direct {
            acknowledge(sim, u);
        }
    }
}

/// Target of Mine gold or Harvest (A8): own building with the flag `GOLDMINE` or `FARM`
/// (`GameObject_MatchesClass`, dead and quartered ones do not match). Construction sites do not have the
/// flags (`PropertyBuilding_StartBuild` strips them).
fn gather_target(sim: &mut Sim, player: u8, target: ObjectId, class: u32, what: &str) -> bool {
    // Script path (190 code 1/2): the original does not check the target (E-214); the gather module reads
    // any target, even a missing or foreign one (test `script_commands_survive_any_target`).
    if script_path(sim) {
        return true;
    }
    let ok = own_building(sim, player, target) && matches_class(sim, target, class);
    if !ok {
        reject(sim, player, what, format!("{} does not match", target.0));
    }
    ok
}

/// Inputs of `0x01`, read from the command variant.
struct SinglePlacement<'a> {
    guid: u32,
    rot: f32,
    pos: [f32; 3],
    workers: &'a [ObjectId],
    border: Option<i32>,
}

/// `GameWorld_CmdPlaceSingleBuilding` (`0x004896D0`, also queued with `enqueue`): A8
/// (occupancy border 0 ... [`MAX_BORDER`], type, build menu of an own worker, prerequisites,
/// one-time type, building site with the first worker: the command position must be the snapped position
/// of the UI, [`check_site`]; on the AI check path only finite and on the map), then
/// like the original: check the costs at the tile of the command position
/// (`Player_CanAffordObject`, not silent -> message 2/1/0/3), pay, **quirk:**
/// HQ lock not until afterwards (costs lost; unreachable through A8, SID 2500 is missing), create the
/// construction site at the command position with the command rotation (`0xD3`, no renewed snapping, no
/// refund on failure), extra word `0x3303 {n}`, then per worker the construction order (or queued
/// `{0x7B, 0x02}` - also only once under Ctrl) and with `direct` emptying.
pub(super) fn place_single_building(
    sim: &mut Sim,
    player: u8,
    kind: &CommandKind,
    direct: bool,
    enqueue_workers: bool,
) {
    let CommandKind::PlaceBuilding {
        guid,
        rotation,
        x,
        y,
        z,
        workers,
        border,
    } = kind
    else {
        return;
    };
    let cmd = SinglePlacement {
        guid: *guid,
        rot: rotation.get(),
        pos: [x.get(), y.get(), z.get()],
        workers,
        border: *border,
    };
    place_single(sim, player, &cmd, direct, enqueue_workers);
}

fn place_single(
    sim: &mut Sim,
    player: u8,
    cmd: &SinglePlacement,
    direct: bool,
    enqueue_workers: bool,
) {
    let guid = cmd.guid;
    if let Some(n) = cmd.border
        && !(0..=MAX_BORDER).contains(&n)
    {
        reject(
            sim,
            player,
            "Place building",
            format!("occupancy border {n} outside 0 ... {MAX_BORDER}"),
        );
        return;
    }
    let workers = own_units(sim, player, cmd.workers);
    if let Err(e) = validate::check_build(sim, player, guid, &workers) {
        reject(sim, player, "Place building", e);
        return;
    }
    let slot = i32::from(player);
    let first = workers.first().copied();
    if sim.origin == Origin::Ki {
        // AI check path: the AI sends the position of the building site search (rotation 0, height 0),
        // never the snapped position of the build preview, and checks no sight (`0x4E84`) - snapping and
        // sight are UI checks and are dropped; the position must be finite and on the map.
        if let Err(e) = check_pose(sim, cmd.pos, cmd.rot) {
            reject(sim, player, "Place building", e);
            return;
        }
    } else {
        if let Err(e) = check_site(sim, guid, cmd.pos, cmd.rot, slot, first) {
            reject(sim, player, "Place building", e);
            return;
        }
        if let Err(e) = check_site_fog(sim, slot, cmd.pos, false) {
            reject(sim, player, "Place building", e);
            return;
        }
    }
    let (pos, rot) = (cmd.pos, cmd.rot);
    // Tile only for the message position (`x / SX` in `f32`, truncated).
    let cell = sim.rules.map.cell;
    let tile = (ftol(pos[0] / cell[0] as f32), ftol(pos[2] / cell[2] as f32));
    if !can_afford_object(sim, slot, guid, false, tile) {
        reject(sim, player, "Place building", "costs not affordable");
        return;
    }
    add_object_costs_percent(sim, slot, guid, -100);
    if HEADQUARTERS.contains(&guid)
        && guid_count(sim, slot, guid).wrapping_add(pending_guid_count(sim, slot, guid)) != 0
    {
        reject(
            sim,
            player,
            "Place building",
            "main building already exists",
        );
        return;
    }
    let Some(b) = create_site(sim, slot, guid, pos, rot) else {
        reject(
            sim,
            player,
            "Place building",
            "building site cannot be created",
        );
        return;
    };
    if let Some(n) = cmd.border {
        send(sim, b, &Message::new(bmsg::EXPAND_OCCUPANCY, &[n]));
    }
    for &w in &workers {
        if sim.object(w).is_none() {
            continue;
        }
        if enqueue_workers {
            let entry = QueuedCommand {
                cyclic: false,
                command: Command {
                    player,
                    kind: CommandKind::ConstructAt {
                        building: b,
                        units: vec![w],
                    },
                },
            };
            enqueue(sim, w, entry);
        } else {
            post_construct(sim, w, b);
            if direct {
                acknowledge(sim, w);
            }
        }
    }
}

/// Position and rotation of a building site (A7/A8, before the building site check): finite and on the map
/// ([`pose_on_map`]); the check itself also rejects such positions, here with a reason.
fn check_pose(sim: &Sim, pos: [f32; 3], rot: f32) -> Result<(), String> {
    if pose_on_map(&sim.rules.map, pos, rot) {
        return Ok(());
    }
    Err(format!(
        "position ({}, {}, {}) with rotation {rot} not finite or outside the map",
        pos[0], pos[1], pos[2]
    ))
}

/// Building site of a command (A8; the original checks nothing in the command): position finite and on the
/// map ([`check_pose`]), the check valid from the command position, and its snapping (tower, gate, field,
/// gold mine; for types with a rotation lock from rotation 0) does **not** change position and rotation -
/// the UI sends the already snapped position ([`snapped_command_pose`]). Building happens at the
/// command position like in the original. Sight is checked afterwards by [`check_site_fog`]. Error:
/// the reason.
fn check_site(
    sim: &mut Sim,
    guid: u32,
    pos: [f32; 3],
    rot: f32,
    slot: i32,
    worker: Option<ObjectId>,
) -> Result<(), String> {
    check_pose(sim, pos, rot)?;
    let Some(snapped) = snapped_command_pose(sim, guid, pos, rot, slot, worker) else {
        return Err(format!("building site for {guid} invalid"));
    };
    if !same_pose(snapped, (pos, rot)) {
        let ([x, y, z], r) = snapped;
        return Err(format!(
            "position for {guid} not snapped: ({}, {}, {}) with rotation {rot}, snapped \
             ({x}, {y}, {z}) with rotation {r}",
            pos[0], pos[1], pos[2]
        ));
    }
    Ok(())
}

/// Sight of the sender on the building site (**deviation from the original under A8**: the original only checks in
/// the build preview `BuildCursor_UpdatePlacement` `0x0044FDB0`): the tile under the command position
/// seen (`IsWorldPosVisibleToPlayer` `0x0047B7B0` without rectangle); wall segments like
/// `BuildCursor_CheckWallSegments` (`0x00450870`): seen or explored, both without rectangle
/// (`0x0047B7B0` or `0x0047B840` with third argument 0, calls at `0x0045099F`/`0x004509B6`).
/// The view switch "Fog off" is only known to the host; it does not apply here.
fn check_site_fog(sim: &Sim, slot: i32, pos: [f32; 3], wall: bool) -> Result<(), String> {
    let view = FogView::new(&*sim.state);
    let map = &sim.rules.map;
    let (ok, what) = if wall {
        (
            view.world_pos_visible(map, pos, slot, false)
                || view.world_pos_explored(map, pos, slot, false),
            "explored",
        )
    } else {
        (view.world_pos_visible(map, pos, slot, false), "seen")
    };
    if ok {
        return Ok(());
    }
    let (kx, kz) = FogView::world_tile(map, pos);
    Err(format!(
        "building site on tile ({kx}, {kz}) is not {what} for player {slot}"
    ))
}

/// Construction site with `ObjDesc` flags `0xD3` (color of the player, `Player_GetColor` `0x004FABC0`).
fn create_site(sim: &mut Sim, slot: i32, guid: u32, pos: [f32; 3], rot: f32) -> Option<ObjectId> {
    let mut desc = ObjectDesc {
        flags: PLACE_FLAGS,
        guid,
        player: slot,
        position: pos,
        scale: [1.0; 3],
        rotation: [0.0, rot, 0.0],
        color: player_color(sim, slot),
    };
    create_object(sim, &mut desc)
}

/// Building site of a part of `0x18` (A8): wall segments like the UI
/// (`BuildCursor_CheckWallSegments` `0x00450870`) with the fixed flags `0x100003` without layer and
/// region (does not snap), all other types like `0x01` ([`check_site`]) with the first
/// worker. Building happens at the position of the part.
fn check_part(
    sim: &mut Sim,
    part: &BuildingPart,
    slot: i32,
    worker: Option<ObjectId>,
) -> Result<(), String> {
    let (pos, rot) = (
        [part.x.get(), part.y.get(), part.z.get()],
        part.rotation.get(),
    );
    let guid = part.guid;
    let wall = building_flags(sim.rules, guid).unwrap_or(0) & pflags::WALL_SEGMENT != 0;
    if !wall {
        check_site(sim, guid, pos, rot, slot, worker)?;
        return check_site_fog(sim, slot, pos, false);
    }
    check_pose(sim, pos, rot)?;
    let invalid = || format!("building site for {guid} invalid");
    let hull = PlacementBox::of(sim.rules, guid).ok_or_else(invalid)?;
    let args = PlacementArgs {
        flags: WALL_SEGMENT_CHECK_FLAGS,
        player: slot,
        layer: -1,
        region: -1,
    };
    let (mut p, mut r) = (pos, rot);
    if check_building_placement(sim, &hull, &mut p, &mut r, &args) {
        check_site_fog(sim, slot, pos, true)
    } else {
        Err(invalid())
    }
}

/// Check of a part in the wall cursor (`BuildCursor_CheckWallSegments` `0x00450870`): for each
/// part (corner like wall segment) `GameEngine_CheckBuildingPlacement` with the fixed flags `0x100003` without layer
/// and region at the position of the part, then seen **or** explored (`0x0047B7B0`/`0x0047B840`, third argument 0)
/// at the position returned by the check. The size rule (`|dx|`, `|dz|` < 4 -> all parts invalid) is checked by the
/// cursor itself. Only reads (the check gets copies of position and rotation).
pub fn wall_cursor_part_valid(
    sim: &mut Sim,
    guid: u32,
    pos: [f32; 3],
    rot: f32,
    slot: i32,
) -> bool {
    let Some(hull) = PlacementBox::of(sim.rules, guid) else {
        return false;
    };
    let args = PlacementArgs {
        flags: WALL_SEGMENT_CHECK_FLAGS,
        player: slot,
        layer: -1,
        region: -1,
    };
    let (mut p, mut r) = (pos, rot);
    if !check_building_placement(sim, &hull, &mut p, &mut r, &args) {
        return false;
    }
    let view = FogView::new(&*sim.state);
    let map = &sim.rules.map;
    view.world_pos_visible(map, p, slot, false) || view.world_pos_explored(map, p, slot, false)
}

/// `GameWorld_CmdPlaceBuilding` (`0x004899B0`, cannot be queued). A8: 1 ... [`MAX_WALL_PARTS`] parts;
/// all of the type of the first or - like the ring of the fort cursor (`FUN_00450BE0`: part 0 a corner,
/// then wall segments and corners) - from exactly one wall/corner pair ([`wall_corner_pair`]); several
/// parts only for wall segments (`0x100000`); build menu, prerequisites and one-time rule with the
/// wall GUID of the pair (the menu lists only the wall) or the type, one-time types once for the whole
/// command; per part position finite and on the map and building site ([`check_part`]); no part on
/// a tile of one already accepted. Then like the original: cost(part 0) * m for gold, wood,
/// food (population simply), first shortage -> message 2/1/0/3 without tile (-1, -1) and end;
/// pay, create the parts in order with their GUID at their command position (**quirk:**
/// if one fails, end without refund and without the link of the parts), `0x1B03 {m − 1, members…}` to
/// the master (part 0, also with m = 1), per worker the construction order to the master and with `direct`
/// emptying. No HQ lock, no `0x3303`.
fn place_buildings(
    sim: &mut Sim,
    player: u8,
    parts: &[BuildingPart],
    workers: &[ObjectId],
    direct: bool,
) {
    let workers = own_units(sim, player, workers);
    let Some(guid) = parts.first().map(|p| p.guid) else {
        reject(sim, player, "Place wall", "no parts");
        return;
    };
    if parts.len() > MAX_WALL_PARTS {
        reject(
            sim,
            player,
            "Place wall",
            format!("{} parts, at most {MAX_WALL_PARTS}", parts.len()),
        );
        return;
    }
    let pair = wall_corner_pair(sim.rules, guid);
    let allowed = |g: u32| g == guid || pair.is_some_and(|(w, c)| g == w || g == c);
    if let Some(p) = parts.iter().find(|p| !allowed(p.guid)) {
        reject(
            sim,
            player,
            "Place wall",
            format!(
                "parts of different types ({guid} and {}; only wall and corner of one pair)",
                p.guid
            ),
        );
        return;
    }
    let wall = building_flags(sim.rules, guid).unwrap_or(0) & pflags::WALL_SEGMENT != 0;
    if parts.len() > 1 && !wall {
        reject(
            sim,
            player,
            "Place wall",
            format!(
                "{} parts of {guid}: several only for wall segments",
                parts.len()
            ),
        );
        return;
    }
    let menu_guid = pair.map_or(guid, |(w, _)| w);
    if let Err(e) = validate::check_build_count(sim, player, menu_guid, &workers, parts.len()) {
        reject(sim, player, "Place wall", e);
        return;
    }
    let slot = i32::from(player);
    let mut placed = Vec::with_capacity(parts.len());
    let mut rects: Vec<[i32; 4]> = Vec::with_capacity(parts.len());
    for p in parts {
        if let Err(e) = check_part(sim, p, slot, workers.first().copied()) {
            reject(sim, player, "Place wall", e);
            return;
        }
        let (pos, rot) = ([p.x.get(), p.y.get(), p.z.get()], p.rotation.get());
        // Parts must not overlap: the building site check only sees existing things, not the
        // parts of the same command. The parts of a ring each lie on their own tile.
        if let Some(h) = PlacementBox::of(sim.rules, p.guid)
            && let Some(rect) = placement_rect(h.mn, h.mx, pos, rot, &sim.rules.map)
        {
            if rects.iter().any(|o| rects_share_a_tile(o, &rect)) {
                reject(
                    sim,
                    player,
                    "Place wall",
                    format!("parts of {menu_guid} overlap"),
                );
                return;
            }
            rects.push(rect);
        }
        placed.push((p.guid, pos, rot));
    }
    let m = parts.len() as i32;
    // Without a value set the original reads stack contents; here free of charge.
    let mut cost = object_costs(sim, slot, guid).unwrap_or([0; 5]);
    for c in cost.iter_mut().take(3) {
        *c = c.wrapping_mul(m);
    }
    let max = sim.state.economy.max_living_space;
    let Some(p) = sim.state.players.get(usize::from(player)) else {
        reject(sim, player, "Place wall", "slot not present");
        return;
    };
    let missing = find_unaffordable_resource(p, max, &cost);
    if missing >= 0 {
        announce(
            sim,
            slot,
            message::MISSING[missing as usize],
            (-1, -1),
            message::flags::PLACE_BUILDINGS,
        );
        reject(sim, player, "Place wall", "costs not affordable");
        return;
    }
    if let Some(p) = sim.state.players.get_mut(usize::from(player)) {
        pay_costs(p, &cost);
    }
    let mut ids = Vec::with_capacity(parts.len());
    for (part_guid, pos, rot) in placed {
        let Some(id) = create_site(sim, slot, part_guid, pos, rot) else {
            reject(sim, player, "Place wall", "building site cannot be created");
            return;
        };
        ids.push(id);
    }
    let master = ids[0];
    let mut args = vec![m - 1];
    args.extend(ids[1..].iter().map(|id| id.as_i32()));
    send(sim, master, &Message::new(bmsg::SET_SLAVE_LIST, &args));
    for &w in &workers {
        if sim.object(w).is_none() {
            continue;
        }
        post_construct(sim, w, master);
        if direct {
            acknowledge(sim, w);
        }
    }
}

/// Two inclusive tile rectangles `[x0, z0, x1, z1]` share a tile.
fn rects_share_a_tile(a: &[i32; 4], b: &[i32; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// `GameWorld_CmdConstructAtBuilding` (`0x0047C8C0`, building and repairing): A8 - own, not
/// dead building (the target filters 412/413 never match `DIED`); per own unit the construction order,
/// with `direct` emptying.
fn construct_at(sim: &mut Sim, player: u8, building: ObjectId, units: &[ObjectId], direct: bool) {
    // Script path (190 code 0): the target is not checked; building a foreign building is the
    // "repair" of the original (E-214).
    if !script_path(sim)
        && (!own_building(sim, player, building) || validate::is_dead(sim, building))
    {
        reject(
            sim,
            player,
            "Build",
            format!("{} is not an own living building", building.0),
        );
        return;
    }
    for &u in units {
        if !owns(sim, player, u) {
            continue;
        }
        post_construct(sim, u, building);
        if direct {
            acknowledge(sim, u);
        }
    }
}

/// `GameWorld_CmdChopWood` (`0x0047CD50`): if the tree is missing, nothing; A8 - target with tree
/// module. Per unit `P({0x108, tree}, 1)`, then **always** acknowledgment and emptying (**quirk:** also
/// executed when queued - the acknowledgment comes, later entries of the queue are lost)
/// and the cyclic entry
/// `{0x7C, {0x7F, tree tile, unit}}` ("next tree"). The original then keeps reading from the
/// entry just released (hypothesis: use-after-free); here the loop runs to the end with the
/// original values.
fn chop_wood(sim: &mut Sim, player: u8, tree_id: ObjectId, units: &[ObjectId]) {
    let script = script_path(sim);
    let Some(t) = sim.object(tree_id) else {
        reject(
            sim,
            player,
            "Chop wood",
            format!("tree {} not present", tree_id.0),
        );
        return;
    };
    // Script path (190 code 6): only the tree must exist, the tree module is dropped (E-214).
    if !script && !t.has_module(tree::INDEX, false) {
        reject(
            sim,
            player,
            "Chop wood",
            format!("{} is not a tree", tree_id.0),
        );
        return;
    }
    let (tile_x, tile_z) = t.tile;
    let m = Message::new(lumber::msg::START, &[tree_id.as_i32()]);
    for &u in units {
        if !owns(sim, player, u) {
            continue;
        }
        post_message(sim, u, m.clone(), true);
        acknowledge(sim, u);
        // The "next tree" sequence runs later as input: on the script path it carries the slot of the
        // unit (the sender is only that of the first), so that it passes the ownership check; for
        // slot 8 the sender stays and the sequence fails later (deviation, E-224).
        let owner = Some(u.slot()).filter(|&p| usize::from(p) < crate::id::PLAYER_SLOTS);
        let entry = QueuedCommand {
            cyclic: true,
            command: Command {
                player: if script {
                    owner.unwrap_or(player)
                } else {
                    player
                },
                kind: CommandKind::NextTree {
                    tile_x,
                    tile_z,
                    unit: u,
                },
            },
        };
        enqueue(sim, u, entry);
    }
}

/// Filter of `0x7F`: AND, module 4 present with bit `0x100` ("slot free"), tree list
/// (`0x1016`), runtime words -1. The remaining words are stack contents in the original; the mode does not
/// read them.
pub fn next_tree_filter() -> MatchFilter {
    MatchFilter {
        mode: 0x1016,
        match_bits: 0x104,
        prop_id: tree::INDEX as u32,
        ..MatchFilter::constructed()
    }
}

/// `GameWorld_CmdLumberNearPosition` (`0x0047BEE0`, type `0x7F`): nearest free tree to the
/// tile corner `(kx, kz) << 12` (whole map, center distance, slot of the owner of the unit),
/// `P({0x108, tree}, 1)`, with `direct` emptying. If no tree is found, the original reads the
/// ID from a null pointer (crash); here nothing happens.
fn next_tree(sim: &mut Sim, player: u8, (kx, kz): (i32, i32), unit: ObjectId, direct: bool) {
    if !owns(sim, player, unit) {
        return;
    }
    let pos = (kx.wrapping_shl(12), kz.wrapping_shl(12));
    let f = next_tree_filter();
    let owner = i32::from(unit.slot());
    let Some(t) = find_nearest_matching(sim, pos, &f, -1, owner, -1) else {
        reject(
            sim,
            player,
            "Next tree",
            format!("unit {}: no tree left", unit.0),
        );
        return;
    };
    post_message(
        sim,
        unit,
        Message::new(lumber::msg::START, &[t.as_i32()]),
        true,
    );
    if direct {
        acknowledge(sim, unit);
    }
}

/// Delivery code per carried kind (`GameWorld_CmdDeliverLoad` `0x0047CE70`): gold `0x607`, wood
/// `0x608`, food with hunting module `0xA16`, with fishing `0x637`, otherwise `0x60C`; other kinds none.
pub fn deliver_code(sim: &Sim, unit: ObjectId) -> Option<u32> {
    let o = sim.object(unit)?;
    match o.load_kind {
        gather::load::GOLD => Some(mining::msg::DELIVER),
        gather::load::WOOD => Some(lumber::msg::DELIVER),
        gather::load::FOOD if o.has_module(HUNTING, false) => Some(DELIVER_HUNTED),
        gather::load::FOOD if o.has_module(FISHING, false) => Some(gather::fishing::msg::DELIVER),
        gather::load::FOOD => Some(harvest::msg::DELIVER),
        _ => None,
    }
}

/// Acceptance class for the delivery code (acceptance bit of the load kind and `CONSTRUCTED_FINISHED`, like the
/// gatherers look for their storage); hunting spoils like food.
fn deliver_class(code: u32) -> u32 {
    match code {
        mining::msg::DELIVER => gather::MINING.deliver_class,
        lumber::msg::DELIVER => gather::LUMBERING.deliver_class,
        gather::fishing::msg::DELIVER => gather::FISHING.deliver_class,
        _ => gather::HARVESTING.deliver_class,
    }
}

/// `GameWorld_CmdDeliverLoad` (`0x0047CE70`): per own unit the code of its load as
/// `PC({code, building, 1}, without feedback, 1)` and with `direct` emptying; without a matching load
/// nothing (not even emptying). The amount is not checked. A8: own building, and per unit it must
/// accept its load kind and be finished (otherwise the unit is skipped).
fn deliver(sim: &mut Sim, player: u8, building: ObjectId, units: &[ObjectId], direct: bool) {
    // Script path (190 code 3): the target is not checked, neither ownership nor acceptance nor completion
    // (E-214); the load kind remains (0 to 2, otherwise skipped).
    let script = script_path(sim);
    if !script && !own_building(sim, player, building) {
        reject(
            sim,
            player,
            "Deliver",
            format!("{} is not an own building", building.0),
        );
        return;
    }
    for &u in units {
        if !owns(sim, player, u) {
            continue;
        }
        let Some(code) = deliver_code(sim, u) else {
            skip_passed(
                sim,
                player,
                "Deliver",
                format!("unit {} carries nothing deliverable", u.0),
            );
            continue;
        };
        let f = MatchFilter {
            mode: 0x150,
            match_bits: deliver_class(code),
            ..MatchFilter::constructed()
        };
        if !script && !matches(sim, building, &f, i32::from(player)) {
            skip_passed(
                sim,
                player,
                "Deliver",
                format!("{} does not accept the load of {}", building.0, u.0),
            );
            continue;
        }
        let env = Envelope::bare(Message::new(code, &[building.as_i32(), 1]));
        post_command(sim, u, env, true);
        if direct {
            acknowledge(sim, u);
        }
    }
}

/// Own container with quarters module (A8), building or mobile container (ship, with
/// walk module). `0x13` checks the target additionally in [`quarter_open`] or [`vehicle_open`],
/// `0x14`/`0x15` act on both kinds (re/ships 2.4, 3.5).
fn own_container(sim: &Sim, player: u8, container: ObjectId) -> bool {
    let ok = sim
        .object(container)
        .is_some_and(|o| container.slot() == player && o.has_module(quarter::INDEX, false));
    if !ok {
        reject(
            sim,
            player,
            "Quarters",
            format!("{} is not own quarters", container.0),
        );
    }
    ok
}

/// The container is a mobile one: an **enabled** walk module (`HasModule(0, 0)`,
/// `0x0047D9EB`) - for every origin (input, AI, script; E-223). A container with a
/// disabled walk module (e.g. a ship after a variant switch) takes the building path
/// ([`quarter_open`]).
fn is_vehicle(sim: &Sim, container: ObjectId) -> bool {
    sim.object(container)
        .is_some_and(|o| o.has_module(VEHICLE_WALK, false))
}

/// Target of `0x13` like the UI (`targets.ini` 414, entry #18 in
/// re/garrison-and-commands): living (not `DIED`), finished (`CONSTRUCTED_FINISHED`) and
/// with free spaces (`FREEFORQUARTER`). Otherwise units would move into a construction site or a
/// sold building and be destroyed with it (A8).
fn quarter_open(sim: &Sim, container: ObjectId) -> bool {
    !validate::is_dead(sim, container)
        && validate::is_finished(sim, container)
        && sim
            .module(container, quarter::INDEX)
            .is_some_and(|m| m.base.has_flags(quarter::flags::FREE_FOR_QUARTER))
}

/// Target of `0x13` for mobile containers like the UI (`targets.ini` 414 "in vehicles",
/// entry #19; re/ships 2.4): living and visible to the sender in the fog (`MM_CHECKFOG`,
/// [`fog_visible`]). **Quirk:** neither free spaces nor capacity are checked - a full ship
/// and the Mayflower (capacity 0) accept the command (W-198).
fn vehicle_open(sim: &Sim, player: u8, container: ObjectId) -> bool {
    !validate::is_dead(sim, container) && fog_visible(sim, container, i32::from(player))
}

/// `GameWorld_CmdQuarterUnits` (`0x0047D8D0`, cannot be queued): if the container is missing, nothing; A8 -
/// own quarters, living and accessible: a finished building with free spaces
/// ([`quarter_open`]), a mobile container visible ([`vehicle_open`]). Units in
/// command order that are own and have the class `CANQUARTER` (living, not
/// quartered).
///
/// **Script path** ([`script_path`], 334, 335, 190 code 5; E-217, re/commands 9): at the container the
/// original only checks that it exists (`GetObjectById(id, 0)`: even dying, foreign, construction site,
/// without quarters module, full, in the fog) and at the units only the class `CANQUARTER`.
/// [`own_container`], [`quarter_open`], [`vehicle_open`] and the ownership of the units ([`own_units`]
/// there means "present") are dropped. If the container has no quarters module, the units walk there and
/// the move-in fizzles (the message `0x1617` hits no module); foreign units are turned away by the quarters on
/// arrival (`accept_unit`). Buildings: per unit `PC(FINDTO {container, −1, 1}, success {0x1617, id} to the
/// container, no fault, abort 1)`, with `direct` emptying; the capacity is not checked until `AcceptUnit` on
/// arrival. Mobile containers: [`quarter_units_vehicle`].
fn quarter_units(sim: &mut Sim, player: u8, container: ObjectId, units: &[ObjectId], direct: bool) {
    if sim.object(container).is_none() {
        reject(
            sim,
            player,
            "Quarters",
            format!("container {} not present", container.0),
        );
        return;
    }
    let script = script_path(sim);
    if !script && !own_container(sim, player, container) {
        return;
    }
    let vehicle = is_vehicle(sim, container);
    let open = script
        || if vehicle {
            vehicle_open(sim, player, container)
        } else {
            quarter_open(sim, container)
        };
    if !open {
        reject(
            sim,
            player,
            "Quarters",
            format!("{} is dead, unfinished, full or hidden", container.0),
        );
        return;
    }
    let mut list: Vec<ObjectId> = own_units(sim, player, units)
        .into_iter()
        .filter(|&u| {
            let ok = matches_class(sim, u, CAN_QUARTER);
            if !ok {
                skip_passed(
                    sim,
                    player,
                    "Quarters",
                    format!("unit {} cannot be quartered", u.0),
                );
            }
            ok
        })
        .collect();
    if vehicle {
        // Deviation (re/ships 3.1, W-207): each unit only once, in command order.
        let mut seen = std::collections::BTreeSet::new();
        list.retain(|u| seen.insert(u.0));
        quarter_units_vehicle(sim, player, container, &list, direct);
        return;
    }
    for u in list {
        let env = Envelope {
            msg: Message::new(0x200, &[container.as_i32(), -1, 1]),
            success: Some(Message::new(quarter::msg::ENTER, &[u.as_i32()])),
            fault: None,
            success_target: container,
            fault_target: ObjectId::NONE,
        };
        post_command(sim, u, env, true);
        if direct {
            acknowledge(sim, u);
        }
    }
}

/// Branch "container has walk" of `GameWorld_CmdQuarterUnits` (`0x0047D9EB…0x0047DC3E`, re/ships
/// 3.1). `list` are the quarterable own units in command order without duplicates
/// (empty -> nothing). Target radius `hard(container) + hard(first) + 1024`.
///
/// - If the meeting point is already set (`0x2017` returns 1, status 1), all units go there and there is
///   **no** `0x1E17`: the new units are not in the wait list (W-199).
/// - Otherwise meeting points `a` (land) and `b` (water) from [`compute_boarding_points`] for the first
///   unit; without a result the whole command lapses. `0x1E17 {a, list…}` goes with abort to the
///   container (it forgets waiting units and sail order), then `FINDTO(b)` with success `0x1F17` to the
///   container itself (`success_target` empty = own object, `send_success`; fault empty - a
///   failed path leaves status 1 standing, W-202); the units walk to `a`.
/// - **Deviation (W-207):** The wait list carries the filtered `list` instead of the raw command IDs
///   of the original (`c.ids`). Raw IDs would let own objects without walk move in at `0x1F17`
///   (`0x2C00` -> -1 != 0) and could carry duplicates or overlong lists into the savegame. The UI only sends
///   selection units; whoever is not in `list` gets no path and never parks - for the waiting ones this is not
///   observable.
/// - Per unit `PC(FINDTO(a), success {0x1617, u} to the container, abort 1)`, with `direct` emptying.
fn quarter_units_vehicle(
    sim: &mut Sim,
    player: u8,
    container: ObjectId,
    list: &[ObjectId],
    direct: bool,
) {
    let Some(&first) = list.first() else { return };
    let (Some(c), Some(f)) = (sim.object(container), sim.object(first)) else {
        return;
    };
    let radius = c
        .hard_radius
        .wrapping_add(f.hard_radius)
        .wrapping_add(VEHICLE_APPROACH);
    let mut walk = find_to_msg(0, 0, radius, NO_DIRECTION, -1, false, 1);
    let set_goal = |walk: &mut Message, p: (i32, i32)| {
        walk.args[0] = p.0;
        walk.args[1] = p.1;
    };
    let mut point = Message::new(quarter::msg::GET_MEETING_POINT, &[-1, -1]);
    if send_mut(sim, container, &mut point) == 1 {
        set_goal(&mut walk, (point.arg(0), point.arg(1)));
    } else {
        let Some((a, b)) = compute_boarding_points(sim, first, container) else {
            reject(
                sim,
                player,
                "Quarters",
                format!("no meeting point for {} and {}", first.0, container.0),
            );
            return;
        };
        let mut args = vec![a.0, a.1];
        args.extend(list.iter().map(|u| u.as_i32()));
        let set = Message::new(quarter::msg::SET_MEETING_POINT, &args);
        post_message(sim, container, set, true);
        set_goal(&mut walk, b);
        let sail = Envelope {
            msg: walk.clone(),
            success: Some(Message::new(quarter::msg::CONTAINER_ARRIVED, &[])),
            fault: None,
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        };
        post_command(sim, container, sail, false);
        set_goal(&mut walk, a);
    }
    for &u in list {
        let env = Envelope {
            msg: walk.clone(),
            success: Some(Message::new(quarter::msg::ENTER, &[u.as_i32()])),
            fault: None,
            success_target: container,
            fault_target: ObjectId::NONE,
        };
        post_command(sim, u, env, true);
        if direct {
            acknowledge(sim, u);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit::*;
    use crate::command::{F32Bits, Outcome, process_queue};
    use crate::economy::resource::{FOOD, GOLD, LIVING_SPACE, POPULATION, WOOD};
    use crate::fx::Fx;
    use crate::message::Posted;
    use crate::module::building::fort_testkit::{CORNER, fort_ring};
    use crate::module::building::{building, building_mut};
    use crate::module::construct::{construct, state as cstate};
    use crate::module::gather::{load, set_carried_load};
    use crate::motion::NO_DIRECTION;
    use crate::object::status;
    use crate::script::world::view::lock_button;
    use crate::state::SimEvent;
    use std::f32::consts::FRAC_PI_2;

    fn objects_of(sim: &Sim, guid: u32) -> Vec<ObjectId> {
        sim.state
            .objects()
            .filter(|o| o.guid == guid)
            .map(|o| o.id)
            .collect()
    }

    fn posted(sim: &Sim, u: ObjectId) -> Vec<Posted> {
        sim.object(u).unwrap().fifo.iter().cloned().collect()
    }

    fn codes(sim: &Sim, u: ObjectId) -> Vec<u32> {
        posted(sim, u).iter().map(Posted::code).collect()
    }

    fn queue(sim: &Sim, u: ObjectId) -> Vec<CommandKind> {
        sim.object(u)
            .unwrap()
            .commands
            .iter()
            .map(|q| q.command.kind.clone())
            .collect()
    }

    fn res(sim: &Sim, i: usize) -> i32 {
        sim.state.resource(0, i)
    }

    fn messages(sim: &Sim) -> Vec<(u8, i32, (i32, i32))> {
        sim.state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::PlayerMessage {
                    to: Some(player),
                    id,
                    tile,
                    ..
                } => Some((*player, *id, *tile)),
                _ => None,
            })
            .collect()
    }

    /// Flag words of the messages in order.
    fn message_flags(sim: &Sim) -> Vec<u32> {
        sim.state
            .events
            .iter()
            .filter_map(|e| match e {
                SimEvent::PlayerMessage { flags, .. } => Some(*flags),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn missing_resources_carry_priority_3() {
        // `Player_CanAffordObject` `0x0047D500`: flag `0x03010000` (priority 3, queue).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (24, 26));
        account(&mut sim, 0, [100, 149, 100, 0, 10]);
        run(&mut sim, 0, place(HOUSE, (24, 20), &[w]));
        assert_eq!(messages(&sim), [(0, message::NO_WOOD, (24, 20))]);
        assert_eq!(message_flags(&sim), [0x0301_0000]);
    }

    #[test]
    fn place_buildings_lack_carries_priority_4_without_tile() {
        // `GameWorld_CmdPlaceBuilding` `0x004899B0`: flag `0x04010000`, location (-1, -1).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (30, 40));
        account(&mut sim, 0, [0, 14, 0, 0, 10]);
        let part = |t: i32| BuildingPart {
            guid: WALL,
            rotation: F32Bits::new(0.0),
            x: F32Bits::new(world(t)),
            y: F32Bits::new(0.0),
            z: F32Bits::new(world(30)),
        };
        run(
            &mut sim,
            0,
            CommandKind::PlaceBuildings {
                parts: vec![part(30), part(31), part(32)],
                workers: vec![w],
            },
        );
        assert_eq!(messages(&sim), [(0, message::NO_WOOD, (-1, -1))]);
        assert_eq!(message_flags(&sim), [0x0401_0000]);
    }

    /// Walking to tile (2, 2), as filler of the queue.
    fn walk_away(u: ObjectId) -> CommandKind {
        CommandKind::Move {
            units: vec![u],
            x: 2 << 12,
            z: 2 << 12,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: crate::command::NO_SPREAD,
            flags: 0,
        }
    }

    fn queued(inner: CommandKind) -> CommandKind {
        CommandKind::Queue {
            cyclic: false,
            inner: Box::new(inner),
        }
    }

    #[test]
    fn placing_a_building_without_wood_reports_message_1_and_builds_nothing() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (24, 26));
        account(&mut sim, 0, [100, 149, 100, 0, 10]);
        run(&mut sim, 0, place(HOUSE, (24, 20), &[w]));
        assert!(objects_of(&sim, HOUSE).is_empty());
        assert_eq!(messages(&sim), [(0, message::NO_WOOD, (24, 20))]);
        assert_eq!(res(&sim, WOOD), 149);
        assert!(sim.object(w).unwrap().fifo.is_empty());
    }

    /// Finca with the rotation lock of the shipped data (flags `0x10000200`) and fertile soil.
    fn field_rules() -> crate::rules::Rules {
        let types = TYPES.replace("VALUEID 40003 6 1 512\n", "VALUEID 40003 6 1 268435968\n");
        rules_from(&format!("{types}{SOIL_TYPE}"))
    }

    fn place_at(guid: u32, pos: [f32; 3], rot: f32, workers: &[ObjectId]) -> CommandKind {
        CommandKind::PlaceBuilding {
            guid,
            rotation: F32Bits::new(rot),
            x: F32Bits::new(pos[0]),
            y: F32Bits::new(pos[1]),
            z: F32Bits::new(pos[2]),
            workers: workers.to_vec(),
            border: None,
        }
    }

    #[test]
    fn a_site_stands_at_the_command_pose_which_must_be_the_snapped_one() {
        // The original builds at the command position without renewed snapping (`0x004896D0`); A8 only accepts
        // the position the UI sends (snapped, rotation lock).
        use crate::placement::cursor_placement;
        let rules = field_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let soil = spawn(&mut sim, SOIL, 10, (20, 20));
        account(&mut sim, 0, [0, 5000, 0, 0, 10]);
        // On the deposit itself: the check snaps to the tile boundary -> rejected, nothing paid.
        let raw = sim.object(soil).unwrap().world_pos;
        run(&mut sim, 0, place_at(FARM, raw, 0.0, &[w]));
        assert!(objects_of(&sim, FARM).is_empty());
        assert_eq!(res(&sim, WOOD), 5000);
        // Like the build cursor (mouse next to the deposit, a quarter turn): snapped, rotation 0.
        let ground = [world(21), 0.0, world(19)];
        let c = cursor_placement(&mut sim, FARM, ground, FRAC_PI_2, 0, Some(w)).unwrap();
        assert!(c.valid);
        assert_eq!(c.rot, 0.0);
        assert_ne!(c.pos, raw);
        // Rotated despite the rotation lock: rejected.
        run(&mut sim, 0, place_at(FARM, c.pos, FRAC_PI_2, &[w]));
        assert!(objects_of(&sim, FARM).is_empty());
        // Snapped: construction site exactly at the command position.
        run(&mut sim, 0, place_at(FARM, c.pos, 0.0, &[w]));
        let farms = objects_of(&sim, FARM);
        assert_eq!(farms.len(), 1);
        let o = sim.object(farms[0]).unwrap();
        let fx = |v: f32| Fx(Fx::from_f32(v).0 / 128);
        assert_eq!((o.grid.x, o.grid.z), (fx(c.pos[0]), fx(c.pos[2])));
        assert_eq!(o.rotation[1], 0.0);
        assert_eq!(res(&sim, WOOD), 5000 - 300);
        // A house (without snapping in the check) stands at every free command position.
        let free = [world(40) + 3.25, 0.0, world(40) - 17.5];
        run(&mut sim, 0, place_at(HOUSE, free, 0.3, &[w]));
        let house = objects_of(&sim, HOUSE);
        assert_eq!(house.len(), 1);
        let o = sim.object(house[0]).unwrap();
        assert_eq!((o.grid.x, o.grid.z), (fx(free[0]), fx(free[2])));
        assert_eq!(o.rotation[1], 0.3);
    }

    #[test]
    fn a_border_outside_0_to_20_tiles_is_rejected() {
        // Without a limit `0x3303 {i32::MAX}` would run into an overflow and a large border over
        // arbitrarily many tiles (and off the map the savegame accepts).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (24, 26));
        account(&mut sim, 0, [0, 1000, 0, 0, 10]);
        let with_border = |n| {
            let mut cmd = place(HOUSE, (32, 32), &[w]);
            if let CommandKind::PlaceBuilding { border, .. } = &mut cmd {
                *border = Some(n);
            }
            cmd
        };
        for n in [-1, MAX_BORDER + 1, i32::MAX, i32::MIN] {
            run(&mut sim, 0, with_border(n));
            assert!(objects_of(&sim, HOUSE).is_empty(), "border {n}");
            assert_eq!(res(&sim, WOOD), 1000, "border {n}");
        }
        run(&mut sim, 0, with_border(MAX_BORDER));
        let house = objects_of(&sim, HOUSE);
        assert_eq!(house.len(), 1);
        let b = crate::module::building::building(&sim, house[0]).unwrap();
        assert_eq!(b.occupancy_rect[0], b.rect[0] - MAX_BORDER);
        assert_eq!(b.occupancy_rect[3], b.rect[3] + MAX_BORDER);
        assert!(sim.state.occupancy.test(b.rect[0] - MAX_BORDER, b.rect[1]));
    }

    #[test]
    fn a_house_next_to_the_hq_becomes_a_site_and_the_workers_walk_there() {
        let rules = rules();
        let mut state = state(&rules);
        let (w1, w2, house) = {
            let mut sim = Sim::new(&mut state, &rules);
            spawn(&mut sim, HQ, 0, (20, 20));
            let w1 = spawn(&mut sim, COLONIST, 0, (24, 26));
            let w2 = spawn(&mut sim, COLONIST, 0, (26, 26));
            account(&mut sim, 0, [0, 1000, 0, 0, 10]);
            // On the HQ: building site occupied, rejected.
            run(&mut sim, 0, place(HOUSE, (20, 20), &[w1, w2]));
            assert!(objects_of(&sim, HOUSE).is_empty());
            // Next to it: construction site, paid, "in progress" 1, construction order with abort to both;
            // the extra word `0x3303 {1}` lays an occupancy border around the rectangle 23...25 x 19...21.
            assert!(!sim.state.occupancy.test(22, 18));
            let mut cmd = place(HOUSE, (24, 20), &[w1, w2]);
            if let CommandKind::PlaceBuilding { border, .. } = &mut cmd {
                *border = Some(1);
            }
            run(&mut sim, 0, cmd);
            assert!(sim.state.occupancy.test(22, 18) && sim.state.occupancy.test(26, 22));
            let house = objects_of(&sim, HOUSE);
            assert_eq!(house.len(), 1);
            let house = house[0];
            assert!(!validate::is_finished(&sim, house));
            let o = sim.object(house).unwrap();
            assert_eq!((o.render_bits, o.progress_scale), (4, 0.1));
            assert_eq!(res(&sim, WOOD), 850);
            assert_eq!(pending_guid_count(&sim, 0, HOUSE), 1);
            for w in [w1, w2] {
                let p = posted(&sim, w);
                let Posted::Command { env, abort: true } = &p[0] else {
                    panic!("{p:?}");
                };
                assert_eq!(env.msg, Message::new(0x101, &[house.as_i32()]));
                assert_eq!(env.success, Some(Message::new(0x87F, &[])));
                assert_eq!(env.fault, Some(Message::new(0x87F, &[])));
            }
            (w1, w2, house)
        };
        for _ in 0..6 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        let sim = Sim::new(&mut state, &rules);
        for w in [w1, w2] {
            let c = construct(&sim, w).unwrap();
            assert_eq!(c.building, house);
            assert!(
                [cstate::STARTED, cstate::WALKING].contains(&c.status),
                "{}",
                c.status
            );
        }
    }

    // QUIRK W-103: deviation
    #[test]
    fn requirements_once_buildings_and_build_menus_are_checked() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (30, 40));
        let boat = spawn(&mut sim, BOAT, 0, (34, 40));
        let foreign = spawn(&mut sim, COLONIST, 1, (36, 40));
        account(&mut sim, 0, [0, 5000, 0, 0, 10]);
        // Barracks without HQ: prerequisite missing.
        run(&mut sim, 0, place(BARRACKS, (10, 10), &[w]));
        assert!(objects_of(&sim, BARRACKS).is_empty());
        // Without a build menu (boat) or with a foreign worker: rejected; likewise without a building module.
        run(&mut sim, 0, place(HOUSE, (10, 10), &[boat]));
        run(&mut sim, 0, place(HOUSE, (10, 10), &[foreign]));
        let trees = objects_of(&sim, TREE).len();
        run(&mut sim, 0, place(TREE, (10, 10), &[w]));
        assert!(objects_of(&sim, HOUSE).is_empty());
        assert_eq!(objects_of(&sim, TREE).len(), trees);
        assert_eq!(res(&sim, WOOD), 5000);
        // Nonsensical positions (NaN, infinite, far outside the map) and rotations: no building site,
        // no panic - not even for a wall segment whose check only looks at the first tile
        // (outside the map "free"), as `0x01` and as a part of `0x18`.
        let bad = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1.0e30,
            3.0e9,
            -64.0,
        ];
        for guid in [HOUSE, WALL] {
            for v in bad {
                // The rotation only non-finite (a large finite one is valid).
                for axis in 0..if v.is_finite() { 2 } else { 3 } {
                    let mut cmd = place(guid, (10, 10), &[w]);
                    if let CommandKind::PlaceBuilding { rotation, x, z, .. } = &mut cmd {
                        let word = match axis {
                            0 => x,
                            1 => z,
                            _ => rotation,
                        };
                        *word = F32Bits::new(v);
                    }
                    run(&mut sim, 0, cmd);
                }
                let part = BuildingPart {
                    guid: WALL,
                    rotation: F32Bits::new(0.0),
                    x: F32Bits::new(v),
                    y: F32Bits::new(0.0),
                    z: F32Bits::new(world(10)),
                };
                let turned = BuildingPart {
                    rotation: F32Bits::new(v),
                    x: F32Bits::new(world(10)),
                    ..part
                };
                let both = if v.is_finite() {
                    vec![part]
                } else {
                    vec![part, turned]
                };
                for p in both {
                    let parts = vec![p];
                    let cmd = CommandKind::PlaceBuildings {
                        parts,
                        workers: vec![w],
                    };
                    run(&mut sim, 0, cmd);
                }
            }
        }
        assert!(objects_of(&sim, HOUSE).is_empty());
        assert!(objects_of(&sim, WALL).is_empty());
        assert_eq!(res(&sim, WOOD), 5000);
        // First HQ: yes; second (in progress 1): ONLYONCE.
        run(&mut sim, 0, place(HQ, (10, 10), &[w]));
        run(&mut sim, 0, place(HQ, (10, 20), &[w]));
        assert_eq!(objects_of(&sim, HQ).len(), 1);
        assert_eq!(res(&sim, WOOD), 4500);
        // The construction site does not count toward the object counter: the barracks stay locked.
        run(&mut sim, 0, place(BARRACKS, (10, 20), &[w]));
        assert!(objects_of(&sim, BARRACKS).is_empty());
        // With a finished HQ (object counter 1): barracks yes, another HQ no.
        spawn(&mut sim, HQ, 0, (40, 10));
        run(&mut sim, 0, place(BARRACKS, (10, 20), &[w]));
        run(&mut sim, 0, place(HQ, (20, 30), &[w]));
        assert_eq!(objects_of(&sim, BARRACKS).len(), 1);
        assert_eq!(objects_of(&sim, HQ).len(), 2);
        // Another player may build their first HQ.
        account(&mut sim, 1, [0, 1000, 0, 0, 10]);
        run(&mut sim, 1, place(HQ, (50, 50), &[foreign]));
        assert_eq!(objects_of(&sim, HQ).len(), 3);
    }

    // QUIRK W-109: kept
    #[test]
    fn a_wall_of_three_parts_has_a_master_with_two_links_and_costs_three_times() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (30, 40));
        account(&mut sim, 0, [0, 14, 0, 0, 10]);
        let part = |t: i32, guid: u32| BuildingPart {
            guid,
            rotation: F32Bits::new(0.0),
            x: F32Bits::new(world(t)),
            y: F32Bits::new(0.0),
            z: F32Bits::new(world(30)),
        };
        let wall = |guid2: u32| CommandKind::PlaceBuildings {
            parts: vec![part(30, WALL), part(31, WALL), part(32, guid2)],
            workers: vec![w],
        };
        // 3 * 5 wood > 14: message 1 without tile, nothing built.
        run(&mut sim, 0, wall(WALL));
        assert!(objects_of(&sim, WALL).is_empty());
        assert_eq!(messages(&sim), [(0, message::NO_WOOD, (-1, -1))]);
        account(&mut sim, 0, [0, 100, 0, 0, 10]);
        // Mixed types: rejected (A8).
        run(&mut sim, 0, wall(HOUSE));
        assert!(objects_of(&sim, WALL).is_empty());
        run(&mut sim, 0, wall(WALL));
        let parts = objects_of(&sim, WALL);
        assert_eq!(parts.len(), 3);
        assert_eq!(res(&sim, WOOD), 85);
        let master = building(&sim, parts[0]).unwrap();
        assert_eq!(master.slaves, [parts[1], parts[2]]);
        for &s in &parts[1..] {
            assert_eq!(building(&sim, s).unwrap().master, parts[0]);
        }
        let p = posted(&sim, w);
        let [Posted::Command { env, abort: true }] = &p[..] else {
            panic!("{p:?}");
        };
        assert_eq!(env.msg, Message::new(0x101, &[parts[0].as_i32()]));
        // On the occupied tiles: rejected.
        run(&mut sim, 0, wall(WALL));
        assert_eq!(objects_of(&sim, WALL).len(), 3);
        // Two parts on the same tile (or a shared one): rejected, nothing paid.
        let wood = res(&sim, WOOD);
        for (a, b) in [(40, 40), (41, 41)] {
            let mut second = part(b, WALL);
            second.x = F32Bits::new(world(b) + 10.0);
            let cmd = CommandKind::PlaceBuildings {
                parts: vec![part(a, WALL), second],
                workers: vec![w],
            };
            run(&mut sim, 0, cmd);
        }
        assert_eq!(objects_of(&sim, WALL).len(), 3);
        assert_eq!(res(&sim, WOOD), wood);
        // Several parts only for wall segments; a single house via `0x18` works.
        let houses = |n: i32| CommandKind::PlaceBuildings {
            parts: (0..n).map(|k| part(10 + 4 * k, HOUSE)).collect(),
            workers: vec![w],
        };
        account(&mut sim, 0, [0, 1000, 0, 0, 10]);
        run(&mut sim, 0, houses(2));
        assert!(objects_of(&sim, HOUSE).is_empty());
        run(&mut sim, 0, houses(1));
        assert_eq!(objects_of(&sim, HOUSE).len(), 1);
    }

    fn wall_rules() -> crate::rules::Rules {
        let corner =
            "GUID 1012\nNAME SPA_Ecke\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 1589248\n";
        let mut r = rules_from(&format!("{TYPES}{corner}"));
        r.defaults
            .insert(CORNER, [(11, 4), (13, 6), (14, 1000)].into_iter().collect());
        let wall_shape = r.building_shapes[&WALL].clone();
        r.building_shapes.insert(CORNER, wall_shape);
        let wall_box = r.model_boxes[&WALL];
        r.model_boxes.insert(CORNER, wall_box);
        r
    }

    #[test]
    fn a_fort_ring_of_corners_and_walls_is_accepted_and_built() {
        // The UI sends `0x18` as a ring of corners and wall segments (part 0 a corner); the build
        // menu lists only the wall. A8 must not reject such a command.
        let rules = wall_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        account(&mut sim, 0, [0, 1000, 0, 0, 10]);
        let ring = fort_ring((20, 20), 4, 4);
        assert_eq!(ring.len(), 20);
        let cmd = |parts: Vec<BuildingPart>| CommandKind::PlaceBuildings {
            parts,
            workers: vec![w],
        };
        // With a part of another type: rejected.
        let mut foreign = ring.clone();
        foreign[3].guid = HOUSE;
        run(&mut sim, 0, cmd(foreign));
        assert!(objects_of(&sim, WALL).is_empty() && objects_of(&sim, CORNER).is_empty());
        run(&mut sim, 0, cmd(ring.clone()));
        let (corners, walls) = (objects_of(&sim, CORNER), objects_of(&sim, WALL));
        assert_eq!((corners.len(), walls.len()), (4, 16));
        // Cost of part 0 (corner, 4 wood) times 20.
        assert_eq!(res(&sim, WOOD), 1000 - 4 * 20);
        // Master is part 0 (the first corner), the other 19 are members; each part stands at
        // its command position with its rotation.
        let master = building(&sim, corners[0]).unwrap();
        assert_eq!(master.slaves.len(), 19);
        let ids: Vec<ObjectId> = sim
            .state
            .objects()
            .filter(|o| [WALL, CORNER].contains(&o.guid))
            .map(|o| o.id)
            .collect();
        assert_eq!(ids[0], corners[0]);
        assert_eq!(master.slaves, ids[1..]);
        for (id, part) in ids.iter().zip(&ring) {
            let o = sim.object(*id).unwrap();
            assert_eq!(o.guid, part.guid);
            assert_eq!(o.rotation[1].to_bits(), part.rotation.get().to_bits());
            assert_eq!(
                o.tile,
                (ftol(part.x.get() / 128.0), ftol(part.z.get() / 128.0))
            );
        }
        let p = posted(&sim, w);
        let [Posted::Command { env, abort: true }] = &p[..] else {
            panic!("{p:?}");
        };
        assert_eq!(env.msg, Message::new(0x101, &[corners[0].as_i32()]));
        // Largest ring of the UI (22 x 22, 92 parts) works, more does not (A8).
        account(&mut sim, 0, [0, 10_000, 0, 0, 10]);
        let big = fort_ring((30, 30), 22, 22);
        assert_eq!(big.len(), MAX_WALL_PARTS);
        let mut too_big = big.clone();
        too_big.push(BuildingPart {
            x: F32Bits::new(world(10)),
            z: F32Bits::new(world(60)),
            ..big[1]
        });
        run(&mut sim, 0, cmd(too_big));
        assert_eq!(objects_of(&sim, WALL).len(), 16);
        run(&mut sim, 0, cmd(big));
        assert_eq!(objects_of(&sim, WALL).len(), 16 + 88);
        // A single corner also checks the menu with the wall.
        let single = vec![BuildingPart {
            x: F32Bits::new(world(50)),
            z: F32Bits::new(world(50)),
            ..ring[0]
        }];
        run(&mut sim, 0, cmd(single));
        assert_eq!(objects_of(&sim, CORNER).len(), 4 + 4 + 1);
    }

    #[test]
    fn wall_pairs_are_derived_from_the_flags() {
        let rules = wall_rules();
        assert_eq!(wall_corner_pair(&rules, WALL), Some((WALL, CORNER)));
        assert_eq!(wall_corner_pair(&rules, CORNER), Some((WALL, CORNER)));
        assert_eq!(wall_corner_pair(&rules, HOUSE), None);
        // Without a corner (wall + 1 missing) no pair.
        assert_eq!(
            wall_corner_pair(&super::super::testkit::rules(), WALL),
            None
        );
    }

    #[test]
    fn a_once_only_type_is_once_per_wall_command() {
        // A one-time wall segment (synthetic: button ONLYONCE) applies to the whole command.
        let mut rules = rules();
        for b in rules.toolbar.menus.get_mut(&110).unwrap() {
            if b.msg == WALL as i32 {
                b.flags = (b.flags & !opennml_formats::gifcon::button_kind::KIND_MASK)
                    | opennml_formats::gifcon::button_kind::ONLYONCE;
            }
        }
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (30, 40));
        account(&mut sim, 0, [0, 100, 0, 0, 10]);
        let part = |t: i32| BuildingPart {
            guid: WALL,
            rotation: F32Bits::new(0.0),
            x: F32Bits::new(world(t)),
            y: F32Bits::new(0.0),
            z: F32Bits::new(world(30)),
        };
        let wall = |n: i32| CommandKind::PlaceBuildings {
            parts: (0..n).map(|k| part(30 + k)).collect(),
            workers: vec![w],
        };
        run(&mut sim, 0, wall(2));
        assert!(objects_of(&sim, WALL).is_empty());
        run(&mut sim, 0, wall(1));
        assert_eq!(objects_of(&sim, WALL).len(), 1);
        run(&mut sim, 0, wall(1));
        assert_eq!(objects_of(&sim, WALL).len(), 1);
    }

    // QUIRK W-137: kept
    #[test]
    fn queued_chop_wood_clears_later_entries_and_appends_the_next_tree_entry() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let t1 = spawn(&mut sim, TREE, 9, (14, 12));
        let t2 = spawn(&mut sim, TREE, 9, (20, 12));
        let hq = spawn(&mut sim, HQ, 0, (30, 30));
        // No tree: nothing, not even emptying.
        run(&mut sim, 0, queued(walk_away(w)));
        run(
            &mut sim,
            0,
            CommandKind::ChopWood {
                tree: hq,
                units: vec![w],
            },
        );
        assert_eq!(queue(&sim, w).len(), 1);
        assert!(codes(&sim, w).is_empty());
        // Queued: chop wood, then walk.
        sim.object_mut(w).unwrap().commands.clear();
        let chop = CommandKind::ChopWood {
            tree: t1,
            units: vec![w],
        };
        run(&mut sim, 0, queued(chop.clone()));
        run(&mut sim, 0, queued(walk_away(w)));
        assert_eq!(queue(&sim, w), [chop, walk_away(w)]);
        // Execute: 0x108 with abort; the queue is emptied (the walk is lost,
        // quirk) and has the cyclic "next tree" at the tree tile.
        assert!(process_queue(&mut sim, w));
        let p = posted(&sim, w);
        assert!(matches!(
            &p[..],
            [Posted::Message { msg, abort: true }] if *msg == Message::new(0x108, &[t1.as_i32()])
        ));
        let next = CommandKind::NextTree {
            tile_x: 14,
            tile_z: 12,
            unit: w,
        };
        assert_eq!(queue(&sim, w), std::slice::from_ref(&next));
        assert!(sim.object(w).unwrap().commands[0].cyclic);
        // The cyclic entry looks for the next tree to the tile corner and stays.
        sim.object_mut(w).unwrap().fifo.clear();
        crate::world::destroy_object(&mut sim, t1);
        assert!(process_queue(&mut sim, w));
        let p = posted(&sim, w);
        assert!(matches!(
            &p[..],
            [Posted::Message { msg, abort: true }] if *msg == Message::new(0x108, &[t2.as_i32()])
        ));
        assert_eq!(queue(&sim, w), [next]);
        // Foreign unit: nothing.
        let foreign = spawn(&mut sim, COLONIST, 1, (12, 10));
        run(
            &mut sim,
            0,
            CommandKind::ChopWood {
                tree: t2,
                units: vec![foreign],
            },
        );
        assert!(codes(&sim, foreign).is_empty() && queue(&sim, foreign).is_empty());
    }

    // QUIRK W-141: kept
    #[test]
    fn delivering_uses_the_code_of_the_load_and_an_accepting_finished_building() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let boat = spawn(&mut sim, BOAT, 0, (12, 10));
        let store = spawn(&mut sim, STORE, 0, (20, 20));
        let store_site = site(&mut sim, STORE, 0, (30, 20));
        let house = spawn(&mut sim, HOUSE, 0, (20, 30));
        let foreign = spawn(&mut sim, STORE, 1, (40, 40));
        let deliver = |building| CommandKind::Deliver {
            building,
            units: vec![w],
        };
        for (kind, code) in [
            (load::GOLD, 0x607),
            (load::WOOD, 0x608),
            (load::FOOD, 0x60C),
        ] {
            set_carried_load(&mut sim, w, 10, kind);
            sim.object_mut(w).unwrap().fifo.clear();
            run(&mut sim, 0, deliver(store));
            let p = posted(&sim, w);
            let [Posted::Command { env, abort: true }] = &p[..] else {
                panic!("{p:?}");
            };
            assert_eq!(
                *env,
                Envelope::bare(Message::new(code, &[store.as_i32(), 1]))
            );
        }
        // Construction site, house without acceptance bit, foreign storage: nothing.
        sim.object_mut(w).unwrap().fifo.clear();
        for b in [store_site, house, foreign] {
            run(&mut sim, 0, deliver(b));
        }
        assert!(codes(&sim, w).is_empty());
        // Without a load nothing (not even emptying the queue).
        set_carried_load(&mut sim, w, 0, load::NONE);
        run(&mut sim, 0, queued(walk_away(w)));
        run(&mut sim, 0, deliver(store));
        assert!(codes(&sim, w).is_empty());
        assert_eq!(queue(&sim, w).len(), 1);
        // Fish (boat) have the code 0x637 and need a harbor.
        set_carried_load(&mut sim, boat, 10, load::FOOD);
        assert_eq!(deliver_code(&sim, boat), Some(0x637));
        run(
            &mut sim,
            0,
            CommandKind::Deliver {
                building: store,
                units: vec![boat],
            },
        );
        assert!(codes(&sim, boat).is_empty());
    }

    // QUIRK W-47: deviation
    #[test]
    fn gathering_building_and_fishing_check_their_targets() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let boat = spawn(&mut sim, BOAT, 0, (12, 10));
        let mine = spawn(&mut sim, MINE, 0, (20, 20));
        let farm = spawn(&mut sim, FARM, 0, (30, 20));
        let house_site = site(&mut sim, HOUSE, 0, (20, 30));
        let foreign_site = site(&mut sim, HOUSE, 1, (30, 30));
        let foreign_mine = spawn(&mut sim, MINE, 1, (40, 20));
        let basket = spawn(&mut sim, BASKET, 10, (5, 30));
        let tree = spawn(&mut sim, TREE, 9, (5, 40));
        let check = |sim: &mut Sim, u: ObjectId, kind: CommandKind, want: Option<u32>| {
            sim.object_mut(u).unwrap().fifo.clear();
            sim.object_mut(u).unwrap().commands.clear();
            run(sim, 0, queued(walk_away(u)));
            run(sim, 0, kind.clone());
            let c = codes(sim, u);
            match want {
                // Accepted: the message, and the queue emptied directly.
                Some(code) => {
                    assert_eq!(c, [code], "{kind:?}");
                    assert!(queue(sim, u).is_empty(), "{kind:?}");
                }
                None => {
                    assert!(c.is_empty(), "{kind:?}");
                    assert_eq!(queue(sim, u).len(), 1, "{kind:?}");
                }
            }
        };
        let u = || vec![w];
        let cases = [
            (CommandKind::MineGold { mine, units: u() }, Some(0x107)),
            (
                CommandKind::MineGold {
                    mine: farm,
                    units: u(),
                },
                None,
            ),
            (
                CommandKind::MineGold {
                    mine: foreign_mine,
                    units: u(),
                },
                None,
            ),
            (CommandKind::Harvest { farm, units: u() }, Some(0x10C)),
            (
                CommandKind::Harvest {
                    farm: mine,
                    units: u(),
                },
                None,
            ),
            (
                CommandKind::ConstructAt {
                    building: house_site,
                    units: u(),
                },
                Some(0x101),
            ),
            (
                CommandKind::ConstructAt {
                    building: foreign_site,
                    units: u(),
                },
                None,
            ),
            (
                CommandKind::ConstructAt {
                    building: tree,
                    units: u(),
                },
                None,
            ),
        ];
        for (kind, want) in cases {
            check(&mut sim, w, kind, want);
        }
        let fish = |basket| CommandKind::Fish {
            basket,
            units: vec![boat],
        };
        check(&mut sim, boat, fish(basket), Some(0x137));
        check(&mut sim, boat, fish(tree), None);
        // Foreign unit: skipped.
        let foreign = spawn(&mut sim, COLONIST, 1, (14, 10));
        let kind = CommandKind::MineGold {
            mine,
            units: vec![foreign],
        };
        run(&mut sim, 0, kind);
        assert!(codes(&sim, foreign).is_empty());
    }

    #[test]
    fn production_trains_colonists_trades_researches_and_refuses_rails() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        let hq_site = site(&mut sim, HQ, 0, (40, 20));
        let market = spawn(&mut sim, MARKET, 0, (20, 40));
        let make = |building, guid| CommandKind::EnqueueProduction { building, guid };
        let running = |sim: &Sim, b| {
            let b = building(sim, b).unwrap();
            (b.make_guid, b.make_queue.len())
        };
        // Rails at the headquarters (the button 2518 is only in the menu of the station,
        // `rail_orders_need_a_station`), GUID 0, unknown type, building instead of training, foreign
        // sender, construction site, wrong menu, research in the wrong menu: rejected.
        for (player, b, guid) in [
            (0, market, 2806),
            (0, hq, 2518),
            (0, hq, 0),
            (0, hq, 4711),
            (0, hq, HOUSE),
            (1, hq, COLONIST),
            (0, hq_site, COLONIST),
            (0, market, COLONIST),
        ] {
            run(&mut sim, player, make(b, guid));
            assert_eq!(running(&sim, b), (0, 0), "{guid}");
        }
        // Research 2806 is accepted, runs 10 s and takes effect at the end (second headquarters, so that
        // the queue of the first stays untouched).
        let hq2 = spawn(&mut sim, HQ, 0, (40, 40));
        run(&mut sim, 0, make(hq2, 2806));
        assert_eq!(running(&sim, hq2), (2806, 0));
        assert_eq!(pending_guid_count(&sim, 0, 2806), 1);
        // One-time: while it is "in progress", not again.
        assert_eq!(
            validate::check_production(&sim, 0, hq2, 2806),
            Err("2806 exists only once".into())
        );
        for _ in 0..101 {
            building_tick(&mut sim, hq2);
        }
        assert_eq!(running(&sim, hq2), (0, 0));
        assert_eq!(guid_count(&sim, 0, 2806), 1);
        assert_eq!(sim.state.applied_upgrades[0], [2806]);
        // Colonist in the HQ, trade in the trading post.
        run(&mut sim, 0, make(hq, COLONIST));
        run(&mut sim, 0, make(hq, COLONIST));
        assert_eq!(running(&sim, hq), (COLONIST, 1));
        run(&mut sim, 0, make(market, 490));
        assert_eq!(running(&sim, market), (490, 0));
        // Cancel: only the owner.
        let cancel = CommandKind::CancelProduction {
            building: hq,
            index: 1,
            arg: 0,
        };
        run(&mut sim, 1, cancel.clone());
        assert_eq!(running(&sim, hq), (COLONIST, 1));
        // Outside the queue and without a running order: rejected (otherwise "in progress" would drop
        // for GUID 0).
        let pending = |sim: &Sim| {
            (
                pending_guid_count(sim, 0, 0),
                pending_guid_count(sim, 0, COLONIST),
            )
        };
        let before = pending(&sim);
        for (b, index) in [(hq, 2), (hq, -1), (hq, i32::MAX), (hq_site, 0), (market, 1)] {
            let c = CommandKind::CancelProduction {
                building: b,
                index,
                arg: 0,
            };
            run(&mut sim, 0, c);
        }
        assert_eq!(pending(&sim), before);
        assert_eq!(running(&sim, hq), (COLONIST, 1));
        assert_eq!(
            validate::check_cancel(&sim, 0, hq_site, 0),
            Err(format!(
                "no order with index 0 in {} (running 0, 0 waiting)",
                hq_site.0
            ))
        );
        run(&mut sim, 0, cancel);
        assert_eq!(running(&sim, hq), (COLONIST, 0));
        // Reasons name the ID as a number.
        assert_eq!(
            validate::check_production(&sim, 1, hq, COLONIST),
            Err(format!("{} is not an own building", hq.0))
        );
        assert_eq!(
            validate::check_production(&sim, 0, hq_site, COLONIST),
            Err(format!("{} is not finished or dead", hq_site.0))
        );
    }

    // QUIRK W-223: deviation
    #[test]
    fn research_of_a_disabled_guid_is_rejected() {
        // `Player_IsGuidDisabled`: config 1 = 0 in the value set of the slot locks the GUID for this
        // slot; without config 1 in the set it stays free (re/upgrades 10.4).
        let make = |building, guid| CommandKind::EnqueueProduction { building, guid };
        let set = |rules: &mut crate::rules::Rules, slot: u8, guid: u32, values: &[(u32, i32)]| {
            rules
                .level
                .slot_configs
                .entry(slot)
                .or_default()
                .insert(guid, values.iter().copied().collect());
        };
        let mut rules = rules();
        set(&mut rules, 0, RESEARCH, &[(1, 0), (13, 10)]);
        set(&mut rules, 0, HOUSE, &[(1, 0)]);
        let mut st = state(&rules);
        let mut sim = Sim::new(&mut st, &rules);
        let hq0 = spawn(&mut sim, HQ, 0, (20, 20));
        let hq1 = spawn(&mut sim, HQ, 1, (40, 20));
        let worker = spawn(&mut sim, COLONIST, 0, (10, 10));
        let gated = |sim: &Sim, b| {
            let b = building(sim, b).unwrap();
            (b.make_guid, b.make_queue.len())
        };
        assert_eq!(
            validate::check_production(&sim, 0, hq0, RESEARCH),
            Err(format!("{RESEARCH} is locked on this map"))
        );
        run(&mut sim, 0, make(hq0, RESEARCH));
        assert_eq!(gated(&sim, hq0), (0, 0));
        assert_eq!(pending_guid_count(&sim, 0, RESEARCH), 0);
        // Slot 1 has no set of its own (and `default.dat` no config 1): free.
        assert_eq!(validate::check_production(&sim, 1, hq1, RESEARCH), Ok(()));
        run(&mut sim, 1, make(hq1, RESEARCH));
        assert_eq!(gated(&sim, hq1), (RESEARCH, 0));
        // The same lock applies to building (`0x01`/`0x18`): slot 0 may not place a house,
        // another type stays allowed.
        assert_eq!(
            validate::check_build(&sim, 0, HOUSE, &[worker]),
            Err(format!("{HOUSE} is locked on this map"))
        );
        assert_eq!(validate::check_build(&sim, 0, STORE, &[worker]), Ok(()));
        run(&mut sim, 0, place(HOUSE, (14, 14), &[worker]));
        assert_eq!(objects_of(&sim, HOUSE).len(), 0);
        // A set without config 1 does not lock (own world: the value 0 of other configs does not count).
        let mut rules = self::rules();
        set(&mut rules, 0, RESEARCH, &[(13, 10)]);
        let mut st = state(&rules);
        let mut sim = Sim::new(&mut st, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        assert_eq!(validate::check_production(&sim, 0, hq, RESEARCH), Ok(()));
        run(&mut sim, 0, make(hq, RESEARCH));
        assert_eq!(gated(&sim, hq), (RESEARCH, 0));
        // Config 1 = 1 is free.
        let mut rules = self::rules();
        set(&mut rules, 0, RESEARCH, &[(1, 1), (13, 10)]);
        let mut st = state(&rules);
        let mut sim = Sim::new(&mut st, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        assert_eq!(validate::check_production(&sim, 0, hq, RESEARCH), Ok(()));
    }

    // QUIRK E-164: kept
    #[test]
    fn button_lock_207_blocks_build_and_production_for_every_slot() {
        // The script lock 207 applies to all slots (`GameWorld+0x21E0` is global) and takes effect in the
        // checks "like the command bar".
        let rules = rules();
        let mut st = state(&rules);
        let mut sim = Sim::new(&mut st, &rules);
        let hq0 = spawn(&mut sim, HQ, 0, (20, 20));
        let hq1 = spawn(&mut sim, HQ, 1, (40, 20));
        let worker = spawn(&mut sim, COLONIST, 0, (10, 10));
        assert_eq!(validate::check_production(&sim, 0, hq0, RESEARCH), Ok(()));
        assert_eq!(validate::check_production(&sim, 1, hq1, RESEARCH), Ok(()));
        assert_eq!(validate::check_build(&sim, 0, HOUSE, &[worker]), Ok(()));
        lock_button(&mut sim, RESEARCH, true);
        lock_button(&mut sim, HOUSE, true);
        let locked = |guid: u32| Err(format!("{guid} is locked on this map"));
        assert_eq!(
            validate::check_production(&sim, 0, hq0, RESEARCH),
            locked(RESEARCH)
        );
        assert_eq!(
            validate::check_production(&sim, 1, hq1, RESEARCH),
            locked(RESEARCH)
        );
        assert_eq!(
            validate::check_build(&sim, 0, HOUSE, &[worker]),
            locked(HOUSE)
        );
        // Another type stays free; 208 releases again (the entry stays with 0).
        assert_eq!(validate::check_build(&sim, 0, STORE, &[worker]), Ok(()));
        lock_button(&mut sim, RESEARCH, false);
        lock_button(&mut sim, HOUSE, false);
        assert_eq!(sim.state.button_locks.get(&RESEARCH), Some(&0));
        assert_eq!(validate::check_production(&sim, 0, hq0, RESEARCH), Ok(()));
        assert_eq!(validate::check_build(&sim, 0, HOUSE, &[worker]), Ok(()));
    }

    // QUIRK E-200: kept
    #[test]
    fn button_unlock_208_cannot_lift_level_lock() {
        // `Player_IsGuidDisabled` (`0x0047DE80`): a script byte 0 falls back to the level data; 208
        // therefore only lifts the script lock (E-200).
        let mut rules = rules();
        rules
            .level
            .slot_configs
            .entry(0)
            .or_default()
            .insert(HOUSE, [(1, 0)].into_iter().collect());
        let mut st = state(&rules);
        let mut sim = Sim::new(&mut st, &rules);
        let worker0 = spawn(&mut sim, COLONIST, 0, (10, 10));
        let worker1 = spawn(&mut sim, COLONIST, 1, (50, 10));
        let locked = Err(format!("{HOUSE} is locked on this map"));
        assert_eq!(validate::check_build(&sim, 0, HOUSE, &[worker0]), locked);
        assert_eq!(validate::check_build(&sim, 1, HOUSE, &[worker1]), Ok(()));
        lock_button(&mut sim, HOUSE, false);
        assert_eq!(validate::check_build(&sim, 0, HOUSE, &[worker0]), locked);
        // 207 also locks the slot without a level data lock; 208 releases this one again, slot 0 not.
        lock_button(&mut sim, HOUSE, true);
        assert_eq!(validate::check_build(&sim, 1, HOUSE, &[worker1]), locked);
        lock_button(&mut sim, HOUSE, false);
        assert_eq!(validate::check_build(&sim, 1, HOUSE, &[worker1]), Ok(()));
        assert_eq!(validate::check_build(&sim, 0, HOUSE, &[worker0]), locked);
        assert!(crate::world::is_guid_disabled(&sim, 0, HOUSE));
        assert!(!crate::world::is_guid_disabled(&sim, 1, HOUSE));
    }

    #[test]
    fn counter_upgrades_cannot_be_produced_in_buildings() {
        // A `COUNTERUPGRADE` button (missionary, menu 601) executes a unit; if put in a building
        // menu, production rejects it (the UI opens no production there).
        let mut rules = rules();
        let button = opennml_formats::gifcon::ToolbarButton {
            msg: 3374,
            flags: opennml_formats::gifcon::button_kind::COUNTERUPGRADE,
            ..Default::default()
        };
        rules.upgrades.insert(3374);
        rules
            .toolbar
            .menus
            .get_mut(&(HQ as i32))
            .unwrap()
            .push(button.clone());
        rules.toolbar.entries.insert(3374, button);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        assert_eq!(
            validate::check_production(&sim, 0, hq, 3374),
            Err("counter-upgrade 3374 executes a unit".into())
        );
        run(
            &mut sim,
            0,
            CommandKind::EnqueueProduction {
                building: hq,
                guid: 3374,
            },
        );
        let b = building(&sim, hq).unwrap();
        assert_eq!((b.make_guid, b.make_queue.len()), (0, 0));
        assert_eq!(pending_guid_count(&sim, 0, 3374), 0);
        // The ordinary research of the same menu stays possible.
        assert_eq!(validate::check_production(&sim, 0, hq, RESEARCH), Ok(()));
    }

    // QUIRK W-19: kept
    #[test]
    fn rail_orders_need_a_station() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        let station = spawn(&mut sim, STATION, 0, (20, 40));
        let foreign = spawn(&mut sim, STATION, 1, (40, 40));
        let make = |building, guid| CommandKind::EnqueueProduction { building, guid };
        let running = |sim: &Sim, b| {
            let b = building(sim, b).unwrap();
            (b.make_guid, b.make_queue.len())
        };
        // At the headquarters the button is not in the menu: rejected.
        assert_eq!(
            validate::check_production(&sim, 0, hq, 2518),
            Err(format!("2518 is not in the menu of {HQ}"))
        );
        run(&mut sim, 0, make(hq, 2518));
        assert_eq!(running(&sim, hq), (0, 0));
        // At the own station accepted (running order, "in progress" 1), at the foreign one not.
        assert_eq!(validate::check_production(&sim, 0, station, 2518), Ok(()));
        run(&mut sim, 0, make(station, 2518));
        assert_eq!(running(&sim, station), (2518, 0));
        assert_eq!(pending_guid_count(&sim, 0, 2518), 1);
        run(&mut sim, 0, make(foreign, 2518));
        assert_eq!(running(&sim, foreign), (0, 0));
        // A second order queues up.
        run(&mut sim, 0, make(station, 2518));
        assert_eq!(running(&sim, station), (2518, 1));
    }

    // QUIRK W-76: deviation
    #[test]
    fn building_and_cancelling_at_dead_buildings_is_rejected() {
        // A8: The UI offers neither building/repairing (target filters 412/413, `DIED` never
        // matches) nor the production bar for dead buildings (`PropertyBuilding_Destroy` removes the
        // building from the selection).
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let house = site(&mut sim, HOUSE, 0, (20, 20));
        let hq = spawn(&mut sim, HQ, 0, (30, 30));
        account(&mut sim, 0, [1000, 1000, 1000, 0, 10]);
        run(
            &mut sim,
            0,
            CommandKind::EnqueueProduction {
                building: hq,
                guid: COLONIST,
            },
        );
        run(
            &mut sim,
            0,
            CommandKind::EnqueueProduction {
                building: hq,
                guid: COLONIST,
            },
        );
        let running = |sim: &Sim| {
            let b = building(sim, hq).unwrap();
            (b.make_guid, b.make_queue.len())
        };
        assert_eq!(running(&sim), (COLONIST, 1));
        for b in [house, hq] {
            crate::module::building::destroy::destroy(&mut sim, b);
            assert!(validate::is_dead(&sim, b));
        }
        let before = running(&sim);
        run(
            &mut sim,
            0,
            CommandKind::ConstructAt {
                building: house,
                units: vec![w],
            },
        );
        assert!(codes(&sim, w).is_empty());
        for index in [0, 1] {
            run(
                &mut sim,
                0,
                CommandKind::CancelProduction {
                    building: hq,
                    index,
                    arg: 0,
                },
            );
        }
        assert_eq!(running(&sim), before);
        assert_eq!(
            validate::check_cancel(&sim, 0, hq, 0),
            Err(format!("{} is dead", hq.0))
        );
    }

    /// Several buildings: one own is sold, a foreign one skipped -> `Partial`.
    #[test]
    fn selling_several_buildings_reports_partial_effects() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let own = site(&mut sim, HOUSE, 0, (20, 20));
        let foreign = site(&mut sim, HOUSE, 1, (30, 20));
        let sell = |buildings: Vec<ObjectId>| Command {
            player: 0,
            kind: CommandKind::SellBuildings { buildings },
        };
        let out = crate::command::execute(&mut sim, &sell(vec![foreign, own]), true);
        assert!(
            matches!(&out, Outcome::Partial(why) if why.contains(&foreign.0.to_string())),
            "{out:?}"
        );
        assert!(validate::is_dead(&sim, own), "the own one is sold");
        assert!(!validate::is_dead(&sim, foreign));
        // Only foreign (or already sold): rejected.
        let out = crate::command::execute(&mut sim, &sell(vec![foreign, own]), true);
        assert!(matches!(out, Outcome::Rejected(_)), "{out:?}");
        // Rally point: own + foreign.
        let own2 = site(&mut sim, HOUSE, 0, (40, 20));
        let rally = |buildings: Vec<ObjectId>| Command {
            player: 0,
            kind: CommandKind::SetRallyPoint {
                x: 0,
                z: 0,
                buildings,
            },
        };
        let out = crate::command::execute(&mut sim, &rally(vec![own2, foreign]), true);
        assert!(matches!(out, Outcome::Partial(_)), "{out:?}");
        let out = crate::command::execute(&mut sim, &rally(vec![own2]), true);
        assert_eq!(out, Outcome::Accepted);
    }

    // QUIRK W-82: deviation
    #[test]
    fn selling_works_for_own_living_buildings_only() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let own = site(&mut sim, HOUSE, 0, (20, 20));
        let foreign = site(&mut sim, HOUSE, 1, (30, 20));
        let wood0 = sim.state.players[0].resource_raw(WOOD);
        let wood1 = sim.state.players[1].resource_raw(WOOD);
        let sell = |b| CommandKind::SellBuildings { buildings: vec![b] };
        run(&mut sim, 0, sell(foreign));
        assert_eq!(sim.state.players[1].resource_raw(WOOD), wood1);
        assert!(!validate::is_dead(&sim, foreign));
        // Own construction site: 1 % of the remaining value back, then dead.
        run(&mut sim, 0, sell(own));
        let refund = sim.state.players[0].resource_raw(WOOD) - wood0;
        assert!(refund > 0);
        assert!(validate::is_dead(&sim, own));
        // Already dead: no second sale (the original refunded again).
        run(&mut sim, 0, sell(own));
        assert_eq!(sim.state.players[0].resource_raw(WOOD) - wood0, refund);
    }

    /// E-223: A container only counts as a vehicle with an **enabled** walk module (`HasModule(0, 0)`,
    /// `0x0047D9EB`), on every path. A building with a disabled walk module takes the building path
    /// (`quarter_open`, `FINDTO {container, −1, 1}`, move-in via `0x1617`), not the vehicle path.
    // QUIRK E-223: kept
    #[test]
    fn external_quarter_treats_a_container_with_disabled_walk_as_a_building() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        // The walk module of the colonist (module 0) as the disabled module 0 of the headquarters.
        let mut walk = sim.module(w, VEHICLE_WALK).expect("walk").clone();
        walk.base.enabled = false;
        sim.object_mut(hq).unwrap().modules[VEHICLE_WALK] = Some(walk);
        assert!(!is_vehicle(&sim, hq), "disabled: no vehicle");
        run(
            &mut sim,
            0,
            CommandKind::Quarter {
                container: hq,
                units: vec![w],
            },
        );
        let p = posted(&sim, w);
        let [Posted::Command { env, abort: true }] = &p[..] else {
            panic!("building path expected: {p:?}");
        };
        assert_eq!(env.msg, Message::new(0x200, &[hq.as_i32(), -1, 1]));
        assert_eq!(env.success, Some(Message::new(0x1617, &[w.as_i32()])));
        assert_eq!((env.success_target, &env.fault), (hq, &None));
        // Move-in like after arrival.
        send(&mut sim, hq, &Message::new(0x1617, &[w.as_i32()]));
        assert_ne!(sim.object(w).unwrap().status & status::QUARTERED, 0);
        // Counter-check: enabled it counts as a vehicle (sight check instead of finished/space check).
        sim.object_mut(w).unwrap().status &= !status::QUARTERED;
        sim.object_mut(w).unwrap().fifo.clear();
        sim.module_mut(hq, VEHICLE_WALK).unwrap().base.enabled = true;
        assert!(is_vehicle(&sim, hq));
    }

    // QUIRK W-136: deviation
    #[test]
    fn quartering_and_rally_points_need_own_containers() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        let house = spawn(&mut sim, HOUSE, 0, (30, 20));
        let foreign_hq = spawn(&mut sim, HQ, 1, (40, 20));
        let hq_site = site(&mut sim, HQ, 0, (40, 40));
        let sold_hq = spawn(&mut sim, HQ, 0, (50, 40));
        run(
            &mut sim,
            0,
            CommandKind::SellBuildings {
                buildings: vec![sold_hq],
            },
        );
        assert!(validate::is_dead(&sim, sold_hq));
        let quarter = |container| CommandKind::Quarter {
            container,
            units: vec![w],
        };
        // No quarters, foreign, construction site, sold (dying): rejected.
        for c in [house, foreign_hq, hq_site, sold_hq] {
            run(&mut sim, 0, quarter(c));
        }
        assert!(codes(&sim, w).is_empty());
        run(&mut sim, 0, quarter(hq));
        let p = posted(&sim, w);
        let [Posted::Command { env, abort: true }] = &p[..] else {
            panic!("{p:?}");
        };
        assert_eq!(env.msg, Message::new(0x200, &[hq.as_i32(), -1, 1]));
        assert_eq!(env.success, Some(Message::new(0x1617, &[w.as_i32()])));
        assert_eq!((env.success_target, &env.fault), (hq, &None));
        // Move in like after arrival, then unquarter: only the owner.
        send(&mut sim, hq, &Message::new(0x1617, &[w.as_i32()]));
        assert_ne!(sim.object(w).unwrap().status & status::QUARTERED, 0);
        // A quartered unit is not selectable: no commands.
        sim.object_mut(w).unwrap().fifo.clear();
        let tree = spawn(&mut sim, TREE, 9, (12, 12));
        run(&mut sim, 0, walk_away(w));
        run(
            &mut sim,
            0,
            CommandKind::ChopWood {
                tree,
                units: vec![w],
            },
        );
        assert!(codes(&sim, w).is_empty());
        assert!(queue(&sim, w).is_empty());
        run(&mut sim, 1, CommandKind::UnquarterAll { container: hq });
        assert_ne!(sim.object(w).unwrap().status & status::QUARTERED, 0);
        run(
            &mut sim,
            0,
            CommandKind::Unquarter {
                container: hq,
                unit: w,
            },
        );
        assert_eq!(sim.object(w).unwrap().status & status::QUARTERED, 0);
        // Rally point: own buildings yes, foreign no.
        let before = building(&sim, foreign_hq).unwrap().meeting_point;
        run(
            &mut sim,
            0,
            CommandKind::SetRallyPoint {
                x: 5 << 12,
                z: 6 << 12,
                buildings: vec![hq, foreign_hq],
            },
        );
        let mp = building(&sim, hq).unwrap().meeting_point;
        assert_eq!((mp.x.0, mp.z.0), (5 << 12, 6 << 12));
        assert_eq!(building(&sim, foreign_hq).unwrap().meeting_point, before);
    }

    // QUIRK W-140: kept
    #[test]
    fn queued_variants_follow_execute_queued_command() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (24, 26));
        let boat = spawn(&mut sim, BOAT, 0, (12, 10));
        let basket = spawn(&mut sim, BASKET, 10, (5, 30));
        account(&mut sim, 0, [0, 1000, 0, 0, 10]);
        // Building under Ctrl: building immediately, the worker gets a one-time 0x02.
        let ctrl = |inner| CommandKind::Queue {
            cyclic: true,
            inner: Box::new(inner),
        };
        run(&mut sim, 0, ctrl(place(HOUSE, (24, 20), &[w])));
        let house = objects_of(&sim, HOUSE)[0];
        assert_eq!(res(&sim, WOOD), 850);
        assert!(codes(&sim, w).is_empty());
        let o = sim.object(w).unwrap();
        assert_eq!(o.commands.len(), 1);
        assert!(!o.commands[0].cyclic);
        assert_eq!(
            o.commands[0].command.kind,
            CommandKind::ConstructAt {
                building: house,
                units: vec![w]
            }
        );
        // Gold queued per unit (cyclic stays cyclic).
        let mine = spawn(&mut sim, MINE, 0, (40, 40));
        let gold = CommandKind::MineGold {
            mine,
            units: vec![w],
        };
        run(&mut sim, 0, ctrl(gold));
        let o = sim.object(w).unwrap();
        assert_eq!(o.commands.len(), 2);
        assert!(o.commands[1].cyclic);
        // Fishing runs immediately and leaves the queue standing (cannot be queued).
        run(&mut sim, 0, queued(walk_away(boat)));
        let fish = CommandKind::Fish {
            basket,
            units: vec![boat],
        };
        run(&mut sim, 0, queued(fish));
        assert_eq!(codes(&sim, boat), [0x137]);
        assert_eq!(queue(&sim, boat).len(), 1);
    }

    /// End to end with commands only: build a house (living space +15), train a colonist in the HQ
    /// (food -60, one unit more), buy wood at the trading post, chop wood and deliver.
    #[test]
    fn the_economy_can_be_built_up_with_commands_only() {
        let rules = rules();
        let mut state = state(&rules);
        let (hq, market, w1, w2, tree) = {
            let mut sim = Sim::new(&mut state, &rules);
            let hq = spawn(&mut sim, HQ, 0, (20, 20));
            let market = spawn(&mut sim, MARKET, 0, (20, 26));
            let w1 = spawn(&mut sim, COLONIST, 0, (26, 24));
            let w2 = spawn(&mut sim, COLONIST, 0, (28, 24));
            let tree = spawn(&mut sim, TREE, 9, (50, 50));
            account(&mut sim, 0, [1000, 1000, 1000, 0, 5]);
            (hq, market, w1, w2, tree)
        };
        let cmd = |kind| [Command { player: 0, kind }];
        // 1. House.
        let living0 = state.resource(0, LIVING_SPACE);
        crate::tick::step(&mut state, &rules, &cmd(place(HOUSE, (26, 20), &[w1, w2])));
        assert_eq!(state.resource(0, WOOD), 850);
        let house = objects_of(&Sim::new(&mut state, &rules), HOUSE)[0];
        run_until(&mut state, &rules, 3000, |sim| {
            validate::is_finished(sim, house)
        });
        assert_eq!(state.resource(0, LIVING_SPACE), living0 + 15);
        // 2. Train a colonist.
        let colonists = objects_of(&Sim::new(&mut state, &rules), COLONIST).len();
        let food0 = state.resource(0, FOOD);
        let train = CommandKind::EnqueueProduction {
            building: hq,
            guid: COLONIST,
        };
        crate::tick::step(&mut state, &rules, &cmd(train));
        run_until(&mut state, &rules, 10, |sim| {
            sim.state.resource(0, FOOD) == food0 - 60
        });
        run_until(&mut state, &rules, 400, |sim| {
            objects_of(sim, COLONIST).len() == colonists + 1
        });
        // 3. Trade: buy 100 wood.
        let (gold0, wood0) = (state.resource(0, GOLD), state.resource(0, WOOD));
        let trade = CommandKind::EnqueueProduction {
            building: market,
            guid: 490,
        };
        crate::tick::step(&mut state, &rules, &cmd(trade));
        run_until(&mut state, &rules, 400, |sim| {
            sim.state.resource(0, WOOD) == wood0 + 100
        });
        assert!(state.resource(0, GOLD) < gold0);
        // 4. Chop wood and deliver.
        let wood1 = state.resource(0, WOOD);
        let chop = CommandKind::ChopWood {
            tree,
            units: vec![w1],
        };
        crate::tick::step(&mut state, &rules, &cmd(chop));
        assert!(matches!(
            queue(&Sim::new(&mut state, &rules), w1)[..],
            [CommandKind::NextTree {
                tile_x: 50,
                tile_z: 50,
                ..
            }]
        ));
        run_until(&mut state, &rules, 5000, |sim| {
            sim.state.resource(0, WOOD) > wood1
        });
        assert_eq!(state.resource(0, WOOD), wood1 + 20);
    }
    /// Debug "complete immediately" at `building`, order `index` of type `guid`.
    fn complete(building: ObjectId, index: i32, guid: u32) -> CommandKind {
        CommandKind::DebugCompleteOrder {
            building,
            index,
            guid,
        }
    }

    /// A building tick (`PropertyBuilding_Tick(100)`).
    fn building_tick(sim: &mut Sim, b: ObjectId) {
        crate::module::tick(sim, b, crate::module::building::INDEX, 100);
    }

    /// Running order, length of the queue, remaining and total time.
    fn order(sim: &Sim, b: ObjectId) -> (u32, usize, i32, i32) {
        let b = building(sim, b).unwrap();
        (b.make_guid, b.make_queue.len(), b.make_time, b.make_start)
    }

    fn train(sim: &mut Sim, hq: ObjectId, n: usize) {
        for _ in 0..n {
            run(
                sim,
                0,
                CommandKind::EnqueueProduction {
                    building: hq,
                    guid: COLONIST,
                },
            );
        }
    }

    #[test]
    fn completing_a_running_order_has_the_effects_of_the_normal_end() {
        // Two identical worlds: in one the colonist runs for 15 s, in the other it is completed
        // immediately after the start. Output, counters and account are equal.
        let rules = rules();
        let world = || {
            let mut state = state(&rules);
            let hq = {
                let mut sim = Sim::new(&mut state, &rules);
                let hq = spawn(&mut sim, HQ, 0, (20, 20));
                account(&mut sim, 0, [0, 0, 100, 0, 5]);
                train(&mut sim, hq, 1);
                building_tick(&mut sim, hq);
                hq
            };
            (state, hq)
        };
        let outcome = |sim: &Sim| {
            let units: Vec<_> = sim
                .state
                .objects()
                .filter(|o| o.guid == COLONIST)
                .map(|o| (o.id, o.world_pos.map(f32::to_bits), o.tile))
                .collect();
            (
                units,
                guid_count(sim, 0, COLONIST),
                pending_guid_count(sim, 0, COLONIST),
                sim.state.players[0].resources_fx,
            )
        };
        let (mut normal, hq) = world();
        let mut sim = Sim::new(&mut normal, &rules);
        assert_eq!(order(&sim, hq), (COLONIST, 0, 15_000, 15_000));
        for _ in 0..150 {
            building_tick(&mut sim, hq);
        }
        let expected = outcome(&sim);
        assert_eq!(expected.0.len(), 1);
        assert_eq!((expected.1, expected.2), (1, 0));
        let (mut debug, hq) = world();
        let mut sim = Sim::new(&mut debug, &rules);
        run(&mut sim, 0, complete(hq, 0, COLONIST));
        assert_eq!(outcome(&sim), expected);
        assert_eq!(order(&sim, hq), (0, 0, 0, 0));
        let making = sim
            .module(hq, crate::module::building::INDEX)
            .is_some_and(|m| m.base.has_flags(bflags::MAKINGGUID));
        assert!(!making);
    }

    #[test]
    fn completing_hands_over_to_the_next_order_which_starts_in_the_next_tick() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        account(&mut sim, 0, [0, 0, 200, 0, 5]);
        train(&mut sim, hq, 2);
        building_tick(&mut sim, hq);
        assert_eq!(res(&sim, FOOD), 140);
        run(&mut sim, 0, complete(hq, 0, COLONIST));
        assert_eq!(objects_of(&sim, COLONIST).len(), 1);
        assert_eq!(order(&sim, hq), (COLONIST, 0, 0, 0));
        assert_eq!(pending_guid_count(&sim, 0, COLONIST), 1);
        building_tick(&mut sim, hq);
        assert_eq!(res(&sim, FOOD), 80);
        assert_eq!(order(&sim, hq), (COLONIST, 0, 15_000, 15_000));
    }

    #[test]
    fn a_waiting_order_passes_the_start_check_and_finishes_at_once() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        account(&mut sim, 0, [0, 0, 200, 0, 5]);
        train(&mut sim, hq, 3);
        building_tick(&mut sim, hq);
        assert_eq!(res(&sim, FOOD), 140);
        assert_eq!(order(&sim, hq), (COLONIST, 2, 15_000, 15_000));
        // The second waiting one: paid like at the start (-60 food, population +1) and finished;
        // the running and the first waiting one stay.
        run(&mut sim, 0, complete(hq, 2, COLONIST));
        assert_eq!((res(&sim, FOOD), res(&sim, POPULATION)), (80, 2));
        assert_eq!(objects_of(&sim, COLONIST).len(), 1);
        assert_eq!(order(&sim, hq), (COLONIST, 1, 15_000, 15_000));
        assert_eq!(guid_count(&sim, 0, COLONIST), 1);
        assert_eq!(pending_guid_count(&sim, 0, COLONIST), 2);
        assert!(messages(&sim).is_empty());
    }

    #[test]
    fn an_unaffordable_order_stays_with_the_message_of_the_original() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        account(&mut sim, 0, [0, 0, 60, 0, 5]);
        train(&mut sim, hq, 2);
        building_tick(&mut sim, hq);
        let before = (order(&sim, hq), sim.state.players[0].resources_fx);
        let tile = sim.object(hq).unwrap().tile;
        // Waiting one without food: nothing booked, "food missing" - on every click.
        for n in 1..=2 {
            run(&mut sim, 0, complete(hq, 1, COLONIST));
            assert_eq!((order(&sim, hq), sim.state.players[0].resources_fx), before);
            assert_eq!(messages(&sim), vec![(0, message::NO_FOOD, tile); n]);
        }
        assert!(objects_of(&sim, COLONIST).is_empty());
        assert_eq!(pending_guid_count(&sim, 0, COLONIST), 2);
        // Unpaid running order likewise.
        run(
            &mut sim,
            0,
            CommandKind::CancelProduction {
                building: hq,
                index: 0,
                arg: 0,
            },
        );
        assert_eq!(order(&sim, hq), (COLONIST, 0, 0, 0));
        // The cancel refunds the whole 60 food (remainder 100 %); empty it again.
        sim.state.players[0].resources_fx[FOOD] = 0;
        let account_before = sim.state.players[0].resources_fx;
        run(&mut sim, 0, complete(hq, 0, COLONIST));
        assert_eq!(order(&sim, hq), (COLONIST, 0, 0, 0));
        assert_eq!(sim.state.players[0].resources_fx, account_before);
        assert_eq!(messages(&sim).len(), 3);
        assert!(objects_of(&sim, COLONIST).is_empty());
    }

    #[test]
    fn trades_complete_in_command_order_and_the_second_sale_lacks_wood() {
        // Sale 492 costs `Convert(wood price, −100)` wood (price 1024: 400), paid only at the end
        // (`PropertyBuilding_PayTrade` only from `TickProduction`). With 500 wood it is enough for
        // one sale; the second click in the same tick is rejected.
        let rules = rules();
        let mut state = state(&rules);
        let market = {
            let mut sim = Sim::new(&mut state, &rules);
            let market = spawn(&mut sim, MARKET, 0, (20, 20));
            account(&mut sim, 0, [0, 500, 0, 0, 5]);
            for _ in 0..2 {
                run(
                    &mut sim,
                    0,
                    CommandKind::EnqueueProduction {
                        building: market,
                        guid: 492,
                    },
                );
            }
            market
        };
        let cmd = |kind| Command { player: 0, kind };
        let events = crate::tick::step(
            &mut state,
            &rules,
            &[cmd(complete(market, 0, 492)), cmd(complete(market, 0, 492))],
        );
        let sim = Sim::new(&mut state, &rules);
        assert_eq!((res(&sim, GOLD), res(&sim, WOOD)), (100, 100));
        assert_eq!(sim.state.economy.wood_price, 1024 - 400 / 25);
        // The second one now runs (started in the building tick of the step: 8 s, nothing paid).
        assert_eq!(order(&sim, market), (492, 0, 8000, 8000));
        let tile = sim.object(market).unwrap().tile;
        let shown: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                SimEvent::PlayerMessage {
                    to: Some(player),
                    id,
                    tile,
                    flags,
                } => Some((*player, *id, *tile, *flags)),
                _ => None,
            })
            .collect();
        // `PropertyBuilding_PayTrade` `0x00505170`: flag `0x01010000` (priority 1).
        assert_eq!(shown, [(0, message::NO_WOOD, tile, 0x0101_0000)]);
        // Trade never counts "in progress" back (quirk like at the normal end).
        assert_eq!(pending_guid_count(&sim, 0, 492), 2);
    }

    #[test]
    fn completing_is_refused_for_foreign_dead_or_missing_orders() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        let idle = spawn(&mut sim, HQ, 0, (40, 20));
        let w = spawn(&mut sim, COLONIST, 0, (30, 30));
        account(&mut sim, 0, [0, 0, 1000, 0, 50]);
        account(&mut sim, 1, [0, 0, 1000, 0, 50]);
        train(&mut sim, hq, 2);
        let before = (order(&sim, hq), sim.state.players.clone());
        let units = objects_of(&sim, COLONIST).len();
        for (player, b, index) in [
            (1, hq, 0),
            (1, hq, 1),
            (0, hq, 2),
            (0, hq, -1),
            (0, hq, i32::MAX),
            (0, idle, 0),
            (0, w, 0),
            (0, ObjectId::NONE, 0),
        ] {
            run(&mut sim, player, complete(b, index, COLONIST));
            let now = (order(&sim, hq), sim.state.players.clone());
            assert!(now == before, "{player} {b:?} {index}");
        }
        assert_eq!(objects_of(&sim, COLONIST).len(), units);
        // Another type at the point (the queue moved up): rejected.
        building_mut(&mut sim, hq)
            .unwrap()
            .make_queue
            .push_back(MARKET);
        let before = (order(&sim, hq), sim.state.players.clone());
        for (index, guid) in [(0, MARKET), (1, MARKET), (3, COLONIST), (2, 0)] {
            run(&mut sim, 0, complete(hq, index, guid));
            let now = (order(&sim, hq), sim.state.players.clone());
            assert!(now == before, "{index} {guid}");
        }
        assert_eq!(
            validate::check_debug_order(&sim, 0, hq, 1, MARKET),
            Err(format!("order 1 in {} is {COLONIST}, not {MARKET}", hq.0))
        );
        assert_eq!(validate::check_debug_order(&sim, 0, hq, 2, MARKET), Ok(()));
        assert_eq!(
            validate::check_debug_order(&sim, 0, hq, 0, COLONIST),
            Ok(())
        );
        building_mut(&mut sim, hq).unwrap().make_queue.pop_back();
        assert_eq!(
            validate::check_order(&sim, 1, hq, 0),
            Err(format!("{} is not an own building", hq.0))
        );
        crate::module::building::destroy::destroy(&mut sim, hq);
        run(&mut sim, 0, complete(hq, 0, COLONIST));
        assert_eq!(objects_of(&sim, COLONIST).len(), units);
        assert_eq!(
            validate::check_order(&sim, 0, hq, 0),
            Err(format!("{} is dead", hq.0))
        );
    }

    /// Command acknowledgments of the step (`SimEvent::UnitResponse`) in order.
    fn responses(sim: &Sim) -> Vec<(ObjectId, i32)> {
        sim.state
            .events
            .iter()
            .filter_map(|e| match *e {
                SimEvent::UnitResponse { unit, action, .. } => Some((unit, action)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn direct_commands_acknowledge_each_own_unit_and_queued_ones_stay_silent() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w: Vec<_> = (0..3)
            .map(|i| spawn(&mut sim, COLONIST, 0, (10 + 2 * i, 10)))
            .collect();
        let foreign = spawn(&mut sim, COLONIST, 1, (10, 14));
        let mine = spawn(&mut sim, MINE, 0, (30, 30));
        let mine_gold = |units: Vec<ObjectId>| CommandKind::MineGold { mine, units };
        // Direct: per own unit in command order action 12, the foreign one not (A8).
        run(&mut sim, 0, mine_gold(vec![w[2], foreign, w[0], w[1]]));
        assert_eq!(responses(&sim), [(w[2], 12), (w[0], 12), (w[1], 12)]);
        // The event carries owner and type at the time of the call.
        assert!(sim.state.events.iter().all(|e| match *e {
            SimEvent::UnitResponse { owner, guid, .. } => (owner, guid) == (0, COLONIST),
            _ => true,
        }));
        // The same wrapper queued: silent when queueing and when executing (`+0x78 = 0`).
        sim.state.events.clear();
        run(&mut sim, 0, queued(mine_gold(w.clone())));
        assert!(responses(&sim).is_empty());
        for &u in &w {
            assert!(process_queue(&mut sim, u));
        }
        assert!(responses(&sim).is_empty());
        // Place building: per worker; the wrapper only queues the workers (silent).
        account(&mut sim, 0, [1000, 1000, 1000, 0, 10]);
        sim.state.events.clear();
        run(&mut sim, 0, place(HOUSE, (40, 10), &[w[0], w[1]]));
        assert_eq!(responses(&sim), [(w[0], 12), (w[1], 12)]);
        sim.state.events.clear();
        run(&mut sim, 0, queued(place(HOUSE, (40, 20), &[w[0]])));
        assert!(process_queue(&mut sim, w[0]));
        assert!(responses(&sim).is_empty());
    }

    // QUIRK W-137: kept
    #[test]
    fn chopping_wood_acknowledges_even_from_the_queue_but_the_next_tree_does_not() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let tree = spawn(&mut sim, TREE, 9, (14, 12));
        spawn(&mut sim, TREE, 9, (20, 12));
        let chop = CommandKind::ChopWood {
            tree,
            units: vec![w],
        };
        run(&mut sim, 0, queued(chop));
        assert!(responses(&sim).is_empty());
        // `GameWorld_CmdChopWood` acknowledges without a `+0x78` check.
        assert!(process_queue(&mut sim, w));
        assert_eq!(responses(&sim), [(w, 12)]);
        // The cyclic "next tree" runs from the queue: silent.
        sim.state.events.clear();
        crate::world::destroy_object(&mut sim, tree);
        assert!(process_queue(&mut sim, w));
        assert!(responses(&sim).is_empty());
        // Given directly it acknowledges.
        let next = CommandKind::NextTree {
            tile_x: 14,
            tile_z: 12,
            unit: w,
        };
        run(&mut sim, 0, next);
        assert_eq!(responses(&sim), [(w, 12)]);
    }

    #[test]
    fn moving_acknowledges_once_for_the_first_selected_unit() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w: Vec<_> = (0..3)
            .map(|i| spawn(&mut sim, COLONIST, 0, (10 + 2 * i, 10)))
            .collect();
        let walk = |units: Vec<ObjectId>| CommandKind::Move {
            units,
            x: 30 << 12,
            z: 30 << 12,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: crate::command::NO_SPREAD,
            flags: 0,
        };
        run(&mut sim, 0, walk(vec![w[1], w[0], w[2]]));
        assert_eq!(responses(&sim), [(w[1], 12)]);
        // Queued: per unit a walk from the queue, silent.
        sim.state.events.clear();
        run(&mut sim, 0, queued(walk(w.clone())));
        for &u in &w {
            assert!(process_queue(&mut sim, u));
        }
        assert!(responses(&sim).is_empty());
    }

    // QUIRK W-104: deviation
    #[test]
    fn building_sites_must_be_seen_by_the_sender_and_wall_parts_explored() {
        let rules = rules();
        let mut state = fogged_state(&rules);
        let w = {
            let mut sim = Sim::new(&mut state, &rules);
            account(&mut sim, 0, [1000, 1000, 1000, 0, 50]);
            spawn(&mut sim, COLONIST, 0, (20, 20))
        };
        // Before the first copy nothing is seen.
        run(
            &mut Sim::new(&mut state, &rules),
            0,
            place(HOUSE, (24, 20), &[w]),
        );
        assert!(objects_of(&Sim::new(&mut state, &rules), HOUSE).is_empty());
        for _ in 0..2 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        let mut sim = Sim::new(&mut state, &rules);
        let wall = |z: i32| CommandKind::PlaceBuildings {
            parts: vec![BuildingPart {
                guid: WALL,
                rotation: F32Bits::new(0.0),
                x: F32Bits::new(world(20)),
                y: F32Bits::new(0.0),
                z: F32Bits::new(world(z)),
            }],
            workers: vec![w],
        };
        // Sight 32 at (20, 20): seen up to z = 28, explored up to z = 30.
        run(&mut sim, 0, place(HOUSE, (20, 30), &[w]));
        assert!(objects_of(&sim, HOUSE).is_empty(), "explored, but not seen");
        // Wall segments: the tile itself seen or explored, without rectangle (`0x00450870`).
        run(&mut sim, 0, wall(32));
        assert!(
            objects_of(&sim, WALL).is_empty(),
            "tile z 32 unexplored, only the rectangle +/-2 reaches into the explored area"
        );
        run(&mut sim, 0, wall(30));
        assert_eq!(
            objects_of(&sim, WALL).len(),
            1,
            "tile z 30 explored, not seen"
        );
        run(&mut sim, 0, place(HOUSE, (24, 20), &[w]));
        assert_eq!(objects_of(&sim, HOUSE).len(), 1);
        assert_eq!(res(&sim, WOOD), 1000 - 150 - 5);
    }

    /// Sight of slot 1 on the tile of the object (`MM_CHECKFOG` reads "seen").
    fn see(sim: &mut Sim, id: ObjectId) {
        let t = sim.object(id).unwrap().tile;
        sim.state.fog.seen.or(t.0, t.1, 1 << 1);
    }

    // QUIRK W-198: kept
    #[test]
    fn vehicle_quarter_commands_need_an_own_visible_ship() {
        // A8 / re/ships 2.4, 3.1: A vehicle accepts `0x13` if it is own, living and visible in the fog of
        // the sender - without `FREEFORQUARTER`, without "finished", also the Mayflower
        // (capacity 0, W-198). Foreign or hidden: discarded (neither meeting point nor paths).
        use crate::module::boarding::testkit as bk;
        let rules = bk::rules();
        let mut state = bk::shore_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let k = bk::spawn(&mut sim, bk::COLONIST, 1, (5, 10));
        let own = bk::spawn(&mut sim, bk::TRANSPORT, 1, (14, 10));
        let mayflower = bk::spawn(&mut sim, bk::MAYFLOWER, 1, (14, 14));
        let foreign = bk::spawn(&mut sim, bk::TRANSPORT, 2, (14, 18));
        let hidden = bk::spawn(&mut sim, bk::TRANSPORT, 1, (14, 22));
        // Own objects see themselves; the test sets the sight explicitly.
        sim.state.fog.seen.clear();
        sim.state.fog.cleared.clear();
        for s in [own, mayflower, foreign] {
            see(&mut sim, s);
        }
        let free = quarter::flags::FREE_FOR_QUARTER;
        let has_free = |sim: &Sim, s| sim.module(s, quarter::INDEX).unwrap().base.has_flags(free);
        assert!(
            !has_free(&sim, mayflower),
            "the Mayflower has no free space"
        );
        let quarter_into = |sim: &mut Sim, c| {
            sim.object_mut(k).unwrap().fifo.clear();
            sim.object_mut(c).unwrap().fifo.clear();
            run(
                sim,
                1,
                CommandKind::Quarter {
                    container: c,
                    units: vec![k],
                },
            );
            (codes(sim, k), codes(sim, c))
        };
        for c in [foreign, hidden] {
            assert_eq!(
                quarter_into(&mut sim, c),
                (vec![], vec![]),
                "{c:?} discarded"
            );
        }
        // Own visible ship: meeting point (0x1E17, with abort), sail order and walk of the unit.
        let (unit, ship) = quarter_into(&mut sim, own);
        assert_eq!((unit, ship), (vec![0x200], vec![0x1E17, 0x200]));
        let p = posted(&sim, k);
        let [Posted::Command { env, abort: true }] = &p[..] else {
            panic!("{p:?}");
        };
        assert_eq!(env.success, Some(Message::new(0x1617, &[k.as_i32()])));
        assert_eq!((env.success_target, &env.fault), (own, &None));
        // The Mayflower (full) accepts the command as well.
        assert_eq!(
            quarter_into(&mut sim, mayflower),
            (vec![0x200], vec![0x1E17, 0x200])
        );
        // A dead container accepts nothing anymore (`DIED`).
        sim.object_mut(own).unwrap().status |= status::DIED;
        assert_eq!(quarter_into(&mut sim, own), (vec![], vec![]));
    }

    #[test]
    fn unload_commands_accept_own_ships() {
        // `0x14`/`0x15` act on own mobile containers (re/ships 3.5); foreign senders do not.
        use crate::module::boarding::testkit as bk;
        let rules = bk::rules();
        let mut state = bk::shore_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let k1 = bk::spawn(&mut sim, bk::COLONIST, 1, (5, 10));
        let k2 = bk::spawn(&mut sim, bk::COLONIST, 1, (5, 12));
        let ship = bk::spawn(&mut sim, bk::TRANSPORT, 1, (11, 14));
        for u in [k1, k2] {
            assert_eq!(quarter::accept_unit(&mut sim, ship, u), 0);
        }
        let inside = |sim: &Sim, u| sim.object(u).unwrap().status & status::QUARTERED != 0;
        run(&mut sim, 2, CommandKind::UnquarterAll { container: ship });
        run(
            &mut sim,
            2,
            CommandKind::Unquarter {
                container: ship,
                unit: k1,
            },
        );
        assert!(inside(&sim, k1) && inside(&sim, k2), "foreign sender");
        run(
            &mut sim,
            1,
            CommandKind::Unquarter {
                container: ship,
                unit: k1,
            },
        );
        assert!(!inside(&sim, k1) && inside(&sim, k2));
        run(&mut sim, 1, CommandKind::UnquarterAll { container: ship });
        assert!(!inside(&sim, k2));
    }
}
