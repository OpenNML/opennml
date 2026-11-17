//! Group manager and groups (re/groups-formations 1, 3). Groups are pure state
//! in [`SimState::groups`](crate::state::SimState); access only via the ID, look up again after every
//! synchronous send (the group may be deleted).

pub mod formation;
pub mod geometry;
pub mod march;
pub mod mfc;
pub mod none;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use formation::FormationState;
pub use mfc::MfcMap;

use crate::message::Envelope;
use crate::messaging::post_command;
use crate::module::walk::{find_to_msg, formation::set_group, walk};
use crate::motion::NO_DIRECTION;
use crate::{id::ObjectId, message::Message, messaging::send, object::status, sim::Sim};

/// No group (walk `+0x208`).
pub const NO_GROUP: i32 = -1;

/// Group manager (`0x0067365C`). In the original process-wide and never saved (K-216); here per
/// match in the savegame (deviation from the original).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupManager {
    /// Next ID (`+0x00`), start 0.
    pub next_id: i32,
    /// ID -> group (`+0x04`).
    pub map: MfcMap<Group>,
}

/// Base class `Group` (`0x0056E520`, re/groups-formations 1.3) including the type-0 fields; formations
/// additionally carry [`FormationState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// `+0x60`.
    pub id: i32,
    /// `MoveCommand.formation` 0...6 (`GroupManager_CreateGroup` `0x0056F400`).
    pub kind: u8,
    /// `+0x20` members in command order; head = leader.
    pub members: Vec<ObjectId>,
    /// `+0x04` member -> target place (x, z, direction).
    pub targets: MfcMap<(i32, i32, i32)>,
    /// `+0x3C` type 0: world offset to the centroid; formation: place (lx, lz) unscaled.
    pub offsets: MfcMap<(i32, i32)>,
    /// `+0x5C` 1 = realign/march.
    pub marching: bool,
    /// `+0x64` click point (x, z, direction); x = -1 -> centroid.
    pub anchor: (i32, i32, i32),
    /// `+0x74` run.
    pub run: bool,
    /// `+0x78` largest distance member-centroid.
    pub max_dist: i32,
    /// `+0x7C` centroid at capture.
    pub centroid: (i32, i32),
    /// `+0x88` smallest `DefaultSpeed`.
    pub matched_speed: i32,
    /// `+0x8C` members with this speed; -1 = matching off (type 0).
    pub matched_count: i32,
    /// `+0x90` pack leader -> followers.
    pub packs: MfcMap<Vec<ObjectId>>,
    /// `+0xAC` FINDTO flags of the command.
    pub walk_flags: i32,
    /// `+0xB0`/`+0xB4` copies of the feedback messages, consumed once.
    pub success: Option<Message>,
    pub fault: Option<Message>,
    /// `+0xB8` compression (type 0: on, type 5: off).
    pub compress: bool,
    /// Formations 1-4 and 6 (`GroupFormation` `0x00429450`).
    pub formation: Option<Box<FormationState>>,
}

impl Group {
    /// Constructors by type (`0x0056F400`): type 0 compression on, matching off; type 5
    /// compression off, matching on; formations with [`FormationState`]. The caller does not
    /// create type >= 7.
    pub fn new(id: i32, kind: u8, anchor: (i32, i32, i32), run: bool) -> Self {
        let formation = matches!(kind, 1..=4 | 6).then(|| Box::new(FormationState::new(kind)));
        Self {
            id,
            kind,
            members: Vec::new(),
            targets: MfcMap::new(),
            offsets: MfcMap::new(),
            marching: false,
            anchor,
            run,
            max_dist: 0,
            centroid: (0, 0),
            matched_speed: i32::MAX,
            matched_count: if kind == 0 { -1 } else { 0 },
            packs: MfcMap::new(),
            walk_flags: 0,
            success: None,
            fault: None,
            compress: kind == 0,
            formation,
        }
    }

    /// vf04: type ID; type 5 shares the VTable of type 0 and reports 0 (re/groups-formations 1.5).
    pub fn type_id(&self) -> i32 {
        match self.kind {
            5 => 0,
            k => i32::from(k),
        }
    }
}

/// Key of a group ID in [`MfcMap`].
pub fn key(gid: i32) -> u32 {
    gid as u32
}

/// `GroupManager_Find` (`0x0056F340`).
pub fn find<'s>(sim: &'s Sim, gid: i32) -> Option<&'s Group> {
    if gid < 0 {
        return None;
    }
    sim.state.groups.map.get(key(gid))
}

pub fn find_mut<'s>(sim: &'s mut Sim, gid: i32) -> Option<&'s mut Group> {
    if gid < 0 {
        return None;
    }
    sim.state.groups.map.get_mut(key(gid))
}

/// `DefaultSpeed` (walk `+0xA4`), 0 without walk (`0x0056E170`).
pub fn default_speed(sim: &Sim, id: ObjectId) -> i32 {
    walk(sim, id).map_or(0, |w| w.speed.default)
}

/// Group ID of a unit (`0x1500`), -1 without walk.
pub fn group_of(sim: &mut Sim, id: ObjectId) -> i32 {
    if sim.object(id).is_none() {
        return NO_GROUP;
    }
    send(sim, id, &Message::new(0x1500, &[]))
}

/// Rider/boat occupant -> carrier (`target`), otherwise the unit itself; `None` without a carrier.
fn carrier_of(sim: &Sim, o: ObjectId) -> Option<ObjectId> {
    let ob = sim.object(o)?;
    if ob.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) == 0 {
        return Some(o);
    }
    sim.object(ob.target).map(|t| t.id)
}

/// `Group_AccountSpeed` (`0x0056E170`).
pub fn account_speed(sim: &mut Sim, gid: i32, id: ObjectId) {
    let v = default_speed(sim, id);
    let Some(g) = find_mut(sim, gid) else { return };
    if g.matched_count < 0 {
        return;
    }
    if v < g.matched_speed {
        g.matched_speed = v;
        g.matched_count = 1;
    } else if v == g.matched_speed {
        g.matched_count = g.matched_count.wrapping_add(1);
    }
}

/// `Group_BroadcastSpeed` (`0x0056E1C0`): `0x1700 {matched, 1}` to all existing members.
pub fn broadcast_speed(sim: &mut Sim, gid: i32) {
    let Some(g) = find(sim, gid) else { return };
    if g.matched_count < 0 {
        return;
    }
    let (members, v) = (g.members.clone(), g.matched_speed);
    for m in members {
        if sim.object(m).is_some() {
            send(sim, m, &Message::new(0x1700, &[v, 1]));
        }
    }
}

/// `Group_RecomputeSpeed` (`0x0056E320`): recompute, send on change.
fn recompute_speed(sim: &mut Sim, gid: i32) {
    let Some(g) = find_mut(sim, gid) else { return };
    let old = g.matched_speed;
    g.matched_speed = i32::MAX;
    let members = g.members.clone();
    for m in members {
        account_speed(sim, gid, m);
    }
    if find(sim, gid).is_some_and(|g| g.matched_speed != old) {
        broadcast_speed(sim, gid);
    }
}

/// vf24 base (`0x0056E3A0`): 0 without walk, 3 already a member, otherwise append and 1.
fn add_member_base(sim: &mut Sim, gid: i32, o: ObjectId) -> i32 {
    if !sim.object(o).is_some_and(|x| x.has_module(0, false)) {
        return 0;
    }
    let Some(g) = find_mut(sim, gid) else {
        return 0;
    };
    if g.members.contains(&o) {
        return 3;
    }
    g.members.push(o);
    account_speed(sim, gid, o);
    1
}

/// vf28 base (`0x0056E700`): remove offset and member; with active matching
/// (`matched_count > 0`) count, recompute if needed, and send the unit `0x1700 {−1, 0}`.
fn remove_member_base(sim: &mut Sim, gid: i32, o: ObjectId, id: ObjectId) -> bool {
    let v = default_speed(sim, o);
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    g.offsets.remove(id.0);
    let Some(i) = g.members.iter().position(|&m| m == id) else {
        return false;
    };
    g.members.remove(i);
    if g.matched_count > 0 {
        if v == g.matched_speed {
            g.matched_count -= 1;
            if g.matched_count == 0 {
                recompute_speed(sim, gid);
            }
        }
        send(sim, o, &Message::new(0x1700, &[-1, 0]));
    }
    true
}

/// Formation group (type 1-4, 6)? Then the formation versions of the virtual calls apply.
fn is_formation(sim: &Sim, gid: i32) -> bool {
    find(sim, gid).is_some_and(|g| g.formation.is_some())
}

/// vf24 by type.
fn add_member_vf(sim: &mut Sim, gid: i32, o: ObjectId) -> i32 {
    if is_formation(sim, gid) {
        formation::add_member(sim, gid, o)
    } else {
        add_member_base(sim, gid, o)
    }
}

/// vf28 by type.
fn remove_member_vf(sim: &mut Sim, gid: i32, o: ObjectId, id: ObjectId, still: bool) -> bool {
    if is_formation(sim, gid) {
        formation::remove_member(sim, gid, o, id, still)
    } else {
        remove_member_base(sim, gid, o, id)
    }
}

/// Rear group of a formation (type 0, has none itself; the load check rejects chains).
fn rear_of(g: &Group) -> i32 {
    g.formation.as_ref().map_or(NO_GROUP, |f| f.rear)
}

/// vf08: number of members including the rear group.
pub fn member_count(sim: &Sim, gid: i32) -> usize {
    let Some(g) = find(sim, gid) else { return 0 };
    let rear = rear_of(g);
    g.members.len()
        + if rear == gid {
            0
        } else {
            member_count(sim, rear)
        }
}

/// vf0C `Group_IsMember` (`0x0056E090`): member, also of the rear group. Riders and boat occupants
/// (`0x2000`/`0x8000`) are mapped to their target `+0x2CC`, without checking whether it lives (-1 finds
/// nothing, re/riding 13.2); so a second move command to the same riders recognizes the group of their
/// horses.
pub fn is_member(sim: &Sim, gid: i32, id: ObjectId) -> bool {
    let id = match sim.object(id) {
        Some(o) if o.status & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) != 0 => o.target,
        _ => id,
    };
    is_member_of(sim, gid, id)
}

/// Search in the group and its rear group (without mapping the riders).
fn is_member_of(sim: &Sim, gid: i32, id: ObjectId) -> bool {
    let Some(g) = find(sim, gid) else {
        return false;
    };
    let rear = rear_of(g);
    g.members.contains(&id) || (rear != gid && is_member_of(sim, rear, id))
}

/// `Group_AddMember` (`0x0056F360`): rider/occupant -> carrier; on 1 set the ID and quietly
/// take it out of the old group. Success on 1 and 2 (2 = rank 0 into the rear group).
pub fn add_member(sim: &mut Sim, gid: i32, o: ObjectId) -> bool {
    let Some(o) = carrier_of(sim, o) else {
        return false;
    };
    let r = add_member_vf(sim, gid, o);
    if r == 1 {
        let old = group_of(sim, o);
        // `set_group` instead of `0x1600 {gid}` is equivalent (synchronous query to the own
        // walk module) - except for dead units (`DIED`): `walk::query` answers there only
        // `0xC00`, the original ignores `0x1600`; the reimplementation sets the ID anyway
        // (A7: prevents references to deleted groups). If the walk module is missing, `set_group`
        // does nothing (like the query without a receiver).
        set_group(sim, o, gid);
        remove_unit(sim, old, o, o, true);
    }
    matches!(r, 1 | 2)
}

/// `GroupManager_RemoveUnitFromGroup` (`0x0056F280`): without a group 0; rider/occupant with `id ==
/// o` -> carrier; success and not quiet -> `0x1600 {−1}` (as [`set_group`], deviation for
/// dead units as with [`add_member`]: the reimplementation sets -1, A7); delete an empty group.
pub fn remove_unit(sim: &mut Sim, gid: i32, o: ObjectId, id: ObjectId, still: bool) -> bool {
    if find(sim, gid).is_none() {
        return false;
    }
    let (o, id) = if id == o {
        match carrier_of(sim, o) {
            Some(c) => (c, c),
            None => return false,
        }
    } else {
        (o, id)
    };
    let ok = remove_member_vf(sim, gid, o, id, still);
    if ok {
        if !still {
            set_group(sim, o, NO_GROUP);
        }
        if member_count(sim, gid) == 0 {
            sim.state.groups.map.remove(key(gid));
        }
    }
    ok
}

/// Parameters of a move command for the group (`MoveCommand`, re/commands 1.7); `units` in
/// command order (`MoveCommand_GetUnit(0…n−1)` = selection backwards).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupOrder {
    pub units: Vec<ObjectId>,
    pub x: i32,
    pub z: i32,
    pub formation: i32,
    pub direction: i32,
    pub spread: i32,
    pub run: bool,
}

/// Linked object (`CARRIING`) -> its target, otherwise itself (`0x0056DF90`).
pub(crate) fn linked_target(sim: &Sim, id: ObjectId) -> ObjectId {
    match sim.object(id) {
        Some(o) if o.status & status::CARRIING != 0 && sim.object(o.target).is_some() => o.target,
        _ => id,
    }
}

/// `GroupManager_CreateGroup` (`0x0056F400`, re/groups-formations 1.2).
pub fn create_group(sim: &mut Sim, order: &GroupOrder) -> Option<i32> {
    let mgr = &mut sim.state.groups;
    let id = mgr.next_id;
    mgr.next_id = id.wrapping_add(1);
    if id == -1 {
        return None;
    }
    let kind = match order.formation {
        0..=6 => order.formation as u8,
        _ => return None,
    };
    let anchor = (order.x, order.z, order.direction);
    sim.state
        .groups
        .map
        .set(key(id), Group::new(id, kind, anchor, order.run));
    for &u in &order.units {
        if sim.object(u).is_some_and(|o| o.status & status::DIED == 0) {
            add_member(sim, id, u);
        }
    }
    capture(sim, id);
    if member_count(sim, id) == 0 {
        sim.state.groups.map.remove(key(id));
        return None;
    }
    Some(id)
}

/// vf30 by type.
fn capture(sim: &mut Sim, gid: i32) {
    if is_formation(sim, gid) {
        formation::capture(sim, gid);
    } else {
        none::capture(sim, gid);
    }
}

/// vf34 by type.
fn compute_targets(sim: &mut Sim, gid: i32) -> bool {
    if is_formation(sim, gid) {
        formation::compute_targets(sim, gid)
    } else {
        none::compute_targets(sim, gid)
    }
}

/// vf10 `Group_HasSameMembers` (`0x0056DE90`): same members; same formation -> 1; otherwise
/// with direction -> 1 (K-213); without direction the command gets the old direction and it is formed
/// anew.
pub fn has_same_members(sim: &Sim, gid: i32, order: &mut GroupOrder) -> bool {
    let Some(g) = find(sim, gid) else {
        return false;
    };
    if member_count(sim, gid) != order.units.len()
        || !order.units.iter().all(|&u| is_member(sim, gid, u))
    {
        return false;
    }
    if order.formation == g.type_id() || order.direction != NO_DIRECTION {
        return true;
    }
    order.direction = g.anchor.2;
    false
}

/// `Group_HandleSingleMember` (`0x0056E130`): responsible for <= 1 member (`+0x2C`, without the rear group,
/// K-256); anchor x == -1 -> centroid of the group, direction "none".
fn handle_single_member(sim: &mut Sim, gid: i32) -> bool {
    let Some(g) = find_mut(sim, gid) else {
        return true;
    };
    match g.members.as_slice() {
        [] => true,
        [m] => {
            let m = *m;
            if g.anchor.0 == -1 {
                g.anchor = (g.centroid.0, g.centroid.1, NO_DIRECTION);
            }
            let anchor = g.anchor;
            send_move_to_member(sim, gid, m, anchor);
            true
        }
        _ => false,
    }
}

/// `Group_MoveInFormation` (`0x0056EF10`).
pub fn move_in_formation(
    sim: &mut Sim,
    gid: i32,
    walk_flags: i32,
    ok: Option<Message>,
    fail: Option<Message>,
) -> bool {
    let Some(g) = find_mut(sim, gid) else {
        return false;
    };
    g.marching = false;
    g.walk_flags = walk_flags;
    if handle_single_member(sim, gid) {
        return true;
    }
    if !compute_targets(sim, gid) {
        return false;
    }
    broadcast_speed(sim, gid);
    if let Some(g) = find_mut(sim, gid) {
        g.success = ok;
        g.fault = fail;
    }
    none::build_packs(sim, gid);
    let leaders = find(sim, gid).map_or(Vec::new(), |g| g.packs.keys());
    for l in leaders {
        let leader = ObjectId(l);
        let Some(&pos) = find(sim, gid).and_then(|g| g.targets.get(l)) else {
            continue;
        };
        send_move_to_member(sim, gid, leader, pos);
    }
    true
}

/// `Group_SendMoveToMember` (`0x0056DF90`).
pub fn send_move_to_member(sim: &mut Sim, gid: i32, id: ObjectId, pos: (i32, i32, i32)) {
    let Some(g) = find(sim, gid) else { return };
    let group_flag = if member_count(sim, gid) != 1 { 8 } else { 0 };
    let msg = find_to_msg(pos.0, pos.1, -1, pos.2, 0, g.run, g.walk_flags | group_flag);
    let env = Envelope {
        msg,
        success: g.success.clone(),
        fault: g.fault.clone(),
        success_target: ObjectId::NONE,
        fault_target: ObjectId::NONE,
    };
    let to = linked_target(sim, id);
    post_command(sim, to, env, true);
}

/// `Group_Realign` (`0x0056E2D0`).
pub fn realign(sim: &mut Sim, gid: i32, pos: (i32, i32, i32), spread: i32, run: bool) {
    let Some(g) = find_mut(sim, gid) else { return };
    g.marching = true;
    g.anchor = pos;
    g.run = run;
    if handle_single_member(sim, gid) {
        return;
    }
    broadcast_speed(sim, gid);
    realign_vf(sim, gid, spread);
}

/// vf2C by type: type 0/5 moves with the flags of the group, without feedback messages (`0x004E4BE0`).
fn realign_vf(sim: &mut Sim, gid: i32, spread: i32) {
    if is_formation(sim, gid) {
        formation::realign(sim, gid, spread);
        return;
    }
    let flags = find(sim, gid).map_or(0, |g| g.walk_flags);
    move_in_formation(sim, gid, flags, None, None);
}

/// vf14 (FINDTO of the leader with flag 8): base does nothing; formations `0x0042A270` (re/groups-formations 12.9).
pub fn on_leader_find_to(sim: &mut Sim, gid: i32, id: ObjectId) {
    if is_formation(sim, gid) {
        march::on_leader_find_to(sim, gid, id);
    }
}

/// vf18 (PATH of the leader with flag 8): base distributes the path (re/groups-formations 12.8); formations
/// `0x0042ABB0` (re/groups-formations 12.3).
pub fn on_leader_path(sim: &mut Sim, gid: i32, id: ObjectId) -> bool {
    if is_formation(sim, gid) {
        march::on_leader_path(sim, gid, id)
    } else {
        none::on_leader_path(sim, gid, id)
    }
}
