//! Module 0 `CPropPtToPtWalking` (re/walk): movement as a service for all orders.
//! FINDTO plans a tile path (`PlanHighPath`), PATH creates the waypoints, the trajectory is projected up to 101
//! points ahead with the movement model and one point is played back per object tick
//! (`goto`, `plan`, `project`); also replanning with the B* local planner, collision handling
//! (`collide`), the combat part (`seek`: FINDTO_UNIT, pursuit) and the walking animation (`anim`).
//! Groups and formations (`formation`, re/groups-formations); riders and boat occupants
//! pass commands and queries on to their mount (`mount_of`), carts and stagecoaches pull their rear part
//! along via `module::multipart` (the spin threshold `0x2F00 {180}` is set by the front part).

mod anim;
pub mod behaviour;
mod collide;
pub(crate) mod formation;
mod goto;
mod plan;
mod project;
mod seek;

use std::collections::VecDeque;

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::bstar::Request;
use crate::group::NO_GROUP;
use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::messaging::{dispatch_command, send, send_fault, send_mut};
use crate::motion::{NO_DIRECTION, SpeedInfo, TrajPoint};
use crate::object::status;
use crate::path::{base_layer, layer_from_move_mask};
use crate::sim::Sim;

use super::ModuleKind;
use behaviour::BehaviourSettings;

pub use anim::frame_animation;
pub use collide::PairMap;
pub use goto::{clamp_pos_to_map, facing_dir_with_offset, find_to_msg, goal_radius};

/// Module index.
pub const INDEX: usize = 0;
/// Status bit "walk waits or plans" (`GameObject+0x1AC`).
pub const WAITING: u32 = 0x10;
/// Status bit "reevaluate collisions"; blocks the command queue for one tick.
pub const REEVALUATE: u32 = 0x800;
/// Walk flag `UNITCLEARINGUP` (10003): clearing-up sight (`PropertyWalk_InitBase` `0x00532A5D`).
pub const UNITCLEARINGUP: u32 = 0x8_0000;
/// Largest distance `Fixed_Hypot` can return (fixed point 1/4096), with headroom. `fixed_div(hi, 0x1000)`
/// is always in `[−2²¹, 2²¹]` because of the `i32` wraparound, `fixed_mul(entry, s)` with `entry ≤ ~1.415·4096`
/// hence at most about 1.415*2^21; `edge_distance_to_point` and the hard radius are >= 0. `3 · 2²¹` leaves
/// plenty of margin. (Where `Fixed_Div` returns the divisor 0 for differences around 2^22, `fixed_hypot` ends with 0,
/// E-118.)
pub const MAX_SEEK_DISTANCE: i32 = 3 << 21;
/// Largest tether of the commands `0x6A`/`0x6C` (`Walk::tether`, fixed point 1/4096; A7). The only calculation
/// with it (`seek.rs`: `dist - tether > max_r || tether + dist < min_r`, `i32`) would overflow as soon as
/// `tether + dist` rises above `i32::MAX`. With `dist ≤` [`MAX_SEEK_DISTANCE`] the sum stays in `i32` for every
/// map; `dist − tether` (both >= 0) anyway. A larger tether (only from a savegame)
/// is calculated there with wraparound like the original. -1 means no tether, -2 and -3 are sentinel values of the
/// behavior message.
pub const MAX_TETHER: i32 = i32::MAX - MAX_SEEK_DISTANCE;
/// Largest step size per tick the planner can still divide (`0x800 / cur` >= 1, plan.rs).
pub(crate) const MAX_STEP: i32 = 0x800;

/// Waypoint of the HighPath list (`HighPoint`, 16 bytes); `id` replaces the node pointer of the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighPoint {
    pub id: u32,
    pub x: i32,
    pub z: i32,
}

/// `PropertyWalk` (`0x2F4` bytes, re/walk 1.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Walk {
    /// `+0x70 m_HighPath`.
    pub high_path: VecDeque<HighPoint>,
    /// `+0x27C m_NextTargetHighPathPos` (ID of the waypoint).
    pub next_hp: Option<u32>,
    /// Next free waypoint ID.
    pub next_point_id: u32,
    /// `+0x9C m_SpeedInfo`.
    pub speed: SpeedInfo,
    /// `+0xD0 m_IsBarging`.
    pub barging: bool,
    /// `+0xD4 m_faceThisPoint` (-1, -1 = none).
    pub face_point: (i32, i32),
    /// `+0xE0 m_targetDir` (`NO_DIRECTION` = any).
    pub target_dir: i32,
    /// `+0xE4 m_targetDirTolerance`.
    pub tolerance: i32,
    /// `+0xE8 m_WaitCounter`.
    pub wait_counter: i32,
    /// `+0xEC` planning phase.
    pub phase: i32,
    /// `+0xF0 m_EndeErreicht`: 0 goal, 1 continue, 2 blocked, 3 ended without a goal, 4 error.
    pub ende: i32,
    /// `+0xF4 m_Trajectory`.
    pub trajectory: VecDeque<TrajPoint>,
    /// `+0x110 m_WaitForHID`.
    pub wait_for: ObjectId,
    /// `+0x114…+0x120`: walking, running, turning positive, turning negative.
    pub anims: [i32; 4],
    /// `+0x124 m_DestHID`.
    pub dest: ObjectId,
    /// `+0x128`, `+0x12E`: last seen status bits and "turning" of the animation choice.
    pub last_status: u32,
    pub last_turning: bool,
    /// `+0x12F m_InObstacle`.
    pub in_obstacle: bool,
    /// `+0x134 m_RestoreStatus`.
    pub restore_state: i32,
    /// `+0x138 m_Status`.
    pub state: i32,
    /// `+0x140 m_MinAttackRadius` (-1 = none).
    pub min_attack_radius: i32,
    /// `+0x144 m_MaxAttackRadius` (-1 = none).
    pub max_attack_radius: i32,
    /// `+0x148 m_MaxGoalRadius`.
    pub max_goal_radius: i32,
    /// `+0x150 m_QuarryHID`: the pursued target (`NONE` = none).
    pub quarry: ObjectId,
    /// `+0x158/+0x15C m_LastKnownPos`: last known position of the target.
    pub last_known_pos: (i32, i32),
    /// `+0x160 m_TicksSinceLastUpdate`: ticks since the last update of the target pursuit.
    pub ticks_since_update: i32,
    /// `+0x164 m_Escort`: escort instead of attacker.
    pub escort: bool,
    /// `+0x165 m_Attack`: attack instead of simple pursuit.
    pub attack: bool,
    /// `+0x16C m_TryToFollowVanishedUnits`.
    pub try_to_follow: bool,
    /// `+0x16D m_MobileHome`.
    pub mobile_home: bool,
    /// `+0x170 m_HomePosition` (x, z, direction).
    pub home: (i32, i32, i32),
    /// `+0x180 m_TetherLength`.
    pub tether: i32,
    /// `+0x184 m_targetDirOffset`.
    pub target_dir_offset: i32,
    /// `+0x188 m_targetOffset` (x, z).
    pub target_offset: (i32, i32),
    /// `+0x194 m_offsetInObjectSpace`.
    pub offset_in_object_space: bool,
    /// `+0x198` return-home counter.
    pub home_timer: i32,
    /// `+0x19C m_RestoreBehaviourSettings`: source for `0x2100 {}`; only the constructor and loading
    /// write it.
    pub restore_behaviour: BehaviourSettings,
    /// `+0x1D0 m_BackupRestoreBehaviourSettings`: backup of `ApplyBehaviourMsg` (`arg4 == 1`),
    /// source for `0x3D00`.
    pub backup_behaviour: BehaviourSettings,
    /// `+0x204` last B* result (8 = replanning requested).
    pub last_bstar_result: i32,
    /// `+0x210 m_PPHandleNxN`.
    pub layer: usize,
    /// `+0x21C m_BackupMsg`.
    pub backup: Option<Envelope>,
    /// `+0x230 m_FindToMsg`.
    pub find_to: Option<Message>,
    /// `+0x23C m_CanMove`.
    pub can_move: bool,
    /// `+0x240` B* request.
    pub request: Request,
    /// `+0x278 m_HIDToEscape`.
    pub hid_to_escape: ObjectId,
    /// `+0x280 m_IsFollowingBStar`.
    pub following_bstar: bool,
    /// `+0x284 m_HowManyAttackersBeforeSurrounded` (`InitBase`: `(hard radius << 2) / 0x200`);
    /// stored for the savegame and fidelity to the original; the reimplementation does not read it.
    pub attackers_before_surrounded: i32,
    /// `+0x288 m_SwimmingDisallowedForCombat`: is never reset (quirk, re/walk 16.1);
    /// `stop_moving` evaluates it, it is set in the walk combat part (`seek`).
    pub swim_disallowed_for_combat: bool,
    /// `+0x2F0` A* retries.
    pub retries: i32,
    /// `+0x28C`, `+0x2A8`: reported hard and soft collisions of the running tick.
    pub hard_pairs: PairMap,
    pub soft_pairs: PairMap,
    /// `+0x208` group ID (`0x1500`/`0x1600`, re/groups-formations 2.1).
    pub group: i32,
    /// `+0x214` `m_FormationNum` (VALUEID 10011, rank 0…9).
    pub formation_num: i32,
    /// `+0x218` `m_FormationNumMounted` (VALUEID 10012).
    pub formation_num_mounted: i32,
    /// `+0x2C4` ticks since the last catching up (state 0xB); -1 after `StopMoving`.
    pub ticks_since_chase: i32,
    /// `+0x2C8` a catch-up point is at the front of the HighPath.
    pub chase_pos: bool,
}

impl Walk {
    /// `PropertyWalk_Construct` (`0x005382A0`) up to just before `ApplyBehaviourMsg`: no home, no
    /// tether, both behavior copies like `BehaviourSettings_Construct`; return-home counter `ID mod 17`.
    /// [`init`] applies the behavior mode of the owner slot.
    pub fn new(id: ObjectId) -> Self {
        Self {
            high_path: VecDeque::new(),
            next_hp: None,
            next_point_id: 0,
            speed: SpeedInfo::default(),
            barging: false,
            face_point: (-1, -1),
            target_dir: NO_DIRECTION,
            tolerance: 0,
            wait_counter: 0,
            phase: 0,
            ende: 1,
            trajectory: VecDeque::new(),
            wait_for: ObjectId::NONE,
            anims: [-1; 4],
            dest: ObjectId::NONE,
            last_status: u32::MAX,
            last_turning: false,
            in_obstacle: false,
            restore_state: 0,
            state: 0,
            min_attack_radius: -1,
            max_attack_radius: -1,
            max_goal_radius: -1,
            quarry: ObjectId::NONE,
            last_known_pos: (-1, -1),
            ticks_since_update: 0,
            escort: false,
            attack: false,
            try_to_follow: true,
            mobile_home: false,
            home: (-1, -1, NO_DIRECTION),
            tether: -1,
            target_dir_offset: 0,
            target_offset: (0, 0),
            offset_in_object_space: false,
            home_timer: id.as_i32() % 17,
            restore_behaviour: BehaviourSettings::default(),
            backup_behaviour: BehaviourSettings::default(),
            last_bstar_result: 8,
            layer: 0,
            backup: None,
            find_to: None,
            can_move: true,
            request: Request::default(),
            hid_to_escape: ObjectId::NONE,
            following_bstar: false,
            attackers_before_surrounded: -1,
            swim_disallowed_for_combat: false,
            retries: 0,
            hard_pairs: PairMap::default(),
            soft_pairs: PairMap::default(),
            group: NO_GROUP,
            formation_num: 0,
            formation_num_mounted: 0,
            ticks_since_chase: -1,
            chase_pos: false,
        }
    }

    /// 1x1 layer of the same terrain class (`PropertyWalk_GetLayer1x1` `0x00533CB0`).
    pub fn layer_1x1(&self) -> usize {
        base_layer(self.layer.min(crate::path::LAYER_COUNT - 1))
    }

    /// The running behavior settings (`+0x16C…+0x198`).
    pub fn behaviour(&self) -> BehaviourSettings {
        BehaviourSettings {
            try_to_follow: self.try_to_follow,
            mobile_home: self.mobile_home,
            home: self.home,
            tether: self.tether,
            target_dir_offset: self.target_dir_offset,
            target_offset: self.target_offset,
            offset_in_object_space: self.offset_in_object_space,
            home_timer: self.home_timer,
        }
    }

    /// Take over behavior settings (copy of the `0x34` bytes to `+0x168`).
    pub fn set_behaviour(&mut self, b: &BehaviourSettings) {
        self.try_to_follow = b.try_to_follow;
        self.mobile_home = b.mobile_home;
        self.home = b.home;
        self.tether = b.tether;
        self.target_dir_offset = b.target_dir_offset;
        self.target_offset = b.target_offset;
        self.offset_in_object_space = b.offset_in_object_space;
        self.home_timer = b.home_timer;
    }

    /// New waypoint with a fresh ID.
    pub(crate) fn point(&mut self, x: i32, z: i32) -> HighPoint {
        let id = self.next_point_id;
        self.next_point_id = self.next_point_id.wrapping_add(1);
        HighPoint { id, x, z }
    }

    /// Position of the waypoint `id` in the list.
    fn index_of(&self, id: Option<u32>) -> Option<usize> {
        let id = id?;
        self.high_path.iter().position(|p| p.id == id)
    }
}

pub(crate) fn walk<'s>(sim: &'s Sim, id: ObjectId) -> Option<&'s Walk> {
    match &sim.module(id, INDEX)?.kind {
        ModuleKind::Walk(w) => Some(w),
        _ => None,
    }
}

pub(crate) fn walk_mut<'s>(sim: &'s mut Sim, id: ObjectId) -> Option<&'s mut Walk> {
    match &mut sim.module_mut(id, INDEX)?.kind {
        ModuleKind::Walk(w) => Some(w),
        _ => None,
    }
}

/// Grid position (x, z) of the object.
pub(crate) fn position(sim: &Sim, id: ObjectId) -> (i32, i32) {
    sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0))
}

/// `GameObject_GetEffectiveDirection` (`0x00428B80`); the direction offset belongs to SiegeEngine.
pub(crate) fn direction(sim: &Sim, id: ObjectId) -> i32 {
    sim.object(id).map_or(0, |o| o.direction.0)
}

pub(crate) fn set_status(sim: &mut Sim, id: ObjectId, flags: u32, on: bool) {
    if let Some(o) = sim.object_mut(id) {
        o.set_status_flags(flags, on);
    }
}

/// `PropertyWalk_SetState` (`0x00533F00`).
pub(crate) fn set_state(sim: &mut Sim, id: ObjectId, state: i32) {
    if let Some(w) = walk_mut(sim, id) {
        w.state = state;
    }
}

/// `PropertyWalk_InitBase` (`0x00532850`, slot 4 = `ReadParams`, re/walk 1.3, re/motion 5).
pub fn read_params(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    let value = |v: u32| desc.value_int(v).unwrap_or(0) as i32;
    let flags = value(10003) as u32;
    let hard = value(10004);
    let speed_attr = sim.attribute(id, 16);
    let mut speed = walk(sim, id).map_or_else(SpeedInfo::default, |w| w.speed);
    speed.apply_unit(value(10006), value(10007), speed_attr, hard);
    if let Some(o) = sim.object_mut(id) {
        o.hard_radius = hard;
        o.soft_radius = value(10005);
        if flags & UNITCLEARINGUP != 0 {
            o.clear_up_sight = true;
        }
    }
    let Some(m) = sim.module_mut(id, INDEX) else {
        return;
    };
    m.base.flags = flags;
    if let ModuleKind::Walk(w) = &mut m.kind {
        w.layer = layer_from_move_mask(flags >> 8);
        w.anims = [value(10001), value(10008), value(10009), value(10010)];
        w.speed = speed;
        w.attackers_before_surrounded = (hard << 2) / 0x200;
        w.formation_num = value(10011);
        w.formation_num_mounted = value(10012);
    }
}

/// `PropertyWalk_Init` (`0x0053CC30`): apply the player's behavior default, read parameters; if the unit stands on a
/// tile blocked in its own layer, set it to the center of the nearest free tile (otherwise height onto the
/// terrain); state 1.
pub fn init(sim: &mut Sim, id: ObjectId, desc: &ModuleDesc) {
    if let Some(o) = sim.object_mut(id) {
        o.render_bits |= 3;
    }
    if let Some(m) = sim.module_mut(id, INDEX) {
        m.kind = ModuleKind::Walk(Box::new(Walk::new(id)));
    }
    // Constructor default (`0x005382A0`): player behavior mode of the owner slot
    // (`GameWorld+0x2294`, default 2), without `AbortAllModules` (no modules registered yet).
    let mode = sim.state.combat.behavior_mode(id.slot());
    let msg = crate::module::quarter::walk_behavior_msg(mode, false);
    behaviour::apply_behaviour_msg(sim, id, &msg, false);
    read_params(sim, id, desc);
    let layer = walk(sim, id).map_or(0, |w| w.layer);
    let tile = sim.object(id).map_or((0, 0), |o| o.tile);
    if sim.state.path.cell_cost(tile.0, tile.1, layer) != crate::path::grid::COST_FREE {
        if let Some((x, z)) = sim.state.path.layers[layer].find_free_cell_near(tile.0, tile.1) {
            let y = sim.object(id).map_or(crate::fx::Fx::ZERO, |o| o.grid.y);
            let p = crate::fx::FxVec3::new(
                crate::fx::Fx((x << 12) + 0x800),
                y,
                crate::fx::Fx((z << 12) + 0x800),
            );
            crate::world::set_grid_position(sim, id, p, true);
        }
    } else {
        crate::world::snap_height_to_terrain(sim, id);
    }
    set_state(sim, id, 1);
}

/// `PropertyWalk_Abort` (`0x00535120`, slot 11): halt and send a fault message.
pub fn abort(sim: &mut Sim, id: ObjectId) -> bool {
    if let Some(w) = walk_mut(sim, id) {
        w.retries = 0;
    }
    goto::stop_moving(sim, id, false);
    send_fault(sim, id, INDEX);
    true
}

/// Dead (`0x40000`): only `0xC00` left (re/walk 2.1).
fn dead(sim: &Sim, id: ObjectId) -> bool {
    sim.object(id).is_none_or(|o| o.status & status::DIED != 0)
}

/// Rider and occupant bits (`ISMOUNTED | ISMOUNTEDONBOAT`).
const RIDER_BITS: u32 = status::ISMOUNTED | status::ISMOUNTEDONBOAT;

/// The mount or boat of a rider or occupant: status `0x2000`/`0x8000` and an existing
/// target (`target` neither `NONE` nor 0, object present, `GameObject_GetTargetObject`
/// `0x00466910`). Never keep a reference across `send`/`dispatch_command` (re/riding 4.1).
pub(crate) fn mount_of(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    let o = sim.object(id)?;
    if o.status & RIDER_BITS == 0 {
        return None;
    }
    let t = o.target;
    (!t.is_none() && t.0 != 0 && sim.object(t).is_some()).then_some(t)
}

/// `PropertyWalk_OnMessage` (`0x0053ACF0`, slot 12, re/walk 2.1/2.2).
pub fn on_message(sim: &mut Sim, id: ObjectId, env: &Envelope) -> bool {
    let code = env.msg.code;
    if dead(sim, id) && code != 0xC00 {
        return false;
    }
    // Pre-check (re/walk 2.1): riders and occupants pass everything on to the mount and never plan
    // themselves; without a target both bits fall and the walker handles the message itself.
    if sim.object(id).is_some_and(|o| o.status & RIDER_BITS != 0) {
        if let Some(m) = mount_of(sim, id) {
            return dispatch_command(sim, m, env) != 0;
        }
        set_status(sim, id, status::ISMOUNTED, false);
        set_status(sim, id, status::ISMOUNTEDONBOAT, false);
    }
    match code {
        0x100 | 0x200 | 0x300 => {
            let run = code == 0x100
                && sim
                    .module(id, INDEX)
                    .is_some_and(|m| m.base.flags & 0x4000 != 0);
            goto::on_msg_find_to(sim, id, env, run)
        }
        0x500 | 0x600 => {
            let run = code == 0x600
                && sim
                    .module(id, INDEX)
                    .is_some_and(|m| m.base.flags & 0x4000 != 0);
            goto::on_msg_path_to(sim, id, env, run, true)
        }
        0x400 => seek::on_msg_find_to_unit(sim, id, env),
        0x800 => collide::on_msg_collision_record(sim, id, &env.msg),
        0x900 => goto::on_msg_check_pos(sim, id, env),
        0xC00 => {
            if env.msg.args.len() == 1 {
                goto::stop_moving(sim, id, true);
                true
            } else {
                abort(sim, id)
            }
        }
        _ => false,
    }
}

/// `PropertyWalk_Query` (`0x0053B5A0`, slot 7, re/walk 2.5): synchronous queries; all other
/// codes go to `OnMessage` without feedback messages. `formation::query` answers the formation queries.
/// Riders and occupants pass all queries except `0x2100` and `0x2000` on to the mount
/// (re/walk 2.1, re/riding 4.1); without a target they handle them themselves and keep the bits.
pub fn query(sim: &mut Sim, id: ObjectId, msg: &mut Message) -> i32 {
    if dead(sim, id) && msg.code != 0xC00 {
        return -1;
    }
    if !matches!(msg.code, 0x2100 | 0x2000)
        && let Some(m) = mount_of(sim, id)
    {
        return send_mut(sim, m, msg);
    }
    if let Some(r) = formation::query(sim, id, msg) {
        return r;
    }
    let Some(w) = walk(sim, id) else { return -1 };
    match msg.code {
        0x700 => collide::on_msg_walk_wait(sim, id, msg),
        // `GetCurrentHighPoint` (`0x005349C0`, re/walk 2.5/16.4): head of the HighPath list (not
        // `next_hp`), empty -> own grid position; answer in `arg0/arg1`, return value 1 = list not empty.
        0xA00 => {
            let (head, r) = match w.high_path.front() {
                Some(h) => ((h.x, h.z), 1),
                None => (position(sim, id), 0),
            };
            msg.set_arg(0, head.0);
            msg.set_arg(1, head.1);
            r
        }
        0xB00 => w.layer_1x1() as i32,
        0xD00 => w.speed.cur,
        0xE00 => {
            let on = msg.arg(0) != 0;
            let changed = walk_mut(sim, id).is_some_and(|w| {
                if on {
                    w.speed.start_sprint()
                } else {
                    w.speed.reset_to_cruising()
                }
            });
            let moving = sim.object(id).is_some_and(|o| o.status & status::MOVE != 0);
            if changed && moving {
                project::project_trajectory(sim, id, true, false);
            }
            walk(sim, id).map_or(0, |w| w.layer_1x1() as i32)
        }
        0x1000 => plan::on_msg_repath(sim, id, ObjectId::from_i32(msg.arg(0))),
        0x1400 => w.layer as i32,
        0x1A00 => {
            anim::select_walk_anim(sim, id);
            1
        }
        0x1B00 => {
            let backup = w.backup.clone();
            i32::from(backup.is_some_and(|env| on_message(sim, id, &env)))
        }
        0x1C00 => {
            if let Some(w) = walk_mut(sim, id)
                && w.ende == 0
            {
                w.ende = 1;
            }
            0
        }
        0x1D00 => i32::from(goto::is_idle_at(sim, id, msg)),
        0x1E00 => i32::from(w.barging),
        0x1F00 => w.quarry.as_i32(),
        0x2100 => {
            if msg.args.is_empty() {
                behaviour::restore_behaviour(sim, id);
            } else {
                behaviour::apply_behaviour_msg(sim, id, msg, true);
            }
            // Passing on the original message to mount/boat (`0x2000`/`0x8000`, target present).
            let (st, t) = sim
                .object(id)
                .map_or((0, ObjectId::NONE), |o| (o.status, o.target));
            if st & (status::ISMOUNTED | status::ISMOUNTEDONBOAT) != 0 && !t.is_none() && t.0 != 0 {
                send(sim, t, msg);
            }
            0
        }
        0x2200 => {
            if w.can_move {
                w.tether
            } else {
                0
            }
        }
        0x2400 => i32::from(w.in_obstacle),
        0x2600 => i32::from(w.speed.running),
        0x2500 => {
            let p = match msg.args.len() {
                2 => Some((msg.arg(0), msg.arg(1))),
                0 => Some(position(sim, id)),
                _ => None,
            };
            if let (Some(p), Some(w)) = (p, walk_mut(sim, id)) {
                w.home.0 = p.0;
                w.home.1 = p.1;
            }
            1
        }
        0x2700 => i32::from(w.mobile_home && w.home.0 != -1),
        0x2800 => i32::from(w.can_move),
        0x2900 => {
            if let Some(w) = walk_mut(sim, id) {
                w.can_move = msg.arg(0) != 0;
            }
            1
        }
        0x2B00 => crate::fx::fixed_mul(w.speed.cur, crate::motion::speed::TIME_UNIT),
        0x2C00 => i32::from(w.state == 6),
        0x2E00 => plan::on_msg_wait_patiently(sim, id),
        0x2D00 => {
            if w.state == 1 {
                set_state(sim, id, 6);
            }
            1
        }
        // `IsQuarterPartner` (`0x0053B5A0`, re/ships 13.1): 1 exactly if the stored
        // success message is `0x1617` and its target is `hid` (the argument of the message, the
        // unit, is not compared); after sending it is empty (`send_success`).
        0x3800 => i32::from(sim.module(id, INDEX).is_some_and(|m| {
            m.base.success.as_ref().is_some_and(|s| s.code == 0x1617)
                && m.base.success_target.as_i32() == msg.arg(0)
        })),
        // FOLLOW (re/walk 2.5, 15.4): FINDTO_UNIT to itself, flags 0x2021 (0x2000 follow permanently),
        // tolerance Fixed(180) -> 5 degrees (K-193); return value as for 0x400.
        0x3900 => {
            let m = Message::new(0x400, &[msg.arg(0), 0, 0x1000, 0, 180 << 12, 0x2021, 0, 0]);
            send(sim, id, &m)
        }
        0x2F00 => {
            let g = msg.arg(0);
            if (1..=180).contains(&g) {
                if let Some(w) = walk_mut(sim, id) {
                    w.speed.spin_threshold = g << 12;
                }
                1
            } else {
                0
            }
        }
        0x3000 => collide::query_should_ignore(sim, id, msg),
        0x3100 => i32::from(seek::is_quarry_surrounded(
            sim,
            id,
            ObjectId::from_i32(msg.arg(0)),
        )),
        0x3300 => {
            let p = if w.can_move {
                (w.home.0, w.home.1)
            } else {
                position(sim, id)
            };
            msg.set_arg(0, p.0);
            msg.set_arg(1, p.1);
            1
        }
        0x3400 => i32::from(w.following_bstar),
        0x3500 => w.min_attack_radius,
        0x3700 => i32::from(
            crate::path::LAYER_MASKS.get(w.layer) == Some(&0x24)
                && sim
                    .module(id, INDEX)
                    .is_some_and(|m| m.base.has_flags(0x20000)),
        ),
        0x3D00 => {
            behaviour::restore_backup_behaviour(sim, id);
            0
        }
        0x3E00 => 0,
        0x3F00 => {
            let (old, new) = (
                ObjectId::from_i32(msg.arg(0)),
                ObjectId::from_i32(msg.arg(1)),
            );
            let ok = msg.args.len() == 2 && w.quarry == old;
            if let Some(m) = sim.module_mut(id, INDEX) {
                if m.base.success_target == old {
                    m.base.success_target = new;
                }
                if let ModuleKind::Walk(w) = &mut m.kind
                    && ok
                {
                    w.quarry = new;
                }
            }
            i32::from(ok)
        }
        _ => i32::from(on_message(sim, id, &Envelope::bare(msg.clone()))),
    }
}

/// `PropertyWalk_Tick` (`0x0053E120`, slot 13): `dt` is not read; the counters count
/// object ticks.
pub fn tick(sim: &mut Sim, id: ObjectId) {
    anim::tick_stamina(sim, id);
    match walk(sim, id).map_or(0, |w| w.state) {
        1 => goto::tick_idle_go_home(sim, id),
        2 | 5 => project::tick_walking(sim, id),
        // Catching up the formation (re/groups-formations 3.6): first `TickFormationChase(0)`, then as for 2.
        // State 10 waits.
        0xB => {
            formation::tick_formation_chase(sim, id, false);
            project::tick_walking(sim, id);
        }
        3 => collide::tick_waiting(sim, id),
        4 => plan::tick_planning(sim, id),
        7 => plan::tick_check_pos(sim, id),
        8 => plan::tick_wait_patiently(sim, id),
        9 => plan::tick_retry_find_to(sim, id),
        0xC => seek::tick_quarry_surrounded(sim, id),
        _ => {}
    }
    seek::update_seek_target(sim, id, false, false);
    collide::process_collisions(sim, id);
}

#[cfg(test)]
mod tests;
