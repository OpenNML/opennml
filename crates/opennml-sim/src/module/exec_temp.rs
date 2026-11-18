//! Module 44 `EXECTEMPUPGRADE` (`PropertyExecTempUpgrade_Construct` `0x00512050`, vtable
//! `0x00623808`, `0xC0` bytes, re/temp-upgrades 1.3): execution list (`+0x70`, 450001, GUIDs of the
//! temporary upgrades) and influence list (`+0x7C`, 450002, indices into the execution list).
//! It implements execution `0x12C` (re/temp-upgrades 3.3), the tick (timers only) and the
//! owner change (delete group `0x80`), as well as influence and aura
//! (`0x22C`/`0x32C`/`0x42C`, [`super::influence`], re/evoke-aura 3.6-3.9) including the init posting from
//! 450002, owner change with the old slot and the destructor. The execution effect (450004...,
//! `0x52C`) and the influence effect (450003) are rendering.

use opennml_formats::property::ModuleDesc;
use opennml_formats::targets::MatchFilter;
use opennml_formats::temp_upgrades::flags;
use serde::{Deserialize, Serialize};

use crate::collision::query_radius;
use crate::id::ObjectId;
use crate::message::Message;
use crate::sim::Sim;
use crate::upgrades::temp;

use super::events::update_events;
use super::stand::parse_int_list;
use super::{ModuleKind, influence, init_flags};

/// Module index of ExecTempUpgrade.
pub const INDEX: usize = 0x2C;
/// Group of the timers `0x52C`, `0x22C` (init) and `0x42C` (the owner change deletes them).
pub(crate) const GROUP: u8 = 0x80;
/// Delay of `0x52C` after executing (ms).
pub const EFFECT_MS: i32 = 1500;
/// Sight (attribute 18): a quarter of it is the radius of the execution.
const SIGHT: u32 = 18;

/// Messages to module 44 (query `0x005133B0`, re/temp-upgrades 2.1).
pub mod msg {
    /// `{idx}`: execute the temporary upgrade `450001[idx]` (`0x00512120`).
    pub const EXECUTE: u32 = 0x12C;
    /// `{idx}`: start influence (`0x00512D20`, re/evoke-aura 3.6).
    pub const INFLUENCE_START: u32 = 0x22C;
    /// `{idx}`: end influence (`0x00512580`, re/evoke-aura 3.8).
    pub const INFLUENCE_STOP: u32 = 0x32C;
    /// Aura tick (`0x00512F30`, re/evoke-aura 3.7).
    pub const AURA_TICK: u32 = 0x42C;
    /// Remove the execution effect (rendering).
    pub const EFFECT_OFF: u32 = 0x52C;
}

/// Entry of the influence list `m_InfluenceUpgrade` (`+0x90`, re/evoke-aura 1.4, 5): GUID of the
/// record and the enemy flag of the filter (`ENEMYLIST` instead of `FRIENDLIST` with
/// `ENEMYUPGRADE`). The rules supply the record, the reimplementation derives the filter from
/// `enemy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Influence {
    /// GUID of the temporary upgrade (from the execution list).
    pub guid: u32,
    /// Flag `ENEMYUPGRADE` of the record (filter `ENEMYLIST`).
    pub enemy: bool,
}

/// `CPropertyExecTempUpgrade`: execution part and influence part (re/evoke-aura 1.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecTempUpgrade {
    /// Execution list (`+0x70`, 450001): GUIDs from `tempupgrades.ini`.
    pub exec_list: Vec<u32>,
    /// Influence list (`+0x7C`, 450002): indices into [`ExecTempUpgrade::exec_list`].
    pub influence_list: Vec<i32>,
    /// Running influences (`m_InfluenceUpgrade` `+0x90`) in insertion order.
    pub influences: Vec<Influence>,
    /// Influenced objects (`m_InfluenceHandles` `+0x88`): `None` = `NULL` (no set), otherwise
    /// the IDs in insertion order; they may be stale.
    pub influenced: Option<Vec<ObjectId>>,
}

/// Delay of the `0x22C` from init (ms).
const INIT_INFLUENCE_MS: i32 = 100;

/// `PropertyExecTempUpgrade_Init` (`0x005122C0`, re/temp-upgrades 1.3): 450001 -> `+0x70`, 450002 ->
/// `+0x7C` (`IntList_ParseFromString`); per entry `i` of 450002 `{0x22C, 450002[i]}` after
/// 100 ms (group `0x80`; in the shipped data 450002 is empty everywhere, several entries start several
/// aura tick chains, C-291). The entries fire in **reverse** order: `PostDelayedMessage`
/// adds at the front of `m_TimersWS` (AddHead, re/module-runtime 1, section "Creating"), so
/// `450002 = "0 1"` runs as `[4461, 4460]` into the influence list (measured, test
/// `init_posts_direct_influences`). The effect names (450003...450006) are rendering.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let flags = init_flags(INDEX, desc);
    let text = |v: u32| desc.values.get(&v).cloned().unwrap_or_default();
    let (exec, influence) = (text(450001), text(450002));
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
        if let ModuleKind::ExecTempUpgrade(e) = &mut m.kind {
            e.exec_list = parse_int_list(&exec, &[])
                .into_iter()
                .map(|g| g as u32)
                .collect();
            e.influence_list = parse_int_list(&influence, &[]);
        }
        let starts = match &m.kind {
            ModuleKind::ExecTempUpgrade(e) => e.influence_list.clone(),
            _ => Vec::new(),
        };
        for idx in starts {
            m.base.post_delayed_message(
                INIT_INFLUENCE_MS,
                Message::new(msg::INFLUENCE_START, &[idx]),
                false,
                GROUP,
            );
        }
    }
}

/// Length of the execution list of the object (`None` without module 44).
pub fn exec_len(sim: &Sim, id: ObjectId) -> Option<usize> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::ExecTempUpgrade(e) => Some(e.exec_list.len()),
        _ => None,
    }
}

/// Tick (slot 13, `0x005119A0`, re/temp-upgrades 1.3): only `Module_UpdateEvents` (`0x52C` runs out).
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    update_events(sim, id, INDEX, dt);
}

/// Owner change (slot 5, `0x005127C0`, re/temp-upgrades 1.3, re/evoke-aura 3.9; `id` is the new
/// ID): first delete timer group `0x80` (`0x52C`, `0x42C`), then take the set and
/// remove with the filter of the **old** slot from everything in it ([`influence::release_set`]).
/// The influence list stays: the aura is off for good (C-223).
pub fn on_owner_changed(sim: &mut Sim, id: ObjectId, old: ObjectId) {
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.clear_event_group(GROUP);
    }
    influence::release_set(sim, id, i32::from(old.slot()));
}

/// `Query` (slot 7, `0x005133B0`): `0x12C` execute, `0x22C`/`0x32C`/`0x42C` influence
/// ([`influence`]), `0x52C` -> 0 (effect, rendering); otherwise -1.
pub fn query(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    match m.code {
        msg::EXECUTE => execute(sim, id, m.arg(0)),
        msg::INFLUENCE_START => influence::start(sim, id, m.arg(0)),
        msg::INFLUENCE_STOP => influence::stop(sim, id, m.arg(0)),
        msg::AURA_TICK => influence::aura_tick(sim, id),
        msg::EFFECT_OFF => 0,
        _ => -1,
    }
}

/// `0x12C {idx}` (`0x00512120`, re/temp-upgrades 3.3): GUID = `450001[idx]` (`IntList_GetAt`
/// `0x00405840`: outside the list 0, so no record); without a record in the rules -> 1.
/// Otherwise `0x52C` after 1500 ms (group `0x80`), then flag `RADIUS` -> in the surroundings, else
/// `HANDLELIST` -> all objects of the owner ([`temp::apply_to_player_objects`]); both return
/// 0. Without both flags (animal records 4460-4462) nothing is applied, return value 1 (re/temp-upgrades 8 no. 9).
fn execute(sim: &mut Sim, id: ObjectId, idx: i32) -> i32 {
    let guid = match &sim.module(id, INDEX).map(|m| &m.kind) {
        Some(ModuleKind::ExecTempUpgrade(e)) => usize::try_from(idx)
            .ok()
            .and_then(|i| e.exec_list.get(i).copied())
            .unwrap_or(0),
        _ => return -1,
    };
    let Some(rec_flags) = sim.rules.temp_upgrade(guid).map(|r| r.flags) else {
        return 1;
    };
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base
            .post_delayed_message(EFFECT_MS, Message::new(msg::EFFECT_OFF, &[]), false, GROUP);
    }
    if rec_flags & flags::RADIUS != 0 {
        apply_in_radius(sim, id, guid, rec_flags);
        0
    } else if rec_flags & flags::HANDLELIST != 0 {
        let slot = sim.object(id).map_or(-1, |o| i32::from(o.owner()));
        temp::apply_to_player_objects(sim, slot, guid);
        0
    } else {
        1
    }
}

/// `0x00511B40` (re/temp-upgrades 3.3): radius `r = (current(18) << 12) / 4` (truncation toward zero),
/// filter `MYLIST` (`0x100`) or `ENEMYLIST` (`0x400`) with `ENEMYUPGRADE`, radius search around the
/// grid position with the slot of the owner. Own: types of the upgrade outside (`OBJ_UPGRADES`),
/// hits inside, only objects without the GUID; enemy: all hits without a type check (C-221). The
/// list is fixed before the first application.
fn apply_in_radius(sim: &mut Sim, id: ObjectId, guid: u32, rec_flags: u32) {
    let Some((pos, slot)) = sim
        .object(id)
        .map(|o| ((o.grid.x.0, o.grid.z.0), i32::from(o.owner())))
    else {
        return;
    };
    let r = sim.attribute(id, SIGHT).wrapping_shl(12) / 4;
    let enemy = rec_flags & flags::ENEMY != 0;
    let filter = MatchFilter {
        mode: if enemy { 0x400 } else { 0x100 },
        ..MatchFilter::constructed()
    };
    let found = query_radius(sim, pos, &filter, r, slot, false);
    let targets: Vec<ObjectId> = if enemy {
        found
    } else {
        let view: &Sim = sim;
        view.rules
            .types_with_upgrade(guid)
            .iter()
            .flat_map(|&ty| {
                found.iter().copied().filter(move |&t| {
                    view.object(t).is_some_and(|o| {
                        o.guid == ty && !o.temp_upgrades.iter().any(|e| e.guid == guid)
                    })
                })
            })
            .collect()
    };
    for t in targets {
        temp::apply(sim, t, guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandKind;
    use crate::command::testkit::run;
    use crate::command::upgrade::check_exec_temp_upgrade;
    use crate::messaging::send;
    use crate::module::spell::MANA;
    use crate::save::checksum;
    use crate::tick::object_tick;
    use crate::upgrades::testkit::*;

    /// Execution list of the governor: 0 combat view, 1 taxes, 2 build time; of the undertaker:
    /// 0 darken (enemy, radius), 1 wolf (without radius and handle list).
    const BATTLE_SIGHT: u32 = 4452;
    const BUILD: u32 = 4451;
    const DARKEN: u32 = 4459;

    fn execute(sim: &mut Sim, id: ObjectId, idx: i32) -> i32 {
        send(sim, id, &Message::new(msg::EXECUTE, &[idx]))
    }

    fn temp_guids(sim: &Sim, id: ObjectId) -> Vec<u32> {
        sim.object(id)
            .unwrap()
            .temp_upgrades
            .iter()
            .map(|e| e.guid)
            .collect()
    }

    /// Timers of module 44 (newly created and running).
    fn timers(sim: &Sim, id: ObjectId) -> usize {
        let b = &sim.module(id, INDEX).unwrap().base;
        b.timers_ws.len() + b.events.len()
    }

    fn ticks(sim: &mut Sim, id: ObjectId, n: usize) {
        for _ in 0..n {
            object_tick(sim, id, 100);
        }
    }

    #[test]
    fn battle_sight_applies_in_a_radius_of_a_quarter_sight() {
        // re/temp-upgrades 9 V5: 0x12C {0} (4452, radius): radius 32*4096/4 = 32,768 (8 tiles); own objects of the
        // types (pikeman, governor) in the radius, not the colonist (type missing), not the distant
        // pikeman, not the enemy one. Governor itself: sight 32 -> 56, ranged 20 -> 40.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        let near = spawn(&mut sim, PIKEMAN, 0, (12, 10));
        let edge = spawn(&mut sim, PIKEMAN, 0, (17, 10));
        let far = spawn(&mut sim, PIKEMAN, 0, (30, 10));
        let colonist = spawn(&mut sim, COLONIST, 0, (11, 10));
        let enemy = spawn(&mut sim, PIKEMAN, 1, (10, 12));
        assert_eq!(sim.attribute(g, 18).wrapping_shl(12) / 4, 32_768);
        assert_eq!(execute(&mut sim, g, 0), 0);
        assert_eq!(temp_guids(&sim, g), [BATTLE_SIGHT]);
        assert_eq!(temp_guids(&sim, near), [BATTLE_SIGHT]);
        assert_eq!(temp_guids(&sim, edge), [BATTLE_SIGHT]);
        for id in [far, colonist, enemy] {
            assert!(temp_guids(&sim, id).is_empty(), "{id:?}");
        }
        assert_eq!((sim.attribute(g, 18), sim.attribute(g, 20)), (56, 40));
        // Second execution: holders of the GUID are skipped (no duplicate).
        assert_eq!(execute(&mut sim, g, 0), 0);
        assert_eq!(temp_guids(&sim, near), [BATTLE_SIGHT]);
    }

    #[test]
    fn the_effect_timer_runs_out_in_the_module_tick() {
        // re/temp-upgrades 1.3, slot 13 = `Module_UpdateEvents`: 0x52C after 1500 ms runs out in the
        // tick of the module - counted from the first tick, gone in the 17th tick.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        execute(&mut sim, g, 0);
        assert_eq!(timers(&sim, g), 1);
        ticks(&mut sim, g, 16);
        assert_eq!(timers(&sim, g), 1);
        ticks(&mut sim, g, 1);
        assert_eq!(timers(&sim, g), 0);
    }

    #[test]
    fn an_owner_change_clears_the_effect_timer() {
        // Slot 5 (`0x005127C0`): first delete group 0x80.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        execute(&mut sim, g, 0);
        assert_eq!(timers(&sim, g), 1);
        let new = crate::world::change_owner(&mut sim, g, 1).expect("new owner");
        assert_eq!(timers(&sim, new), 0);
    }

    // QUIRK C-221: kept
    #[test]
    fn enemy_radius_upgrades_skip_the_type_check() {
        // re/temp-upgrades 9 V7, C-221: 4459 (ENEMYUPGRADE) hits enemy colonists and buildings in the radius without
        // a type check (no `OBJ_UPGRADES`), no own ones and no distant ones.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = spawn(&mut sim, UNDERTAKER, 0, (10, 10));
        let enemy = spawn(&mut sim, COLONIST, 1, (12, 10));
        let hq = spawn(&mut sim, HQ, 1, (10, 14));
        let far = spawn(&mut sim, COLONIST, 1, (40, 40));
        let own = spawn(&mut sim, COLONIST, 0, (11, 10));
        assert!(rules.types_with_upgrade(DARKEN).is_empty());
        assert_eq!(execute(&mut sim, u, 0), 0);
        assert_eq!(temp_guids(&sim, enemy), [DARKEN]);
        assert_eq!(temp_guids(&sim, hq), [DARKEN]);
        for id in [far, own, u] {
            assert!(temp_guids(&sim, id).is_empty(), "{id:?}");
        }
    }

    #[test]
    fn records_without_radius_or_handle_list_apply_nothing() {
        // re/temp-upgrades 8 no. 9 (`0x00512120`): 4460 without `0x10`/`0x20` -> no entry, not even on the archer
        // (type of the upgrade); the timer 0x52C starts anyway, return value 1. An index
        // outside the list (`IntList_GetAt` -> 0) and a GUID without a record: no record,
        // no timer, return value 1, no panic.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u = spawn(&mut sim, UNDERTAKER, 0, (10, 10));
        let archer = spawn(&mut sim, ARCHER, 0, (11, 10));
        assert_eq!(execute(&mut sim, u, 1), 1);
        assert!(temp_guids(&sim, u).is_empty() && temp_guids(&sim, archer).is_empty());
        assert_eq!(timers(&sim, u), 1);
        for idx in [2, -1] {
            assert_eq!(execute(&mut sim, u, idx), 1);
        }
        if let Some(ModuleKind::ExecTempUpgrade(e)) = sim.module_mut(u, INDEX).map(|m| &mut m.kind)
        {
            e.exec_list.push(4453);
        }
        assert_eq!(execute(&mut sim, u, 2), 1);
        assert_eq!(timers(&sim, u), 1);
        // Influence (re/evoke-aura 2.3): `0x22C`/`0x32C` answer 0, `0x42C` 0 without and 1
        // with a running influence (4459 has a record); 0x52C with 0.
        let code = |c: u32, args: &[i32]| Message::new(c, args);
        assert_eq!(send(&mut sim, u, &code(msg::AURA_TICK, &[])), 0);
        assert_eq!(send(&mut sim, u, &code(msg::INFLUENCE_START, &[0])), 0);
        assert_eq!(send(&mut sim, u, &code(msg::AURA_TICK, &[])), 1);
        assert_eq!(send(&mut sim, u, &code(msg::INFLUENCE_STOP, &[0])), 0);
        assert_eq!(send(&mut sim, u, &code(msg::AURA_TICK, &[])), 0);
        assert_eq!(send(&mut sim, u, &Message::new(msg::EFFECT_OFF, &[])), 0);
    }

    #[test]
    fn exec_temp_upgrade_rejects_an_index_outside_the_list() {
        // A7/A8: idx 3 (list of length 3) and -1 rejected, likewise an object without module 44, a
        // foreign one and a dead one; checksum and mana unchanged.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        let foreign = spawn(&mut sim, GOVERNOR, 1, (40, 10));
        let colonist = spawn(&mut sim, COLONIST, 0, (12, 10));
        let dead = spawn(&mut sim, GOVERNOR, 0, (14, 10));
        send(&mut sim, dead, &Message::new(DIE, &[]));
        reject_exec(&mut sim, g, 544, 3, "outside");
        reject_exec(&mut sim, g, 544, -1, "outside");
        reject_exec(&mut sim, colonist, 544, 0, "no execute module");
        reject_exec(&mut sim, foreign, 544, 2, "not an own");
        reject_exec(&mut sim, dead, 544, 2, "not an own");
        assert_eq!(sim.attribute(g, MANA), 100);
        check_exec_temp_upgrade(&sim, 0, g, 544, 2).expect("accepted");
    }

    /// Dying (`PropertyDying_StartDying`, module 16).
    const DIE: u32 = 0x110;

    /// `0x5E` rejected with message `why`, checksum unchanged.
    fn reject_exec(sim: &mut Sim, object: ObjectId, button: i32, idx: i32, why: &str) {
        let err = check_exec_temp_upgrade(sim, 0, object, button, idx).unwrap_err();
        assert!(err.contains(why), "{button}/{idx}: {err}");
        let before = checksum(sim.state);
        let kind = CommandKind::ExecTempUpgrade {
            object,
            button,
            idx,
        };
        run(sim, 0, kind);
        assert_eq!(checksum(sim.state), before, "{button}/{idx}: {err}");
    }

    #[test]
    fn exec_temp_upgrade_checks_the_button_like_the_bar() {
        // Without a button check, a button without a value set would pay no mana and
        // bypass `REQUIRED_GUID` ("Collect taxes" with button 0). Rejected: button 0 and negative, not an
        // ability button, index does not match the button, not in the menu, 543 without 2828.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        reject_exec(&mut sim, g, 0, 1, "not an ability button");
        reject_exec(&mut sim, g, -543, 1, "not an ability button");
        reject_exec(&mut sim, g, 550, 1, "not an ability button");
        reject_exec(&mut sim, g, 544, 1, "does not match button 544");
        reject_exec(&mut sim, g, 549, 0, "not in the menu");
        reject_exec(&mut sim, g, 543, 1, "prerequisite");
        assert!(temp_guids(&sim, g).is_empty() && sim.attribute(g, MANA) == 100);
        sim.state.add_guid_count(0, 2828, 1);
        check_exec_temp_upgrade(&sim, 0, g, 543, 1).expect("accepted");
    }

    #[test]
    fn exec_temp_upgrade_pays_mana_and_executes() {
        // `GameWorld_CmdExecTempUpgrade` (`0x0047EE90`): idx 2 with button 544 (mana 50) -> mana 50
        // immediately, in the object tick 4451 on all colonists and governors of the slot (handle list),
        // not on those of slot 1. Too little mana (button 543 costs 75, 2828 researched) -> nothing.
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let g = spawn(&mut sim, GOVERNOR, 0, (10, 10));
        let c1 = spawn(&mut sim, COLONIST, 0, (12, 10));
        let c2 = spawn(&mut sim, COLONIST, 0, (40, 40));
        let other = spawn(&mut sim, COLONIST, 1, (13, 10));
        let exec = |object, button, idx| CommandKind::ExecTempUpgrade {
            object,
            button,
            idx,
        };
        run(&mut sim, 0, exec(g, 544, 2));
        assert_eq!(sim.attribute(g, MANA), 50);
        assert!(temp_guids(&sim, g).is_empty(), "posted");
        ticks(&mut sim, g, 1);
        for id in [g, c1, c2] {
            assert_eq!(temp_guids(&sim, id), [BUILD], "{id:?}");
        }
        assert!(temp_guids(&sim, other).is_empty());
        sim.state.add_guid_count(0, 2828, 1);
        check_exec_temp_upgrade(&sim, 0, g, 543, 1).expect("accepted");
        let before = checksum(sim.state);
        run(&mut sim, 0, exec(g, 543, 1));
        assert_eq!(checksum(sim.state), before, "too little mana: nothing");
        assert!(sim.object(g).unwrap().fifo.is_empty());
    }
}
