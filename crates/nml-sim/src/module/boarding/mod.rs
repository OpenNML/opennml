//! Transport in mobile containers (ships; re/ships): the meeting point of
//! unit and container ([`compute_boarding_points`], re/ships 3.2) and the messages of the quarters of
//! mobile containers (re/ships 2.1, 3.3, 3.4): set meeting point ([`on_set_meeting_point`]), arrival
//! with pickup of the waiting units ([`on_container_arrived`]) and read meeting point ([`get_meeting_point`]).
//! The loading command `0x13` is in `command/economy.rs`, parking and pickup on the
//! arrival of the unit in `quarter::on_enter`, the collision path in `quarter::touch_partner`.

use crate::id::ObjectId;
use crate::message::Message;
use crate::messaging::{send, send_mut};
use crate::path::BLOCKED;
use crate::path::reach::find_boarding_cells;
use crate::sim::Sim;

use super::quarter::{self, quarter, quarter_mut};

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;

/// Module index building (containers with module 3 give their exit as position).
const BUILDING: usize = 3;
/// Walk `0x1400`: layer of the unit (`m_PPHandleNxN`), without walk -1.
const WALK_LAYER: u32 = 0x1400;
/// Surcharge of the inner search to the radius (`rad + 2`, re/ships 3.2).
const INNER_RADIUS_EXTRA: i32 = 2;
/// Center of a cell (`[0x00671380]` = 1/2 tile).
const HALF_CELL: i32 = 0x800;

/// Region ID of the cell in the layer (`PathPlanner_GetRegionId` `0x004F6970`); layers outside 0...8
/// and cells outside the map return `0xFFFF`.
fn region(sim: &Sim, layer: i32, cell: (i32, i32)) -> u16 {
    usize::try_from(layer).map_or(BLOCKED, |l| sim.state.path.region_id(l, cell.0, cell.1))
}

/// Center of the cell in fixed point (`(k << 12) + 2048`).
fn cell_centre(cell: (i32, i32)) -> (i32, i32) {
    ((cell.0 << 12) + HALF_CELL, (cell.1 << 12) + HALF_CELL)
}

/// `GameWorld_ComputeBoardingPoints(u, c, &a, &b)` (`0x00478180`, re/ships 3.2): land point
/// `a` and water point `b` (fixed-point cell centers) for the unit `unit` and the mobile container
/// `container`.
///
/// Branch 1: if the container stands in the land region of the unit (region of the container tile in the layer of
/// the unit), both meet on the container tile. Branch 2: if the unit stands in the
/// water region of the container, they meet on the unit tile. Otherwise the ring search
/// with a second layer ([`find_boarding_cells`], surcharge `rad + 2` with `rad = (hard_c + hard_u) >> 12`)
/// runs; without a match `None` (the caller aborts the command). For ships the third
/// branch always applies. For a container with a building module the position is its exit (`0x2503`); this never
/// applies to ships (the raw values of the original are fixed point, read here as a tile).
pub fn compute_boarding_points(
    sim: &mut Sim,
    unit: ObjectId,
    container: ObjectId,
) -> Option<((i32, i32), (i32, i32))> {
    let (ut, ct) = (
        send(sim, unit, &Message::new(WALK_LAYER, &[])),
        send(sim, container, &Message::new(WALK_LAYER, &[])),
    );
    let (u, c) = (sim.object(unit)?, sim.object(container)?);
    let (ucell, mut ccell) = (u.tile, c.tile);
    let hard = c.hard_radius.wrapping_add(u.hard_radius);
    if c.has_module(BUILDING, false) {
        let mut exit = Message::new(0x2503, &[0, 0, 0]);
        send_mut(sim, container, &mut exit);
        ccell = (exit.arg(0) >> 12, exit.arg(2) >> 12);
    }
    let r1 = region(sim, ut, ucell);
    let r2 = region(sim, ct, ccell);
    if region(sim, ut, ccell) == r1 {
        let p = cell_centre(ccell);
        return Some((p, p));
    }
    if region(sim, ct, ucell) == r2 {
        let p = cell_centre(ucell);
        return Some((p, p));
    }
    let (a, b) = find_boarding_cells(sim, ut, ucell, ccell, ct, (hard >> 12) + INNER_RADIUS_EXTRA)?;
    Some((cell_centre(a), cell_centre(b)))
}

/// Walk `0x2C00`: unit is parked at the meeting point (state 6).
const WALK_PARKED: u32 = 0x2C00;

/// `PropertyQuarter_OnMsgSetMeetingPoint` (`0x005207C0`, `0x1E17 {x, z, ids...}`, re/ships 2.1):
/// meeting point = `(x, z)`, all IDs from argument 2 on appended at the **end** of the waiting list (without
/// duplicate check), status to 1, return value 0. The command posts the message with abort: `Abort`
/// has cleared waiting list and status beforehand.
pub fn on_set_meeting_point(sim: &mut Sim, id: ObjectId, msg: &Message) -> i32 {
    if let Some(q) = quarter_mut(sim, id) {
        q.meeting_point = Some((msg.arg(0), msg.arg(1)));
        q.waiting
            .extend(msg.args.iter().skip(2).map(|&u| ObjectId::from_i32(u)));
        q.status = 1;
    }
    0
}

/// `PropertyQuarter_OnMsgContainerArrived` (`0x00520090`, `0x1F17`, re/ships 2.1, 3.3): status 2; each
/// ID of the waiting list from the front: if the unit is alive and its walk is parked (`0x2C00 != 0`),
/// `0x1617 {u}` goes to the container itself - at status 2 thus immediately `AcceptUnit` (without
/// distance check, W-200). Units still moving are skipped. Afterwards the list is empty. Return value 0.
pub fn on_container_arrived(sim: &mut Sim, id: ObjectId) -> i32 {
    let Some(q) = quarter_mut(sim, id) else {
        return 0;
    };
    q.status = 2;
    let list = q.waiting.clone();
    for u in list {
        if sim.state.object(u, true).is_some() && send(sim, u, &Message::new(WALK_PARKED, &[])) != 0
        {
            send(sim, id, &Message::new(quarter::msg::ENTER, &[u.as_i32()]));
        }
    }
    if let Some(q) = quarter_mut(sim, id) {
        q.waiting.clear();
    }
    0
}

/// `PropertyQuarter_OnMsgGetMeetingPoint` (`0x0051FC60`, `0x2017 {->x, ->z}`, re/ships 2.1): at status 1
/// the meeting point in the arguments and 1, otherwise 0 (arguments untouched).
pub fn get_meeting_point(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    let Some((x, z)) = quarter(sim, id)
        .filter(|q| q.status == 1)
        .and_then(|q| q.meeting_point)
    else {
        return 0;
    };
    msg.set_arg(0, x);
    msg.set_arg(1, z);
    1
}
