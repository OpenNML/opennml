//! Flow of the headhunter (re/headhunter-robbing 3.3-3.6, 3.10): target search, approach,
//! way home and tick, melee kill with success and dynamite throw (3.4, 3.5).
//!
//! Re-entrancy: `dispatch_command` can deliver the feedback messages synchronously (target already
//! within the radius, walk plan error). Therefore [`seek_target`] and [`go_home`] write all fields
//! **before** the call and change nothing afterwards; no borrow is held across the call. Likewise
//! the swimming melee kill calls [`on_kill`] synchronously, and [`on_success`] ends with [`go_home`].

use opennml_formats::targets::MatchFilter;

use crate::collision::{find_nearest_matching, quartering_container};
use crate::combat::hit_zone::{hit_zone_world_pos, world_pos};
use crate::combat::projectile::launch_throw;
use crate::command::headhunter::{COLOR_GREEN, SFX_SUCCESS, TEXT_SUCCESS};
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send};
use crate::module::special::{FINDTO, FINDTO_UNIT, post};
use crate::object::status;
use crate::sim::Sim;
use crate::state::{SimEvent, TextArg};
use crate::world::queue_destroy;

use super::{DEFAULT_PRICE, FAR, GROUP, INDEX, NEAR, headhunter, headhunter_mut, msg};

/// Search filter `MM_HL_MYLIST | MM_ANDALL | MM_CHECKGUID` (`0x118`, `0x00518490`).
const SEEK_MODE: u32 = 0x118;
/// `MM2_DONOTIGNOREQUARTERED` (`0x00518490`): quartered units count.
const SEEK_MODE2: u32 = 4;
/// Way-home filter `MM_CHECKPROPERTY` (`0x005182D0`), OR mode only with module 3.
const HOME_MODE: u32 = 0x4;
/// Building module (`PROP_BUILDING`).
const BUILDING: usize = 3;
/// Damage of the melee kill and the impact: -110 % of the hit points (`0x27F`, 2.3).
const KILL_PERCENT: i32 = -110;
/// Explosion at the target on impact (`0x00622BF0`, 1.4; presentation only).
const EXPLOSION: &str = "data/effects/explosion_pulverhaus.pef";
/// Mesh of the dynamite in the hand (`0x00624624`), hidden during the throw.
const DYNAMITE_MESH: &str = "cylinder01";

/// `0x322` "think" (`0x00518760`, jump table `0x005187B8`, 3.3): state 0 -> [`seek_target`],
/// 1 to 5 -> [`retry`] (C-317), returns 1; otherwise 0.
pub fn think(sim: &mut Sim, id: ObjectId) -> i32 {
    match headhunter(sim, id).map(|h| h.state) {
        Some(0) => {
            seek_target(sim, id);
            1
        }
        Some(1..=5) => {
            retry(sim, id);
            1
        }
        _ => 0,
    }
}

/// `0x00517F60` (via `0x00518020`): state 0 and `0x322` after 2000 ms in group `0x80`.
pub fn retry(sim: &mut Sim, id: ObjectId) {
    if let Some(h) = headhunter_mut(sim, id) {
        h.state = 0;
    }
    post(sim, id, INDEX, 2000, Message::new(msg::THINK, &[]));
}

/// Target search (`0x00518490`, 3.3, C-312): the nearest object of the target GUID at the target
/// player on the whole map (center distance, without fog, also quartered units). None -> [`go_home`].
/// Otherwise `target_hid`; building (module 3) or quartered -> state 2 with 4 tiles, where a
/// quartered unit without module 3 is replaced by its container (without a container: end without
/// an order, the tick catches that, 3.10); otherwise state 1 with 1 tile. Then `last_target` and
/// FINDTO_UNIT `{target, 0, distance, 0, -1, 1, 0, 0}` with `0x822`/`0x922` to itself.
pub fn seek_target(sim: &mut Sim, id: ObjectId) {
    let (Some(h), Some(o)) = (headhunter(sim, id), sim.object(id)) else {
        return;
    };
    let (guid, player) = (h.target_guid, h.target_player);
    let pos = (o.grid.x.0, o.grid.z.0);
    let f = MatchFilter {
        mode: SEEK_MODE,
        mode2: SEEK_MODE2,
        guid,
        ..MatchFilter::constructed()
    };
    let Some(found) = find_nearest_matching(sim, pos, &f, -1, player, -1) else {
        go_home(sim, id);
        return;
    };
    if let Some(h) = headhunter_mut(sim, id) {
        h.target_hid = found;
    }
    let Some((building, quartered)) = sim.object(found).map(|t| {
        (
            t.has_module(BUILDING, false),
            t.status & status::QUARTERED != 0,
        )
    }) else {
        return;
    };
    let mut target = found;
    let (dist, state) = if building || quartered {
        if quartered && !building {
            match quartering_container(sim, found) {
                Some(c) => target = c,
                None => return,
            }
        }
        (FAR, 2)
    } else {
        (NEAR, 1)
    };
    if let Some(h) = headhunter_mut(sim, id) {
        h.state = state;
        h.last_target = target;
    }
    let env = Envelope {
        msg: Message::new(FINDTO_UNIT, &[target.as_i32(), 0, dist, 0, -1, 1, 0, 0]),
        success: Some(Message::new(msg::WALK_OK, &[])),
        fault: Some(Message::new(msg::WALK_FAULT, &[])),
        success_target: id,
        fault_target: id,
    };
    dispatch_command(sim, id, &env);
}

/// `0x822` walk success (`0x00518800`, jump table `0x00518878`, 3.3): 1 -> start the melee kill,
/// 2 -> `last_target` is missing (`GetObjectById(..., false)`, the dead also count) -> state 0 and
/// [`think`] (returns 0), otherwise dynamite at the building; 4/5 -> [`think`]; state 0 and 3: nothing
/// (C-317).
pub fn on_walk_success(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some((state, last)) = headhunter(sim, id).map(|h| (h.state, h.last_target)) else {
        return 0;
    };
    match state {
        1 => {
            start_near_kill(sim, id, last);
            1
        }
        2 => {
            if sim.state.object(last, false).is_none() {
                if let Some(h) = headhunter_mut(sim, id) {
                    h.state = 0;
                }
                think(sim, id);
                0
            } else {
                throw_at_building(sim, id);
                1
            }
        }
        4 | 5 => {
            think(sim, id);
            1
        }
        _ => 0,
    }
}

/// `0x922` walk fault (`0x005186D0`, jump table `0x00518718`, 3.3): 1 -> dynamite at the unit
/// (C-313), 2 -> dynamite at the building, 4/5 -> [`go_home`] (returns 1); otherwise [`retry`], returns 0.
pub fn on_walk_fault(sim: &mut Sim, id: ObjectId) -> i32 {
    match headhunter(sim, id).map(|h| h.state) {
        Some(1) => {
            throw_at_unit(sim, id);
            1
        }
        Some(2) => {
            throw_at_building(sim, id);
            1
        }
        Some(4 | 5) => {
            go_home(sim, id);
            1
        }
        Some(_) => {
            retry(sim, id);
            0
        }
        None => 0,
    }
}

/// Way home (`0x005182D0`, 3.6, C-316): the nearest building of **any** owner (filter only
/// "module 3 switched on", also foreign, neutral and unfinished ones) on the whole map, edge distance
/// with the own radius (`+0x1B0`), player = own slot (8). None -> debug text "vanishing
/// directly" and `0x522` to itself; otherwise FINDTO `{building, -1, 1}` with `0x522` as success and
/// fault.
pub fn go_home(sim: &mut Sim, id: ObjectId) {
    let Some(o) = sim.object(id) else {
        return;
    };
    let (pos, owner, hard) = (
        (o.grid.x.0, o.grid.z.0),
        i32::from(o.owner()),
        o.hard_radius,
    );
    let f = MatchFilter {
        mode: HOME_MODE,
        mode2: 0,
        prop_id: BUILDING as u32,
        ..MatchFilter::constructed()
    };
    let Some(b) = find_nearest_matching(sim, pos, &f, -1, owner, hard) else {
        log::debug!("Headhunter {id:?}: vanishing directly");
        vanish(sim, id);
        return;
    };
    let env = Envelope {
        msg: Message::new(FINDTO, &[b.as_i32(), -1, 1]),
        success: Some(Message::new(msg::VANISH, &[])),
        fault: Some(Message::new(msg::VANISH, &[])),
        success_target: id,
        fault_target: id,
    };
    dispatch_command(sim, id, &env);
}

/// `0x522` (`0x00517D90`, 3.6): `GameWorld_QueueObjectDestroy(own HID)` - without dying and
/// without refund; the destructor clears the table entry and the sight source. Returns 0.
pub fn vanish(sim: &mut Sim, id: ObjectId) -> i32 {
    queue_destroy(sim, id);
    0
}

/// `0x622` (`0x00517DB0`): clear group `0x80`, `0x322` after 1000 ms. Returns 1.
pub fn restart(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(m) = sim.module_mut(id, INDEX) else {
        return 1;
    };
    m.base.clear_event_group(GROUP);
    post(sim, id, INDEX, 1000, Message::new(msg::THINK, &[]));
    1
}

/// Tick (`0x00518730`, 3.10, C-324) after `Module_UpdateEvents`: without `MOVE`, without
/// `ANIM_LOCKED` (`0x800001`) and without events (`m_Events`, original `+0x50 <= 0`) -> [`go_home`].
/// After `update_events` `m_TimersWS` is always empty (step D takes over everything), so "`events` empty"
/// is exactly the condition of the original; inactive entries (only from a corrupted savegame) count
/// as in the original.
pub fn after_update_events(sim: &mut Sim, id: ObjectId) {
    let Some(st) = sim.object(id).map(|o| o.status) else {
        return;
    };
    let Some(m) = sim.module(id, INDEX) else {
        return;
    };
    debug_assert!(m.base.timers_ws.is_empty());
    if st & (status::MOVE | status::ANIM_LOCKED) == 0 && m.base.events.is_empty() {
        go_home(sim, id);
    }
}

/// Start the melee kill (`0x00517CE0`, also `0x722`, 3.4): if the headhunter is swimming (`SWIMMING`,
/// byte `+0x1AE` bit 0), it calls the query with `0x422 {target}` immediately - without animation, lock
/// and timer (C-319). Otherwise lock `0x800000` on, `m_KillAnimNear` (speed 128) and
/// `0x422 {target}` at time marker 1 (group `0x80`).
pub fn start_near_kill(sim: &mut Sim, id: ObjectId, target: ObjectId) {
    let Some(anim) = headhunter(sim, id).map(|h| h.kill_anim_near) else {
        return;
    };
    let Some(o) = sim.object_mut(id) else {
        return;
    };
    if o.status & status::SWIMMING != 0 {
        on_kill(sim, id, target);
        return;
    }
    o.set_status_flags(status::ANIM_LOCKED, true);
    o.set_animation(anim, 0, false, false, false);
    let guid = o.guid;
    let t = sim.rules.anim_label_time_at(guid, anim, 1);
    post(
        sim,
        id,
        INDEX,
        t,
        Message::new(msg::KILL, &[target.as_i32()]),
    );
}

/// `0x422 {target}` melee kill (`0x005190B0`, 3.4): lock off; if the target exists (`GetObjectById(...,
/// false)`, also dead), `0x27F {-110, headhunter}` at it and [`on_success`], otherwise [`retry`]. Returns 0.
pub fn on_kill(sim: &mut Sim, id: ObjectId, target: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    if sim.object(target).is_some() {
        send(
            sim,
            target,
            &Message::new(0x27F, &[KILL_PERCENT, id.as_i32()]),
        );
        on_success(sim, id);
    } else {
        retry(sim, id);
    }
    0
}

/// Success (`0x00519000`, 3.4, C-314): first the kill counter of the customer (`0x0051901C`,
/// `GameWorld+0x20E0[customer]` by target GUID, E-189; a customer outside 0 to 7 is discarded). Message
/// 10011 with the type name of the headhunter, green, 3000 ms, and sound `SFX_HEADHUNTERSUCCESS` to all
/// (D-238); then `0x005187D0`: price hard 500 (C-315), target player, target GUID and customer -1,
/// `target_hid` none, [`go_home`] - the state stays.
fn on_success(sim: &mut Sim, id: ObjectId) {
    if let Some((customer, target)) = headhunter(sim, id).map(|h| (h.customer, h.target_guid)) {
        crate::world::add_kill_count(sim, customer, target as u32);
    }
    let Some(guid) = sim.object(id).map(|o| o.guid) else {
        return;
    };
    sim.state.events.push(SimEvent::TextMessage {
        to: None,
        text: TEXT_SUCCESS,
        args: vec![TextArg::TypeName(guid)],
        color: COLOR_GREEN,
        duration_ms: 3000,
    });
    sim.state.events.push(SimEvent::Sound2d {
        name: SFX_SUCCESS.into(),
    });
    if let Some(h) = headhunter_mut(sim, id) {
        h.cur_reward = DEFAULT_PRICE;
        h.target_player = -1;
        h.target_guid = -1;
        h.customer = -1;
        h.target_hid = ObjectId::NONE;
    }
    go_home(sim, id);
}

/// Dynamite at the unit `last_target` (`0x00517EB0`, 3.5): throw `0xA22`.
fn throw_at_unit(sim: &mut Sim, id: ObjectId) {
    throw_anim(sim, id, msg::THROW_UNIT);
}

/// Dynamite at the building `last_target` (`0x00517E00`, 3.5): throw `0xB22`.
fn throw_at_building(sim: &mut Sim, id: ObjectId) {
    throw_anim(sim, id, msg::THROW_BUILDING);
}

/// Throw animation (`0x00517EB0`/`0x00517E00`, 3.5): `m_KillAnimFar` (speed 128), `throw`
/// at time marker 2 and `0xC22` at time marker 1 (group `0x80`), lock `0x800000` on.
fn throw_anim(sim: &mut Sim, id: ObjectId, throw: u32) {
    let Some(anim) = headhunter(sim, id).map(|h| h.kill_anim_far) else {
        return;
    };
    let Some(o) = sim.object_mut(id) else {
        return;
    };
    o.set_animation(anim, 0, false, false, false);
    let guid = o.guid;
    let t2 = sim.rules.anim_label_time_at(guid, anim, 2);
    let t1 = sim.rules.anim_label_time_at(guid, anim, 1);
    post(sim, id, INDEX, t2, Message::new(throw, &[0]));
    post(sim, id, INDEX, t1, Message::new(msg::THROW_END, &[0]));
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, true);
    }
}

/// `0xA22`/`0xB22` throw (`0x00518C60`/`0x005188C0`, 3.5) with the impact `impact`
/// (`0xD22`/`0xE22`): if `last_target` is missing (`GetObjectById(..., false)`), only the
/// unit throw retries ([`retry`]), returns 0. Otherwise throw at the hit zone 0 of the target
/// ([`launch_throw`], without target HID) and hide the dynamite in the hand; returns 1.
pub fn on_throw(sim: &mut Sim, id: ObjectId, impact: u32) -> i32 {
    let Some(last) = headhunter(sim, id).map(|h| h.last_target) else {
        return 0;
    };
    let Some(aim) = hit_zone_world_pos(sim, last, 0) else {
        if impact == msg::IMPACT_UNIT {
            retry(sim, id);
        }
        return 0;
    };
    launch_throw(sim, id, aim, impact);
    let starts = sim.object(id).map_or(0, |o| o.anim.starts);
    sim.state.events.push(SimEvent::MeshVisibility {
        object: id,
        names: DYNAMITE_MESH.into(),
        visible: false,
        anim_starts: starts,
    });
    1
}

/// `0xD22`/`0xE22` impact (`0x00518050`/`0x00518190`, 3.5): if `last_target` is missing, [`retry`] and
/// returns 0. Otherwise explosion at its world position (presentation), `0x27F {-110, headhunter}` and
/// for the building (`0xE22`) **always** [`retry`] (C-317); no success (C-314). Returns 1.
pub fn on_impact(sim: &mut Sim, id: ObjectId, building: bool) -> i32 {
    let Some(last) = headhunter(sim, id).map(|h| h.last_target) else {
        return 0;
    };
    let Some(pos) = world_pos(sim, last) else {
        retry(sim, id);
        return 0;
    };
    sim.state.events.push(SimEvent::EffectAt {
        path: EXPLOSION.into(),
        pos,
        yaw: 0.0,
        scale: 1.0,
        start_ms: 0,
    });
    send(
        sim,
        last,
        &Message::new(0x27F, &[KILL_PERCENT, id.as_i32()]),
    );
    if building {
        retry(sim, id);
    }
    1
}

/// `0xC22` end of the throw animation (`0x00518030`): lock off, [`retry`]. Returns 1.
pub fn on_throw_end(sim: &mut Sim, id: ObjectId) -> i32 {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(status::ANIM_LOCKED, false);
    }
    retry(sim, id);
    1
}
