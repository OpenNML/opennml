//! Construction on the building side (re/construction, re/buildings): the build step `0x203`
//! (construction, repair, completion), the search for the nearest construction site, the forwarding
//! in the wall group with its messages, the construction site flag and the burning down in the
//! building tick and the damage image (`Building_UpdateDamageState`). [`forward_build_tick`] and
//! [`on_set_slave_list`] register the group's enemy watch via
//! [`super::watch::update_wall_area_watch`] (`0x00507EB0`, re/arming-gates 3.D1).
//!
//! Completion writes the statistic "building finished" ([`crate::accounting`]). Not reproduced: fire
//! and dummy effects and the selection indicator (presentation and UI only).

use crate::accounting::{add_statistic, kind as stat};
use crate::collision::find_nearest_matching;
use crate::economy::{
    announce, find_unaffordable_costs_percent, message, pay_costs_percent, resource,
};
use crate::filter::matches_class;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::{send, send_mut};
use crate::module::construct::site_filter;
use crate::object::status;
use crate::sim::Sim;
use crate::state::SimEvent;

use super::{
    INDEX, building, building_mut, flags, has_flags, mark_footprint, set_building_animation,
    set_flags,
};

/// Sound of completion (`Object_PlayCreatedSound(0x68)`).
pub const SFX_CREATED: i32 = 0x68;
/// Search radius of the nearest construction site (`Fixed_FromInt(10)`).
pub const NEXT_SITE_RADIUS: i32 = 10 << 12;
/// Burns down below this HP share in percent; lower in campaign mode (game mode < 0).
pub const BURN_LIMIT: i32 = 40;
pub const CAMPAIGN_BURN_LIMIT: i32 = 10;
/// From this construction progress on (percent, exclusive) the building is back in the
/// `BuildingTileMap`.
const TILE_MAP_PROGRESS: i32 = 70;
/// `0.1f32` (`0x00618FB4`): smallest scale of a construction site.
const MIN_SCALE: f32 = f32::from_bits(0x3DCC_CCCD);
/// `0.01f32` (`0x0061623C`) of the damage image.
const PERCENT: f32 = f32::from_bits(0x3C23_D70A);

/// Messages to the own object on completion.
mod notify {
    /// Quarters: building finished.
    pub const QUARTER_FINISHED: u32 = 0x1A17;
    /// Trade (module 43): building finished, starts the tax timer (re/trading 3.6).
    pub const TRADING_FINISHED: u32 = 0x72B;
    /// Wall group: new master.
    pub const SET_MASTER: u32 = 0x1D03;
}

/// `PropertyBuilding_OnBuildTick` (`0x00508FB0`, `0x203 {speed, _}`): one build step with
/// `s = speed / 10` (direction 0). Dead -> 0; sleeping -> forward to the group (without members 0).
/// Finished: full -> [`forward_build_tick`], otherwise repair with `r = s / 4`. Unfinished: above 70 %
/// progress (before the deduction) back into the `BuildingTileMap`, lock the path grids (every time,
/// takes effect once), remaining work -s, HP share `+HpPerTick · s`; at remainder < 1 completion
/// ([`complete`]), otherwise scale and HP at most `Max − 1`. Return value 1 continue, 0 stop,
/// 2 next construction site in `args[1]`.
pub fn on_build_tick(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let s = m.arg(0) / 10;
    let Some(b) = building(sim, id) else {
        return -1;
    };
    if b.is_dead {
        return 0;
    }
    if b.is_sleeping {
        return if b.slaves.is_empty() {
            0
        } else {
            forward_build_tick(sim, id, m)
        };
    }
    if has_flags(sim, id, flags::CONSTRUCTED_FINISHED) {
        if sim.object(id).is_some_and(|o| o.attributes.is_full(14)) {
            return forward_build_tick(sim, id, m);
        }
        return repair_step(sim, id, s);
    }
    let (counter, build_time, hpt, inserted) = (
        b.build_counter,
        b.build_time,
        b.hp_per_tick,
        b.heightmap_inserted,
    );
    let progress = 100i32.wrapping_sub(counter.wrapping_mul(100).wrapping_div(build_time));
    if progress > TILE_MAP_PROGRESS && !inserted {
        if let Some(b) = building_mut(sim, id) {
            b.heightmap_inserted = true;
        }
        crate::tile_map::add_building(sim, id, true);
    }
    mark_footprint(sim, id, true, true);
    let rest = counter.wrapping_sub(s);
    if let Some(b) = building_mut(sim, id) {
        b.build_counter = rest;
    }
    if let Some(o) = sim.object_mut(id) {
        o.attributes.add_fraction(14, hpt.wrapping_mul(s));
    }
    if rest < 1 {
        return complete(sim, id, s, m);
    }
    let scale = progress_scale(rest, build_time);
    let p = upgrade_percent(sim, id);
    if let Some(o) = sim.object_mut(id) {
        o.progress_scale = scale;
        let max = o.attributes.max_value(14, p);
        if o.attributes.current_value(14, p) == max {
            o.attributes.set_current_value(14, max.wrapping_sub(1), p);
        }
    }
    1
}

/// Scale of a construction site: `FILD remainder; FIDIV BuildTime; FSUBR 1.0; FSTP f32`, then
/// `0.1 ≤ v ? v : 0.1` (`FCOMP`, unordered keeps `v`). The original computes in x87 with a 24-bit
/// mantissa: integer operands exactly, quotient and difference each rounded once to 24 bits -
/// that is `f32` arithmetic as long as remainder and build time (configuration 13 * 100) are below
/// 2^24 in absolute value (otherwise the conversion itself would already round here; at most 9000 in the shipped data).
pub fn progress_scale(rest: i32, build_time: i32) -> f32 {
    let v = 1.0f32 - rest as f32 / build_time as f32;
    if MIN_SCALE <= v || v.is_nan() {
        v
    } else {
        MIN_SCALE
    }
}

fn upgrade_percent(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id)
        .map_or(0, |o| sim.type_upgrade_percent(o.owner(), o.guid, 14))
}

/// Repair step of a finished, damaged building: cost `CostPerTick · r · 100 %` (raw, direction 0) is
/// paid by the **building owner**; if something is missing, message 8 once (flag `+0x74`, set in the
/// original only for the local player - here for everyone) and 0. Otherwise reset the flag, pay,
/// HP share `+HpPerTick · r`, 1. A neutral building has no owner: the original reads a null pointer
/// (crash), here 0 (exception A7).
fn repair_step(sim: &mut Sim, id: ObjectId, s: i32) -> i32 {
    let r = s / 4;
    let pct = r.wrapping_mul(100);
    let Some(b) = building(sim, id) else {
        return 0;
    };
    let (cost, hpt, shown) = (b.cost_per_tick, b.hp_per_tick, b.repair_msg_shown);
    let slot = usize::from(id.slot());
    let Some(p) = sim.state.players.get(slot) else {
        log::warn!(
            "Repair on {id:?} without an owner (original: null pointer, exception A7): no step"
        );
        return 0;
    };
    if find_unaffordable_costs_percent(p, &cost, pct) >= 0 {
        if !shown {
            let tile = sim.object(id).map_or((0, 0), |o| o.tile);
            announce(
                sim,
                slot as i32,
                message::REPAIR_TOO_EXPENSIVE,
                tile,
                message::flags::REPAIR,
            );
            if let Some(b) = building_mut(sim, id) {
                b.repair_msg_shown = true;
            }
        }
        return 0;
    }
    if let Some(b) = building_mut(sim, id) {
        b.repair_msg_shown = false;
    }
    if let Some(p) = sim.state.players.get_mut(slot) {
        pay_costs_percent(p, &cost, pct);
    }
    if let Some(o) = sim.object_mut(id) {
        o.attributes.add_fraction(14, hpt.wrapping_mul(r));
    }
    1
}

/// Completion in the last build step: scale 1, sound `0x68`, HP share a **second** time
/// `+HpPerTick · s` (quirk), render bits 3, flags from the backup `+0x348`, `m_IsExplode`,
/// an animated building draws **one** table random number for its animation, `FINISHED`, quarters
/// `0x1A17`, object counter +1 (`GameWorld+0x9BC`), "in progress" -1, population and living space from
/// the **attributes** 0x16/0x1B to the owner (message 11 when the living space reaches the limit),
/// trade `0x72B`, then [`find_next_construction_site`].
fn complete(sim: &mut Sim, id: ObjectId, s: i32, m: &mut Message) -> i32 {
    let Some(b) = building(sim, id) else {
        return 0;
    };
    let (hpt, backup) = (b.hp_per_tick, b.flags_backup);
    if let Some(o) = sim.object_mut(id) {
        o.progress_scale = 1.0;
        o.attributes.add_fraction(14, hpt.wrapping_mul(s));
        o.render_bits = 3;
    }
    sim.state.events.push(SimEvent::Sound {
        object: id,
        sound: SFX_CREATED,
    });
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = backup;
    }
    // Statistic "building finished" (`0x00509281`: `FUN_00478A10(owner, 1, GUID)`, not at
    // `HasFlags(0x2000003)`, tracks - O-169).
    if !has_flags(sim, id, flags::TRACK) {
        let guid = sim.object(id).map_or(0, |o| o.guid);
        add_statistic(sim, i32::from(id.slot()), stat::BUILDING_BUILT, guid as i32);
    }
    let explode = has_flags(sim, id, flags::BUILDINGEXPLODE);
    if let Some(b) = building_mut(sim, id) {
        b.is_explode = explode;
    }
    if has_flags(sim, id, flags::ISANIMATED) {
        let r = sim.state.rng.rand();
        let anim = building(sim, id).map_or(-1, |b| super::super::stand::list_modulo(&b.anim, r));
        if let Some(b) = building_mut(sim, id) {
            b.cur_anim = anim;
        }
        set_building_animation(sim, id, anim);
    }
    set_flags(sim, id, flags::CONSTRUCTED_FINISHED);
    send(sim, id, &Message::new(notify::QUARTER_FINISHED, &[]));
    // Dummy effects (`GameObject_AttachDummyEffects(1)`): presentation.
    let Some(o) = sim.object(id) else {
        return 0;
    };
    let (guid, tile) = (o.guid, o.tile);
    let slot = id.slot();
    sim.state.add_guid_count(slot, guid, 1);
    crate::economy::add_owner_pending_guid_count(sim.state, id, guid, -1);
    if let Some(b) = building_mut(sim, id) {
        b.is_build_building = false;
    }
    let population = sim.attribute(id, 0x16);
    let living = sim.attribute(id, 0x1B);
    let max_living = sim.state.economy.max_living_space;
    if let Some(p) = sim.state.players.get_mut(usize::from(slot)) {
        p.add_resource(resource::POPULATION, population);
        p.add_resource(resource::LIVING_SPACE, living);
        if living > 0 && max_living <= p.resource(resource::LIVING_SPACE, max_living) {
            announce(
                sim,
                i32::from(slot),
                message::LIVING_SPACE_FULL,
                tile,
                message::flags::LIVING_SPACE_FULL,
            );
        }
    }
    send(sim, id, &Message::new(notify::TRADING_FINISHED, &[]));
    // `GameObject_NotifyUiIfSelected(0)`: UI.
    find_next_construction_site(sim, id, m)
}

/// `PropertyBuilding_FindNextConstructionSite` (`0x00508030`): nearest own construction site (module 3
/// without `CONSTRUCTED_FINISHED`, dead ones drop out) within 10 tiles around the center
/// (center distance) -> `args[1] = ID`, 2; otherwise 0.
pub fn find_next_construction_site(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let Some(o) = sim.object(id) else {
        return 0;
    };
    let pos = (o.grid.x.0, o.grid.z.0);
    let filter = site_filter();
    let player = i32::from(id.slot());
    match find_nearest_matching(sim, pos, &filter, NEXT_SITE_RADIUS, player, -1) {
        Some(next) => {
            m.set_arg(1, next.as_i32());
            2
        }
        None => 0,
    }
}

/// `PropertyBuilding_ForwardBuildTick` (`0x00508860`): with a master, its answer (tail call). If
/// the master or its building module is missing, the original calls `SendMessage` on a null pointer;
/// here answer 0 (exception A7: the construction worker ends instead of sending a useless step every
/// tick - reachable only via a savegame, in the game, master and members die together).
/// Otherwise the members in list order: the first one that is not "finished and intact" gets the step
/// (from a distance); if its answer is not 2, it is the answer. If all members are finished and
/// intact, the master registers its enemy watch ([`super::watch::update_wall_area_watch`], only
/// without a running query; way b, W-271) and then looks for the next construction site.
pub fn forward_build_tick(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let Some(b) = building(sim, id) else {
        return 0;
    };
    let (master, slaves) = (b.master, b.slaves.clone());
    if master != ObjectId(0) {
        if building(sim, master).is_none() {
            log::warn!("Build step at {id:?}: master {master:?} is missing (exception A7)");
            return 0;
        }
        return send_mut(sim, master, m);
    }
    for s in slaves {
        let Some(o) = sim.object(s) else {
            continue;
        };
        if o.attributes.is_full(14) && matches_class(sim, s, flags::CONSTRUCTED_FINISHED) {
            continue;
        }
        let r = send_mut(sim, s, m);
        if r != 2 {
            return r;
        }
    }
    super::watch::update_wall_area_watch(sim, id);
    find_next_construction_site(sim, id, m)
}

/// `PropertyBuilding_OnMsgSetSlaveList` (`0x005087B0`, `0x1B03 {k, ids[k]}`): no master, members
/// anew in command order (present and not oneself), each gets `0x1D03 {own ID}`.
/// The original reads more IDs than arguments behind the message; here the list ends at the
/// arguments. If the building is completely created (`is_loading`, `ObjDesc.flags & 0x900`), it then
/// registers its enemy watch ([`super::watch::update_wall_area_watch`], way a, W-271).
pub fn on_set_slave_list(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let k = usize::try_from(m.arg(0))
        .unwrap_or(0)
        .min(m.args.len().saturating_sub(1));
    if let Some(b) = building_mut(sim, id) {
        b.master = ObjectId(0);
        b.slaves.clear();
    }
    for i in 0..k {
        let Some(slave) = sim.object(ObjectId::from_i32(m.arg(1 + i))).map(|o| o.id) else {
            continue;
        };
        if slave == id {
            continue;
        }
        if let Some(b) = building_mut(sim, id) {
            b.slaves.push(slave);
        }
        send(
            sim,
            slave,
            &Message::new(notify::SET_MASTER, &[id.as_i32()]),
        );
    }
    if building(sim, id).is_some_and(|b| b.is_loading) {
        super::watch::update_wall_area_watch(sim, id);
    }
    0
}

/// `PropertyBuilding_OnMsgSetMaster` (`0x005067F0`, `0x1D03 {id}`): forget own members,
/// set master.
pub fn on_set_master(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    if let Some(b) = building_mut(sim, id) {
        b.slaves.clear();
        b.master = ObjectId::from_i32(m.arg(0));
    }
    0
}

/// `0x1C03 {id}` (`0x00507B80`): first matching member from the list.
pub fn on_remove_slave(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    let slave = ObjectId::from_i32(m.arg(0));
    if let Some(b) = building_mut(sim, id)
        && let Some(i) = b.slaves.iter().position(|&s| s == slave)
    {
        b.slaves.remove(i);
    }
    0
}

/// `0x2103 {id}` (`0x00508CD0`): append a member at the end (without a check).
pub fn on_add_slave(sim: &mut Sim, id: ObjectId, m: &Message) -> i32 {
    if let Some(b) = building_mut(sim, id) {
        b.slaves.push(ObjectId::from_i32(m.arg(0)));
    }
    0
}

/// `0x2203` (`0x00504D50`): `args[0]` = master, without a master the own ID.
pub fn on_get_master(sim: &mut Sim, id: ObjectId, m: &mut Message) -> i32 {
    let master = building(sim, id).map_or(ObjectId(0), |b| b.master);
    m.set_arg(
        0,
        if master == ObjectId(0) {
            id.as_i32()
        } else {
            master.as_i32()
        },
    );
    0
}

/// Part of `PropertyBuilding_Tick` (`0x0050CD30`) after timers and deposit mirror: dead ->
/// nothing (the sinking branch is dead, `m_IsDying` always 0). Finished: below 40 % HP (campaign, i.e.
/// game mode `GameEngine+0x108` < 0: 10 %) a building with `m_BurnDown` burns down each tick with
/// `AddScaledToCurrent(14, −3, 10)`, damage image, end. Unfinished: if the construction site is still
/// in the `BuildingTileMap` and at most half is built (remainder >= BuildTime/2, direction 0), it
/// comes out - in the first building tick of each construction site (`CreateObject` enters it).
/// Finally the damage image. Between the dead check and burning down a production order runs
/// (`make_guid != 0`, [`super::production::tick_production`]).
pub fn tick_state(sim: &mut Sim, id: ObjectId) {
    let Some(b) = building(sim, id) else {
        return;
    };
    if b.is_dead {
        return;
    }
    if b.make_guid != 0 {
        super::production::tick_production(sim, id);
    }
    let Some(b) = building(sim, id) else {
        return;
    };
    let (inserted, counter, build_time, burn_down) = (
        b.heightmap_inserted,
        b.build_counter,
        b.build_time,
        b.burn_down,
    );
    if has_flags(sim, id, flags::CONSTRUCTED_FINISHED) {
        let pct = sim
            .object(id)
            .map_or(0, |o| o.attributes.current_fraction(14) / 1000);
        let limit = if sim.state.mode >= 0 {
            BURN_LIMIT
        } else {
            CAMPAIGN_BURN_LIMIT
        };
        if pct < limit && burn_down {
            let p = upgrade_percent(sim, id);
            if let Some(o) = sim.object_mut(id) {
                o.attributes.add_scaled_to_current(14, -3, 10, p);
            }
            update_damage_state(sim, id);
            return;
        }
    } else if inserted && counter >= build_time / 2 {
        // `BuildingTileMap_RemoveObjectId` (`0x00490B50`).
        if let Some(b) = building_mut(sim, id) {
            b.heightmap_inserted = false;
        }
        sim.state.tile_map.remove_id(id.as_i32());
    }
    update_damage_state(sim, id);
}

/// `Building_UpdateDamageState` (`0x00504B30`): only a living, finished, awake (not sleeping) building with
/// hit points. `p = share / 1000`; fire `(50 − p) · 20` (presentation); `p > 40` -> render bit
/// `0x20` off, 1 on; otherwise 1 off, `0x20` (burning) on and the scale `f32(p) · 2.5 · 0.01`
/// (`FILD`, two `FMUL` with `f32` constants, `FSTP`).
pub fn update_damage_state(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else {
        return;
    };
    if o.status & status::DIED != 0 || !o.attributes.is_present(14) {
        return;
    }
    if !has_flags(sim, id, flags::CONSTRUCTED_FINISHED)
        || building(sim, id).is_none_or(|b| b.is_sleeping)
    {
        return;
    }
    let Some(o) = sim.object_mut(id) else {
        return;
    };
    let p = o.attributes.current_fraction(14) / 1000;
    if p > BURN_LIMIT {
        o.render_bits = (o.render_bits & !0x20) | 1;
    } else {
        o.render_bits = (o.render_bits & !1) | 0x20;
        o.progress_scale = p as f32 * 2.5 * PERCENT;
    }
}

/// Savegame check of the wall group and of the state bits on which the forwarding depends;
/// `Some((building, reason))` at the first violation. [`forward_build_tick`] and
/// `PropertyBuilding_OnMsgFinallyDeath` (`0x1E03` to the members) follow master and members without a
/// limit like the original; a cycle would run into a stack overflow. The running game creates
/// none: `0x1B03` goes only to the master just built with its members just built (command
/// `0x18`) and skips its own ID, `0x1D03` and `0x2103` on a change of owner write
/// only the new ID into the same role. Therefore what is checked here holds:
/// - no building is its own master or member;
/// - a member (master != 0) has no members of its own, and an existing master has no
///   master itself (the chain is at most one member long);
/// - every existing building in a member list has a master (so no members);
/// - a finished, not dead building carries neither `DIED` nor `QUARTERED` (only the dead side of
///   `Destroy` and sleeping under a tower set `DIED`, both not finished or dead). Otherwise
///   the member loop of the master would overlook a "finished and intact" member (the class filter
///   rejects dead and quartered objects) whose build step would return to the master.
pub fn wall_group_fault(state: &crate::state::SimState) -> Option<(ObjectId, &'static str)> {
    let wall = |id: ObjectId| -> Option<&super::Building> {
        match &state.object(id, false)?.module(INDEX)?.kind {
            crate::module::ModuleKind::Building(b) => Some(&**b),
            _ => None,
        }
    };
    for o in state.objects() {
        let Some(m) = o.module(INDEX) else { continue };
        let crate::module::ModuleKind::Building(b) = &m.kind else {
            continue;
        };
        if b.master == o.id || b.slaves.contains(&o.id) {
            return Some((
                o.id,
                "is its own master or its own member in the wall group",
            ));
        }
        if b.master != ObjectId(0) {
            if !b.slaves.is_empty() {
                return Some((o.id, "is a member of a wall group and has members itself"));
            }
            if wall(b.master).is_some_and(|m| m.master != ObjectId(0)) {
                return Some((
                    o.id,
                    "has a master in the wall group that itself has a master",
                ));
            }
        }
        if b.slaves
            .iter()
            .any(|&s| wall(s).is_some_and(|s| s.master == ObjectId(0)))
        {
            return Some((o.id, "has a member without a master in the wall group"));
        }
        if m.base.has_flags(flags::CONSTRUCTED_FINISHED)
            && !b.is_dead
            && o.status & (status::DIED | status::QUARTERED) != 0
        {
            return Some((
                o.id,
                "is finished and alive, but is set to dead or quartered",
            ));
        }
    }
    None
}

/// `PropertyBuilding_StartBuild` via `0x103` (`PMSG_BUILDING_STARTBUILD`): return value 0.
/// `StartBuild` (`0x00504840`) itself returns 1, but the query discards it
/// (`0x0050BE49 CALL 0x00504840; XOR EAX, EAX; RET 4`).
pub fn on_start_build(sim: &mut Sim, id: ObjectId) -> i32 {
    super::start_build(sim, id);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::building::module_flags as flags_of;
    use crate::module::building::msg as bmsg;
    use crate::rules::{BuildingShape, Rules};
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object, desc_flags};

    /// House 1001, HQ 1000 (burns down, quarters 5), wall 1011 (wall + wall piece), mill 1002
    /// (animated, list 4 7 9), field 1003.
    const TYPES: &str = "GUID 1000\nNAME SPA_Hauptquartier\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 524288\nPROPID 23\nGUID 1001\nNAME SPA_Wohnhaus\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 0\nGUID 1002\nNAME Muehle\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 262144\nVALUEID 40004 1 1 4 7 9\nGUID 1003\nNAME Feld\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 512\nGUID 1011\nNAME SPA_Mauer\nVALUEID 100 6 1 0\nPROPID 3\nVALUEID 40003 6 1 3145728\n";

    /// `default.dat` values: HQ 500 wood, 90 s, 4000 HP, quarters 5; house 150 wood, 20 s, 900
    /// HP, living space 15; wall 6 s, 1000 HP.
    fn rules() -> Rules {
        let mut rules = Rules::synthetic(TYPES, 64);
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        rules
            .defaults
            .insert(1000, set(&[(11, 500), (13, 90), (14, 4000), (23, 5)]));
        rules
            .defaults
            .insert(1001, set(&[(11, 150), (13, 20), (14, 900), (27, 15)]));
        rules.defaults.insert(1002, set(&[(13, 20), (14, 900)]));
        rules.defaults.insert(1003, set(&[(13, 20), (14, 900)]));
        rules.defaults.insert(1011, set(&[(13, 6), (14, 1000)]));
        rules
    }

    fn place(sim: &mut Sim, guid: u32, slot: i32, tile: (i32, i32), ready: bool) -> ObjectId {
        let world = |t: i32| t as f32 * 128.0 + 64.0;
        let mut desc = ObjectDesc::new(guid, slot, [world(tile.0), 0.0, world(tile.1)]);
        if ready {
            desc.flags |= desc_flags::PLACED;
        }
        create_object(sim, &mut desc).unwrap()
    }

    fn build_tick(sim: &mut Sim, id: ObjectId, speed: i32) -> (i32, Message) {
        let mut m = Message::new(bmsg::BUILD_TICK, &[speed, 0]);
        let r = crate::messaging::send_mut(sim, id, &mut m);
        (r, m)
    }

    fn row(sim: &Sim, id: ObjectId) -> (i32, i32, i32, f32) {
        let o = sim.object(id).unwrap();
        (
            building(sim, id).unwrap().build_counter,
            o.attributes.current_fraction(14),
            o.attributes.current_value(14, 0),
            o.progress_scale,
        )
    }

    /// (Steps, tick of completion, stamp (step, tick)).
    type Outcome = (usize, usize, (usize, usize));

    /// Result of a construction with a building tick before the steps of each tick: (steps, tick of
    /// completion counted from the first step = 0, stamp (step, tick)).
    fn run(sim: &mut Sim, b: ObjectId, workers: impl Fn(usize) -> usize, speed: i32) -> Outcome {
        let mut steps = 0;
        let mut stamp = None;
        for tick in 0..5000 {
            crate::module::tick(sim, b, INDEX, 100);
            for _ in 0..workers(tick) {
                let (r, _) = build_tick(sim, b, speed);
                steps += 1;
                if stamp.is_none() && building(sim, b).unwrap().heightmap_inserted {
                    stamp = Some((steps, tick));
                }
                if r != 1 {
                    assert_eq!(r, 0);
                    return (steps, tick, stamp.unwrap());
                }
            }
        }
        panic!("not finished");
    }

    #[test]
    fn construction_follows_the_step_table() {
        let rules = rules();
        let cases: [(u32, usize, i32, Outcome); 4] = [
            (1001, 1, 100, (200, 199, (142, 141))),
            (1001, 2, 100, (200, 99, (142, 70))),
            (1000, 1, 100, (900, 899, (632, 631))),
            (1000, 2, 100, (900, 449, (632, 315))),
        ];
        for (guid, n, speed, expected) in cases {
            let mut state = SimState::new(0, 0);
            state.init_map(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let b = place(&mut sim, guid, 0, (20, 20), false);
            assert_eq!(run(&mut sim, b, |_| n, speed), expected, "{guid} x {n}");
        }
        // Second worker from tick 100; wall; governor speed 300.
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let hq = place(&mut sim, 1000, 0, (20, 20), false);
        let late = |t: usize| if t < 100 { 1 } else { 2 };
        assert_eq!(run(&mut sim, hq, late, 100), (900, 499, (632, 365)));
        let wall = place(&mut sim, 1011, 0, (40, 40), false);
        let b = building(&sim, wall).unwrap();
        assert_eq!((b.build_time, b.hp_per_tick), (600, 167));
        assert_eq!(
            sim.object(wall).unwrap().attributes.current_fraction(14),
            100
        );
        assert_eq!(run(&mut sim, wall, |_| 1, 100), (60, 59, (44, 43)));
        let house = place(&mut sim, 1001, 0, (40, 20), false);
        assert_eq!(run(&mut sim, house, |_| 1, 300), (67, 66, (48, 47)));
    }

    #[test]
    fn house_steps_match_the_reference_rows() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let b = place(&mut sim, 1001, 0, (20, 20), false);
        assert_eq!(sim.object(b).unwrap().attributes.current_fraction(14), 111);
        assert!(!building(&sim, b).unwrap().blocked);
        let f = |v: f32| v.to_bits();
        let mut rows = Vec::new();
        for k in 1..=200 {
            let (r, _) = build_tick(&mut sim, b, 100);
            assert_eq!(r, if k < 200 { 1 } else { 0 }, "step {k}");
            if k == 1 {
                // The first step locks the path grid.
                assert!(building(&sim, b).unwrap().blocked);
            }
            if [1, 142, 195, 196, 199, 200].contains(&k) {
                let (c, a, hp, s) = row(&sim, b);
                rows.push((k, c, a, hp, f(s)));
            }
        }
        assert_eq!(
            rows,
            [
                (1, 1990, 621, 6, f(0.1)),
                // 1 - f32(0.29) lies exactly between two f32 and rounds to the even mantissa:
                // one unit above f32(0.71).
                (142, 580, 72531, 653, 0x3F35_C290),
                (195, 50, 99561, 896, f(0.975)),
                (196, 40, 99888, 899, f(0.98)),
                (199, 10, 99888, 899, f(0.995)),
                (200, 0, 100_000, 900, f(1.0)),
            ]
        );
    }

    #[test]
    fn the_scale_is_f32_arithmetic_with_a_floor() {
        assert_eq!(progress_scale(1990, 2000).to_bits(), 0x3DCC_CCCD);
        assert_eq!(progress_scale(580, 2000), 1.0f32 - 580.0f32 / 2000.0f32);
        assert_eq!(
            progress_scale(1, 3).to_bits(),
            (1.0f32 - 1.0 / 3.0f32).to_bits()
        );
        // 1 - 1/3 in f32 is not 2/3 rounded in f64.
        assert_eq!(progress_scale(2, 3), 1.0f32 - 2.0f32 / 3.0f32);
        assert_eq!(progress_scale(9000, 9000).to_bits(), 0x3DCC_CCCD);
    }

    #[test]
    fn completion_books_counts_living_space_and_the_quarter() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        state.economy.max_living_space = 10;
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), false);
        let hq = place(&mut sim, 1000, 0, (40, 40), false);
        let living = sim.state.players[0].resource_raw(4);
        assert_eq!(crate::economy::pending_guid_count(&sim, 0, 1001), 1);
        assert_eq!(crate::economy::guid_count(&sim, 0, 1001), 0);
        building_mut(&mut sim, house).unwrap().build_counter = 10;
        sim.state.events.clear();
        let (r, _) = build_tick(&mut sim, house, 100);
        assert_eq!(r, 0);
        let o = sim.object(house).unwrap();
        assert_eq!((o.render_bits, o.progress_scale), (3, 1.0));
        let b = building(&sim, house).unwrap();
        assert!(!b.is_build_building);
        assert_eq!(flags_of(&sim, house), 0x400);
        assert_eq!(crate::economy::pending_guid_count(&sim, 0, 1001), 0);
        assert_eq!(crate::economy::guid_count(&sim, 0, 1001), 1);
        // Living space 15 from attribute 0x1B, raw; the limit 10 is reached -> message 11.
        assert_eq!(sim.state.players[0].resource_raw(4), living + (15 << 12));
        let tile = sim.object(house).unwrap().tile;
        assert_eq!(
            sim.state.events,
            [
                SimEvent::Sound {
                    object: house,
                    sound: SFX_CREATED
                },
                SimEvent::PlayerMessage {
                    to: Some(0),
                    id: 11,
                    tile,
                    // Living space full is not submitted: priority 3 without `0x10000`.
                    flags: 0x0300_0000,
                }
            ]
        );
        // Quarters: not free until the end of construction.
        let quarter_free = |sim: &Sim| {
            sim.module(hq, crate::module::quarter::INDEX)
                .unwrap()
                .base
                .has_flags(crate::module::quarter::flags::FREE_FOR_QUARTER)
        };
        assert!(!quarter_free(&sim));
        building_mut(&mut sim, hq).unwrap().build_counter = 10;
        build_tick(&mut sim, hq, 100);
        assert!(quarter_free(&sim));
    }

    #[test]
    fn the_next_own_site_within_ten_tiles_is_handed_over() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), false);
        // Foreign (3 tiles), finished (5 tiles), own at 9 tiles, own at 12 tiles.
        place(&mut sim, 1001, 1, (23, 20), false);
        place(&mut sim, 1001, 0, (25, 20), true);
        let next = place(&mut sim, 1001, 0, (29, 20), false);
        place(&mut sim, 1001, 0, (20, 32), false);
        building_mut(&mut sim, house).unwrap().build_counter = 10;
        let (r, m) = build_tick(&mut sim, house, 100);
        assert_eq!((r, m.arg(1)), (2, next.as_i32()));
        // A finished, intact building also sends the worker on (here only with the share of the
        // shortcut: fill up).
        sim.object_mut(house).unwrap().attributes.fill_current(14);
        let (r, m) = build_tick(&mut sim, house, 100);
        assert_eq!((r, m.arg(1)), (2, next.as_i32()));
        // Without a construction site in range: 0.
        let lone = place(&mut sim, 1001, 0, (50, 50), true);
        assert_eq!(build_tick(&mut sim, lone, 100).0, 0);
    }

    #[test]
    fn repairs_cost_the_owner_wood_and_announce_once_when_poor() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        sim.object_mut(house)
            .unwrap()
            .attributes
            .set_current_value(14, 1, 0);
        let frac = sim.object(house).unwrap().attributes.current_fraction(14);
        sim.state.players[0].set_resource_raw(1, 10_000);
        let (r, _) = build_tick(&mut sim, house, 100);
        assert_eq!(r, 1);
        // CostPerTick 307 raw * 200 % -> -614; HpPerTick 51 * 2 -> +102.
        assert_eq!(sim.state.players[0].resource_raw(1), 10_000 - 614);
        let o = sim.object(house).unwrap();
        assert_eq!(o.attributes.current_fraction(14), frac + 102);
        // Without wood: 0, message 8 once.
        sim.state.players[0].set_resource_raw(1, 100);
        sim.state.events.clear();
        assert_eq!(build_tick(&mut sim, house, 100).0, 0);
        assert_eq!(build_tick(&mut sim, house, 100).0, 0);
        let tile = sim.object(house).unwrap().tile;
        assert_eq!(
            sim.state.events,
            [SimEvent::PlayerMessage {
                to: Some(0),
                id: 8,
                tile,
                flags: 0x0301_0000,
            }]
        );
        assert!(building(&sim, house).unwrap().repair_msg_shown);
        assert_eq!(sim.state.players[0].resource_raw(1), 100);
        // Wood again: the flag is cleared.
        sim.state.players[0].set_resource_raw(1, 10_000);
        assert_eq!(build_tick(&mut sim, house, 100).0, 1);
        assert!(!building(&sim, house).unwrap().repair_msg_shown);
    }

    #[test]
    fn an_animated_building_draws_exactly_one_table_random_on_completion() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let mill = place(&mut sim, 1002, 0, (20, 20), false);
        let house = place(&mut sim, 1001, 0, (40, 40), false);
        let start = sim.state.rng.index();
        building_mut(&mut sim, house).unwrap().build_counter = 10;
        build_tick(&mut sim, house, 100);
        assert_eq!(sim.state.rng.index(), start);
        building_mut(&mut sim, mill).unwrap().build_counter = 10;
        build_tick(&mut sim, mill, 100);
        assert_eq!(sim.state.rng.index(), start + 1);
        let r = i32::from(crate::rng::RAND_TABLE[usize::from(start) + 1]);
        let expected = [4, 7, 9][(r % 3) as usize];
        assert_eq!(building(&sim, mill).unwrap().cur_anim, expected);
        assert_eq!(sim.object(mill).unwrap().anim.wanted, expected);
    }

    #[test]
    fn the_first_building_tick_takes_a_site_out_of_the_tile_map() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let site = place(&mut sim, 1001, 0, (20, 20), false);
        let ready = place(&mut sim, 1001, 0, (40, 40), true);
        assert!(building(&sim, site).unwrap().heightmap_inserted);
        crate::module::tick(&mut sim, site, INDEX, 100);
        crate::module::tick(&mut sim, ready, INDEX, 100);
        assert!(!building(&sim, site).unwrap().heightmap_inserted);
        assert!(building(&sim, ready).unwrap().heightmap_inserted);
        // More than half built: stays (remainder < BuildTime/2).
        let b = building_mut(&mut sim, site).unwrap();
        b.heightmap_inserted = true;
        b.build_counter = 999;
        crate::module::tick(&mut sim, site, INDEX, 100);
        assert!(building(&sim, site).unwrap().heightmap_inserted);
    }

    #[test]
    fn the_tile_map_is_stamped_above_seventy_percent_and_cleared_by_the_building_tick() {
        let mut rules = rules();
        rules.building_shapes.insert(
            1001,
            BuildingShape {
                center: [0.0; 3],
                half: [300.0, 50.0, 100.0],
                free_nodes: Vec::new(),
                helper_nodes: Default::default(),
            },
        );
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let site = place(&mut sim, 1001, 0, (20, 20), false);
        let stamped = |sim: &Sim| {
            sim.state
                .tile_map
                .occupied()
                .map(|(_, _, hid)| hid)
                .collect::<Vec<_>>()
        };
        // Created: the construction site is in the map (`GameWorld_CreateObject`).
        assert_eq!(stamped(&sim), vec![site.as_i32(); 5]);
        // First building tick (remainder >= BuildTime/2): `BuildingTileMap_RemoveObjectId`.
        crate::module::tick(&mut sim, site, INDEX, 100);
        assert!(!building(&sim, site).unwrap().heightmap_inserted);
        assert!(stamped(&sim).is_empty());
        // Exactly 70 % progress is not enough (`progress > 70`).
        let time = building(&sim, site).unwrap().build_time;
        building_mut(&mut sim, site).unwrap().build_counter = time * 3 / 10;
        build_tick(&mut sim, site, 100);
        assert!(!building(&sim, site).unwrap().heightmap_inserted);
        assert!(stamped(&sim).is_empty());
        // Above 70 %: `BuildingTileMap_AddBuilding(go, 1)`.
        building_mut(&mut sim, site).unwrap().build_counter = time / 10;
        build_tick(&mut sim, site, 100);
        assert!(building(&sim, site).unwrap().heightmap_inserted);
        assert_eq!(stamped(&sim), vec![site.as_i32(); 5]);
    }

    #[test]
    fn damaged_buildings_burn_down_below_forty_or_ten_percent_in_campaigns() {
        let rules = rules();
        for (mode, limit) in [(0, 40), (-1, 10)] {
            let mut state = SimState::new(mode, 0);
            state.init_map(&rules);
            let mut sim = Sim::new(&mut state, &rules);
            let hq = place(&mut sim, 1000, 0, (20, 20), true);
            assert!(building(&sim, hq).unwrap().burn_down);
            let set = |sim: &mut Sim, f: i32| {
                sim.object_mut(hq)
                    .unwrap()
                    .attributes
                    .set_current_fraction(14, f)
            };
            let frac = |sim: &Sim| sim.object(hq).unwrap().attributes.current_fraction(14);
            // Not at the limit.
            set(&mut sim, limit * 1000);
            crate::module::tick(&mut sim, hq, INDEX, 100);
            assert_eq!(frac(&sim), limit * 1000);
            assert_eq!(sim.object(hq).unwrap().render_bits & 0x21, 0x20);
            // Below it -7 per tick (-300000 / 40000, direction 0); burns, scale p * 2.5 * 0.01.
            set(&mut sim, limit * 1000 - 1);
            crate::module::tick(&mut sim, hq, INDEX, 100);
            assert_eq!(frac(&sim), limit * 1000 - 8);
            let o = sim.object(hq).unwrap();
            assert_eq!(o.render_bits & 0x21, 0x20);
            let p = (limit - 1) as f32;
            assert_eq!(o.progress_scale.to_bits(), (p * 2.5 * 0.01f32).to_bits());
            // Above 40 %: image "intact".
            set(&mut sim, 41_000);
            crate::module::tick(&mut sim, hq, INDEX, 100);
            assert_eq!(sim.object(hq).unwrap().render_bits & 0x21, 1);
        }
        // Without BURNDOWN nothing burns down.
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        sim.object_mut(house)
            .unwrap()
            .attributes
            .set_current_fraction(14, 5000);
        crate::module::tick(&mut sim, house, INDEX, 100);
        assert_eq!(
            sim.object(house).unwrap().attributes.current_fraction(14),
            5000
        );
    }

    #[test]
    fn wall_links_forward_steps_to_their_first_unfinished_member() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let master = place(&mut sim, 1011, 0, (20, 20), true);
        let a = place(&mut sim, 1011, 0, (21, 20), true);
        let b = place(&mut sim, 1011, 0, (22, 20), false);
        // 0x1B03 {k, ids}: the master skips itself, the members get 0x1D03.
        let list = Message::new(
            bmsg::SET_SLAVE_LIST,
            &[3, a.as_i32(), master.as_i32(), b.as_i32()],
        );
        assert_eq!(send(&mut sim, master, &list), 0);
        assert_eq!(building(&sim, master).unwrap().slaves, [a, b]);
        assert_eq!(building(&sim, a).unwrap().master, master);
        let mut q = Message::new(bmsg::GET_MASTER, &[]);
        send_mut(&mut sim, a, &mut q);
        assert_eq!(q.args, [master.as_i32()]);
        let mut q = Message::new(bmsg::GET_MASTER, &[]);
        send_mut(&mut sim, master, &mut q);
        assert_eq!(q.args, [master.as_i32()]);
        // The finished, intact master forwards to the first unfinished member (b), also via a
        // member (a -> master -> b).
        let before = building(&sim, b).unwrap().build_counter;
        assert_eq!(build_tick(&mut sim, master, 100).0, 1);
        assert_eq!(build_tick(&mut sim, a, 100).0, 1);
        assert_eq!(building(&sim, b).unwrap().build_counter, before - 20);
        // 0x1C03 removes, 0x2103 appends.
        send(
            &mut sim,
            master,
            &Message::new(bmsg::REMOVE_SLAVE, &[b.as_i32()]),
        );
        assert_eq!(building(&sim, master).unwrap().slaves, [a]);
        send(
            &mut sim,
            master,
            &Message::new(bmsg::ADD_SLAVE, &[b.as_i32()]),
        );
        assert_eq!(building(&sim, master).unwrap().slaves, [a, b]);
        // 0x1D03 forgets the own members.
        send(
            &mut sim,
            master,
            &Message::new(bmsg::SET_MASTER, &[a.as_i32()]),
        );
        let m = building(&sim, master).unwrap();
        assert_eq!((m.master, m.slaves.len()), (a, 0));
    }

    #[test]
    fn a_link_whose_master_is_gone_answers_finished() {
        // Exception A7 (reachable only via a savegame; the original calls `SendMessage` on a null
        // pointer): answer 0 so that the construction worker ends instead of sending endlessly.
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let gone = place(&mut sim, 1011, 0, (20, 20), true);
        let link = place(&mut sim, 1011, 0, (21, 20), true);
        send(
            &mut sim,
            link,
            &Message::new(bmsg::SET_MASTER, &[gone.as_i32()]),
        );
        crate::world::destroy_object(&mut sim, gone);
        assert!(sim.object(gone).is_none());
        let before = row(&sim, link);
        assert_eq!(build_tick(&mut sim, link, 100).0, 0);
        assert_eq!(row(&sim, link), before);
    }

    #[test]
    fn start_build_turns_a_building_back_into_a_site() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let house = place(&mut sim, 1001, 0, (20, 20), true);
        assert_eq!(
            send(&mut sim, house, &Message::new(bmsg::START_BUILD, &[])),
            0
        );
        let b = building(&sim, house).unwrap();
        assert!(b.is_build_building);
        assert_eq!(b.build_counter, 2000);
        assert_eq!(flags_of(&sim, house), 0);
        assert_eq!(sim.object(house).unwrap().render_bits, 4);
        assert_eq!(crate::economy::pending_guid_count(&sim, 0, 1001), 1);
        // Sleeping without members: no step.
        building_mut(&mut sim, house).unwrap().is_sleeping = true;
        assert_eq!(build_tick(&mut sim, house, 100).0, 0);
        assert_eq!(building(&sim, house).unwrap().build_counter, 2000);
    }
}
