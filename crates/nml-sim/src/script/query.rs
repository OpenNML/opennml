//! Query layer of the scripts (`FUN_00425F90`, re/script-conditions D.2).
//! Layer I: the queries 100 to 400, 500, 1200, 2300 and 2400, needed by conditions on counters and areas.
//! Layer II: the object-related queries 1400, 1500, 2100, 2200 and 2600. The codes are those of the
//! original; return values as there (`i32`). The layer only reads - except for [`area_seen`] (query 300),
//! which swaps the corners of the stored area (E-190).
//!
//! Players outside the table bounds return 0 (exception A7) where the original would read undefined data.
//! The load check (`nml_formats::abl::catalog::check`) rejects the documented crash case (query 100 with
//! kind 0 to 4 through a null pointer, E-193) and condition 33 with player < 0; the latter does not crash
//! but reads the local player of the machine (host state, E-191).

pub use super::area;

use nml_formats::targets::MatchFilter;

use crate::collision;
use crate::economy;
use crate::filter;
use crate::fog::FogView;
use crate::id::PLAYER_SLOTS;
use crate::math::atan2_deg;
use crate::module::walk;
use crate::names;
use crate::sim::Sim;
use crate::tile_map;
use crate::world;

/// Attribute "hit points" (`AttributeSet_GetCurrentFraction(0xE)`).
const HIT_POINTS: u32 = 14;
/// Attribute "sight" (`AttributeSet_GetCurrentValue(0x12)`).
const SIGHT: u32 = 18;

/// Slot as index 0 to 7 (`GameEngine_GetPlayer` `0x0042D560`: otherwise no player).
fn player_index(slot: i32) -> Option<usize> {
    usize::try_from(slot).ok().filter(|&s| s < PLAYER_SLOTS)
}

/// Does `FUN_0047E390` know the filter ID? A type GUID (>= 1000) always (also an unknown type), otherwise
/// only an entry of `flowcontroltragets.ini`; [`filter::from_id`] does not report this (it then returns
/// the default filter).
pub(super) fn filter_known(sim: &Sim, id: i32) -> bool {
    id >= 1000 || sim.rules.flowcontrol_target(id).is_some()
}

/// Filter of a filter ID as for 35, 197 and 204: a known ID ([`filter_known`]) via [`filter::from_id`],
/// an unknown one as a raw-value GUID filter (`mode 8`, `guid = id`).
pub(super) fn filter_or_raw_guid(sim: &Sim, id: i32) -> MatchFilter {
    if filter_known(sim, id) {
        filter::from_id(sim.rules, id)
    } else {
        MatchFilter {
            mode: 8,
            guid: id,
            ..MatchFilter::constructed()
        }
    }
}

/// Query 100 (`Player_GetResource` `0x004FAD30`): resource `kind` of slot `slot` as `raw >> 12` (arithmetic);
/// kind not in 0 to 4 -> 0, kind 4 (living space) at most the living space maximum; slot not in 0 to 7 -> 0.
pub fn resource(sim: &Sim, slot: i32, kind: i32) -> i32 {
    match (player_index(slot), usize::try_from(kind)) {
        (Some(s), Ok(k)) if k <= 4 => sim.state.resource(s, k),
        _ => 0,
    }
}

/// Query 200 (`FUN_00435B30`): number of objects that pass the filter `guid` (type GUID or filter ID) with
/// the reference player `slot` and whose **tile** lies in the area `area_name` (inclusive).
///
/// GUID 0, unknown area and unknown filter ID (< 1000 without an entry in `flowcontroltragets.ini`) count
/// 0. The corners are **not** swapped: a swapped rectangle counts 0 until [`area_seen`] has normalized it
/// (E-190). `slot < 0` takes each object's owner as the reference, 9 the tree list
/// ([`crate::area::collect_in_tile_rect`]).
pub fn count_in_area(sim: &mut Sim, slot: i32, guid: i32, area_name: &str) -> i32 {
    if guid == 0 {
        return 0;
    }
    let Some(rect) = area(sim, area_name) else {
        return 0;
    };
    if !filter_known(sim, guid) {
        return 0;
    }
    let f = filter::from_id(sim.rules, guid);
    let list = crate::area::collect_in_tile_rect(sim, rect, &f, slot);
    i32::try_from(list.len()).unwrap_or(i32::MAX)
}

/// Query 300 (`FUN_00435C50(p, area, 0)` -> `FUN_0047B650`, mask 2): does any tile of the area (inclusive)
/// have the fog state "currently seen" for the slot (`& 2`, shared vision included)?
///
/// Slot not in 0 to 7 or unknown area -> false, without a swap. Otherwise it swaps swapped corners
/// (`x0 > x1`, `z0 > z1`) in the **stored** rectangle (`ScriptState::area_overrides`), which affects 200,
/// 1300 and all further readers (E-190).
pub fn area_seen(sim: &mut Sim, slot: i32, area_name: &str) -> bool {
    let Some(rect) = area(sim, area_name) else {
        return false;
    };
    let Some(p) = player_index(slot) else {
        return false;
    };
    let [mut x0, mut z0, mut x1, mut z1] = rect;
    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
    }
    if z0 > z1 {
        std::mem::swap(&mut z0, &mut z1);
    }
    let rect_now = [x0, z0, x1, z1];
    if rect_now != rect {
        sim.state
            .script
            .area_overrides
            .insert(super::area_key(area_name), rect_now);
    }
    FogView::new(&*sim.state).rect(rect_now, p as i32) & 2 != 0
}

/// Query 400 (`FUN_004303F0(p, g, 0)`): number for a type GUID (object counter with groups) or filter ID
/// (object table of the slot, without dead and quartered), without orders. Slot not in 0 to 7 (also
/// neutral 8) and unknown filter ID count 0.
pub fn count(sim: &mut Sim, slot: i32, guid: i32) -> i32 {
    if player_index(slot).is_none() || !filter_known(sim, guid) {
        return 0;
    }
    economy::count_guid_or_filter(sim, slot, guid, false)
}

/// Query 500 (`FUN_004374A0`): center of the area `((x0 + x1) / 2, (z0 + z1) / 2)` with C division
/// (truncated toward zero) from the run-time table ([`area`], so with swapped or enlarged rectangles,
/// E-190); an unknown name gives `(0, 0)`. Reader: AI start 0/1.
pub fn area_center(sim: &Sim, name: &str) -> (i32, i32) {
    area(sim, name).map_or((0, 0), |[x0, z0, x1, z1]| {
        (x0.wrapping_add(x1) / 2, z0.wrapping_add(z1) / 2)
    })
}

/// Query 1200 (`FUN_0047E7F0`): kill counter of the slot for the GUID (groups resolved);
/// slot not in 0 to 7 -> 0.
pub fn kill_count(sim: &Sim, slot: i32, guid: i32) -> i32 {
    world::kill_count(sim, slot, guid as u32)
}

/// Queries 2300/2400 (`Player_GetGuidCount` `0x0047DD90`): object counter without filter IDs; slot not in 0 to 7 -> 0.
pub fn guid_count(sim: &Sim, slot: i32, guid: i32) -> i32 {
    economy::guid_count(sim, slot, guid as u32)
}

/// Query 1400 (`FUN_0042D980(id, 0xE)`): hit points of the first ID of the name in percent. Name
/// unknown or first ID 0 -> -1; no object any more (stale ID, `GetObjectById(id, 0)` - **with**
/// dying ones) -> 0; otherwise `current_fraction(14) / 1000` (full = 100,000, never negative, rounded down).
/// Condition 28 (`r < n && r != -1`) is therefore true for a stale ID with n > 0 (E-203).
pub fn hp_percent(sim: &Sim, name: &str) -> i32 {
    let Some(id) = names::first_id(&sim.state.names, name) else {
        return -1;
    };
    sim.object(id)
        .map_or(0, |o| o.attributes.current_fraction(HIT_POINTS) / 1000)
}

/// Query 1500 (`FUN_004315F0`): entry `index` of the AI global list (`m_listGlobal`) of the engine of
/// slot `slot`. Slot not in 0 to 7, no engine or `index >= length` (signed) -> 0; `index < 0` throws an
/// exception in the original and here is an error with an error message (exception A7), which condition 29
/// reports as false with `Guarded` (deviation from the original).
pub fn ki_global(sim: &Sim, slot: i32, index: i32) -> Result<i32, String> {
    let Some(p) = player_index(slot) else {
        return Ok(0);
    };
    let Some(engine) = sim.state.ki.engines[p].as_ref() else {
        return Ok(0);
    };
    let list = &engine.mem.global_list;
    match usize::try_from(index) {
        Ok(i) => Ok(list.get(i).copied().unwrap_or(0)),
        Err(_) => Err(format!(
            "AI global of player {slot}: index {index} is negative"
        )),
    }
}

/// Query 2100 (`FUN_00430510` -> `FUN_0047B700`): number of objects that the observer (first ID of the
/// name, also dying) "sees": filter `guid` (type GUID or filter ID) with the observer's owner as the
/// reference, within the radius `attribute 18 << 10` (current value) around its center - center distance
/// `<=`. It counts itself if it passes the filter (a dying one does not); camouflaged ones count. Missing
/// observer or unknown filter ID (also 0) -> -1.
pub fn sees_count(sim: &mut Sim, guid: i32, name: &str) -> i32 {
    let Some(id) = names::first_id(&sim.state.names, name) else {
        return -1;
    };
    let Some(o) = sim.object(id) else {
        return -1;
    };
    let (pos, owner) = ((o.grid.x.0, o.grid.z.0), i32::from(o.owner()));
    if !filter_known(sim, guid) {
        return -1;
    }
    let radius = sim.attribute(id, SIGHT).wrapping_shl(10);
    let f = filter::from_id(sim.rules, guid);
    let list = collision::query_radius(sim, pos, &f, radius, owner, false);
    i32::try_from(list.len()).unwrap_or(i32::MAX)
}

/// Query 2200 (`FUN_0047B320`): is the tile of the object (first ID of the name, also dying) currently seen
/// for the slot `slot` (fog state 3 or 7, shared vision included)? Missing object -> false. Slot >= 8 ->
/// false as in the original (no shared vision); slot < 0 is the local player there (not deterministic in
/// network play, E-191) and false here - the load check rejects condition 33 with player < 0.
pub fn tile_seen(sim: &Sim, slot: i32, name: &str) -> bool {
    let Some(p) = player_index(slot) else {
        return false;
    };
    let Some(o) = names::first_id(&sim.state.names, name).and_then(|id| sim.object(id)) else {
        return false;
    };
    (FogView::new(&*sim.state).state_at(o.tile.0, o.tile.1, p as i32) >> 1) & 1 == 1
}

/// Query 2600 (`FUN_0043AA70(name, g, p2, angle)`): does the observer (first ID of the name, also
/// dying) see an object in the cone `angle` (degrees) in front of it?
///
/// Sight `r = attribute 18 << 10` (`r < 1` -> false). Filter `filter::from_id(guid)` **without** abort
/// on an unknown ID, then `mode |= 0x100` (list bit "owner == p2"): an unknown filter ID hits every object
/// of `p2`, a type GUID only type and owner `p2`, a filter ID without `MM_ANDALL` is extended by "or `p2`
/// itself" (E-192). The reference player is `p2`. For each candidate of the preselection (center distance
/// `<= r`, in grid order): not itself, edge distance `<= r`, bearing `Fixed_Atan2Deg(dz, dx)`,
/// `delta = wrap(facing - bearing)` and `-h < delta < h` with `h = (angle << 12) / 2` (strict), plus line
/// of sight - the first hit returns true.
///
/// The facing is `walk::direction`; the addition `+0x168` for a siege engine (status `0x5000000`) is not
/// known to the reimplementation (deviation from the original): it is 0.
pub fn sees_in_cone(sim: &mut Sim, name: &str, guid: i32, p2: i32, angle: i32) -> bool {
    let Some(observer) = names::first_id(&sim.state.names, name) else {
        return false;
    };
    let Some(o) = sim.object(observer) else {
        return false;
    };
    let pos = (o.grid.x.0, o.grid.z.0);
    let radius = sim.attribute(observer, SIGHT).wrapping_shl(10);
    if radius < 1 {
        return false;
    }
    let mut f = filter::from_id(sim.rules, guid);
    f.mode |= 0x100;
    let half = angle.wrapping_shl(12) / 2;
    let facing = walk::direction(sim, observer);
    for cand in collision::query_radius(sim, pos, &f, radius, p2, false) {
        if cand == observer {
            continue;
        }
        if collision::edge_distance_to_object(sim, observer, cand) > radius {
            continue;
        }
        let Some(c) = sim.object(cand) else { continue };
        let dx = c.grid.x.0.wrapping_sub(pos.0);
        let dz = c.grid.z.0.wrapping_sub(pos.1);
        let delta = world::wrap_angle(facing.wrapping_sub(atan2_deg(dz, dx)));
        if half.wrapping_neg() < delta
            && delta < half
            && tile_map::has_line_of_sight(sim, observer, cand)
        {
            return true;
        }
    }
    false
}
