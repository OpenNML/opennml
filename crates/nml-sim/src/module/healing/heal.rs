//! The healing flows of module 29 (re/healing 3.6, 3.7): area healing of the healing buildings
//! and shipyards ([`heal_area`]) and single healing of the healer units ([`single_heal`] with the
//! healing step [`apply`]). Both run from the tick (re/healing 3.1) as long as the module is ready.
//!
//! The target effects (`heilen.pef` in the two effect maps with `m_UseTargetMap1`) and the
//! hit list `+0x70` are presentation or a temporary store and are missing from the state: for each
//! healed target and tick the simulation reports [`SimEvent::Healed`], the host keeps the effect
//! as long as the message arrives every object tick.

use crate::collision::{
    edge_distance_to_point, find_nearest_matching, fixed_pos_distance, query_radius,
};
use crate::filter;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::module::gather::{list_at, set_animation};
use crate::sim::Sim;
use crate::state::SimEvent;

use super::{INDEX, flags, healing, healing_mut, match_filter};

/// Hit points (attribute `0xE`).
const HITPOINTS: u32 = 14;
/// Extra energy (attribute `0xF`): the mana of the healers.
const MANA: u32 = 15;
/// Factor of single healing between values and healing amount per minute (`IMUL ..., 0x3C`, C-243).
const MANA_FACTOR: i32 = 60;
/// Minimum amount of single healing per tick (`0x00519F64`): 60 per minute = 0.1 HP.
const MIN_POINTS: i32 = 60;
/// `FINDTO_UNIT` to the own walk (re/walk 2.5).
const FIND_TO_UNIT: u32 = 0x400;
/// Sight (attribute `0x12`): radius of area healing.
const SIGHT: u32 = 18;
/// Object ticks per minute: scale of `AttributeSet_AddScaledToCurrent` (healing amount per minute).
const TICKS_PER_MINUTE: i32 = 600;
/// `0x910 {0}`: cure plague (dying module, re/combat-damage 1.5).
const CURE_PEST: u32 = 0x910;

/// `PropertyHealing_HealArea` (`0x00519830`, re/healing 3.6) without the effect maps: only with
/// healing amount > 0. Radius `3 * (sight << 10) / 4` (`0x005198C1...D4`, truncated; sight 40 ->
/// 30,720 = 7.5 tiles), query `SpatialGrid_QueryRadius` (`0x004B6CD0`) around the center with
/// the healing filter ([`match_filter`]) for the current owner (center distance <= r). For each hit
/// in order of discovery:
/// - `HEALINGONLYGUIDS` and GUID not in `remove_anim` -> **loop end** (`0x0051997A`, C-242):
///   a soldier in front of the ship stops the shipyard for this tick;
/// - not full (`AttributeSet_IsFull` `0x0048F5C0`; the filter has already checked this with
///   `ENERGYLOST`) -> `AddScaledToCurrent(14, H, 600)` (`0x005199AF`, as W-135: from
///   `Max * 600 > H * 100 000` on no gain) and [`SimEvent::Healed`];
/// - always `0x910 {0}` (cure plague, even without healing).
///
/// Only own, non-quartered walk objects (`MYLIST`, `QUARTERED` pre-test), without swimming,
/// diving or riding exception (C-246). The hits are IDs; a target that disappears along the way
/// is skipped.
pub fn heal_area(sim: &mut Sim, id: ObjectId) {
    let Some(h) = healing(sim, id) else { return };
    let points = h.healing_points;
    if points <= 0 {
        return;
    }
    let guids = h.remove_anim.clone();
    let Some(m) = sim.module(id, INDEX) else {
        return;
    };
    let word = m.base.flags;
    let only_guids = m.base.has_flags(flags::ONLYGUIDS);
    let Some(o) = sim.object(id) else { return };
    let pos = (o.grid.x.0, o.grid.z.0);
    let player = i32::from(o.owner());
    // `AttributeSet_GetCurrentValue(0x12)`: sight has no current value, so the maximum with
    // the upgrades of the owner.
    let sight = sim.attribute(id, SIGHT);
    let radius = 3 * (sight << 10) / 4;
    let hits = query_radius(sim, pos, &match_filter(word), radius, player, false);
    for target in hits {
        let Some(t) = sim.object(target) else {
            continue;
        };
        if only_guids && !guids.iter().any(|&g| g as u32 == t.guid) {
            break;
        }
        if !t.attributes.is_full(HITPOINTS) {
            let percent = sim.type_upgrade_percent(t.owner(), t.guid, HITPOINTS);
            if let Some(t) = sim.object_mut(target) {
                t.attributes
                    .add_scaled_to_current(HITPOINTS, points, TICKS_PER_MINUTE, percent);
            }
            sim.state.events.push(SimEvent::Healed {
                healer: id,
                target,
                points,
            });
        }
        send(sim, target, &Message::new(CURE_PEST, &[0]));
    }
}

/// Resolve an ID like `GameWorld_GetObjectById(id, 0)` (`0x00475300`): 0 is none, the dead count
/// (a carcass gets a new ID after the change to slot 8 anyway, C-142).
fn resolve(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    (id != ObjectId(0) && sim.object(id).is_some()).then_some(id)
}

/// Single healing of the healer units (`0x00519C00`, re/healing 3.7) without the target effect (host).
/// Range `r = Fixed_FromInt(sight) >> 4` (= sight * 256; medic 10,240, pilgrim
/// 8,192). Three distance measures (C-244):
/// - **Keeping** the current target `m_SickPersonHID` (also dead, [`resolve`]): not full,
///   edge distance to the healer center <= r (`GameObject_GetEdgeDistanceToPoint` `0x00468930`) and no
///   requested target -> straight to [`apply`]; otherwise given up.
/// - **Search** without a requested target: `m_SickPersonHID = 0`, then
///   `CollisionMgr_FindNearestMatching` (`0x004B7620`) with the healing filter for the owner in
///   edge mode with the own hard radius (hit: edge distance <= r + r_self). A target in the
///   band `(r, r + r_self]` is thus given up and found again every tick; the injured
///   healer itself stands at distance 0 in its bucket and heals itself first (C-245).
/// - **Requested target** `m_NewTarget` (`0x81D`, one equal to the current one does not count): at
///   center distance <= r (`FixedPos_Distance` `0x00401200`, without radii) taken over without a filter
///   check and `m_NewTarget = 0`. Otherwise, if it satisfies the healing filter (player = its
///   owner), `0x400` (`FINDTO_UNIT`) to the own walk and end - in the trance walk is off,
///   the order goes nowhere; as long as `m_NewTarget` is set, the healer heals nobody (C-251).
///
/// A new target becomes `m_SickPersonHID`; with a target [`apply`], without a target the stand animation
/// `m_StandAnim[0]`.
pub fn single_heal(sim: &mut Sim, id: ObjectId) {
    let Some(h) = healing(sim, id) else { return };
    let (sick, requested, stand) = (h.sick, h.new_target, list_at(&h.stand_anim, 0));
    let Some(m) = sim.module(id, INDEX) else {
        return;
    };
    let filter = match_filter(m.base.flags);
    let Some(o) = sim.object(id) else { return };
    let pos = (o.grid.x.0, o.grid.z.0);
    let (player, own_radius) = (i32::from(o.owner()), o.hard_radius);
    let mut target = resolve(sim, sick);
    // `Fixed_FromInt(GetCurrentValue(0x12)) >> 4` (`0x00519C4B...5D`, SAR).
    let r = sim.attribute(id, SIGHT).wrapping_shl(12) >> 4;
    let mut request = resolve(sim, requested);
    if request == target {
        request = None;
    }
    if let Some(current) = target {
        let far = edge_distance_to_point(sim, current, pos) > r;
        let full = sim
            .object(current)
            .is_some_and(|t| t.attributes.is_full(HITPOINTS));
        if !full && !far && request.is_none() {
            apply(sim, id, current);
            return;
        }
        // The host deletes the target effect on the old target.
        target = None;
    }
    match request {
        None => {
            if let Some(h) = healing_mut(sim, id) {
                h.sick = ObjectId(0);
            }
            target = find_nearest_matching(sim, pos, &filter, r, player, own_radius);
        }
        Some(q) => {
            let q_pos = sim.object(q).map_or((0, 0), |t| (t.grid.x.0, t.grid.z.0));
            if fixed_pos_distance(pos, q_pos) <= r {
                if let Some(h) = healing_mut(sim, id) {
                    h.new_target = ObjectId(0);
                }
                target = Some(q);
                request = None;
            }
        }
    }
    if let Some(n) = target
        && let Some(h) = healing_mut(sim, id)
    {
        h.sick = n;
    }
    if let Some(q) = request
        && filter::matches(sim, q, &filter, -1)
    {
        // `0x400 {target, 0, r - r_target - r_self, 0, -1, 1, 0, 0}` (`0x00519E4F...A6`).
        let q_radius = sim.object(q).map_or(0, |t| t.hard_radius);
        let distance = r.wrapping_sub(q_radius).wrapping_sub(own_radius);
        let order = Message::new(FIND_TO_UNIT, &[q.as_i32(), 0, distance, 0, -1, 1, 0, 0]);
        send(sim, id, &order);
        return;
    }
    match target {
        Some(n) => apply(sim, id, n),
        None => set_animation(sim, id, stand),
    }
}

/// Healing step of single healing (`0x00519EBE...0x0051A066`, re/healing 3.7, C-243) with the healing
/// amount H per minute:
/// - `pay = Mana * 60` if `H >= Mana * 60`, otherwise H (mana = `GetCurrentValue(0xF)`, rounded);
///   `pay <= 0` -> stand animation `m_StandAnim[0]`, otherwise nothing;
/// - `need = (Max - Current) * 60` of the target (integer values), `v = min(max(min(H, need), 60), pay)`
///   (final stretch: 1/10 of the shortfall per tick, at least 0.1 HP);
/// - target `AddScaledToCurrent(0xE, v, 600)`, healer `AddScaledToCurrent(0xF, -v, 600)`,
///   healing animation `m_HealAnim`, [`SimEvent::Healed`] with H, `0x910 {0}` to the target.
///
/// The target is not checked: full, foreign or dying objects are healed and paid for as well (a full
/// requested target costs mana for one tick without any gain).
pub fn apply(sim: &mut Sim, id: ObjectId, target: ObjectId) {
    let Some(h) = healing(sim, id) else { return };
    let (points, stand, heal_anim) = (h.healing_points, list_at(&h.stand_anim, 0), h.heal_anim);
    let mana = sim.attribute(id, MANA);
    let cap = mana.wrapping_mul(MANA_FACTOR);
    let pay = if points >= cap { cap } else { points };
    let Some(t) = sim.object(target).filter(|_| pay > 0) else {
        set_animation(sim, id, stand);
        return;
    };
    let percent = sim.type_upgrade_percent(t.owner(), t.guid, HITPOINTS);
    let need = t
        .attributes
        .max_value(HITPOINTS, percent)
        .wrapping_sub(t.attributes.current_value(HITPOINTS, percent))
        .wrapping_mul(MANA_FACTOR);
    let v = if points >= need { need } else { points };
    let v = if v < MIN_POINTS { MIN_POINTS } else { v };
    let v = if v < pay { v } else { pay };
    if let Some(t) = sim.object_mut(target) {
        t.attributes
            .add_scaled_to_current(HITPOINTS, v, TICKS_PER_MINUTE, percent);
    }
    if let Some(o) = sim.object(id) {
        let mana_percent = sim.type_upgrade_percent(o.owner(), o.guid, MANA);
        if let Some(o) = sim.object_mut(id) {
            o.attributes.add_scaled_to_current(
                MANA,
                v.wrapping_neg(),
                TICKS_PER_MINUTE,
                mana_percent,
            );
        }
    }
    set_animation(sim, id, heal_anim);
    sim.state.events.push(SimEvent::Healed {
        healer: id,
        target,
        points,
    });
    send(sim, target, &Message::new(CURE_PEST, &[0]));
}
