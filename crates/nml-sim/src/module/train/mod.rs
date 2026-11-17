//! Module 33 `CPropertyTrain` (re/train-multipart): locomotive, tender and wagon. Here the state
//! (load check in [`super::ModuleKind::check`], section 5), init with snapping and coupling (3.1)
//! and the queries (2.1); drive curve in [`curve`], tick and drive step in [`drive`], track
//! blocking in [`blocking`], supply in [`supply`].

use nml_formats::property::ModuleDesc;
use nml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::collision::find_nearest_matching;
use crate::id::ObjectId;
use crate::math::{cos_deg, rotate_y_by_direction, sin_deg};
use crate::message::Message;
use crate::sim::Sim;
use crate::world::{
    ObjectDesc, create_object, desc_flags, set_direction, set_grid_position, world_to_grid,
};

use super::{ModuleKind, init_flags};

pub mod blocking;
pub mod curve;
pub mod drive;
pub mod supply;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;

/// Module index of the train (`PROPID TRAIN`).
pub const INDEX: usize = 0x21;
/// `SID_Lokomitive`: the only locomotive (master of the slaves).
pub const LOCO: u32 = 2613;
/// `SID_Tender` (slave).
pub const TENDER: u32 = 2618;
/// `SID_Wagon` (slave, with quarters).
pub const WAGON: u32 = 2617;
/// Track construction site or segment (building with flag `TRACK`).
pub const TRACK: u32 = 2518;
/// Station (builds `TRACK`).
pub const STATION: u32 = 2517;

/// Largest time of the drive curve in ms (re/train-multipart 3.3, 5).
pub const MAX_ACC_TIME: i32 = 2400;

/// Bits of `m_Status` (`+0xA8`, re/train-multipart 1.1).
pub mod status {
    /// Accelerating.
    pub const ACCEL: u32 = 1;
    /// Driving.
    pub const MOVING: u32 = 2;
    /// Braking.
    pub const BRAKING: u32 = 4;
    /// Forward.
    pub const FORWARD: u32 = 8;
    /// Backward.
    pub const BACKWARD: u32 = 0x10;
    /// All used bits.
    pub const ALL: u32 = ACCEL | MOVING | BRAKING | FORWARD | BACKWARD;
}

/// `CPropertyTrain` (`0x108` bytes, `PropertyTrain_Construct` `0x0052F810`); fields in the order
/// of the savegame (`PropertyTrain_SerializeExtra` `0x005317D0`, re/train-multipart 1.1, 5). The
/// particle list (`+0xDC`, brake sparks) is presentation and missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Train {
    /// `m_SlaveList` (`+0x70`): only the locomotive; tender and wagon in `AddTail` order.
    pub slaves: Vec<ObjectId>,
    /// `m_MasterHID` (`+0x8C`): slave -> locomotive.
    pub master: ObjectId,
    /// `m_MoveForwardAnim`, `m_MoveBackwardAnim`, `m_StopAnim`, `m_WaitAnim` (340002...340005).
    pub fwd_anim: i32,
    pub back_anim: i32,
    pub stop_anim: i32,
    pub wait_anim: i32,
    /// `m_GameSpeed` (`+0xA0`): distance per tick (fixed point).
    pub speed: i32,
    /// `m_NextGameSpeed` (`+0x104`): distance of the next tick (frame interpolation).
    pub next_speed: i32,
    /// `m_AccTime` (`+0xA4`): time index of the drive curve in ms, multiple of 100.
    pub acc_time: i32,
    /// `m_Status` (`+0xA8`), Bits in [`status`].
    pub status: u32,
    /// `m_IsSlave` (`+0xAC`): tender or wagon (no own tick).
    pub is_slave: bool,
    /// `m_Direction` (`+0xB0`): travel vector of the object direction (negated backward).
    pub dir: (i32, i32),
    /// `m_ZielRasterPos` (`+0xD0`) with `m_IsZielRasterPosValid` (`+0xD8`).
    pub target: Option<(i32, i32)>,
    /// `m_TimeCounter` (`+0xFC`): remainder until the self-departure in ms.
    pub time_counter: i32,
    /// `m_HasSupplyLoaded` (`+0x100`).
    pub has_supply: bool,
    /// `m_OutsideStatus` (`+0xF8`): 0 in the map, 1 out backward, 2 out forward.
    pub outside: i32,
    /// `m_RasterDirection` (`+0xC0/+0xC4`): `(cos >> 12, sin >> 12)` of the init direction.
    pub raster_dir: (i32, i32),
    /// Last blocked tile (`+0xC8/+0xCC`); not in the original savegame, the reimplementation needs
    /// it because it also saves the path grid (re/train-multipart 5).
    pub last_block_tile: (i32, i32),
}

impl Default for Train {
    /// `PropertyTrain_Construct`; the constructor does not set `m_OutsideStatus` (a remainder != 0
    /// is possible, K-243) - the reimplementation takes 0 (A7).
    fn default() -> Self {
        Self {
            slaves: Vec::new(),
            master: ObjectId::NONE,
            fwd_anim: 0,
            back_anim: 0,
            stop_anim: 0,
            wait_anim: 0,
            speed: 0,
            next_speed: 0,
            acc_time: 0,
            status: 0,
            is_slave: false,
            dir: (0, 0),
            target: None,
            time_counter: 0,
            has_supply: false,
            outside: 0,
            raster_dir: (0, 0),
            last_block_tile: (0, 0),
        }
    }
}

/// Train state of the object.
pub fn train<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Train> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Train(t) => Some(t),
        _ => None,
    }
}

pub fn train_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Train> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Train(t) => Some(t),
        _ => None,
    }
}

/// Search radius of the snapping: 4 tiles (`[0x00672D80] * 4`).
const SNAP_RADIUS: i32 = 4 << 12;
/// Creation flags of the slaves (`0x8D3`: type, position, rotation y, slot, color, finished; with
/// `PLACED` if the locomotive is created finished or pre-placed).
const SLAVE_DESC_FLAGS: u32 = desc_flags::GUID
    | desc_flags::POSITION
    | desc_flags::ROT_Y
    | desc_flags::PLAYER
    | desc_flags::COLOR
    | desc_flags::READY;

/// `Train_SetupWagonsAndSmoke` (`0x00530220`, slot 9, re/train-multipart 3.1): flags (340001),
/// animations (340002-340005), render bits `|= 3`. Only the locomotive (GUID 2613, K-240) snaps to
/// the nearest track within 4 tiles (direction, rotation, position with `keep_y`), sets the
/// raster direction and blocks its track tiles; every train object gets the travel vector of its
/// direction, status 0 and the waiting animation. Tender and wagon (GUID != 2613) are slaves and end
/// here; the locomotive couples tender and wagon. The smoke (`lok_rauch.pef` at `dampf%02d`) is
/// presentation (host). The original sets `m_MasterHID` to 0 here; the reimplementation leaves "none"
/// (`ObjectId::NONE`), an ID 0 does not exist.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc, ready: bool) {
    let flags = init_flags(INDEX, desc);
    let value = |k: u32| desc.value_int(k).unwrap_or(0) as i32;
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.base.flags = flags;
    }
    if let Some(t) = train_mut(sim, id) {
        t.fwd_anim = value(340002);
        t.back_anim = value(340003);
        t.stop_anim = value(340004);
        t.wait_anim = value(340005);
    }
    let Some(o) = sim.object_mut(id) else { return };
    o.render_bits |= 3;
    let (guid, grid) = (o.guid, o.grid);
    let is_loco = guid == LOCO;
    if is_loco {
        let filter = MatchFilter {
            mode: 2,
            match_bits: 0x0200_0003,
            ..MatchFilter::constructed()
        };
        let pos = (grid.x.0, grid.z.0);
        let player = i32::from(id.slot());
        if let Some(track) = find_nearest_matching(sim, pos, &filter, SNAP_RADIUS, player, -1)
            && let Some(t) = sim.object(track)
        {
            let (dir, rot_y, tgrid) = (t.direction.0, t.rotation[1], t.grid);
            set_direction(sim, id, dir);
            if let Some(o) = sim.object_mut(id) {
                o.rotation[1] = rot_y;
            }
            set_grid_position(sim, id, tgrid, true);
        }
    }
    let Some(d) = sim.object(id).map(|o| o.direction.0) else {
        return;
    };
    let wait = train(sim, id).map_or(0, |t| t.wait_anim);
    if let Some(t) = train_mut(sim, id) {
        t.dir = (cos_deg(d), sin_deg(d));
        t.status = 0;
    }
    if let Some(o) = sim.object_mut(id) {
        o.set_animation(wait, 0, true, true, false);
    }
    if !is_loco {
        if let Some(t) = train_mut(sim, id) {
            t.is_slave = true;
        }
        return;
    }
    if let Some(t) = train_mut(sim, id) {
        t.raster_dir = (t.dir.0 >> 12, t.dir.1 >> 12);
    }
    blocking::block_track_tiles(sim, id);
    let Some(tender) = spawn_coupled(sim, id, id, TENDER, ready) else {
        log::debug!("Train {id:?}: tender not created");
        return;
    };
    attach_slave(sim, id, tender);
    let Some(wagon) = spawn_coupled(sim, id, tender, WAGON, ready) else {
        log::debug!("Train {id:?}: wagon not created");
        return;
    };
    attach_slave(sim, id, wagon);
}

/// Coupling node of a type (world units in object space); a missing node is, as in the original,
/// the zero vector (with a debug line).
fn train_node(sim: &Sim, guid: u32, hitch: bool) -> [f32; 3] {
    let nodes = sim
        .rules
        .train_nodes
        .get(&guid)
        .copied()
        .unwrap_or_default();
    let node = if hitch { nodes.hitch } else { nodes.coupling };
    node.unwrap_or_else(|| {
        let name = if hitch { "vhinten01" } else { "vvorne01" };
        log::debug!("Train type {guid}: node {name} missing, zero vector");
        [0.0; 3]
    })
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Slave `guid` at the hitch `vhinten01` of `front` (3.1 `spawn_coupled`): hitch point
/// `h = world position(front) + R*hitch`, created with slot, color and rotation of the locomotive
/// (nested init), waiting animation of the locomotive, then shifted so that its `vvorne01`
/// lies on `h` (`0x00473B20` = `GridPos_FromWorldFloat` + `SetGridPosition(..., 1)`). The
/// original uses for hitch and coupling of tender **and** wagon a matrix from the rotation of the
/// locomotive (`+0x158`); the reimplementation therefore always rotates with the direction of the
/// locomotive (deviation from the original: table sine instead of x87, `f32` like the original).
fn spawn_coupled(
    sim: &mut Sim,
    lok: ObjectId,
    front: ObjectId,
    guid: u32,
    ready: bool,
) -> Option<ObjectId> {
    let (dir, rot_y, color) = sim
        .object(lok)
        .map(|o| (o.direction, o.rotation[1], o.color))?;
    let (front_pos, front_guid) = sim.object(front).map(|o| (o.world_pos, o.guid))?;
    let wait = train(sim, lok).map_or(0, |t| t.wait_anim);
    let hitch = add(
        rotate_y_by_direction(train_node(sim, front_guid, true), dir),
        front_pos,
    );
    let mut desc = ObjectDesc {
        flags: SLAVE_DESC_FLAGS | if ready { desc_flags::PLACED } else { 0 },
        guid,
        player: i32::from(lok.slot()),
        position: hitch,
        scale: [1.0; 3],
        rotation: [0.0, rot_y, 0.0],
        color,
    };
    let new = create_object(sim, &mut desc)?;
    // Script name with the locomotive as the old ID (`0x00530855` `"tender"`, `0x00530B8C` `"wagon"`).
    // The name of the locomotive is already set: `create_object_named` enters it before the module init.
    let suffix = if guid == TENDER { "tender" } else { "wagon" };
    crate::names::transfer(&mut sim.state.names, lok, new, suffix);
    if let Some(o) = sim.object_mut(new) {
        o.set_animation(wait, 0, true, true, false);
    }
    let pos = sim.object(new)?.world_pos;
    let coupling = add(
        rotate_y_by_direction(train_node(sim, guid, false), dir),
        pos,
    );
    let diff = [
        hitch[0] - coupling[0],
        hitch[1] - coupling[1],
        hitch[2] - coupling[2],
    ];
    let cell = sim.rules.map.cell;
    set_grid_position(sim, new, world_to_grid(add(pos, diff), cell), true);
    Some(new)
}

/// `AddTail` into the slave list of the locomotive, then `0x621 {loco}` to the slave.
fn attach_slave(sim: &mut Sim, lok: ObjectId, slave: ObjectId) {
    if let Some(t) = train_mut(sim, lok) {
        t.slaves.push(slave);
        t.is_slave = false;
    }
    crate::messaging::send(sim, slave, &Message::new(0x621, &[lok.as_i32()]));
}

/// `PropertyTrain_Query` (`0x00531560`, slot 7, re/train-multipart 2.1). Unknown codes return 0
/// instead of -1 (K-250); brake sparks `0x921`/`0xA21` are presentation and answer only 1.
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    match msg.code {
        0x121 | 0x221 => {
            let target = (msg.args.len() == 2).then(|| (msg.arg(0), msg.arg(1)));
            drive::depart(sim, id, msg.code == 0x221, target);
        }
        0x321 => drive::start_braking(sim, id),
        0x421 => drive::whistle(sim, id),
        0x521 => {
            // `{delay, code, argc, args...}` (`0x0052EF80`).
            let argc = usize::try_from(msg.arg(2)).unwrap_or(0);
            let args = msg.args.get(3..).unwrap_or(&[]);
            let inner = Message::new(msg.arg(1) as u32, &args[..argc.min(args.len())]);
            let delay = msg.arg(0);
            if let Some(m) = sim.module_mut(id, INDEX) {
                m.base.post_delayed_message(delay, inner, false, 0x80);
            }
        }
        0x621 => {
            let master = ObjectId::from_i32(msg.arg(0));
            if let Some(t) = train_mut(sim, id) {
                t.master = master;
            }
        }
        0x721 => drive::leave_map(sim, id),
        0x821 => {
            // `Train_LoadSupply` (`0x0052EFA0`): only without loaded supply.
            let ms = msg.arg(0);
            if let Some(t) = train_mut(sim, id)
                && !t.has_supply
            {
                t.time_counter = ms;
                t.has_supply = true;
            }
        }
        0x921 | 0xA21 => return 1,
        0xB21 => {
            // Travel vector and group progress (in tick 0) for the frame interpolation.
            let Some(t) = train(sim, id) else { return 0 };
            let (dir, moving, next) = (t.dir, t.status & status::MOVING != 0, t.next_speed);
            msg.set_arg(0, dir.0);
            msg.set_arg(1, dir.1);
            msg.set_arg(2, 0);
            return if moving { next } else { -1 };
        }
        _ => {}
    }
    0
}

/// `PropertyTrain_Tick` (`0x00531690`, Slot 13): [`drive::tick`].
pub fn tick(sim: &mut Sim, id: ObjectId, dt: i32) {
    drive::tick(sim, id, dt);
}
