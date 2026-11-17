//! Check of the economy commands against the sender (exception A8). The original checks neither ownership nor
//! prerequisites in the commands; the UI (`Toolbar_IsButtonLocked`
//! `0x0044AF60`, `Toolbar_BuildButtonSet` `0x004557C0`) and the AI (`Player_CanBuildOrMakeGuid`
//! `0x004305E0`) do so beforehand (re/construction, re/garrison-and-commands). Here
//! the same rule is applied in the command so that a command effects only what a player could trigger through the
//! UI; the counters are simulation state, hence deterministic.
//!
//! **Build menus:** The build bar shows the menu of the selected worker (menu GUID = type GUID, e.g.
//! colonist 1100) with the submenus of its `TARGET_DO` buttons (110/111 "Erect
//! buildings"). A construction is allowed only if an own worker of the command has the type in its
//! menu or one of these submenus as a `BUILD`/`ONLYONCE` button - that also fits the
//! nation without the simulation having to know nations. Production menus belong to the building type
//! (menu GUID = building GUID, e.g. HQ 1000 -> colonist 1100 `MAKE`).
//!
//! **Lock from the level data:** `Player_IsGuidDisabled` (`0x0047DE80`) asks `Toolbar_BuildButtonSet`
//! before any kind check - a type or an upgrade with config 1 = 0 in the value set
//! of the slot is hidden ([`Rules::is_guid_disabled`], re/upgrades 2, 10.4). The second part of the original is the
//! **global** map `+0x21E0` (the same for all slots, script action 207/208, `SimState::button_locks`);
//! [`crate::world::is_guid_disabled`] combines both parts. The commands `0x01`/`0x18` and `0x0E` reject
//! such a GUID with "is locked on this map" (the original checks the lock only in the command bar,
//! the headhunter buttons and the AI query; deviation from the original).
//!
//! **Research:** A button of kind `UPGRADE` in the menu of the building can be queued like a training
//! (`PropertyBuilding_FinishResearch` applies it at the end,
//! [`crate::module::building::production`]). A `COUNTERUPGRADE` button executes a unit
//! (missionary, menu 601) and does not belong in the production of a building. Whether a GUID is an
//! upgrade is decided solely by the kind of the button: [`Rules::is_upgrade`] also contains the
//! types from `default.dat` and serves here only to let upgrades count as "known".

use nml_formats::gifcon::{ToolbarButton, button_kind};

use crate::economy::{
    can_afford_object, guid_count, guid_count_in, pending_guid_count, pending_guid_count_in,
};
use crate::id::ObjectId;
use crate::module::ModuleKind;
use crate::module::building::production::{PADDOCK_A, PADDOCK_B, is_trade_guid};
use crate::module::building::{self, flags};
use crate::object::{GameObject, status};
use crate::placement::building_flags;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::SimState;
use crate::upgrades::counter;

/// Kinds with prerequisite check (`Toolbar_IsButtonLocked` `0x0044AF60`, likewise step 1 of drawing
/// `0x0044F250`). `nml-ui` keeps the same list (`hud::toolbar::model::CHECKED_KINDS`); the equality is
/// checked by the test `toolbar_locks_match_the_sim_for_every_corpus_button` in `nml-client`. Whoever calls
/// [`missing_required_in`] checks the kind beforehand with this list.
pub const CHECKED_KINDS: [u32; 9] = [
    button_kind::BUILD,
    button_kind::UPGRADE,
    button_kind::MAKE,
    button_kind::COUNTERUPGRADE,
    button_kind::ONLYONCE,
    button_kind::MAKEONLY_ONCE,
    button_kind::TARGET,
    button_kind::TARGET_DO,
    button_kind::DO,
];
/// One-time kinds: only while the object counter and "in progress" are both 0.
const ONCE_KINDS: [u32; 3] = [
    button_kind::UPGRADE,
    button_kind::ONLYONCE,
    button_kind::MAKEONLY_ONCE,
];
/// Build kinds of the build menus (`BUILD_MSG`, with `ONLYONCE_MSG` ORed gives `ONLYONCE`).
const BUILD_KINDS: [u32; 2] = [button_kind::BUILD, button_kind::ONLYONCE];
/// Training and trade kinds of the production menus.
const MAKE_KINDS: [u32; 2] = [button_kind::MAKE, button_kind::MAKEONLY_ONCE];

/// Kinds whose click opens **no** submenu (click handler `FUN_00458130`): placement (`BUILD`,
/// `ONLYONCE`), special actions (`0x5000000`, `0x14000000`, `0x16000000`) and the commands of the
/// production bar (`FUN_0044CF50`: `MAKE`, `UPGRADE`, `DO`). Every other kind - also 0 and
/// `TARGET_DO` - shows the menu with the GUID of the button (`Toolbar_BuildButtonSet`).
const NO_SUBMENU_KINDS: [u32; 8] = [
    button_kind::BUILD,
    button_kind::ONLYONCE,
    0x500_0000,
    0x1400_0000,
    0x1600_0000,
    button_kind::MAKE,
    button_kind::UPGRADE,
    button_kind::DO,
];

/// All buttons of the menu `menu` in menu order, then those of the submenus that a click
/// on a button of the menu opens (one level, in menu order; every kind except
/// [`NO_SUBMENU_KINDS`] opens the menu with the GUID of the button). The female settler (2602) and the
/// female pioneer (2002) have the build buttons 260/261 and 190/191 without a kind and build anyway
/// (re/construction). Unknown menu: empty.
pub fn menu_buttons(rules: &Rules, menu: u32) -> Vec<&ToolbarButton> {
    let menus = &rules.toolbar.menus;
    let Some(buttons) = menus.get(&(menu as i32)) else {
        return Vec::new();
    };
    let subs = buttons
        .iter()
        .filter(|b| !NO_SUBMENU_KINDS.contains(&b.kind()))
        .filter_map(|b| menus.get(&b.msg))
        .flatten();
    buttons.iter().chain(subs).collect()
}

/// First button of the menu `menu` that satisfies `pred`; otherwise the first in the submenus
/// ([`menu_buttons`]).
pub fn menu_button(
    rules: &Rules,
    menu: u32,
    pred: impl Fn(&ToolbarButton) -> bool,
) -> Option<&ToolbarButton> {
    menu_buttons(rules, menu).into_iter().find(|b| pred(b))
}

/// `Toolbar_IsButtonLocked` (`0x0044AF60`) for the slot `slot`: locked if a
/// `REQUIRED_GUID` (up to the first 0) has no object counter (`Player_GetGuidCount`, with groups).
/// **Quirk:** `REQUIRED_GUID_NATION` always asks for GUID 0 via a stub and thus locks
/// (unused in the shipped data). A `COUNTERUPGRADE` button is additionally locked while its GUID
/// runs anywhere (`FUN_00412620`) or the slot has a running counter-upgrade
/// (`FUN_00479690(p) ≠ 0`; re/upgrades 2, [`counter::is_running`], [`counter::count_of_slot`]). The
/// source is checked by the command ([`crate::command::upgrade::check_counter_upgrade`]).
pub fn button_locked(sim: &Sim, slot: i32, b: &ToolbarButton) -> bool {
    button_locked_in(sim.state, sim.rules, slot, b)
}

/// Read-only variant of [`button_locked`] on state and rules (the command bar of the UI holds no `&Sim`):
/// the same calculation - only kinds with prerequisite check, then
/// [`missing_required_in`], the nation stub and [`counter_blocked_in`]. No state change.
///
/// The lock of the drawn command bar (`0x0044F250` step 1, `nml_ui` `ToolbarModel::locks_for`) is the same
/// **without** the nation stub: drawing reads only the `REQUIRED_GUID` list of the button (`+0x50`), the button
/// does not carry `REQUIRED_GUID_NATION` at all (Ghidra; re/game-ui N.5.1, O-230). In the shipped data the
/// nation list is empty, both locks are the same there (corpus test in `nml-client`).
pub fn button_locked_in(state: &SimState, rules: &Rules, slot: i32, b: &ToolbarButton) -> bool {
    if !CHECKED_KINDS.contains(&b.kind()) {
        return false;
    }
    missing_required_in(state, rules, slot, b)
        || missing_in(state, rules, slot, &b.required_nation, &|_| 0)
        || counter_blocked_in(state, slot, b)
}

/// First part of `Toolbar_IsButtonLocked` (`0x0044AF60`): a `REQUIRED_GUID` (up to the first 0) has no
/// object counter. Without a check of the kind (the lock of the command bar, [`button_locked_in`], does that
/// beforehand) and without the nation stub; the command bar needs this part separately (button 602 and veil).
pub fn missing_required_in(state: &SimState, rules: &Rules, slot: i32, b: &ToolbarButton) -> bool {
    missing_in(state, rules, slot, &b.required, &|g| g as u32)
}

/// Second part of `Toolbar_IsButtonLocked` (`0x0044AF60`): a `COUNTERUPGRADE` button whose GUID
/// runs anywhere or whose slot already has a running counter-upgrade ([`counter::is_running`],
/// [`counter::count_of_slot`]); false for every other kind.
pub fn counter_blocked_in(state: &SimState, slot: i32, b: &ToolbarButton) -> bool {
    b.kind() == button_kind::COUNTERUPGRADE
        && (counter::is_running(state, b.msg as u32)
            || u8::try_from(slot).is_ok_and(|p| counter::count_of_slot(state, p) != 0))
}

/// One of the GUIDs of the list (up to the first 0, mapped via `map`) has no object counter.
fn missing_in(
    state: &SimState,
    rules: &Rules,
    slot: i32,
    list: &[i32; 5],
    map: &dyn Fn(i32) -> u32,
) -> bool {
    list.iter()
        .take_while(|g| **g != 0)
        .any(|&g| guid_count_in(state, rules, slot, map(g)) == 0)
}

/// One-time kind (`ONLYONCE`, `UPGRADE`, `MAKEONLY_ONCE`) and "in progress" != 0 or object counter != 0
/// (`Toolbar_BuildButtonSet` `0x004557C0`, branch `LAB_004558e2`: two separate checks; the
/// button is then hidden).
pub fn once_taken(sim: &Sim, slot: i32, b: &ToolbarButton, guid: u32) -> bool {
    once_taken_in(sim.state, sim.rules, slot, b, guid)
}

/// Read-only variant of [`once_taken`] on state and rules: the command bar hides one-time kinds
/// with object counter or order. No state change.
pub fn once_taken_in(
    state: &SimState,
    rules: &Rules,
    slot: i32,
    b: &ToolbarButton,
    guid: u32,
) -> bool {
    ONCE_KINDS.contains(&b.kind())
        && (pending_guid_count_in(state, rules, slot, guid) != 0
            || guid_count_in(state, rules, slot, guid) != 0)
}

/// `Player_CanBuildOrMakeGuid` (`0x004305E0`, re/ki-queries 5.12.4): can the slot `slot` build
/// or make the GUID? The checks of the AI in this order:
///
/// 1. Cost affordable ([`can_afford_object`], **silent**: the original reports the shortage to the local
///    player, the simulation knows none, deviation, E-67);
/// 2. not locked ([`crate::world::is_guid_disabled`]: level data or script);
/// 3. Toolbar entry of the GUID: the **first** button with `BUTTONMSG == guid` across all menus
///    (`GameWorld_LookupToolbarEntry`, [`nml_formats::gifcon::Toolbar::lookup`]), not the menu
///    of a worker as with the commands; if it is missing, `false`;
/// 4. one-time kinds (`UPGRADE`, `ONLYONCE`, `MAKEONLY_ONCE`) with object counter `≥ 1` or order
///    `≥ 1`: `false`;
/// 5. each of the five `REQUIRED_GUID` (`≠ 0`) with object counter `< 1`: `false`.
///
/// `REQUIRED_GUID_NATION` is never checked by the function (E-68; the nation stub returns 0). The comparisons
/// are its own (`≥ 1`, `< 1`), not those of [`button_locked`] and [`once_taken`], which differ for
/// negative counters.
pub fn can_build_or_make_guid(sim: &mut Sim, slot: i32, guid: u32) -> bool {
    if !can_afford_object(sim, slot, guid, true, (-1, -1)) {
        return false;
    }
    if crate::world::is_guid_disabled(sim, slot, guid) {
        return false;
    }
    let Some(button) = sim.rules.toolbar.lookup(guid as i32) else {
        return false;
    };
    if ONCE_KINDS.contains(&button.kind())
        && (guid_count(sim, slot, guid) >= 1 || pending_guid_count(sim, slot, guid) >= 1)
    {
        return false;
    }
    button
        .required
        .iter()
        .all(|&r| r == 0 || guid_count(sim, slot, r as u32) >= 1)
}

/// The object exists, belongs to the slot `player` and has the building module.
pub fn own_building(sim: &Sim, player: u8, id: ObjectId) -> bool {
    sim.object(id)
        .is_some_and(|o| id.slot() == player && o.has_module(building::INDEX, false))
}

/// The building is finished (`CONSTRUCTED_FINISHED`).
pub fn is_finished(sim: &Sim, id: ObjectId) -> bool {
    sim.module(id, building::INDEX)
        .is_some_and(|m| m.base.has_flags(flags::CONSTRUCTED_FINISHED))
}

/// The building is dying or dead (`m_IsDead` or state `DIED`).
pub fn is_dead(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).is_some_and(object_is_dead)
}

/// [`is_dead`] for an object (also for UIs that only read the state): building module with
/// `m_IsDead` or state `DIED` (a dormant building counts as dead).
pub fn object_is_dead(object: &GameObject) -> bool {
    matches!(object.module(building::INDEX).map(|m| &m.kind),
        Some(ModuleKind::Building(b)) if b.is_dead)
        || object.status & status::DIED != 0
}

/// Build check of the commands `0x01`/`0x18` without a building site: the type has the building module, an own
/// worker from `workers` shows it in its build menu (first matching worker in
/// command order), the button is not locked and - for `ONLYONCE` - object counter and "in
/// progress" are 0. Error: reason for the log.
pub fn check_build(sim: &Sim, player: u8, guid: u32, workers: &[ObjectId]) -> Result<(), String> {
    check_build_count(sim, player, guid, workers, 1)
}

/// [`check_build`] for `count` parts of a command (`0x18`): a one-time type (`ONLYONCE`) applies
/// to the whole command - more than one part is rejected.
pub fn check_build_count(
    sim: &Sim,
    player: u8,
    guid: u32,
    workers: &[ObjectId],
    count: usize,
) -> Result<(), String> {
    if building_flags(sim.rules, guid).is_none() {
        return Err(format!("type {guid} is not a building"));
    }
    if crate::world::is_guid_disabled(sim, i32::from(player), guid) {
        return Err(format!("{guid} is locked on this map"));
    }
    let button = workers
        .iter()
        .filter(|w| w.slot() == player)
        .filter_map(|&w| sim.object(w))
        .find_map(|o| {
            menu_button(sim.rules, o.guid, |b| {
                b.msg == guid as i32 && BUILD_KINDS.contains(&b.kind())
            })
        })
        .ok_or_else(|| format!("no own worker has {guid} in the build menu"))?;
    let slot = i32::from(player);
    if button_locked(sim, slot, button) {
        return Err(format!("prerequisite for {guid} missing"));
    }
    if once_taken(sim, slot, button, guid) || (count > 1 && ONCE_KINDS.contains(&button.kind())) {
        return Err(format!("{guid} exists only once"));
    }
    Ok(())
}

/// Check of `0x0F`: own, non-dead building (a dead one can no longer be selected,
/// `PropertyBuilding_Destroy` removes it from the selection); index 0 only with a running order
/// (`make_guid ≠ 0`), `1 … n` only within the queue. Otherwise `0x1103` would abort an order
/// to GUID 0 and lower its "in progress" counter (the handler checks nothing with the running order).
pub fn check_cancel(
    sim: &Sim,
    player: u8,
    building_id: ObjectId,
    index: i32,
) -> Result<(), String> {
    check_order(sim, player, building_id, index)
}

/// An order `index` (0 = running, `1 … n` = waiting) of an own, non-dead building
/// (a dormant one is dead): check of `0x0F` ([`check_cancel`]) and of the debug command "complete
/// immediately".
pub fn check_order(sim: &Sim, player: u8, building_id: ObjectId, index: i32) -> Result<(), String> {
    if !own_building(sim, player, building_id) {
        return Err(format!("{} is not an own building", building_id.0));
    }
    if is_dead(sim, building_id) {
        return Err(format!("{} is dead", building_id.0));
    }
    let b = building::building(sim, building_id);
    let (running, waiting) = b.map_or((0, 0), |b| (b.make_guid, b.make_queue.len()));
    let ok = match index {
        0 => running != 0,
        i => usize::try_from(i).is_ok_and(|i| (1..=waiting).contains(&i)),
    };
    if !ok {
        return Err(format!(
            "no order with index {index} in {} (running {running}, {waiting} waiting)",
            building_id.0
        ));
    }
    Ok(())
}

/// Check of the debug command "complete immediately": [`check_order`] and the order at `index`
/// has the type `guid` that the sender saw there (otherwise the command would hit a different entry after the
/// queue advanced).
pub fn check_debug_order(
    sim: &Sim,
    player: u8,
    building_id: ObjectId,
    index: i32,
    guid: u32,
) -> Result<(), String> {
    check_order(sim, player, building_id, index)?;
    let b = building::building(sim, building_id);
    let actual = match index {
        0 => b.map_or(0, |b| b.make_guid),
        i => usize::try_from(i - 1)
            .ok()
            .and_then(|i| b.and_then(|b| b.make_queue.get(i).copied()))
            .unwrap_or(0),
    };
    if actual != guid {
        return Err(format!(
            "order {index} in {} is {actual}, not {guid}",
            building_id.0
        ));
    }
    Ok(())
}

/// Check of `0x0E`: own, finished, living building; GUID != 0, a known type, an
/// upgrade or trade (490-493) or a paddock (3423/3424), not locked on the map
/// ([`crate::world::is_guid_disabled`]); the GUID is a `MAKE`/`MAKEONLY_ONCE` or `UPGRADE`
/// button in the menu of the building type, not locked, one-time ones only with object counter and "in
/// progress" 0. A `COUNTERUPGRADE` button is rejected (it executes a unit).
pub fn check_production(
    sim: &Sim,
    player: u8,
    building_id: ObjectId,
    guid: u32,
) -> Result<(), String> {
    if !own_building(sim, player, building_id) {
        return Err(format!("{} is not an own building", building_id.0));
    }
    if !is_finished(sim, building_id) || is_dead(sim, building_id) {
        return Err(format!("{} is not finished or dead", building_id.0));
    }
    if guid == 0 {
        return Err("GUID 0".into());
    }
    let known = is_trade_guid(guid)
        || matches!(guid, PADDOCK_A | PADDOCK_B)
        || sim.rules.types.contains_key(&guid)
        || sim.rules.is_upgrade(guid);
    if !known {
        return Err(format!("unknown type {guid}"));
    }
    if crate::world::is_guid_disabled(sim, i32::from(player), guid) {
        return Err(format!("{guid} is locked on this map"));
    }
    let menu = sim.object(building_id).map_or(0, |o| o.guid);
    let button = menu_button(sim.rules, menu, |b| b.msg == guid as i32)
        .ok_or_else(|| format!("{guid} is not in the menu of {menu}"))?;
    if button.kind() == button_kind::COUNTERUPGRADE {
        return Err(format!("counter-upgrade {guid} executes a unit"));
    }
    if !MAKE_KINDS.contains(&button.kind()) && button.kind() != button_kind::UPGRADE {
        return Err(format!("{guid} is not a training order"));
    }
    let slot = i32::from(player);
    if button_locked(sim, slot, button) {
        return Err(format!("prerequisite for {guid} missing"));
    }
    if once_taken(sim, slot, button, guid) {
        return Err(format!("{guid} exists only once"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SimState;
    use nml_formats::gifcon::parse_toolbar;

    const TOOLBAR: &str = "GUID 1100\nBUTTONID 0\nBUTTONMSG 110\nTARGET_DO_MSG\nBUTTONID 1\nBUTTONMSG 413\nTARGET_MSG\nENDGUID\nGUID 1102\nBUTTONID 0\nBUTTONMSG 110\nBUTTONID 1\nBUTTONMSG 1000\nDO_MSG\nENDGUID\nGUID 110\nBUTTONID 0\nBUTTONMSG 1000\nBUILD_MSG\nGROUP_AND_MSG\nONLYONCE_MSG\nBUTTONID 1\nBUTTONMSG 1001\nBUILD_MSG\nBUTTONID 2\nBUTTONMSG 1005\nBUILD_MSG\nREQUIRED_GUID 1000\nBUTTONID 3\nBUTTONMSG 1006\nBUILD_MSG\nREQUIRED_GUID_NATION 5\nENDGUID\nGUID 1000\nBUTTONID 0\nBUTTONMSG 1100\nMAKE_MSG\nREQUIRED_GUID 1000\nBUTTONID 1\nBUTTONMSG 2806\nUPGRADE_MSG\nENDGUID\n";

    #[test]
    fn menus_are_searched_with_one_level_of_submenus() {
        let mut rules = Rules::synthetic("", 16);
        rules.toolbar = parse_toolbar(TOOLBAR);
        let build = |b: &ToolbarButton| BUILD_KINDS.contains(&b.kind());
        let found = |menu, guid: i32| {
            menu_button(&rules, menu, |b| b.msg == guid && build(b)).map(|b| b.kind())
        };
        assert_eq!(found(1100, 1000), Some(button_kind::ONLYONCE));
        assert_eq!(found(1100, 1001), Some(button_kind::BUILD));
        // A button without a kind opens its submenu just the same (female settler); a DO button
        // does not, even if its number is a menu.
        assert_eq!(found(1102, 1001), Some(button_kind::BUILD));
        assert_eq!(
            menu_button(&rules, 1102, |b| b.msg == 1100).map(|b| b.kind()),
            None
        );
        // The HQ menu shows no buildings, not even via the training 1100 (MAKE).
        assert_eq!(found(1000, 1001), None);
        assert_eq!(found(4711, 1001), None);
        // The whole list by the same rule: the menu, then the submenus (110 via the button without
        // a kind, not 1000 via DO).
        let msgs = |menu| {
            menu_buttons(&rules, menu)
                .iter()
                .map(|b| b.msg)
                .collect::<Vec<_>>()
        };
        assert_eq!(msgs(1102), [110, 1000, 1000, 1001, 1005, 1006]);
        assert_eq!(msgs(1100), [110, 413, 1000, 1001, 1005, 1006]);
        assert_eq!(msgs(1000), [1100, 2806]);
        assert!(msgs(4711).is_empty());
        // Prerequisites: 1005 needs the HQ; the nation stub always locks.
        let mut state = SimState::new(0, 0);
        let b1005 = menu_button(&rules, 110, |b| b.msg == 1005).unwrap();
        let b1006 = menu_button(&rules, 110, |b| b.msg == 1006).unwrap();
        let b1001 = menu_button(&rules, 110, |b| b.msg == 1001).unwrap();
        {
            let sim = Sim::new(&mut state, &rules);
            assert!(button_locked(&sim, 0, b1005));
            assert!(button_locked(&sim, 0, b1006));
            assert!(!button_locked(&sim, 0, b1001));
        }
        state.add_guid_count(0, 1000, 1);
        let sim = Sim::new(&mut state, &rules);
        assert!(!button_locked(&sim, 0, b1005));
        let hq = menu_button(&rules, 110, |b| b.msg == 1000).unwrap();
        assert!(once_taken(&sim, 0, hq, 1000));
        assert!(!once_taken(&sim, 1, hq, 1000));
        assert!(!once_taken(&sim, 0, b1001, 1001));
    }

    #[test]
    fn button_lock_207_blocks_toolbar_ki_query_and_headhunter_buttons() {
        // The AI query `Player_CanBuildOrMakeGuid` (`0x004305E0`) asks `Player_IsGuidDisabled`; the
        // script lock 207 applies globally (`GameWorld+0x21E0`), the same for every slot. The
        // bar, the headhunter buttons and the counter-upgrades have their own tests (`command/economy.rs`,
        // `command/headhunter.rs`, `upgrades/counter.rs`).
        let mut rules = Rules::synthetic("", 16);
        rules.toolbar = parse_toolbar(CAN_TOOLBAR);
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        assert!(can_build_or_make_guid(&mut sim, 0, 1001));
        for slot in [0, 1, 7] {
            assert!(!crate::world::is_guid_disabled(&sim, slot, 1001));
        }
        crate::script::world::view::lock_button(&mut sim, 1001, true);
        for slot in [0, 1, 7, 8] {
            assert!(crate::world::is_guid_disabled(&sim, slot, 1001));
        }
        assert!(!can_build_or_make_guid(&mut sim, 0, 1001));
        assert!(!can_build_or_make_guid(&mut sim, 1, 1001));
        // Other GUIDs stay free; 208 releases.
        assert!(can_build_or_make_guid(&mut sim, 0, 1000));
        crate::script::world::view::lock_button(&mut sim, 1001, false);
        assert!(can_build_or_make_guid(&mut sim, 0, 1001));
        assert!(can_build_or_make_guid(&mut sim, 1, 1001));
    }

    #[test]
    fn once_checks_stock_and_pending_separately() {
        // `Toolbar_BuildButtonSet` (`0x004557C0`): hidden as soon as "in progress" != 0 **or**
        // object counter != 0 - even if both cancel out to 0.
        let mut rules = Rules::synthetic("", 16);
        rules.toolbar = parse_toolbar(TOOLBAR);
        let hq = menu_button(&rules, 110, |b| b.msg == 1000).unwrap().clone();
        let mut state = SimState::new(0, 0);
        state.add_guid_count(0, 1000, -1);
        crate::economy::add_pending_guid_count(&mut state, 0, 1000, 1);
        let sim = Sim::new(&mut state, &rules);
        assert_eq!(
            guid_count(&sim, 0, 1000) + pending_guid_count(&sim, 0, 1000),
            0
        );
        assert!(once_taken(&sim, 0, &hq, 1000));
    }

    /// Menus for `Player_CanBuildOrMakeGuid`: house 1001, one-time HQ 1000, 1005 with
    /// prerequisite HQ, 1006 with nation prerequisite, research 2806 and a building (1020) with
    /// five prerequisites. The GUID 1000 is only in the first menu (first mention wins).
    const CAN_TOOLBAR: &str = "GUID 110
BUTTONID 0
BUTTONMSG 1000
BUILD_MSG
ONLYONCE_MSG
BUTTONID 1
BUTTONMSG 1001
BUILD_MSG
BUTTONID 2
BUTTONMSG 1005
BUILD_MSG
REQUIRED_GUID 1000
BUTTONID 3
BUTTONMSG 1006
BUILD_MSG
REQUIRED_GUID_NATION 5
ENDGUID
GUID 1000
BUTTONID 0
BUTTONMSG 2806
UPGRADE_MSG
ENDGUID
GUID 120
BUTTONID 0
BUTTONMSG 1020
BUILD_MSG
REQUIRED_GUID 1000
REQUIRED_GUID 1001
REQUIRED_GUID 1002
REQUIRED_GUID 1003
REQUIRED_GUID 1004
ENDGUID
";

    #[test]
    fn can_build_or_make_guid_follows_the_original_checks() {
        let mut rules = Rules::synthetic("", 16);
        rules.toolbar = parse_toolbar(CAN_TOOLBAR);
        // House 1001 costs 150 wood (config 11); 1005 and 1000 are free.
        rules.defaults.insert(1001, [(11, 150)].into());
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        fn can(sim: &mut Sim, guid: u32) -> bool {
            can_build_or_make_guid(sim, 0, guid)
        }

        // 1. Too expensive: silent (no message), then affordable.
        sim.state.players[0].set_resource(1, 100);
        assert!(!can(&mut sim, 1001));
        assert!(sim.state.events.is_empty(), "silent");
        sim.state.players[0].set_resource(1, 200);
        assert!(can(&mut sim, 1001));

        // 2. Locked (config 1 = 0 in the value set).
        let mut disabled = rules.clone();
        disabled.defaults.insert(1001, [(11, 150), (1, 0)].into());
        let mut state2 = SimState::new(0, 0);
        state2.players[0].set_resource(1, 200);
        let mut sim2 = Sim::new(&mut state2, &disabled);
        assert!(!can(&mut sim2, 1001));

        // 3. No toolbar entry (also not searched in a foreign menu): false.
        assert!(!can(&mut sim, 9999));

        // 4. One-time kinds: object counter or order >= 1 locks, zeros or negatives do not.
        assert!(can(&mut sim, 1000), "ONLYONCE without object counter");
        sim.state.add_guid_count(0, 1000, 1);
        assert!(!can(&mut sim, 1000));
        sim.state.add_guid_count(0, 1000, -2); // object counter -1: not >= 1 (unlike `once_taken`)
        assert!(can(&mut sim, 1000));
        crate::economy::add_pending_guid_count(sim.state, 0, 1000, 1);
        assert!(!can(&mut sim, 1000), "order >= 1");
        crate::economy::add_pending_guid_count(sim.state, 0, 1000, -1);
        sim.state.add_guid_count(0, 1000, 1);
        assert!(can(&mut sim, 1000));
        // UPGRADE (2806) in the HQ menu: same rule; the button does not need the HQ object counter.
        assert!(can(&mut sim, 2806));
        sim.state.add_guid_count(0, 2806, 1);
        assert!(!can(&mut sim, 2806));
        // Another slot has its own counters.
        assert!(can_build_or_make_guid(&mut sim, 1, 1000));

        // 5. Prerequisites: 1005 needs the HQ (object counter >= 1, a negative object counter is not enough).
        assert!(!can(&mut sim, 1005));
        sim.state.add_guid_count(0, 1000, -1);
        assert!(!can(&mut sim, 1005));
        sim.state.add_guid_count(0, 1000, 2); // object counter 1
        assert!(can(&mut sim, 1005));
        sim.state.add_guid_count(0, 1000, -1);
        assert!(!can(&mut sim, 1005));
        // All five prerequisites count, also the last.
        for g in [1000, 1001, 1002, 1003] {
            sim.state.add_guid_count(0, g, 1);
        }
        assert!(!can(&mut sim, 1020), "the fifth (1004) is missing");
        sim.state.add_guid_count(0, 1004, 1);
        assert!(can(&mut sim, 1020));

        // 6. `REQUIRED_GUID_NATION` is never checked by this function (E-68), unlike `button_locked`.
        assert!(can(&mut sim, 1006));
        let b1006 = rules.toolbar.lookup(1006).unwrap();
        assert!(button_locked(&sim, 0, b1006));
    }

    #[test]
    fn disabled_and_locked_match_the_sim_variants() {
        // The `&Sim` variants are wrappers around the read-only `_in` variants; what is compared here is mainly
        // that the parts `missing_required_in` and `counter_blocked_in` compose `button_locked_in`. The
        // equality with the lock of the command bar is checked by `toolbar_locks_match_the_sim_for_every_corpus_button`
        // (`nml-client`, corpus).
        let counter_toolbar =
            format!("{TOOLBAR}GUID 601\nBUTTONID 0\nBUTTONMSG 3374\nCOUNTERUPGRADE_MSG\nENDGUID\n");
        let mut rules = Rules::synthetic("", 16);
        rules.toolbar = parse_toolbar(&counter_toolbar);
        rules
            .level
            .slot_configs
            .entry(0)
            .or_default()
            .insert(1001, [(1, 0)].into_iter().collect());
        let mut state = SimState::new(0, 0);
        let all: Vec<ToolbarButton> = rules.toolbar.menus.values().flatten().cloned().collect();
        assert!(all.len() >= 10);
        let compare = |state: &mut SimState, rules: &Rules| {
            let sim = Sim::new(state, rules);
            for slot in -1..9 {
                for guid in [0, 1000, 1001, 1005, 1100, 3374, 5555] {
                    assert_eq!(
                        crate::world::is_guid_disabled_in(sim.state, sim.rules, slot, guid),
                        crate::world::is_guid_disabled(&sim, slot, guid),
                        "locked {slot} {guid}"
                    );
                }
                for b in &all {
                    let locked = button_locked(&sim, slot, b);
                    assert_eq!(
                        button_locked_in(sim.state, sim.rules, slot, b),
                        locked,
                        "locked {slot} {}",
                        b.msg
                    );
                    assert_eq!(
                        once_taken_in(sim.state, sim.rules, slot, b, b.msg as u32),
                        once_taken(&sim, slot, b, b.msg as u32),
                        "one-time {slot} {}",
                        b.msg
                    );
                    // The parts make up the lock (nation stub excepted: unused in the shipped data).
                    if CHECKED_KINDS.contains(&b.kind()) && b.required_nation[0] == 0 {
                        assert_eq!(
                            locked,
                            missing_required_in(sim.state, sim.rules, slot, b)
                                || counter_blocked_in(sim.state, slot, b),
                            "parts {slot} {}",
                            b.msg
                        );
                    }
                }
            }
        };
        compare(&mut state, &rules);
        // Object counter, running order, script lock and a running counter-upgrade change results, never the equality.
        state.add_guid_count(0, 1000, 1);
        state.pending_counts[0].insert(1001, 1);
        state.button_locks.insert(1005, 1);
        compare(&mut state, &rules);
        state
            .counter_upgrades
            .push(crate::upgrades::CounterUpgrade {
                guid: 3374,
                source: ObjectId::new(0, 0, 1),
                rest_ms: 5000,
            });
        compare(&mut state, &rules);
        // Samples: prerequisite, nation stub, counter-upgrade, level data lock.
        let b = |msg: i32| all.iter().find(|b| b.msg == msg).unwrap();
        assert!(!missing_required_in(&state, &rules, 0, b(1005)));
        assert!(missing_required_in(&state, &rules, 1, b(1005)));
        assert!(button_locked_in(&state, &rules, 0, b(1006)));
        assert!(!missing_required_in(&state, &rules, 0, b(1006)));
        assert!(counter_blocked_in(&state, 1, b(3374)));
        assert!(!counter_blocked_in(&state, 1, b(1005)));
        // One-time kind: the running order (only "in progress", object counter 0) is enough for "taken".
        let once = ToolbarButton {
            msg: 1001,
            flags: button_kind::ONLYONCE,
            ..Default::default()
        };
        assert!(once_taken_in(&state, &rules, 0, &once, 1001));
        assert!(!once_taken_in(&state, &rules, 1, &once, 1001));
        assert_eq!(
            crate::economy::pending_guid_count_in(&state, &rules, 0, 1001),
            1
        );
        assert!(crate::world::is_guid_disabled_in(&state, &rules, 0, 1001));
        assert!(!crate::world::is_guid_disabled_in(&state, &rules, 1, 1001));
        assert!(crate::world::is_guid_disabled_in(&state, &rules, 1, 1005));
    }
}
