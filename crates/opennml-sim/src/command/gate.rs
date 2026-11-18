//! Gate commands (re/arming-gates 2.C, 10.4): `0x19` open gate (`GameWorld_CmdOpenGate` `0x004768A0`)
//! and `0x1A` close gate (`GameWorld_CmdCloseGate` `0x00476900`).
//!
//! The original sends `OpenDoor` (`0x2303`) or `CloseDoor` (`0x2403`) to the gate unchecked and
//! additionally `0xC029`/`0xD029` to module 41 (K-294, dropped in the reimplementation: the module has
//! no effect in the shipped data). Like the command bar, the reimplementation checks (A8, W-269): an
//! own, living building with `TOR`; `0x19` only with `TORCLOSE`, `0x1A` only with `TOROPEN` (the command
//! bar shows 430 only with `TORCLOSE`, 431 only with `TOROPEN`; so never for a construction site); the
//! button 430 or 431 is in the menu of the type and is not locked.

use super::validate::{button_locked, is_dead, menu_button};
use super::{CommandKind, owns, reject, script_path};
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::module::building::{self, flags, msg};
use crate::sim::Sim;

/// Command bar button "Open gate" (`BUTTONMSG 430`, active with `TORCLOSE`).
pub const OPEN_BUTTON: i32 = 430;
/// Command bar button "Close gate" (`BUTTONMSG 431`, active with `TOROPEN`).
pub const CLOSE_BUTTON: i32 = 431;

/// Entry point for the commands `0x19`/`0x1A` (`command::execute` routes them here). Both run
/// synchronously, empty no queue and send no acknowledgment.
pub fn execute(sim: &mut Sim, player: u8, kind: &CommandKind, _direct: bool) {
    let (gate, open, what) = match *kind {
        CommandKind::OpenGate { gate } => (gate, true, "Open gate"),
        CommandKind::CloseGate { gate } => (gate, false, "Close gate"),
        _ => unreachable!("gate::execute only for 0x19/0x1A"),
    };
    if let Err(why) = check_gate(sim, player, gate, open) {
        reject(sim, player, what, why);
        return;
    }
    // `GameWorld_CmdOpenGate`: `S({0x2303})`; `GameWorld_CmdCloseGate`: `S({0x2403})`. The
    // messages `0xC029`/`0xD029` to module 41 are dropped (K-294).
    let code = if open {
        msg::OPEN_DOOR
    } else {
        msg::CLOSE_DOOR
    };
    send(sim, gate, &Message::new(code, &[]));
}

/// Check of `0x19` (`open`) or `0x1A`: an own, living building with `TOR` in the matching state
/// (`TORCLOSE` for opening, `TOROPEN` for closing) whose type menu has the button 430 or 431 and
/// does not lock it. Error: the reason for the log.
///
/// **Script path** ([`script_path`], script actions 165/166, E-216, re/commands 9): the original
/// fetches the gate with `GetObjectById(id, 0)` and sends `OpenDoor`/`CloseDoor` without any further
/// check (`0x004768A0`, `0x00476900`); ownership, death, gate flag, state open/closed, menu button
/// and lock are dropped. What remains: the object exists and has a building module (otherwise the
/// message has no receiver). A building without the gate flag opens or closes like in the original
/// (`PropertyBuilding_OpenDoor` only checks "already open"); robustness: test
/// `gate_165_on_a_building_without_gate_flag_works_like_the_original` (headquarters: no panic,
/// the savegame loads).
pub fn check_gate(sim: &Sim, player: u8, gate: ObjectId, open: bool) -> Result<(), String> {
    if !owns(sim, player, gate) {
        return Err(if script_path(sim) {
            "gate not present".into()
        } else {
            "gate not present, foreign or dead".into()
        });
    }
    let Some(guid) = sim.object(gate).map(|o| o.guid) else {
        return Err("gate not present".into());
    };
    // The bit word like `validate::is_finished`: `ModuleBase::has_flags` masks out the low byte.
    let Some(module) = sim.module(gate, building::INDEX) else {
        return Err("object is not a building".into());
    };
    if script_path(sim) {
        return Ok(());
    }
    if is_dead(sim, gate) {
        return Err("gate is dying or destroyed".into());
    }
    if !module.base.has_flags(flags::TOR) {
        return Err("building is not a gate".into());
    }
    let (state_flag, button_msg) = if open {
        (flags::TORCLOSE, OPEN_BUTTON)
    } else {
        (flags::TOROPEN, CLOSE_BUTTON)
    };
    if !module.base.has_flags(state_flag) {
        return Err(if open {
            "gate is not closed".into()
        } else {
            "gate is not open".into()
        });
    }
    let Some(button) = menu_button(sim.rules, guid, |b| b.msg == button_msg) else {
        return Err(format!("no button {button_msg} in the menu of the type"));
    };
    if button_locked(sim, i32::from(player), button) {
        return Err(format!("button {button_msg} locked"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_formats::gifcon::ToolbarButton;

    use crate::command::{Command, execute as run_command};
    use crate::module::building::fort_testkit::*;
    use crate::module::building::gate::close_door;
    use crate::module::building::{building_mut, flags};
    use crate::rules::Rules;
    use crate::save::checksum;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};

    fn open_cmd(player: u8, gate: ObjectId) -> Command {
        Command {
            player,
            kind: CommandKind::OpenGate { gate },
        }
    }

    fn close_cmd(player: u8, gate: ObjectId) -> Command {
        Command {
            player,
            kind: CommandKind::CloseGate { gate },
        }
    }

    /// Gate state like [`check_gate`]: `(TOROPEN, TORCLOSE)` of the building module.
    fn state_of(sim: &Sim, id: ObjectId) -> (bool, bool) {
        let m = sim.module(id, building::INDEX).expect("building module");
        (
            m.base.has_flags(flags::TOROPEN),
            m.base.has_flags(flags::TORCLOSE),
        )
    }

    /// Executes the command; checksum before and after.
    fn run(sim: &mut Sim, command: &Command) -> (u64, u64) {
        let before = checksum(sim.state);
        run_command(sim, command, true);
        (before, checksum(sim.state))
    }

    /// The command is rejected: the check reports an error and the checksum stays the same
    /// (a state change of an already open or closed gate would otherwise be invisible, because
    /// `OpenDoor`/`CloseDoor` without forcing do nothing there).
    fn assert_rejected(sim: &mut Sim, command: &Command) {
        let (gate, open) = match command.kind {
            CommandKind::OpenGate { gate } => (gate, true),
            CommandKind::CloseGate { gate } => (gate, false),
            ref k => panic!("not a gate command: {k:?}"),
        };
        assert!(
            check_gate(sim, command.player, gate, open).is_err(),
            "{command:?} not rejected"
        );
        let (before, after) = run(sim, command);
        assert_eq!(before, after, "{command:?} changes the state");
    }

    /// Finished gate (open) of `slot` at a free point of the 64 x 64 world.
    fn gate_at(sim: &mut Sim, slot: i32, tile_x: i32) -> ObjectId {
        spawn_at(sim, GATE, slot, (tile_x as f32 * 128.0, 40.5 * 128.0), 0.0)
    }

    /// Gate construction site (not finished, neither open nor closed).
    fn gate_site(sim: &mut Sim, slot: i32, tile_x: i32) -> ObjectId {
        let mut desc = ObjectDesc::new(GATE, slot, [tile_x as f32 * 128.0, 0.0, 50.5 * 128.0]);
        create_object(sim, &mut desc).expect("gate created")
    }

    /// Test world; the menu of the headquarters additionally lists the gate buttons 430 and 431, so
    /// that for a building without `TOR` only the gate check rejects.
    fn fresh() -> (Rules, SimState) {
        let mut rules = rules();
        let gate_buttons: Vec<ToolbarButton> = rules.toolbar.menus[&(GATE as i32)]
            .iter()
            .filter(|b| b.msg == OPEN_BUTTON || b.msg == CLOSE_BUTTON)
            .cloned()
            .collect();
        assert_eq!(gate_buttons.len(), 2);
        rules
            .toolbar
            .menus
            .get_mut(&(HQ as i32))
            .expect("menu of the headquarters")
            .extend(gate_buttons);
        let state = state(&rules);
        (rules, state)
    }

    /// Rules of the test world whose gate command bar `f` changes (remove a button, set `REQUIRED_GUID`).
    fn rules_with_gate_menu(f: impl FnOnce(&mut Vec<ToolbarButton>)) -> Rules {
        let mut r = rules();
        f(r.toolbar
            .menus
            .get_mut(&(GATE as i32))
            .expect("gate command bar"));
        r
    }

    // QUIRK W-269: deviation
    #[test]
    fn gate_commands_check_owner_state_and_button() {
        let (rules, mut first) = fresh();
        let mut sim = Sim::new(&mut first, &rules);
        let own = gate_at(&mut sim, 0, 10);
        assert_eq!(state_of(&sim, own), (true, false), "finished and open");

        // Foreign gate (slot 1), commands from slot 0.
        let foreign = gate_at(&mut sim, 1, 20);
        assert_rejected(&mut sim, &close_cmd(0, foreign));
        assert_eq!(state_of(&sim, foreign), (true, false));
        // Unknown ID.
        assert_rejected(&mut sim, &close_cmd(0, ObjectId::NONE));
        assert_rejected(&mut sim, &open_cmd(0, ObjectId::NONE));

        // Gate under construction: neither `TOROPEN` nor `TORCLOSE`.
        let site = gate_site(&mut sim, 0, 30);
        assert_eq!(state_of(&sim, site), (false, false));
        assert_rejected(&mut sim, &open_cmd(0, site));
        assert_rejected(&mut sim, &close_cmd(0, site));
        assert_eq!(state_of(&sim, site), (false, false));

        // Opening an open gate.
        assert_rejected(&mut sim, &open_cmd(0, own));
        assert_eq!(state_of(&sim, own), (true, false));

        // Closing a closed gate (close it by command first).
        let (c0, c1) = run(&mut sim, &close_cmd(0, own));
        assert_ne!(c0, c1, "a valid close changes the state");
        assert_eq!(state_of(&sim, own), (false, true));
        assert_rejected(&mut sim, &close_cmd(0, own));
        assert_eq!(state_of(&sim, own), (false, true));

        // Building without `TOR`: the headquarters.
        let hq = spawn(&mut sim, HQ, 0, (44, 20));
        assert_rejected(&mut sim, &close_cmd(0, hq));
        assert_rejected(&mut sim, &open_cmd(0, hq));
        // Even with both state bits (merely invented) the gate check rejects.
        sim.module_mut(hq, building::INDEX)
            .expect("module")
            .base
            .flags |= flags::TOROPEN | flags::TORCLOSE;
        assert_rejected(&mut sim, &close_cmd(0, hq));
        assert_rejected(&mut sim, &open_cmd(0, hq));
        // Not a building: a colonist.
        let colonist = spawn(&mut sim, COLONIST, 0, (48, 20));
        assert_rejected(&mut sim, &close_cmd(0, colonist));
        assert_rejected(&mut sim, &open_cmd(0, colonist));

        // Dead gate (`m_IsDead`): otherwise able to open (closed).
        let dying = gate_at(&mut sim, 0, 52);
        run(&mut sim, &close_cmd(0, dying));
        assert_eq!(state_of(&sim, dying), (false, true));
        building_mut(&mut sim, dying).expect("building").is_dead = true;
        assert_rejected(&mut sim, &open_cmd(0, dying));
        assert_eq!(state_of(&sim, dying), (false, true));

        // Valid: the closed own gate opens.
        let (c0, c1) = run(&mut sim, &open_cmd(0, own));
        assert_ne!(c0, c1);
        assert_eq!(state_of(&sim, own), (true, false));

        // Type without button 431 (close) or 430 (open) in the menu of the gate.
        let rules = rules_with_gate_menu(|menu| menu.retain(|b| b.msg != CLOSE_BUTTON));
        let mut without_close = state(&rules);
        let mut sim = Sim::new(&mut without_close, &rules);
        let gate = gate_at(&mut sim, 0, 10);
        assert_rejected(&mut sim, &close_cmd(0, gate));
        assert_eq!(state_of(&sim, gate), (true, false));
        let rules = rules_with_gate_menu(|menu| menu.retain(|b| b.msg != OPEN_BUTTON));
        let mut without_open = state(&rules);
        let mut sim = Sim::new(&mut without_open, &rules);
        let gate = gate_at(&mut sim, 0, 10);
        close_door(&mut sim, gate, false);
        assert_eq!(state_of(&sim, gate), (false, true));
        assert_rejected(&mut sim, &open_cmd(0, gate));
        assert_eq!(state_of(&sim, gate), (false, true));
    }

    #[test]
    fn gate_commands_respect_a_locked_button() {
        // Button 431 with `REQUIRED_GUID` headquarters: locked while its object counter is 0.
        let rules = rules_with_gate_menu(|menu| {
            let b = menu.iter_mut().find(|b| b.msg == CLOSE_BUTTON).unwrap();
            b.required[0] = HQ as i32;
        });
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gate = gate_at(&mut sim, 0, 10);
        assert_rejected(&mut sim, &close_cmd(0, gate));
        assert_eq!(state_of(&sim, gate), (true, false));
        // With a headquarters the button is free.
        spawn(&mut sim, HQ, 0, (44, 20));
        let (c0, c1) = run(&mut sim, &close_cmd(0, gate));
        assert_ne!(c0, c1);
        assert_eq!(state_of(&sim, gate), (false, true));
        // The object counter of another slot does not count: slot 1 has no headquarters.
        let other = gate_at(&mut sim, 1, 20);
        assert_rejected(&mut sim, &close_cmd(1, other));
        assert_eq!(state_of(&sim, other), (true, false));
    }

    #[test]
    fn close_and_open_commands_switch_the_gate() {
        let (rules, mut state) = fresh();
        let mut sim = Sim::new(&mut state, &rules);
        let gate = gate_at(&mut sim, 0, 10);
        assert_eq!(state_of(&sim, gate), (true, false));
        run(&mut sim, &close_cmd(0, gate));
        assert_eq!(state_of(&sim, gate), (false, true));
        run(&mut sim, &open_cmd(0, gate));
        assert_eq!(state_of(&sim, gate), (true, false));
        // The building stays a gate and lives.
        assert!(!is_dead(&sim, gate));
    }
}
