//! Upgrade and arming commands (re/upgrades 2): the
//! console commands `0x39` Apply upgrade and `0x3A` Remove upgrade
//! (`GameWorld_ExecuteCommand` `0x0048D8B6…0x0048D904`) and `0x2F` Execute counter-upgrade
//! (`GameWorld_CmdStartCounterUpgrade` `0x004848F0`, re/temp-upgrades 3.5), `0x36` Cast spell
//! (`GameWorld_CmdCastSpell` `0x004774F0`) and `0x5E` Execute temporary upgrade
//! (`GameWorld_CmdExecTempUpgrade` `0x0047EE90`, re/temp-upgrades 2.3, 15), `0x38`/`0x72`
//! Arm/Disarm (`GameWorld_CmdToggleArming` `0x00475C10`) and `0x40`/`0x41` Fire arrows
//! (`GameWorld_CmdSetArrowType` `0x00475B90`, re/arming-gates 2.A, 2.B).

use opennml_formats::gifcon::button_kind;

use super::ability::{check_ability, ki_path};
use super::economy::clear_queue;
use super::validate::{button_locked, menu_button};
use super::{CommandKind, owns, reject, script_path, skip_passed};
use crate::id::{ObjectId, SLOT_COUNT};
use crate::message::Message;
use crate::messaging::post_message;
use crate::module::{arming, exec_temp, fight_far, spell};
use crate::placement::pose_on_map;
use crate::sim::Sim;
use crate::upgrades::{apply_upgrade, counter, remove_upgrade};

/// First and last GUID of `0x39` with GUID < 0 (`EBX = 0xAF0 … < 0xD48`).
pub const APPLY_ALL_FIRST: u32 = 0xAF0;
pub const APPLY_ALL_LAST: u32 = 0xD47;

/// Ability buttons with command `0x36` kind 4 as `(button, anim, idx)` (`Toolbar_DispatchButtonCommand`
/// `0x0044C040`, re/temp-upgrades 15 no. 8): the UI sends fixed values per button.
pub const ABILITY_BUTTONS: [(i32, i32, i32); 10] = [
    (543, 2, 1),
    (544, 0, 2),
    (545, 1, 0),
    (549, 0, 0),
    (552, 0, 0),
    (557, 0, 0),
    (559, 0, 1),
    (560, 0, 0),
    (561, 0, 0),
    (563, 0, 0),
];

/// Entry point for the commands of this group (`command::execute` routes them here).
///
/// `0x39`/`0x3A` check nothing in the original (console, script): no ownership check, also for foreign
/// slots. Deviation (exception A7): `0x39` with a slot outside 0...10 would write foreign memory and
/// is rejected. `0x3A` outside 0...7 stays without effect as in the original. The script action 38
/// (`script::command::upgrade`) uses `0x39` unchanged via the script path: sent twice
/// it takes effect twice.
///
/// `0x2F` first runs through the command check [`check_counter_upgrade`] (A8), then through the start
/// of the original ([`counter::start`], with its own locks and the cost check).
///
/// `0x36`/`0x5E` run through [`check_cast_spell`] and [`check_exec_temp_upgrade`] (A7/A8);
/// `0x36` takes kinds 1/2/3 (command bar check via
/// [`check_ability`](super::ability::check_ability)) and 4, kind 5 only via `0x4E`.
///
/// `0x38`/`0x72` run through [`check_toggle_arming`] (A8, deviation from W-264), `0x40`/`0x41` are always
/// rejected by the check for players (A8, C-224: no command bar produces them).
pub fn execute(sim: &mut Sim, player: u8, kind: &CommandKind, direct: bool) {
    match *kind {
        CommandKind::ApplyUpgrade { slot, guid } => {
            if !(0..SLOT_COUNT as i32).contains(&slot) {
                reject(
                    sim,
                    player,
                    "Apply upgrade",
                    format!("slot {slot} (allowed 0 to 10)"),
                );
                return;
            }
            if let Ok(guid) = u32::try_from(guid) {
                apply_upgrade(sim, slot, guid, false);
            } else {
                for guid in APPLY_ALL_FIRST..=APPLY_ALL_LAST {
                    apply_upgrade(sim, slot, guid, false);
                }
            }
        }
        CommandKind::RemoveUpgrade { slot, guid } => remove_upgrade(sim, slot, guid),
        CommandKind::CounterUpgrade { guid, source } => {
            if let Err(why) = check_counter_upgrade(sim, player, guid, source) {
                reject(sim, player, "Execute counter-upgrade", why);
                return;
            }
            counter::start(sim, guid, source);
        }
        CommandKind::CastSpell {
            caster,
            art,
            anim,
            idx,
            button,
        } => {
            if let Err(why) = check_cast_spell(sim, player, caster, art, anim, idx, button) {
                reject(sim, player, "Cast spell", why);
                return;
            }
            // `GameWorld_CmdCastSpell`: post with abort, then with `direct` empty the
            // queue.
            let msg = Message::new(spell::msg::CAST, &[art, anim, idx, button]);
            post_message(sim, caster, msg, true);
            if direct {
                clear_queue(sim, caster);
            }
        }
        CommandKind::ExecTempUpgrade {
            object,
            button,
            idx,
        } => {
            if let Err(why) = check_exec_temp_upgrade(sim, player, object, button, idx) {
                reject(sim, player, "Execute temporary upgrade", why);
                return;
            }
            exec_temp_upgrade(sim, player, object, button, idx);
        }
        CommandKind::ToggleArming { ref units, arm } => {
            let what = if arm { "Arm" } else { "Disarm" };
            match check_toggle_arming(sim, player, units, arm) {
                Ok(units) => toggle_arming(sim, &units, arm, direct),
                Err(why) => reject(sim, player, what, why),
            }
        }
        CommandKind::SetArrowType { fire, .. } => {
            let what = if fire {
                "Fire arrows on"
            } else {
                "Fire arrows off"
            };
            reject(
                sim,
                player,
                what,
                "fire arrow commands exist only for scripts (A8)",
            );
        }
        _ => {}
    }
}

/// Command check of `0x2F` (A8; deviation from W-231): the original checks neither ownership nor menu
/// nor prerequisite, the UI shows the button only that way. Required are:
/// - the source is an own, selectable unit (not quartered, not dying);
/// - a button `msg == guid` of kind `COUNTERUPGRADE` in the menu of the source type including submenus
///   ([`menu_buttons`](super::validate::menu_buttons), missionary 1113 -> 601);
/// - the two locks of the counter-upgrades (`Toolbar_IsButtonLocked` `0x0044AF60`, re/upgrades 2)
///   with their own message: the GUID runs nowhere ([`counter::is_running`]), the sender has none
///   running ([`counter::count_of_slot`]) - after that [`button_locked`] locks only via
///   `REQUIRED_GUID` (research 335x);
/// - the GUID is not locked on the map (`Player_IsGuidDisabled`, level data).
pub fn check_counter_upgrade(
    sim: &Sim,
    player: u8,
    guid: u32,
    source: ObjectId,
) -> Result<(), String> {
    if !owns(sim, player, source) {
        return Err(format!(
            "source {} is not an own, selectable unit",
            source.0
        ));
    }
    let menu = sim.object(source).map_or(0, |o| o.guid);
    let button = menu_button(sim.rules, menu, |b| {
        b.msg == guid as i32 && b.kind() == button_kind::COUNTERUPGRADE
    })
    .ok_or_else(|| format!("counter-upgrade {guid} is not in the menu of {menu}"))?;
    if counter::is_running(sim.state, guid) {
        return Err(format!("counter-upgrade {guid} is already running"));
    }
    if counter::count_of_slot(sim.state, player) != 0 {
        return Err(format!(
            "slot {player} already has a running counter-upgrade"
        ));
    }
    let slot = i32::from(player);
    if button_locked(sim, slot, button) {
        return Err(format!("prerequisite for {guid} missing"));
    }
    if crate::world::is_guid_disabled(sim, slot, guid) {
        return Err(format!("{guid} is locked on this map"));
    }
    Ok(())
}

/// Button "Arm" of the worker command bars (`TARGET_DO_MSG`, `REQUIRED_GUID` = arming upgrade of the
/// nation, re/arming-gates 2.A).
pub const ARM_BUTTON: i32 = 485;

/// Command check of `0x38`/`0x72` (A8; deviation from W-264): the
/// original checks neither ownership nor upgrade, the command bar shows 485 only with the arming upgrade.
/// What remains are the own, selectable units with module 39 for which
/// - when arming (`arm`) the button 485 is in the menu of their type including submenus and is not
///   locked ([`button_locked`], `REQUIRED_GUID`),
/// - when disarming FightFar (0x13) is switched on.
///
/// Units that do not fit drop out; if none remains, the command is rejected. On the script path
/// ([`script_path`]) "present" is enough (module 39, button and lock are omitted).
pub fn check_toggle_arming(
    sim: &Sim,
    player: u8,
    units: &[ObjectId],
    arm: bool,
) -> Result<Vec<ObjectId>, String> {
    let what = if arm { "Arm" } else { "Disarm" };
    let fits = |u: ObjectId| {
        if !owns(sim, player, u) {
            return false;
        }
        // Script path (189 code 7, E-214): only "present"; module 39, button 485 and lock are omitted.
        // The ranged-combat condition is checked by [`toggle_arming`] like the original (`0x00475C10`).
        if script_path(sim) {
            return true;
        }
        let Some(o) = sim.object(u) else {
            skip_passed(sim, player, what, format!("unit {} missing", u.0));
            return false;
        };
        if o.module(arming::INDEX).is_none() {
            skip_passed(
                sim,
                player,
                what,
                format!("unit {} has no arming module", u.0),
            );
            return false;
        }
        let ok = if arm {
            menu_button(sim.rules, o.guid, |b| b.msg == ARM_BUTTON)
                .is_some_and(|b| !button_locked(sim, i32::from(player), b))
        } else {
            o.has_module(fight_far::INDEX, false)
        };
        if !ok {
            skip_passed(
                sim,
                player,
                what,
                format!("unit {}: {what} not possible", u.0),
            );
        }
        ok
    };
    let ok: Vec<ObjectId> = units.iter().copied().filter(|&u| fits(u)).collect();
    if ok.is_empty() {
        let what = if arm { "Arm" } else { "Disarm" };
        return Err(format!("{what} not possible"));
    }
    Ok(ok)
}

/// `GameWorld_CmdToggleArming` (`0x00475C10`, re/arming-gates 3.A): per present unit with
/// `direct` empty the queue; only if `arm` != "FightFar switched on", post `0x127` with
/// abort (FIFO flag 5: `AbortAllModules` before the armory search).
fn toggle_arming(sim: &mut Sim, units: &[ObjectId], arm: bool, direct: bool) {
    for &u in units {
        let Some(o) = sim.object(u) else { continue };
        let ff = o.has_module(fight_far::INDEX, false);
        if direct {
            clear_queue(sim, u);
        }
        if arm != ff {
            let msg = Message::new(arming::msg::GO_TO_ARMORY, &[]);
            post_message(sim, u, msg, true);
        }
    }
}

/// Spell kinds of `0x36` with command bar button (re/spells 2.3, 2.5): kind 1 explosion <-> 480, kind 2 storm
/// <-> 481, kind 3 reincarnation <-> 482 (deviation from the original, A8).
pub const SPELL_ART_BUTTONS: [(i32, i32); 3] = [
    (spell::ART_EXPLOSION, 480),
    (spell::ART_STORM, 481),
    (spell::ART_REINCARNATE, 482),
];

/// Command check of `0x36` (A8, re/temp-upgrades 15 no. 8, re/spells 2.3): the original
/// checks only that the caster exists and the command has five words; the UI sends
/// fixed values per button (480/481 only for the first selected object with the world position of the
/// target, 482 and 543 ff. to every selected object). Required per kind:
/// - kinds 1/2/3: the button of the kind ([`SPELL_ART_BUTTONS`]), then the caster like the command bar
///   ([`check_ability`] with module 38: own, living, selectable, button in the menu and free, MY filter
///   `PROP_SPELL` with flag `0x100`/`0x200` at 480/481); for kind 1/2 `(anim, idx)` is the
///   target point as `f32` bits, finite and on the map ([`pose_on_map`]); for kind 3
///   `anim == idx == 0`. The target itself is not checked until the targeting mode
///   ([`target_command`](super::ability::target_command)).
/// - kind 4: an own, selectable caster with module 38
///   (mounted stays allowed: the cast of the original is then ineffective and the mana stays);
///   `(anim, idx)` as in [`ABILITY_BUTTONS`] for the button; the button in the menu of the
///   caster type including submenus and not locked ([`button_locked`], `REQUIRED_GUID`);
///   module 44 with `idx` in its execution list (otherwise a savegame would arise that loading
///   rejects).
/// - kind 5 only via `0x4E` (drain mana), all other kinds are rejected.
///
/// AI check path ([`ki_path`]): the button of the kind ([`SPELL_ART_BUTTONS`], E-95: the storm
/// `0x281` comes with 480) and for kind 4 the ability button including menu and lock
/// ([`check_ability_button`]) are omitted, as is the command bar check in [`check_ability`]. What remains are
/// ownership, death, module 38, the target point on the map (kind 1/2), `(0, 0)` for kind 3 and for kind 4 the
/// index in the execution list (A7: otherwise loading would reject the spell state).
pub fn check_cast_spell(
    sim: &mut Sim,
    player: u8,
    caster: ObjectId,
    art: i32,
    anim: i32,
    idx: i32,
    button: i32,
) -> Result<(), String> {
    if art == spell::ART_DRAIN {
        return Err("spell kind 5 only via drain mana (0x4E)".into());
    }
    if let Some(&(_, expected)) = SPELL_ART_BUTTONS.iter().find(|&&(a, _)| a == art) {
        if button != expected && !ki_path(sim) {
            return Err(format!(
                "button {button} does not match spell kind {art} ({expected})"
            ));
        }
        check_ability(sim, player, caster, Some(spell::INDEX), &[button])?;
        if art == spell::ART_REINCARNATE {
            if (anim, idx) != (0, 0) {
                return Err(format!(
                    "animation {anim} and index {idx} must be 0 (spell kind 3)"
                ));
            }
        } else {
            let (x, z) = (f32::from_bits(anim as u32), f32::from_bits(idx as u32));
            if !pose_on_map(&sim.rules.map, [x, 0.0, z], 0.0) {
                return Err(format!(
                    "target point ({x}, {z}) not finite or outside the map"
                ));
            }
        }
        return Ok(());
    }
    if art != spell::ART_EXEC_TEMP {
        return Err(format!("spell kind {art} unknown"));
    }
    if !owns(sim, player, caster) {
        return Err(format!(
            "caster {} is not an own, selectable unit",
            caster.0
        ));
    }
    let Some(o) = sim.object(caster) else {
        return Err(format!("caster {} missing", caster.0));
    };
    if o.module(spell::INDEX).is_none() {
        return Err(format!("type {} has no spell module (38)", o.guid));
    }
    if !ki_path(sim) {
        check_ability_button(sim, player, o.guid, button, Some(anim), idx)?;
    }
    check_exec_index(sim, caster, idx)
}

/// Command check of `0x5E` (A7/A8): an own, selectable object with module 44 and `idx` in
/// its execution list, plus the button like the UI ([`check_ability_button`]: an
/// ability button with this index, in the menu of the type, not locked). The original checks none
/// of this (`GameWorld_CmdExecTempUpgrade` `0x0047EE90`, only the AI produces the command); without
/// the button check a button without a value set would pay no mana and bypass `REQUIRED_GUID`. Players
/// are not rejected on principle so that AI commands take the same path.
///
/// AI check path ([`ki_path`], E-93): the AI sends button and index that do not match each other and
/// do not match the command bar (governor `{543, 0}`, shadow warrior `{549, 2}`, smith `{522, 0}`,
/// flag bearer `{557, 1}`). What remains are ownership and module 44; index and button are not checked by the AI - it
/// pays by the button ([`exec_temp_upgrade`]: 522 without a value set is free), an index outside
/// the list has no effect (`0x12C` finds no record), the mana is gone anyway.
pub fn check_exec_temp_upgrade(
    sim: &Sim,
    player: u8,
    object: ObjectId,
    button: i32,
    idx: i32,
) -> Result<(), String> {
    if !owns(sim, player, object) {
        return Err(format!(
            "object {} is not an own, selectable unit",
            object.0
        ));
    }
    if ki_path(sim) {
        return match exec_temp::exec_len(sim, object) {
            Some(_) => Ok(()),
            None => Err(format!("object {} has no execute module (44)", object.0)),
        };
    }
    check_exec_index(sim, object, idx)?;
    let guid = sim.object(object).map_or(0, |o| o.guid);
    check_ability_button(sim, player, guid, button, None, idx)
}

/// Ability button like the UI (`Toolbar_DispatchButtonCommand` `0x0044C040`,
/// `Toolbar_IsButtonLocked` `0x0044AF60`): `button` is in [`ABILITY_BUTTONS`] with this `idx`
/// (and, if given, this animation), in the menu of the type `menu` including submenus and is not
/// locked for the slot (`REQUIRED_GUID`). Shared by `0x36` and `0x5E`.
fn check_ability_button(
    sim: &Sim,
    player: u8,
    menu: u32,
    button: i32,
    anim: Option<i32>,
    idx: i32,
) -> Result<(), String> {
    let Some(&(_, a, i)) = ABILITY_BUTTONS.iter().find(|&&(b, _, _)| b == button) else {
        return Err(format!("button {button} is not an ability button"));
    };
    match anim {
        Some(anim) if (a, i) != (anim, idx) => {
            return Err(format!(
                "animation {anim} and index {idx} do not match button {button} ({a}, {i})"
            ));
        }
        None if i != idx => {
            return Err(format!(
                "index {idx} does not match button {button} (expected {i})"
            ));
        }
        _ => {}
    }
    let b = menu_button(sim.rules, menu, |b| b.msg == button)
        .ok_or_else(|| format!("button {button} is not in the menu of {menu}"))?;
    if button_locked(sim, i32::from(player), b) {
        return Err(format!("prerequisite for button {button} missing"));
    }
    Ok(())
}

/// Module 44 present and `idx` in its execution list.
fn check_exec_index(sim: &Sim, id: ObjectId, idx: i32) -> Result<(), String> {
    let Some(len) = exec_temp::exec_len(sim, id) else {
        return Err(format!("object {} has no execute module (44)", id.0));
    };
    if !usize::try_from(idx).is_ok_and(|i| i < len) {
        return Err(format!(
            "index {idx} outside the execution list ({len} entries)"
        ));
    }
    Ok(())
}

/// `GameWorld_CmdExecTempUpgrade` (`0x0047EE90`, re/temp-upgrades 2.3): cost like the cast from
/// config 15 of `button` in the value set of the owner; `current(15) < cost` -> nothing, otherwise
/// subtract (set missing: without cost), then post `{0x12C, [idx]}` with abort (the mana is
/// gone even if the filter of the object discards the message).
fn exec_temp_upgrade(sim: &mut Sim, player: u8, object: ObjectId, button: i32, idx: i32) {
    let Some(owner) = sim.object(object).map(|o| o.owner()) else {
        return;
    };
    if let Some(cost) = spell::button_cost(sim, owner, button) {
        if sim.attribute(object, spell::MANA) < cost {
            reject(
                sim,
                player,
                "Execute temporary upgrade",
                format!("too little mana (cost {cost})"),
            );
            return;
        }
        spell::pay_mana(sim, object, cost);
    }
    let msg = Message::new(exec_temp::msg::EXECUTE, &[idx]);
    post_message(sim, object, msg, true);
}
