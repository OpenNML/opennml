//! Evaluate match filters like `GameObject_MatchesFilter` (`0x0046AA40`, `docs/formats/targets.md`,
//! re/match-filter, re/combat-targeting 3). All bits are evaluated, including
//! the combat bits (`MM2_IS_ATTACKING`, `MM2_IS_IN_WEAPON_RANGE`, `MM_METHODINDEX 3`, viewing angle).
//! The fog (`MM_CHECKFOG`) checks the player `f[6]`: the TARGET callers set it on the head filter
//! ([`matches_target`]), all other filters carry the parser value 0 (re/vision 6.2). The
//! built-in check `mode & 0x20` reads the run-time words of the filter (re/match-filter, section "Appendix")
//! and, for word 0x11 != 0, searches the nearest reachable cell ([`crate::path::reach`]); its
//! line of sight runs over the building map ([`crate::tile_map`]).

use opennml_formats::targets::MatchFilter;

use crate::collision::{
    edge_distance_to_object, edge_distance_to_point, fixed_pos_distance, quartering_container,
};
use crate::fog::FogView;
use crate::fx::fixed_hypot;
use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::send;
use crate::module::walk;
use crate::object::status;
use crate::path::reach::{ReachFilter, find_nearest_reachable_cell};
use crate::rules::Rules;
use crate::sim::Sim;
use crate::world::region_id_at;

/// `GameObject_MatchesFilter(obj, f, player)`; `player < 0` = owner of the object. `MM_CHECKFOG`
/// checks the player `f[6]` of every filter - for MY filters, variant conditions and built-in
/// filters the parser value 0, i.e. the fog of player 0 (quirk).
pub fn matches(sim: &mut Sim, id: ObjectId, filter: &MatchFilter, player: i32) -> bool {
    matches_with(sim, id, filter, player, None, &mut None)
}

/// Like [`matches`] for the TARGET callers (`TargetActions_FindForTargetObject` `0x00480600`,
/// `0x0047ACF0`, `Priorities_FindMatchIndex` `0x00500BF0`): they set `f[6]` only on the head filter
/// to `fog_player`; following filters keep the parser value 0 (quirk; re/match-filter, section "Reference player").
pub fn matches_target(
    sim: &mut Sim,
    id: ObjectId,
    filter: &MatchFilter,
    player: i32,
    fog_player: i32,
) -> bool {
    matches_with(sim, id, filter, player, Some(fog_player), &mut None)
}

/// Like [`matches_target`] (or [`matches`] with `fog_player = None`); the cell search of the
/// built-in check `mode & 0x20` writes the cell found (fixed-point center) to `cell`
/// (`out[1]`, `out[2]` of `GameObject_MatchesFilter`, `0x0042D260`). `cell` is valid only when the return value is
/// `true`: the cell is already written at the check step, a later step or following filter
/// can still make the filter fail afterwards. Callers therefore read it only on `true`.
pub fn matches_with_cell(
    sim: &mut Sim,
    id: ObjectId,
    filter: &MatchFilter,
    player: i32,
    fog_player: Option<i32>,
    cell: &mut Option<(i32, i32)>,
) -> bool {
    matches_with(sim, id, filter, player, fog_player, cell)
}

fn matches_with(
    sim: &mut Sim,
    id: ObjectId,
    filter: &MatchFilter,
    player: i32,
    fog_head: Option<i32>,
    cell: &mut Option<(i32, i32)>,
) -> bool {
    let mut f = filter;
    let mut player = player;
    let mut fog = fog_head.unwrap_or(filter.fog_player);
    loop {
        let b = evaluate(sim, id, f, &mut player, fog, cell);
        let m = f.mode;
        let next = f.next.as_deref();
        if m & 0x5000_0000 != 0 {
            if !b {
                return false;
            }
            let Some(next) = next else { return false };
            if m & 0x1000_0000 != 0 {
                f = next;
                fog = next.fog_player;
                continue;
            }
            return !matches_with(sim, id, next, player, None, cell);
        }
        if m & 0xA000_0000 != 0 {
            if b {
                return true;
            }
            let Some(next) = next else { return false };
            if m & 0x2000_0000 != 0 {
                f = next;
                fog = next.fog_player;
                continue;
            }
            return !matches_with(sim, id, next, player, None, cell);
        }
        return b;
    }
}

/// Filter ID of the AI -> filter (`FUN_0047E390` `0x0047E390`, `this = GameWorld`, re/ki-queries 5.2):
/// - `id ≥ 1000`: filter "type GUID `id`, list of the reference player, AND" (`mode 0x118`, `guid = id`);
///   if the type is a building (descriptor of module 3 with mask != 0, [`type_has_module_desc`]),
///   additionally `HANDLETYPE 0x403` (`mode |= 2`): only **finished** buildings (E-51);
/// - `id < 1000` (also 0 and negative): MY filter of the first entry of `flowcontroltragets.ini`
///   with `BUTTONMSG == id`; without an entry an empty filter (`mode = mode2 = 0`), which at the head gives "all
///   objects of the reference player", as a following filter and in `0x4E28` never matches (E-52).
///
/// Fields that the original leaves as stack contents have the default of
/// [`MatchFilter::constructed`] here (no effect, like K-273).
pub fn from_id(rules: &Rules, id: i32) -> MatchFilter {
    if id >= 1000 {
        let building = type_has_module_desc(rules, id as u32, 3);
        let mut f = MatchFilter {
            mode: 0x118,
            guid: id,
            ..MatchFilter::constructed()
        };
        if building {
            f.mode |= 0x2;
            f.match_bits = 0x403;
        }
        return f;
    }
    match rules.flowcontrol_target(id) {
        Some(entry) => entry.my.clone(),
        None => MatchFilter::constructed(),
    }
}

/// `ObjectType_FindModuleDesc(m, 0xFFFF_FFFF)` (`0x0054CF30`, re/ki-queries 5.15.3): does the type
/// `guid` have a descriptor of module `module` with a variant mask != 0 ("in any variant")?
/// A descriptor with mask 0 does not count; unknown type -> no.
pub fn type_has_module_desc(rules: &Rules, guid: u32, module: u32) -> bool {
    rules
        .types
        .get(&guid)
        .is_some_and(|t| t.modules.iter().any(|m| m.index == module && m.mask != 0))
}

/// Filter of `GameObject_MatchesClass(bits, 0, 0)` (`0x0046B9D0`): `{mode 2, match_bits}`, otherwise
/// like [`MatchFilter::constructed`]; to be evaluated with reference player -1 ([`matches_class`]).
pub fn class_filter(bits: u32) -> MatchFilter {
    MatchFilter {
        mode: 2,
        match_bits: bits,
        ..MatchFilter::constructed()
    }
}

/// `GameObject_MatchesClass(bits, 0, 0)` (`0x0046B9D0`): filter [`class_filter`] with the
/// object's own owner - **one** bit of the class suffices (`CHECKHANDLETYPE` in OR mode), no
/// owner and no list comparison; dead and quartered objects do not match. The `MM_PROPID`, uninitialized
/// in the original, is 0 here (switched-off modules count).
pub fn matches_class(sim: &mut Sim, id: ObjectId, bits: u32) -> bool {
    matches(sim, id, &class_filter(bits), -1)
}

/// Fog check of `MM_CHECKFOG` (`0x0046AE62`, re/vision 6.2) for the reference player
/// `fog_player`: not stealthed or owner: "seen" at the tile (shared vision counts); stealthed
/// only with `WAITING4KILLING` via the reconnaissance of an enemy of the owner, otherwise never (quirk).
/// `m_FogShowMode` has no effect here. The commands use it for their target check (A8),
/// so that it agrees with the right-click assignment.
pub(crate) fn fog_visible(sim: &Sim, id: ObjectId, fog_player: i32) -> bool {
    let Some(o) = sim.object(id) else {
        return false;
    };
    let (st, tile, owner) = (o.status, o.tile, i32::from(o.owner()));
    let view = FogView::new(&*sim.state);
    if st & status::MASKED == 0 || fog_player == owner {
        view.state_at(tile.0, tile.1, fog_player) & 2 != 0
    } else if st & status::WAITING4KILLING != 0 {
        view.cleared_by_enemy_of(tile.0, tile.1, owner)
    } else {
        false
    }
}

/// Sight rule of `player::Sight::Fog` (extension of the reimplementation; not a function of the
/// original, whose AI sees everything, E-25): own objects (owner slot == `player`) always, foreign ones
/// according to [`fog_visible`] for `player`. A missing object is not seen.
pub(crate) fn seen_by(sim: &Sim, id: ObjectId, player: u8) -> bool {
    sim.object(id).is_some_and(|o| o.owner() == player) || fog_visible(sim, id, i32::from(player))
}

/// A filter without chaining: result `b` before the chain evaluation.
fn evaluate(
    sim: &mut Sim,
    id: ObjectId,
    f: &MatchFilter,
    player: &mut i32,
    fog_player: i32,
    cell: &mut Option<(i32, i32)>,
) -> bool {
    let Some(o) = sim.object(id) else {
        return false;
    };
    let st = o.status;
    if f.mode & 0x400_0000 == 0 && st & status::DIED != 0 {
        return false;
    }
    if f.mode2 & 0x4 == 0 && st & status::QUARTERED != 0 {
        return false;
    }
    if *player < 0 {
        *player = i32::from(o.owner());
    }
    if f.mode & 0x800_0000 != 0 {
        return false;
    }
    if f.mode2 & 0x80 != 0 {
        return true;
    }
    let and = f.mode & 0x10 != 0;
    // `gate`: in AND mode a `false` aborts, otherwise a `true`.
    macro_rules! gate {
        ($b:expr) => {{
            let b = $b;
            if and != b {
                return b;
            }
        }};
    }
    let owner = i32::from(o.owner());
    let id_bits = id.0 & 0xF800;
    // 1. State (for riders/boats additionally MOVE of the carrier).
    if f.mode & 0x2001 != 0 {
        let mut s = st;
        if s & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) != 0
            && !o.target.is_none()
            && o.target.0 != 0
            && let Some(c) = sim.object(o.target)
        {
            s |= c.status & status::MOVE;
        }
        let b = f.status & s != 0;
        gate!(b != (f.mode & 0x2000 != 0));
    }
    let o = sim.object(id).expect("present");
    // 2. Type bits: low byte 0xFF = category, otherwise flags of the module; switched-off modules count
    // only with MM_PROPID 0 or 0x11 (quirk).
    if f.mode & 0x4042 != 0 {
        let m = (f.match_bits & 0xFF) as usize;
        let bits = f.match_bits & !0xFF;
        let have = if m == 0xFF {
            Some(o.category & !0xFF)
        } else {
            let include_disabled = f.prop_id == 0 || f.prop_id == 0x11;
            o.has_module(m, include_disabled)
                .then(|| o.module(m).map_or(0, |x| x.base.flags & !0xFF))
        };
        let b = match have {
            Some(h) if f.mode & 0x40 != 0 => h & bits == bits,
            Some(h) if f.mode & 0x80 != 0 => bits == h,
            Some(h) => bits & h != 0,
            None => false,
        };
        gate!(b != (f.mode & 0x4000 != 0));
    }
    // 3. Module present.
    if f.mode & 0x8004 != 0 {
        let p = f.prop_id as usize;
        let b = o.has_module(p, f.prop_id == 0 || f.prop_id == 0x11);
        gate!(b != (f.mode & 0x8000 != 0));
    }
    // 4. Type GUID.
    if f.mode & 0x10008 != 0 {
        let b = f.guid as u32 == o.guid;
        gate!(b != (f.mode & 0x10000 != 0));
    }
    let (load_amount, load_kind) = (o.load_amount, o.load_kind);
    let tile = o.tile;
    let hp_present = o.attributes.is_present(14) && !o.attributes.is_full(14);
    let energy_lost = o.attributes.is_present(15) && !o.attributes.is_full(15);
    let hp_fraction = o.attributes.current_fraction(14);
    // 5. Not too many opponents (Walk answers `0x3100`; if it is missing: -1).
    if f.mode2 & 0x40 != 0 {
        let b = send(sim, id, &Message::new(0x3100, &[f.ref_object])) != 1;
        gate!(b);
    }
    // 6. Load (CHECKNOTLOAD alone checks nothing).
    if f.mode & 0x70_0000 != 0 {
        let b = load_amount > 0
            && ((f.mode & 0x40_0000 != 0 && load_kind == 2)
                || (f.mode & 0x10_0000 != 0 && load_kind == 0)
                || (f.mode & 0x20_0000 != 0 && load_kind == 1));
        gate!(b != (f.mode & 0x2_0000 != 0));
    }
    // 7. Player lists, always evaluated.
    {
        let o = sim.object(id).expect("present");
        let d = &sim.state.diplomacy;
        let mut any = false;
        let mut b = false;
        if f.mode & 0x100 != 0 {
            any = true;
            b = owner == *player;
        }
        if f.mode & 0x200 != 0 {
            any = true;
            b |= id_bits == 0x4000;
        }
        if f.mode & 0x1000 != 0 {
            any = true;
            b |= id_bits == 0x4800;
        }
        if f.mode & 0x800 != 0 {
            any = true;
            b |= d.is_friend(*player, owner);
        }
        if f.mode & 0x400 != 0 {
            any = true;
            b |= d.is_enemy(*player, owner);
            // Quirk: without a building or Walk module the other lists drop out too.
            if !(o.has_module(3, false) || o.has_module(0, true)) {
                b = false;
            }
        }
        if f.mode & 0x4_0000 != 0 {
            b |= id_bits < 0x4000;
        } else if !any {
            b = and;
        }
        gate!(b);
    }
    // 8. Hit points not full.
    if f.mode & 0x80_0000 != 0 {
        gate!(hp_present);
    }
    // 9. Fog (`0x0046AE62`, re/vision 6.2): only with `f[6] < 8`, see [`fog_visible`].
    if f.mode & 0x200_0000 != 0 && fog_player < 8 {
        gate!(fog_visible(sim, id, fog_player));
    }
    // 10. Marked by the AI (`0x0046AEDF`, re/ki-queries 5.6): "not marked by player `f[6]`",
    // without the limit `< 8` of the fog; bit `f[6] & 31` of `GO+4`, the reimplementation knows
    // only the bits 0-7 (`ai_marks`). It is read fresh, the steps before can send.
    if f.mode & 0x8_0000 != 0 {
        let q = fog_player & 31;
        let marked = q < 8 && sim.object(id).is_some_and(|o| o.ai_marks >> q & 1 == 1);
        gate!(!marked);
    }
    // 11. Extra energy not full.
    if f.mode2 & 0x1 != 0 {
        gate!(energy_lost);
    }
    // 12. Special checks via messages (missing or idle module: -1).
    if f.mode2 & 0x2 != 0 {
        let b = match f.method {
            0 => send(sim, id, &Message::new(0x329, &[0])) == 1,
            1 => send(sim, id, &Message::new(0x329, &[1])) == 1,
            2 => send(sim, id, &Message::new(0x1C17, &[])) == 1,
            4 => send(sim, id, &Message::new(0x603, &[0, -2])) >= 0,
            5 => !quartered_in_masked(sim, id),
            // 3: tile distance to the reference object at most sight/3 (`0x0046AFEE`) (quirk).
            3 => {
                let r = ObjectId::from_i32(f.ref_object);
                match sim.object(r).map(|ro| ro.tile) {
                    None => false,
                    Some(rt) => {
                        let s = sim.attribute(r, 18) / 3;
                        // Wrapped like x86: the tiles come unchecked from the savegame.
                        let (dx, dz) = (rt.0.wrapping_sub(tile.0), rt.1.wrapping_sub(tile.1));
                        let d = fixed_hypot(dx << 12, dz << 12) >> 12;
                        d <= s
                    }
                }
            }
            _ => false,
        };
        gate!(b);
    }
    // 13. Attacks (`0x0046B105`): target of the attack module (`0x1511`). ME: it is the reference object
    // (then the filter applies at once, `SETZ BL; JNZ`); MY_FRIEND: friend of the player whose
    // edge distance to the reference is below its sight range (re/combat-targeting 3.4); otherwise
    // MY_FORMATION: target and reference in the same formation ([`same_formation`]).
    if f.mode2 & 0x400 != 0 {
        let t = send(sim, id, &Message::new(0x1511, &[0]));
        let (a, r) = (ObjectId::from_i32(t), ObjectId::from_i32(f.ref_object));
        let both =
            t >= 0 && sim.state.object(a, true).is_some() && sim.state.object(r, true).is_some();
        let b = both
            && (f.who & 1 != 0 && t == f.ref_object
                || f.who & 2 != 0
                    && sim.state.diplomacy.is_friend(*player, i32::from(a.slot()))
                    && edge_distance_to_object(sim, a, r) < sim.attribute(r, 18) << 10
                || f.who & 4 != 0 && same_formation(sim, a, r));
        gate!(b);
    }
    // 14. Built-in: distance range to the reference object or reachability via the region.
    if f.mode & 0x20 != 0 {
        let b = range_or_region(sim, id, f, cell);
        gate!(b);
    }
    // 15. Viewing angle to the reference object (`0x0046B4EC`; no effect in the shipped data).
    if f.mode2 & 0x8 != 0 {
        let r = ObjectId::from_i32(f.ref_object);
        let b = match sim
            .object(r)
            .map(|ro| ((ro.grid.x.0, ro.grid.z.0), walk::direction(sim, r)))
        {
            None => false,
            Some((rp, dir)) => {
                let o = sim.object(id).expect("present");
                // Wrapped like the tile distance above: positions from a savegame would otherwise overflow in the
                // debug build.
                let a = crate::math::atan2_deg(
                    o.grid.z.0.wrapping_sub(rp.1),
                    o.grid.x.0.wrapping_sub(rp.0),
                );
                let diff = crate::world::wrap_angle(a.wrapping_sub(dir));
                let h = (f.angle_tolerance << 12) / 2;
                a != 0 && diff < h && -h < diff // Quirk: target exactly in +x fails
            }
        };
        gate!(b);
    }
    // 16. Can be shot at.
    if f.mode2 & 0x20 != 0 && !and {
        return true;
    }
    // 17. Can fight at range.
    if f.mode2 & 0x100 != 0 {
        let b = send(sim, id, &Message::new(0x1311, &[])) == 1;
        gate!(b);
    }
    // 18. In weapon range of the reference object (`0x0046B5D8`).
    if f.mode2 & 0x200 != 0 {
        let r = ObjectId::from_i32(f.ref_object);
        let b = sim.object(r).is_some()
            && edge_distance_to_object(sim, id, r) < sim.attribute(r, 21) << 10;
        gate!(b);
    }
    // 19. At least 70 % hit points (without `gate`).
    if f.mode2 & 0x800 != 0 {
        return hp_fraction / 1000 >= 70;
    }
    and
}

/// Built-in check `mode & 0x20` of `GameObject_MatchesFilter` (`0x0046B24F`...,
/// re/match-filter, section "Appendix"). With reference object `r` (word 8, also dying):
/// - `r` quartered in `c`: edge distance `c`->object (building: `GetEdgeDistanceToObject(c, obj)`,
///   otherwise `GetEdgeDistanceToPoint(c, obj)`) in the range `range_min − R(r)` ... `range_max − R(r)`;
/// - otherwise, word 0x11 = 0 (`tether`): center distance `r`<->object in `range_min … range_max`, with
///   byte 0x13 additionally the line of sight over the building map (`GameWorld_HasLineOfSight`,
///   [`crate::tile_map`]);
/// - word 0x11 != 0: nearest reachable cell (`PathPlanner_FindNearestReachableCell`,
///   [`crate::path::reach`]); the cell found (fixed-point center) goes to `cell`.
///
/// Without a reference object: region (word 0xB) -1 or `0xFFFF` -> not satisfied; a building asks
/// `0x2C03 {layer, region}` (!= -1 = reachable), all others compare the region at their own
/// tile and the four neighbors (±1 tile in x, then z; a negative coordinate at the map edge
/// reads the own position, as in the original) in the layer of word 0xA.
fn range_or_region(
    sim: &mut Sim,
    id: ObjectId,
    f: &MatchFilter,
    cell: &mut Option<(i32, i32)>,
) -> bool {
    let Some(o) = sim.object(id) else {
        return false;
    };
    let (pos, building) = ((o.grid.x.0, o.grid.z.0), o.has_module(3, false));
    let r = ObjectId::from_i32(f.ref_object);
    if let Some(ro) = sim.object(r) {
        let (rpos, rr) = ((ro.grid.x.0, ro.grid.z.0), ro.hard_radius);
        if let Some(c) = quartering_container(sim, r) {
            let (hi, lo) = (f.range_max.wrapping_sub(rr), f.range_min.wrapping_sub(rr));
            let d = if building {
                edge_distance_to_object(sim, c, id)
            } else {
                edge_distance_to_point(sim, c, pos)
            };
            return d <= hi && d >= lo;
        }
        if f.tether == 0 {
            let d = fixed_pos_distance(rpos, pos);
            let in_range = f.range_min <= d && d <= f.range_max;
            // Byte 0x13 (`CHECKLINEOFSIGHT`): `GameWorld_HasLineOfSight(r, obj)` (`0x00491C10`).
            return in_range
                && (!f.line_of_sight || crate::tile_map::has_line_of_sight(sim, r, id));
        }
        let rf = ReachFilter {
            range_min: f.range_min,
            range_max: f.range_max,
            home: f.home,
            tether: f.tether,
            line_of_sight: f.line_of_sight,
        };
        let found = find_nearest_reachable_cell(
            sim,
            f.layer,
            (rpos.0 >> 12, rpos.1 >> 12),
            (pos.0 >> 12, pos.1 >> 12),
            (f.range_max >> 12) + 1,
            Some(&rf),
            Some(id),
        );
        if let Some(c) = found {
            *cell = Some(((c.0 << 12) + 0x800, (c.1 << 12) + 0x800));
        }
        return found.is_some();
    }
    let region = f.region;
    if region == -1 || region == 0xFFFF {
        return false;
    }
    if building {
        return send(sim, id, &Message::new(0x2C03, &[f.layer, region])) != -1;
    }
    let one = 1 << 12;
    let (x, z) = pos;
    [
        (x, z),
        (x.wrapping_sub(one), z),
        (x, z.wrapping_sub(one)),
        (x.wrapping_add(one), z),
        (x, z.wrapping_add(one)),
    ]
    .into_iter()
    .any(|(px, pz)| i32::from(region_id_at(sim, id, f.layer, px, pz)) == region)
}

/// `MY_FORMATION` (`0x0046B218`, re/combat-targeting, table `MM2_IS_ATTACKING`, re/groups-formations 4.2): both
/// report the same group (`0x1500` >= 0) and it is a formation (vf1C).
pub(crate) fn same_formation(sim: &mut Sim, a: ObjectId, r: ObjectId) -> bool {
    let ga = send(sim, a, &Message::new(0x1500, &[]));
    let gr = send(sim, r, &Message::new(0x1500, &[]));
    ga >= 0 && ga == gr && crate::group::find(sim, ga).is_some_and(|g| g.formation.is_some())
}

/// `MM_METHODINDEX 5`: the object is not quartered in a stealthed carrier.
fn quartered_in_masked(sim: &Sim, id: ObjectId) -> bool {
    let Some(o) = sim.object(id) else {
        return false;
    };
    if o.status & status::QUARTERED == 0 {
        return false;
    }
    sim.object(o.carrier)
        .is_some_and(|c| c.status & 0x10_0000 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;
    use crate::state::SimState;
    use crate::world::{ObjectDesc, create_object};
    use opennml_formats::targets::{parse_target_defines, parse_targets};

    const TYPES: &str = "GUID 1100\nNAME Bauer\nVALUEID 100 6 1 8192\nPROPID 0\nPROPID 39\nVALUEID 400001 6 1 256\nGUID 1200\nNAME Krieger\nVALUEID 100 6 1 0\nPROPID 0\nGUID 3602\nNAME Baum\nVALUEID 100 6 1 256\nPROPID 4\n";
    const DEFINES: &str =
        "MID_PROP_ARMING_ISARMED 295\nPROP_ARMING 39\nMID_PROP_GLOBAL_HURTBY_SWORD 8447\n";

    fn filter(lines: &str) -> MatchFilter {
        let d = parse_target_defines(DEFINES);
        let text = format!("BEGIN\nMY_MM\n{lines}\nMM_END\nEND\n");
        parse_targets(&text, &d).entries.remove(0).my
    }

    #[test]
    fn filters_check_type_bits_modules_lists_and_chains() {
        let rules = Rules::synthetic(TYPES, 16);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let at = |guid, p| ObjectDesc::new(guid, p, [100.0, 0.0, 100.0]);
        let armed = create_object(&mut sim, &mut at(1100, 0)).unwrap();
        let warrior = create_object(&mut sim, &mut at(1200, 1)).unwrap();
        let tree = create_object(&mut sim, &mut at(3602, 0)).unwrap();
        // 485 "not armed" / 486 "armed".
        let not_armed =
            filter("MM_MODE MM_CHECKNOTHANDLETYPE\nMM_MATCHBITS MID_PROP_ARMING_ISARMED");
        let is_armed = filter("MM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_ARMING_ISARMED");
        assert!(matches(&mut sim, armed, &is_armed, -1));
        assert!(!matches(&mut sim, armed, &not_armed, -1));
        // Without an Arming module: "not armed" matches.
        assert!(matches(&mut sim, warrior, &not_armed, -1));
        // Category bits (low byte 0xFF): HURTBY_SWORD 0x2000.
        let sword = filter("MM_MODE MM_CHECKHANDLETYPE\nMM_MATCHBITS MID_PROP_GLOBAL_HURTBY_SWORD");
        assert!(matches(&mut sim, armed, &sword, -1) && !matches(&mut sim, warrior, &sword, -1));
        // Lists: own/enemies; enemies only with a Walk or building module.
        let mine = filter("MM_MODE MM_HL_MYLIST");
        assert!(matches(&mut sim, armed, &mine, 0) && !matches(&mut sim, warrior, &mine, 0));
        let enemies = filter("MM_MODE MM_HL_ENEMYLIST\nMM_MODE MM_HL_TREELIST");
        assert!(matches(&mut sim, warrior, &enemies, 0));
        assert!(!matches(&mut sim, tree, &enemies, 0));
        // AND: module present and own list; without list bits the list is neutral.
        let and = filter("MM_MODE MM_ANDALL\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_ARMING");
        assert!(matches(&mut sim, armed, &and, -1) && !matches(&mut sim, warrior, &and, -1));
        let empty_or = filter("");
        assert!(!matches(&mut sim, armed, &empty_or, -1));
        // Chaining: OR following filter.
        let chain = filter(
            "MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_ARMING\nMM_MODE MM_ORNEXTMATCH\nMM_NEXT\nMM_MODE MM_HL_MYLIST",
        );
        assert!(matches(&mut sim, warrior, &chain, 1));
        assert!(!matches(&mut sim, warrior, &chain, 0));
        // The dead drop out beforehand, except with MM_RETURN_DIED.
        sim.object_mut(warrior).unwrap().status |= status::DIED;
        assert!(!matches(&mut sim, warrior, &mine, 1));
        assert!(matches(
            &mut sim,
            warrior,
            &filter("MM_MODE MM_RETURN_DIED\nMM_MODE MM_HL_MYLIST"),
            1
        ));
    }

    #[test]
    fn the_builtin_check_compares_regions_around_the_tile_or_the_reference_distance() {
        let types = format!("{TYPES}GUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 3\n");
        let rules = Rules::synthetic(&types, 32);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        // Wall at x = 8 separates the map into two regions.
        for z in 0..32 {
            state.path.mark_cell(1, 8, z, true);
        }
        state.path.build_regions(u32::MAX);
        let mut sim = Sim::new(&mut state, &rules);
        let at = |x: f32, z: f32| ObjectDesc::new(1200, 0, [x * 128.0, 0.0, z * 128.0]);
        let unit = create_object(&mut sim, &mut at(5.5, 5.5)).unwrap();
        let west = i32::from(sim.state.path.region_id(0, 5, 5));
        let east = i32::from(sim.state.path.region_id(0, 12, 5));
        assert_ne!(west, east);
        let region = |region: i32| MatchFilter {
            mode: 0x20,
            layer: 0,
            region,
            ..MatchFilter::constructed()
        };
        assert!(matches(&mut sim, unit, &region(west), -1));
        assert!(!matches(&mut sim, unit, &region(east), -1));
        assert!(!matches(&mut sim, unit, &region(-1), -1));
        assert!(!matches(&mut sim, unit, &region(0xFFFF), -1));
        // On a blocked tile (region 0xFFFF) the four neighbors count: x = 7 borders on
        // the western region (6, z), x = 9 on the eastern one.
        let on_wall = create_object(&mut sim, &mut at(8.5, 3.5)).unwrap();
        crate::world::set_grid_position(
            &mut sim,
            on_wall,
            crate::fx::FxVec3::new(
                crate::fx::Fx(0x8800),
                crate::fx::Fx(0),
                crate::fx::Fx(0x3800),
            ),
            false,
        );
        assert!(matches(&mut sim, on_wall, &region(west), -1));
        assert!(matches(&mut sim, on_wall, &region(east), -1));
        // Layer -1: the layer of the Walk module (0x1400); without Walk 0xFFFF.
        let walk_layer = MatchFilter {
            layer: -1,
            ..region(west)
        };
        assert!(matches(&mut sim, unit, &walk_layer, -1));
        // Buildings ask 0x2C03: the docking places around tile (4, 4) lie in the west.
        let hq =
            create_object(&mut sim, &mut ObjectDesc::new(1000, 0, [600.0, 0.0, 600.0])).unwrap();
        assert!(matches(&mut sim, hq, &region(west), -1));
        assert!(!matches(&mut sim, hq, &region(east), -1));
        // Reference object: center distance in the range (word 0x11 = 0), otherwise cell search.
        let other = create_object(&mut sim, &mut at(5.5, 7.5)).unwrap();
        let dist = |min: i32, max: i32, reach: i32| MatchFilter {
            mode: 0x20,
            ref_object: other.as_i32(),
            range_min: min,
            range_max: max,
            tether: reach,
            ..MatchFilter::constructed()
        };
        assert!(matches(&mut sim, unit, &dist(8192, 8192, 0), -1));
        assert!(!matches(&mut sim, unit, &dist(8193, 9000, 0), -1));
        assert!(!matches(&mut sim, unit, &dist(0, 8191, 0), -1));
        assert!(
            matches(
                &mut sim,
                unit,
                &MatchFilter {
                    layer: 0,
                    ..dist(0, 9000, -1)
                },
                -1
            ),
            "tether -1: cell search finds a cell in the band"
        );
        // Layer -1 -> cell search fails, re/combat-targeting 3.2.
        assert!(!matches(&mut sim, unit, &dist(0, 9000, -1), -1));
        // Quartered reference object: edge distance from the container, range shifted by R(reference).
        {
            let o = sim.object_mut(other).unwrap();
            o.status |= status::QUARTERED;
            o.carrier = hq;
            o.hard_radius = 1000;
        }
        let d = crate::collision::edge_distance_to_point(&mut sim, hq, (0x5800, 0x5800));
        assert!(matches(&mut sim, unit, &dist(d + 1000, d + 1000, 0), -1));
        assert!(!matches(&mut sim, unit, &dist(d, d, 0), -1));
    }

    #[test]
    fn checkfog_uses_the_head_player_and_follow_filters_check_player_0() {
        use crate::command::testkit as kit;
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = kit::spawn(&mut sim, kit::COLONIST, 1, (10, 10));
        sim.state.fog.seen.or(10, 10, 1 << 1);
        let fog = filter("MM_MODE MM_CHECKFOG");
        assert!(matches_target(&mut sim, unit, &fog, 1, 1));
        assert!(!matches_target(&mut sim, unit, &fog, 1, 0));
        assert!(
            !matches(&mut sim, unit, &fog, -1),
            "MY filter: parser value 0"
        );
        // `f[6]` >= 8: no check, the gate is neutral (in OR mode alone without a hit).
        assert!(!matches_target(&mut sim, unit, &fog, 1, 8));
        let neutral = filter(
            "MM_MODE MM_ANDALL
MM_MODE MM_CHECKFOG",
        );
        assert!(
            matches_target(&mut sim, unit, &neutral, 1, 8),
            "f[6] >= 8: no check"
        );
        assert!(!matches_target(&mut sim, unit, &neutral, 1, 0));
        // Quirk: following filters check player 0 (parser value), even if the head checks player 1.
        let chain = filter(
            "MM_MODE2 MM2_RETURNTRUE
MM_MODE MM_ANDNEXTMATCH
MM_NEXT
MM_MODE MM_CHECKFOG",
        );
        assert!(!matches_target(&mut sim, unit, &chain, 1, 1));
        sim.state.fog.seen.or(10, 10, 1);
        assert!(matches_target(&mut sim, unit, &chain, 1, 1));
        // Explored alone is not enough; "fog off" does not exist in the simulation.
        sim.state.fog.seen.clear();
        sim.state.fog.explored.fill(0xFF);
        assert!(!matches_target(&mut sim, unit, &fog, 1, 1));
    }

    // QUIRK E-57: kept
    #[test]
    fn bit_0x80000_excludes_objects_marked_by_the_fog_player() {
        use crate::command::testkit as kit;
        let rules = kit::rules();
        let mut state = kit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let own = kit::spawn(&mut sim, kit::COLONIST, 1, (10, 10));
        let foreign = kit::spawn(&mut sim, kit::COLONIST, 0, (12, 10));
        sim.object_mut(own).unwrap().ai_marks = 1 << 2;
        sim.object_mut(foreign).unwrap().ai_marks = 1 << 2;
        // AND mode: the owner matches, `0x80000` is the exclusion condition.
        let and = MatchFilter {
            mode: 0x10 | 0x100 | 0x8_0000,
            ..MatchFilter::constructed()
        };
        for (fog, expected) in [(2, false), (3, true), (34, false), (40, true), (-30, false)] {
            assert_eq!(
                matches_target(&mut sim, own, &and, 1, fog),
                expected,
                "fog_player {fog}"
            );
            // The head value from the filter itself applies just the same (`f[6]`).
            let head = MatchFilter {
                fog_player: fog,
                ..and.clone()
            };
            assert_eq!(matches(&mut sim, own, &head, 1), expected, "f[6] {fog}");
        }
        // OR mode: a hit of the owner list ends the check before the bit.
        let or = MatchFilter {
            mode: 0x100 | 0x8_0000,
            ..MatchFilter::constructed()
        };
        assert!(
            matches_target(&mut sim, own, &or, 1, 2),
            "(own, marked) matches"
        );
        assert!(
            !matches_target(&mut sim, foreign, &or, 1, 2),
            "(foreign, marked) does not match"
        );
        sim.object_mut(foreign).unwrap().ai_marks = 0;
        assert!(
            matches_target(&mut sim, foreign, &or, 1, 2),
            "(foreign, unmarked) matches"
        );
        // Following filters read their own value `f[6]`, not that of the head.
        let chained = |head_fog: i32, next_fog: i32| MatchFilter {
            mode: 0x1000_0000 | 0x10 | 0x100,
            fog_player: head_fog,
            next: Some(Box::new(MatchFilter {
                fog_player: next_fog,
                ..and.clone()
            })),
            ..MatchFilter::constructed()
        };
        assert!(matches(&mut sim, own, &chained(2, 3), 1));
        assert!(!matches(&mut sim, own, &chained(3, 2), 1));
    }

    #[test]
    fn masked_objects_show_to_strangers_only_while_waiting_for_killing_and_cleared_up() {
        use crate::command::testkit as kit;
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let unit = kit::spawn(&mut sim, kit::COLONIST, 1, (10, 10));
        sim.state.fog.seen.or(10, 10, 0xFF);
        sim.object_mut(unit).unwrap().status |= status::MASKED;
        let fog = filter("MM_MODE MM_CHECKFOG");
        assert!(
            !matches_target(&mut sim, unit, &fog, 0, 0),
            "stealthed: never for foreigners"
        );
        assert!(matches_target(&mut sim, unit, &fog, 1, 1), "the owner sees");
        sim.object_mut(unit).unwrap().status |= status::WAITING4KILLING;
        assert!(!matches_target(&mut sim, unit, &fog, 0, 0));
        // Player 2 is an enemy of 1 (default: everyone against everyone) and has cleared up the tile.
        sim.state.fog.cleared.or(10, 10, 1 << 2);
        assert!(matches_target(&mut sim, unit, &fog, 0, 0));
    }

    /// Test world of the combat bits: warrior 1200 with Walk (without radii); sight 32, range 8, HP 160.
    fn combat_world() -> (Rules, SimState) {
        let mut rules = Rules::synthetic(TYPES, 32);
        let set = |v: &[(u32, i32)]| v.iter().copied().collect();
        rules
            .defaults
            .insert(1200, set(&[(14, 160), (18, 32), (21, 8)]));
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        state.path.build_regions(u32::MAX);
        (rules, state)
    }

    // QUIRK W-169, C-41, C-42: kept
    #[test]
    fn combat_bits_check_weapon_range_sight_third_and_angle() {
        let (rules, mut state) = combat_world();
        let mut sim = Sim::new(&mut state, &rules);
        let at = |p: i32, x: f32, z: f32| ObjectDesc::new(1200, p, [x * 128.0, 0.0, z * 128.0]);
        let r = create_object(&mut sim, &mut at(0, 5.5, 5.5)).unwrap();
        let a = create_object(&mut sim, &mut at(1, 6.5, 5.5)).unwrap();
        let far = create_object(&mut sim, &mut at(1, 25.5, 5.5)).unwrap();
        let with = |mode2, method| MatchFilter {
            mode: 0x10,
            mode2,
            method,
            ref_object: r.as_i32(),
            ..MatchFilter::constructed()
        };
        // Without module 17 `0x1511` answers -1 -> "attacks" never satisfied.
        assert!(!matches(
            &mut sim,
            a,
            &MatchFilter {
                who: 1,
                ..with(0x400, 0)
            },
            -1
        ));
        // Weapon range of the reference: edge distance < range(r)*1024 (8 -> 8192).
        assert!(
            matches(&mut sim, a, &with(0x200, 0), -1),
            "1 tile (without radii) < 8192"
        );
        assert!(!matches(&mut sim, far, &with(0x200, 0), -1));
        // MM_METHODINDEX 3: tile distance <= sight(r)/3 (32/3 = 10 tiles) (quirk).
        assert!(matches(&mut sim, a, &with(0x2, 3), -1));
        assert!(!matches(&mut sim, far, &with(0x2, 3), -1), "20 tiles > 10");
        // Angle: target exactly in +x direction -> atan2 = 0 -> always false (quirk).
        let angle = MatchFilter {
            angle_tolerance: 90,
            ..with(0x8, 0)
        };
        assert!(!matches(&mut sim, a, &angle, -1));
    }

    // QUIRK C-8: kept
    #[test]
    fn is_attacking_follows_the_target_of_the_attack_module() {
        use crate::combat::testkit as ck;
        let rules = ck::rules();
        let mut state = ck::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let r = ck::spawn(&mut sim, ck::WARRIOR, 1, (10, 10));
        let a = ck::spawn(&mut sim, ck::SABRE, 0, (12, 10));
        if let crate::module::ModuleKind::Attack(s) = &mut sim.module_mut(a, 0x11).unwrap().kind {
            s.target = r; // Status 0: counts anyway (C-8, re/combat-attack 9)
        }
        let f = |who| MatchFilter {
            mode: 0x10,
            mode2: 0x400,
            who,
            ref_object: r.as_i32(),
            ..MatchFilter::constructed()
        };
        assert!(matches(&mut sim, a, &f(1), -1), "ME");
        // MY_FRIEND: target belongs to a friend of the player, edge distance < sight(r)*1024.
        let r2 = ck::spawn(&mut sim, ck::WARRIOR, 1, (11, 10));
        let f2 = MatchFilter {
            who: 2,
            ref_object: r2.as_i32(),
            ..f(2)
        };
        assert!(matches(&mut sim, a, &f2, 1), "slot 1 is its own friend");
        assert!(!matches(&mut sim, a, &f2, 0));
    }

    #[test]
    fn my_formation_needs_the_same_group_and_a_formation() {
        // re/groups-formations 4.2, re/combat-targeting `MM2_IS_ATTACKING`: `0x1500` of both equal (>= 0) and vf1C.
        use crate::group::testkit::{INFANTRY, mv, rules, spawn, state};
        use crate::motion::NO_DIRECTION;
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let u: Vec<_> = (0..4)
            .map(|i| spawn(&mut sim, INFANTRY, (10 + i, 10)))
            .collect();
        crate::command::execute(&mut sim, &mv(&u[..2], (30, 30), 1, NO_DIRECTION), true);
        crate::command::execute(&mut sim, &mv(&u[2..], (30, 40), 0, NO_DIRECTION), true);
        assert_eq!(crate::group::find(&sim, 0).map(|g| g.kind), Some(1));
        assert_eq!(crate::group::find(&sim, 1).map(|g| g.kind), Some(0));
        assert!(same_formation(&mut sim, u[0], u[1]), "line");
        assert!(
            !same_formation(&mut sim, u[2], u[3]),
            "type 0 is not a formation"
        );
        assert!(!same_formation(&mut sim, u[0], u[2]), "different groups");
        let lone = spawn(&mut sim, INFANTRY, (5, 5));
        let other = spawn(&mut sim, INFANTRY, (6, 5));
        assert!(!same_formation(&mut sim, lone, other), "both -1");
    }

    #[test]
    fn not_too_many_opponents_asks_walk_with_the_reference_object() {
        use crate::combat::testkit as ck;
        use crate::module::test_hook;
        thread_local! {
            static SEEN: std::cell::RefCell<Vec<Message>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        fn record(_: &mut Sim, _: ObjectId, _: usize, msg: &Message) {
            if msg.code == 0x3100 {
                SEEN.with(|s| s.borrow_mut().push(msg.clone()));
            }
        }
        let rules = ck::rules();
        let mut state = ck::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let t = ck::spawn(&mut sim, ck::WARRIOR, 1, (10, 10));
        let me = ck::spawn(&mut sim, ck::SABRE, 0, (12, 10));
        let f = MatchFilter {
            mode: 0x10,
            mode2: 0x40,
            ref_object: me.as_i32(),
            ..MatchFilter::constructed()
        };
        // Free slots around the target: Walk answers 0 -> matches.
        assert!(test_hook::observe(record, || matches(&mut sim, t, &f, -1)));
        let seen = SEEN.with(|s| s.take());
        assert_eq!(seen, [Message::new(0x3100, &[me.as_i32()])]);
        // Reference missing: Walk reports "surrounded" (1) -> does not match.
        let gone = MatchFilter {
            ref_object: -1,
            ..f.clone()
        };
        assert!(!matches(&mut sim, t, &gone, -1));
        // Without Walk (observer) nobody answers (-1) -> matches as well.
        let o = ck::spawn(&mut sim, ck::OBSERVER, 1, (20, 20));
        assert!(matches(&mut sim, o, &f, -1));
    }

    #[test]
    fn the_reach_check_searches_a_cell_in_band_and_tether_and_reports_it() {
        let (rules, mut state) = combat_world();
        let mut sim = Sim::new(&mut state, &rules);
        let at = |p: i32, x: f32, z: f32| ObjectDesc::new(1200, p, [x * 128.0, 0.0, z * 128.0]);
        let me = create_object(&mut sim, &mut at(0, 2.5, 2.5)).unwrap();
        let target = create_object(&mut sim, &mut at(1, 10.5, 10.5)).unwrap();
        let reach = |tether, home| MatchFilter {
            mode: 0x30,
            ref_object: me.as_i32(),
            layer: 0,
            range_min: -1,
            range_max: 512 + 2048 + 512,
            tether,
            home,
            ..MatchFilter::constructed()
        };
        let mut cell = None;
        assert!(matches_with_cell(
            &mut sim,
            target,
            &reach(-1, (-1, -1)),
            0,
            None,
            &mut cell
        ));
        assert_eq!(cell, Some(((10 << 12) + 0x800, (10 << 12) + 0x800)));
        // Tether of 4 tiles around home (2.5, 2.5): the target is 8 tiles away -> no cell.
        let home = ((2 << 12) + 0x800, (2 << 12) + 0x800);
        let mut cell = None;
        assert!(!matches_with_cell(
            &mut sim,
            target,
            &reach(4 << 12, home),
            0,
            None,
            &mut cell
        ));
        assert_eq!(cell, None);
        // Tether 0: center distance instead of cell search (units without m_CanMove).
        assert!(
            !matches(&mut sim, target, &reach(0, home), 0),
            "8 tiles > 3072"
        );
    }

    #[test]
    fn the_centre_distance_branch_checks_the_line_of_sight() {
        use crate::command::testkit::{COLONIST, HOUSE, rules, spawn, state};
        let rules = rules();
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a = spawn(&mut sim, COLONIST, 0, (10, 10));
        let b = spawn(&mut sim, COLONIST, 1, (20, 10));
        let f = |los| MatchFilter {
            mode: 0x30,
            ref_object: a.as_i32(),
            range_min: 0,
            range_max: 12 << 12,
            tether: 0,
            line_of_sight: los,
            ..MatchFilter::constructed()
        };
        assert!(matches(&mut sim, b, &f(true), 0));
        spawn(&mut sim, HOUSE, 2, (15, 10));
        assert!(
            matches(&mut sim, b, &f(false), 0),
            "no check without byte 0x13"
        );
        assert!(
            !matches(&mut sim, b, &f(true), 0),
            "house (height 3200) above the ray (3072)"
        );
    }
}
