//! Right-click assignment (re/garrison-and-commands): from selection and target the command that
//! the UI of the original would produce - as pure logic, so that the viewer and the control channel
//! use the same assignment.
//!
//! 1. **Active list** (`GameWorld_RebuildTargetActions` `0x0047D6D0`): every entry of
//!    `targets.ini` in file order for which an object of the selection satisfies the MY filter
//!    (player -1 = owner; in the shipped data all entries are active).
//! 2. **Target** (`TargetActions_FindForTargetObject` `0x00480600`): the first active entry
//!    (`BUTTONMSG` 427 skipped) whose TARGET filter the object satisfies with the slot of the
//!    clicking player. If none matches, the **ground** (`TargetActions_FindGroundAction`
//!    `0x0047B040`): the first active entry with TARGET mode 0 and mode 2 in {0, `0x80`} (447
//!    rally point before 448 "Go to"). The ranks of `Input_PickTargetAction` (`0x00480E80`, ray test
//!    over the render list) are a matter for the UI: here it supplies the object under the mouse.
//! 3. **Command** (`Input_IssueTargetAction` `0x0044D350`): to the **whole** selection, not only to
//!    the objects that satisfy the MY filter (the commands check the sender themselves, A8).
//!
//! **Fog:** Almost all TARGET filters check `MM_CHECKFOG` (400, 401, 402, 398, 444, 445, 446,
//! 412, 413, 414 ...) for the clicking slot ([`crate::filter::matches_target`], `f[6]` only at the
//! head filter). Quirk: MY filters and follow-up filters check the fog of player 0. A target in the fog
//! thus has no object action, the right-click falls through to the ground. Combat parts (weapon range,
//! attacks, `MM_METHODINDEX 3`) check the filters like any other part. Buildings, trees and resources in the
//! fog are shown by the host as a fog memory; the right-click on them is assigned by
//! [`memory_target_action`] via the snapshot.

use crate::command::CommandKind;
use crate::filter::{matches, matches_target};
use crate::fog::{FogView, MemorySnapshot, memory_matches};
use crate::fx::ftol;
use crate::id::ObjectId;
use crate::motion::NO_DIRECTION;
use crate::sim::Sim;

/// `BUTTONMSG` of the right-click actions that are an economy or movement command.
pub mod button {
    pub const FISH: i32 = 398;
    pub const CHOP_WOOD: i32 = 400;
    pub const HARVEST: i32 = 401;
    pub const MINE_GOLD: i32 = 402;
    pub const BUILD: i32 = 412;
    pub const REPAIR: i32 = 413;
    pub const QUARTER: i32 = 414;
    /// Mounting a horse (rider with module 13, `0x0A`).
    pub const MOUNT_HORSE: i32 = 403;
    /// Boarding a boat (occupant with module 14, `0x0C`).
    pub const BOARD_BOAT: i32 = 404;
    /// Garrison attack (own building with a garrison against an enemy target).
    pub const GARRISON_ATTACK: i32 = 410;
    /// Attacking (unit with an attack module against an enemy unit).
    pub const ATTACK_UNIT: i32 = 411;
    /// Attack in attack mode; skipped for objects (right-click), takes effect via
    /// [`super::attack_actions`] (key X, F7).
    pub const ATTACK: i32 = 427;
    /// Gutting (hunter -> carcass with `ISROT`, `0x16`).
    pub const GUT: i32 = 418;
    /// Hunting (hunter -> animal without `ISROT`, `0x17`).
    pub const HUNT: i32 = 419;
    /// Following (own unit -> own unit, `0x74`).
    pub const FOLLOW: i32 = 428;
    pub const DELIVER_WOOD: i32 = 444;
    pub const DELIVER_GOLD: i32 = 445;
    pub const DELIVER_FOOD: i32 = 446;
    pub const RALLY_POINT: i32 = 447;
    pub const GO_TO: i32 = 448;
    /// Further repair action that `Input_IssueTargetAction` maps to `0x02` (not in
    /// the `targets.ini` of the shipped data).
    pub const REPAIR_2: i32 = 569;
}

/// Target of a command: object or ground point in fixed-point tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Object(ObjectId),
    Ground(i32, i32),
}

/// `GameWorld_RebuildTargetActions`: indices of the active entries of `targets.ini` with the
/// first object of the selection that satisfies the MY filter.
pub fn active_actions(sim: &mut Sim, selection: &[ObjectId]) -> Vec<(usize, ObjectId)> {
    let rules = sim.rules;
    let mut out = Vec::new();
    for (i, e) in rules.targets.iter().enumerate() {
        if !e.enabled {
            continue;
        }
        let first = selection
            .iter()
            .copied()
            .find(|&o| sim.object(o).is_some() && matches(sim, o, &e.my, -1));
        if let Some(o) = first {
            out.push((i, o));
        }
    }
    out
}

/// `TargetActions_FindForTargetObject` (`0x00480600`): index of the first active entry (without
/// `BUTTONMSG` 427) whose TARGET filter the object `target` satisfies with the slot `player`.
fn object_action(
    sim: &mut Sim,
    player: u8,
    active: &[(usize, ObjectId)],
    target: ObjectId,
) -> Option<usize> {
    let rules = sim.rules;
    sim.object(target)?;
    active.iter().map(|&(i, _)| i).find(|&i| {
        let e = &rules.targets[i];
        e.button_msg != button::ATTACK
            && matches_target(sim, target, &e.target, i32::from(player), i32::from(player))
    })
}

/// Does the object `target` have an object action for the selection (`0x00480600` returns true)? With it
/// `Input_PickTargetAction` (`0x00480E80`) lets a rank fall through whose last accepted hit has
/// none. An action for which no command is built also counts.
pub fn has_object_action(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    target: ObjectId,
) -> bool {
    let active = active_actions(sim, selection);
    object_action(sim, player, &active, target).is_some()
}

/// Right-click of the slot `player` with the selection `selection` on the object `target` (under the
/// mouse, if there is one) or the ground point `ground` (fixed-point tiles `x, z`): `BUTTONMSG` of the
/// chosen entry and the command. `None` if no entry matches or no command is built for the
/// matching one.
pub fn pick_target_action(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    target: Option<ObjectId>,
    ground: (i32, i32),
) -> Option<(i32, CommandKind)> {
    let (i, object) = pick_row(sim, player, selection, target)?;
    let b = sim.rules.targets[i].button_msg;
    let aim = match object {
        Some(t) => Target::Object(t),
        None => Target::Ground(ground.0, ground.1),
    };
    let cmd = command_for(b, selection, aim)?;
    Some((b, cmd))
}

/// Row of the right-click action as `Input_PickTargetAction` (`0x00480E80`) chooses it for the object `target`
/// or the ground: index in `rules.targets` of the first matching row of the **active** list (O-281), first
/// the object action (`0x00480600`), otherwise the ground action (`TargetActions_FindGroundAction`
/// `0x0047B040`). Pure query without a command: the pointer follows the found row (`MAUSICONID`,
/// `0x00453790`), even if no command is built for the row. [`pick_target_action`] builds the command
/// on the same choice. There is no ownership check of the selection here (the original does not check it until
/// sending, `0x00457C50`).
pub fn pick_target_entry(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    target: Option<ObjectId>,
) -> Option<usize> {
    pick_row(sim, player, selection, target).map(|(i, _)| i)
}

/// The row choice of [`pick_target_entry`]: index and, for an object action, the object (otherwise ground).
fn pick_row(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    target: Option<ObjectId>,
) -> Option<(usize, Option<ObjectId>)> {
    let rules = sim.rules;
    let active = active_actions(sim, selection);
    if let Some(t) = target
        && let Some(i) = object_action(sim, player, &active, t)
    {
        return Some((i, Some(t)));
    }
    let (i, _) = active.iter().copied().find(|&(i, _)| {
        let f = &rules.targets[i].target;
        f.mode == 0 && (f.mode2 == 0 || f.mode2 == 0x80)
    })?;
    Some((i, None))
}

/// `TargetActions_FindForFogMemory` (`0x0047D770`): index of the first active entry without
/// `BUTTONMSG` 427 whose TARGET filter the snapshot satisfies (`f[6]` = slot at the head filter). Pure
/// query without a command like [`pick_target_entry`] (pointer over a fog memory).
pub fn memory_target_entry(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    snap: &MemorySnapshot,
) -> Option<usize> {
    let rules = sim.rules;
    let active = active_actions(sim, selection);
    let view = FogView::new(&*sim.state);
    let p = i32::from(player);
    active.iter().map(|&(i, _)| i).find(|&i| {
        let e = &rules.targets[i];
        e.button_msg != button::ATTACK && memory_matches(snap, &view, &e.target, p, p)
    })
}

/// Does the fog memory `snap` have an action for the selection (`0x0047D770` returns true as soon as
/// a filter matches)? With it `Input_PickTargetAction` (`0x00480E80`) lets the rank of the memories
/// fall through. As with [`has_object_action`], an action for which no command is built also
/// counts.
pub fn has_memory_action(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    snap: &MemorySnapshot,
) -> bool {
    memory_target_entry(sim, player, selection, snap).is_some()
}

/// Right-click on an active fog memory (`0x0047ACF0`, `TargetActions_FindForFogMemory`
/// `0x0047D770`): the entry from [`has_memory_action`]; the command goes to the real ID.
/// If the object no longer exists, the command rejects itself (A8). `None` without a matching entry
/// or if no command is built for the matching one.
pub fn memory_target_action(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    snap: &MemorySnapshot,
) -> Option<(i32, CommandKind)> {
    let i = memory_target_entry(sim, player, selection, snap)?;
    let b = sim.rules.targets[i].button_msg;
    let mut cmd = command_for(b, selection, Target::Object(snap.id))?;
    // Attacking a fog memory (`0x71`): the position of the memory goes along, so that the unit
    // walks there if the target is gone at execution. `Fixed_FromFloat(world position)` is
    // the fixed-point grid position * 128, divided by 128 in the attack module.
    if let CommandKind::Attack { pos, .. } = &mut cmd {
        *pos = Some(snap.world_pos.map(|w| ftol(w * 4096.0)));
    }
    Some((b, cmd))
}

/// Targeting mode of a command bar button (TARGET): the entry with this `BUTTONMSG`
/// (`GameWorld_FindTargetEntryByButtonMsg` `0x0047B420`) and the target like the check of the target cursor
/// `FUN_004515B0`:
/// - TARGET mode 0 -> always the ground `ground` (also over an object);
/// - otherwise the object `target` if it satisfies the TARGET filter with the slot `player` (`FUN_0047ACF0`);
/// - otherwise the ground if mode 2 carries the bit `0x1000`; otherwise no target.
///
/// The target cursor does not check the MY filter (the button was already in the bar); the command goes to the
/// whole selection like `Input_IssueTargetAction` ([`command_for`]). `None` without an entry, without a
/// matching target or if no command is built for the action. Only reads.
pub fn button_target_action(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    button_msg: i32,
    target: Option<ObjectId>,
    ground: (i32, i32),
) -> Option<CommandKind> {
    let aim = button_target_aim(sim, player, button_msg, target, ground)?;
    command_for(button_msg, selection, aim)
}

/// Validity of the target cursor of a command bar button (`FUN_004515B0`): there is an entry with this
/// `BUTTONMSG` and a target by the rule of [`button_target_action`] (mode 0 -> ground; object with a satisfied
/// TARGET filter; otherwise ground for mode 2 `& 0x1000`). Whether a command results from it, the cursor does
/// not check. Only reads.
pub fn button_target_valid(
    sim: &mut Sim,
    player: u8,
    button_msg: i32,
    target: Option<ObjectId>,
) -> bool {
    button_target_aim(sim, player, button_msg, target, (0, 0)).is_some()
}

/// Target of a target button according to `FUN_004515B0` (see [`button_target_action`]).
fn button_target_aim(
    sim: &mut Sim,
    player: u8,
    button_msg: i32,
    target: Option<ObjectId>,
    ground: (i32, i32),
) -> Option<Target> {
    let rules = sim.rules;
    let entry = rules.target_entry(button_msg)?;
    let f = &entry.target;
    let p = i32::from(player);
    Some(if f.mode == 0 {
        Target::Ground(ground.0, ground.1)
    } else if let Some(t) =
        target.filter(|&t| sim.object(t).is_some() && matches_target(sim, t, f, p, p))
    {
        Target::Object(t)
    } else if f.mode2 & GROUND_TARGET != 0 {
        Target::Ground(ground.0, ground.1)
    } else {
        return None;
    })
}

/// Mode 2 bit of the TARGET filter: the targeting mode substitutes the ground (`FUN_004515B0`, `+0x5C & 0x1000`).
const GROUND_TARGET: u32 = 0x1000;

/// `Input_IssueTargetAction`: action -> command to the whole selection. Rally point on the ground `{x, z}`,
/// on an object `{−1, target}`; "Go to" like the UI without target radius, direction and flags.
fn command_for(button_msg: i32, selection: &[ObjectId], target: Target) -> Option<CommandKind> {
    use Target::{Ground, Object};
    let units = selection.to_vec();
    Some(match (button_msg, target) {
        (button::CHOP_WOOD, Object(tree)) => CommandKind::ChopWood { tree, units },
        (button::HARVEST, Object(farm)) => CommandKind::Harvest { farm, units },
        (button::MINE_GOLD, Object(mine)) => CommandKind::MineGold { mine, units },
        (button::FISH, Object(basket)) => CommandKind::Fish { basket, units },
        (button::DELIVER_WOOD | button::DELIVER_GOLD | button::DELIVER_FOOD, Object(building)) => {
            CommandKind::Deliver { building, units }
        }
        (button::BUILD | button::REPAIR | button::REPAIR_2, Object(building)) => {
            CommandKind::ConstructAt { building, units }
        }
        (button::QUARTER, Object(container)) => CommandKind::Quarter { container, units },
        (button::MOUNT_HORSE, Object(horse)) => CommandKind::Mount { horse, units },
        (button::BOARD_BOAT, Object(boat)) => CommandKind::BoardBoat { boat, units },
        (button::RALLY_POINT, Ground(x, z)) => CommandKind::SetRallyPoint {
            x,
            z,
            buildings: units,
        },
        (button::RALLY_POINT, Object(o)) => CommandKind::SetRallyPoint {
            x: -1,
            z: o.as_i32(),
            buildings: units,
        },
        (button::ATTACK_UNIT, Object(target)) => CommandKind::Attack {
            units,
            target,
            pos: None,
            mods: 0,
        },
        (button::GUT | button::HUNT, Object(animal)) => CommandKind::Hunt {
            animal,
            units,
            gut: button_msg == button::GUT,
        },
        (button::FOLLOW, Object(target)) => CommandKind::Follow { target, units },
        (button::GARRISON_ATTACK, Object(target)) => CommandKind::GarrisonAttack {
            buildings: units,
            target,
        },
        (button::GO_TO, Ground(x, z)) => CommandKind::Move {
            units,
            x,
            z,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: crate::command::NO_SPREAD,
            flags: 0,
        },
        _ => return None,
    })
}

/// Attack mode (key X, F7; right-click action 427): if the object satisfies entry 427, attack
/// with modifier 4 (`0x11`); otherwise on the ground `0x12` mode 3 (offensive stays put) and movement
/// with flag 4 (walk `0x400`, attack march; re/commands 1.7, re/combat-attack 7.4). The
/// unit does not remember the march target (C-13).
pub fn attack_actions(
    sim: &mut Sim,
    player: u8,
    selection: &[ObjectId],
    target: Option<ObjectId>,
    ground: (i32, i32),
) -> Vec<CommandKind> {
    let units = selection.to_vec();
    if let Some(t) = target {
        let rules = sim.rules;
        let p = i32::from(player);
        for (i, _) in active_actions(sim, selection) {
            let e = &rules.targets[i];
            if e.button_msg == button::ATTACK && matches_target(sim, t, &e.target, p, p) {
                return vec![CommandKind::Attack {
                    units,
                    target: t,
                    pos: None,
                    mods: 4,
                }];
            }
        }
    }
    vec![
        CommandKind::SetBehavior {
            units: units.clone(),
            mode: 3,
        },
        CommandKind::Move {
            units,
            x: ground.0,
            z: ground.1,
            target: -1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: crate::command::NO_SPREAD,
            flags: 4,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit::*;
    use crate::module::gather::{load, set_carried_load};
    use crate::module::quarter;
    use crate::state::SimState;

    /// The pure row choice (`pick_target_entry`) is the same as that of the command, but also yields
    /// a row without a command and does not check the ownership of the selection (`0x00453790` -> `0x00480E80`).
    #[test]
    fn pick_target_entry_is_the_row_of_the_right_click() {
        let rules = rules();
        let mut state: SimState = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let tree = spawn(&mut sim, TREE, 9, (5, 5));
        let store = spawn(&mut sim, STORE, 0, (50, 10));
        let foreign = spawn(&mut sim, COLONIST, 1, (12, 10));
        let ground = (7 << 12, 9 << 12);
        let row = |sim: &mut Sim, sel: &[ObjectId], t: Option<ObjectId>| {
            pick_target_entry(sim, 0, sel, t).map(|i| sim.rules.targets[i].button_msg)
        };
        for (sel, t) in [(&[w][..], Some(tree)), (&[w], Some(store)), (&[w], None)] {
            let action = pick_target_action(&mut sim, 0, sel, t, ground).map(|a| a.0);
            assert_eq!(row(&mut sim, sel, t), action, "{t:?}");
        }
        assert_eq!(row(&mut sim, &[w], Some(tree)), Some(400));
        assert_eq!(row(&mut sim, &[w], None), Some(448));
        // Carrying wood over the own warehouse: the first matching row 444, not 413.
        set_carried_load(&mut sim, w, 20, load::WOOD);
        assert_eq!(row(&mut sim, &[w], Some(store)), Some(444));
        // Without a selection no active row; a foreign selection has its rows (MY filter with the owner).
        assert_eq!(row(&mut sim, &[], Some(tree)), None);
        assert_eq!(row(&mut sim, &[foreign], Some(tree)), Some(400));
    }

    #[test]
    fn right_clicks_follow_the_target_table_of_a_worker() {
        let rules = rules();
        let mut state: SimState = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let tree = spawn(&mut sim, TREE, 9, (5, 5));
        let mine = spawn(&mut sim, MINE, 0, (20, 10));
        let farm = spawn(&mut sim, FARM, 0, (30, 10));
        let store_site = site(&mut sim, STORE, 0, (40, 10));
        let store = spawn(&mut sim, STORE, 0, (50, 10));
        let hq = spawn(&mut sim, HQ, 0, (20, 30));
        let enemy_store = spawn(&mut sim, STORE, 1, (40, 30));
        let ground = (7 << 12, 9 << 12);
        let pick = |sim: &mut Sim, t: Option<ObjectId>| {
            pick_target_action(sim, 0, &[w], t, ground).map(|(b, c)| (b, c.type_code() & 0xFF))
        };
        // Tree -> 400 / 0x06; own gold mine -> 402; farm with free space -> 401.
        assert_eq!(pick(&mut sim, Some(tree)), Some((400, 0x06)));
        assert_eq!(pick(&mut sim, Some(mine)), Some((402, 0x03)));
        assert_eq!(pick(&mut sim, Some(farm)), Some((401, 0x07)));
        // Own construction site -> 412 / 0x02 (also a warehouse: construction sites have no acceptance bit).
        assert_eq!(pick(&mut sim, Some(store_site)), Some((412, 0x02)));
        // Warehouse without a load, undamaged -> 413 #21 ("Repair" without damage).
        assert_eq!(pick(&mut sim, Some(store)), Some((413, 0x02)));
        // HQ with free space -> 414 / 0x13.
        assert_eq!(pick(&mut sim, Some(hq)), Some((414, 0x13)));
        // Foreign warehouse: nothing matches -> ground "Go to".
        assert_eq!(pick(&mut sim, Some(enemy_store)), Some((448, 0x04)));
        assert_eq!(pick(&mut sim, None), Some((448, 0x04)));
        // Object action (`0x00480600`) for the fall-through of the ranks of `0x00480E80`.
        assert!(has_object_action(&mut sim, 0, &[w], tree));
        assert!(has_object_action(&mut sim, 0, &[w], hq));
        assert!(!has_object_action(&mut sim, 0, &[w], enemy_store));
        assert!(!has_object_action(&mut sim, 0, &[], tree));
        // Carrying wood: 444 before 413.
        set_carried_load(&mut sim, w, 20, load::WOOD);
        assert_eq!(pick(&mut sim, Some(store)), Some((444, 0x09)));
        assert_eq!(pick(&mut sim, Some(store_site)), Some((412, 0x02)));
        // The command goes to the whole selection, "Go to" with the ground point.
        let boat = spawn(&mut sim, BOAT, 0, (12, 12));
        let r = pick_target_action(&mut sim, 0, &[w, boat], Some(tree), ground);
        assert_eq!(
            r,
            Some((
                400,
                CommandKind::ChopWood {
                    tree,
                    units: vec![w, boat]
                }
            ))
        );
        let r = pick_target_action(&mut sim, 0, &[w], None, ground);
        assert_eq!(
            r,
            Some((
                448,
                CommandKind::Move {
                    units: vec![w],
                    x: 7 << 12,
                    z: 9 << 12,
                    target: -1,
                    formation: 0,
                    direction: NO_DIRECTION,
                    spread: crate::command::NO_SPREAD,
                    flags: 0
                }
            ))
        );
    }

    #[test]
    fn buildings_set_rally_points_on_the_ground_and_boats_fish() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = spawn(&mut sim, HQ, 0, (20, 20));
        let basket = spawn(&mut sim, BASKET, 10, (5, 5));
        let boat = spawn(&mut sim, BOAT, 0, (8, 8));
        let ground = (3 << 12, 4 << 12);
        // Building selected + ground -> 447 before 448 (448 requires the walk module).
        assert_eq!(
            pick_target_action(&mut sim, 0, &[hq], None, ground),
            Some((
                447,
                CommandKind::SetRallyPoint {
                    x: 3 << 12,
                    z: 4 << 12,
                    buildings: vec![hq]
                }
            ))
        );
        // An object never matches 447 (empty TARGET filter): the ground again.
        assert_eq!(
            pick_target_action(&mut sim, 0, &[hq], Some(basket), ground).map(|r| r.0),
            Some(447)
        );
        // Boat on the fisher basket -> 398 / 0x7D; colonist without fishing module -> ground.
        assert_eq!(
            pick_target_action(&mut sim, 0, &[boat], Some(basket), ground)
                .map(|(b, c)| (b, c.type_code() & 0xFF)),
            Some((398, 0x7D))
        );
        // Empty selection: no active action.
        assert_eq!(
            pick_target_action(&mut sim, 0, &[], Some(basket), ground),
            None
        );
    }

    #[test]
    fn right_clicks_into_the_fog_offer_no_object_action() {
        let rules = rules();
        let mut state = fogged_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let tree = spawn(&mut sim, TREE, 9, (40, 40));
        let ground = (40 << 12 | 0x800, 40 << 12 | 0x800);
        let (b, _) = pick_target_action(&mut sim, 0, &[w], Some(tree), ground).unwrap();
        assert_eq!(b, button::GO_TO, "tree in the fog: ground action");
        assert!(!has_object_action(&mut sim, 0, &[w], tree));
        crate::fog::reveal_map(&mut *sim.state, 0);
        let (b, _) = pick_target_action(&mut sim, 0, &[w], Some(tree), ground).unwrap();
        assert_eq!(b, button::CHOP_WOOD);
    }

    #[test]
    fn right_click_403_and_404_mount_and_board() {
        // re/riding 2.5: 403 applies to colonists (module 13) on every free saddle horse, also a
        // foreign one (target without player list); 404 for warriors (module 14) only on own free canoes.
        use crate::module::riding::testkit as rk;
        let rules = rk::rules();
        let mut state = rk::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let colonist = rk::spawn(&mut sim, rk::COLONIST, 0, (10, 10));
        let warrior = rk::spawn(&mut sim, rk::WARRIOR, 0, (12, 10));
        let own_horse = rk::spawn(&mut sim, rk::HORSE, 0, (14, 10));
        let foreign_horse = rk::spawn(&mut sim, rk::HORSE, 1, (16, 10));
        let own_canoe = rk::spawn(&mut sim, rk::CANOE, 0, (18, 10));
        let foreign_canoe = rk::spawn(&mut sim, rk::CANOE, 1, (20, 10));
        let ground = (7 << 12, 9 << 12);
        let pick = |sim: &mut Sim, sel: &[ObjectId], t: ObjectId| {
            pick_target_action(sim, 0, sel, Some(t), ground)
        };
        assert_eq!(
            pick(&mut sim, &[colonist], own_horse),
            Some((
                403,
                CommandKind::Mount {
                    horse: own_horse,
                    units: vec![colonist]
                }
            ))
        );
        // Foreign horse: also 403 (K-224).
        assert_eq!(
            pick(&mut sim, &[colonist], foreign_horse).map(|r| r.0),
            Some(403)
        );
        // The command goes to the whole selection.
        assert_eq!(
            pick(&mut sim, &[colonist, warrior], own_horse),
            Some((
                403,
                CommandKind::Mount {
                    horse: own_horse,
                    units: vec![colonist, warrior]
                }
            ))
        );
        assert!(has_object_action(&mut sim, 0, &[colonist], foreign_horse));
        // Warrior -> own canoe: 404; foreign canoe: no action; canoe for the colonist: none.
        assert_eq!(
            pick(&mut sim, &[warrior], own_canoe),
            Some((
                404,
                CommandKind::BoardBoat {
                    boat: own_canoe,
                    units: vec![warrior]
                }
            ))
        );
        assert_eq!(pick(&mut sim, &[warrior], foreign_canoe), None);
        assert!(!has_object_action(&mut sim, 0, &[warrior], foreign_canoe));
        assert_eq!(pick(&mut sim, &[colonist], own_canoe), None);
        // The warrior has no module 13: the horse offers him nothing.
        assert_eq!(pick(&mut sim, &[warrior], own_horse), None);
        // A horse with a rider (`CARRIING`) is not offered.
        let rider = rk::spawn(&mut sim, rk::DRAGOON, 0, (14, 12));
        rk::link(&mut sim, rider, own_horse);
        assert_eq!(pick(&mut sim, &[colonist], own_horse), None);
        // In the fog (map not revealed) there is no object action (`MM_CHECKFOG`).
        let mut fogged = crate::command::testkit::fogged_state(&rules);
        let mut sim = Sim::new(&mut fogged, &rules);
        let c = rk::spawn(&mut sim, rk::COLONIST, 0, (10, 10));
        let h = rk::spawn(&mut sim, rk::HORSE, 0, (40, 40));
        assert_eq!(pick(&mut sim, &[c], h), None);
    }

    #[test]
    fn right_clicks_on_fog_memories_use_the_snapshot_and_skip_attacks() {
        let rules = rules();
        let mut state = fogged_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let w = spawn(&mut sim, COLONIST, 0, (10, 10));
        let tree = spawn(&mut sim, TREE, 9, (40, 40));
        let snap = crate::fog::MemorySnapshot::take(sim.object(tree).unwrap(), (40, 40), None);
        let before = crate::save::checksum(&*sim.state);
        // The tree itself is in the fog; its fog memory offers chopping wood.
        let (b, cmd) = memory_target_action(&mut sim, 0, &[w], &snap).unwrap();
        assert_eq!((b, cmd.type_code() & 0xFF), (button::CHOP_WOOD, 0x06));
        assert!(has_memory_action(&mut sim, 0, &[w], &snap));
        assert!(
            !has_memory_action(&mut sim, 0, &[], &snap),
            "without a selection"
        );
        assert_eq!(crate::save::checksum(&*sim.state), before);
    }

    #[test]
    fn right_click_414_boards_own_vehicles() {
        // re/ships 2.4: the entry 414 "into vehicles" applies to own visible containers with quarters and
        // walk - also full ones or the Mayflower (capacity 0, W-198). Foreign transports, warships
        // without quarters and ships in the fog have no object action.
        use crate::module::boarding::testkit as bk;
        let rules = bk::rules();
        let mut state = bk::shore_state(&rules);
        crate::fog::reveal_map(&mut state, -1);
        let mut sim = Sim::new(&mut state, &rules);
        let k = bk::spawn(&mut sim, bk::COLONIST, 1, (5, 10));
        let ship = bk::spawn(&mut sim, bk::TRANSPORT, 1, (14, 10));
        let mayflower = bk::spawn(&mut sim, bk::MAYFLOWER, 1, (14, 14));
        let foreign = bk::spawn(&mut sim, bk::TRANSPORT, 2, (14, 18));
        let warship = bk::spawn(&mut sim, bk::WARSHIP, 1, (14, 22));
        let ground = (7 << 12, 9 << 12);
        let pick = |sim: &mut Sim, t| pick_target_action(sim, 1, &[k], Some(t), ground);
        let board = |c| {
            Some((
                button::QUARTER,
                CommandKind::Quarter {
                    container: c,
                    units: vec![k],
                },
            ))
        };
        assert_eq!(pick(&mut sim, ship), board(ship));
        assert_eq!(pick(&mut sim, mayflower), board(mayflower));
        // A full ship (no `FREEFORQUARTER`) remains a target.
        sim.module_mut(ship, quarter::INDEX).unwrap().base.flags &=
            !(quarter::flags::FREE_FOR_QUARTER & 0xFFFF_FF00);
        assert_eq!(pick(&mut sim, ship), board(ship));
        assert_eq!(pick(&mut sim, foreign), None);
        assert_eq!(pick(&mut sim, warship), None);
        assert!(has_object_action(&mut sim, 1, &[k], mayflower));
        assert!(!has_object_action(&mut sim, 1, &[k], warship));
        // In the fog: no object action.
        sim.state.fog.seen.clear();
        sim.state.fog.cleared.clear();
        assert_eq!(pick(&mut sim, ship), None);
        let t = sim.object(ship).unwrap().tile;
        sim.state.fog.seen.or(t.0, t.1, 1 << 1);
        assert_eq!(pick(&mut sim, ship), board(ship));
    }
}
