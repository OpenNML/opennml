//! Behavior settings of the walk module (`BehaviourSettings`, `0x34` bytes; `0x2100`, `0x3D00`,
//! `ApplyBehaviourMsg` `0x00537800`; re/combat-attack 7a.11/7a.12).

use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::Message;
use crate::motion::NO_DIRECTION;
use crate::sim::Sim;

use super::{position, walk_mut};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviourSettings {
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
    /// `+0x188 m_targetOffset`.
    pub target_offset: (i32, i32),
    /// `+0x194 m_offsetInObjectSpace`.
    pub offset_in_object_space: bool,
    /// `+0x198` return-home counter.
    pub home_timer: i32,
}

impl Default for BehaviourSettings {
    /// `BehaviourSettings_Construct` (`0x00532640`).
    fn default() -> Self {
        Self {
            try_to_follow: true,
            mobile_home: true,
            home: (-1, -1, NO_DIRECTION),
            tether: -1,
            target_dir_offset: 0,
            target_offset: (0, 0),
            offset_in_object_space: false,
            home_timer: 0,
        }
    }
}

/// `ApplyBehaviourMsg` (`0x00537800`), message `{follow, mobile, tether, abort, backup}`:
/// `backup == 1` -> current settings to `+0x1D0`; offsets 0; `MobileHome` -> home = own position
/// (direction NONE), tether `[2]` (-2 -> sight*1024, -3 -> 8 tiles), otherwise no home, tether -1;
/// `abort == 1` -> `AbortAllModules` (without effect when creating).
pub fn apply_behaviour_msg(sim: &mut Sim, id: ObjectId, msg: &Message, abort: bool) {
    let pos = position(sim, id);
    let sight = sim.attribute(id, 18);
    let Some(w) = walk_mut(sim, id) else { return };
    if msg.arg(4) == 1 {
        w.backup_behaviour = w.behaviour();
    }
    w.target_dir_offset = 0;
    w.target_offset = (0, 0);
    w.offset_in_object_space = false;
    w.try_to_follow = msg.arg(0) == 1;
    w.mobile_home = msg.arg(1) == 1;
    if w.mobile_home {
        w.home = (pos.0, pos.1, NO_DIRECTION);
        w.tether = match msg.arg(2) {
            -2 => sight << 10,
            -3 => 8 << 12,
            t => t,
        };
    } else {
        w.home = (-1, -1, NO_DIRECTION);
        w.tether = -1;
    }
    if abort && msg.arg(3) == 1 {
        crate::messaging::abort_all_modules(sim, id);
    }
}

/// `0x2100 {}`: settings from `+0x19C` (only constructor and loading write them), return-home counter -1.
pub fn restore_behaviour(sim: &mut Sim, id: ObjectId) {
    if let Some(w) = walk_mut(sim, id) {
        let mut b = w.restore_behaviour;
        b.home_timer = -1;
        w.set_behaviour(&b);
    }
}

/// `0x3D00`: settings from `+0x1D0` including the return-home counter.
pub fn restore_backup_behaviour(sim: &mut Sim, id: ObjectId) {
    if let Some(w) = walk_mut(sim, id) {
        let b = w.backup_behaviour;
        w.set_behaviour(&b);
    }
}
