//! A game object (`GameObject`, `0x800` bytes, re/sim-core 2): type, position in the fixed-point grid, status bits,
//! modules, messages, command queue and attributes.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::attributes::AttributeSet;
use crate::command::QueuedCommand;
use crate::fixed_vec::FixedVec;
use crate::fx::{Fx, FxVec3};
use crate::id::ObjectId;
use crate::message::{MessageFilter, Posted};
use crate::module::Module;

/// Number of module slots (`GameObject+0x64 + 4n`, n < 0x3C).
pub const MODULE_SLOTS: usize = 0x3C;

/// Status bits (`GameObject+0x1AC`, names from `targets.ini`).
pub mod status {
    pub const MOVE: u32 = 0x1;
    /// Fight in progress (`HS_FIGHT`): set before the blow/shot order of the attack module; blocks the command queue
    /// via [`BUSY_MASK`].
    pub const FIGHTING: u32 = 0x2;
    pub const QUARTERED: u32 = 0x100;
    /// Collision exemption zone active (`GameObject+0x1AC & 0x1000`, re/collision A2.2): the exempt object does not
    /// count as an obstacle when walking (demolition expert to the ship, re/special-attacks 3.1, 3.9, 4.2). The local
    /// constants in `dying.rs`, `gather/lumber.rs` and `riding/mount.rs` stay.
    pub const EXEMPT_ZONE: u32 = 0x1000;
    pub const ISMOUNTED: u32 = 0x2000;
    pub const CARRIING: u32 = 0x4000;
    pub const ISMOUNTEDONBOAT: u32 = 0x8000;
    pub const SWIMMING: u32 = 0x10000;
    pub const DIVING: u32 = 0x2_0000;
    pub const DIED: u32 = 0x40000;
    /// Healing trance (`HEAL`, module 29, re/healing 3.2): set by `0x11D`, cleared by `0x41D`; stand and dying then
    /// ask the healing module for their animation.
    pub const HEAL: u32 = 0x8_0000;
    /// Stealthed (`MASKED`): set by the stealth module 25 (`module::masking`, `0x319` or immediately for a building),
    /// cleared by `0x419`/`0x619` and by dying; fog and hit filters evaluate it (re/vision 6.2, 7; re/masking 4).
    pub const MASKED: u32 = 0x10_0000;
    /// Evocation in progress (`EVOKING`): set by the ghost evocation (module 26, `0x11A` `0x00510D90`,
    /// re/ghosts-banish 3.1) and the animal evocation (module 32, `0x120` `0x005103B0`, re/evoke-aura 3.1), cleared
    /// with the end (`0x31A`, `0x220`) and on abort. Lies in [`BUSY_MASK`]: the command queue waits, the FIFO does
    /// not.
    pub const EVOKING: u32 = 0x20_0000;
    /// Waiting to kill (`WAITING4KILLING`): stealthed, but visible to enemy reconnaissance.
    pub const WAITING4KILLING: u32 = 0x40_0000;
    pub const ONWALL: u32 = 0x0200_0000;
    /// Animation lock in progress (`GameObject_StartAnimationLock` `0x00467C60`). Without a lock time, camouflage
    /// (camouflage/uncamouflage animation), digging in (digging out) and healing (trance transition) also set and
    /// clear the bit (re/masking 14 no. 11, re/healing 3.2).
    pub const ANIM_LOCKED: u32 = 0x80_0000;
    /// States in which an object is "calm" and works through its command queue (`NOTWAITING`).
    pub const BUSY_MASK: u32 = 0xF8EC_0FFF;
}

/// Wanted and arrived animation (`+0x1CC` ... `+0x1DC`, `+0x16C`). The simulation never reads these fields; the
/// rendering plays `current`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimState {
    /// Animation arrived at the rendering (`+0x1CC`, -1 = none, -2 = force restart).
    pub current: i32,
    /// Wanted animation (`+0x1D0`): index of the `ANIMATION` blocks of the object's `.cfg`.
    pub wanted: i32,
    /// Start time within the animation (ms, `+0x1D8`).
    pub start_ms: i32,
    /// Speed (`+0x1D4`, 128 = 1.0).
    pub speed: f32,
    /// Crossfade request (`+0x1DC`).
    pub blend_request: bool,
    /// Crossfade allowed (`+0x16C`, set unless `NO_BLENDING`).
    pub blend_allowed: bool,
    /// Counter of the starts (for the rendering: a new start even of the same animation).
    pub starts: u32,
    /// The last start was requested with a crossfade.
    pub blend: bool,
}

impl Default for AnimState {
    fn default() -> Self {
        Self {
            current: -1,
            wanted: -1,
            start_ms: 0,
            speed: 128.0,
            blend_request: false,
            blend_allowed: true,
            starts: 0,
            blend: false,
        }
    }
}

/// Collision exemption zone (`GameObject+0x2B4…+0x2C8`, effective with status bit `0x1000`): mode 0 = around an
/// object (-1 = always exempt), otherwise a circle (re/collision A2.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollisionExempt {
    /// `+0x2C8`.
    pub mode: i32,
    /// `+0x2C4`.
    pub object: ObjectId,
    /// `+0x2B4/+0x2B8`.
    pub center: (i32, i32),
    /// `+0x2C0`.
    pub radius: i32,
}

impl Default for CollisionExempt {
    fn default() -> Self {
        Self {
            mode: 0,
            object: ObjectId::NONE,
            center: (0, 0),
            radius: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameObject {
    /// ID (`+0x58`); the player slot is encoded in it.
    pub id: ObjectId,
    /// Type GUID (`+0x54`).
    pub guid: u32,
    /// Tick counter of the world at creation (`+0x50`).
    pub created_tick: u32,
    /// World position for the rendering (`+0x08`), derived from the grid.
    pub world_pos: [f32; 3],
    /// Scale (`+0x1C`).
    pub scale: [f32; 3],
    /// Rotation X/Y/Z in radians (`+0x154`), rendering.
    pub rotation: [f32; 3],
    /// Grid position in tiles, fixed point (`+0x19C` x, `+0x1A0` z, `+0x1A4` y).
    pub grid: FxVec3,
    /// Tile (`+0x328/+0x32C`).
    pub tile: (i32, i32),
    /// Facing direction in degrees, fixed point, in [-180 degrees, 180 degrees) (`+0x1C0`).
    pub direction: Fx,
    /// Status bits (`+0x1AC`).
    pub status: u32,
    /// Status changed (`+0x28`, set by `SetStatusFlags`).
    pub dirty: bool,
    /// Category bits (`+0x330`).
    pub category: u32,
    /// Display color (`+0x2D0`).
    pub color: i32,
    /// Height offset (`+0x324`).
    pub height_offset: i32,
    /// Hard collision radius `m_ColRadius` (`+0x1B0`, fixed point).
    pub hard_radius: i32,
    /// Soft collision radius `m_ColWarnRadius` (`+0x1B4`).
    pub soft_radius: i32,
    /// Collision exemption zone.
    pub exempt: CollisionExempt,
    /// Modules at their fixed slot (`+0x64 + 4n`).
    pub modules: FixedVec<Option<Module>, MODULE_SLOTS>,
    /// Tick list of the modules in insertion order (`+0x758`).
    pub tick_list: Vec<u8>,
    /// Queued (de)activations (`+0x770`): `n` = on, `!n` = off.
    pub pending_activation: Vec<i32>,
    /// Message FIFO (`+0x7A8`).
    pub fifo: VecDeque<Posted>,
    /// Message filter (`+0x730`).
    pub filter: MessageFilter,
    /// Command queue (`+0x7CC`, at most 25) and read cursor (`+0x30`).
    pub commands: Vec<QueuedCommand>,
    pub command_cursor: i32,
    pub anim: AnimState,
    /// Animation lock in ms (`+0x174`).
    pub anim_lock_ms: i32,
    pub attributes: AttributeSet,
    /// Carrier (`+0x6E0`).
    pub carrier: ObjectId,
    /// Target/linked object (`+0x2CC`).
    pub target: ObjectId,
    /// Last attacker (`+0x1B8`) and ticks since the last hit (`+0x1BC`).
    pub last_attacker: ObjectId,
    pub ticks_since_hit: i32,
    /// AI mark `m_KISelectionFlag` (`+0x04`, re/ki-queries 5.9, E-57): bit `p` = "marked by the AI on slot `p`". The
    /// original keeps a 32-bit word; the AI sets only bits 0...7 (own slot), so `u8` suffices (bits from 8 on read
    /// 0). `DoMount` clears the whole mask on the mount, death clears nothing.
    pub ai_marks: u8,
    /// `+0x7A4 m_LastExperience`: rank 0...5 (re/combat-damage 4.1).
    pub rank: u8,
    /// Round-robin counter of the hit zone selection (`+0x2DC`, `GameObject_PickHitZoneTowards` `0x0046F2E0`).
    pub hit_zone_counter: i32,
    /// Carried amount and kind (`+0x2A8/+0x2AC`: 0 gold, 1 wood, 2 food).
    pub load_amount: i32,
    pub load_kind: i32,
    /// Tick group (`+0x7C8`: 0/1, -1 = none).
    pub tick_group: i8,
    /// Stored message for the change of owner (`+0x160/+0x164`).
    pub owner_change_target: ObjectId,
    pub owner_change_msg: i32,
    /// Render bits (`+0x1C4`, re/object-classes): `1` model, `2` shadow, `4` crossfade (construction site,
    /// fade-in/out), `8` own effects, `0x20` burning, `0x40` carcass. 0 in the constructor, the module `Init`s set
    /// them (base modules and module 3). Rendering; the simulation writes them like the original.
    pub render_bits: u32,
    /// Scale of the construction progress (`+0x18`, `f32` as in the original: 0.1 ... 1.0 during construction,
    /// constructor 0). Rendering only.
    pub progress_scale: f32,
    /// Selectable (`+0x1A8`, `GameObject_SetSelectable` `0x00466B00`; constructor 1, nature 0).
    pub selectable: bool,
    /// Clear-up sight (`+0x6DC`, `GameObject_SetClearUpSight`): additionally stamps "cleared up"; set by the category
    /// `CANCLEARUP` and the walk flag `UNITCLEARINGUP`, never reset.
    pub clear_up_sight: bool,
    /// Temporary upgrades (`m_TempUpgradeList`, `GameObject+0x788`, re/temp-upgrades 1.2) in insertion
    /// order.
    pub temp_upgrades: Vec<crate::upgrades::TempUpgradeEntry>,
    /// Running counter-upgrades on the object (`m_ShowCounterUpgrade`, `+0x2C`, re/upgrades 1.3): display counter,
    /// never negative (`FUN_00468170`: `max(0, v + d)`).
    pub show_counter_upgrade: i32,
}

impl GameObject {
    /// `GameObject_Construct` (`0x00470500`).
    pub fn new(id: ObjectId, created_tick: u32) -> Self {
        Self {
            id,
            guid: 0,
            created_tick,
            world_pos: [0.0; 3],
            scale: [1.0; 3],
            rotation: [0.0; 3],
            grid: FxVec3::ZERO,
            tile: (0, 0),
            direction: Fx::ZERO,
            status: 0,
            dirty: false,
            category: 0,
            color: 0,
            height_offset: 0,
            hard_radius: 0,
            soft_radius: 0,
            exempt: CollisionExempt::default(),
            modules: FixedVec::default(),
            tick_list: Vec::new(),
            pending_activation: Vec::new(),
            fifo: VecDeque::new(),
            filter: MessageFilter::default(),
            commands: Vec::new(),
            command_cursor: 0,
            anim: AnimState::default(),
            anim_lock_ms: 0,
            attributes: AttributeSet::default(),
            carrier: ObjectId::NONE,
            target: ObjectId::NONE,
            last_attacker: ObjectId::NONE,
            ticks_since_hit: i32::MAX,
            ai_marks: 0,
            rank: 0,
            hit_zone_counter: 0,
            load_amount: 0,
            load_kind: 0,
            tick_group: -1,
            owner_change_target: ObjectId::NONE,
            owner_change_msg: 0,
            render_bits: 0,
            progress_scale: 0.0,
            selectable: true,
            clear_up_sight: false,
            temp_upgrades: Vec::new(),
            show_counter_upgrade: 0,
        }
    }

    /// Player slot (`GameObject_GetOwnerSlot` `0x004669C0`).
    pub fn owner(&self) -> u8 {
        self.id.slot()
    }

    /// `GameObject_HasModule(n, includeDisabled)` (`0x004668A0`).
    pub fn has_module(&self, index: usize, include_disabled: bool) -> bool {
        self.modules
            .get(index)
            .and_then(Option::as_ref)
            .is_some_and(|m| m.base.enabled || include_disabled)
    }

    pub fn module(&self, index: usize) -> Option<&Module> {
        self.modules.get(index)?.as_ref()
    }

    /// Walk module (slot 0), if present (load check of the groups).
    pub fn walk(&self) -> Option<&crate::module::walk::Walk> {
        match &self.module(crate::module::walk::INDEX)?.kind {
            crate::module::ModuleKind::Walk(w) => Some(w),
            _ => None,
        }
    }

    pub fn module_mut(&mut self, index: usize) -> Option<&mut Module> {
        self.modules.get_mut(index)?.as_mut()
    }

    /// `GameObject_SetStatusFlags` (`0x004662D0`): set `dirty` on a change.
    pub fn set_status_flags(&mut self, flags: u32, on: bool) {
        let new = if on {
            self.status | flags
        } else {
            self.status & !flags
        };
        if new != self.status {
            self.status = new;
            self.dirty = true;
        }
    }

    /// `GameObject_SetCollisionExemptCircle` (`0x004668E0`): circle around `(x, z)` with radius `r`
    /// (`+0x2B4/+0x2B8/+0x2C0`), mode 1 (`+0x2C8`); effective only with status bit `0x1000`.
    pub fn set_collision_exempt_circle(&mut self, x: i32, z: i32, r: i32) {
        self.exempt.center = (x, z);
        self.exempt.radius = r;
        self.exempt.mode = 1;
    }

    /// `GameObject_SetCollisionExemptObject` (`0x00467C40`): exempt object (`+0x2C4`), mode 0
    /// (`+0x2C8`).
    pub fn set_collision_exempt_object(&mut self, object: ObjectId) {
        self.exempt.object = object;
        self.exempt.mode = 0;
    }

    /// `GameObject_QueueModuleTickChange` (`0x00467030`).
    pub fn queue_module_tick_change(&mut self, index: usize, on: bool) {
        if index < MODULE_SLOTS {
            let i = index as i32;
            self.pending_activation.push(if on { i } else { !i });
        }
    }

    /// `GameObject_ApplyModuleActivationChanges` (`0x0046CA60`): in insertion order; enabling appends at the back (not
    /// for idle modules), disabling removes.
    pub fn apply_module_activation_changes(&mut self) {
        let pending = std::mem::take(&mut self.pending_activation);
        for v in pending {
            let (n, on) = if v < 0 {
                ((!v) as usize, false)
            } else {
                (v as usize, true)
            };
            let Some(m) = self.modules.get_mut(n).and_then(Option::as_mut) else {
                continue;
            };
            match self.tick_list.iter().position(|&i| usize::from(i) == n) {
                None if on && !m.base.idle => {
                    self.tick_list.push(n as u8);
                    m.base.enabled = true;
                }
                Some(p) if !on => {
                    self.tick_list.remove(p);
                    m.base.enabled = false;
                }
                _ => {}
            }
        }
    }

    /// `GameObject_SetAnimationEx` (`0x00466800`) with speed 128.
    pub fn set_animation(&mut self, anim: i32, start_ms: i32, reset: bool, now: bool, blend: bool) {
        self.set_animation_speed(anim, start_ms, 128.0, reset, now, blend);
    }

    /// `GameObject_SetAnimationEx` (`0x00466800`): wanted animation, start time and speed (128 = normal); `reset`
    /// forces a restart of the same animation; `now` hands over to the rendering immediately (otherwise in the next
    /// object tick). The rendering never rejects here; the 200 ms lock of the crossfade is reproduced by the
    /// renderer.
    pub fn set_animation_speed(
        &mut self,
        anim: i32,
        start_ms: i32,
        speed: f32,
        reset: bool,
        now: bool,
        blend: bool,
    ) {
        self.anim.speed = speed;
        self.anim.wanted = anim;
        self.anim.start_ms = start_ms;
        if reset {
            self.anim.current = -2;
        }
        if now && self.anim.current != anim {
            self.anim.current = anim;
            self.anim.starts = self.anim.starts.wrapping_add(1);
            self.anim.blend = blend;
        }
    }

    /// Handover of a delayed wanted animation in the object tick (`GameObject_Tick`): crossfades if requested or
    /// allowed.
    pub fn transfer_animation(&mut self) {
        if self.anim.current != self.anim.wanted {
            self.anim.current = self.anim.wanted;
            self.anim.blend = self.anim.blend_request || self.anim.blend_allowed;
            self.anim.blend_request = false;
            self.anim.starts = self.anim.starts.wrapping_add(1);
        }
    }

    /// `Module_SetEnabled` (`0x004FD380`): only on a change; takes effect via the pending list.
    pub fn set_module_enabled(&mut self, index: usize, on: bool) {
        let Some(m) = self.module_mut(index) else {
            return;
        };
        if m.base.enabled != on {
            m.base.enabled = on;
            self.queue_module_tick_change(index, on);
        }
    }
}
