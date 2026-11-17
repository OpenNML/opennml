//! Tent commands (re/tents 2.4, 3.4, 10.4): `0x2D` Pack tent
//! (`GameWorld_CmdPackTent` `0x00477170`) and `0x2E` Unpack tent (`GameWorld_CmdUnpackTent`
//! `0x004771B0`).
//!
//! The original checks neither owner nor type nor state. The reimplementation checks like the UI
//! (A8): `0x2D` only on an own, selectable tent (module 42
//! switched on) with the button 452 unlocked in the menu of its type, `0x2E` only on an own
//! travois with flag `TRANSPORTINGTENT` and the button 453 unlocked, target on the map. This
//! way a player can no longer reach the second travois of an already hidden tent (W-247, the
//! simulation stays faithful to the original), and the ineffective button 452 of the storage camp 1603 (W-262)
//! is rejected. There is no space or sight check of the target in the command, as in the original;
//! the space check runs on arrival (`0x22A`), the cursor shows only the preview.

use super::validate::{button_locked, menu_button};
use super::{CommandKind, owns, reject};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::tent::{self, transport};
use crate::motion::NO_DIRECTION;
use crate::sim::Sim;

/// Command bar button "Pack tent" (`BUTTONMSG 452`).
pub const PACK_BUTTON: i32 = 452;
/// Command bar button "Unpack tent" (`BUTTONMSG 453`).
pub const UNPACK_BUTTON: i32 = 453;

/// Entry point for the commands `0x2D`/`0x2E` (`command::execute` routes them here). Both run
/// synchronously, empty no queue and do not acknowledge (re/tents 2.4).
pub fn execute(sim: &mut Sim, player: u8, kind: &CommandKind, _direct: bool) {
    match *kind {
        CommandKind::PackTent { tent } => {
            if let Err(why) = check_pack_tent(sim, player, tent) {
                reject(sim, player, "Pack tent", why);
                return;
            }
            // `GameWorld_CmdPackTent`: `S({0x12A, 0})`.
            send(sim, tent, &Message::new(tent::msg::PACK, &[]));
        }
        CommandKind::UnpackTent { unit, x, z } => {
            if let Err(why) = check_unpack_tent(sim, player, unit, x, z) {
                reject(sim, player, "Unpack tent", why);
                return;
            }
            // `GameWorld_CmdUnpackTent`: FINDTO `{0x200, x, z, −1, direction any, −1, 1}` with success
            // `{0x223, x, z}` and failure `{0x323}` (re/tents 2.4).
            let env = Envelope {
                msg: Message::new(0x200, &[x, z, -1, NO_DIRECTION, -1, 1]),
                success: Some(Message::new(transport::msg::ARRIVED, &[x, z])),
                fault: Some(Message::new(transport::msg::WALK_FAILED, &[])),
                success_target: ObjectId::NONE,
                fault_target: ObjectId::NONE,
            };
            dispatch_command(sim, unit, &env);
        }
        _ => unreachable!("tent::execute only for 0x2D/0x2E"),
    }
}

/// Check of `0x2D`: own, living, selectable object with module 42 switched on whose type menu
/// has the button 452 and is not locked. A hidden (packed) tent is unselectable
/// and is rejected (W-247).
pub fn check_pack_tent(sim: &Sim, player: u8, tent: ObjectId) -> Result<(), String> {
    if !owns(sim, player, tent) {
        return Err("tent not present, foreign or dead".into());
    }
    let Some(o) = sim.object(tent) else {
        return Err("tent not present".into());
    };
    if !o.has_module(tent::INDEX, false) {
        return Err("object is not a tent (module 42 missing)".into());
    }
    if !o.selectable {
        return Err("tent is not selectable (already packed)".into());
    }
    let Some(button) = menu_button(sim.rules, o.guid, |b| b.msg == PACK_BUTTON) else {
        return Err("no button \"Pack tent\" in the menu of the type".into());
    };
    if button_locked(sim, i32::from(player), button) {
        return Err("button \"Pack tent\" locked".into());
    }
    Ok(())
}

/// Check of `0x2E`: own, living object with module 35 and flag `TRANSPORTINGTENT` whose type menu
/// has the button 453 and is not locked; the target `(x, z)` (fixed-point tiles) lies
/// on the map. Space and sight are not checked by the command (re/tents 3.4).
pub fn check_unpack_tent(
    sim: &Sim,
    player: u8,
    unit: ObjectId,
    x: i32,
    z: i32,
) -> Result<(), String> {
    if !owns(sim, player, unit) {
        return Err("travois not present, foreign or dead".into());
    }
    let Some(o) = sim.object(unit) else {
        return Err("travois not present".into());
    };
    let carries = o
        .module(transport::INDEX)
        .is_some_and(|m| m.base.flags & transport::TRANSPORTING_TENT != 0);
    if !carries {
        return Err("object carries no tent".into());
    }
    let Some(button) = menu_button(sim.rules, o.guid, |b| b.msg == UNPACK_BUTTON) else {
        return Err("no button \"Unpack tent\" in the menu of the type".into());
    };
    if button_locked(sim, i32::from(player), button) {
        return Err("button \"Unpack tent\" locked".into());
    }
    let map = &sim.rules.map;
    let inside = |v: i32, dim: i32| (0..i64::from(dim) << 12).contains(&i64::from(v));
    if !inside(x, map.dim_x) || !inside(z, map.dim_z) {
        return Err(format!("target ({x}, {z}) outside the map"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use nml_formats::gifcon::parse_toolbar;

    use super::*;
    use crate::command::{Command, CommandKind, execute};
    use crate::module::ModuleKind;
    use crate::module::tent::status;
    use crate::module::tent::testkit::*;
    use crate::object::status as object_status;
    use crate::rules::Rules;
    use crate::save::checksum;
    use crate::state::SimState;

    /// Snapped position of the tents (re/tents 9.2).
    const SNAP: (f32, f32) = (5000.0, 5000.0);

    fn pack_cmd(player: u8, tent: ObjectId) -> Command {
        Command {
            player,
            kind: CommandKind::PackTent { tent },
        }
    }

    fn unpack_cmd(player: u8, unit: ObjectId, x: i32, z: i32) -> Command {
        Command {
            player,
            kind: CommandKind::UnpackTent { unit, x, z },
        }
    }

    fn tent_state(sim: &Sim, id: ObjectId) -> (i32, ObjectId) {
        match &sim.module(id, tent::INDEX).expect("module 42").kind {
            ModuleKind::Tent(t) => (t.status, t.travois),
            k => panic!("no tent: {k:?}"),
        }
    }

    fn travois_count(sim: &Sim, slot: u8) -> usize {
        sim.state.tables[usize::from(slot)]
            .objects
            .iter()
            .flatten()
            .filter(|o| o.guid == TRAVOIS)
            .count()
    }

    fn flags_35(sim: &Sim, id: ObjectId) -> u32 {
        sim.module(id, transport::INDEX).map_or(0, |m| m.base.flags)
    }

    fn object_count(sim: &Sim) -> usize {
        sim.state
            .tables
            .iter()
            .map(|t| t.objects.iter().flatten().count())
            .sum()
    }

    /// Execute the command; checksum before and after.
    fn run(sim: &mut Sim, command: &Command) -> (u64, u64) {
        let before = checksum(sim.state);
        execute(sim, command, true);
        (before, checksum(sim.state))
    }

    /// The command is rejected: checksum the same.
    fn assert_rejected(sim: &mut Sim, command: &Command) {
        let (before, after) = run(sim, command);
        assert_eq!(before, after, "{command:?} changes the state");
    }

    /// Packed, snapped tent after the fade-out: `(tent, travois)`.
    fn packed(sim: &mut Sim, guid: u32, slot: i32) -> (ObjectId, ObjectId) {
        let tent = spawn_snapped(sim, guid, slot, SNAP);
        execute(sim, &pack_cmd(slot as u8, tent), true);
        run_steps(sim, 50);
        assert_eq!(tent_state(sim, tent).0, status::REST);
        (tent, tent_state(sim, tent).1)
    }

    /// Test world with the menu of 1601 including the prerequisite `REQUIRED_GUID 1600` for 452 and of 1717
    /// including the prerequisite for 453.
    fn locked_rules() -> Rules {
        let mut r = rules();
        r.toolbar = parse_toolbar(concat!(
            "GUID 1601\nBUTTONID 5\nBUTTONMSG 452\nGROUP_AND_MSG\nTARGET_DO_MSG\n",
            "REQUIRED_GUID 1600\nENDGUID\n",
            "GUID 1717\nBUTTONID 5\nBUTTONMSG 453\nTARGET_DO_MSG\nREQUIRED_GUID 1600\nENDGUID\n",
        ));
        r
    }

    fn fresh() -> (Rules, SimState) {
        let rules = rules();
        let state = state(&rules);
        (rules, state)
    }

    // QUIRK W-262: kept
    // QUIRK W-247: deviation
    #[test]
    fn pack_tent_checks_owner_module_selectable_and_button() {
        let (rules, mut state) = fresh();
        let mut sim = Sim::new(&mut state, &rules);
        // Foreign tent (slot 1), command from slot 0.
        let foreign = spawn(&mut sim, DWELLING_TENT, 1, (20, 20));
        assert_rejected(&mut sim, &pack_cmd(0, foreign));
        assert_eq!(tent_state(&sim, foreign).0, status::REST);
        // Dead tent (`DIED`, `owns` rejects).
        let dead = spawn(&mut sim, DWELLING_TENT, 0, (24, 20));
        sim.object_mut(dead).unwrap().status |= object_status::DIED;
        assert_rejected(&mut sim, &pack_cmd(0, dead));
        // Unknown ID.
        assert_rejected(&mut sim, &pack_cmd(0, ObjectId::NONE));
        // Storage camp 1603: button 452, but no module 42 (W-262).
        let storage = spawn(&mut sim, STORAGE_TENT, 0, (28, 20));
        assert!(!sim.object(storage).unwrap().has_module(tent::INDEX, false));
        assert_rejected(&mut sim, &pack_cmd(0, storage));
        // Object without button 452 in the menu: the travois.
        let travois = spawn(&mut sim, TRAVOIS, 0, (32, 20));
        assert_rejected(&mut sim, &pack_cmd(0, travois));
        // Hidden tent (W-247): unselectable, no second travois.
        let (hidden, first) = packed(&mut sim, DWELLING_TENT, 0);
        assert!(!sim.object(hidden).unwrap().selectable);
        let travois_before = travois_count(&sim, 0);
        assert_rejected(&mut sim, &pack_cmd(0, hidden));
        assert_eq!(travois_count(&sim, 0), travois_before);
        assert_eq!(tent_state(&sim, hidden), (status::REST, first));

        // Own 1601: the travois is created, the tent fades out.
        let own = spawn(&mut sim, DWELLING_TENT, 0, (40, 30));
        let before = travois_count(&sim, 0);
        let (c0, c1) = run(&mut sim, &pack_cmd(0, own));
        assert_ne!(c0, c1);
        assert_eq!(travois_count(&sim, 0), before + 1);
        let (st, travois) = tent_state(&sim, own);
        assert_eq!(st, status::FADE_OUT);
        assert_eq!(
            flags_35(&sim, travois) & transport::TRANSPORTING_TENT,
            0x100
        );
    }

    #[test]
    fn pack_tent_respects_a_locked_button() {
        let rules = locked_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let tent = spawn(&mut sim, DWELLING_TENT, 0, (20, 20));
        // Without a chief's tent the button is locked (`REQUIRED_GUID`).
        assert_rejected(&mut sim, &pack_cmd(0, tent));
        assert_eq!(travois_count(&sim, 0), 0);
        spawn(&mut sim, CHIEF_TENT, 0, (30, 30));
        let (c0, c1) = run(&mut sim, &pack_cmd(0, tent));
        assert_ne!(c0, c1);
        assert_eq!(travois_count(&sim, 0), 1);
    }

    #[test]
    fn unpack_tent_checks_owner_flag_button_and_map() {
        let (rules, mut state) = fresh();
        let mut sim = Sim::new(&mut state, &rules);
        let (_, travois) = packed(&mut sim, DWELLING_TENT, 0);
        assert_eq!(
            flags_35(&sim, travois) & transport::TRANSPORTING_TENT,
            transport::TRANSPORTING_TENT
        );
        let ok = (30 << 12, 30 << 12);
        // Travois without the flag (never carried a tent).
        let empty = spawn(&mut sim, TRAVOIS, 0, (20, 20));
        assert_rejected(&mut sim, &unpack_cmd(0, empty, ok.0, ok.1));
        // Colonist: neither module 35 nor button 453.
        let colonist = spawn(&mut sim, COLONIST, 0, (24, 20));
        assert_rejected(&mut sim, &unpack_cmd(0, colonist, ok.0, ok.1));
        // Foreign travois with tent (slot 1), command from slot 0.
        let (_, foreign) = packed(&mut sim, DWELLING_TENT, 1);
        assert_rejected(&mut sim, &unpack_cmd(0, foreign, ok.0, ok.1));
        // Target outside the map: -1 and exactly `dim << 12` (end excluded).
        let (dim_x, dim_z) = (rules.map.dim_x, rules.map.dim_z);
        for (x, z) in [
            (-1, ok.1),
            (ok.0, -1),
            (dim_x << 12, ok.1),
            (ok.0, dim_z << 12),
            (i32::MIN, ok.1),
        ] {
            assert_rejected(&mut sim, &unpack_cmd(0, travois, x, z));
        }
        // Dead travois.
        let (_, dead) = packed(&mut sim, DWELLING_TENT, 0);
        sim.object_mut(dead).unwrap().status |= object_status::DIED;
        assert_rejected(&mut sim, &unpack_cmd(0, dead, ok.0, ok.1));
        assert_ne!(flags_35(&sim, travois) & transport::TRANSPORTING_TENT, 0);

        // Valid: the travois gets the walk order (checksum changes), also at the edge.
        let (c0, c1) = run(&mut sim, &unpack_cmd(0, travois, ok.0, ok.1));
        assert_ne!(c0, c1);
        let (c0, c1) = run(
            &mut sim,
            &unpack_cmd(0, travois, (dim_x << 12) - 1, (dim_z << 12) - 1),
        );
        assert_ne!(c0, c1);
    }

    #[test]
    fn unpack_tent_respects_a_locked_button() {
        let rules = locked_rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // For packing the message is enough here (452 is locked).
        let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
        crate::messaging::send(&mut sim, tent, &Message::new(tent::msg::PACK, &[]));
        let travois = tent_state(&sim, tent).1;
        assert!(!travois.is_none());
        let target = (30 << 12, 30 << 12);
        assert_rejected(&mut sim, &unpack_cmd(0, travois, target.0, target.1));
        spawn(&mut sim, CHIEF_TENT, 0, (50, 50));
        let (c0, c1) = run(&mut sim, &unpack_cmd(0, travois, target.0, target.1));
        assert_ne!(c0, c1);
    }

    #[test]
    fn pack_and_unpack_commands_run_the_tent_cycle() {
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let tent = spawn_snapped(&mut sim, DWELLING_TENT, 0, SNAP);
        let home = sim.object(tent).unwrap().grid;
        let total = object_count(&sim);

        // Packing by command via `tick::step`.
        crate::tick::step(sim.state, sim.rules, &[pack_cmd(0, tent)]);
        let (st, travois) = tent_state(&sim, tent);
        assert_eq!(st, status::FADE_OUT);
        assert_eq!(object_count(&sim), total + 1);
        run_steps(&mut sim, 50);
        assert_eq!(tent_state(&sim, tent).0, status::REST);
        assert!(!sim.object(tent).unwrap().selectable);

        // Unpacking by command: the travois walks to the target via FINDTO.
        let (x, z) = (home.x.0 - (8 << 12), home.z.0 - (6 << 12));
        let start = sim.object(travois).unwrap().grid;
        crate::tick::step(sim.state, sim.rules, &[unpack_cmd(0, travois, x, z)]);
        assert_eq!(tent_state(&sim, tent).0, status::REST, "still on the way");
        run_steps(&mut sim, 4);
        let moved = sim.object(travois).unwrap().grid;
        assert_ne!((moved.x, moved.z), (start.x, start.z), "travois walks");
        assert_ne!(
            flags_35(&sim, travois) & transport::TRANSPORTING_TENT,
            0,
            "on the way it still carries the tent"
        );
        // Until arrival (at most 400 steps).
        let mut steps = 0;
        while tent_state(&sim, tent).0 != status::FADE_IN {
            assert!(steps < 400, "travois did not arrive");
            run_steps(&mut sim, 1);
            steps += 1;
        }
        let o = sim.object(tent).unwrap();
        assert_eq!((o.grid.x.0, o.grid.z.0), (x, z));
        assert!(!sim.object(travois).unwrap().selectable);
        assert_eq!(flags_35(&sim, travois) & transport::TRANSPORTING_TENT, 0);

        // Fade in (25 object ticks), then the tent stands again, the travois is gone.
        run_steps(&mut sim, 52);
        assert_eq!(tent_state(&sim, tent).0, status::REST);
        assert!(sim.object(tent).unwrap().selectable);
        assert!(sim.object(travois).is_none());
        assert_eq!(object_count(&sim), total);
    }
}
