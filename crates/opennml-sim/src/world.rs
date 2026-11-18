//! Create and destroy objects and change the owner like `GameWorld_CreateObject`
//! (`0x00485F70`), `GameWorld_DestroyObject` (`0x00481BB0`) and `GameWorld_ChangeObjectOwner`
//! (`0x00481730`); see re/sim-core 2.

use crate::attributes::AttributeSet;
use crate::collision;
use crate::fx::{Fx, FxVec3, ftol};
use crate::height::terrain_height_fx;
use crate::id::{NATURE_SLOT, NEUTRAL_SLOT, ObjectId, TREE_SLOT};
use crate::message::Message;
use crate::module::{self, Module, has_class};
use crate::object::{GameObject, MODULE_SLOTS};
use crate::path::grid::COST_FREE;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::{Control, SimEvent, SimState};

pub mod hide;
mod kills;

pub use hide::is_hidden;
pub use kills::{add_kill_count, kill_count, kill_count_in, kill_slot};

/// `Player_IsGuidDisabled` (`0x0047DE80`): locked if the script lock
/// (`SimState::button_locks`, action 207) for the GUID is != 0 **or** the level data lock it
/// ([`crate::rules::Rules::is_guid_disabled`]). The script lock applies equally to all slots.
pub fn is_guid_disabled(sim: &Sim, slot: i32, guid: u32) -> bool {
    is_guid_disabled_in(sim.state, sim.rules, slot, guid)
}

/// Read-only version of [`is_guid_disabled`] on state and rules (the bar of the UI holds no `&Sim`):
/// the same computation, no state change.
pub fn is_guid_disabled_in(state: &SimState, rules: &Rules, slot: i32, guid: u32) -> bool {
    state.button_locks.get(&guid).is_some_and(|&v| v != 0) || rules.is_guid_disabled(slot, guid)
}

/// Flags of the creation description (`ObjDesc+0x00`).
pub mod desc_flags {
    pub const GUID: u32 = 0x1;
    pub const POSITION: u32 = 0x2;
    pub const SCALE: u32 = 0x4;
    pub const ROT_X: u32 = 0x8;
    pub const ROT_Y: u32 = 0x10;
    pub const ROT_Z: u32 = 0x20;
    /// Player slot set.
    pub const PLAYER: u32 = 0x40;
    /// Display color set.
    pub const COLOR: u32 = 0x80;
    /// Preplaced (`.wld`).
    pub const PLACED: u32 = 0x100;
    /// Without model and category bits.
    pub const NO_MODEL: u32 = 0x200;
    /// Finished (start, resupply, script objects).
    pub const READY: u32 = 0x800;
    /// Start and resupply objects (`0x8C3`).
    pub const START_OBJECT: u32 = READY | COLOR | PLAYER | POSITION | GUID;
}

/// Description of an object to be created (`ObjDesc`).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectDesc {
    pub flags: u32,
    pub guid: u32,
    pub player: i32,
    pub position: [f32; 3],
    pub scale: [f32; 3],
    pub rotation: [f32; 3],
    pub color: i32,
}

impl ObjectDesc {
    /// Description with type, position and player slot.
    pub fn new(guid: u32, player: i32, position: [f32; 3]) -> Self {
        Self {
            flags: desc_flags::GUID | desc_flags::POSITION | desc_flags::PLAYER,
            guid,
            player,
            position,
            scale: [1.0; 3],
            rotation: [0.0; 3],
            color: player,
        }
    }
}

/// `180/π` of the original (`0x42652EE0`, one ulp below the exact value).
pub const RAD_TO_DEG: f32 = f32::from_bits(0x4265_2EE0);

/// `Fixed_WrapAngleDeg` (`0x00547AF0`): fold to [-180 degrees, 180 degrees).
pub fn wrap_angle(mut a: i32) -> i32 {
    const LO: i32 = -180 << 12;
    const HI: i32 = 180 << 12;
    const RANGE: i32 = HI - LO;
    while a < LO {
        a = a.wrapping_add(RANGE);
    }
    while a >= HI {
        a = a.wrapping_sub(RANGE);
    }
    a
}

/// Greatest depth of the rider translation (the original recurses without a limit, exception A7).
const MAX_TRANSLATION_DEPTH: usize = 8;

/// Types with this Stand class (`MatchesClass(0x102)`, `ONWATER`) stay on the water.
const STAND: usize = 2;
const ONWATER: u32 = 0x100;

/// `GameWorld_CreateObject`: create an object from a description. Rider types are translated into
/// rider and horse (return value: the rider). `None` if the type is missing, the object lies
/// outside the map or the slot table is full (silent in the original, here with a log
/// line).
pub fn create_object(sim: &mut Sim, desc: &mut ObjectDesc) -> Option<ObjectId> {
    create_object_named(sim, desc, None)
}

/// Like [`create_object`] with a script name (`.wld` `OBJ_NAME`, actions 151/217): a non-empty
/// name is entered as soon as the object is in the slot table - before `ApplyPlacement` and
/// thus before the module init (`0x00486414` before `0x0048641C`), so that e.g. the train setup
/// already passes the name of the locomotive on to tender and wagons. For rider types the rider
/// gets `name`, the horse `name + "pferd"` (`0x004860CC`, E-146).
pub fn create_object_named(
    sim: &mut Sim,
    desc: &mut ObjectDesc,
    name: Option<&str>,
) -> Option<ObjectId> {
    create_translated(sim, desc, 0, name.unwrap_or(""))
}

fn create_translated(
    sim: &mut Sim,
    desc: &mut ObjectDesc,
    depth: usize,
    name: &str,
) -> Option<ObjectId> {
    if desc.flags & desc_flags::GUID == 0 {
        return None;
    }
    if let Some(parts) = sim.rules.translations.get(&desc.guid).cloned()
        && !parts.is_empty()
        && depth < MAX_TRANSLATION_DEPTH
    {
        desc.guid = parts[0];
        let rider = create_translated(sim, desc, depth + 1, name)?;
        if let Some(&second) = parts.get(1) {
            desc.guid = second;
            let horse_name = if name.is_empty() {
                String::new()
            } else {
                format!("{name}pferd")
            };
            // Quirk: if the horse fails, the return value is empty although the rider exists.
            let horse = create_translated(sim, desc, depth + 1, &horse_name)?;
            crate::messaging::send(sim, rider, &Message::new(0x40D, &[horse.as_i32()]));
        }
        crate::messaging::send(sim, rider, &Message::new(0x142E, &[1]));
        return Some(rider);
    }
    let ty = sim.rules.types.get(&desc.guid)?;
    let category = ty.category();
    let tile = sim.rules.map.cell;
    // Player slot from the category bits (trees, nature, neutrals) or the description.
    if category & 0x100 != 0 {
        desc.flags |= desc_flags::PLAYER;
        desc.player = i32::from(TREE_SLOT);
        // A tree is created only on a tile that is free in layer 0 (terrain or an earlier
        // placed object blocks; before the boundary check, truncated toward 0).
        let tx = ftol(desc.position[0] / tile[0] as f32);
        let tz = ftol(desc.position[2] / tile[2] as f32);
        if sim.state.path.cell_cost(tx, tz, 0) != COST_FREE {
            return None;
        }
    } else if category & 0x200 != 0 {
        desc.flags |= desc_flags::PLAYER;
        desc.player = i32::from(NATURE_SLOT);
    } else if category & 0x400 != 0 {
        desc.flags |= desc_flags::PLAYER;
        desc.player = i32::from(NEUTRAL_SLOT);
    } else if desc.flags & desc_flags::PLAYER == 0 {
        if !(0..=10).contains(&desc.player) {
            return None;
        }
        desc.flags |= desc_flags::PLAYER;
    } else if !(0..=8).contains(&desc.player) {
        // Quirk: slots 9/10 with the slot flag set become 0.
        desc.player = 0;
    }
    if desc.flags & desc_flags::POSITION == 0 {
        return None;
    }
    // Map boundaries: lower inclusive, upper exclusive, NaN passes (comparisons in f32).
    let [x, _, z] = desc.position;
    let width = sim.rules.map.dim_x.wrapping_mul(tile[0]) as f32;
    let depth_ = sim.rules.map.dim_z.wrapping_mul(tile[2]) as f32;
    if x < 0.0 || x >= width || z < 0.0 || z >= depth_ {
        return None;
    }
    let slot = desc.player as u8;
    let table = &mut sim.state.tables[usize::from(slot)];
    let Some(index) = table.alloc() else {
        log::warn!(
            "Object {} not created: slot table {slot} full (original: silently discarded)",
            desc.guid
        );
        return None;
    };
    let generation = table.generations[index].wrapping_add(1);
    let id = ObjectId::new(index as u16, slot, generation);
    table.count += 1;
    table.generations[index] = generation;
    table.objects[index] = Some(GameObject::new(id, sim.state.tick));
    if !name.is_empty() {
        crate::names::register(&mut sim.state.names, name, id);
    }
    apply_placement(sim, id, desc);
    // `GameWorld_CreateObject` `0x0048648C`: every module 3 object, leaving out the frei tiles.
    if sim.object(id).is_some_and(|o| o.has_module(3, false)) {
        crate::tile_map::add_building(sim, id, true);
    }
    crate::variants::update_variant_mask(sim, id, u32::MAX, 0);
    if desc.flags & (desc_flags::PLACED | desc_flags::READY) != 0 {
        let guid = desc.guid;
        sim.state.add_guid_count(slot, guid, 1);
    }
    // `0x0048644B`: for a hidden slot into the special list (ticked via `L[p]`, entered as
    // otherwise from `ApplyPlacement`), `tick_group = -1`; otherwise into the shorter tick group.
    if sim.state.slot_running[usize::from(slot)] {
        add_to_tick_group(sim, id);
    } else {
        hide::push_inactive(sim.state, usize::from(slot), id);
        if let Some(o) = sim.object_mut(id) {
            o.tick_group = -1;
        }
    }
    sim.state.events.push(SimEvent::Created { id });
    Some(id)
}

/// `GameObject_ApplyPlacement` (`0x00473250`): position, direction, attributes, modules, height,
/// living space and the hit points from the `[CONFIG]` of the `.wld`.
fn apply_placement(sim: &mut Sim, id: ObjectId, desc: &ObjectDesc) {
    let cell = sim.rules.map.cell;
    let rot = |flag: u32, v: f32| if desc.flags & flag != 0 { v } else { 0.0 };
    let rotation = [
        rot(desc_flags::ROT_X, desc.rotation[0]),
        rot(desc_flags::ROT_Y, desc.rotation[1]),
        rot(desc_flags::ROT_Z, desc.rotation[2]),
    ];
    let degrees = rotation[1] * RAD_TO_DEG;
    let direction = wrap_angle(Fx::from_f32(degrees).0.wrapping_sub(90 << 12));
    let grid = world_to_grid(desc.position, cell);
    let owner = id.slot();
    let ai = sim
        .state
        .players
        .get(usize::from(owner))
        .is_some_and(|p| p.control == Control::Ai);
    let rules = sim.rules;
    let attributes = AttributeSet::initial(|a| rules.config_value(owner, desc.guid, a), ai);
    {
        let o = sim.object_mut(id).expect("just created");
        o.guid = desc.guid;
        o.world_pos = desc.position;
        o.scale = if desc.flags & desc_flags::SCALE != 0 {
            desc.scale
        } else {
            [1.0; 3]
        };
        o.rotation = rotation;
        o.direction = Fx(direction);
        o.grid = grid;
        o.tile = grid.tile();
        o.attributes = attributes;
    }
    // Entered twice: before the modules (list A or B only) and after them (also C). The
    // area queries therefore see the object twice (`insert` notifies them each time):
    // the first time it lacks the modules and the filter rejects it; it is admitted the
    // second time, and the `remove` in between removes it everywhere - exactly one entry.
    // Quartering (does not remove) and packing a tent (`tent::on_hide` -> `remove`) behave
    // by the same mechanism: a quartered object stays listed and moves with the
    // carrier, a packed tent leaves all lists.
    collision::insert(sim, id);
    init_from_type(sim, id, desc);
    snap_height_to_terrain(sim, id);
    collision::remove(sim, id);
    collision::insert(sim, id);
    finish_placement(sim, id, desc);
    if let Some(&hp) = sim.state.object_config.get(&14)
        && hp != 0
        && let Some(o) = sim.object_mut(id)
    {
        o.attributes.set_modifier(14, hp, false, 0);
    }
}

/// `GameObject_InitFromType` (`0x00471060`): category, color, per module index ascending the
/// **first** descriptor (`Init` with its values), then take over the activations.
fn init_from_type(sim: &mut Sim, id: ObjectId, desc: &ObjectDesc) {
    let rules = sim.rules;
    let Some(ty) = rules.types.get(&desc.guid) else {
        return;
    };
    {
        let o = sim.object_mut(id).expect("object present");
        if desc.flags & desc_flags::NO_MODEL == 0 {
            o.category = ty.category();
            // `0x0046CA20`: `CANCLEARUP` (`0x10FF`) sets the reconnaissance sight, `NO_BLENDING`
            // (`0x200FF`) forbids the crossfade.
            if o.category & crate::fog::CANCLEARUP != 0 {
                o.clear_up_sight = true;
            }
            o.anim.blend_allowed = o.category & 0x20000 == 0;
        }
        o.color = if desc.flags & desc_flags::COLOR != 0 {
            desc.color
        } else {
            desc.player
        };
    }
    let slot = desc.player.clamp(0, 11) as u8;
    let ready = desc.flags & (desc_flags::PLACED | desc_flags::READY) != 0;
    for n in 0..MODULE_SLOTS {
        if !has_class(n) {
            continue;
        }
        let Some(d) = ty.module(n as u32) else {
            continue;
        };
        {
            let o = sim.object_mut(id).expect("object present");
            let mut m = Module::new(n as u8);
            m.base.prop_bits = d.mask;
            o.modules[n] = Some(m);
        }
        module::init(sim, id, n, d, slot, ready);
        if let Some(o) = sim.object_mut(id) {
            o.queue_module_tick_change(n, true);
        }
    }
    if let Some(o) = sim.object_mut(id) {
        o.apply_module_activation_changes();
    }
}

/// World position from the grid (`GridPos_ToWorldFloat` `0x0042FF70`): `(g · cell) · 2⁻¹²`.
pub fn grid_to_world(grid: FxVec3, cell: [i32; 3]) -> [f32; 3] {
    let f = |g: Fx, c: i32| g.0.wrapping_mul(c) as f32 * (1.0 / 4096.0);
    [f(grid.x, cell[0]), f(grid.y, cell[1]), f(grid.z, cell[2])]
}

/// Grid from the world position (`GridPos_FromWorldFloat`): per axis `ftol(p · 4096) / cell` (division
/// truncates toward zero), also y. Inverse of [`grid_to_world`] up to rounding; the
/// swim tick (re/swim-dive 3.1) and `ApplyPlacement` take this path.
pub fn world_to_grid(p: [f32; 3], cell: [i32; 3]) -> FxVec3 {
    FxVec3::new(
        Fx(Fx::from_f32(p[0]).0 / cell[0]),
        Fx(Fx::from_f32(p[1]).0 / cell[1]),
        Fx(Fx::from_f32(p[2]).0 / cell[2]),
    )
}

/// `GameObject_SnapHeightToTerrain` (`0x0046F780`): height from the terrain (not from the `.wld`),
/// Stand `ONWATER` at least water height; x/z quantized to the grid in the process. The ray of the
/// height query starts above the **old** height of the object (re/motion 6.3).
pub fn snap_height_to_terrain(sim: &mut Sim, id: ObjectId) {
    let rules = sim.rules;
    let map = rules.map;
    let Some(o) = sim.object_mut(id) else { return };
    let (gx, gz) = (o.grid.x.0, o.grid.z.0);
    if !(0..=map.dim_x << 12).contains(&gx) || !(0..=map.dim_z << 12).contains(&gz) {
        return;
    }
    let mut gy =
        terrain_height_fx(rules, grid_to_world(o.grid, map.cell)).wrapping_add(o.height_offset);
    let on_water = o
        .module(STAND)
        .is_some_and(|m| m.base.enabled && m.base.has_flags(ONWATER));
    if on_water {
        gy = gy.max(Fx::from_f32(map.water_level).0 / map.cell[1]);
    }
    o.grid.y = Fx(gy);
    o.world_pos = grid_to_world(o.grid, map.cell);
}

/// Carried object (`GameObject_GetTargetObject` `0x00466910`) with `CARRIING` set.
fn carried(sim: &Sim, id: ObjectId) -> Option<ObjectId> {
    let o = sim.object(id)?;
    let t = o.target;
    (o.status & crate::object::status::CARRIING != 0 && !t.is_none() && t.0 != 0)
        .then_some(t)
        .filter(|&t| sim.object(t).is_some())
}

/// Carried object on the state (like [`carried`], without `Sim`).
fn carried_in(state: &crate::state::SimState, id: ObjectId) -> Option<ObjectId> {
    let o = state.object(id, false)?;
    let t = o.target;
    (o.status & crate::object::status::CARRIING != 0 && !t.is_none() && t.0 != 0)
        .then_some(t)
        .filter(|&t| state.object(t, false).is_some())
}

/// Savegame check: no `CARRIING` chain runs in a circle (not even an object that carries
/// itself). `set_grid_position` and `set_direction` follow the chain without a limit like the
/// original; in normal play it is at most two members long. Each object is visited at most
/// once.
pub fn carry_chains_are_acyclic(state: &crate::state::SimState) -> bool {
    let mut known = std::collections::BTreeSet::new();
    for start in state.objects().map(|o| o.id) {
        let mut path = std::collections::BTreeSet::new();
        let mut cur = Some(start);
        while let Some(id) = cur {
            if known.contains(&id.0) {
                break;
            }
            if !path.insert(id.0) {
                return false;
            }
            cur = carried_in(state, id);
        }
        known.extend(path);
    }
    true
}

/// `GameObject_SetGridPosition(pos, keepY)` (`0x00470F00`, re/motion 6.1): the tile is clamped
/// to the map, the position is not; on a tile change the object is moved in the collision manager
/// (with the unclamped position); with `keep_y` the old height stays and is set to the terrain
/// again (except with the state bits `0x0100_0000`/`0x0400_0000`). A carried object (`CARRIING`)
/// takes over the full position with its own height.
pub fn set_grid_position(sim: &mut Sim, id: ObjectId, pos: FxVec3, keep_y: bool) {
    let map = sim.rules.map;
    let (mut id, mut pos, mut keep_y) = (id, pos, keep_y);
    loop {
        let Some(o) = sim.object_mut(id) else { return };
        let tz = (pos.z.0 >> 12).max(0).min(map.dim_z - 1);
        let tx = (pos.x.0 >> 12).max(0).min(map.dim_x - 1);
        // Tile change: remove from the old bucket (trees never), enter after setting.
        let moved = (tx, tz) != o.tile && id.slot() != TREE_SLOT;
        if moved {
            collision::remove(sim, id);
        }
        let Some(o) = sim.object_mut(id) else { return };
        if (tx, tz) != o.tile {
            o.tile = (tx, tz);
        }
        let old_y = o.grid.y;
        o.grid = pos;
        if keep_y {
            o.grid.y = old_y;
        }
        let snap = keep_y && o.status & 0x0100_0000 == 0 && o.status & 0x0400_0000 == 0;
        o.world_pos = grid_to_world(o.grid, map.cell);
        if snap {
            snap_height_to_terrain(sim, id);
        }
        if moved {
            collision::insert(sim, id);
        }
        let Some(t) = carried(sim, id) else { return };
        pos = sim.object(id).map_or(pos, |o| o.grid);
        keep_y = true;
        id = t;
    }
}

/// `GameObject_SetDirection` (`0x00468B70`, re/motion 6.2): view direction folded and the
/// presentation rotation `rad(dir + 90°)` (unfolded, `f32` like `FMUL` with pi/180 `0x3C8EFA35`);
/// a carried object rotates with it. The direction offset `+0x168` belongs to SiegeEngine.
pub fn set_direction(sim: &mut Sim, id: ObjectId, dir: i32) {
    let mut id = id;
    loop {
        let Some(o) = sim.object_mut(id) else { return };
        o.direction = Fx(wrap_angle(dir));
        let r = o.direction.0.wrapping_add(90 << 12);
        o.rotation[1] = r as f32 * (1.0 / 4096.0) * f32::from_bits(0x3C8E_FA35);
        let Some(t) = carried(sim, id) else { return };
        id = t;
    }
}

/// `GameWorld_SetObjectEventBit` (`0x00481610`, re/train-multipart 13.2): sets or clears bit `bit`
/// (`1 << bit`, the shift counts like `shl` mod 32) in the event word of the object; a missing
/// entry counts as 0 and is always created in the process. The entry also stays after the
/// object is removed.
pub fn set_object_event_bit(sim: &mut Sim, id: ObjectId, bit: u32, on: bool) {
    let mask = 1u32.wrapping_shl(bit);
    let word = sim.state.object_event_bits.entry(id).or_insert(0);
    if on {
        *word |= mask;
    } else {
        *word &= !mask;
    }
}

/// `GameObject_StartAnimationLock` (`0x00467C60`, command `0x42`, E-211, E-212): abort all modules,
/// request the animation `anim` (speed 128, no restart, not immediately), set `ANIM_LOCKED` and enter
/// the lock time `seconds * 1000` ms (integer, with overflow); `seconds == 0` instead takes
/// time marker 1 of the animation (if it is missing, 0: the status bit then stays set without the lock
/// ever expiring); `seconds < 0` is a permanent lock until [`stop_animation_lock`]. A running lock
/// is overwritten, the message FIFO stays. If the object is missing, nothing happens.
pub fn start_animation_lock(sim: &mut Sim, id: ObjectId, anim: i32, seconds: i32) {
    let Some(guid) = sim.object(id).map(|o| o.guid) else {
        return;
    };
    crate::messaging::abort_all_modules(sim, id);
    let lock_ms = if seconds == 0 {
        sim.rules.anim_label_time_at(guid, anim, 1)
    } else {
        seconds.wrapping_mul(1000)
    };
    if let Some(o) = sim.object_mut(id) {
        o.set_animation_speed(anim, 0, 128.0, false, false, false);
        o.set_status_flags(crate::object::status::ANIM_LOCKED, true);
        o.anim_lock_ms = lock_ms;
    }
}

/// `GameObject_StopAnimationLock` (`0x00467CC0`, command `0x43`, E-213): lock time 0 and `ANIM_LOCKED`
/// off - even if another setter (trance, banishing, ghost, assassin, headhunter) set the shared
/// bit, and even without a running lock. The animation stays, no event bit 3 (E-186).
pub fn stop_animation_lock(sim: &mut Sim, id: ObjectId) {
    if let Some(o) = sim.object_mut(id) {
        o.anim_lock_ms = 0;
        o.set_status_flags(crate::object::status::ANIM_LOCKED, false);
    }
}

/// Event word of the object (0 without an entry).
pub fn object_event_bits(sim: &Sim, id: ObjectId) -> u32 {
    sim.state.object_event_bits.get(&id).copied().unwrap_or(0)
}

/// `FUN_0047BA50(p, mode)` (shared for command `0x69` and the script actions 203/204): only for
/// `slot ∈ 0…7` and `mode ∈ 0…4`. For each object of the slot table (table order)
/// first the walk behavior `0x2100`, **then** the attack mode `0x611` (`0x0047BAE0`...`0x0047BAF3`);
/// with control kind != human and mode 3 or 2 additionally the AI attack mode `0x611 {4, 3, 2|1, 0}`.
/// The caller stores the mode itself (`combat.behavior_mode`, `FUN_00478B20`). Buildings get
/// the same messages (no `0x3503`, E-100).
pub fn apply_player_behavior(sim: &mut Sim, slot: i32, mode: i32) {
    if !(0..8).contains(&slot) || !(0..=4).contains(&mode) {
        return;
    }
    let walk = crate::module::quarter::walk_behavior_msg(mode, false);
    let attack = crate::module::quarter::attack_mode_msg(mode, false);
    for id in crate::upgrades::objects_of_slot(sim.state, slot as u8) {
        crate::messaging::send(sim, id, &walk);
        crate::messaging::send(sim, id, &attack);
        let ai = sim
            .state
            .players
            .get(slot as usize)
            .is_some_and(|p| p.control != Control::Human);
        let extra = match mode {
            3 if ai => Some(crate::module::quarter::ai_attack_mode_msg(4, 2)),
            2 if ai => Some(crate::module::quarter::ai_attack_mode_msg(4, 1)),
            _ => None,
        };
        if let Some(m) = extra {
            crate::messaging::send(sim, id, &m);
        }
    }
}

/// `GameObject_FinishPlacement` (`0x00470350`), economy part: preplaced or finished
/// objects of players 0-7 book occupied (22) and provided (27) living space.
fn finish_placement(sim: &mut Sim, id: ObjectId, desc: &ObjectDesc) {
    if desc.flags & (desc_flags::PLACED | desc_flags::READY) == 0 {
        return;
    }
    let owner = id.slot();
    let (used, provided) = (sim.attribute(id, 22), sim.attribute(id, 27));
    if let Some(p) = sim.state.players.get_mut(usize::from(owner)) {
        p.add_resource(3, used);
        p.add_resource(4, provided);
    }
}

/// `GameWorld_AddToTickGroup` (`0x0047F230`): to the shorter of the two groups (tie:
/// group 0); slots 9 and 10 are never ticked.
pub fn add_to_tick_group(sim: &mut Sim, id: ObjectId) {
    let groups = &mut sim.state.tick_groups;
    if groups.iter().any(|g| g.contains(&id)) || id.slot() >= TREE_SLOT {
        return;
    }
    let g = usize::from(groups[1].len() < groups[0].len());
    groups[g].push(id);
    if let Some(o) = sim.object_mut(id) {
        o.tick_group = g as i8;
    }
}

/// `GameWorld_RemoveFromTickGroup` (`0x0047F2B0`).
pub(crate) fn remove_from_tick_group(sim: &mut Sim, id: ObjectId) {
    for g in &mut sim.state.tick_groups {
        g.retain(|&x| x != id);
    }
}

/// `GameWorld_AddExtraTickObject` (`0x0047E460`): at the end of the extra tick list, if not yet
/// in it.
pub fn add_extra_tick(sim: &mut Sim, id: ObjectId) {
    if !sim.state.extra_tick.contains(&id) {
        sim.state.extra_tick.push(id);
    }
}

/// `GameWorld_RemoveExtraTickObject` (`0x0047E490`): remove the entry if present.
pub fn remove_extra_tick(sim: &mut Sim, id: ObjectId) {
    if let Some(p) = sim.state.extra_tick.iter().position(|&x| x == id) {
        sim.state.extra_tick.remove(p);
    }
}

/// `GameObject_GetRegionIdAt` (`0x00466B30`): region in the path layer `layer` (negative: the layer of the
/// walk module, query `0x1400`; without walk -1 -> `0xFFFF`) at the grid position `(x, z)`
/// (fixed point; negative: its own). `PathPlanner_GetRegionId` outside -> `0xFFFF`.
pub fn region_id_at(sim: &mut Sim, id: ObjectId, layer: i32, x: i32, z: i32) -> u16 {
    let layer = if layer < 0 {
        crate::messaging::send(sim, id, &Message::new(0x1400, &[]))
    } else {
        layer
    };
    let (gx, gz) = sim.object(id).map_or((0, 0), |o| (o.grid.x.0, o.grid.z.0));
    let x = if x < 0 { gx } else { x };
    let z = if z < 0 { gz } else { z };
    match usize::try_from(layer) {
        Ok(l) => sim.state.path.region_id(l, x >> 12, z >> 12),
        Err(_) => crate::path::BLOCKED,
    }
}

/// `GameWorld_QueueObjectDestroy` (`0x0047C7D0`): without a duplicate check.
pub fn queue_destroy(sim: &mut Sim, id: ObjectId) {
    if !id.is_none() {
        sim.state.destroy_queue.push(id);
    }
}

/// `GameWorld_ProcessDestroyQueue` (`0x00481DB0`): also entries queued during the pass; duplicates are
/// harmless (the second resolution fails).
pub fn process_destroy_queue(sim: &mut Sim) {
    while !sim.state.destroy_queue.is_empty() {
        let id = sim.state.destroy_queue.remove(0);
        destroy_object(sim, id);
    }
}

/// `GameWorld_DestroyObject` (`0x00481BB0`): from the tick groups, abort all modules,
/// type counter -1 (also without a previous +1), table entry free; the generation stays.
pub fn destroy_object(sim: &mut Sim, id: ObjectId) {
    let Some(guid) = sim.object(id).map(|o| o.guid) else {
        return;
    };
    remove_from_tick_group(sim, id);
    crate::messaging::abort_all_modules(sim, id);
    sim.state.add_guid_count(id.slot(), guid, -1);
    for n in 0..MODULE_SLOTS {
        if sim.module(id, n).is_some() {
            module::destruct(sim, id, n);
        }
    }
    // `GameWorld_DestroyObject` -> `BuildingTileMap_RemoveObjectId`; only buildings are stamped.
    if sim.object(id).is_some_and(|o| o.has_module(3, true)) {
        sim.state.tile_map.remove_id(id.as_i32());
    }
    collision::remove(sim, id);
    let table = &mut sim.state.tables[usize::from(id.slot())];
    table.objects[id.index()] = None;
    table.count -= 1;
    sim.state.events.push(SimEvent::Destroyed { id });
}

/// `GameWorld_QueueOwnerChange` (`0x0047D890`): only if the slot changes.
pub fn queue_owner_change(sim: &mut Sim, id: ObjectId, slot: u8) {
    if id.slot() != slot {
        sim.state.owner_changes.push((id, slot));
    }
}

/// `GameWorld_ProcessOwnerChangeQueue` (`0x00481DE0`).
pub fn process_owner_changes(sim: &mut Sim) {
    let changes = std::mem::take(&mut sim.state.owner_changes);
    for (id, slot) in changes {
        change_owner(sim, id, slot);
    }
}

/// `GameWorld_ChangeObjectOwner` (`0x00481730`): new ID in the target slot, tick group by the
/// rule "shorter list", rebook living space and type counter, notify modules; only the
/// connected object (`+0x2CC`) learns the new ID. A full target slot aborts (a memory error in
/// the original, exception A7). Return value: new ID.
pub fn change_owner(sim: &mut Sim, id: ObjectId, slot: u8) -> Option<ObjectId> {
    let old_slot = id.slot();
    sim.object(id)?;
    if old_slot == slot || usize::from(slot) >= sim.state.tables.len() {
        return None;
    }
    let Some(index) = sim.state.tables[usize::from(slot)].alloc() else {
        log::warn!("Change of owner of {id:?} to {slot}: slot table full, aborted");
        return None;
    };
    let mut object = sim.state.tables[usize::from(old_slot)].objects[id.index()]
        .take()
        .expect("present");
    sim.state.tables[usize::from(old_slot)].count -= 1;
    let table = &mut sim.state.tables[usize::from(slot)];
    table.generations[index] = table.generations[index].wrapping_add(1);
    let new_id = ObjectId::new(index as u16, slot, table.generations[index]);
    object.id = new_id;
    table.objects[index] = Some(object);
    table.count += 1;
    sim.state.collision.replace(id, new_id);
    // Area queries: the original holds pointers; the reimplementation replaces the ID at the same place.
    sim.state.area_queries.replace(id, new_id);
    sim.state.tile_map.replace_id(id.as_i32(), new_id.as_i32());
    // `0x0048185A` onward: the name moves to the new ID (on death, slot 8, deleted), before
    // rebooking the tick group.
    crate::names::owner_changed(&mut sim.state.names, id, new_id, slot);
    // `GameWorld_ChangeObjectOwner` (`0x00481730`): hidden old slot -> out of the special list (the object
    // can no longer be found under the old ID: no removal), otherwise out of the tick group; hidden new slot -> into
    // the special list without entering, otherwise into the tick group. The occupancy thus stays as it was
    // (E-198).
    if sim.state.slot_running[usize::from(old_slot)] {
        remove_from_tick_group(sim, id);
    } else {
        hide::remove_inactive(sim, id);
    }
    if sim.state.slot_running[usize::from(slot)] {
        add_to_tick_group(sim, new_id);
    } else {
        hide::add_inactive(sim, new_id, true);
    }
    on_owner_changed(sim, id, new_id);
    // `GameWorld_ChangeObjectOwner` `0x0048198E`: only `notify_area_queries_insert` (no removal);
    // the new owner can take the object out of the filter (it then stays listed, K-296) or
    // admit it only now.
    crate::area::notify_insert(sim, new_id);
    let guid = sim.object(new_id).map_or(0, |o| o.guid);
    sim.state.add_guid_count(old_slot, guid, -1);
    sim.state.add_guid_count(slot, guid, 1);
    let target = sim.object(new_id).map_or(ObjectId::NONE, |o| o.target);
    if let Some(t) = target_object(sim, target) {
        t.target = new_id;
    }
    sim.state.events.push(SimEvent::OwnerChanged {
        old: id,
        new: new_id,
    });
    Some(new_id)
}

/// `GameObject_GetTargetObject` (`0x00466910`): `+0x2CC ∉ {−1, 0}`.
fn target_object<'s>(sim: &'s mut Sim, target: ObjectId) -> Option<&'s mut GameObject> {
    if target.is_none() || target.0 == 0 {
        return None;
    }
    sim.object_mut(target)
}

/// `GameObject_OnOwnerChanged` (`0x0046F850`): rebook living space, color of the new player,
/// all modules (slot 5), stored message `+0x164` to `+0x160`.
fn on_owner_changed(sim: &mut Sim, old: ObjectId, new: ObjectId) {
    let (used, provided) = (sim.attribute(new, 22), sim.attribute(new, 27));
    for (slot, sign) in [(old.slot(), -1), (new.slot(), 1)] {
        if let Some(p) = sim.state.players.get_mut(usize::from(slot)) {
            p.add_resource(3, sign * used);
            p.add_resource(4, sign * provided);
        }
    }
    let color = sim
        .state
        .players
        .get(usize::from(new.slot()))
        .map_or(8, |p| p.color);
    if let Some(o) = sim.object_mut(new) {
        o.color = color;
    }
    for n in 0..MODULE_SLOTS {
        if sim.module(new, n).is_some() {
            module::on_owner_changed(sim, new, n, old);
        }
    }
    let Some(o) = sim.object_mut(new) else { return };
    if o.owner_change_target == old {
        o.owner_change_target = new;
    }
    let (target, msg) = (o.owner_change_target, o.owner_change_msg);
    if !target.is_none() && target.0 != 0 {
        o.owner_change_target = ObjectId::NONE;
        o.owner_change_msg = 0;
        crate::messaging::send(sim, target, &Message::new(msg as u32, &[new.as_i32()]));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::names::ObjectNames;
    use crate::rules::Rules;
    use crate::state::SimState;

    const TYPES: &str = "GUID 1100\nNAME SPA_Kolonist\nVALUEID 100 6 1 33558528\nPROPID 0\nVALUEID 10003 6 1 2048\nPROPID 2 6\nVALUEID 30001 1 1 0\nVALUEID 30003 6 1 0\nPROPID 2 7\nVALUEID 30001 1 1 18\nGUID 3602\nNAME Laubbaum\nVALUEID 100 6 1 33816832\nPROPID 4\nGUID 3700\nNAME Stein\nVALUEID 100 6 1 512\nPROPID 5\nGUID 3409\nNAME Pferd\nVALUEID 100 6 1 33554432\nPROPID 0\nGUID 1000\nNAME HQ\nVALUEID 100 6 1 0\nPROPID 3 2\nVALUEID 40003 6 1 800768\nPROPID 3 1\nVALUEID 40003 6 1 276480\n";

    fn rules() -> Rules {
        let mut r = Rules::synthetic(TYPES, 16);
        r.translations.insert(1101, vec![1100, 3409]);
        r.defaults
            .insert(1100, [(14, 120), (22, 1)].into_iter().collect());
        r.defaults
            .insert(1000, [(14, 3000), (27, 10)].into_iter().collect());
        r
    }

    #[test]
    fn objects_get_ids_slots_grid_positions_and_directions() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let mut desc = ObjectDesc::new(1100, 2, [1000.0, 55.0, 300.0]);
        desc.flags |= desc_flags::ROT_Y | desc_flags::PLACED;
        desc.rotation[1] = std::f32::consts::FRAC_PI_2;
        let id = create_object(&mut sim, &mut desc).unwrap();
        assert_eq!((id.slot(), id.index(), id.generation()), (2, 0, 1));
        let o = sim.object(id).unwrap();
        // Grid = trunc(x * 4096) / 128; height from the terrain (0), not from the .wld.
        assert_eq!(o.grid, FxVec3::new(Fx(32000), Fx(0), Fx(9600)));
        assert_eq!(o.tile, (7, 2));
        assert_eq!(o.world_pos, [1000.0, 0.0, 300.0]);
        // 90 degrees - 90 degrees = 0 (the constant is one ulp below 180/pi: 89.99999...).
        assert!((-1..=0).contains(&o.direction.0));
        assert_eq!(o.attributes.current_value(14, 0), 120);
        assert_eq!(o.tick_group, 0);
        // First descriptor of each index: Stand with mask 1 << 6.
        assert_eq!(o.module(2).unwrap().base.prop_bits, 1 << 6);
        assert_eq!(o.tick_list, [0, 2]);
        // Preplaced: type counter and occupied living space.
        assert_eq!(sim.state.guid_counts[2][&1100], 1);
        assert_eq!(sim.state.resource(2, 3), 1);
        // Trees, nature: own slots, never ticked.
        let tree =
            create_object(&mut sim, &mut ObjectDesc::new(3602, 4, [64.0, 0.0, 64.0])).unwrap();
        let stone =
            create_object(&mut sim, &mut ObjectDesc::new(3700, 0, [64.0, 0.0, 64.0])).unwrap();
        assert_eq!((tree.slot(), stone.slot()), (9, 10));
        assert_eq!(
            sim.state.tick_groups[0].len() + sim.state.tick_groups[1].len(),
            1
        );
        // Second player object: shorter group 1.
        let second =
            create_object(&mut sim, &mut ObjectDesc::new(1000, 2, [10.0, 0.0, 10.0])).unwrap();
        assert_eq!(sim.object(second).unwrap().tick_group, 1);
        // An object with category 0x400 lands on slot 8; unknown types do not exist.
        assert!(create_object(&mut sim, &mut ObjectDesc::new(4711, 0, [1.0, 0.0, 1.0])).is_none());
    }

    #[test]
    fn module_inits_set_render_bits_and_nature_is_not_selectable() {
        // Fisher-basket-like: Stand with flag 0x200, resource without 0x400 -> invisible (0).
        let types = format!(
            "{TYPES}GUID 3505\nNAME Korb\nVALUEID 100 6 1 33554432\nPROPID 2\nVALUEID 30003 6 1 768\nPROPID 6\nVALUEID 70002 6 1 0\nGUID 3500\nNAME Gold\nVALUEID 100 6 1 512\nPROPID 6\nVALUEID 70002 6 1 1280\n"
        );
        let rules = Rules::synthetic(&types, 16);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let mut make = |guid, x: f32| {
            create_object(&mut sim, &mut ObjectDesc::new(guid, 0, [x, 0.0, 300.0])).unwrap()
        };
        let ids = [
            make(1100, 100.0),
            make(3602, 500.0),
            make(3700, 900.0),
            make(1000, 1300.0),
            make(3505, 1700.0),
            make(3500, 1900.0),
        ];
        let o = |i: usize| sim.object(ids[i]).unwrap();
        let bits: Vec<u32> = (0..ids.len()).map(|i| o(i).render_bits).collect();
        // Without the placement flag the building is a construction site: `StartBuild` sets 4.
        assert_eq!(bits, [3, 3, 3, 4, 0, 3]);
        let selectable: Vec<bool> = (0..ids.len()).map(|i| o(i).selectable).collect();
        assert_eq!(selectable, [true, true, false, true, true, true]);
        // Construction progress: constructor 0, only the construction start of the site sets 0.1.
        let scale: Vec<f32> = (0..ids.len()).map(|i| o(i).progress_scale).collect();
        assert_eq!(
            scale,
            [0.0, 0.0, 0.0, f32::from_bits(0x3DCC_CCCD), 0.0, 0.0]
        );
    }

    #[test]
    fn map_bounds_are_exclusive_at_the_top_and_riders_become_two_objects() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let at = |x: f32| ObjectDesc::new(1100, 0, [x, 0.0, 10.0]);
        assert!(create_object(&mut sim, &mut at(2048.0)).is_none());
        assert!(create_object(&mut sim, &mut at(2047.9)).is_some());
        assert!(create_object(&mut sim, &mut at(-0.1)).is_none());
        let rider =
            create_object(&mut sim, &mut ObjectDesc::new(1101, 1, [50.0, 0.0, 50.0])).unwrap();
        assert_eq!(sim.object(rider).unwrap().guid, 1100);
        assert_eq!(sim.state.tables[1].count, 2);
        assert_eq!(sim.state.events.len(), 3);
    }

    #[test]
    fn destroy_and_owner_change_keep_generations_and_move_counters() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let mut desc = ObjectDesc::new(1000, 0, [100.0, 0.0, 100.0]);
        desc.flags |= desc_flags::READY;
        let hq = create_object(&mut sim, &mut desc).unwrap();
        assert_eq!(sim.state.resource(0, 4), 10);
        queue_owner_change(&mut sim, hq, 0);
        assert!(sim.state.owner_changes.is_empty());
        queue_owner_change(&mut sim, hq, 3);
        process_owner_changes(&mut sim);
        let new = ObjectId::new(0, 3, 1);
        assert!(sim.object(hq).is_none());
        assert_eq!(sim.object(new).unwrap().color, 3);
        assert_eq!(
            (sim.state.resource(0, 4), sim.state.resource(3, 4)),
            (0, 10)
        );
        assert_eq!(sim.state.guid_counts[3][&1000], 1);
        assert_eq!(sim.state.guid_counts[0][&1000], 0);
        queue_destroy(&mut sim, new);
        queue_destroy(&mut sim, new);
        process_destroy_queue(&mut sim);
        assert!(sim.object(new).is_none());
        assert_eq!(sim.state.tables[3].count, 0);
        assert_eq!(sim.state.guid_counts[3][&1000], 0);
        // The index is reused with the next generation; the old ID stays dead.
        let again =
            create_object(&mut sim, &mut ObjectDesc::new(1000, 3, [1.0, 0.0, 1.0])).unwrap();
        assert_eq!((again.index(), again.generation()), (0, 2));
        assert!(sim.object(new).is_none());
        assert!(sim.state.tick_groups.iter().all(|g| !g.contains(&new)));
    }

    #[test]
    fn positions_clamp_the_tile_snap_the_height_and_carry_objects_along() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let a =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [100.0, 0.0, 100.0])).unwrap();
        let b =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [900.0, 0.0, 900.0])).unwrap();
        // Outside the map: tile clamped, position not, height not recomputed (outside).
        let p = FxVec3::new(Fx(-5000), Fx(77), Fx(17 << 12));
        set_grid_position(&mut sim, a, p, false);
        let o = sim.object(a).unwrap();
        assert_eq!((o.tile, o.grid), ((0, 15), p));
        // With keepY: old height, then onto the terrain (0); presentation from the grid.
        let q = FxVec3::new(Fx(3 << 12), Fx(999), Fx(4 << 12));
        set_grid_position(&mut sim, a, q, true);
        let o = sim.object(a).unwrap();
        assert_eq!(
            (o.tile, o.grid.y, o.world_pos),
            ((3, 4), Fx(0), [384.0, 0.0, 512.0])
        );
        // Carried: b follows a with position and direction.
        {
            let o = sim.object_mut(a).unwrap();
            o.target = b;
            o.status |= crate::object::status::CARRIING;
        }
        set_grid_position(
            &mut sim,
            a,
            FxVec3::new(Fx(5 << 12), Fx(0), Fx(6 << 12)),
            true,
        );
        assert_eq!(sim.object(b).unwrap().tile, (5, 6));
        set_direction(&mut sim, a, 270 << 12);
        let (oa, ob) = (sim.object(a).unwrap(), sim.object(b).unwrap());
        assert_eq!((oa.direction, ob.direction), (Fx(-90 << 12), Fx(-90 << 12)));
        assert_eq!(oa.rotation[1], 0.0);
        set_direction(&mut sim, a, 90 << 12);
        assert_eq!(
            sim.object(a).unwrap().rotation[1].to_bits(),
            std::f32::consts::PI.to_bits()
        );
    }

    fn named(sim: &Sim, name: &str) -> Option<String> {
        sim.state.names.get(name).map(str::to_owned)
    }

    // QUIRK E-146: kept
    #[test]
    fn mounted_type_registers_pferd() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let mut desc = ObjectDesc::new(1101, 1, [50.0, 0.0, 50.0]);
        let rider = create_object_named(&mut sim, &mut desc, Some("R_Dragoner1")).unwrap();
        let horse = sim
            .state
            .objects()
            .find(|o| o.guid == 3409)
            .expect("horse")
            .id;
        assert_eq!(named(&sim, "r_dragoner1"), Some(rider.as_i32().to_string()));
        assert_eq!(
            named(&sim, "r_dragoner1pferd"),
            Some(horse.as_i32().to_string())
        );
        // Without and with an empty name: nothing entered.
        create_object(&mut sim, &mut ObjectDesc::new(1101, 1, [90.0, 0.0, 50.0])).unwrap();
        create_object_named(
            &mut sim,
            &mut ObjectDesc::new(1100, 1, [90.0, 0.0, 90.0]),
            Some(""),
        )
        .unwrap();
        assert_eq!(sim.state.names.len(), 2);
    }

    // QUIRK E-145: kept
    #[test]
    fn owner_change_moves_the_name_death_removes_it() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let make = |sim: &mut Sim, name: &str, x: f32| {
            create_object_named(
                sim,
                &mut ObjectDesc::new(1100, 0, [x, 0.0, 50.0]),
                Some(name),
            )
            .unwrap()
        };
        let a = make(&mut sim, "Held", 50.0);
        let b = make(&mut sim, "Opfer", 90.0);
        let c = make(&mut sim, "Mitglied", 130.0);
        // A group "c;a" (like 393/394): on a change only the first ID counts.
        sim.state
            .names
            .set("trupp", format!("{};{}", c.as_i32(), a.as_i32()));
        // Change to slot 2: the name moves to the new ID.
        let a2 = change_owner(&mut sim, a, 2).unwrap();
        assert_eq!(named(&sim, "held"), Some(a2.as_i32().to_string()));
        // Death (slot 8): name deleted.
        change_owner(&mut sim, b, 8).unwrap();
        assert_eq!(named(&sim, "opfer"), None);
        // The backward search finds "trupp" before "mitglied" (bucket 0 before 4): the group shrinks
        // to the new first ID, "mitglied" stays with the old one.
        assert!(ObjectNames::bucket_of("trupp") < ObjectNames::bucket_of("mitglied"));
        let c2 = change_owner(&mut sim, c, 3).unwrap();
        assert_eq!(named(&sim, "trupp"), Some(c2.as_i32().to_string()));
        assert_eq!(named(&sim, "mitglied"), Some(c.as_i32().to_string()));
    }

    // QUIRK E-145: kept
    #[test]
    fn removal_without_death_keeps_the_stale_id() {
        let rules = rules();
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let id = create_object_named(
            &mut sim,
            &mut ObjectDesc::new(1100, 0, [50.0, 0.0, 50.0]),
            Some("Bote"),
        )
        .unwrap();
        queue_destroy(&mut sim, id);
        process_destroy_queue(&mut sim);
        assert!(sim.object(id).is_none());
        assert_eq!(named(&sim, "bote"), Some(id.as_i32().to_string()));
        assert_eq!(crate::names::first_id(&sim.state.names, "Bote"), Some(id));
    }

    #[test]
    fn angles_wrap_into_minus_180_to_180() {
        assert_eq!(wrap_angle(180 << 12), -180 << 12);
        assert_eq!(wrap_angle(-181 << 12), 179 << 12);
        assert_eq!(wrap_angle(725 << 12), 5 << 12);
        assert_eq!(
            RAD_TO_DEG.to_bits() + 1,
            ((180.0f64 / std::f64::consts::PI) as f32).to_bits()
        );
    }

    // QUIRK E-137: kept
    #[test]
    fn kill_count_sums_guid_groups() {
        // `FUN_0047E7F0`: group = sum of the members (recursive), the group's own entry
        // does not count; slot outside 0-7 reads 0, writes nothing.
        let mut rules = rules();
        rules.guid_groups = BTreeMap::from([(2008, vec![1100, 3409]), (2009, vec![2008, 1000])]);
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        for (slot, guid) in [
            (2, 1100),
            (2, 1100),
            (2, 3409),
            (2, 1000),
            (2, 2008),
            (3, 1100),
        ] {
            add_kill_count(&mut sim, slot, guid);
        }
        for slot in [-1, 8, 10, 31] {
            add_kill_count(&mut sim, slot, 1100);
        }
        assert_eq!(sim.state.kill_counts[2][&1100], 2);
        assert_eq!(
            sim.state.kill_counts[2][&2008], 1,
            "entry of the group GUID"
        );
        assert_eq!(kill_count(&sim, 2, 1100), 2);
        assert_eq!(kill_count(&sim, 2, 2008), 3, "1100 + 3409, own entry not");
        assert_eq!(kill_count(&sim, 2, 2009), 4, "2008 + 1000");
        assert_eq!(kill_count(&sim, 3, 2008), 1);
        assert_eq!(kill_count(&sim, 2, 4711), 0);
        for slot in [-1, 8, 31] {
            assert_eq!(kill_count(&sim, slot, 1100), 0);
        }
        assert!(sim.state.kill_counts.iter().skip(4).all(BTreeMap::is_empty));
        assert_eq!(kill_slot(ObjectId::NONE), 31);
        assert_eq!(kill_slot(ObjectId::new(5, 2, 1)), 2);
    }

    thread_local! {
        /// `(object, message)` of the codes `0x2100`/`0x611` during [`behavior_log`].
        static BEHAVIOR: std::cell::RefCell<Vec<(ObjectId, Message)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record_behavior(_: &mut Sim, id: ObjectId, _: usize, m: &Message) {
        if m.code == 0x2100 || m.code == 0x611 {
            BEHAVIOR.with(|b| b.borrow_mut().push((id, m.clone())));
        }
    }

    fn behavior_log(f: impl FnOnce()) -> Vec<(ObjectId, Message)> {
        BEHAVIOR.with(|b| b.borrow_mut().clear());
        crate::module::test_hook::observe(record_behavior, f);
        BEHAVIOR.with(|b| std::mem::take(&mut *b.borrow_mut()))
    }

    // QUIRK E-100: kept
    #[test]
    fn player_behavior_sends_2100_before_611() {
        // `FUN_0047BA50` (`0x0047BAE0`...`0x0047BAF3`): per object first `0x2100`, then `0x611`, with
        // control kind != human and mode 3 last the AI attack mode `{4, 3, 2, 0}`; the command `0x69`
        // takes the same path.
        use crate::combat::testkit as ck;
        use crate::command::{Command, CommandKind, Outcome, execute};
        let rules = ck::rules();
        let mut state = ck::state(&rules);
        state.players[0].control = Control::Human;
        state.players[1].control = Control::Ai;
        let mut sim = Sim::new(&mut state, &rules);
        let a = ck::spawn(&mut sim, ck::SABRE, 1, (10, 10));
        let b = ck::spawn(&mut sim, ck::SABRE, 1, (14, 10));
        let h = ck::spawn(&mut sim, ck::SABRE, 0, (18, 10));
        use crate::module::quarter::{ai_attack_mode_msg, attack_mode_msg, walk_behavior_msg};
        let expected = |ids: &[ObjectId], ai: bool| {
            let mut v = Vec::new();
            for &id in ids {
                v.push((id, walk_behavior_msg(3, false)));
                v.push((id, attack_mode_msg(3, false)));
                if ai {
                    v.push((id, ai_attack_mode_msg(4, 2)));
                }
            }
            v
        };
        let log = behavior_log(|| apply_player_behavior(&mut sim, 1, 3));
        assert_eq!(log, expected(&[a, b], true));
        let log = behavior_log(|| apply_player_behavior(&mut sim, 0, 3));
        assert_eq!(
            log,
            expected(&[h], false),
            "human slot: without AI addition"
        );
        // Slot or mode outside the limits: nothing.
        for (slot, mode) in [(8, 3), (-1, 3), (1, 5), (1, -1)] {
            assert!(behavior_log(|| apply_player_behavior(&mut sim, slot, mode)).is_empty());
        }
        // Command `0x69` (`GameWorld_CmdAiSetPlayerBehavior` `0x0047BC70`): store the mode, then
        // the same sequence.
        let cmd = Command {
            player: 1,
            kind: CommandKind::AiPlayerBehavior { slot: 1, mode: 3 },
        };
        let log = behavior_log(|| assert_eq!(execute(&mut sim, &cmd, true), Outcome::Accepted));
        assert_eq!(log, expected(&[a, b], true));
        assert_eq!(sim.state.combat.behavior_mode[1], 3);
    }
}
