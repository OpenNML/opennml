//! Catalog of the script actions (0-512) and conditions (1-56) and the load check of an `.abl`.
//!
//! Sources: re/script-actions A.3 (case `0x72`), A.4 (waiting), A.8 (case, minimum count,
//! message), A.10, B, C (meaning), C.15; re/script-conditions D.1.
//!
//! `params` lists the parameters in `.abl` order; area parameters ([`ParamKind::Area`]) are exactly
//! the places the original blindly shortens with `Mid(5)` (A.8 column "message", `M`).

use super::{AblError, RtAction, RtCondition, ScriptFile};
use crate::property::atol;
// Short forms for the tables at the end of the file.
use Effect as E;
use ParamKind::*;
use WaitKind as W;

/// Kind of a parameter (editor, load check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// Integer (`atol`).
    Int,
    /// Player slot (`Player n`).
    Player,
    /// GUID or category filter (< 1000, re/script-actions B.2.3).
    Guid,
    /// Object or group name of the name table.
    Object,
    /// Area `area <name>`.
    Area,
    /// Named camera.
    Camera,
    /// Text ID of the text table.
    TextId,
    /// Name of a dialog script in the level folder.
    Dialog,
    /// Switch (container `+0x50`).
    Switch,
    /// Random switch (container `+0x34`).
    RandomSwitch,
    /// Timer (container `+0x6C`).
    Timer,
    /// Variable (container `+0x88`).
    Variable,
    /// Other text (e.g. name of a light preset).
    Text,
}

/// How an action with the `w` flag waits; everything except `None` calls `SuspendAtAction` (A.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    None,
    /// Fixed time or time computed from parameters, in ms.
    Time,
    /// Until the text line ends (query 600).
    Text,
    /// Until all objects of the name have the event bit (query 2500).
    EventBit(u8),
    /// Dialog start: always waits, continued at the next call.
    Dialog,
}

/// Route of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Layer E, script state (I).
    Internal,
    /// Directly on world, player, match (W).
    World,
    /// Game command via the script route (B).
    Command,
    /// Presentation event (P).
    Presentation,
    /// Has a case but no effect (-, E-133).
    NoEffect,
    /// D case present, not reachable from `.abl` (E-133).
    Unreachable,
    /// Case `0x72`: is skipped.
    NoCase,
}

/// Kind of a condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondEffect {
    /// Reads only the script state (container).
    Script,
    /// Script query via `FUN_00425F90` (code in `query`).
    Query,
    /// Depends on UI state (selection, button).
    Ui,
    /// No case: always false.
    NoCase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSpec {
    pub number: u16,
    /// Short editor text; empty for case `0x72`.
    pub name: &'static str,
    pub params: &'static [ParamKind],
    /// Fewer parameters raise an exception in the original (A.8 "min", E-127).
    pub min_params: u8,
    pub wait: WaitKind,
    pub effect: Effect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionSpec {
    pub number: u8,
    /// Short editor text; empty without a case.
    pub name: &'static str,
    pub params: &'static [ParamKind],
    pub min_params: u8,
    /// Code of the script query (`FUN_00425F90`, decimal).
    pub query: Option<u16>,
    pub effect: CondEffect,
}

/// Actions 0-512, index = number.
pub static ACTIONS: [ActionSpec; 513] = build_actions();

/// Conditions 1-56, index = number - 1.
pub static CONDITIONS: [ConditionSpec; 56] = build_conditions();

/// Catalog row of an action number (also case `0x72`); `None` outside 0-512.
pub fn action(number: i32) -> Option<&'static ActionSpec> {
    usize::try_from(number).ok().and_then(|n| ACTIONS.get(n))
}

/// Catalog row of a condition number (also without a case); `None` outside 1-56.
pub fn condition(number: i32) -> Option<&'static ConditionSpec> {
    usize::try_from(number)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| CONDITIONS.get(i))
}

impl AblError {
    /// User-visible message "Script {path}, line {line}: ...".
    pub fn in_script(&self, path: &str) -> String {
        format!("Script {path}, {self}")
    }
}

/// Load check: too few parameters (E-127), area parameters without `area `
/// (blind `Mid(5)`), action 21/22 with a slot outside 0-7 (E-159), action 39 in a dialog script
/// (E-174), action 35/197/202 with a slot outside 0-10 (E-197), action 203/204 with a stance outside
/// 0-4 (E-155) and 159/160/211/214 with a slot outside 0-10 (E-153), condition 2/12
/// with a player outside 0-7 and resource kind 0-4 (null pointer in the original, E-193) and condition 33 with
/// player < 0 (local player, E-191). The first violation in file order wins.
/// Numbers outside the catalog and without a case are valid (E-133). [`AblError::in_script`] prepends
/// the path to the message; `_path` is kept for the interface and is not needed here.
pub fn check(file: &ScriptFile, _path: &str, dialog: bool) -> Result<(), AblError> {
    let first = file
        .blocks
        .iter()
        .flat_map(|block| {
            let conditions = block.conditions.iter().filter_map(check_condition);
            let actions = block
                .actions
                .iter()
                .filter_map(move |a| check_action(a, dialog));
            conditions.chain(actions)
        })
        .min_by_key(|e| e.line);
    match first {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn check_condition(c: &RtCondition) -> Option<AblError> {
    let spec = condition(c.number)?;
    let message = check_params(
        "condition",
        c.number,
        spec.params,
        spec.min_params,
        &c.params,
    )
    .or_else(|| check_condition_player(c))?;
    Some(AblError {
        line: c.line,
        message,
    })
}

/// Player parameters the original cannot tolerate: condition 2/12 (query 100) with a player outside 0-7 and
/// resource kind 0-4 reads through a null pointer (E-193; another kind yields 0 without access), condition 33
/// (query 2200) with player < 0 reads the local player of the machine (E-191, host state).
fn check_condition_player(c: &RtCondition) -> Option<String> {
    let param = |i: usize| c.params.get(i).map(|p| atol(p) as i32);
    let player = param(0)?;
    let invalid = match c.number {
        2 | 12 => !(0..=7).contains(&player) && param(2).is_some_and(|k| (0..=4).contains(&k)),
        33 => player < 0,
        _ => false,
    };
    invalid.then(|| format!("condition {} with invalid player {}", c.number, c.params[0]))
}

fn check_action(a: &RtAction, dialog: bool) -> Option<AblError> {
    let spec = action(a.number)?;
    let message = check_params("action", a.number, spec.params, spec.min_params, &a.params)
        .or_else(|| match a.number {
            21 | 22 => check_slot(a, 0, 7),
            // 37 writes behind the engine table in the original without a slot check, 340 throws an exception for a
            // negative index, 380 reads behind the player table with a customer outside 0-7 (E-221,
            // E-220, E-218). The target player of 380 stays unchecked (E-218).
            37 => check_slot(a, 0, 7),
            340 => check_index(a, 1),
            380 => check_slot(a, 0, 7),
            // Target slot of 35/197 (third parameter) and 202 (second): the original does not check it
            // and indexes tables with 11 entries (E-197).
            35 | 197 => check_slot(a, 2, 10),
            202 => check_slot(a, 1, 10),
            // Stance of 203 (second parameter) and 204 (fourth): the original writes every value; for a
            // stance outside 0-4 it leaves words of the messages `0x611`/`0x2100` unset (heap remnants, not
            // deterministic, C-341; E-155).
            203 => check_behavior(a, 1),
            204 => check_behavior(a, 3),
            // Slot of 159/160/211/214 (first parameter): the original does not check it and indexes
            // tables with 11 entries (E-153).
            159 | 160 | 211 | 214 => check_slot(a, 0, 10),
            // Speaking slot of 150 (first slot < 0 writes behind the text slots in the original, E-241),
            // pos of 213 (slot pos + 4 < 0, likewise; also above `i32::MAX - 4`, where pos + 4 overflows and
            // becomes negative): rejection (deviation from the original). The player of 7 stays unchecked
            // (E-244).
            150 => check_min(a, 1, 0, "slot"),
            213 => check_min(a, 1, -4, "slot").or_else(|| check_max(a, 1, i32::MAX - 4, "slot")),
            39 if dialog => Some("dialog script starts a dialog (action 39)".to_string()),
            _ => None,
        })?;
    Some(AblError {
        line: a.line,
        message,
    })
}

/// Slot parameter `index` of the action lies in `0..=max`, otherwise the message "action {n} with invalid slot {p}".
fn check_slot(a: &RtAction, index: usize, max: i32) -> Option<String> {
    let param = a.params.get(index);
    let valid = param.is_some_and(|p| (0..=max).contains(&(atol(p) as i32)));
    (!valid).then(|| {
        let shown = param.map_or("(missing)", String::as_str);
        format!("action {} with invalid slot {shown}", a.number)
    })
}

/// Parameter `index` of the action is at least `min` (missing = 0 like `atol("")`), otherwise the message
/// "action {n} with invalid {what} {p}".
fn check_min(a: &RtAction, index: usize, min: i32, what: &str) -> Option<String> {
    let param = a.params.get(index);
    let valid = param.is_none_or(|p| atol(p) as i32 >= min);
    (!valid).then(|| {
        format!(
            "action {} with invalid {what} {}",
            a.number,
            param.map_or("", String::as_str)
        )
    })
}

/// Parameter `index` of the action is at most `max` (missing = 0 like `atol("")`), otherwise the message
/// "action {n} with invalid {what} {p}".
fn check_max(a: &RtAction, index: usize, max: i32, what: &str) -> Option<String> {
    let param = a.params.get(index);
    let valid = param.is_none_or(|p| atol(p) as i32 <= max);
    (!valid).then(|| {
        format!(
            "action {} with invalid {what} {}",
            a.number,
            param.map_or("", String::as_str)
        )
    })
}

/// Index parameter `index` of the action is not negative (missing = 0 like `atol("")`), otherwise the message
/// "action {n} with negative index {i}".
fn check_index(a: &RtAction, index: usize) -> Option<String> {
    let param = a.params.get(index);
    let valid = param.is_none_or(|p| atol(p) as i32 >= 0);
    (!valid).then(|| {
        format!(
            "action {} with negative index {}",
            a.number,
            param.map_or("", String::as_str)
        )
    })
}

/// Stance (parameter `index`, missing = 0 like `atol("")`) of 203/204 lies in 0-4, otherwise the message
/// "action {n} with invalid stance {h}".
fn check_behavior(a: &RtAction, index: usize) -> Option<String> {
    let param = a.params.get(index);
    let valid = param.is_none_or(|p| (0..=4).contains(&(atol(p) as i32)));
    (!valid).then(|| {
        format!(
            "action {} with invalid stance {}",
            a.number,
            param.map_or("", String::as_str)
        )
    })
}

fn check_params(
    what: &str,
    number: i32,
    kinds: &[ParamKind],
    min: u8,
    params: &[String],
) -> Option<String> {
    if params.len() < usize::from(min) {
        return Some(format!(
            "{what} {number} needs at least {min} parameters (has {})",
            params.len()
        ));
    }
    kinds
        .iter()
        .zip(params)
        .find(|(kind, p)| **kind == ParamKind::Area && !p.starts_with("area "))
        .map(|(_, p)| format!("area parameter \"{p}\" without \"area \""))
}

// ---------------------------------------------------------------------------------------------
// Tables

const fn act(
    number: u16,
    name: &'static str,
    params: &'static [ParamKind],
    min_params: u8,
    wait: WaitKind,
    effect: Effect,
) -> ActionSpec {
    ActionSpec {
        number,
        name,
        params,
        min_params,
        wait,
        effect,
    }
}

const fn cond(
    number: u8,
    name: &'static str,
    params: &'static [ParamKind],
    query: Option<u16>,
    effect: CondEffect,
) -> ConditionSpec {
    // Minimum count = number of parameters read (D.1: if one is missing, the original throws).
    ConditionSpec {
        number,
        name,
        params,
        min_params: params.len() as u8,
        query,
        effect,
    }
}

const NO_CASE: ActionSpec = act(0, "", &[], 0, W::None, E::NoCase);

const fn build_actions() -> [ActionSpec; 513] {
    let mut table = [NO_CASE; 513];
    let mut i = 0;
    while i < table.len() {
        table[i].number = i as u16;
        i += 1;
    }
    let mut r = 0;
    while r < ACTION_ROWS.len() {
        let row = ACTION_ROWS[r];
        let n = row.number as usize;
        assert!(
            matches!(table[n].effect, Effect::NoCase),
            "duplicate action number in the catalog"
        );
        table[n] = row;
        r += 1;
    }
    table
}

const fn build_conditions() -> [ConditionSpec; 56] {
    let mut table = [cond(0, "", &[], None, CondEffect::NoCase); 56];
    let mut i = 0;
    while i < table.len() {
        table[i].number = i as u8 + 1;
        i += 1;
    }
    let mut r = 0;
    while r < CONDITION_ROWS.len() {
        let row = CONDITION_ROWS[r];
        let n = row.number as usize - 1;
        assert!(
            matches!(table[n].effect, CondEffect::NoCase),
            "duplicate condition number in the catalog"
        );
        table[n] = row;
        r += 1;
    }
    table
}

/// All actions with a case (A.8) and the five unreachable D cases (A.10); the remaining numbers
/// are case `0x72`. Column order: number, name, parameters, "min" (A.8), waiting (A.4),
/// route.
#[rustfmt::skip]
const ACTION_ROWS: &[ActionSpec] = &[
    act(0, "Start AI module", &[Player, Int], 0, W::None, E::World),
    act(1, "Start AI module concurrently", &[Player, Int], 0, W::None, E::World),
    act(2, "Camera to area", &[Player, Area], 2, W::None, E::Presentation),
    act(3, "Create units in area", &[Player, Int, Guid, Area], 4, W::None, E::World),
    act(4, "Defeat for player", &[Player], 1, W::None, E::Command),
    act(5, "Show on-screen text", &[TextId, Int, Int], 0, W::None, E::Presentation),
    act(6, "Remove objects in area", &[Player, Guid, Area], 3, W::None, E::World),
    act(7, "Map signal on area", &[Player, Area], 2, W::None, E::Presentation),
    act(8, "Move objects between areas", &[Player, Guid, Area, Area], 4, W::None, E::Command),
    act(9, "Play music", &[Int], 1, W::None, E::Presentation),
    act(10, "Block repeatable", &[], 0, W::None, E::Internal),
    act(11, "Set countdown", &[Int], 1, W::None, E::Internal),
    act(12, "Set switch", &[Switch], 1, W::None, E::Internal),
    act(13, "Victory for player", &[Player], 1, W::None, E::Command),
    act(14, "Create neutral object on tile", &[Guid, Int, Int], 0, W::None, E::World),
    act(15, "Show countdown", &[], 0, W::None, E::World),
    act(16, "Hide countdown", &[], 0, W::None, E::World),
    act(17, "No effect", &[Int], 1, W::None, E::NoEffect),
    act(18, "Lock user interface", &[], 0, W::None, E::Presentation),
    act(19, "Unlock user interface", &[], 0, W::None, E::Presentation),
    act(20, "Reveal area", &[Player, Area], 2, W::None, E::World),
    act(21, "Give resources", &[Player, Int, Int], 0, W::None, E::World),
    act(22, "Take resources", &[Player, Int, Int], 0, W::None, E::World),
    act(23, "Clear switch", &[Switch], 1, W::None, E::Internal),
    act(24, "Cover area again", &[Player, Area], 2, W::None, E::World),
    act(25, "Set timer", &[Timer, Int], 2, W::None, E::Internal),
    act(26, "Clear timer", &[Timer], 1, W::None, E::Internal),
    act(27, "Set main objective", &[Int, TextId], 0, W::None, E::World),
    act(28, "Shared vision on", &[Player, Player], 0, W::None, E::World),
    act(29, "Shared vision off", &[Player, Player], 0, W::None, E::World),
    act(30, "Ally players", &[Player, Player], 0, W::None, E::World),
    act(31, "Make players enemies", &[Player, Player], 0, W::None, E::World),
    act(32, "Set players neutral", &[Player, Player], 0, W::None, E::World),
    act(33, "Set random switch", &[RandomSwitch, Int], 2, W::None, E::Internal),
    act(34, "Kill objects in area", &[Player, Guid, Area], 3, W::None, E::Command),
    act(35, "Transfer objects in area", &[Guid, Area, Player], 3, W::None, E::World),
    act(36, "Create object on tile", &[Guid, Int, Int, Player], 0, W::None, E::World),
    act(37, "Halt AI", &[Player], 0, W::None, E::World),
    act(38, "Instant upgrade", &[Player, Guid], 2, W::None, E::Command),
    act(39, "Start dialog", &[Dialog], 1, W::Dialog, E::Internal),
    act(40, "Wait", &[Int], 1, W::Time, E::Internal),
    act(137, "Set switch (variant)", &[Switch], 1, W::None, E::Internal),
    act(138, "Clear switch (variant)", &[Switch], 1, W::None, E::Internal),
    act(139, "Set timer (variant)", &[Timer, Int], 2, W::None, E::Internal),
    act(140, "Clear timer (variant)", &[Timer], 1, W::None, E::Internal),
    act(141, "Camera to camera point", &[Player, Camera], 2, W::None, E::Presentation),
    act(142, "Camera move to camera point", &[Player, Camera, Int], 3, W::Time, E::Presentation),
    act(143, "Store camera position", &[], 0, W::None, E::Presentation),
    act(144, "Set stored camera position", &[], 0, W::None, E::Presentation),
    act(145, "Fade scene in", &[Int], 1, W::Time, E::Presentation),
    act(146, "Fade scene out", &[Int], 1, W::Time, E::Presentation),
    act(147, "Cinemascope on", &[], 0, W::None, E::Presentation),
    act(148, "Cinemascope off", &[], 0, W::None, E::Presentation),
    act(149, "Show portrait", &[Int, Int, Int], 3, W::Time, E::Presentation),
    act(150, "Speak dialog line", &[TextId, Int], 2, W::Text, E::Presentation),
    act(151, "Place named object", &[Player, Guid, Object, Area], 4, W::None, E::World),
    act(152, "Mark object", &[Object], 1, W::None, E::Presentation),
    act(153, "Move unit", &[Object, Area, Area], 3, W::EventBit(1), E::Command),
    act(154, "Turn unit toward area", &[Object, Area], 2, W::None, E::Command),
    act(155, "Remove mark", &[Object], 1, W::None, E::Presentation),
    act(156, "Hide portrait", &[Int], 1, W::Time, E::Presentation),
    act(157, "Play animation", &[Object, Int, Int], 3, W::None, E::Command),
    act(158, "End animation", &[Object], 1, W::None, E::Command),
    act(159, "Hide player", &[Player], 1, W::None, E::World),
    act(160, "Show player", &[Player], 1, W::None, E::World),
    act(161, "Hide object", &[Object], 1, W::None, E::World),
    act(162, "Show object", &[Object], 1, W::None, E::World),
    act(163, "Drive train forward", &[Object, Area], 2, W::EventBit(0), E::Command),
    act(164, "Drive train backward", &[Object, Area], 2, W::EventBit(0), E::Command),
    act(165, "Open gate", &[Object], 1, W::None, E::Command),
    act(166, "Close gate", &[Object], 1, W::None, E::Command),
    act(167, "Attack", &[Object, Object], 2, W::None, E::Command),
    act(168, "Show info window", &[TextId, Int, Int, Int], 4, W::None, E::Presentation),
    act(169, "Close info window", &[TextId], 1, W::None, E::Presentation),
    act(170, "Camera to object", &[Object], 1, W::None, E::Presentation),
    act(171, "Set light preset", &[Text], 1, W::None, E::Presentation),
    act(172, "Camera to game view", &[Int], 1, W::Time, E::Presentation),
    act(173, "Apply time-of-day upgrades", &[Player], 1, W::None, E::World),
    act(174, "Halt units in area", &[Area], 1, W::None, E::World),
    act(175, "Attach camera to object", &[Object], 1, W::None, E::Presentation),
    act(176, "Detach camera", &[], 0, W::None, E::Presentation),
    act(177, "Detach camera (variant)", &[Object], 1, W::None, E::Presentation),
    act(178, "Detach camera (variant 2)", &[Object], 1, W::None, E::Presentation),
    act(179, "Set combat mode", &[Object, Int], 2, W::None, E::Command),
    act(180, "Timer to random value", &[Timer, Int, Int], 3, W::None, E::Internal),
    act(181, "Move to object", &[Object, Object], 2, W::EventBit(1), E::Command),
    act(182, "Set secondary objective", &[Int, TextId], 0, W::None, E::World),
    act(183, "Show hint", &[TextId, Int], 0, W::Text, E::Presentation),
    act(184, "Secondary objective fulfilled", &[Int], 1, W::None, E::World),
    act(185, "Main objective fulfilled", &[Int], 1, W::None, E::World),
    act(186, "Secondary objective failed", &[Int], 1, W::None, E::World),
    act(187, "Main objective failed", &[Int], 1, W::None, E::World),
    act(188, "Move unit fast", &[Object, Area, Area], 3, W::EventBit(1), E::Command),
    act(189, "Special command", &[Object, Int], 2, W::None, E::Command),
    act(190, "Special command with target", &[Object, Int, Object], 3, W::None, E::Command),
    act(191, "Halt unit", &[Object], 1, W::None, E::Command),
    act(192, "Set maximum hit points", &[Object, Int], 2, W::None, E::World),
    act(193, "Kill object", &[Object], 1, W::None, E::Command),
    act(194, "Remove object", &[Object], 1, W::None, E::World),
    act(195, "Remove objects in area (variant)", &[Player, Guid, Area], 3, W::None, E::World),
    act(196, "Remove objects in area (variant 2)", &[Player, Guid, Area], 3, W::None, E::World),
    act(197, "Transfer objects in area (variant)", &[Guid, Area, Player], 3, W::None, E::World),
    act(198, "Reset secondary objectives", &[Int], 1, W::None, E::World),
    act(199, "Reset main objectives", &[Int], 1, W::None, E::World),
    act(200, "Set GUID flag", &[Guid], 0, W::None, E::World),
    act(201, "Set hit points", &[Object, Int], 2, W::None, E::World),
    act(202, "Transfer object", &[Object, Player], 2, W::None, E::World),
    act(203, "Stance of a player", &[Player, Int], 0, W::None, E::World),
    act(204, "Combat mode in area", &[Player, Guid, Area, Int], 4, W::None, E::World),
    act(205, "Apply time-of-day upgrades (variant)", &[Player], 0, W::None, E::World),
    act(206, "Play voice line (unreachable)", &[], 0, W::None, E::Unreachable),
    act(207, "Lock build button", &[Guid], 1, W::None, E::World),
    act(208, "Unlock build button", &[Guid], 1, W::None, E::World),
    act(209, "Start weather", &[Int], 1, W::None, E::Presentation),
    act(210, "End weather", &[Int], 1, W::None, E::Presentation),
    act(211, "Add area objects to inactive list", &[Player, Guid, Area], 3, W::None, E::World),
    act(212, "Rotate camera", &[Int, Int], 2, W::Time, E::Presentation),
    act(213, "Info window with portrait", &[TextId, Int, Int, Int], 4, W::Text, E::Presentation),
    act(214, "Remove area objects from inactive list", &[Player, Guid, Area], 3, W::None, E::World),
    act(215, "Refill special energy", &[Object], 1, W::None, E::World),
    act(216, "Move to stored camera", &[Int], 1, W::Time, E::Presentation),
    act(217, "Place named object (with angle)", &[Player, Guid, Object, Area, Int], 5, W::None, E::World),
    act(220, "Camera move over voice line length", &[Player, Camera, Int, TextId], 4, W::None, E::Presentation),
    act(221, "Camera rotation over voice line length", &[Int, Int, TextId], 3, W::None, E::Presentation),
    act(222, "Stored camera over voice line length", &[Int, TextId], 2, W::None, E::Presentation),
    act(223, "Game view over voice line length", &[Int, TextId], 2, W::None, E::Presentation),
    act(230, "Timer from voice line length", &[Timer, Int, TextId], 3, W::None, E::Internal),
    act(231, "Clear object event", &[Object, Int], 2, W::None, E::World),
    act(232, "Increase timer by voice line length", &[Timer, TextId], 2, W::None, E::Internal),
    act(233, "Timer from timer", &[Timer, Int, Timer], 3, W::None, E::Internal),
    act(240, "Camera move over timer", &[Player, Camera, Int, Timer], 4, W::Time, E::Presentation),
    act(241, "Camera rotation over timer", &[Int, Int, Timer], 3, W::Time, E::Presentation),
    act(242, "Stored camera over timer", &[Int, Timer], 2, W::Time, E::Presentation),
    act(243, "Game view over timer", &[Int, Timer], 2, W::None, E::Presentation),
    act(333, "Only mark as executed", &[], 0, W::None, E::Internal),
    act(334, "Quarter", &[Object, Object], 2, W::EventBit(1), E::Command),
    act(335, "Quarter (variant)", &[Object, Object], 2, W::EventBit(1), E::Command),
    act(336, "Put into boat", &[Object, Object], 2, W::EventBit(1), E::Command),
    act(337, "Mount", &[Object, Object], 2, W::EventBit(1), E::Command),
    act(338, "Dismount", &[Object], 1, W::None, E::Command),
    act(340, "Set AI value", &[Player, Int, Int], 3, W::None, E::World),
    act(341, "Hand over all objects", &[Player, Player], 2, W::None, E::World),
    act(342, "Invulnerable on", &[Player], 1, W::None, E::World),
    act(343, "Invulnerable off", &[Player], 1, W::None, E::World),
    act(344, "No effect", &[Int, Int, Text], 3, W::None, E::NoEffect),
    act(345, "No effect", &[Int, Int, Text], 3, W::None, E::NoEffect),
    act(346, "No effect", &[Int, Int, Text], 3, W::None, E::NoEffect),
    act(347, "Cover area", &[Player, Area], 2, W::None, E::World),
    act(350, "Increase hit points (no effect)", &[Object, Int], 2, W::None, E::NoEffect),
    act(351, "Increase hit points by percent (no effect)", &[Object, Int], 2, W::None, E::NoEffect),
    act(352, "Increase current hit points (no effect)", &[Object, Int], 2, W::None, E::NoEffect),
    act(353, "Refill hit points by percent", &[Object, Int], 2, W::None, E::World),
    act(354, "Refill hit points", &[Object], 1, W::None, E::World),
    act(359, "Variable from GUID count", &[Player, Variable, Guid], 3, W::None, E::Internal),
    act(360, "Set variable", &[Variable, Int], 2, W::None, E::Internal),
    act(361, "Increase variable", &[Variable, Int], 2, W::None, E::Internal),
    act(362, "Decrease variable", &[Variable, Int], 2, W::None, E::Internal),
    act(363, "Multiply variable", &[Variable, Int], 2, W::None, E::Internal),
    act(365, "Hide objects of a GUID again", &[Player, Guid], 2, W::None, E::World),
    act(366, "Show objects of a GUID", &[Player, Guid], 2, W::None, E::World),
    act(370, "Set combat music", &[Int], 1, W::None, E::Presentation),
    act(371, "Set combat music threshold", &[Int], 1, W::None, E::Presentation),
    act(372, "Cover area completely", &[Player, Area], 2, W::None, E::World),
    act(373, "Move objects without combat", &[Player, Guid, Area, Area], 4, W::None, E::Command),
    act(374, "Move unit without combat", &[Object, Area, Area], 3, W::EventBit(1), E::Command),
    act(375, "Move to object (variant)", &[Object, Object], 2, W::EventBit(1), E::Command),
    act(376, "Move unit fast without combat", &[Object, Area, Area], 3, W::EventBit(1), E::Command),
    act(380, "Hire headhunter", &[Player, Int, Player, Guid], 4, W::None, E::Command),
    act(381, "Reset object light to level value", &[], 0, W::None, E::Presentation),
    act(382, "Write variable to profile", &[Variable], 1, W::None, E::Presentation),
    act(383, "Read variable from profile", &[Variable], 1, W::None, E::Presentation),
    act(384, "Set attribute", &[Int, Object, Int], 3, W::None, E::World),
    act(385, "Set attribute maximum", &[Int, Object, Int], 3, W::None, E::World),
    act(390, "Turn toward object", &[Object, Object], 2, W::None, E::Command),
    act(391, "Collision off", &[Object], 1, W::None, E::World),
    act(392, "Collision on", &[Object], 1, W::None, E::World),
    act(393, "Add to group", &[Object, Object], 2, W::None, E::World),
    act(394, "Remove from group", &[Object, Object], 2, W::None, E::World),
    act(395, "Delete name or group", &[Object], 1, W::None, E::World),
    act(396, "Create units in area with group", &[Player, Int, Guid, Area, Object], 5, W::None, E::World),
    act(397, "Area objects into group", &[Player, Area, Object], 3, W::None, E::World),
    act(400, "Empty case (unreachable)", &[], 0, W::None, E::Unreachable),
    act(401, "Empty case (unreachable)", &[], 0, W::None, E::Unreachable),
    act(402, "Crew siege engine", &[Object, Object], 2, W::EventBit(1), E::Command),
    act(403, "No effect (waits)", &[Object, Object], 2, W::EventBit(1), E::NoEffect),
    act(406, "Search name in list (unreachable)", &[], 0, W::None, E::Unreachable),
    act(407, "Read list field (unreachable)", &[], 0, W::None, E::Unreachable),
    act(501, "Fade fog in", &[], 0, W::None, E::Presentation),
    act(502, "Fade fog out", &[], 0, W::None, E::Presentation),
    act(503, "Fog of war on", &[], 0, W::None, E::Presentation),
    act(504, "Fog of war off", &[], 0, W::None, E::Presentation),
    act(505, "Set fog values", &[Int, Int, Int], 3, W::None, E::Presentation),
    act(506, "Crossfade fog values", &[Int, Int, Int, Int], 4, W::None, E::Presentation),
    act(507, "Play sound effect", &[Int], 1, W::None, E::Presentation),
    act(508, "Quicksave", &[], 0, W::None, E::Presentation),
    // A.8 says 1; with a variable present, the original accesses `p1`: 2.
    act(510, "Increase variable by variable", &[Variable, Variable], 2, W::None, E::Internal),
    act(511, "Decrease variable by variable", &[Variable, Variable], 2, W::None, E::Internal),
    act(512, "Multiply variable by variable", &[Variable, Variable], 2, W::None, E::Internal),
];

/// Conditions with a case (D.1); the remaining numbers (15, 34, 39, 42-44, 48, 49) are always false.
#[rustfmt::skip]
const CONDITION_ROWS: &[ConditionSpec] = &[
    cond(1, "Always", &[], None, CondEffect::Script),
    cond(2, "Resource at least", &[Player, Int, Int], Some(100), CondEffect::Query),
    cond(3, "At least n of a GUID in area", &[Player, Int, Guid, Area], Some(200), CondEffect::Query),
    cond(4, "Countdown at most", &[Int], None, CondEffect::Script),
    cond(5, "Switch set", &[Switch], None, CondEffect::Script),
    cond(6, "Area sighted", &[Player, Area], Some(300), CondEffect::Query),
    cond(7, "Fewer than n of a GUID", &[Player, Int, Guid], Some(400), CondEffect::Query),
    cond(8, "More than n of a GUID", &[Player, Int, Guid], Some(400), CondEffect::Query),
    cond(9, "Fewer than n of a GUID (variant)", &[Player, Int, Guid], Some(400), CondEffect::Query),
    cond(10, "More than n of a GUID (variant)", &[Player, Int, Guid], Some(400), CondEffect::Query),
    cond(11, "Exactly n of a GUID in area", &[Player, Int, Guid, Area], Some(200), CondEffect::Query),
    cond(12, "Resource less than", &[Player, Int, Int], Some(100), CondEffect::Query),
    cond(13, "More than n of a GUID in area", &[Player, Int, Guid, Area], Some(200), CondEffect::Query),
    cond(14, "Fewer than n of a GUID in area", &[Player, Int, Guid, Area], Some(200), CondEffect::Query),
    cond(16, "Timer expired", &[Timer], None, CondEffect::Script),
    cond(17, "Random switch equals", &[RandomSwitch, Int], None, CondEffect::Script),
    cond(18, "Switch not set", &[Switch], None, CondEffect::Script),
    cond(19, "Text ended", &[TextId], Some(600), CondEffect::Query),
    cond(20, "Object event occurred", &[Object, Int], Some(700), CondEffect::Query),
    cond(21, "Name one character long", &[Object], Some(800), CondEffect::Query),
    cond(22, "Difficulty equals", &[Int], Some(900), CondEffect::Query),
    cond(23, "Object alive", &[Object], Some(1000), CondEffect::Query),
    cond(24, "Object not alive", &[Object], Some(1100), CondEffect::Query),
    cond(25, "Kills more than n", &[Player, Guid, Int], Some(1200), CondEffect::Query),
    cond(26, "Kills fewer than n", &[Player, Guid, Int], Some(1200), CondEffect::Query),
    cond(27, "Object in area", &[Object, Area], Some(1300), CondEffect::Query),
    cond(28, "Hit points below percent", &[Object, Int], Some(1400), CondEffect::Query),
    cond(29, "AI global equals", &[Player, Int, Int], Some(1500), CondEffect::Query),
    cond(30, "Hit points at least percent", &[Object, Int], Some(1400), CondEffect::Query),
    cond(31, "Always false (distance)", &[Object, Object], Some(2000), CondEffect::Query),
    cond(32, "Object sees at least n of a GUID", &[Object, Int, Guid], Some(2100), CondEffect::Query),
    cond(33, "Player sees object", &[Player, Object], Some(2200), CondEffect::Query),
    cond(35, "Objects selected", &[Object], Some(1600), CondEffect::Ui),
    cond(36, "Objects not selected", &[Object], Some(1700), CondEffect::Ui),
    cond(37, "Selected object with GUID", &[Player, Guid], Some(1800), CondEffect::Ui),
    cond(38, "Last pressed button", &[Player, Int], Some(1900), CondEffect::Ui),
    cond(40, "GUID count at least n", &[Player, Guid, Int], Some(2300), CondEffect::Query),
    cond(41, "GUID count above 0", &[Player, Guid], Some(2400), CondEffect::Query),
    cond(45, "Variable equals", &[Variable, Int], None, CondEffect::Script),
    cond(46, "Variable less than", &[Variable, Int], None, CondEffect::Script),
    cond(47, "Variable greater than", &[Variable, Int], None, CondEffect::Script),
    cond(50, "Count less than variable", &[Player, Variable, Guid], Some(400), CondEffect::Query),
    cond(51, "Count greater than variable", &[Player, Variable, Guid], Some(400), CondEffect::Query),
    cond(52, "Count equals variable", &[Player, Variable, Guid], Some(400), CondEffect::Query),
    cond(53, "Count in area greater than variable", &[Player, Variable, Guid, Area], Some(200), CondEffect::Query),
    cond(54, "Count in area less than variable", &[Player, Variable, Guid, Area], Some(200), CondEffect::Query),
    cond(55, "Count in area equals variable", &[Player, Variable, Guid, Area], Some(200), CondEffect::Query),
    cond(56, "Object sees object at angle", &[Object, Guid, Player, Int], Some(2600), CondEffect::Query),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abl::{ScriptFile, parse};
    use std::collections::BTreeSet;

    fn script(text: &str) -> ScriptFile {
        parse(&crate::text::encode_cp1252(text)).runtime().unwrap()
    }

    /// A block from line 1: `{`, `[BESCHREIBUNG=b]`, then the lines from line 3.
    fn block(lines: &[&str]) -> ScriptFile {
        let mut text = String::from("{\r\n\t[BESCHREIBUNG=b]\r\n");
        for line in lines {
            text.push('\t');
            text.push_str(line);
            text.push_str("\r\n");
        }
        text.push_str("}\r\n");
        script(&text)
    }

    fn set(ranges: &[(u16, u16)]) -> BTreeSet<u16> {
        ranges.iter().flat_map(|&(a, b)| a..=b).collect()
    }

    fn numbers(list: &[u16]) -> BTreeSet<u16> {
        list.iter().copied().collect()
    }

    fn actions_with(effect: Effect) -> BTreeSet<u16> {
        ACTIONS
            .iter()
            .filter(|a| a.effect == effect)
            .map(|a| a.number)
            .collect()
    }

    /// Case `0x72` (re/script-actions A.3): numbers without a case in layer E.
    fn case_0x72() -> BTreeSet<u16> {
        set(&[
            (41, 136),
            (206, 206),
            (218, 219),
            (224, 229),
            (234, 239),
            (244, 332),
            (339, 339),
            (348, 349),
            (355, 358),
            (364, 364),
            (367, 369),
            (377, 379),
            (386, 389),
            (398, 401),
            (404, 500),
            (509, 509),
        ])
    }

    const UNREACHABLE: [u16; 5] = [206, 400, 401, 406, 407];

    #[test]
    fn catalog_rows_are_complete_and_ordered() {
        assert_eq!(ACTIONS.len(), 513);
        assert_eq!(CONDITIONS.len(), 56);
        for (i, a) in ACTIONS.iter().enumerate() {
            assert_eq!(usize::from(a.number), i);
            assert!(usize::from(a.min_params) <= a.params.len(), "action {i}");
            assert_eq!(a.name.is_empty(), a.effect == Effect::NoCase, "action {i}");
            assert_eq!(action(i as i32), Some(a));
        }
        for (i, c) in CONDITIONS.iter().enumerate() {
            assert_eq!(usize::from(c.number), i + 1);
            assert!(
                usize::from(c.min_params) <= c.params.len(),
                "condition {}",
                i + 1
            );
            assert_eq!(c.name.is_empty(), c.effect == CondEffect::NoCase);
            assert_eq!(condition(i as i32 + 1), Some(c));
        }
        assert_eq!(action(-1), None);
        assert_eq!(action(513), None);
        assert_eq!(condition(0), None);
        assert_eq!(condition(57), None);
        // Conditions without a case (re/script-conditions D.1): false, without parameters, without query.
        let no_case: BTreeSet<u8> = CONDITIONS
            .iter()
            .filter(|c| c.effect == CondEffect::NoCase)
            .map(|c| c.number)
            .collect();
        assert_eq!(no_case, [15, 34, 39, 42, 43, 44, 48, 49].into());
        for c in CONDITIONS.iter().filter(|c| c.effect == CondEffect::NoCase) {
            assert_eq!((c.min_params, c.query), (0, None));
        }
        // Query code exactly for the conditions via `FUN_00425F90`.
        let without_query: BTreeSet<u8> = CONDITIONS
            .iter()
            .filter(|c| c.effect != CondEffect::NoCase && c.query.is_none())
            .map(|c| c.number)
            .collect();
        assert_eq!(without_query, [1, 4, 5, 16, 17, 18, 45, 46, 47].into());
        for c in &CONDITIONS {
            assert_eq!(
                c.effect == CondEffect::Script,
                without_query.contains(&c.number)
            );
        }
        let ui: BTreeSet<u8> = CONDITIONS
            .iter()
            .filter(|c| c.effect == CondEffect::Ui)
            .map(|c| c.number)
            .collect();
        assert_eq!(ui, [35, 36, 37, 38].into());
        assert_eq!(condition(19).unwrap().query, Some(600));
        assert_eq!(condition(56).unwrap().query, Some(2600));
    }

    // QUIRK E-133: kept
    #[test]
    fn effects_follow_appendix_a() {
        let internal = numbers(&[
            10, 11, 12, 23, 25, 26, 33, 39, 40, 137, 138, 139, 140, 180, 230, 232, 233, 333, 359,
            360, 361, 362, 363, 510, 511, 512,
        ]);
        let no_effect = numbers(&[17, 344, 345, 346, 350, 351, 352, 403]);
        let unreachable = numbers(&UNREACHABLE);
        let world = numbers(&[
            3, 6, 14, 15, 16, 20, 21, 22, 24, 27, 28, 29, 30, 31, 32, 35, 36, 151, 159, 160, 161,
            162, 173, 182, 184, 185, 186, 187, 192, 194, 195, 196, 197, 198, 199, 200, 201, 202,
            203, 204, 205, 207, 208, 211, 214, 215, 217, 231, 341, 342, 343, 347, 353, 354, 365,
            366, 372, 384, 385, 391, 392, 393, 394, 395, 396, 397, 0, 1, 37, 340, 174,
        ]);
        let command = numbers(&[
            4, 13, 8, 34, 38, 153, 154, 157, 158, 163, 164, 165, 166, 167, 179, 181, 188, 189, 190,
            191, 193, 334, 335, 336, 337, 338, 373, 374, 375, 376, 380, 390, 402,
        ]);
        let presentation = numbers(&[
            2, 5, 7, 9, 18, 19, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 152, 155, 156,
            168, 169, 170, 171, 172, 175, 176, 177, 178, 183, 209, 210, 212, 213, 216, 220, 221,
            222, 223, 240, 241, 242, 243, 370, 371, 381, 382, 383, 501, 502, 503, 504, 505, 506,
            507, 508,
        ]);
        assert_eq!(actions_with(Effect::Internal), internal);
        assert_eq!(actions_with(Effect::NoEffect), no_effect);
        assert_eq!(actions_with(Effect::Unreachable), unreachable);
        assert_eq!(actions_with(Effect::World), world);
        assert_eq!(actions_with(Effect::Command), command);
        assert_eq!(actions_with(Effect::Presentation), presentation);
        // Without a case: exactly case `0x72` without the five unreachable D cases (they are in
        // case `0x72`, but are listed separately as unreachable).
        let no_case: BTreeSet<u16> = case_0x72().difference(&unreachable).copied().collect();
        assert_eq!(actions_with(Effect::NoCase), no_case);
        for a in ACTIONS.iter().filter(|a| a.effect == Effect::NoCase) {
            assert_eq!(
                (a.min_params, a.params.len(), a.wait),
                (0, 0, WaitKind::None)
            );
        }
        // A.10 "without D call": the listed numbers and all of case `0x72`.
        let without_dispatch = numbers(&[
            10, 11, 12, 23, 25, 26, 33, 40, 137, 138, 139, 140, 180, 230, 232, 233, 333, 359, 360,
            361, 362, 363, 510, 511, 512,
        ]);
        for n in without_dispatch.union(&case_0x72()) {
            let effect = ACTIONS[usize::from(*n)].effect;
            let allowed = matches!(effect, Effect::Internal | Effect::NoCase)
                || (UNREACHABLE.contains(n) && effect == Effect::Unreachable);
            assert!(allowed, "action {n}: {effect:?}");
        }
    }

    #[test]
    fn wait_kinds_follow_a4() {
        let expected = |n: u16| match n {
            40 | 142 | 145 | 146 | 149 | 156 | 172 | 212 | 216 | 240 | 241 | 242 => WaitKind::Time,
            150 | 183 | 213 => WaitKind::Text,
            153 | 181 | 188 | 334 | 335 | 336 | 337 | 374 | 375 | 376 | 402 | 403 => {
                WaitKind::EventBit(1)
            }
            163 | 164 => WaitKind::EventBit(0),
            39 => WaitKind::Dialog,
            _ => WaitKind::None,
        };
        for a in &ACTIONS {
            assert_eq!(a.wait, expected(a.number), "action {}", a.number);
        }
        for n in [167, 220, 221, 222, 223, 243] {
            assert_eq!(ACTIONS[n].wait, WaitKind::None);
        }
    }

    // QUIRK E-127: deviation
    #[test]
    fn min_params_samples() {
        let min = |n: i32| action(n).unwrap().min_params;
        assert_eq!(min(0), 0);
        assert_eq!(min(3), 4);
        assert_eq!(min(39), 1);
        assert_eq!(min(40), 1);
        assert_eq!(min(151), 4);
        assert_eq!(min(217), 5);
        assert_eq!(min(240), 4);
        assert_eq!(min(396), 5);
        assert_eq!(min(510), 2);
        assert_eq!(min(511), 2);
        assert_eq!(min(512), 2);
        let cmin = |n: i32| condition(n).unwrap().min_params;
        assert_eq!(cmin(1), 0);
        assert_eq!(cmin(4), 1);
        assert_eq!(cmin(27), 2);
        assert_eq!(cmin(45), 2);
        assert_eq!(cmin(46), 2);
        assert_eq!(cmin(47), 2);
    }

    /// Area parameters = the `M` positions of the message in re/script-actions A.8 (blind `Mid(5)`)
    /// and the area parameters in re/script-conditions D.1.
    #[test]
    fn area_params_follow_a8_mid5() {
        let areas: BTreeSet<(u16, usize)> = ACTIONS
            .iter()
            .flat_map(|a| {
                a.params
                    .iter()
                    .enumerate()
                    .filter(|(_, k)| **k == ParamKind::Area)
                    .map(move |(i, _)| (a.number, i))
            })
            .collect();
        let expected: BTreeSet<(u16, usize)> = [
            (2, 1),
            (3, 3),
            (6, 2),
            (7, 1),
            (8, 2),
            (8, 3),
            (20, 1),
            (24, 1),
            (34, 2),
            (35, 1),
            (151, 3),
            (153, 1),
            (153, 2),
            (154, 1),
            (163, 1),
            (164, 1),
            (174, 0),
            (188, 1),
            (188, 2),
            (195, 2),
            (196, 2),
            (197, 1),
            (204, 2),
            (211, 2),
            (214, 2),
            (217, 3),
            (347, 1),
            (372, 1),
            (373, 2),
            (373, 3),
            (374, 1),
            (374, 2),
            (376, 1),
            (376, 2),
            (396, 3),
            (397, 1),
        ]
        .into();
        assert_eq!(areas, expected);
        let condition_areas: BTreeSet<(u8, usize)> = CONDITIONS
            .iter()
            .flat_map(|c| {
                c.params
                    .iter()
                    .enumerate()
                    .filter(|(_, k)| **k == ParamKind::Area)
                    .map(move |(i, _)| (c.number, i))
            })
            .collect();
        let expected: BTreeSet<(u8, usize)> = [
            (3, 3),
            (6, 1),
            (11, 3),
            (13, 3),
            (14, 3),
            (27, 1),
            (53, 3),
            (54, 3),
            (55, 3),
        ]
        .into();
        assert_eq!(condition_areas, expected);
    }

    // QUIRK E-127, E-174: deviation
    #[test]
    fn check_rejects_in_german() {
        let path = "data/levels/test/test.abl";
        let reject = |file: &ScriptFile, dialog: bool| {
            let err = check(file, path, dialog).unwrap_err();
            let text = err.in_script(path);
            assert!(text.starts_with(&format!("Script {path}, line {}: ", err.line)));
            assert_eq!(text.matches("line").count(), 1, "{text}");
            text
        };
        let file = block(&["[AKTION=n1;2;3:x]", "[AKTION=n151;2;1000:Set object]"]);
        assert_eq!(
            reject(&file, false),
            format!("Script {path}, line 4: action 151 needs at least 4 parameters (has 2)")
        );
        let file = block(&["[BEDINGUNG=&27;Drake:in area]", "[AKTION=n10:x]"]);
        assert_eq!(
            reject(&file, false),
            format!("Script {path}, line 3: condition 27 needs at least 2 parameters (has 1)")
        );
        let file = block(&["[AKTION=n7;Player 2;Gate:Signal]"]);
        assert_eq!(
            reject(&file, false),
            format!("Script {path}, line 3: area parameter \"gate\" without \"area \"")
        );
        let file = block(&["[BEDINGUNG=&27;drake;Gate:x]"]);
        assert_eq!(
            reject(&file, false),
            format!("Script {path}, line 3: area parameter \"gate\" without \"area \"")
        );
        let file = block(&["[AKTION=n21;9;100;0:Gold]"]);
        assert_eq!(
            reject(&file, false),
            format!("Script {path}, line 3: action 21 with invalid slot 9")
        );
        let file = block(&["[AKTION=n22;Player -1;100;0:Gold]"]);
        assert_eq!(
            reject(&file, false),
            format!("Script {path}, line 3: action 22 with invalid slot -1")
        );
        let file = block(&["[AKTION=n21:without slot]"]);
        assert_eq!(
            reject(&file, false),
            format!("Script {path}, line 3: action 21 with invalid slot (missing)")
        );
        let file = block(&["[AKTION=n39;Dialog Intro:Dialog]"]);
        assert_eq!(
            reject(&file, true),
            format!("Script {path}, line 3: dialog script starts a dialog (action 39)")
        );
        assert_eq!(check(&file, path, false), Ok(()));
        // The first violation in file order wins, even across conditions and blocks.
        let text = "{\r\n\t[BESCHREIBUNG=a]\r\n\t[AKTION=n10:x]\r\n\t[BEDINGUNG=&5:x]\r\n\t[AKTION=n3;1:x]\r\n}\r\n{\r\n\t[BESCHREIBUNG=b]\r\n\t[AKTION=n2;1:x]\r\n}\r\n";
        let err = check(&script(text), path, false).unwrap_err();
        assert_eq!(err.line, 4);
        assert_eq!(
            err.message,
            "condition 5 needs at least 1 parameters (has 0)"
        );
        // Slots 0-7 and `area ` are valid.
        let file = block(&[
            "[AKTION=n21;Player 0;100;0:x]",
            "[AKTION=n22;Player 7;100;0:x]",
            "[AKTION=n7;Player 2;Area Gate:x]",
            "[AKTION=n510;a;b:x]",
        ]);
        assert_eq!(check(&file, path, true), Ok(()));
    }

    // QUIRK E-193: deviation
    #[test]
    fn check_rejects_condition_2_and_12_with_invalid_player() {
        // E-193: a player outside 0-7 with resource kind 0-4 reads through a null pointer in the original.
        let path = "data/levels/test/test.abl";
        for (number, player, line) in [(2, "8", 3), (12, "-1", 3), (2, "99", 3)] {
            let file = block(&[&format!("[BEDINGUNG=&{number};Player {player};5;0:Gold]")]);
            let err = check(&file, path, false).unwrap_err();
            assert_eq!(err.line, line);
            assert_eq!(
                err.in_script(path),
                format!(
                    "Script {path}, line {line}: condition {number} with invalid player {player}"
                )
            );
        }
        // Kind 4 (living space) counts as well; a kind outside 0-4 yields 0 without access and stays valid.
        let file = block(&["[BEDINGUNG=&2;Player 9;5;4:Living space]"]);
        assert!(check(&file, path, false).is_err());
        let file = block(&[
            "[BEDINGUNG=&2;Player 9;5;5:Kind outside]",
            "[BEDINGUNG=&12;Player 0;5;0:Gold]",
            "[BEDINGUNG=&2;Player 7;5;4:Living space]",
        ]);
        assert_eq!(check(&file, path, false), Ok(()));
    }

    // QUIRK E-191: deviation
    #[test]
    fn check_rejects_condition_33_with_negative_player() {
        // E-191: player < 0 reads the local player (host state); >= 8 yields 0 and stays valid.
        let path = "data/levels/test/test.abl";
        let file = block(&["[BEDINGUNG=&33;Player -1;hero:visible]"]);
        assert_eq!(
            check(&file, path, false).unwrap_err().in_script(path),
            format!("Script {path}, line 3: condition 33 with invalid player -1")
        );
        let file = block(&[
            "[BEDINGUNG=&33;Player 0;hero:visible]",
            "[BEDINGUNG=&33;Player 9;hero:visible]",
        ]);
        assert_eq!(check(&file, path, false), Ok(()));
    }

    // QUIRK E-157: kept
    // QUIRK E-197: deviation
    #[test]
    fn check_rejects_202_197_35_with_slot_outside_0_to_10() {
        // E-197: the original does not check the target slot and indexes tables with 11 entries.
        let path = "data/levels/test/test.abl";
        for (text, number, shown) in [
            ("[AKTION=n35;Player 1000;Area Gate;Player 11:x]", 35, "11"),
            ("[AKTION=n197;Player 1000;Area Gate;Player -1:x]", 197, "-1"),
            ("[AKTION=n202;hero;Player 99:x]", 202, "99"),
        ] {
            let file = block(&[text]);
            let err = check(&file, path, false).unwrap_err();
            assert_eq!(err.line, 3);
            assert_eq!(
                err.in_script(path),
                format!("Script {path}, line 3: action {number} with invalid slot {shown}")
            );
        }
        // Slots 0-10 (including neutral 8, trees 9 and nature 10) are valid.
        let file = block(&[
            "[AKTION=n35;Player 1000;Area Gate;Player 0:x]",
            "[AKTION=n197;Player 1000;Area Gate;Player 8:x]",
            "[AKTION=n202;hero;Player 10:x]",
            "[AKTION=n202;hero;Player 9:x]",
        ]);
        assert_eq!(check(&file, path, false), Ok(()));
    }

    // QUIRK E-218: kept
    // QUIRK E-220, E-221: deviation
    #[test]
    fn check_rejects_37_and_380_bad_slots_and_340_negative_index() {
        // 37 writes behind the table in the original, 340 throws an exception for a negative index,
        // 380 reads behind the player table with a customer outside 0-7. Rejecting them is a deviation from the
        // original. The target player of 380 stays unchecked (E-218).
        let path = "data/levels/test/test.abl";
        for (text, message) in [
            ("[AKTION=n37;Player 8:x]", "action 37 with invalid slot 8"),
            ("[AKTION=n37;Player -1:x]", "action 37 with invalid slot -1"),
            (
                "[AKTION=n380;Player 8;0;Player 6;1715:x]",
                "action 380 with invalid slot 8",
            ),
            (
                "[AKTION=n340;Player 1;-1;5:x]",
                "action 340 with negative index -1",
            ),
            (
                "[AKTION=n340;Player 1;-2147483648;5:x]",
                "action 340 with negative index -2147483648",
            ),
        ] {
            let file = block(&[text]);
            let err = check(&file, path, false).unwrap_err();
            assert_eq!(err.line, 3, "{text}");
            assert_eq!(
                err.in_script(path),
                format!("Script {path}, line 3: {message}"),
                "{text}"
            );
        }
        // Valid: slots 0-7, headhunters of the shipped data (customer 3, target 6), index 0 and large values.
        let file = block(&[
            "[AKTION=n37;Player 0:x]",
            "[AKTION=n37;Player 7:x]",
            "[AKTION=n380;Player 3;0;Player 6;1715:x]",
            "[AKTION=n380;Player 0;2;Player 7;1715:x]",
            "[AKTION=n380;Player 3;0;Player 9;1715:x]",
            "[AKTION=n380;Player 3;0;Player -1;1715:x]",
            "[AKTION=n340;Player 1;0;-5:x]",
            "[AKTION=n340;Player 7;100000;5:x]",
        ]);
        assert_eq!(check(&file, path, false), Ok(()));
    }

    // QUIRK E-199: kept
    #[test]
    fn check_accepts_173_205_with_any_daytime() {
        // E-199: the time of day goes unchecked to `PlayerSetup_ApplyDaytimeUpgrades`; values outside
        // -1 to 3 continue as well.
        let path = "data/levels/test/test.abl";
        let file = block(&[
            "[AKTION=n173;Player 4:x]",
            "[AKTION=n173;Player -1:x]",
            "[AKTION=n173;Player 0:x]",
            "[AKTION=n205;Player -2:x]",
            "[AKTION=n205;Player 99:x]",
            "[AKTION=n205;Player 2147483647:x]",
            "[AKTION=n205;Player 3:x]",
            "[AKTION=n205:x]",
        ]);
        assert_eq!(check(&file, path, false), Ok(()));
    }

    // QUIRK E-155: deviation
    #[test]
    fn check_rejects_203_204_with_behavior_outside_0_to_4() {
        // E-155: the original writes every stance, but for a stance outside 0-4 it leaves words of the
        // messages `0x611`/`0x2100` unset (heap remnants, not deterministic, C-341).
        let path = "data/levels/test/test.abl";
        for (text, number, shown) in [
            ("[AKTION=n203;Player 1;Player 5:x]", 203, "5"),
            ("[AKTION=n203;Player 1;Player -1:x]", 203, "-1"),
            (
                "[AKTION=n204;Player 1;Player 1000;Area Gate;Player 7:x]",
                204,
                "7",
            ),
        ] {
            let file = block(&[text]);
            let err = check(&file, path, false).unwrap_err();
            assert_eq!(err.line, 3);
            assert_eq!(
                err.in_script(path),
                format!("Script {path}, line 3: action {number} with invalid stance {shown}")
            );
        }
        // 0-4 are valid; 203 without a stance reads 0.
        let file = block(&[
            "[AKTION=n203;Player 1;Player 0:x]",
            "[AKTION=n203;Player 8;Player 4:x]",
            "[AKTION=n203;Player 2:x]",
            "[AKTION=n204;Player 1;Player 1000;Area Gate;Player 0:x]",
            "[AKTION=n204;Player 1;Player 1000;Area Gate;Player 4:x]",
        ]);
        assert_eq!(check(&file, path, false), Ok(()));
    }

    // QUIRK E-153: kept
    #[test]
    fn check_rejects_159_160_211_214_with_slot_outside_0_to_10() {
        // E-153: the original does not check the slot and indexes tables with 11 entries.
        let path = "data/levels/test/test.abl";
        for (text, number, shown) in [
            ("[AKTION=n159;Player 11:x]", 159, "11"),
            ("[AKTION=n160;Player -1:x]", 160, "-1"),
            ("[AKTION=n211;Player 12;Player 1000;Area Gate:x]", 211, "12"),
            ("[AKTION=n214;Player -3;Player 1000;Area Gate:x]", 214, "-3"),
        ] {
            let file = block(&[text]);
            let err = check(&file, path, false).unwrap_err();
            assert_eq!(err.line, 3);
            assert_eq!(
                err.in_script(path),
                format!("Script {path}, line 3: action {number} with invalid slot {shown}")
            );
        }
        // Slots 0-10 are valid.
        let file = block(&[
            "[AKTION=n159;Player 0:x]",
            "[AKTION=n160;Player 10:x]",
            "[AKTION=n211;Player 0;Player 1000;Area Gate:x]",
            "[AKTION=n214;Player 10;Player 1000;Area Gate:x]",
        ]);
        assert_eq!(check(&file, path, false), Ok(()));
    }

    // QUIRK E-241: deviation
    #[test]
    fn check_rejects_150_negative_slot_and_213_below_minus_4() {
        // E-241: 150 with slot < 0 and 213 with pos < -4 or pos > i32::MAX - 4 (pos + 4 overflows)
        // write behind the text slots in the original; rejecting them is a deviation (E-241). The player of 7 stays
        // unchecked (E-244).
        let path = "data/levels/test/test.abl";
        for (text, message) in [
            ("[AKTION=n150;100;-1:x]", "action 150 with invalid slot -1"),
            (
                "[AKTION=n150;100;-2147483648:x]",
                "action 150 with invalid slot -2147483648",
            ),
            (
                "[AKTION=n213;100;-5;1;2:x]",
                "action 213 with invalid slot -5",
            ),
            // Pos + 4 overflows i32 and becomes negative (E-241).
            (
                "[AKTION=n213;100;2147483644;1;2:x]",
                "action 213 with invalid slot 2147483644",
            ),
        ] {
            let file = block(&[text]);
            let err = check(&file, path, false).unwrap_err();
            assert_eq!(err.line, 3, "{text}");
            assert_eq!(
                err.in_script(path),
                format!("Script {path}, line 3: {message}"),
                "{text}"
            );
        }
        // Valid: 150 from slot 0 (also large slots, only the UI shows < 4), 213 with pos -4 to
        // i32::MAX - 4, 7 with any player (E-244; shipped data: 1, 2, 3, 6, 7).
        let file = block(&[
            "[AKTION=n150;100;0:x]",
            "[AKTION=n150;100;1:x]",
            "[AKTION=n150;100;7:x]",
            "[AKTION=n213;100;-4;1;2:x]",
            "[AKTION=n213;100;0;1;2:x]",
            "[AKTION=n213;100;1;1;2:x]",
            "[AKTION=n213;100;2147483643;1;2:x]",
            "[AKTION=n7;Player 0;Area Gate:x]",
            "[AKTION=n7;Player 1;Area Gate:x]",
            "[AKTION=n7;Player 9;Area Gate:x]",
            "[AKTION=n7;Player 10;Area Gate:x]",
            "[AKTION=n7;Player -1;Area Gate:x]",
        ]);
        assert_eq!(check(&file, path, false), Ok(()));
    }

    #[test]
    fn unknown_numbers_pass_the_check() {
        let file = block(&[
            "[BEDINGUNG=&15:no case]",
            "[BEDINGUNG=&99:unknown]",
            "[AKTION=n41:no case]",
            "[AKTION=n206:unreachable]",
            "[AKTION=n999:unknown]",
            "[AKTION=n-5:negative]",
            "[AKTION=n17;1:ineffective]",
        ]);
        assert_eq!(check(&file, "x.abl", true), Ok(()));
    }
}
