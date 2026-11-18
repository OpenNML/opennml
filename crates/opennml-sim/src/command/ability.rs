//! Commands for camouflage, dig in and healing (re/masking 2.3, re/healing 2.3), for evoke ghost,
//! evoke animal and banish (re/ghosts-banish 2.5, re/evoke-aura 2.4) and for drain mana and scouting
//! (re/spells 2.3, 3.6; special attacks `0x30`-`0x33`, whirlwind `0x4F`, damage building `0x5F`;
//! re/special-attacks 2.4, re/spells 3.7; target commands of the command bar, [`target_command`]).
//! The original checks neither owner nor module nor button in the commands (re/masking 2.3,
//! re/healing 2.3). The reimplementation checks like the command bar (A8, re/abilities 5): an own,
//! living, selectable object with the ability module switched on whose type menu has the button, which
//! is not locked and whose MY filter from `targets.ini` the object satisfies
//! ([`check_ability`]); for target buttons additionally the target against the TARGET filter ([`target_fits`]).

use super::economy::clear_queue;
use super::validate::{button_locked, is_dead, menu_button};
use super::{CommandKind, Origin, owns, reject, script_path, skip, skip_passed};
use crate::combat::hit_zone::world_pos;
use crate::filter;
use crate::fog::{CLEAR_UP, EXPLORE, SEE, VisionSource, set_vision_source};
use crate::fx::Fx;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, post_message};
use crate::module::ghosts::{banish, evoke};
use crate::module::special::{bomberman, killer, special_move};
use crate::module::spell::{self, button_cost, pay_mana};
use crate::module::{assassin, evoke_animal, healing, masking, robbing};
use crate::placement::pose_on_map;
use crate::sim::Sim;
use crate::state::SimEvent;

/// Command bar button "Camouflage" (`BUTTONMSG 434`, active without `MASKED` and without `SWIMMING`).
pub const MASK_BUTTON: i32 = 434;
/// Command bar button "Uncamouflage" (`BUTTONMSG 435`, active with `MASKED`).
pub const UNMASK_BUTTON: i32 = 435;
/// Command bar button "Dig in" (`BUTTONMSG 437`, active without `WAITING4KILLING`).
pub const DIG_IN_BUTTON: i32 = 437;
/// Command bar button "Dig out" (`BUTTONMSG 438`, active with `WAITING4KILLING`).
pub const DIG_OUT_BUTTON: i32 = 438;
/// Command bar buttons "Start healing" (Pilgrim 554 behind `REQUIRED_GUID 3125`, Medic 458;
/// `Toolbar_DispatchButtonCommand` `0x0044C040`, re/healing 2.4); [`check_ability`] takes the first
/// matching one.
pub const START_HEAL_BUTTONS: [i32; 2] = [554, 458];
/// Command bar buttons "Stop healing" (564 and 459, active in the trance `HEAL`).
pub const STOP_HEAL_BUTTONS: [i32; 2] = [564, 459];
/// Command bar button "Self-healing" (555, `0x60`; in the shipped data only Trapper 2015, Sanders 2100 and
/// McBride 2102, all without module 29).
pub const SELF_HEAL_BUTTON: i32 = 555;
/// Command bar button "Evoke ghost" (`BUTTONMSG 436`, `TARGET_DO | GROUP_AND`, research 2930).
pub const EVOKE_GHOST_BUTTON: i32 = 436;
/// Command bar buttons "Evoke animal" by kind (0 wolf, 1 bear, 2 cat; `Toolbar_DispatchButtonCommand`
/// case `0x1B9`-`0x1BB`, re/evoke-aura 2.4).
pub const EVOKE_ANIMAL_BUTTONS: [i32; 3] = [441, 442, 443];
/// Command bar buttons "Banish" (evoker 1411/1502 547, Missionary 439, Pilgrim 553, among others;
/// `Toolbar_DispatchButtonCommand` -> `FUN_0044C020`, re/ghosts-banish 2.5).
pub const BANISH_BUTTONS: [i32; 6] = [439, 547, 550, 553, 558, 562];
/// Command bar button "Drain mana" (Missionary 1113, `REQUIRED_GUID 2832`, case `0x21D`; re/spells 2.3).
pub const DRAIN_BUTTON: i32 = 541;
/// Command bar buttons "Scout area" (Chief 1715 484, Trapper 2015 556; re/spells 2.3, 2.5).
pub const SCOUT_BUTTONS: [i32; 2] = [484, 556];
/// Size of the vision source that the UI sends when scouting (quarter tiles, re/spells 3.6).
pub const SCOUT_SIZE: i32 = 30;
/// Duration of the vision source when scouting in ms (re/spells 3.6).
pub const SCOUT_MS: i32 = 5000;
/// Flag "also clear up" of the UI when scouting (re/spells 3.6).
pub const SCOUT_FLAG: i32 = 1;
/// Button of `0x30` dynamite at building (`Input_IssueTargetAction` `0x0044D350` fixes it to
/// 454; re/special-attacks 2.4). The assignment of button to command is a deviation from the original (A8).
pub const DYNAMITE_BUILDING_BUTTON: i32 = 454;
/// Button of `0x31` dynamite at ship (fixed 455).
pub const DYNAMITE_SHIP_BUTTON: i32 = 455;
/// Buttons of `0x32` kill unit (near): 456 (Trapper among others), 548 (Shadow warrior); the
/// triggering button stays in the command.
pub const KILL_BUTTONS: [i32; 2] = [456, 548];
/// Buttons of `0x33` eliminate unit (far): 457 (Sharpshooter), 546 (Pistoleer).
pub const SNIPE_BUTTONS: [i32; 2] = [457, 546];
/// Button of `0x4F` Whirlwind (`DO_MSG`, Smith 2013).
pub const WHIRLWIND_BUTTON: i32 = 542;
/// Button of `0x5F` Damage building (`TARGET_MSG`, Smith 2013).
pub const DAMAGE_BUILDING_BUTTON: i32 = 551;
/// Button of `0x4C` Rob corpses (`DO_MSG`, `GROUP_AND_MSG`, `REQUIRED_GUID 3326`;
/// Undertaker 2615, Sanders 2702; re/headhunter-robbing 2.4).
pub const ROB_BUTTON: i32 = 540;
/// All target buttons (`TARGET_MSG`) with a target command in [`target_command`] (re/spells 2.5,
/// re/special-attacks 2.4).
pub const TARGET_BUTTONS: [i32; 12] = [454, 455, 456, 457, 480, 481, 484, 541, 546, 548, 551, 556];
/// Sound of `0x4C`, `0x4F` and `0x5F` with `direct`: `SoundManager_PlayObjectSound3D(0x1F, 0x42, …)`
/// with the button as GUID (re/special-attacks 2.4, re/spells 3.7, re/headhunter-robbing 3.7).
const ATTACK_SOUND: (i32, u32) = (0x1F, 0x42);
/// Success message of the drain (`GameObject_HandleOwnMessage` `0xB7F {source}`): mana of the source
/// -> hit points of the receiver (re/module-runtime 4).
const DRAIN_SUCCESS: u32 = 0xB7F;
/// Effect of scouting (`DAT_0061c3d0`, re/spells 3.6).
const SCOUT_EFFECT: &str = "data/effects/aufklären.pef";
/// Sound of scouting with `direct`: `SoundManager_PlayObjectSound3D(0x1F, 0x42, …)`.
const SCOUT_SOUND: (i32, u32) = (0x1F, 0x42);
/// Attribute hit points (config 14).
const HITPOINTS: u32 = 14;
/// Effect of self-healing (`GameWorld_CmdSelfHeal` `0x0047F020`, re/healing 2.3).
const SELF_HEAL_EFFECT: &str = "data/effects/selbst_heilen.pef";
/// Sound of self-healing with `direct`: `SoundManager_PlayObjectSound3D(0x1E, 0x22, …)`.
const SELF_HEAL_SOUND: (i32, u32) = (0x1E, 0x22);

/// AI check path of the ability commands (re/ki-queries 7.4): the command comes from
/// the AI VM ([`Origin::Ki`]). In the original the AI bypasses the command bar and sends buttons that do not
/// fit the command bar (E-93 ... E-95); therefore exactly the UI checks are dropped for it -
/// selectability, menu button, button lock and MY filter in [`check_ability`], the button-to-kind and
/// button-to-index tables ([`BANISH_BUTTONS`], [`KILL_BUTTONS`], [`SNIPE_BUTTONS`], [`SCOUT_BUTTONS`],
/// `ABILITY_BUTTONS`/`SPELL_ART_BUTTONS` in `upgrade.rs`) and the target filter ([`target_fits`]).
/// Ownership, death, module, cost (after the button sent) and value limits still apply.
pub(crate) fn ki_path(sim: &Sim) -> bool {
    sim.origin == Origin::Ki
}

/// Check helper of the ability commands (A8 like the command bar, re/abilities 5) for the object `unit` of
/// the slot `player`, in this order:
/// 1. own, not quartered, not dead (`owns`), no dying building ([`is_dead`]), selectable;
/// 2. with `Some(m)` the module `m` is present and switched on (`None` for `0x60`, self-healing
///    needs no module, re/healing 2.3);
/// 3. for each button of `buttons` in list order: it is in the menu of the type ([`menu_button`]), is
///    not locked (`Toolbar_IsButtonLocked`, [`button_locked`]) and its MY filter from
///    `targets.ini` fits (like `GameWorld_RebuildTargetActions`: first entry with this
///    `BUTTONMSG`, switched-off entries do not count, player -1 = owner; no entry =
///    satisfied).
///
/// Returns the first button that satisfies everything; otherwise the error of the last button (or of the
/// object or module check) as the reason for the log.
///
/// On the AI check path ([`ki_path`]) selectability and step 3 are dropped; the return value is then the
/// first button of `buttons`. On the script path ([`script_path`], action 189) only "present"
/// (`owns`) remains: death, selectability, module and step 3 are dropped; the return value is likewise the
/// first button (the callers check button and kind themselves, the script sends fixed correct ones).
pub fn check_ability(
    sim: &mut Sim,
    player: u8,
    unit: ObjectId,
    module: Option<usize>,
    buttons: &[i32],
) -> Result<i32, String> {
    // `owns` counts the unit as passed; if the command bar check fails afterwards, it does not
    // count as an effective element.
    let before = sim.verdict.borrow().passed;
    let result = check_ability_unit(sim, player, unit, module, buttons);
    if result.is_err() {
        let mut v = sim.verdict.borrow_mut();
        v.passed = v.passed.min(before);
    }
    result
}

fn check_ability_unit(
    sim: &mut Sim,
    player: u8,
    unit: ObjectId,
    module: Option<usize>,
    buttons: &[i32],
) -> Result<i32, String> {
    if !owns(sim, player, unit) {
        return Err("unit not present, foreign, quartered or dead".into());
    }
    // Script path (E-214, re/commands 9): the handlers only check that the object is there
    // (`owns`); death, selectability, module, menu, lock and MY filter are dropped. If the module is
    // missing, the message aborts all modules on delivery and does nothing else (like the original). The
    // button is the fixed one of the script.
    if script_path(sim) {
        return buttons
            .first()
            .copied()
            .ok_or_else(|| "no button given".to_string());
    }
    if is_dead(sim, unit) {
        return Err("object is dying or destroyed".into());
    }
    let Some((guid, selectable)) = sim.object(unit).map(|o| (o.guid, o.selectable)) else {
        return Err("unit not present".into());
    };
    let ki = ki_path(sim);
    if !selectable && !ki {
        return Err("object is not selectable".into());
    }
    if let Some(m) = module
        && !sim.object(unit).is_some_and(|o| o.has_module(m, false))
    {
        return Err(format!("module {m} missing or switched off"));
    }
    if ki {
        return buttons
            .first()
            .copied()
            .ok_or_else(|| "no button given".to_string());
    }
    // The rules outlive the borrow of `sim`: button and filter may be held across
    // `filter::matches` (needs `&mut Sim`).
    let rules = sim.rules;
    let mut last = Err("no button given".to_string());
    for &msg in buttons {
        let Some(button) = menu_button(rules, guid, |b| b.msg == msg) else {
            last = Err(format!("no button {msg} in the menu of the type"));
            continue;
        };
        if button_locked(sim, i32::from(player), button) {
            last = Err(format!("button {msg} locked"));
            continue;
        }
        if !my_filter_fits(sim, unit, msg) {
            last = Err(format!(
                "button {msg}: MY filter from targets.ini not satisfied"
            ));
            continue;
        }
        return Ok(msg);
    }
    last
}

/// The MY filter from `targets.ini` for the button `msg` fits `unit` (like
/// `GameWorld_RebuildTargetActions`: first entry with this `BUTTONMSG`, switched-off
/// entries do not count, player -1 = owner; no entry = satisfied). Shared step of
/// [`check_ability`] and [`button_state`].
fn my_filter_fits(sim: &mut Sim, unit: ObjectId, msg: i32) -> bool {
    let rules = sim.rules;
    rules
        .target_entry(msg)
        .filter(|e| e.enabled)
        .is_none_or(|e| filter::matches(sim, unit, &e.my, -1))
}

/// State of the ability button `button` in the command bar of the slot `player` for `unit`:
/// `None` = hidden, `Some(true)` = clickable, `Some(false)` =
/// gray. The command bar of the original only knows the menu of the type, the MY filter and
/// `Toolbar_IsButtonLocked` ([`button_locked`]); so `None` also if the object is missing or does not belong to the
/// slot, the button is not in the menu of the type or the MY filter fails, otherwise
/// `Some(!locked)`. **No** module, death or selectability test like in [`check_ability`]: the
/// camouflage of the Archer (module 25 only in variant 2924) is gray in the command bar without 2924
/// instead of hidden; the commands are not rejected until the simulation (A8). Menu and filter are shared by the call
/// with [`check_ability`] ([`menu_button`], [`my_filter_fits`]). Only reads (the checksum stays).
pub fn button_state(sim: &mut Sim, player: u8, unit: ObjectId, button: i32) -> Option<bool> {
    if unit.slot() != player {
        return None;
    }
    let guid = sim.object(unit)?.guid;
    let rules = sim.rules;
    let b = menu_button(rules, guid, |b| b.msg == button)?;
    if !my_filter_fits(sim, unit, button) {
        return None;
    }
    Some(!button_locked(sim, i32::from(player), b))
}

/// Target check of the target buttons (A8; **hypothesis** re/spells 2.5/11: the
/// UI checks the target in targeting mode against the TARGET filter of the `targets.ini` entry with
/// this `BUTTONMSG`): the first entry with `BUTTONMSG = button` as in [`my_filter_fits`]; if it is missing
/// or switched off, `false`. The filter is copied and gets the actor `unit`
/// as reference object (word 8, `MM_METHODINDEX 3` = tile distance <= sight/3 of the actor) at the head and
/// further via `next` as long as the current filter carries a chain bit `0xF000_0000` - like
/// `MatchFilter_SetReferenceChain` (`0x00554EA0`, re/match-filter 4; the follow-up filters of
/// 456/548/546 also calculate with range). Then [`filter::matches_target`] with the slot
/// `player` as player and fog player at the head (follow-up filters keep the parser value 0,
/// re/match-filter, section "Reference player"). Only reads (the checksum stays). The right-click
/// (`targeting::object_action`) still does not set the reference (K-315 open).
pub fn target_fits(
    sim: &mut Sim,
    player: u8,
    unit: ObjectId,
    button: i32,
    target: ObjectId,
) -> bool {
    let rules = sim.rules;
    let Some(e) = rules.target_entry(button).filter(|e| e.enabled) else {
        return false;
    };
    let mut f = e.target.clone();
    let mut cur = &mut f;
    loop {
        cur.ref_object = unit.as_i32();
        if cur.mode & 0xF000_0000 == 0 {
            break;
        }
        match cur.next.as_deref_mut() {
            Some(n) => cur = n,
            None => break,
        }
    }
    let player = i32::from(player);
    filter::matches_target(sim, target, &f, player, player)
}

/// Entry point for the commands `0x1D` camouflage (`GameWorld_CmdMask` `0x00476A60`), `0x1E` uncamouflage
/// (`GameWorld_CmdUnmask` `0x00476AE0`), `0x20` dig in (`GameWorld_CmdDigIn` `0x00476CC0`),
/// `0x21` dig out (`GameWorld_CmdDigOut` `0x00476D50`), `0x34`/`0x35` start/stop healing
/// (`0x004773F0`/`0x00477470`), `0x78` heal target (`0x00479010`) and `0x60` self-healing
/// (`0x0047F020`); `command::execute` routes them here (re/masking 2.3, re/healing 2.3). The original
/// checks nothing; the reimplementation checks each ID individually with [`check_ability`] (module and button per
/// command; `0x78` only ownership, life and module 29). A rejection skips only
/// this ID. The commands do not acknowledge (no reply sound; `0x60` plays the
/// self-healing sound with `direct`).
///
/// - `0x1D`: `PostMessage(0x119, abort)`, with `direct` then clear the queue.
/// - `0x1E`: like `0x1D` with `0x219`, but the list starts with the pseudo ID `n`
///   (K-79, [`unmask_ids`]).
/// - `0x20`: with `direct` clear, `PostMessage(0x11C, abort)`, with `direct` clear again.
/// - `0x21`: `PostMessage(0x21C, abort)`, with `direct` clear.
pub fn execute(sim: &mut Sim, player: u8, kind: &CommandKind, direct: bool) {
    match kind {
        CommandKind::StartHealing { units } => heal_trance(sim, player, units, direct, true),
        CommandKind::StopHealing { units } => heal_trance(sim, player, units, direct, false),
        CommandKind::HealTarget { target, units } => {
            heal_target(sim, player, *target, units, direct);
        }
        CommandKind::SelfHeal { unit, button } => self_heal(sim, player, *unit, *button, direct),
        CommandKind::EvokeGhost { unit, button } => {
            cmd_evoke_ghost(sim, player, *unit, *button, direct);
        }
        CommandKind::EvokeAnimal { unit, art, button } => {
            cmd_evoke_animal(sim, player, *unit, *art, *button, direct);
        }
        CommandKind::Banish { unit, button } => cmd_banish(sim, player, *unit, *button, direct),
        CommandKind::DrainMana {
            unit,
            target,
            button,
        } => cmd_drain_mana(sim, player, *unit, *target, *button, direct),
        CommandKind::ScoutArea { .. } => cmd_scout_area(sim, player, kind, direct),
        CommandKind::DynamiteBuilding { .. }
        | CommandKind::DynamiteShip { .. }
        | CommandKind::KillUnit { .. }
        | CommandKind::SnipeUnit { .. } => cmd_special_attack(sim, player, kind, direct),
        CommandKind::Whirlwind { unit, button } => {
            cmd_whirlwind(sim, player, *unit, *button, direct);
        }
        CommandKind::DamageBuilding {
            unit,
            target,
            button,
        } => cmd_damage_building(sim, player, *unit, *target, *button, direct),
        CommandKind::RobCorpses { unit, button } => {
            cmd_rob_corpses(sim, player, *unit, *button, direct);
        }
        _ => post_to_units(sim, player, kind, direct),
    }
}

/// The commands `0x1D`/`0x1E`/`0x20`/`0x21` (see [`execute`]).
fn post_to_units(sim: &mut Sim, player: u8, kind: &CommandKind, direct: bool) {
    let (what, module, button, code, units) = match kind {
        CommandKind::Mask { units } => (
            "Camouflage",
            masking::INDEX,
            MASK_BUTTON,
            masking::msg::START,
            units.clone(),
        ),
        CommandKind::Unmask { units } => (
            "Uncamouflage",
            masking::INDEX,
            UNMASK_BUTTON,
            masking::msg::END,
            unmask_ids(units),
        ),
        CommandKind::DigIn { units } => (
            "Dig in",
            assassin::INDEX,
            DIG_IN_BUTTON,
            assassin::msg::GO_DOWN,
            units.clone(),
        ),
        CommandKind::DigOut { units } => (
            "Dig out",
            assassin::INDEX,
            DIG_OUT_BUTTON,
            assassin::msg::GO_UP,
            units.clone(),
        ),
        _ => unreachable!("ability::execute only for 0x1D/0x1E/0x20/0x21"),
    };
    let dig_in = matches!(kind, CommandKind::DigIn { .. });
    for unit in units {
        if let Err(why) = check_ability(sim, player, unit, Some(module), &[button]) {
            skip(sim, player, what, why);
            continue;
        }
        if dig_in && direct {
            clear_queue(sim, unit);
        }
        post_message(sim, unit, Message::new(code, &[]), true);
        if direct {
            clear_queue(sim, unit);
        }
    }
}

/// `0x34` start healing (`GameWorld_CmdStartHealing` `0x004773F0`) and `0x35` stop healing
/// (`GameWorld_CmdStopHealing` `0x00477470`, re/healing 2.3): per ID in order
/// `GetObjectById(id, 0)`; **if the object is missing, the whole loop ends** (K-80, unlike
/// `0x78`). Otherwise `PostMessage({0x11D | 0x21D}, abort = 1)` and with `direct` clear the queue.
/// The original checks neither owner nor module nor button; the reimplementation checks like the
/// command bar ([`check_ability`]): start via 554/458 (trance not yet on), stop via 564/459
/// (trance on). A rejection skips only this ID; only a missing object aborts.
fn heal_trance(sim: &mut Sim, player: u8, units: &[ObjectId], direct: bool, start: bool) {
    let (what, buttons, code) = if start {
        ("Start healing", START_HEAL_BUTTONS, healing::msg::BEGIN)
    } else {
        ("Stop healing", STOP_HEAL_BUTTONS, healing::msg::END_REQUEST)
    };
    for &unit in units {
        if sim.object(unit).is_none() {
            break;
        }
        if let Err(why) = check_ability(sim, player, unit, Some(healing::INDEX), &buttons) {
            skip(sim, player, what, why);
            continue;
        }
        post_message(sim, unit, Message::new(code, &[]), true);
        if direct {
            clear_queue(sim, unit);
        }
    }
}

/// `0x78` heal target (`GameWorld_CmdHealTarget` `0x00479010`, re/healing 2.3): per ID; a
/// missing one **skips** only this ID (K-310, unlike `0x34`/`0x35`).
/// `PostMessage({0x81D, target}, abort = 1)`, with `direct` clear the queue. The command bar has
/// no producer (right-click 565 is dead, K-89): the reimplementation only checks own, living and
/// module 29 switched on, not the button. Neither the original nor the reimplementation checks the target
/// (`0x81D` accepts any ID, C-251).
fn heal_target(sim: &mut Sim, player: u8, target: ObjectId, units: &[ObjectId], direct: bool) {
    const WHAT: &str = "Heal target";
    for &unit in units {
        if sim.object(unit).is_none() {
            continue;
        }
        if !owns(sim, player, unit) {
            skip(sim, player, WHAT, "unit foreign, quartered or dead");
            continue;
        }
        if is_dead(sim, unit) {
            skip_passed(sim, player, WHAT, "unit is dying or destroyed");
            continue;
        }
        if !sim
            .object(unit)
            .is_some_and(|o| o.has_module(healing::INDEX, false))
        {
            skip_passed(sim, player, WHAT, "module 29 missing or switched off");
            continue;
        }
        let m = Message::new(healing::msg::NEW_TARGET, &[target.as_i32()]);
        post_message(sim, unit, m, true);
        if direct {
            clear_queue(sim, unit);
        }
    }
}

/// `0x60` self-healing (`GameWorld_CmdSelfHeal` `0x0047F020`, re/healing 2.3): the type value set
/// `(owner, button)` determines the cost (config 15, [`button_cost`]). With a value set and less
/// mana than the cost **nothing** happens (no rejection, like in the original); otherwise the cost is
/// deducted. **If the value set is missing, the healing is free** (K-311). Then `FillCurrent(0xE)`
/// and the effect `selbst_heilen.pef`; with `direct` sound `(0x1E, 0x22)` and clear the queue.
/// No module 29 needed. The reimplementation only allows button 555 and checks like the command bar
/// ([`check_ability`] without module); the mana check stays in the command (K-91).
fn self_heal(sim: &mut Sim, player: u8, unit: ObjectId, button: i32, direct: bool) {
    const WHAT: &str = "Self-healing";
    if button != SELF_HEAL_BUTTON {
        reject(sim, player, WHAT, format!("button {button} is not 555"));
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, None, &[SELF_HEAL_BUTTON]) {
        reject(sim, player, WHAT, why);
        return;
    }
    if let Some(cost) = button_cost(sim, player, button) {
        if sim.attribute(unit, spell::MANA) < cost {
            reject(sim, player, WHAT, format!("too little mana (cost {cost})"));
            return;
        }
        pay_mana(sim, unit, cost);
    }
    let Some(o) = sim.object_mut(unit) else {
        return;
    };
    o.attributes.fill_current(HITPOINTS);
    let pos = o.world_pos;
    sim.state.events.push(SimEvent::EffectOnObject {
        object: unit,
        path: SELF_HEAL_EFFECT.to_string(),
    });
    if direct {
        // The 4th argument of `SoundManager_PlayObjectSound3D` is the button in the original
        // (`0x0047F13B MOV EDX,[EBP+0xC]` -> PUSH EDX), not the type GUID of the unit
        // (re/healing 2.3/15, quirk D-241). For the sound database that means: type 555.
        sim.state.events.push(SimEvent::Sound3d {
            object: unit,
            action: SELF_HEAL_SOUND.0,
            flags: SELF_HEAL_SOUND.1,
            pos,
            guid: button as u32,
        });
        clear_queue(sim, unit);
    }
}

/// `0x1F` evoke ghost (`GameWorld_CmdEvokeGhost` `0x00476C50`, re/ghosts-banish 2.5): with
/// `direct` clear the queue, `PostMessage({0x11A, button}, abort = 1)`, with `direct`
/// clear **again** (C-271). The original checks nothing and does not acknowledge; the reimplementation only allows
/// button 436 and checks like the command bar ([`check_ability`], module 26). The cost and the corpse search
/// belong to the module (`0x11A` with too little mana or without a corpse does nothing; the abort of the
/// other modules has already happened).
fn cmd_evoke_ghost(sim: &mut Sim, player: u8, unit: ObjectId, button: i32, direct: bool) {
    const WHAT: &str = "Evoke ghost";
    if button != EVOKE_GHOST_BUTTON {
        reject(sim, player, WHAT, format!("button {button} is not 436"));
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, Some(evoke::INDEX), &[button]) {
        reject(sim, player, WHAT, why);
        return;
    }
    if direct {
        clear_queue(sim, unit);
    }
    post_message(sim, unit, Message::new(evoke::msg::EVOKE, &[button]), true);
    if direct {
        clear_queue(sim, unit);
    }
}

/// `0x22`-`0x24` evoke animal (`GameWorld_CmdEvokeAnimal` `0x00475AA0`, re/evoke-aura 2.4):
/// `PostMessage({0x120, art, button}, abort = 1)`, with `direct` clear the queue. The
/// reimplementation only allows kind 0-2 with the button of the kind (441/442/443, C-282 does not apply) and
/// checks like the command bar ([`check_ability`], module 32, MY filter 441-443). The module checks for too little
/// mana; a valid command without mana nevertheless aborts all modules (C-281 remainder: FIFO abort before the cast).
fn cmd_evoke_animal(sim: &mut Sim, player: u8, unit: ObjectId, art: u8, button: i32, direct: bool) {
    const WHAT: &str = "Evoke animal";
    let Some(&expected) = EVOKE_ANIMAL_BUTTONS.get(usize::from(art)) else {
        reject(sim, player, WHAT, format!("kind {art} does not exist"));
        return;
    };
    if button != expected {
        reject(
            sim,
            player,
            WHAT,
            format!("button {button} does not match kind {art} ({expected})"),
        );
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, Some(evoke_animal::INDEX), &[button]) {
        reject(sim, player, WHAT, why);
        return;
    }
    let args = [i32::from(art), button];
    post_message(
        sim,
        unit,
        Message::new(evoke_animal::msg::CAST, &args),
        true,
    );
    if direct {
        clear_queue(sim, unit);
    }
}

/// `0x25` banish (`GameWorld_CmdBanish` `0x00476DD0`, re/ghosts-banish 2.5):
/// `PostMessage({0x11F, button}, abort = 1)`, with `direct` clear the queue. The reimplementation
/// only allows the banish buttons ([`BANISH_BUTTONS`]) and checks like the command bar ([`check_ability`],
/// module 31). The module pays the mana when the message is delivered, i.e. not until after the FIFO.
fn cmd_banish(sim: &mut Sim, player: u8, unit: ObjectId, button: i32, direct: bool) {
    const WHAT: &str = "Banish";
    if !ki_path(sim) && !BANISH_BUTTONS.contains(&button) {
        reject(
            sim,
            player,
            WHAT,
            format!("button {button} is not a banish button"),
        );
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, Some(banish::INDEX), &[button]) {
        reject(sim, player, WHAT, why);
        return;
    }
    post_message(sim, unit, Message::new(banish::msg::START, &[button]), true);
    if direct {
        clear_queue(sim, unit);
    }
}

/// `0x4E` drain mana (`GameWorld_CmdDrainMana` `0x00477FD0`, re/spells 2.3): `actor ≠ target` and
/// the actor exists -> `GameObject_DispatchPostedCommand` (`0x00473690`) with the envelope
/// `{0x126, [5, 0, target, button]}`, fault none, success `{0xB7F, [target]}`, both targets -1 -
/// **synchronous**, without FIFO and without `abort_all_modules` (C-304: a second drain during the
/// animation pays twice); with `direct` clear the queue. The original does not check the target;
/// the reimplementation only allows button 541 and checks like the command bar ([`check_ability`], module 38)
/// and the target against the TARGET filter of 541 ([`target_fits`]: enemy with walk module, without fog
/// and range; an already missing target fails here). Mana and target at the cast are checked by the
/// module ([`spell`]).
fn cmd_drain_mana(
    sim: &mut Sim,
    player: u8,
    unit: ObjectId,
    target: ObjectId,
    button: i32,
    direct: bool,
) {
    const WHAT: &str = "Drain mana";
    if button != DRAIN_BUTTON {
        reject(sim, player, WHAT, format!("button {button} is not 541"));
        return;
    }
    if unit == target {
        reject(sim, player, WHAT, "target is the actor itself");
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, Some(spell::INDEX), &[DRAIN_BUTTON]) {
        reject(sim, player, WHAT, why);
        return;
    }
    if !ki_path(sim) && !target_fits(sim, player, unit, DRAIN_BUTTON, target) {
        reject(
            sim,
            player,
            WHAT,
            format!("target {} does not fit the target filter of 541", target.0),
        );
        return;
    }
    let env = Envelope {
        msg: Message::new(
            spell::msg::CAST,
            &[spell::ART_DRAIN, 0, target.as_i32(), button],
        ),
        success: Some(Message::new(DRAIN_SUCCESS, &[target.as_i32()])),
        fault: None,
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    dispatch_command(sim, unit, &env);
    if direct {
        clear_queue(sim, unit);
    }
}

/// `0x37` scout area (`GameWorld_CmdScoutArea` `0x0047E170`, re/spells 3.6): cost = config 15 of the
/// button in the value set of the owner ([`button_cost`]); with a value set and less mana **nothing**
/// (no rejection, like in the original), otherwise deduct. Then `GameWorld_SetVisionSource`
/// (`0x0047E010`) at the tile `(x, z)` with size, slot of the scout, duration and the flags
/// `3 | 4` (`flag ≠ 0` -> also clear up, vision flag 4); the effect `aufklären.pef` at the tile corner
/// `x · cell size` at height 0 (D-237, the host raises it); with `direct` sound
/// `(0x1F, 0x42)` at the position of the scout with the button as GUID and clear the queue. The
/// original checks nothing; the reimplementation only allows the buttons 484/556 with the values of the UI
/// (30, 5000, 1) and a tile on the map and checks like the command bar ([`check_ability`] without
/// module, K-313).
fn cmd_scout_area(sim: &mut Sim, player: u8, kind: &CommandKind, direct: bool) {
    const WHAT: &str = "Scout area";
    let &CommandKind::ScoutArea {
        unit,
        x,
        z,
        size,
        duration_ms,
        flag,
        button,
    } = kind
    else {
        return;
    };
    if !ki_path(sim) && !SCOUT_BUTTONS.contains(&button) {
        reject(
            sim,
            player,
            WHAT,
            format!("button {button} is not a scout button"),
        );
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, None, &[button]) {
        reject(sim, player, WHAT, why);
        return;
    }
    if (size, duration_ms, flag) != (SCOUT_SIZE, SCOUT_MS, SCOUT_FLAG) {
        reject(
            sim,
            player,
            WHAT,
            format!("size {size}, duration {duration_ms}, flag {flag} instead of 30, 5000, 1"),
        );
        return;
    }
    let map = &sim.rules.map;
    if !(0..map.dim_x).contains(&x) || !(0..map.dim_z).contains(&z) {
        reject(
            sim,
            player,
            WHAT,
            format!("tile ({x}, {z}) outside the map"),
        );
        return;
    }
    if let Some(cost) = button_cost(sim, player, button) {
        if sim.attribute(unit, spell::MANA) < cost {
            reject(sim, player, WHAT, format!("too little mana (cost {cost})"));
            return;
        }
        pay_mana(sim, unit, cost);
    }
    let Some((owner, pos)) = sim.object(unit).map(|o| (o.owner(), o.world_pos)) else {
        return;
    };
    let flags = EXPLORE | SEE | if flag != 0 { CLEAR_UP } else { 0 };
    set_vision_source(
        &mut sim.state.fog,
        VisionSource {
            x,
            z_or_hid: z,
            size,
            player: i32::from(owner),
            remaining_ms: duration_ms,
            flags,
        },
    );
    let cell = sim.rules.map.cell;
    sim.state.events.push(SimEvent::EffectAt {
        path: SCOUT_EFFECT.to_string(),
        pos: [x as f32 * cell[0] as f32, 0.0, z as f32 * cell[2] as f32],
        yaw: 0.0,
        scale: 1.0,
        start_ms: 0,
    });
    if direct {
        sim.state.events.push(SimEvent::Sound3d {
            object: unit,
            action: SCOUT_SOUND.0,
            flags: SCOUT_SOUND.1,
            pos,
            guid: button as u32,
        });
        clear_queue(sim, unit);
    }
}

/// `0x30` dynamite at building (`GameWorld_CmdDynamiteBuilding` `0x00477270`), `0x31` dynamite at
/// ship (`GameWorld_CmdDynamiteShip` `0x004772D0`), `0x32` kill unit
/// (`GameWorld_CmdKillUnit` `0x00477330`) and `0x33` eliminate unit
/// (`GameWorld_CmdSnipeUnit` `0x00477390`; re/special-attacks 2.4): actor present ->
/// `PostMessage({0x124 | 0x424 | 0x125 | 0x425, [target, button]}, abort)`, with `direct` then clear the
/// queue (pattern `0x36`). The original checks nothing; the reimplementation only allows the
/// buttons of the command kind (454, 455, 456/548, 457/546), checks the actor like the
/// command bar ([`check_ability`] with module 36 or 37) and the target against the TARGET filter of the
/// button ([`target_fits`]: fog, `MM_METHODINDEX 3`). The module checks and deducts mana
/// (K-320).
fn cmd_special_attack(sim: &mut Sim, player: u8, kind: &CommandKind, direct: bool) {
    let (what, module, buttons, code, unit, target, button): (_, _, &[i32], _, _, _, _) =
        match *kind {
            CommandKind::DynamiteBuilding {
                unit,
                target,
                button,
            } => (
                "Dynamite at building",
                bomberman::INDEX,
                &[DYNAMITE_BUILDING_BUTTON],
                bomberman::msg::ORDER_BUILDING,
                unit,
                target,
                button,
            ),
            CommandKind::DynamiteShip {
                unit,
                target,
                button,
            } => (
                "Dynamite at ship",
                bomberman::INDEX,
                &[DYNAMITE_SHIP_BUTTON],
                bomberman::msg::ORDER_SHIP,
                unit,
                target,
                button,
            ),
            CommandKind::KillUnit {
                unit,
                target,
                button,
            } => (
                "Kill unit",
                killer::INDEX,
                &KILL_BUTTONS,
                killer::msg::KILL,
                unit,
                target,
                button,
            ),
            CommandKind::SnipeUnit {
                unit,
                target,
                button,
            } => (
                "Eliminate unit",
                killer::INDEX,
                &SNIPE_BUTTONS,
                killer::msg::SNIPE,
                unit,
                target,
                button,
            ),
            _ => unreachable!("cmd_special_attack only for 0x30-0x33"),
        };
    if !ki_path(sim) && !buttons.contains(&button) {
        reject(
            sim,
            player,
            what,
            format!("button {button} does not match the command ({buttons:?})"),
        );
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, Some(module), &[button]) {
        reject(sim, player, what, why);
        return;
    }
    if !ki_path(sim) && !target_fits(sim, player, unit, button, target) {
        reject(
            sim,
            player,
            what,
            format!(
                "target {} does not fit the target filter of {button}",
                target.0
            ),
        );
        return;
    }
    post_message(
        sim,
        unit,
        Message::new(code, &[target.as_i32(), button]),
        true,
    );
    if direct {
        clear_queue(sim, unit);
    }
}

/// `0x4F` whirlwind (`GameWorld_CmdWhirlwind` `0x0047E950`, re/special-attacks 2.4, 3.6): with
/// a value set of the button ([`button_cost`]) and less mana than the cost **nothing** (no rejection,
/// like in the original), otherwise **deduct the cost in the command** (K-320: if the module finds no
/// opponent, the mana is gone anyway); with `direct` sound `(0x1F, 0x42)` at the position of the smith with
/// the button as GUID and clear the queue; then `PostMessage({0x131, [0, 0]}, abort)`.
/// The reimplementation only allows button 542 and checks like the command bar ([`check_ability`], module 49).
fn cmd_whirlwind(sim: &mut Sim, player: u8, unit: ObjectId, button: i32, direct: bool) {
    const WHAT: &str = "Whirlwind";
    if button != WHIRLWIND_BUTTON {
        reject(sim, player, WHAT, format!("button {button} is not 542"));
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, Some(special_move::INDEX), &[button]) {
        reject(sim, player, WHAT, why);
        return;
    }
    if let Some(cost) = button_cost(sim, player, button) {
        if sim.attribute(unit, spell::MANA) < cost {
            reject(sim, player, WHAT, format!("too little mana (cost {cost})"));
            return;
        }
        pay_mana(sim, unit, cost);
    }
    if direct {
        attack_sound(sim, unit, button);
        clear_queue(sim, unit);
    }
    let m = Message::new(special_move::msg::EXECUTE, &[0, 0]);
    post_message(sim, unit, m, true);
}

/// `0x4C` rob corpses (`GameWorld_CmdRobCorpses` `0x0047E890`, re/headhunter-robbing 3.7):
/// like [`cmd_whirlwind`] - with a value set of the button
/// ([`button_cost`], 540: 10) and less mana than the cost **nothing** (no rejection), otherwise deduct the
/// cost in the command (K-91); with `direct` sound `(0x1F, 0x42)` at the position of the robber with
/// the button as GUID and clear the queue; then `PostMessage({0x130}, abort)`. The
/// original searches the unit without a dead filter and checks no module 48 (K-317); the reimplementation only allows
/// button 540 and checks like the command bar ([`check_ability`], module 48; A8).
fn cmd_rob_corpses(sim: &mut Sim, player: u8, unit: ObjectId, button: i32, direct: bool) {
    const WHAT: &str = "Rob corpses";
    if button != ROB_BUTTON {
        reject(sim, player, WHAT, format!("button {button} is not 540"));
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, Some(robbing::INDEX), &[button]) {
        reject(sim, player, WHAT, why);
        return;
    }
    if let Some(cost) = button_cost(sim, player, button) {
        if sim.attribute(unit, spell::MANA) < cost {
            reject(sim, player, WHAT, format!("too little mana (cost {cost})"));
            return;
        }
        pay_mana(sim, unit, cost);
    }
    if direct {
        attack_sound(sim, unit, button);
        clear_queue(sim, unit);
    }
    post_message(sim, unit, Message::new(robbing::msg::START, &[]), true);
}

/// `0x5F` damage building (`GameWorld_CmdDamageBuilding` `0x0047EF50`, re/spells 3.7, 9.6):
/// with a value set of the button and less mana than the cost **nothing**, otherwise only **check**
/// the cost (module 49 does not deduct it until after the run, K-314); **without a value set** the
/// original passes the command pointer as the cost (K-87), the reimplementation `i32::MAX` with the same effect (the
/// smith walks there, never hammers; deviation from the original). With `direct` sound `(0x1F, 0x42)` and clear the
/// queue; then `PostMessage({0x231, [1, 1, target, cost]}, abort)`. The reimplementation
/// only allows button 551, checks like the command bar ([`check_ability`], module 49) and the target against the
/// TARGET filter of 551 ([`target_fits`]: enemy building in visible fog).
fn cmd_damage_building(
    sim: &mut Sim,
    player: u8,
    unit: ObjectId,
    target: ObjectId,
    button: i32,
    direct: bool,
) {
    const WHAT: &str = "Damage building";
    if button != DAMAGE_BUILDING_BUTTON {
        reject(sim, player, WHAT, format!("button {button} is not 551"));
        return;
    }
    if let Err(why) = check_ability(sim, player, unit, Some(special_move::INDEX), &[button]) {
        reject(sim, player, WHAT, why);
        return;
    }
    if !ki_path(sim) && !target_fits(sim, player, unit, button, target) {
        reject(
            sim,
            player,
            WHAT,
            format!("target {} does not fit the target filter of 551", target.0),
        );
        return;
    }
    let cost = match button_cost(sim, player, button) {
        Some(cost) if sim.attribute(unit, spell::MANA) < cost => {
            reject(sim, player, WHAT, format!("too little mana (cost {cost})"));
            return;
        }
        Some(cost) => cost,
        None => i32::MAX,
    };
    if direct {
        attack_sound(sim, unit, button);
        clear_queue(sim, unit);
    }
    let m = Message::new(
        special_move::msg::GOTO_BUILDING,
        &[1, 1, target.as_i32(), cost],
    );
    post_message(sim, unit, m, true);
}

/// Sound `(0x1F, 0x42)` of `0x4C`/`0x4F`/`0x5F` at the position of the actor with the button as GUID.
fn attack_sound(sim: &mut Sim, unit: ObjectId, button: i32) {
    let Some(pos) = sim.object(unit).map(|o| o.world_pos) else {
        return;
    };
    sim.state.events.push(SimEvent::Sound3d {
        object: unit,
        action: ATTACK_SOUND.0,
        flags: ATTACK_SOUND.1,
        pos,
        guid: button as u32,
    });
}

/// Target of a target button: an object or a ground point in world coordinates
/// (`x`, `z`), as the targeting mode of the UI delivers it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbilityTarget {
    Object(ObjectId),
    Ground { x: f32, z: f32 },
}

/// Command of a target button like `Input_IssueTargetAction` (`0x0044D350`, re/spells 2.3, 2.5;
/// re/special-attacks 2.4) for the first selected object `unit` of the slot `player`; for the viewer
/// (targeting mode) and the control channel. Beforehand the reimplementation checks the target (A8,
/// **hypothesis** re/spells 2.5/11): object targets against the TARGET filter of the button ([`target_fits`]
/// with fog and `MM_METHODINDEX 3` - the command `0x36` checks no target, only here the range of 480/481
/// shows), ground points against the map ([`pose_on_map`]).
///
/// - 480/481 + object -> `0x36 {1 | 2, x, z, button}` with the world position of the target at the click
///   ([`world_pos`]) as `f32` bits;
/// - 541 + object -> `0x4E`; 454/455/456/548/457/546/551 + object -> `0x30`/`0x31`/`0x32`/`0x33`/`0x5F`;
/// - 484/556 + ground -> `0x37 {kx, kz, 30, 5000, 1, button}` with `kx = Fixed_ToInt(x / cell)`
///   (`Fx::from_f32(x / cell) >> 12`);
/// - wrong target kind or button without a target command -> `Err` with a reason.
///
/// Actor, button and mana are not checked until the command. Only reads (the checksum stays).
pub fn target_command(
    sim: &mut Sim,
    player: u8,
    unit: ObjectId,
    button: i32,
    target: AbilityTarget,
) -> Result<CommandKind, String> {
    if !TARGET_BUTTONS.contains(&button) {
        return Err(format!("button {button} has no target command"));
    }
    let scout = SCOUT_BUTTONS.contains(&button);
    match target {
        AbilityTarget::Ground { x, z } => {
            if !scout {
                return Err(format!("button {button} needs an object as target"));
            }
            if !pose_on_map(&sim.rules.map, [x, 0.0, z], 0.0) {
                return Err(format!("ground point ({x}, {z}) outside the map"));
            }
            let cell = sim.rules.map.cell;
            Ok(CommandKind::ScoutArea {
                unit,
                x: Fx::from_f32(x / cell[0] as f32).0 >> 12,
                z: Fx::from_f32(z / cell[2] as f32).0 >> 12,
                size: SCOUT_SIZE,
                duration_ms: SCOUT_MS,
                flag: SCOUT_FLAG,
                button,
            })
        }
        AbilityTarget::Object(target) => {
            if scout {
                return Err(format!("button {button} needs a ground point as target"));
            }
            if !target_fits(sim, player, unit, button, target) {
                return Err(format!(
                    "target {} does not fit the target filter of {button}",
                    target.0
                ));
            }
            Ok(match button {
                480 | 481 => {
                    let p = world_pos(sim, target)
                        .ok_or_else(|| format!("target {} missing", target.0))?;
                    CommandKind::CastSpell {
                        caster: unit,
                        art: if button == 480 {
                            spell::ART_EXPLOSION
                        } else {
                            spell::ART_STORM
                        },
                        anim: p[0].to_bits() as i32,
                        idx: p[2].to_bits() as i32,
                        button,
                    }
                }
                DRAIN_BUTTON => CommandKind::DrainMana {
                    unit,
                    target,
                    button,
                },
                DYNAMITE_BUILDING_BUTTON => CommandKind::DynamiteBuilding {
                    unit,
                    target,
                    button,
                },
                DYNAMITE_SHIP_BUTTON => CommandKind::DynamiteShip {
                    unit,
                    target,
                    button,
                },
                b if KILL_BUTTONS.contains(&b) => CommandKind::KillUnit {
                    unit,
                    target,
                    button,
                },
                b if SNIPE_BUTTONS.contains(&b) => CommandKind::SnipeUnit {
                    unit,
                    target,
                    button,
                },
                DAMAGE_BUILDING_BUTTON => CommandKind::DamageBuilding {
                    unit,
                    target,
                    button,
                },
                _ => return Err(format!("button {button} has no object command")),
            })
        }
    }
}

/// The IDs that `GameWorld_CmdUnmask` (`0x00476AE0`) iterates (K-79, re/masking 2.3):
/// the original reads the counter (`+4` instead of `+8`) and IDs (from `+8` instead of `+0xC`) offset by
/// one word; the loop over `len` words from `+8` therefore begins with the count `n`, followed by
/// the IDs. **Hypothesis** (not measured): the packet length is `len = n + 1`, so the loop hits the
/// pseudo ID `n` and all IDs. For `n < 2048`, `n` decomposes into index 0, slot 0, generation `n`
/// (larger `n` run into the slot and index bits): it hits only the first object of the slot table
/// of player 0 with this generation; for other command givers the check rejects it as foreign.
fn unmask_ids(units: &[ObjectId]) -> Vec<ObjectId> {
    std::iter::once(ObjectId(units.len() as u32))
        .chain(units.iter().copied())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit::assert_rejected;
    use crate::command::{Command, NO_SPREAD, Outcome, QueuedCommand, execute as run_command};
    use crate::message::Posted;
    use crate::module::ability_testkit::*;
    use crate::module::building::building_mut;
    use crate::module::evoke_animal::evoke_animal;
    use crate::module::ghosts::evoke::evoke_ghost;
    use crate::module::walk::walk;
    use crate::object::status;
    use crate::save::checksum;
    use crate::state::SimState;

    /// Module indices of camouflage and healing.
    const MASKING: usize = 25;
    const HEALING: usize = 29;

    fn set_status(sim: &mut Sim, id: ObjectId, bits: u32) {
        sim.object_mut(id).expect("object").status |= bits;
    }

    /// The check fails with a message that contains `part`.
    fn assert_err(result: Result<i32, String>, part: &str) {
        let err = result.expect_err("check should fail");
        assert!(err.contains(part), "\"{part}\" missing in \"{err}\"");
    }

    #[test]
    fn check_ability_follows_the_toolbar() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Without 2924 module 25 (variant 10) is switched off: the check fails at the module, the lock
        // of the button is not reached.
        let plain = spawn(&mut sim, ARCHER, 0, (10, 10));
        assert_err(
            check_ability(&mut sim, 0, plain, Some(MASKING), &[434]),
            "module 25",
        );
        unlock(sim.state, 0, MASK_UPGRADE);
        let archer = spawn(&mut sim, ARCHER, 0, (12, 10));
        assert_eq!(
            check_ability(&mut sim, 0, archer, Some(MASKING), &[434]),
            Ok(434)
        );
        // Foreign (slot 1 asks) and unknown ID.
        assert_err(
            check_ability(&mut sim, 1, archer, Some(MASKING), &[434]),
            "foreign",
        );
        assert_err(
            check_ability(&mut sim, 0, ObjectId::NONE, Some(MASKING), &[434]),
            "not present",
        );
        // Camouflaged: 434 fails at the MY filter (`CHECKNOTSTATUS MASKED`), 435 fits.
        set_status(&mut sim, archer, status::MASKED);
        assert_err(
            check_ability(&mut sim, 0, archer, Some(MASKING), &[434]),
            "filter",
        );
        assert_eq!(
            check_ability(&mut sim, 0, archer, Some(MASKING), &[435]),
            Ok(435)
        );
        // Swimming: the follow-up filter of 434 (`CHECKNOTSTATUS SWIMMING`) fails.
        let swimmer = spawn(&mut sim, ARCHER, 0, (14, 10));
        set_status(&mut sim, swimmer, status::SWIMMING);
        assert_err(
            check_ability(&mut sim, 0, swimmer, Some(MASKING), &[434]),
            "filter",
        );
        // Dead and quartered (`owns`), not selectable.
        for bits in [status::DIED, status::QUARTERED] {
            let unit = spawn(&mut sim, ARCHER, 0, (16, 10));
            set_status(&mut sim, unit, bits);
            assert_err(
                check_ability(&mut sim, 0, unit, Some(MASKING), &[434]),
                "dead",
            );
        }
        let hidden = spawn(&mut sim, ARCHER, 0, (18, 10));
        sim.object_mut(hidden).unwrap().selectable = false;
        assert_err(
            check_ability(&mut sim, 0, hidden, Some(MASKING), &[434]),
            "selectable",
        );
        // Dying building (`m_IsDead`) without `DIED`.
        let post = spawn(&mut sim, ALARM_POST, 0, (30, 30));
        building_mut(&mut sim, post).expect("building").is_dead = true;
        assert_err(
            check_ability(&mut sim, 0, post, Some(MASKING), &[434]),
            "dying",
        );
        // Scout: module 25 always on; 434 (`TARGET_DO`) needs 3220 and is locked without it.
        let scout = spawn(&mut sim, SCOUT, 0, (20, 10));
        assert_err(
            check_ability(&mut sim, 0, scout, Some(MASKING), &[434]),
            "locked",
        );
        unlock(sim.state, 0, SCOUT_MASK_UPGRADE);
        assert_eq!(
            check_ability(&mut sim, 0, scout, Some(MASKING), &[434]),
            Ok(434)
        );
        // Ranger: module 25 on, but no camouflage button in the menu (he camouflages himself).
        let ranger = spawn(&mut sim, RANGER, 0, (22, 10));
        assert_err(
            check_ability(&mut sim, 0, ranger, Some(MASKING), &[434]),
            "no button 434",
        );
    }

    #[test]
    fn check_ability_takes_the_first_fitting_button() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let pilgrim = spawn(&mut sim, PILGRIM, 0, (10, 10));
        // Self-healing (`0x60`) needs no module.
        assert_eq!(check_ability(&mut sim, 0, pilgrim, None, &[555]), Ok(555));
        // Without 3125, 554 is locked; with 458 behind it the check reports the error of the last
        // button (458 is not in the menu of the Pilgrim).
        assert_err(
            check_ability(&mut sim, 0, pilgrim, Some(HEALING), &[554]),
            "554 locked",
        );
        assert_err(
            check_ability(&mut sim, 0, pilgrim, Some(HEALING), &[554, 458]),
            "no button 458",
        );
        unlock(sim.state, 0, HEAL_UPGRADE);
        assert_eq!(
            check_ability(&mut sim, 0, pilgrim, Some(HEALING), &[554, 458]),
            Ok(554)
        );
        // In the trance (`HEAL`): starting fails at the MY filter, ending fits.
        set_status(&mut sim, pilgrim, status::HEAL);
        assert_err(
            check_ability(&mut sim, 0, pilgrim, Some(HEALING), &[554]),
            "554: MY filter",
        );
        assert_eq!(
            check_ability(&mut sim, 0, pilgrim, Some(HEALING), &[564, 459]),
            Ok(564)
        );
        // Medic: 554 is missing in the menu, 458 fits.
        let medic = spawn(&mut sim, MEDIC, 0, (12, 10));
        assert_eq!(
            check_ability(&mut sim, 0, medic, Some(HEALING), &[554, 458]),
            Ok(458)
        );
        // Type without module 29 with 555 in the menu (`DO`, `REQUIRED_GUID 3126` as in the shipped data).
        let sabre = spawn(&mut sim, SABRE, 0, (14, 10));
        assert_err(
            check_ability(&mut sim, 0, sabre, None, &[555]),
            "555 locked",
        );
        unlock(sim.state, 0, SELF_HEAL_UPGRADE);
        assert_eq!(check_ability(&mut sim, 0, sabre, None, &[555]), Ok(555));
        assert_err(
            check_ability(&mut sim, 0, sabre, Some(HEALING), &[555]),
            "module 29",
        );
    }

    #[test]
    fn button_state_hides_by_menu_and_filter_but_greys_by_lock() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Without 2924 module 25 of the Archer is off: 434 still stays in the command bar, but
        // gray (`check_ability` rejects at the module). 435 falls at the MY filter (`MASKED`).
        let archer = spawn(&mut sim, ARCHER, 0, (10, 10));
        assert_eq!(button_state(&mut sim, 0, archer, 434), Some(false));
        assert_eq!(button_state(&mut sim, 0, archer, 435), None);
        // Button not in the menu, unknown button, missing and foreign object.
        assert_eq!(button_state(&mut sim, 0, archer, 437), None);
        assert_eq!(button_state(&mut sim, 0, archer, 0), None);
        assert_eq!(button_state(&mut sim, 0, ObjectId::NONE, 434), None);
        assert_eq!(button_state(&mut sim, 1, archer, 434), None);
        // Free with 2924; camouflaged it flips to 435 (MY filter), swimming hides 434.
        unlock(sim.state, 0, MASK_UPGRADE);
        let free = spawn(&mut sim, ARCHER, 0, (12, 10));
        assert_eq!(button_state(&mut sim, 0, free, 434), Some(true));
        set_status(&mut sim, free, status::MASKED);
        assert_eq!(button_state(&mut sim, 0, free, 434), None);
        assert_eq!(button_state(&mut sim, 0, free, 435), Some(true));
        let swimmer = spawn(&mut sim, ARCHER, 0, (14, 10));
        set_status(&mut sim, swimmer, status::SWIMMING);
        assert_eq!(button_state(&mut sim, 0, swimmer, 434), None);
        // No death and no selectability test: the state of the command bar depends only on menu,
        // filter and lock.
        let hidden = spawn(&mut sim, ARCHER, 0, (16, 10));
        sim.object_mut(hidden).unwrap().selectable = false;
        assert_eq!(button_state(&mut sim, 0, hidden, 434), Some(true));
        // Heal buttons: 554 gray without 3125, in the trance the filter switches to 564.
        let pilgrim = spawn(&mut sim, PILGRIM, 0, (18, 10));
        assert_eq!(button_state(&mut sim, 0, pilgrim, 554), Some(false));
        assert_eq!(button_state(&mut sim, 0, pilgrim, 564), None);
        assert_eq!(button_state(&mut sim, 0, pilgrim, 555), Some(true));
        // Queries leave the checksum the same.
        let before = checksum(sim.state);
        for button in [434, 435, 554, 564, 555] {
            let _ = button_state(&mut sim, 0, pilgrim, button);
        }
        assert_eq!(checksum(sim.state), before);
    }

    /// `(code, arguments, abort)` of the posted messages of a unit.
    fn posted(sim: &Sim, u: ObjectId) -> Vec<(u32, Vec<i32>, bool)> {
        sim.object(u)
            .unwrap()
            .fifo
            .iter()
            .map(|p| match p {
                Posted::Message { msg, abort } => (msg.code, msg.args.clone(), *abort),
                Posted::Command { env, abort } => (env.msg.code, env.msg.args.clone(), *abort),
            })
            .collect()
    }

    fn queue_len(sim: &Sim, u: ObjectId) -> usize {
        sim.object(u).unwrap().commands.len()
    }

    /// An entry in the command queue (filler that the command is to clear).
    fn fill_queue(sim: &mut Sim, u: ObjectId) {
        crate::command::enqueue(
            sim,
            u,
            QueuedCommand {
                cyclic: false,
                command: Command {
                    player: u.slot(),
                    kind: CommandKind::Halt { units: vec![u] },
                },
            },
        );
        assert_eq!(queue_len(sim, u), 1);
    }

    /// Execute the command (`direct`: as given by the player).
    fn run(sim: &mut Sim, player: u8, kind: CommandKind, direct: bool) {
        run_command(sim, &Command { player, kind }, direct);
    }

    fn masking_flags(sim: &Sim, u: ObjectId) -> (bool, bool) {
        let m = masking::masking(sim, u).expect("module 25");
        (m.is_masked, m.is_busy)
    }

    fn is_masked(sim: &Sim, u: ObjectId) -> bool {
        sim.object(u).unwrap().status & status::MASKED != 0
    }

    /// Dug in: `MASKED | WAITING4KILLING`.
    fn is_dug_in(sim: &Sim, u: ObjectId) -> bool {
        let both = status::MASKED | status::WAITING4KILLING;
        sim.object(u).unwrap().status & both == both
    }

    /// Archer of `slot` that camouflaged itself via `0x119` (done after 18 ticks).
    fn masked_archer(sim: &mut Sim, slot: i32, tile: (i32, i32)) -> ObjectId {
        let a = spawn(sim, ARCHER, slot, tile);
        post_message(sim, a, Message::new(masking::msg::START, &[]), true);
        run_ticks(sim, 18);
        assert!(is_masked(sim, a) && masking_flags(sim, a) == (true, false));
        a
    }

    /// Walk command as the UI gives it.
    fn walk_to(unit: ObjectId, tile: (i32, i32)) -> CommandKind {
        CommandKind::Move {
            units: vec![unit],
            x: (tile.0 << 12) + 0x800,
            z: (tile.1 << 12) + 0x800,
            target: -1,
            formation: 0,
            direction: crate::motion::NO_DIRECTION,
            spread: NO_SPREAD,
            flags: 0,
        }
    }

    /// Multi-object command of the command bar: units that do not fit drop out -
    /// `Partial` if others take effect, otherwise `Rejected`.
    #[test]
    fn ability_loops_report_partial_effects() {
        let rules = rules();
        let mut state = state(&rules);
        unlock(&mut state, 0, MASK_UPGRADE);
        let mut sim = Sim::new(&mut state, &rules);
        let archer = spawn(&mut sim, ARCHER, 0, (10, 10));
        let colonist = spawn(&mut sim, COLONIST, 0, (10, 12));
        let foreign = spawn(&mut sim, ARCHER, 1, (10, 14));
        let mask = |units: &[ObjectId]| Command {
            player: 0,
            kind: CommandKind::Mask {
                units: units.to_vec(),
            },
        };
        // Own Archer + Settler without a camouflage module: only the Archer takes effect.
        let out = run_command(&mut sim, &mask(&[archer, colonist]), true);
        assert!(
            matches!(&out, Outcome::Partial(why) if why.contains("module")),
            "{out:?}"
        );
        assert_eq!(posted(&sim, archer), [(0x119, vec![], true)]);
        assert!(posted(&sim, colonist).is_empty());
        // Own + foreign.
        let out = run_command(&mut sim, &mask(&[archer, foreign]), true);
        assert!(matches!(out, Outcome::Partial(_)), "{out:?}");
        // None fits: rejected (the count of `owns` is withdrawn).
        let out = run_command(&mut sim, &mask(&[colonist, foreign]), true);
        assert!(matches!(out, Outcome::Rejected(_)), "{out:?}");
        // All fit.
        let a2 = spawn(&mut sim, ARCHER, 0, (10, 16));
        let out = run_command(&mut sim, &mask(&[archer, a2]), true);
        assert_eq!(out, Outcome::Accepted);
    }
    #[test]
    fn mask_and_dig_commands_post_with_abort() {
        let rules = rules();
        let mut state = state(&rules);
        unlock(&mut state, 0, MASK_UPGRADE);
        unlock(&mut state, 0, DIG_IN_UPGRADE);
        let mut sim = Sim::new(&mut state, &rules);
        let a1 = spawn(&mut sim, ARCHER, 0, (10, 10));
        let a2 = spawn(&mut sim, ARCHER, 0, (10, 12));
        // Camouflage (`0x1D`): `0x119` with abort, with `direct` the queue is empty. The
        // Archer a1 is walking and stands still after the abort.
        let start = sim.object(a1).unwrap().grid;
        crate::tick::step(
            sim.state,
            sim.rules,
            &[Command {
                player: 0,
                kind: walk_to(a1, (40, 10)),
            }],
        );
        run_ticks(&mut sim, 5);
        assert_ne!(sim.object(a1).unwrap().grid, start, "a1 walks");
        fill_queue(&mut sim, a1);
        fill_queue(&mut sim, a2);
        run(
            &mut sim,
            0,
            CommandKind::Mask {
                units: vec![a1, a2],
            },
            true,
        );
        for a in [a1, a2] {
            assert_eq!(posted(&sim, a), [(0x119, vec![], true)]);
            assert_eq!(queue_len(&sim, a), 0, "direct: queue empty");
        }
        run_ticks(&mut sim, 3);
        let stopped = sim.object(a1).unwrap().grid;
        run_ticks(&mut sim, 20);
        assert_eq!(sim.object(a1).unwrap().grid, stopped, "walk order aborted");
        for a in [a1, a2] {
            assert!(is_masked(&sim, a), "both camouflaged after the FIFO");
            assert_eq!(masking_flags(&sim, a), (true, false));
        }
        // Not direct (command from a queue): it stays.
        let a3 = spawn(&mut sim, ARCHER, 0, (10, 14));
        fill_queue(&mut sim, a3);
        run(&mut sim, 0, CommandKind::Mask { units: vec![a3] }, false);
        assert_eq!(posted(&sim, a3), [(0x119, vec![], true)]);
        assert_eq!(queue_len(&sim, a3), 1);
        // Uncamouflage (`0x1E`): `0x219` with abort. The pseudo ID (count 1 -> `ObjectId(1)`,
        // K-79) hits the first object, here a1, and does the same there.
        fill_queue(&mut sim, a2);
        run(&mut sim, 0, CommandKind::Unmask { units: vec![a2] }, true);
        assert_eq!(posted(&sim, a2), [(0x219, vec![], true)]);
        assert_eq!(posted(&sim, a1), [(0x219, vec![], true)]);
        assert_eq!(queue_len(&sim, a2), 0);
        run_ticks(&mut sim, 2);
        assert_eq!(
            masking_flags(&sim, a2),
            (false, true),
            "uncamouflage running"
        );
        // Dig in (`0x20`): queue empty, `0x11C` with abort.
        let m1 = spawn(&mut sim, ASSASSIN, 0, (20, 10));
        let m2 = spawn(&mut sim, ASSASSIN, 0, (20, 12));
        fill_queue(&mut sim, m1);
        fill_queue(&mut sim, m2);
        run(
            &mut sim,
            0,
            CommandKind::DigIn {
                units: vec![m1, m2],
            },
            true,
        );
        for m in [m1, m2] {
            assert_eq!(posted(&sim, m), [(0x11C, vec![], true)]);
            assert_eq!(queue_len(&sim, m), 0);
        }
        run_ticks(&mut sim, 20);
        assert!(is_dug_in(&sim, m1) && is_dug_in(&sim, m2), "dug in");
        // Dig out (`0x21`): `0x21C` with abort, with `direct` the queue is empty.
        fill_queue(&mut sim, m1);
        run(&mut sim, 0, CommandKind::DigOut { units: vec![m1] }, true);
        assert_eq!(posted(&sim, m1), [(0x21C, vec![], true)]);
        assert_eq!(queue_len(&sim, m1), 0);
        run_ticks(&mut sim, 2);
        assert_eq!(
            sim.object(m1).unwrap().status & (status::MASKED | status::WAITING4KILLING),
            0,
            "dug out"
        );
        assert!(is_dug_in(&sim, m2), "m2 stays down");
    }

    #[test]
    fn ability_commands_skip_ids_that_fail_the_toolbar_check() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Locked without 2924/2922: the variants are off, the modules are missing.
        let locked_archer = spawn(&mut sim, ARCHER, 0, (8, 8));
        let locked_assassin = spawn(&mut sim, ASSASSIN, 0, (8, 10));
        unlock(sim.state, 0, MASK_UPGRADE);
        unlock(sim.state, 0, DIG_IN_UPGRADE);
        let foreign = spawn(&mut sim, ARCHER, 1, (10, 10));
        let already_masked = masked_archer(&mut sim, 0, (10, 14));
        let valid = spawn(&mut sim, ARCHER, 0, (10, 16));
        let dead = spawn(&mut sim, ARCHER, 0, (10, 12));
        set_status(&mut sim, dead, status::DIED);

        // Camouflage: only `valid` takes effect; the invalid ones before it do not abort the list (A8).
        let rejected = vec![ObjectId::NONE, foreign, dead, locked_archer, already_masked];
        assert_rejected(
            &mut sim,
            0,
            CommandKind::Mask {
                units: rejected.clone(),
            },
        );
        let mut list = rejected;
        list.push(valid);
        run(&mut sim, 0, CommandKind::Mask { units: list }, true);
        assert_eq!(posted(&sim, valid), [(0x119, vec![], true)]);
        for u in [foreign, dead, locked_archer, already_masked] {
            assert!(posted(&sim, u).is_empty(), "{u:?} rejected");
        }

        // Uncamouflage: `valid` is not yet camouflaged (the posted `0x119` did not run), foreign,
        // dead and locked ones drop out; the pseudo ID (count 4) hits no object.
        assert_rejected(
            &mut sim,
            0,
            CommandKind::Unmask {
                units: vec![valid, foreign, dead, locked_archer],
            },
        );
        run(
            &mut sim,
            0,
            CommandKind::Unmask {
                units: vec![valid, foreign, already_masked],
            },
            true,
        );
        assert_eq!(posted(&sim, already_masked), [(0x219, vec![], true)]);
        assert!(posted(&sim, foreign).is_empty());

        // Dig in: assassins with module 28 switched off and foreign ones drop out.
        let ok = spawn(&mut sim, ASSASSIN, 0, (20, 10));
        let foreign_assassin = spawn(&mut sim, ASSASSIN, 1, (20, 12));
        assert_rejected(
            &mut sim,
            0,
            CommandKind::DigIn {
                units: vec![locked_assassin, foreign_assassin],
            },
        );
        run(
            &mut sim,
            0,
            CommandKind::DigIn {
                units: vec![locked_assassin, foreign_assassin, ok],
            },
            true,
        );
        assert_eq!(posted(&sim, ok), [(0x11C, vec![], true)]);
        assert!(posted(&sim, locked_assassin).is_empty());

        // Dig out needs `WAITING4KILLING` (button 438); dig in then no longer (437).
        run_ticks(&mut sim, 20);
        assert!(is_dug_in(&sim, ok));
        let standing = spawn(&mut sim, ASSASSIN, 0, (20, 14));
        assert_rejected(
            &mut sim,
            0,
            CommandKind::DigOut {
                units: vec![standing],
            },
        );
        assert_rejected(&mut sim, 0, CommandKind::DigIn { units: vec![ok] });
        run(
            &mut sim,
            0,
            CommandKind::DigOut {
                units: vec![standing, ok],
            },
            true,
        );
        assert_eq!(posted(&sim, ok), [(0x21C, vec![], true)]);
        assert!(posted(&sim, standing).is_empty());
    }

    // QUIRK K-79: kept
    #[test]
    fn unmask_treats_the_count_as_an_id() {
        // re/masking 9, K-79: `n = 1`, `ids = [X]`; the loop first hits the pseudo ID 1
        // (index 0, slot 0, generation 1) and then X.
        let rules = rules();
        let mut state = state(&rules);
        unlock(&mut state, 0, MASK_UPGRADE);
        unlock(&mut state, 1, MASK_UPGRADE);
        let mut sim = Sim::new(&mut state, &rules);
        let first = masked_archer(&mut sim, 0, (10, 10));
        assert_eq!(
            first,
            ObjectId(1),
            "first object of the slot table, generation 1"
        );
        let x = masked_archer(&mut sim, 0, (10, 14));
        let theirs = masked_archer(&mut sim, 1, (30, 10));
        fill_queue(&mut sim, first);
        // Slot 1 gives the command: the pseudo ID hits a foreign object and is rejected
        // (A8), the own ID takes effect.
        run(
            &mut sim,
            1,
            CommandKind::Unmask {
                units: vec![theirs],
            },
            true,
        );
        assert!(posted(&sim, first).is_empty(), "foreign: rejected");
        assert_eq!(queue_len(&sim, first), 1);
        assert_eq!(posted(&sim, theirs), [(0x219, vec![], true)]);
        run_ticks(&mut sim, 2);
        assert_eq!(masking_flags(&sim, theirs), (false, true));
        assert_eq!(masking_flags(&sim, first), (true, false));
        // Slot 0: both get `0x219`; the pseudo ID also takes the queue of the
        // first object.
        run(&mut sim, 0, CommandKind::Unmask { units: vec![x] }, true);
        assert_eq!(posted(&sim, first), [(0x219, vec![], true)]);
        assert_eq!(posted(&sim, x), [(0x219, vec![], true)]);
        assert_eq!(
            queue_len(&sim, first),
            0,
            "direct: its queue is empty as well"
        );
        run_ticks(&mut sim, 2);
        assert_eq!(masking_flags(&sim, first), (false, true));
        assert_eq!(masking_flags(&sim, x), (false, true));
        // Two IDs: the pseudo ID is 2 (generation 2, no hit), both take effect.
        let y = masked_archer(&mut sim, 0, (14, 10));
        let z = masked_archer(&mut sim, 0, (14, 14));
        run(&mut sim, 0, CommandKind::Unmask { units: vec![y, z] }, true);
        assert_eq!(posted(&sim, y), [(0x219, vec![], true)]);
        assert_eq!(posted(&sim, z), [(0x219, vec![], true)]);
        assert!(posted(&sim, first).is_empty(), "pseudo ID 2 hits nothing");
    }

    /// Set the mana (attribute 15) of a unit to `value` (created healers have 0).
    fn set_mana(sim: &mut Sim, id: ObjectId, value: i32) {
        let o = sim.object(id).unwrap();
        let pct = sim.type_upgrade_percent(o.owner(), o.guid, 15);
        sim.object_mut(id)
            .unwrap()
            .attributes
            .set_current_value(15, value, pct);
    }

    fn mana(sim: &Sim, id: ObjectId) -> i32 {
        sim.attribute(id, spell::MANA)
    }

    fn hitpoint_fraction(sim: &Sim, id: ObjectId) -> i32 {
        sim.object(id).unwrap().attributes.current_fraction(14)
    }

    fn is_healing(sim: &Sim, u: ObjectId) -> bool {
        sim.object(u).unwrap().status & status::HEAL != 0
    }

    fn is_ready(sim: &Sim, u: ObjectId) -> bool {
        healing::healing(sim, u).expect("module 29").ready
    }

    fn new_target(sim: &Sim, u: ObjectId) -> ObjectId {
        healing::healing(sim, u).expect("module 29").new_target
    }

    /// Send a healer via a posted `0x11D` (bypassing the command) into the ready trance.
    fn entrance(sim: &mut Sim, u: ObjectId) {
        post_message(sim, u, Message::new(healing::msg::BEGIN, &[]), true);
        run_ticks(sim, 45);
        assert!(is_healing(sim, u) && is_ready(sim, u), "trance ready");
    }

    #[test]
    fn start_and_stop_healing_follow_the_buttons() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let pilgrim = spawn(&mut sim, PILGRIM, 0, (10, 10));
        // Without 3125, 554 is locked; stop needs `HEAL` (MY filter of 564).
        assert_rejected(
            &mut sim,
            0,
            CommandKind::StartHealing {
                units: vec![pilgrim],
            },
        );
        unlock(sim.state, 0, HEAL_UPGRADE);
        assert_rejected(
            &mut sim,
            0,
            CommandKind::StopHealing {
                units: vec![pilgrim],
            },
        );
        // Foreign sender: rejected.
        assert_rejected(
            &mut sim,
            1,
            CommandKind::StartHealing {
                units: vec![pilgrim],
            },
        );
        // Start: `0x11D` with abort, with `direct` the queue is empty.
        fill_queue(&mut sim, pilgrim);
        run(
            &mut sim,
            0,
            CommandKind::StartHealing {
                units: vec![pilgrim],
            },
            true,
        );
        assert_eq!(posted(&sim, pilgrim), [(0x11D, vec![], true)]);
        assert_eq!(queue_len(&sim, pilgrim), 0);
        run_ticks(&mut sim, 1);
        assert!(is_healing(&sim, pilgrim), "trance on");
        // In the trance, starting is rejected (MY filter of 554), stopping is allowed.
        assert_rejected(
            &mut sim,
            0,
            CommandKind::StartHealing {
                units: vec![pilgrim],
            },
        );
        run_ticks(&mut sim, 45);
        assert!(is_ready(&sim, pilgrim));
        // Not direct: the queue stays.
        fill_queue(&mut sim, pilgrim);
        run(
            &mut sim,
            0,
            CommandKind::StopHealing {
                units: vec![pilgrim],
            },
            false,
        );
        assert_eq!(posted(&sim, pilgrim), [(0x21D, vec![], true)]);
        assert_eq!(queue_len(&sim, pilgrim), 1);
        run_ticks(&mut sim, 45);
        assert!(!is_healing(&sim, pilgrim), "trance ended");
        assert!(!is_ready(&sim, pilgrim));

        // Medic: buttons 458/459 without research.
        let medic = spawn(&mut sim, MEDIC, 0, (14, 10));
        assert_rejected(&mut sim, 0, CommandKind::StopHealing { units: vec![medic] });
        run(
            &mut sim,
            0,
            CommandKind::StartHealing { units: vec![medic] },
            true,
        );
        assert_eq!(posted(&sim, medic), [(0x11D, vec![], true)]);
        run_ticks(&mut sim, 60);
        assert!(is_healing(&sim, medic) && is_ready(&sim, medic));
        run(
            &mut sim,
            0,
            CommandKind::StopHealing { units: vec![medic] },
            true,
        );
        assert_eq!(posted(&sim, medic), [(0x21D, vec![], true)]);
        run_ticks(&mut sim, 60);
        assert!(!is_healing(&sim, medic));
        // Type without module 29 (Colonist): rejected.
        let colonist = spawn(&mut sim, COLONIST, 0, (18, 10));
        assert_rejected(
            &mut sim,
            0,
            CommandKind::StartHealing {
                units: vec![colonist],
            },
        );
    }

    // QUIRK K-80: kept
    #[test]
    fn healing_commands_stop_at_the_first_missing_id() {
        // K-80: `GetObjectById == NULL` ends the loop; an existing but rejected object (foreign)
        // skips only itself.
        let rules = rules();
        let mut state = state(&rules);
        unlock(&mut state, 0, HEAL_UPGRADE);
        unlock(&mut state, 1, HEAL_UPGRADE);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, PILGRIM, 0, (10, 10));
        let b = spawn(&mut sim, PILGRIM, 0, (10, 14));
        let foreign = spawn(&mut sim, PILGRIM, 1, (30, 10));
        run(
            &mut sim,
            0,
            CommandKind::StartHealing {
                units: vec![a, ObjectId::NONE, b],
            },
            true,
        );
        assert_eq!(posted(&sim, a), [(0x11D, vec![], true)]);
        assert!(posted(&sim, b).is_empty(), "behind the missing ID");
        let c = spawn(&mut sim, PILGRIM, 0, (10, 18));
        run(
            &mut sim,
            0,
            CommandKind::StartHealing {
                units: vec![foreign, c],
            },
            true,
        );
        assert!(posted(&sim, foreign).is_empty());
        assert_eq!(
            posted(&sim, c),
            [(0x11D, vec![], true)],
            "foreign one only skipped"
        );
        // Stop: the same loop.
        let d = spawn(&mut sim, PILGRIM, 0, (20, 10));
        let e = spawn(&mut sim, PILGRIM, 0, (20, 14));
        entrance(&mut sim, d);
        entrance(&mut sim, e);
        run(
            &mut sim,
            0,
            CommandKind::StopHealing {
                units: vec![d, ObjectId::NONE, e],
            },
            true,
        );
        assert_eq!(posted(&sim, d), [(0x21D, vec![], true)]);
        assert!(posted(&sim, e).is_empty());
    }

    // QUIRK K-310: kept
    #[test]
    fn heal_target_skips_missing_ids() {
        // K-310: `0x78` skips missing IDs; `0x81D` is discarded by the filter
        // `{0, [0x21D]}` in the trance (K-53), stored outside it.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, PILGRIM, 0, (10, 10));
        let target = spawn(&mut sim, COLONIST, 0, (14, 10));
        fill_queue(&mut sim, a);
        run(
            &mut sim,
            0,
            CommandKind::HealTarget {
                target,
                units: vec![ObjectId::NONE, a],
            },
            true,
        );
        assert_eq!(
            posted(&sim, a),
            [(0x81D, vec![target.as_i32()], true)],
            "the ID after the missing one gets the order"
        );
        assert_eq!(queue_len(&sim, a), 0);
        run_ticks(&mut sim, 1);
        assert_eq!(new_target(&sim, a), target);
        // Without `direct` the queue stays.
        let n = spawn(&mut sim, PILGRIM, 0, (10, 14));
        fill_queue(&mut sim, n);
        run(
            &mut sim,
            0,
            CommandKind::HealTarget {
                target,
                units: vec![n],
            },
            false,
        );
        assert_eq!(queue_len(&sim, n), 1);
        // In the trance: the filter discards `0x81D`.
        let b = spawn(&mut sim, PILGRIM, 0, (10, 18));
        entrance(&mut sim, b);
        run(
            &mut sim,
            0,
            CommandKind::HealTarget {
                target,
                units: vec![b],
            },
            true,
        );
        assert!(posted(&sim, b).is_empty(), "already discarded when posting");
        run_ticks(&mut sim, 2);
        assert_eq!(new_target(&sim, b), ObjectId(0), "discarded in the trance");
        // Rejections (A8): foreign, without module 29, dead; the checksum stays.
        let foreign = spawn(&mut sim, PILGRIM, 1, (30, 10));
        let colonist = spawn(&mut sim, COLONIST, 0, (18, 10));
        let dead = spawn(&mut sim, PILGRIM, 0, (22, 10));
        set_status(&mut sim, dead, status::DIED);
        for units in [vec![foreign], vec![colonist], vec![dead]] {
            assert_rejected(&mut sim, 0, CommandKind::HealTarget { target, units });
        }
    }

    // QUIRK K-91, D-241: kept
    #[test]
    fn self_heal_pays_the_button_cost_or_nothing() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let pilgrim = spawn(&mut sim, PILGRIM, 0, (10, 10));
        let hurt = |sim: &mut Sim, id: ObjectId| {
            sim.object_mut(id)
                .unwrap()
                .attributes
                .set_current_fraction(14, 30_000);
        };
        let has_effect = |sim: &Sim, id: ObjectId| {
            sim.state.events.iter().any(|e| {
                matches!(e, SimEvent::EffectOnObject { object, path }
                    if *object == id && path == "data/effects/selbst_heilen.pef")
            })
        };
        let has_sound = |sim: &Sim, id: ObjectId| {
            sim.state.events.iter().any(|e| {
                matches!(e, SimEvent::Sound3d { object, action: 0x1E, flags: 0x22, guid: 555, .. }
                    if *object == id)
            })
        };
        let command = |unit| CommandKind::SelfHeal { unit, button: 555 };

        // Mana 60, cost 50: hit points full, mana 10, effect and sound, queue empty.
        hurt(&mut sim, pilgrim);
        set_mana(&mut sim, pilgrim, 60);
        fill_queue(&mut sim, pilgrim);
        sim.state.events.clear();
        run(&mut sim, 0, command(pilgrim), true);
        assert_eq!(hitpoint_fraction(&sim, pilgrim), 100_000);
        assert_eq!(mana(&sim, pilgrim), 10);
        assert!(has_effect(&sim, pilgrim) && has_sound(&sim, pilgrim));
        assert_eq!(queue_len(&sim, pilgrim), 0);
        assert!(posted(&sim, pilgrim).is_empty(), "nothing posted");

        // Mana 40 < 50: nothing (no effect, queue stays); the command reports the
        // rejection.
        hurt(&mut sim, pilgrim);
        set_mana(&mut sim, pilgrim, 40);
        fill_queue(&mut sim, pilgrim);
        sim.state.events.clear();
        let outcome = run_command(
            &mut sim,
            &Command {
                player: 0,
                kind: command(pilgrim),
            },
            true,
        );
        assert!(
            matches!(&outcome, Outcome::Rejected(why) if why.contains("too little mana")),
            "{outcome:?}"
        );
        assert_eq!(hitpoint_fraction(&sim, pilgrim), 30_000);
        assert_eq!(mana(&sim, pilgrim), 40);
        assert!(!has_effect(&sim, pilgrim) && !has_sound(&sim, pilgrim));
        assert_eq!(queue_len(&sim, pilgrim), 1);

        // Exactly the cost suffices (mana afterwards 0); not direct: no sound, queue stays.
        set_mana(&mut sim, pilgrim, 50);
        sim.state.events.clear();
        run(&mut sim, 0, command(pilgrim), false);
        assert_eq!(hitpoint_fraction(&sim, pilgrim), 100_000);
        assert_eq!(mana(&sim, pilgrim), 0);
        assert!(has_effect(&sim, pilgrim) && !has_sound(&sim, pilgrim));
        assert_eq!(queue_len(&sim, pilgrim), 1);

        // Rejections: foreign, another button, unit without 555 in the menu; checksum the same.
        assert_rejected(&mut sim, 1, command(pilgrim));
        assert_rejected(
            &mut sim,
            0,
            CommandKind::SelfHeal {
                unit: pilgrim,
                button: 554,
            },
        );
        let colonist = spawn(&mut sim, COLONIST, 0, (14, 10));
        assert_rejected(&mut sim, 0, command(colonist));

        // Without a value set of the button GUID 555: free, the mana stays (K-311).
        let mut free_rules = rules_from(&types());
        free_rules.defaults.remove(&555);
        let mut free_state = crate::module::ability_testkit::state(&free_rules);
        let mut sim = Sim::new(&mut free_state, &free_rules);
        let pilgrim = spawn(&mut sim, PILGRIM, 0, (10, 10));
        hurt(&mut sim, pilgrim);
        set_mana(&mut sim, pilgrim, 0);
        sim.state.events.clear();
        run(&mut sim, 0, command(pilgrim), true);
        assert_eq!(hitpoint_fraction(&sim, pilgrim), 100_000);
        assert_eq!(mana(&sim, pilgrim), 0);
        assert!(has_effect(&sim, pilgrim) && has_sound(&sim, pilgrim));
    }

    fn evoking(sim: &Sim, id: ObjectId) -> bool {
        sim.object(id)
            .is_some_and(|o| o.status & status::EVOKING != 0)
    }

    fn evoke_ghost_cmd(unit: ObjectId) -> CommandKind {
        CommandKind::EvokeGhost {
            unit,
            button: EVOKE_GHOST_BUTTON,
        }
    }

    fn evoke_animal_cmd(unit: ObjectId, art: u8, button: i32) -> CommandKind {
        CommandKind::EvokeAnimal { unit, art, button }
    }

    fn banish_cmd(unit: ObjectId, button: i32) -> CommandKind {
        CommandKind::Banish { unit, button }
    }

    /// The commands post with abort; the effect does not come until after the FIFO (one object
    /// tick).
    // QUIRK C-271: kept
    #[test]
    fn evocation_commands_post_with_abort() {
        let rules = evoke_rules();
        let mut state = state(&rules);
        for guid in [
            EVOKE_GHOST_UPGRADE,
            BANISH_GHOSTS_UPGRADE,
            EVOKE_WOLF_UPGRADE,
        ] {
            unlock(&mut state, 0, guid);
        }
        let mut sim = Sim::new(&mut state, &rules);

        // Evoke ghost (`0x1F`): corpse four tiles behind the evoker, who is currently walking
        // east. Posted `{0x11A, 436}` with abort, the (filled) queue is empty.
        let body = corpse(&mut sim, 1, (10, 14));
        let ev = spawn(&mut sim, GHOST_EVOKER, 0, (10, 10));
        set_mana(&mut sim, ev, 150);
        let home = sim.object(ev).unwrap().grid;
        crate::tick::step(
            sim.state,
            sim.rules,
            &[Command {
                player: 0,
                kind: walk_to(ev, (40, 10)),
            }],
        );
        run_ticks(&mut sim, 3);
        assert_ne!(sim.object(ev).unwrap().grid.x, home.x, "he walks");
        fill_queue(&mut sim, ev);
        run(&mut sim, 0, evoke_ghost_cmd(ev), true);
        assert_eq!(posted(&sim, ev), [(0x11A, vec![436], true)]);
        assert_eq!(queue_len(&sim, ev), 0, "direct: queue empty");
        assert_eq!(mana(&sim, ev), 150, "nothing paid before the FIFO");
        // After the FIFO: corpse search has run (corpse remembered, FINDTO with the own position as
        // target and the corpse as look-at point), the walk order is aborted.
        run_ticks(&mut sim, 1);
        let at = sim.object(ev).unwrap().grid;
        let target = sim.object(body).unwrap().grid;
        assert_eq!(evoke_ghost(&sim, ev).unwrap().dead_body, body);
        let find_to = walk(&sim, ev)
            .and_then(|w| w.find_to.clone())
            .expect("Walk has the FINDTO order");
        assert_eq!(
            find_to.args,
            [at.x.0, at.z.0, -1, target.x.0, target.z.0, -1, 1]
        );
        run_ticks(&mut sim, 10);
        let after = sim.object(ev).unwrap().grid;
        assert_eq!(
            (after.x, after.z),
            (at.x, at.z),
            "walk order aborted, the evoker does not walk on"
        );
        // Not direct (command from a queue): it stays.
        let other = spawn(&mut sim, GHOST_EVOKER, 0, (20, 20));
        fill_queue(&mut sim, other);
        run(&mut sim, 0, evoke_ghost_cmd(other), false);
        assert_eq!(posted(&sim, other), [(0x11A, vec![436], true)]);
        assert_eq!(queue_len(&sim, other), 1);

        // Evoke animal (`0x22`): `{0x120, kind, button}`; after the object tick `EVOKING`, mana
        // paid.
        let animal = spawn(&mut sim, ANIMAL_EVOKER, 0, (30, 30));
        set_mana(&mut sim, animal, 100);
        fill_queue(&mut sim, animal);
        run(&mut sim, 0, evoke_animal_cmd(animal, 0, 441), true);
        assert_eq!(posted(&sim, animal), [(0x120, vec![0, 441], true)]);
        assert_eq!(queue_len(&sim, animal), 0);
        assert!(!evoking(&sim, animal));
        run_ticks(&mut sim, 1);
        assert!(evoking(&sim, animal), "EVOKING");
        assert_eq!(mana(&sim, animal), 75);

        // Banish (`0x25`, button 547 at the 1411, only `DO`): `{0x11F, 547}`; the mana is not paid
        // until the FIFO.
        let banisher = spawn(&mut sim, GHOST_EVOKER, 0, (40, 40));
        set_mana(&mut sim, banisher, 150);
        fill_queue(&mut sim, banisher);
        run(&mut sim, 0, banish_cmd(banisher, 547), true);
        assert_eq!(posted(&sim, banisher), [(0x11F, vec![547], true)]);
        assert_eq!(queue_len(&sim, banisher), 0);
        assert_eq!(mana(&sim, banisher), 150, "mana not until after the FIFO");
        run_ticks(&mut sim, 1);
        assert_eq!(mana(&sim, banisher), 125);
        assert_ne!(
            sim.object(banisher).unwrap().status & status::ANIM_LOCKED,
            0,
            "banish animation running"
        );
    }

    /// The commands check like the command bar (A8): ownership, life, selectability, module, menu,
    /// lock, MY filter (436 and 441-443: property and not `SWIMMING`) and the button matching
    /// the kind.
    // QUIRK C-281: deviation
    #[test]
    fn evocation_commands_follow_the_toolbar() {
        let rules = evoke_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let ghost_evoker = spawn(&mut sim, GHOST_EVOKER, 0, (10, 10));
        let animal_evoker = spawn(&mut sim, ANIMAL_EVOKER, 0, (10, 14));
        let missionary = spawn(&mut sim, MISSIONARY, 0, (10, 18));
        // Locked without research (436/2930, 547/2931 - at the 1411 only `DO` without `GROUP_AND` -,
        // 441/2926, 439/2827).
        assert_rejected(&mut sim, 0, evoke_ghost_cmd(ghost_evoker));
        assert_rejected(&mut sim, 0, banish_cmd(ghost_evoker, 547));
        assert_rejected(&mut sim, 0, evoke_animal_cmd(animal_evoker, 0, 441));
        assert_rejected(&mut sim, 0, banish_cmd(missionary, 439));
        for guid in [
            EVOKE_GHOST_UPGRADE,
            BANISH_GHOSTS_UPGRADE,
            EVOKE_WOLF_UPGRADE,
            EVOKE_CAT_UPGRADE,
            MISSIONARY_BANISH_UPGRADE,
        ] {
            unlock(sim.state, 0, guid);
        }
        // Free: all kinds are posted (control against the rejections before and after).
        run(&mut sim, 0, evoke_ghost_cmd(ghost_evoker), true);
        assert_eq!(posted(&sim, ghost_evoker), [(0x11A, vec![436], true)]);
        run(&mut sim, 0, banish_cmd(ghost_evoker, 547), true);
        assert_eq!(posted(&sim, ghost_evoker).len(), 2, "547 at the 1411 free");
        run(&mut sim, 0, evoke_animal_cmd(animal_evoker, 0, 441), true);
        assert_eq!(posted(&sim, animal_evoker), [(0x120, vec![0, 441], true)]);
        run(&mut sim, 0, evoke_animal_cmd(animal_evoker, 2, 443), true);
        assert_eq!(posted(&sim, animal_evoker).len(), 2, "kind 2 with 443");
        run(&mut sim, 0, banish_cmd(missionary, 439), true);
        assert_eq!(posted(&sim, missionary), [(0x11F, vec![439], true)]);

        // Kind and button must match; there is no kind above 2; the bear (442, 2928) is still
        // locked.
        let fresh = spawn(&mut sim, ANIMAL_EVOKER, 0, (14, 14));
        assert_rejected(&mut sim, 0, evoke_animal_cmd(fresh, 0, 442));
        assert_rejected(&mut sim, 0, evoke_animal_cmd(fresh, 2, 441));
        assert_rejected(&mut sim, 0, evoke_animal_cmd(fresh, 3, 441));
        assert_rejected(&mut sim, 0, evoke_animal_cmd(fresh, 255, 441));
        assert_rejected(&mut sim, 0, evoke_animal_cmd(fresh, 1, 442));
        // Button of another ability.
        let fresh_ghost = spawn(&mut sim, GHOST_EVOKER, 0, (14, 10));
        assert_rejected(&mut sim, 0, banish_cmd(fresh_ghost, EVOKE_GHOST_BUTTON));
        assert_rejected(
            &mut sim,
            0,
            CommandKind::EvokeGhost {
                unit: fresh_ghost,
                button: 547,
            },
        );
        assert_rejected(&mut sim, 0, banish_cmd(fresh_ghost, 0));
        // Foreign (slot 1 gives the command for a unit of slot 0; slot 1 has also researched
        // nothing), dead, not selectable.
        assert_rejected(&mut sim, 1, evoke_ghost_cmd(fresh_ghost));
        let foreign = spawn(&mut sim, GHOST_EVOKER, 1, (18, 10));
        assert_rejected(&mut sim, 1, evoke_ghost_cmd(foreign));
        let dead = spawn(&mut sim, GHOST_EVOKER, 0, (18, 14));
        set_status(&mut sim, dead, status::DIED);
        assert_rejected(&mut sim, 0, evoke_ghost_cmd(dead));
        assert_rejected(&mut sim, 0, banish_cmd(dead, 547));
        let hidden = spawn(&mut sim, GHOST_EVOKER, 0, (18, 18));
        sim.object_mut(hidden).unwrap().selectable = false;
        assert_rejected(&mut sim, 0, evoke_ghost_cmd(hidden));
        // Swimming: the MY filter of 436 and that of 441 (`CHECKNOTSTATUS SWIMMING`) reject;
        // the banish buttons have no entry and stay free.
        let swimming_ghost = spawn(&mut sim, GHOST_EVOKER, 0, (22, 10));
        set_status(&mut sim, swimming_ghost, status::SWIMMING);
        assert_rejected(&mut sim, 0, evoke_ghost_cmd(swimming_ghost));
        run(&mut sim, 0, banish_cmd(swimming_ghost, 547), true);
        assert_eq!(posted(&sim, swimming_ghost), [(0x11F, vec![547], true)]);
        let swimming_animal = spawn(&mut sim, ANIMAL_EVOKER, 0, (22, 14));
        set_status(&mut sim, swimming_animal, status::SWIMMING);
        assert_rejected(&mut sim, 0, evoke_animal_cmd(swimming_animal, 0, 441));
        // Without module: the colonist has none of the buttons in the menu.
        let colonist = spawn(&mut sim, COLONIST, 0, (22, 18));
        assert_rejected(&mut sim, 0, evoke_ghost_cmd(colonist));
        assert_rejected(&mut sim, 0, evoke_animal_cmd(colonist, 0, 441));
        assert_rejected(&mut sim, 0, banish_cmd(colonist, 547));
    }

    /// C-281 remainder: a valid animal evocation without enough mana still aborts all modules
    /// (FIFO entry with abort: first `AbortAllModules`, then the cast); the module evokes
    /// nothing.
    // QUIRK C-281: deviation
    #[test]
    fn a_valid_evocation_without_mana_still_aborts() {
        let rules = evoke_rules();
        let mut state = state(&rules);
        unlock(&mut state, 0, EVOKE_WOLF_UPGRADE);
        let mut sim = Sim::new(&mut state, &rules);
        let animal = spawn(&mut sim, ANIMAL_EVOKER, 0, (10, 10));
        set_mana(&mut sim, animal, 0);
        let home = sim.object(animal).unwrap().grid;
        crate::tick::step(
            sim.state,
            sim.rules,
            &[Command {
                player: 0,
                kind: walk_to(animal, (40, 10)),
            }],
        );
        run_ticks(&mut sim, 5);
        let walking = sim.object(animal).unwrap().grid;
        assert_ne!(walking.x, home.x, "he walks");
        run(&mut sim, 0, evoke_animal_cmd(animal, 0, 441), true);
        assert_eq!(posted(&sim, animal), [(0x120, vec![0, 441], true)]);
        run_ticks(&mut sim, 1);
        let stopped = sim.object(animal).unwrap().grid;
        run_ticks(&mut sim, 10);
        assert_eq!(
            sim.object(animal).unwrap().grid,
            stopped,
            "walk order aborted"
        );
        assert!(!evoking(&sim, animal), "no evocation");
        assert_eq!(evoke_animal(&sim, animal).unwrap().reserve, 0);
        assert!(mana(&sim, animal) < 25, "nothing paid");
    }

    /// TARGET filter of the target buttons: enemy/neutral, fog,
    /// `MM_METHODINDEX 3` (tile distance <= sight/3 of the actor, also in the follow-up filter), building,
    /// `STAND_ONWATER`. Only reads (checksum the same).
    #[test]
    fn target_fits_follows_the_target_filters() {
        let rules = spell_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Trapper: sight 52 -> range 17 tiles.
        let trapper = spawn(&mut sim, TRAPPER, 0, (10, 10));
        let enemy = spawn(&mut sim, COLONIST, 1, (14, 10));
        let own = spawn(&mut sim, COLONIST, 0, (10, 14));
        let neutral = spawn(&mut sim, COLONIST, 8, (12, 13));
        let edge = spawn(&mut sim, COLONIST, 1, (10, 27));
        let beyond = spawn(&mut sim, COLONIST, 1, (10, 28));
        // Medicine man: sight 52 -> 17 tiles; Patriot, Missionary.
        let medicine = spawn(&mut sim, MEDICINE_MAN, 0, (40, 40));
        let near_enemy = spawn(&mut sim, COLONIST, 1, (44, 40));
        let far_enemy = spawn(&mut sim, COLONIST, 1, (40, 60));
        let patriot = spawn(&mut sim, PATRIOT, 0, (30, 10));
        let missionary = spawn(&mut sim, MISSIONARY, 0, (30, 14));
        let monastery = spawn(&mut sim, MONASTERY, 1, (50, 10));
        let ship = spawn(&mut sim, SHIP, 1, (50, 20));
        let enemy_medicine = spawn(&mut sim, MEDICINE_MAN, 1, (58, 58));
        assert_eq!(sim.object(neutral).unwrap().owner(), 8);
        let before = checksum(sim.state);
        let fits = |sim: &mut Sim, unit, button, target| target_fits(sim, 0, unit, button, target);
        // 456: enemy or neutral in range, not the own one.
        assert!(fits(&mut sim, trapper, 456, enemy));
        assert!(!fits(&mut sim, trapper, 456, own));
        assert!(
            fits(&mut sim, trapper, 456, neutral),
            "follow-up filter with reference"
        );
        assert!(fits(&mut sim, trapper, 456, edge), "17 tiles");
        assert!(!fits(&mut sim, trapper, 456, beyond), "18 tiles");
        // 480: CALLMETHOD 3 also for the explosion spell.
        assert!(fits(&mut sim, medicine, 480, near_enemy));
        assert!(!fits(&mut sim, medicine, 480, far_enemy), "outside sight/3");
        // 541: enemy with walk module, without range.
        assert!(fits(&mut sim, missionary, 541, enemy_medicine));
        // 454: enemy building; 455: enemy ship (module 2 with `0x100`).
        assert!(fits(&mut sim, patriot, 454, monastery));
        assert!(!fits(&mut sim, patriot, 454, enemy));
        assert!(fits(&mut sim, patriot, 455, ship));
        assert!(!fits(&mut sim, patriot, 455, monastery));
        // Buttons without an entry.
        assert!(!fits(&mut sim, medicine, 482, near_enemy));
        assert!(!fits(&mut sim, patriot, 999, monastery));
        assert_eq!(checksum(sim.state), before);
        // The right-click filter of the shipped data stays without reference (`targeting.rs`, K-315 open).
        assert_eq!(rules.target_entry(456).unwrap().target.ref_object, 0);

        // Fog: without a tick after creation nobody uncovers anything. The same setup without fog
        // fits.
        for fogged in [true, false] {
            let mut world = if fogged {
                state_fogged(&rules)
            } else {
                crate::module::ability_testkit::state(&rules)
            };
            let mut sim = Sim::new(&mut world, &rules);
            let trapper = spawn(&mut sim, TRAPPER, 0, (10, 10));
            let enemy = spawn(&mut sim, COLONIST, 1, (14, 10));
            let missionary = spawn(&mut sim, MISSIONARY, 0, (30, 14));
            let enemy_medicine = spawn(&mut sim, MEDICINE_MAN, 1, (34, 14));
            assert_eq!(
                target_fits(&mut sim, 0, trapper, 456, enemy),
                !fogged,
                "fog {fogged}"
            );
            // 541 does not check the fog.
            assert!(target_fits(&mut sim, 0, missionary, 541, enemy_medicine));
        }
    }

    // `0x4E` drain mana and `0x37` scout area (re/spells 2.3, 3.6, 9.5).

    /// Research of the buttons 541 (Missionary), 484 (Chief) and 556 (Trapper).
    const DRAIN_RESEARCH: u32 = 2832;
    const CHIEF_SCOUT_RESEARCH: u32 = 3022;
    const TRAPPER_SCOUT_RESEARCH: u32 = 3128;

    fn drain(unit: ObjectId, target: ObjectId, button: i32) -> CommandKind {
        CommandKind::DrainMana {
            unit,
            target,
            button,
        }
    }

    /// `0x37` with the values of the UI (30, 5000, 1) on the tile `tile`.
    fn scout(unit: ObjectId, tile: (i32, i32), button: i32) -> CommandKind {
        CommandKind::ScoutArea {
            unit,
            x: tile.0,
            z: tile.1,
            size: SCOUT_SIZE,
            duration_ms: SCOUT_MS,
            flag: SCOUT_FLAG,
            button,
        }
    }

    /// Waiting timers `0x326` of module 38.
    fn pending_effects(sim: &Sim, id: ObjectId) -> usize {
        let b = &sim.module(id, spell::INDEX).unwrap().base;
        b.timers_ws
            .iter()
            .chain(&b.events)
            .filter(|e| e.msg.code == spell::msg::EFFECT)
            .count()
    }

    /// Vision sources on the tile `tile`.
    fn sources_at(sim: &Sim, tile: (i32, i32)) -> Vec<crate::fog::VisionSource> {
        sim.state
            .fog
            .sources
            .iter()
            .filter(|s| (s.x, s.z_or_hid) == tile)
            .cloned()
            .collect()
    }

    // QUIRK C-304: kept
    #[test]
    fn drain_commands_follow_the_toolbar() {
        let rules = spell_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        unlock(sim.state, 0, DRAIN_RESEARCH);
        let m = spawn(&mut sim, MISSIONARY, 0, (10, 10));
        set_mana(&mut sim, m, 200);
        let enemy = spawn(&mut sim, MEDICINE_MAN, 1, (40, 10));
        let own = spawn(&mut sim, COLONIST, 0, (14, 14));
        let colonist = spawn(&mut sim, COLONIST, 0, (16, 14));
        // Target = actor, own target (TARGET filter 541: enemy), wrong button, colonist as
        // actor (no module 38, MY filter `MM_GUID 1113`), foreign command giver.
        assert_rejected(&mut sim, 0, drain(m, m, 541));
        assert_rejected(&mut sim, 0, drain(m, own, 541));
        assert_rejected(&mut sim, 0, drain(m, enemy, 480));
        assert_rejected(&mut sim, 0, drain(colonist, enemy, 541));
        assert_rejected(&mut sim, 1, drain(m, enemy, 541));
        // Without research 2832 the button is locked.
        {
            let mut state = crate::module::ability_testkit::state(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let m = spawn(&mut sim, MISSIONARY, 0, (10, 10));
            let enemy = spawn(&mut sim, MEDICINE_MAN, 1, (40, 10));
            assert_rejected(&mut sim, 0, drain(m, enemy, 541));
        }
        // Valid during a walk order: synchronous (nothing in the FIFO), no
        // `abort_all_modules`; `direct` clears the queue. The walk order ends through the
        // look FINDTO (argc 7 to the own position).
        let start = sim.object(m).unwrap().grid;
        run(&mut sim, 0, walk_to(m, (10, 30)), true);
        run_ticks(&mut sim, 3);
        assert_ne!(sim.object(m).unwrap().grid, start, "on the way");
        fill_queue(&mut sim, m);
        run(&mut sim, 0, drain(m, enemy, 541), true);
        assert_eq!(queue_len(&sim, m), 0);
        assert!(posted(&sim, m).is_empty(), "synchronous, without FIFO");
        let g = sim.object(m).unwrap().grid;
        let order = walk(&sim, m).unwrap().find_to.clone().expect("look order");
        assert_eq!((order.code, order.args.len()), (0x200, 7));
        assert_eq!((order.arg(0), order.arg(1)), (g.x.0, g.z.0));
        // The unit stands at once; the rotation by 90 degrees to the target (+x) takes two ticks
        // (measured), then 0x226 with lock and timers.
        let locked = |sim: &Sim| sim.object(m).unwrap().status & status::ANIM_LOCKED != 0;
        run_ticks(&mut sim, 1);
        assert!(!locked(&sim) && pending_effects(&sim, m) == 0);
        run_ticks(&mut sim, 1);
        assert!(locked(&sim));
        assert_eq!(pending_effects(&sim, m), 1);
        assert_eq!(sim.object(m).unwrap().grid, g, "stands");
        // A second command (not direct) aborts nothing: the timer of the first stays,
        // the queue too.
        run_ticks(&mut sim, 5);
        fill_queue(&mut sim, m);
        run(&mut sim, 0, drain(m, enemy, 541), false);
        assert_eq!(pending_effects(&sim, m), 2);
        assert_eq!(queue_len(&sim, m), 1);
    }

    // QUIRK K-91, K-313: kept
    #[test]
    fn scouting_reveals_a_vision_source_for_five_seconds() {
        // re/spells 9.5, K-313: Chief 1715, button 484 (cost 60): mana 100 -> 40, vision source
        // (x, z, 30, slot, 5000, flags 7), effect `aufklären.pef` at the tile corner at height 0, with
        // `direct` sound 0x1F at the Chief and empty queue. The vision sources expire by 50 ms at the end
        // of every engine step (re/vision 4.2): after the command step 4950.
        let rules = spell_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        unlock(sim.state, 0, CHIEF_SCOUT_RESEARCH);
        unlock(sim.state, 0, TRAPPER_SCOUT_RESEARCH);
        let chief = spawn(&mut sim, CHIEF, 0, (10, 10));
        set_mana(&mut sim, chief, 100);
        fill_queue(&mut sim, chief);
        let tile = (20, 30);
        let step = |sim: &mut Sim, kind: Option<CommandKind>| {
            let commands: Vec<Command> = kind
                .map(|kind| Command { player: 0, kind })
                .into_iter()
                .collect();
            crate::tick::step(sim.state, sim.rules, &commands)
        };
        let events = step(&mut sim, Some(scout(chief, tile, 484)));
        assert_eq!(mana(&sim, chief), 40);
        let source = crate::fog::VisionSource {
            x: tile.0,
            z_or_hid: tile.1,
            size: 30,
            player: 0,
            remaining_ms: 4950,
            flags: crate::fog::EXPLORE | crate::fog::SEE | crate::fog::CLEAR_UP,
        };
        assert_eq!(sources_at(&sim, tile), std::slice::from_ref(&source));
        let cell = rules.map.cell;
        let effects: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, SimEvent::EffectAt { .. }))
            .cloned()
            .collect();
        assert_eq!(
            effects,
            [SimEvent::EffectAt {
                path: "data/effects/aufklären.pef".into(),
                pos: [
                    tile.0 as f32 * cell[0] as f32,
                    0.0,
                    tile.1 as f32 * cell[2] as f32
                ],
                yaw: 0.0,
                scale: 1.0,
                start_ms: 0,
            }]
        );
        let pos = sim.object(chief).unwrap().world_pos;
        assert!(events.contains(&SimEvent::Sound3d {
            object: chief,
            action: 0x1F,
            flags: 0x42,
            pos,
            guid: 484,
        }));
        assert_eq!(queue_len(&sim, chief), 0);
        // Still there after the 100th step (command step counted, remainder 0), gone in the 101st.
        for _ in 1..100 {
            step(&mut sim, None);
        }
        assert_eq!(
            sources_at(&sim, tile),
            [crate::fog::VisionSource {
                remaining_ms: 0,
                ..source
            }]
        );
        step(&mut sim, None);
        assert!(sources_at(&sim, tile).is_empty());

        // Mana 59 < 60: nothing - no source, no deduction, no event, the queue stays
        // (no rejection, like in the original).
        set_mana(&mut sim, chief, 59);
        fill_queue(&mut sim, chief);
        sim.state.events.clear();
        run(&mut sim, 0, scout(chief, tile, 484), true);
        assert!(sources_at(&sim, tile).is_empty());
        assert_eq!(mana(&sim, chief), 59);
        assert!(sim.state.events.is_empty());
        assert_eq!(queue_len(&sim, chief), 1);

        // Trapper 2015 with button 556: cost 25.
        let trapper = spawn(&mut sim, TRAPPER, 0, (12, 10));
        set_mana(&mut sim, trapper, 150);
        run(&mut sim, 0, scout(trapper, (5, 6), 556), true);
        assert_eq!(mana(&sim, trapper), 125);
        assert_eq!(sources_at(&sim, (5, 6)).len(), 1);
    }

    // QUIRK K-313: kept
    #[test]
    fn scout_commands_follow_the_toolbar() {
        let rules = spell_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        unlock(sim.state, 0, CHIEF_SCOUT_RESEARCH);
        unlock(sim.state, 0, TRAPPER_SCOUT_RESEARCH);
        let chief = spawn(&mut sim, CHIEF, 0, (10, 10));
        set_mana(&mut sim, chief, 100);
        let ok = scout(chief, (20, 30), 484);
        let with = |f: fn(&mut CommandKind)| {
            let mut k = ok.clone();
            f(&mut k);
            k
        };
        // Button 480, size 31, duration 4000, flag 0.
        assert_rejected(&mut sim, 0, scout(chief, (20, 30), 480));
        for k in [
            with(|k| {
                if let CommandKind::ScoutArea { size, .. } = k {
                    *size = 31;
                }
            }),
            with(|k| {
                if let CommandKind::ScoutArea { duration_ms, .. } = k {
                    *duration_ms = 4000;
                }
            }),
            with(|k| {
                if let CommandKind::ScoutArea { flag, .. } = k {
                    *flag = 0;
                }
            }),
        ] {
            assert_rejected(&mut sim, 0, k);
        }
        // Tile outside the map.
        let (w, h) = (rules.map.dim_x, rules.map.dim_z);
        for tile in [(-1, 30), (w, 30), (20, -1), (20, h)] {
            assert_rejected(&mut sim, 0, scout(chief, tile, 484));
        }
        // Chief with 556 (not in his menu), foreign command giver.
        assert_rejected(&mut sim, 0, scout(chief, (20, 30), 556));
        assert_rejected(&mut sim, 1, ok.clone());
        // Locked without research 3022.
        let mut state = crate::module::ability_testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let chief = spawn(&mut sim, CHIEF, 0, (10, 10));
        set_mana(&mut sim, chief, 100);
        assert_rejected(&mut sim, 0, scout(chief, (20, 30), 484));
        // The corner of the map is allowed.
        unlock(sim.state, 0, CHIEF_SCOUT_RESEARCH);
        run(&mut sim, 0, scout(chief, (w - 1, h - 1), 484), true);
        assert_eq!(sources_at(&sim, (w - 1, h - 1)).len(), 1);
    }

    // `0x30`-`0x33`, `0x4F`, `0x5F`, `0x36` kinds 1/2/3 and the target commands of the command bar
    // (re/special-attacks 2.4, 2.5, 3.6, 3.7; re/spells 2.3, 2.5, 3.7, 9.6).

    /// Spell world with all research of the spell buttons for slot 0 and behavior 0 on both
    /// slots (no retaliation).
    fn spell_state(rules: &crate::rules::Rules) -> SimState {
        let mut state = state(rules);
        state.combat.behavior_mode[0] = 0;
        state.combat.behavior_mode[1] = 0;
        for &guid in SPELL_UPGRADE_GUIDS {
            unlock(&mut state, 0, guid);
        }
        state
    }

    fn dynamite_building(unit: ObjectId, target: ObjectId, button: i32) -> CommandKind {
        CommandKind::DynamiteBuilding {
            unit,
            target,
            button,
        }
    }

    fn dynamite_ship(unit: ObjectId, target: ObjectId, button: i32) -> CommandKind {
        CommandKind::DynamiteShip {
            unit,
            target,
            button,
        }
    }

    fn kill_unit(unit: ObjectId, target: ObjectId, button: i32) -> CommandKind {
        CommandKind::KillUnit {
            unit,
            target,
            button,
        }
    }

    fn snipe_unit(unit: ObjectId, target: ObjectId, button: i32) -> CommandKind {
        CommandKind::SnipeUnit {
            unit,
            target,
            button,
        }
    }

    fn damage_building(unit: ObjectId, target: ObjectId, button: i32) -> CommandKind {
        CommandKind::DamageBuilding {
            unit,
            target,
            button,
        }
    }

    /// `0x36` with the target point `p` as `f32` bits (like the UI for 480/481).
    fn cast_at(caster: ObjectId, art: i32, p: [f32; 3], button: i32) -> CommandKind {
        CommandKind::CastSpell {
            caster,
            art,
            anim: p[0].to_bits() as i32,
            idx: p[2].to_bits() as i32,
            button,
        }
    }

    /// World position of the tile center `tile` at height 0.
    fn at_tile(tile: (i32, i32)) -> [f32; 3] {
        [world(tile.0), 0.0, world(tile.1)]
    }

    fn whirl_state(sim: &Sim, id: ObjectId) -> (bool, i32) {
        let s = special_move::special_move(sim, id).expect("module 49");
        (s.busy, s.mode)
    }

    fn smith_pending(sim: &Sim, id: ObjectId, code: u32) -> bool {
        sim.module(id, special_move::INDEX)
            .unwrap()
            .base
            .has_pending_timer(code)
    }

    fn hp(sim: &Sim, id: ObjectId) -> i32 {
        sim.attribute(id, HITPOINTS)
    }

    /// Sounds `(object, action, flags, GUID)` of the events.
    fn sounds(events: &[SimEvent]) -> Vec<(ObjectId, i32, u32, u32)> {
        events
            .iter()
            .filter_map(|e| match e {
                SimEvent::Sound3d {
                    object,
                    action,
                    flags,
                    guid,
                    ..
                } => Some((*object, *action, *flags, *guid)),
                _ => None,
            })
            .collect()
    }

    // QUIRK K-314, C-336: kept
    #[test]
    fn special_attack_commands_post_with_abort() {
        // re/special-attacks 2.4, 4.1: per command `{code, [target, button]}` (`0x5F`: `{0x231, [1, 1,
        // target, cost]}`, `0x4F`: `{0x131, [0, 0]}`) with abort into the FIFO, with `direct` the
        // queue is empty. Each actor walks west beforehand; after the FIFO the order of the module runs
        // and the walk order is aborted (it does not get any further west).
        let rules = spell_rules();
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let monastery = spawn(&mut sim, MONASTERY, 1, (50, 10));
        let ship = spawn(&mut sim, SHIP, 1, (50, 24));
        type Make = fn(ObjectId, ObjectId, i32) -> CommandKind;
        let cases: [(u32, i32, Make, u32); 8] = [
            (PATRIOT, 454, dynamite_building, 0x124),
            (PATRIOT, 455, dynamite_ship, 0x424),
            (TRAPPER, 456, kill_unit, 0x125),
            (SHADOW_WARRIOR, 548, kill_unit, 0x125),
            (SHARPSHOOTER, 457, snipe_unit, 0x425),
            (PISTOLERO, 546, snipe_unit, 0x425),
            (SMITH, 551, damage_building, 0x231),
            (
                SMITH,
                542,
                |unit, _, button| CommandKind::Whirlwind { unit, button },
                0x131,
            ),
        ];
        for (i, (guid, button, make, code)) in cases.into_iter().enumerate() {
            let z = 10 + 6 * i as i32;
            let unit = spawn(&mut sim, guid, 0, (10, z));
            set_mana(&mut sim, unit, 100);
            let target = match button {
                454 | 551 => monastery,
                455 => ship,
                // Victim east in range; for the whirlwind an enemy one tile to the west.
                542 => spawn(&mut sim, COLONIST, 1, (9, z)),
                _ => spawn(&mut sim, COLONIST, 1, (14, z)),
            };
            run(&mut sim, 0, walk_to(unit, (1, z)), true);
            run_ticks(&mut sim, 3);
            let x0 = sim.object(unit).unwrap().grid.x;
            assert!(x0.0 < (10 << 12) + 0x800, "{button}: walks west");
            fill_queue(&mut sim, unit);
            run(&mut sim, 0, make(unit, target, button), true);
            let args = match code {
                0x231 => vec![1, 1, target.as_i32(), 60],
                0x131 => vec![0, 0],
                _ => vec![target.as_i32(), button],
            };
            assert_eq!(posted(&sim, unit), [(code, args, true)], "{button}");
            assert_eq!(queue_len(&sim, unit), 0, "{button}: direct");
            run_ticks(&mut sim, 1);
            assert!(posted(&sim, unit).is_empty(), "{button}: FIFO has run");
            match code {
                0x124 | 0x424 => {
                    let b = bomberman::bomberman(&sim, unit).unwrap();
                    assert_eq!(b.target, target, "{button}");
                }
                0x125 | 0x425 => {
                    assert_eq!(killer::killer(&sim, unit).unwrap().target, target);
                }
                0x231 => {
                    let order = walk(&sim, unit).unwrap().find_to.clone();
                    let args = [target.as_i32(), special_move::WALK_TOLERANCE, 1];
                    assert_eq!(
                        order,
                        Some(Message::new(0x200, &args)),
                        "walk to the building"
                    );
                }
                _ => assert_eq!(whirl_state(&sim, unit), (true, 0), "whirlwind running"),
            }
            let x1 = sim.object(unit).unwrap().grid.x;
            run_ticks(&mut sim, 5);
            let x2 = sim.object(unit).unwrap().grid.x;
            assert!(x2.0 >= x1.0, "{button}: walk order aborted");
        }
        // Not direct (from a queue): it stays.
        let p = spawn(&mut sim, PATRIOT, 0, (30, 60));
        set_mana(&mut sim, p, 100);
        fill_queue(&mut sim, p);
        run(&mut sim, 0, dynamite_building(p, monastery, 454), false);
        assert_eq!(
            posted(&sim, p),
            [(0x124, vec![monastery.as_i32(), 454], true)]
        );
        assert_eq!(queue_len(&sim, p), 1);
    }

    #[test]
    fn target_commands_follow_the_toolbar() {
        // A8: button matching the command kind, actor like the command bar
        // (`check_ability`), target by the TARGET filter of the button (`target_fits`).
        let rules = spell_rules();
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Trapper: sight 52 -> range 17 tiles.
        let trapper = spawn(&mut sim, TRAPPER, 0, (10, 10));
        set_mana(&mut sim, trapper, 150);
        let enemy = spawn(&mut sim, COLONIST, 1, (14, 10));
        let own = spawn(&mut sim, COLONIST, 0, (10, 14));
        let beyond = spawn(&mut sim, COLONIST, 1, (10, 28));
        // Wrong button for the command kind (457 belongs to `0x33`), own target, outside sight/3
        // (18 tiles), foreign command giver, actor without module 37 (colonist).
        assert_rejected(&mut sim, 0, kill_unit(trapper, enemy, 457));
        assert_rejected(&mut sim, 0, snipe_unit(trapper, enemy, 456));
        assert_rejected(&mut sim, 0, kill_unit(trapper, own, 456));
        assert_rejected(&mut sim, 0, kill_unit(trapper, beyond, 456));
        assert_rejected(&mut sim, 1, kill_unit(trapper, enemy, 456));
        assert_rejected(&mut sim, 0, kill_unit(own, enemy, 456));
        // Swimming sharpshooter (MY filter 457: not `SWIMMING`); counter-check without.
        let sharp = spawn(&mut sim, SHARPSHOOTER, 0, (30, 10));
        set_mana(&mut sim, sharp, 100);
        let foe = spawn(&mut sim, COLONIST, 1, (34, 10));
        set_status(&mut sim, sharp, status::SWIMMING);
        assert_rejected(&mut sim, 0, snipe_unit(sharp, foe, 457));
        sim.object_mut(sharp).unwrap().status &= !status::SWIMMING;
        run(&mut sim, 0, snipe_unit(sharp, foe, 457), true);
        assert_eq!(posted(&sim, sharp).len(), 1, "valid without SWIMMING");
        // Mounted Patriot: 454 requires "not ISMOUNTED", 455 does not. 454 on a ship and
        // 455 on a building do not fit the target filter.
        let patriot = spawn(&mut sim, PATRIOT, 0, (30, 30));
        set_mana(&mut sim, patriot, 100);
        let monastery = spawn(&mut sim, MONASTERY, 1, (50, 10));
        let ship = spawn(&mut sim, SHIP, 1, (50, 24));
        assert_rejected(&mut sim, 0, dynamite_building(patriot, ship, 454));
        assert_rejected(&mut sim, 0, dynamite_ship(patriot, monastery, 455));
        assert_rejected(&mut sim, 0, dynamite_ship(patriot, ship, 454));
        set_status(&mut sim, patriot, status::ISMOUNTED);
        assert_rejected(&mut sim, 0, dynamite_building(patriot, monastery, 454));
        run(&mut sim, 0, dynamite_ship(patriot, ship, 455), true);
        assert_eq!(posted(&sim, patriot).len(), 1, "455 valid when mounted");
        // Smith: 542 only as whirlwind, 551 only against enemy buildings.
        let smith = spawn(&mut sim, SMITH, 0, (40, 40));
        set_mana(&mut sim, smith, 100);
        let own_monastery = spawn(&mut sim, MONASTERY, 0, (50, 50));
        assert_rejected(
            &mut sim,
            0,
            CommandKind::Whirlwind {
                unit: smith,
                button: 551,
            },
        );
        assert_rejected(&mut sim, 0, damage_building(smith, monastery, 542));
        assert_rejected(&mut sim, 0, damage_building(smith, own_monastery, 551));
        assert_rejected(&mut sim, 0, damage_building(smith, foe, 551));
        assert_rejected(
            &mut sim,
            0,
            CommandKind::Whirlwind {
                unit: trapper,
                button: 542,
            },
        );
        // 480 on an enemy outside sight/3 of the medicine man (52/3 = 17 tiles) is rejected by the
        // target command (`CastSpell` itself checks no target); inside it fits.
        let medicine = spawn(&mut sim, MEDICINE_MAN, 0, (20, 40));
        let near = spawn(&mut sim, COLONIST, 1, (24, 40));
        let far = spawn(&mut sim, COLONIST, 1, (20, 58));
        let before = checksum(sim.state);
        let err = target_command(&mut sim, 0, medicine, 480, AbilityTarget::Object(far));
        // ID as a number like in the other messages.
        assert_eq!(
            err,
            Err(format!(
                "target {} does not fit the target filter of 480",
                far.0
            ))
        );
        assert!(target_command(&mut sim, 0, medicine, 480, AbilityTarget::Object(near)).is_ok());
        assert_eq!(checksum(sim.state), before);

        // Target in the fog: without a tick after creation nobody uncovers anything.
        let mut state = state_fogged(&rules);
        for &guid in SPELL_UPGRADE_GUIDS {
            unlock(&mut state, 0, guid);
        }
        let mut sim = Sim::new(&mut state, &rules);
        let trapper = spawn(&mut sim, TRAPPER, 0, (10, 10));
        set_mana(&mut sim, trapper, 150);
        let enemy = spawn(&mut sim, COLONIST, 1, (14, 10));
        assert_rejected(&mut sim, 0, kill_unit(trapper, enemy, 456));
        assert!(target_command(&mut sim, 0, trapper, 456, AbilityTarget::Object(enemy)).is_err());

        // Without research 3127, 456 is locked.
        let mut state = crate::module::ability_testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let trapper = spawn(&mut sim, TRAPPER, 0, (10, 10));
        set_mana(&mut sim, trapper, 150);
        let enemy = spawn(&mut sim, COLONIST, 1, (14, 10));
        assert_rejected(&mut sim, 0, kill_unit(trapper, enemy, 456));
    }

    /// `check_cast_spell` fails with `part` in the message, and the command changes nothing.
    fn assert_cast_rejected(sim: &mut Sim, kind: CommandKind, part: &str) {
        let CommandKind::CastSpell {
            caster,
            art,
            anim,
            idx,
            button,
        } = kind
        else {
            unreachable!("only 0x36");
        };
        let check =
            crate::command::upgrade::check_cast_spell(sim, 0, caster, art, anim, idx, button);
        assert_err(check.map(|()| 0), part);
        assert_rejected(sim, 0, kind);
    }

    // QUIRK C-308: kept
    #[test]
    fn cast_spell_accepts_arts_one_to_three() {
        // re/spells 2.3: kind 1 <-> 480, kind 2 <-> 481 with the target point as `f32` bits
        // (finite, on the map), kind 3 <-> 482 with `(0, 0)`; kind 5 only via `0x4E`.
        let rules = spell_rules();
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let m = spawn(&mut sim, MEDICINE_MAN, 0, (10, 10));
        set_mana(&mut sim, m, 100);
        let p = at_tile((14, 10));
        let (w, h) = (
            rules.map.dim_x as f32 * rules.map.cell[0] as f32,
            rules.map.dim_z as f32 * rules.map.cell[2] as f32,
        );
        assert_cast_rejected(
            &mut sim,
            cast_at(m, 2, p, 480),
            "does not match spell kind 2",
        );
        assert_cast_rejected(
            &mut sim,
            cast_at(m, 1, p, 481),
            "does not match spell kind 1",
        );
        assert_cast_rejected(&mut sim, cast_at(m, 5, p, 541), "only via drain mana");
        assert_cast_rejected(&mut sim, cast_at(m, 7, p, 480), "unknown");
        for bad in [
            [f32::NAN, 0.0, p[2]],
            [p[0], 0.0, f32::INFINITY],
            [-1.0, 0.0, p[2]],
            [w, 0.0, p[2]],
            [p[0], 0.0, h],
        ] {
            assert_cast_rejected(&mut sim, cast_at(m, 1, bad, 480), "target point");
        }
        let reincarnate = |anim, idx| CommandKind::CastSpell {
            caster: m,
            art: 3,
            anim,
            idx,
            button: 482,
        };
        assert_cast_rejected(&mut sim, reincarnate(1, 0), "must be 0");
        assert_cast_rejected(&mut sim, reincarnate(0, 2), "must be 0");
        // Accepted: `{0x126, [art, anim, idx, button]}` with abort, `direct` clears.
        fill_queue(&mut sim, m);
        run(&mut sim, 0, cast_at(m, 1, p, 480), true);
        run(&mut sim, 0, cast_at(m, 2, p, 481), true);
        run(&mut sim, 0, reincarnate(0, 0), true);
        let (xb, zb) = (p[0].to_bits() as i32, p[2].to_bits() as i32);
        assert_eq!(
            posted(&sim, m),
            [
                (0x126, vec![1, xb, zb, 480], true),
                (0x126, vec![2, xb, zb, 481], true),
                (0x126, vec![3, 0, 0, 482], true),
            ]
        );
        assert_eq!(queue_len(&sim, m), 0);
        // The corner of the map is allowed (tile `(dim − 1, dim − 1)`).
        let corner = at_tile((rules.map.dim_x - 1, rules.map.dim_z - 1));
        let ok = crate::command::upgrade::check_cast_spell(
            &mut sim,
            0,
            m,
            1,
            corner[0].to_bits() as i32,
            corner[2].to_bits() as i32,
            480,
        );
        assert_eq!(ok, Ok(()));

        // C-308: the ghost evoker 1411 carries `SPELLEXPLOSION | SPELLSTORM`, but has no
        // button 480 in the menu.
        let rules = evoke_rules();
        let mut state = crate::module::ability_testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let evoker = spawn(&mut sim, GHOST_EVOKER, 0, (10, 10));
        set_mana(&mut sim, evoker, 150);
        assert!(sim.object(evoker).unwrap().has_module(spell::INDEX, false));
        assert_cast_rejected(&mut sim, cast_at(evoker, 1, p, 480), "no button 480");
    }

    // QUIRK K-91, K-320: kept
    #[test]
    fn whirlwind_pays_in_the_command() {
        // K-320, re/special-attacks 2.4: `0x4F` deducts the cost of 542 (75) in the command and posts
        // `{0x131, [0, 0]}` with abort; with `direct` sound `(0x1F, 0x42)` with the button as GUID and
        // an empty queue.
        let rules = spell_rules();
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let smith = spawn(&mut sim, SMITH, 0, (10, 10));
        spawn(&mut sim, RANGED, 1, (11, 10));
        set_mana(&mut sim, smith, 100);
        fill_queue(&mut sim, smith);
        sim.state.events.clear();
        let whirl = CommandKind::Whirlwind {
            unit: smith,
            button: 542,
        };
        run(&mut sim, 0, whirl, true);
        assert_eq!(mana(&sim, smith), 25);
        assert_eq!(posted(&sim, smith), [(0x131, vec![0, 0], true)]);
        assert_eq!(queue_len(&sim, smith), 0);
        assert_eq!(sounds(&sim.state.events), [(smith, 0x1F, 0x42, 542)]);

        // In the command tick (step with the command): mana 25 directly afterwards - the regeneration of the
        // smith (0.1 per object tick) does not change the integer value; the module pays nothing.
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let smith = spawn(&mut sim, SMITH, 0, (10, 10));
        spawn(&mut sim, RANGED, 1, (11, 10));
        set_mana(&mut sim, smith, 100);
        let command = Command {
            player: 0,
            kind: CommandKind::Whirlwind {
                unit: smith,
                button: 542,
            },
        };
        crate::tick::step(sim.state, sim.rules, &[command]);
        assert_eq!(mana(&sim, smith), 25, "immediately in the command tick");
        for _ in 0..2 {
            if whirl_state(&sim, smith).0 {
                break;
            }
            crate::tick::step(sim.state, sim.rules, &[]);
        }
        assert_eq!(whirl_state(&sim, smith), (true, 0), "whirlwind running");
        assert_eq!(mana(&sim, smith), 25, "the module deducts nothing");

        // Mana 70 < 75: nothing - no deduction, nothing posted, no sound, the queue stays
        // (no rejection, like in the original).
        let other = spawn(&mut sim, SMITH, 0, (30, 30));
        set_mana(&mut sim, other, 70);
        fill_queue(&mut sim, other);
        sim.state.events.clear();
        run(
            &mut sim,
            0,
            CommandKind::Whirlwind {
                unit: other,
                button: 542,
            },
            true,
        );
        assert_eq!(mana(&sim, other), 70);
        assert!(posted(&sim, other).is_empty());
        assert_eq!(queue_len(&sim, other), 1);
        assert!(sim.state.events.is_empty());
    }

    // QUIRK K-320: kept
    #[test]
    fn a_whirlwind_without_enemies_loses_the_mana() {
        // K-320: no enemy within radius 2 (one 3 tiles away, an own one next to it) -> the
        // mana is gone in the command, the module finds nobody: no animation, no lock, no
        // timers, `busy` 0.
        let rules = spell_rules();
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let smith = spawn(&mut sim, SMITH, 0, (10, 10));
        let far = spawn(&mut sim, RANGED, 1, (13, 10));
        let own = spawn(&mut sim, RANGED, 0, (11, 10));
        set_mana(&mut sim, smith, 100);
        run_ticks(&mut sim, 1);
        let anim = sim.object(smith).unwrap().anim.wanted;
        run(
            &mut sim,
            0,
            CommandKind::Whirlwind {
                unit: smith,
                button: 542,
            },
            true,
        );
        assert_eq!(mana(&sim, smith), 25);
        run_ticks(&mut sim, 2);
        assert!(posted(&sim, smith).is_empty(), "FIFO has run");
        assert_eq!(mana(&sim, smith), 25, "mana lost");
        assert!(!whirl_state(&sim, smith).0);
        assert_eq!(sim.object(smith).unwrap().anim.wanted, anim, "no animation");
        assert_eq!(sim.object(smith).unwrap().status & status::ANIM_LOCKED, 0);
        assert!(!smith_pending(&sim, smith, special_move::msg::HIT));
        assert!(!smith_pending(&sim, smith, special_move::msg::END));
        assert_eq!((hp(&sim, far), hp(&sim, own)), (1000, 1000));
    }

    // QUIRK K-91, K-314, K-320: kept
    #[test]
    fn damage_building_checks_but_does_not_pay() {
        // re/spells 9.6, K-314: mana 100 >= 60 -> `{0x231, [1, 1, target, 60]}` with abort, no deduction in the
        // command, with `direct` sound `(0x1F, 0x42)` with GUID 551 and an empty queue; mana 59 ->
        // nothing.
        let rules = spell_rules();
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let smith = spawn(&mut sim, SMITH, 0, (10, 10));
        let monastery = spawn(&mut sim, MONASTERY, 1, (18, 10));
        set_mana(&mut sim, smith, 100);
        fill_queue(&mut sim, smith);
        sim.state.events.clear();
        run(&mut sim, 0, damage_building(smith, monastery, 551), true);
        assert_eq!(
            posted(&sim, smith),
            [(0x231, vec![1, 1, monastery.as_i32(), 60], true)]
        );
        assert_eq!(mana(&sim, smith), 100);
        assert_eq!(queue_len(&sim, smith), 0);
        assert_eq!(sounds(&sim.state.events), [(smith, 0x1F, 0x42, 551)]);

        let other = spawn(&mut sim, SMITH, 0, (10, 20));
        set_mana(&mut sim, other, 59);
        fill_queue(&mut sim, other);
        sim.state.events.clear();
        run(&mut sim, 0, damage_building(other, monastery, 551), true);
        assert!(posted(&sim, other).is_empty());
        assert_eq!(mana(&sim, other), 59);
        assert_eq!(queue_len(&sim, other), 1);
        assert!(sim.state.events.is_empty());
    }

    // QUIRK K-87: deviation
    #[test]
    fn damage_building_without_values_walks_but_never_hits() {
        // K-87 (deviation from the original): without a value set of the button GUID 551 the command passes
        // the cost `i32::MAX` instead of the command pointer: the smith walks there, but never hammers,
        // the mana stays.
        let mut rules = spell_rules();
        rules.defaults.remove(&551);
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let smith = spawn(&mut sim, SMITH, 0, (10, 10));
        let monastery = spawn(&mut sim, MONASTERY, 1, (18, 10));
        set_mana(&mut sim, smith, 100);
        let start = sim.object(smith).unwrap().grid.x;
        run(&mut sim, 0, damage_building(smith, monastery, 551), true);
        assert_eq!(
            posted(&sim, smith),
            [(0x231, vec![1, 1, monastery.as_i32(), i32::MAX], true)]
        );
        let mut hits = 0;
        for _ in 0..80 {
            let events = run_ticks_collect(&mut sim, 1);
            hits += events
                .iter()
                .filter(|e| matches!(e, SimEvent::DamageNumber { .. }))
                .count();
            assert!(!whirl_state(&sim, smith).0, "no hit");
        }
        assert!(sim.object(smith).unwrap().grid.x.0 > start.0, "walks there");
        assert_eq!(hits, 0);
        assert_eq!(hp(&sim, monastery), 10_000);
        assert_eq!(mana(&sim, smith), 100);
    }

    // QUIRK C-336: kept
    #[test]
    fn target_command_builds_the_original_commands() {
        // `Input_IssueTargetAction` (`0x0044D350`, re/special-attacks 2.4, re/spells 2.3): every
        // target button with a fitting target yields the command of the original; 480/481 with the world
        // position of the target as `f32` bits, 484/556 with the tile of the ground point and 30/5000/1.
        // Only reads: the checksum stays.
        let rules = spell_rules();
        let mut state = spell_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let medicine = spawn(&mut sim, MEDICINE_MAN, 0, (10, 10));
        let near = spawn(&mut sim, COLONIST, 1, (14, 10));
        let monastery = spawn(&mut sim, MONASTERY, 1, (14, 20));
        let ship = spawn(&mut sim, SHIP, 1, (30, 40));
        let chief = spawn(&mut sim, CHIEF, 0, (40, 10));
        let missionary = spawn(&mut sim, MISSIONARY, 0, (40, 20));
        let enemy_medicine = spawn(&mut sim, MEDICINE_MAN, 1, (44, 20));
        let patriot = spawn(&mut sim, PATRIOT, 0, (20, 40));
        let trapper = spawn(&mut sim, TRAPPER, 0, (10, 4));
        let shadow = spawn(&mut sim, SHADOW_WARRIOR, 0, (12, 6));
        let sharp = spawn(&mut sim, SHARPSHOOTER, 0, (16, 6));
        let pistolero = spawn(&mut sim, PISTOLERO, 0, (18, 8));
        let smith = spawn(&mut sim, SMITH, 0, (20, 16));
        let before = checksum(sim.state);
        let near_pos = crate::combat::hit_zone::world_pos(&sim, near).unwrap();
        let monastery_pos = crate::combat::hit_zone::world_pos(&sim, monastery).unwrap();
        let bits = |p: [f32; 3]| (p[0].to_bits() as i32, p[2].to_bits() as i32);
        let (nx, nz) = bits(near_pos);
        let (mx, mz) = bits(monastery_pos);
        let (w, h) = (
            rules.map.dim_x as f32 * rules.map.cell[0] as f32,
            rules.map.dim_z as f32 * rules.map.cell[2] as f32,
        );
        let object = AbilityTarget::Object;
        let mut cmd = |unit, button, target| target_command(&mut sim, 0, unit, button, target);
        assert_eq!(
            cmd(medicine, 480, object(near)),
            Ok(CommandKind::CastSpell {
                caster: medicine,
                art: 1,
                anim: nx,
                idx: nz,
                button: 480,
            })
        );
        assert_eq!(
            cmd(medicine, 481, object(monastery)),
            Ok(CommandKind::CastSpell {
                caster: medicine,
                art: 2,
                anim: mx,
                idx: mz,
                button: 481,
            })
        );
        assert_eq!(
            cmd(missionary, 541, object(enemy_medicine)),
            Ok(drain(missionary, enemy_medicine, 541))
        );
        assert_eq!(
            cmd(patriot, 454, object(monastery)),
            Ok(dynamite_building(patriot, monastery, 454))
        );
        assert_eq!(
            cmd(patriot, 455, object(ship)),
            Ok(dynamite_ship(patriot, ship, 455))
        );
        for (unit, button) in [(trapper, 456), (shadow, 548)] {
            assert_eq!(
                cmd(unit, button, object(near)),
                Ok(kill_unit(unit, near, button))
            );
        }
        for (unit, button) in [(sharp, 457), (pistolero, 546)] {
            assert_eq!(
                cmd(unit, button, object(near)),
                Ok(snipe_unit(unit, near, button))
            );
        }
        assert_eq!(
            cmd(smith, 551, object(monastery)),
            Ok(damage_building(smith, monastery, 551))
        );
        // Ground point: tile = `Fx::from_f32(v / cell) >> 12`.
        let ground = AbilityTarget::Ground {
            x: 20.0 * 128.0 + 127.5,
            z: 30.0 * 128.0 + 0.25,
        };
        assert_eq!(cmd(chief, 484, ground), Ok(scout(chief, (20, 30), 484)));
        assert_eq!(cmd(trapper, 556, ground), Ok(scout(trapper, (20, 30), 556)));
        // Wrong target kind, button without a target command, ground point outside the map, unfitting target.
        let ground_near = AbilityTarget::Ground {
            x: near_pos[0],
            z: near_pos[2],
        };
        assert!(cmd(trapper, 456, ground_near).is_err());
        assert!(cmd(medicine, 480, ground_near).is_err());
        assert!(cmd(chief, 484, object(near)).is_err());
        assert!(cmd(medicine, 482, object(near)).is_err());
        assert!(cmd(smith, 542, object(near)).is_err());
        assert!(cmd(chief, 999, ground).is_err());
        for (x, z) in [(-1.0, 100.0), (100.0, f32::NAN), (w, 100.0), (100.0, h)] {
            assert!(cmd(chief, 484, AbilityTarget::Ground { x, z }).is_err());
        }
        assert!(cmd(patriot, 454, object(ship)).is_err());
        assert!(cmd(medicine, 481, object(near)).is_err());
        assert_eq!(checksum(sim.state), before);
    }

    /// Headhunter world with research 3326 for slot 0 (`REQUIRED_GUID` of the
    /// button 540).
    fn rob_state(rules: &crate::rules::Rules) -> SimState {
        let mut state = hunter_state(rules);
        unlock(&mut state, 0, ROB_UPGRADE);
        state
    }

    fn rob(unit: ObjectId, button: i32) -> CommandKind {
        CommandKind::RobCorpses { unit, button }
    }

    // QUIRK K-91: kept
    #[test]
    fn fifteen_robs_from_full_mana() {
        // re/headhunter-robbing 9.3, K-91: mana 150, cost 10 (config 15 of the button 540) -> 15
        // commands; the 16th does nothing (no rejection, no deduction, no sound, nothing posted).
        // All 16 in **one** step (the Undertaker has no module 38 anyway).
        let rules = hunter_rules();
        let mut state = rob_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = spawn(&mut sim, UNDERTAKER, 0, (10, 10));
        set_mana(&mut sim, u, 150);
        let commands: Vec<Command> = (0..16)
            .map(|_| Command {
                player: 0,
                kind: rob(u, ROB_BUTTON),
            })
            .collect();
        let events = crate::tick::step(sim.state, sim.rules, &commands);
        assert_eq!(mana(&sim, u), 0);
        assert_eq!(
            sounds(&events),
            vec![(u, 0x1F, 0x42, 540); 15],
            "15 accepted (direct: sound per command), the 16th not"
        );
        // Without mana: nothing, not even posted.
        let fifo = posted(&sim, u);
        sim.state.events.clear();
        run(&mut sim, 0, rob(u, ROB_BUTTON), true);
        assert_eq!(posted(&sim, u), fifo);
        assert_eq!(mana(&sim, u), 0);
        assert!(sim.state.events.is_empty());
    }

    // QUIRK K-91: kept
    // QUIRK K-317: deviation
    #[test]
    fn rob_commands_follow_the_toolbar() {
        // Deviating from K-317 (A8): `0x4C` like the command bar - own, living unit with module 48
        // and unlocked button 540 (research 3326); button != 540 rejected. Accepted: cost
        // in the command (K-91), `{0x130}` with abort; with `direct` sound `(0x1F, 0x42)` with GUID 540 and
        // an empty queue.
        let rules = hunter_rules();
        // Without research 3326 the button is locked.
        let mut locked = hunter_state(&rules);
        let mut sim = Sim::new(&mut locked, &rules);
        let u = spawn(&mut sim, UNDERTAKER, 0, (10, 10));
        set_mana(&mut sim, u, 150);
        assert_rejected(&mut sim, 0, rob(u, ROB_BUTTON));

        let mut state = rob_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = spawn(&mut sim, UNDERTAKER, 0, (10, 10));
        let sanders = spawn(&mut sim, SANDERS, 0, (12, 10));
        let colonist = spawn(&mut sim, COLONIST, 0, (14, 10));
        let foreign = spawn(&mut sim, UNDERTAKER, 1, (16, 10));
        let dead = spawn(&mut sim, UNDERTAKER, 0, (18, 10));
        for id in [u, sanders, colonist, foreign, dead] {
            set_mana(&mut sim, id, 150);
        }
        crate::messaging::send(&mut sim, dead, &Message::new(0x110, &[]));
        assert_ne!(sim.object(dead).unwrap().status & status::DIED, 0);
        assert_rejected(&mut sim, 0, rob(colonist, ROB_BUTTON));
        assert_rejected(&mut sim, 0, rob(foreign, ROB_BUTTON));
        assert_rejected(&mut sim, 1, rob(u, ROB_BUTTON));
        assert_rejected(&mut sim, 0, rob(dead, ROB_BUTTON));
        assert_rejected(&mut sim, 0, rob(u, 541));
        assert_rejected(&mut sim, 0, rob(u, 0));

        // Direct: mana -10, sound, queue empty, `{0x130}` with abort.
        fill_queue(&mut sim, u);
        sim.state.events.clear();
        run(&mut sim, 0, rob(u, ROB_BUTTON), true);
        assert_eq!(mana(&sim, u), 140);
        assert_eq!(posted(&sim, u), [(0x130, vec![], true)]);
        assert_eq!(queue_len(&sim, u), 0);
        assert_eq!(sounds(&sim.state.events), [(u, 0x1F, 0x42, 540)]);
        // Not direct (from the queue): no sound, the queue stays.
        set_mana(&mut sim, sanders, 100);
        fill_queue(&mut sim, sanders);
        sim.state.events.clear();
        run(&mut sim, 0, rob(sanders, ROB_BUTTON), false);
        assert_eq!(mana(&sim, sanders), 90);
        assert_eq!(posted(&sim, sanders), [(0x130, vec![], true)]);
        assert_eq!(queue_len(&sim, sanders), 1);
        assert!(sounds(&sim.state.events).is_empty());
    }
}
