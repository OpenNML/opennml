//! Formation base `GroupFormation` (`0x00429450`, re/groups-formations 1.4, 12.9): ranks, rear group, layer,
//! radius, spread, place check (K-212) and realigning. The march (frames, vf14/vf18) lives in
//! [`super::march`].

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::geometry::{self, Ends, Layout};
use super::{
    Group, GroupOrder, MfcMap, NO_GROUP, add_member_base, create_group, find, find_mut, key,
    move_in_formation, none, realign as realign_group, remove_member_base, remove_unit,
    send_move_to_member,
};
use crate::collision::footprint_terrain_cost;
use crate::fx::fixed_mul;
use crate::id::ObjectId;
use crate::math::{cos_deg, sin_deg};
use crate::message::Message;
use crate::messaging::send;
use crate::module::walk::formation::{formation_num, set_group};
use crate::motion::NO_DIRECTION;
use crate::motion::step::COST_BLOCKED;
use crate::object::status;
use crate::path::{LAYER_MASKS, layer_from_move_mask};
use crate::sim::Sim;
use crate::world::wrap_angle;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormationState {
    /// `+0xB8 + 0x1C·k` members with rank k.
    pub ranks: [Vec<ObjectId>; 10],
    /// `+0x1D0` frames of the march (member -> world place).
    pub frames: VecDeque<MfcMap<(i32, i32)>>,
    /// `+0x1EC` depth (vf44).
    pub depth: i32,
    /// `+0x1F0` spread factor sp, default `Fixed_FromFloat(2.0)`.
    pub spacing: i32,
    /// `+0x1F4` shared NxN layer; -1 = recompute.
    pub layer: i32,
    /// `+0x1F8` rear group (type 0) for rank 0.
    pub rear: i32,
    /// `+0x1FC` rank-0 members.
    pub rear_members: Vec<ObjectId>,
    /// `+0x218` reference point of the march (x, z, direction).
    pub march_ref: (i32, i32, i32),
    /// `+0x228` corner margin.
    pub corner_margin: i32,
    /// `+0x22C` rank 0 not into the rear group (type 6).
    pub include_rank0: bool,
    /// `+0x230` leader position at the FINDTO.
    pub leader_start: (i32, i32),
    /// `+0x240` leader layer before switching.
    pub leader_layer: i32,
    /// `+0x244`/`+0x248` messages `0x2A00` / arrived at the frame place.
    pub reported: i32,
    pub arrived: i32,
    /// `+0x24C` members in lockstep; head = reference for catching up.
    pub ready: Vec<ObjectId>,
    /// `+0x268` largest hard radius (-1/0 = recompute).
    pub max_radius: i32,
    /// `+0x26C`/`+0x270` wing ends (type 1/3/4).
    pub end_max: ObjectId,
    pub end_min: ObjectId,
}

impl FormationState {
    pub fn new(kind: u8) -> Self {
        Self {
            ranks: Default::default(),
            frames: VecDeque::new(),
            depth: 0,
            spacing: 0x2000,
            layer: layer_from_move_mask(0x39) as i32,
            rear: NO_GROUP,
            rear_members: Vec::new(),
            march_ref: (0, 0, 0),
            corner_margin: 0,
            include_rank0: kind == 6,
            leader_start: (0, 0),
            leader_layer: 0,
            reported: 0,
            arrived: 0,
            ready: Vec::new(),
            max_radius: -1,
            end_max: ObjectId::NONE,
            end_min: ObjectId::NONE,
        }
    }
}

pub(super) fn form(g: &Group) -> &FormationState {
    g.formation.as_deref().expect("Formation")
}

pub(super) fn form_mut(g: &mut Group) -> &mut FormationState {
    g.formation.as_deref_mut().expect("Formation")
}

/// Rank as an index (A7: outside 0...9 like rank 0; the original would write behind the rank list).
/// The running game keeps `formation_num` in 0...9; other values come only from a corrupted
/// savegame.
pub fn rank_index(num: i32) -> usize {
    usize::try_from(num).ok().filter(|&r| r < 10).unwrap_or(0)
}

/// `0x00428BB0` (re/groups-formations 12.9): common class and largest footprint; `None` = class does not fit.
/// Layers outside the table (also -1) count as the start layer `0x39`.
pub fn merge_layer(g_layer: i32, o_layer: i32) -> Option<i32> {
    let mask = |l: i32| {
        usize::try_from(l)
            .ok()
            .and_then(|l| LAYER_MASKS.get(l).copied())
            .unwrap_or(LAYER_MASKS[layer_from_move_mask(0x39)])
    };
    let (g, o) = (mask(g_layer), mask(o_layer));
    let class = g & o & 0x38;
    if class == 0 {
        return None;
    }
    Some(layer_from_move_mask(class | (g & 7).max(o & 7)) as i32)
}

/// `0x00428D60`: recompute the layer from the rank members.
pub fn recompute_layer(sim: &mut Sim, gid: i32) {
    let members = find(sim, gid).map_or(Vec::new(), |g| g.members.clone());
    let mut layer = layer_from_move_mask(0x39) as i32;
    for m in members {
        let l = send(sim, m, &Message::new(0x1400, &[]));
        if let Some(n) = merge_layer(layer, l) {
            layer = n;
        }
    }
    if let Some(g) = find_mut(sim, gid) {
        form_mut(g).layer = layer;
    }
}

/// `GroupFormation_AddMember` vf24 (`0x00429E80`): 0 rejected, 1 accepted, 2 rear group,
/// 3 already there.
pub fn add_member(sim: &mut Sim, gid: i32, o: ObjectId) -> i32 {
    const REJECT: u32 = status::DIVING
        | status::DIED
        | status::SWIMMING
        | status::QUARTERED
        | status::ONWALL
        | status::ISMOUNTEDONBOAT
        | 0x0100_0000
        | status::ISMOUNTED;
    let Some(ob) = sim.object(o) else { return 0 };
    if !ob.has_module(0, false) || ob.status & REJECT != 0 {
        return 0;
    }
    if ob.status & status::CARRIING != 0
        && !sim
            .object(ob.target)
            .is_some_and(|t| t.has_module(0, false))
    {
        return 0;
    }
    let o_layer = send(sim, o, &Message::new(0x1400, &[]));
    let rank = rank_index(formation_num(sim, o));
    let Some(g) = find_mut(sim, gid) else {
        return 0;
    };
    let f = form_mut(g);
    let Some(layer) = merge_layer(f.layer, o_layer) else {
        return 0;
    };
    f.layer = layer;
    if rank == 0 && !f.include_rank0 {
        f.rear_members.push(o);
        return 2;
    }
    let r = add_member_base(sim, gid, o);
    if r == 1
        && let Some(g) = find_mut(sim, gid)
    {
        let f = form_mut(g);
        f.ranks[rank].push(o);
        f.max_radius = -1;
    }
    r
}

/// `GroupFormation_RemoveMember` vf28 (`0x00429640`), type 1/3/4 then wing ends (`0x00406F80`).
pub fn remove_member(sim: &mut Sim, gid: i32, o: ObjectId, id: ObjectId, still: bool) -> bool {
    if remove_member_base(sim, gid, o, id) {
        let Some(g) = find_mut(sim, gid) else {
            return true;
        };
        let kind = g.kind;
        let f = form_mut(g);
        f.max_radius = -1;
        if let Some(r) = f.ranks.iter_mut().find(|r| r.contains(&id)) {
            r.retain(|&m| m != id);
        }
        f.layer = -1;
        if matches!(kind, 1 | 3 | 4) && (f.end_max == id || f.end_min == id) {
            update_ends(sim, gid);
        }
        return true;
    }
    let rear = find(sim, gid).map_or(NO_GROUP, |g| form(g).rear);
    let ok = remove_unit(sim, rear, o, id, still);
    // The rear group disappears when its last member leaves (before the `find_mut`).
    let rear_alive = rear >= 0 && sim.state.groups.map.contains(key(rear));
    if let Some(g) = find_mut(sim, gid) {
        let f = form_mut(g);
        if ok {
            f.rear_members.retain(|&m| m != id);
        }
        if !rear_alive {
            f.rear = NO_GROUP;
        }
    }
    ok
}

/// vf54 by type: recompute the wing ends (column via the ranks, otherwise via the places).
pub fn update_ends(sim: &mut Sim, gid: i32) {
    let Some(g) = find_mut(sim, gid) else { return };
    let ends = if g.kind == 3 {
        Some(geometry::column_ends(&form(g).ranks))
    } else {
        geometry::find_ends(&g.offsets)
    };
    if let Some((max, min)) = ends {
        let f = form_mut(g);
        (f.end_max, f.end_min) = (max, min);
    }
}

/// `0x00429100`: largest hard radius of the rank members (cached).
pub fn max_radius(sim: &mut Sim, gid: i32) -> i32 {
    let Some(g) = find(sim, gid) else { return 0 };
    let mut r = form(g).max_radius;
    if r < 1 {
        for &m in &g.members {
            if let Some(o) = sim.object(m)
                && r < o.hard_radius
            {
                r = o.hard_radius;
            }
        }
        if let Some(g) = find_mut(sim, gid) {
            form_mut(g).max_radius = r;
        }
    }
    r
}

/// vf40 (`0x00429160`): place size, 2 * largest radius.
pub fn slot_size(sim: &mut Sim, gid: i32) -> i32 {
    max_radius(sim, gid).wrapping_mul(2)
}

/// vf50 (`0x00429170`) or `0x00429400`: point of the rear group, `depth` behind the anchor.
pub fn rear_point(sim: &Sim, gid: i32) -> (i32, i32, i32) {
    let Some(g) = find(sim, gid) else {
        return (-1, -1, NO_DIRECTION);
    };
    let a = g.anchor;
    if g.members.is_empty() || a.0 == -1 || a.2 == NO_DIRECTION {
        return a;
    }
    let back = wrap_angle(a.2.wrapping_add(180 << 12));
    let depth = form(g).depth;
    (
        a.0.wrapping_add(fixed_mul(cos_deg(back), depth)),
        a.1.wrapping_add(fixed_mul(sin_deg(back), depth)),
        a.2,
    )
}

/// `0x00428C20`: only `1 ≤ spread ≤ 2` takes effect, `sp = 1 + (spread − 1) · 2.5`.
pub fn apply_spread(sim: &mut Sim, gid: i32, spread: i32) {
    let p = spread.wrapping_sub(0x1000);
    if (0..=0x1000).contains(&p)
        && let Some(g) = find_mut(sim, gid)
    {
        form_mut(g).spacing = fixed_mul(p, 0x3800 - 0x1000) + 0x1000;
    }
}

/// vf30 `GroupFormation_Capture` (`0x00429FD0`) with rear group (`0x00429890`).
pub fn capture(sim: &mut Sim, gid: i32) {
    none::capture(sim, gid);
    let Some(g) = find(sim, gid) else { return };
    let rear_members = form(g).rear_members.clone();
    let run = g.run;
    if !rear_members.is_empty() {
        let p = rear_point(sim, gid);
        // Command order backwards (`MoveCommand_AddUnit`, K-94).
        let order = GroupOrder {
            units: rear_members.iter().rev().copied().collect(),
            x: p.0,
            z: p.1,
            formation: 0,
            direction: NO_DIRECTION,
            spread: crate::command::NO_SPREAD,
            run,
        };
        let rear = create_group(sim, &order).unwrap_or(NO_GROUP);
        if let Some(g) = find_mut(sim, gid) {
            form_mut(g).rear = rear;
        }
        // All carry the ID of the main group, including those of the rear group.
        for m in rear_members {
            if sim.object(m).is_some() {
                set_group(sim, m, gid);
            }
        }
    }
    let Some(g) = find_mut(sim, gid) else { return };
    if g.anchor.0 == -1 {
        let dir = if g.anchor.2 == NO_DIRECTION {
            0
        } else {
            g.anchor.2
        };
        g.anchor = (g.centroid.0, g.centroid.1, dir);
    }
}

/// vf34 `GroupFormation_ComputeTargets` (`0x00429700`) with place check (K-212): a place with cost of
/// at least `COST_BLOCKED` (blocked or outside the map) makes the command lapse. The rear group starts
/// first and is unaffected by this.
pub fn compute_targets(sim: &mut Sim, gid: i32) -> bool {
    let Some(g) = find(sim, gid) else {
        return false;
    };
    let (rear, flags) = (form(g).rear, g.walk_flags);
    if find(sim, rear).is_some() {
        move_in_formation(sim, rear, flags, None, None);
    }
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    let anchor = g.anchor;
    form_mut(g).march_ref = anchor;
    if g.members.is_empty() {
        return false;
    }
    if g.anchor.2 == NO_DIRECTION {
        g.anchor.2 = 0;
    }
    let mut ok = arrange(sim, gid);
    let targets: Vec<(u32, (i32, i32, i32))> = find(sim, gid).map_or(Vec::new(), |g| {
        g.targets.iter().map(|(k, &t)| (k, t)).collect()
    });
    for (k, (x, z, _)) in targets {
        let id = ObjectId(k);
        if sim.object(id).is_none() {
            continue;
        }
        let layer = send(sim, id, &Message::new(0xB00, &[]));
        if footprint_terrain_cost(sim, x, z, id, layer.max(0) as usize) >= COST_BLOCKED {
            ok = false;
        }
    }
    ok
}

/// vf48, applying: places in `offsets`/`targets`, depth, corner margin, wing ends.
fn arrange(sim: &mut Sim, gid: i32) -> bool {
    let s = slot_size(sim, gid);
    let Some(g) = find(sim, gid) else {
        return false;
    };
    let f = form(g);
    let p = geometry::arrange(&Layout {
        kind: g.kind,
        ranks: &f.ranks,
        n: g.members.len(),
        s,
        sp: f.spacing,
        anchor: g.anchor,
    });
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    for (m, local, world) in &p.places {
        g.offsets.set(m.0, *local);
        g.targets.set(m.0, *world);
    }
    let f = form_mut(g);
    if let Some(d) = p.depth {
        f.depth = d;
    }
    if let Some(c) = p.corner_margin {
        f.corner_margin = c;
    }
    match p.ends {
        Ends::Set { max, min } => (f.end_max, f.end_min) = (max, min),
        Ends::FindByOffsets | Ends::FindByRanks => update_ends(sim, gid),
        Ends::Keep => {}
    }
    p.ok
}

/// vf2C `GroupFormation_Realign` (`0x0042A190`); type 6 (`0x0056DB60`) first sets the direction
/// to "none". Places and place check do not run (quirk): only the leader gets a FINDTO.
pub fn realign(sim: &mut Sim, gid: i32, spread: i32) {
    let Some(g) = find_mut(sim, gid) else { return };
    if g.kind == 6 {
        g.anchor.2 = NO_DIRECTION;
    }
    g.targets.clear();
    let (rear, run) = (form(g).rear, g.run);
    if find(sim, rear).is_some() {
        let p = rear_point(sim, gid);
        realign_group(sim, rear, p, spread, run);
    }
    let Some(g) = find_mut(sim, gid) else { return };
    if g.members.is_empty() {
        return;
    }
    let members = g.members.clone();
    form_mut(g).ready = members.clone();
    if g.anchor.0 == -1 {
        return;
    }
    let anchor = g.anchor;
    apply_spread(sim, gid, spread);
    if find(sim, gid).is_some_and(|g| form(g).layer == -1) {
        recompute_layer(sim, gid);
    }
    send_move_to_member(sim, gid, members[0], anchor);
}
