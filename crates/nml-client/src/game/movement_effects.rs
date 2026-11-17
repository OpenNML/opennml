//! Movement effects of the units (`Unit_UpdateMovementEffects` `0x0053CEE0`,
//! re/world-display W.3): footprints and hoof prints on sand and snow, wheel dust and
//! wheel tracks of the wagons, water ring and wading sound in shallow water.
//!
//! In the original, `PropertyWalk_Tick` (`0x0053E120`) calls the function in the object tick of the simulation
//! when `ParticlesQuality != 0` and writes walk fields that are part of the savegame (D-270). The
//! reimplementation keeps the counter, side switch, track type and wheel dust list here in the host: none of it
//! is in the savegame, so the simulation stays untouched. The sync runs
//! like [`crate::game::building_fires::BuildingFires::sync`] once per frame after the rebuild of the
//! objects and catches up on the object groups ticked since the last sync (phase 1 ->
//! group 0, phase 0 -> group 1, alternating, at most 16 ticks); the state is the one at the
//! end of the frame. Positions come from the simulation (`GameObject::world_pos` = `GO+0x08`, grid position
//! for the ring), not from the interpolated presentation. Randomness of the wading sound
//! comes from its own `MsvcRand` (D-273).

use std::collections::{BTreeMap, HashMap};

use glam::Vec3;
use nml_effects::EFFECTS_SEED;
use nml_effects::attached::{Anchor, UNLIMITED_MS};
use nml_effects::marks::{Footprint, SURF_RING_MODEL, SurfRing};
use nml_effects::triggers::numbered_nodes;
use nml_effects::world::{EffectWorld, OwnedKind};
use nml_rda::Vfs;
use nml_render::ModelEntry;
use nml_sim::id::ObjectId;
use nml_sim::module::ModuleKind;
use nml_sim::module::multipart;
use nml_sim::object::{GameObject, status};
use nml_sim::state::SimState;
use nml_sim::world::grid_to_world;
use nml_world::assets::ModelCache;
use nml_world::level::LevelScene;
use nml_world::math::object_world_matrix;
use nml_world::rand::MsvcRand;

use crate::game::notices::SoundRequest;
use crate::game::sim_host::SimHost;

/// Wheel dust (`"data/effects/"` + `"rad_staubwolke.pef"`, `0x00627C8C`/`0x00627BE0`).
pub const WHEEL_DUST: &str = "data/effects/rad_staubwolke.pef";
/// Prefix of the wheel nodes (`sprintf("rad%02d")`).
pub const WHEEL_NODE: &str = "rad";
/// Prefix of the foot nodes (`sprintf("foot%02d")`).
pub const FOOT_NODE: &str = "foot";
/// Flags of the wading sound (`SoundManager_PlayEvent3D(…, 0x11, …)`).
pub const WATERWALK_FLAGS: u32 = 0x11;

/// More ticks since the last sync count as a jump (loading, restart): the sync starts
/// over instead of catching up on ticks.
const MAX_CATCH_UP: u64 = 16;
/// Walk flag `SADDLE` (mount).
const WALK_SADDLE: u32 = 0x8000;
/// Walk flag `NOTHUMAN` (`0x20000`): ships, boats, cannons, travois, stagecoach.
const WALK_NOTHUMAN: u32 = 0x2_0000;
/// Animal module.
const ANIMAL: usize = 0x14;
/// Ghost module.
const GHOST: usize = 0x1B;
/// Status bit `0x10`: does not count as movement (`moving` = `MOVE` without `0x10`).
const NOT_MOVING: u32 = 0x10;
/// Status bit `0x4000000`: siege crew (bit B of module `0x2E` SiegeEngine, unused in the shipped
/// data; the reimplementation never sets it). Blocks the footprints (`0x0053CEE0`).
const SIEGE_CREW: u32 = 0x400_0000;
/// No effect in the tick (`0x0053CEE0`): dead, quartered, rider, boat occupant.
const EXCLUDED: u32 =
    status::DIED | status::QUARTERED | status::ISMOUNTED | status::ISMOUNTEDONBOAT;

/// Track type of a unit (`0x0053D943`..., re/world-display W.3.2), overwritten in this order:
/// 1 (humans); 6 with walk flag `SADDLE`; 6 with the animal module (`0x14`); `0x802` for
/// an animal with `SADDLE`; 0 with walk flag `0x20000`; 0 with the ghost module (`0x1B`); 6 for GUID 2005;
/// 0 with node `foot01`. `MatchesClass(bits)` checks the flags of the walk module here (even when
/// switched off, `filter.rs`); the tick excludes dead and quartered objects beforehand.
pub fn footprint_kind(o: &GameObject, has_foot_nodes: bool) -> u32 {
    let flags = if o.has_module(0, true) {
        o.module(0).map_or(0, |m| m.base.flags & !0xFF)
    } else {
        0
    };
    let saddle = flags & WALK_SADDLE != 0;
    let animal = o.has_module(ANIMAL, false);
    let mut kind = 1;
    if saddle {
        kind = 6;
    }
    if animal {
        kind = 6;
    }
    if animal && saddle {
        kind = 0x802;
    }
    if flags & WALK_NOTHUMAN != 0 {
        kind = 0;
    }
    if o.has_module(GHOST, false) {
        kind = 0;
    }
    if o.guid == 2005 {
        kind = 6;
    }
    if has_foot_nodes {
        kind = 0;
    }
    kind
}

/// Type of the marks at the foot nodes (track type 0): `0x802` for GUID 1110, 1713, 2611, otherwise 1.
pub fn foot_node_kind(guid: u32) -> u32 {
    if matches!(guid, 1110 | 1713 | 2611) {
        0x802
    } else {
        1
    }
}

/// State of a unit with a walk module (walk fields in the original; only in the host here).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitTrack {
    /// `Walk+0x130`: counter (constructor 0).
    counter: i32,
    /// `Walk+0x13C`: "moving" in the last tick that ran.
    moving: bool,
    /// `Walk+0xC8` `m_LeftRight`: +1 before each track, side = bit 0.
    left_right: i32,
    /// `Walk+0xCC` `m_FootprintType` (-1 = not determined yet).
    footprint_kind: Option<u32>,
    /// `Walk+0x2D4`: keys of the own wheel dust effects (cleared at each halt, D-274).
    dust: Vec<u32>,
}

impl UnitTrack {
    /// Counter check (`0x0053CEE0`): counter -1; if it is still >= 1 and "moving" is unchanged, nothing
    /// runs; otherwise counter 2 and "moving" is taken over. So every second object tick (200 ms) or
    /// immediately on a change.
    pub fn due(&mut self, moving: bool) -> bool {
        self.counter -= 1;
        if self.counter >= 1 && self.moving == moving {
            return false;
        }
        self.counter = 2;
        self.moving = moving;
        true
    }

    pub fn counter(&self) -> i32 {
        self.counter
    }

    pub fn footprint_kind(&self) -> Option<u32> {
        self.footprint_kind
    }

    /// Keys of the running wheel dust effects.
    pub fn dust(&self) -> &[u32] {
        &self.dust
    }
}

/// What the movement effects need from the map ([`crate::game::map::MapView`], a fake
/// in tests).
pub trait MovementWorld {
    /// `Terrain_GetTextureBitsAt` (`0x0042F0F0`) & 1 at the tile `(trunc(x / cell), trunc(z /
    /// cell))`; `false` outside.
    fn tracks_at(&self, x: f32, z: f32) -> bool;
    /// Like [`MovementWorld::tracks_at`] for a tile (`GameObject_GetTilePos`).
    fn tracks_at_tile(&self, x: i32, z: i32) -> bool;
    /// `Terrain_GetHeightAt` (`0x0055A940`).
    fn ground_height(&self, x: f32, z: f32) -> f32;
    /// Water level (`GameWorld+0x198`).
    fn water_level(&self) -> f32;
    /// World positions of the nodes `<prefix>01`, `<prefix>02`, ... of the object up to the first missing one
    /// (`ModelInstance_GetHelperNodePosition`); empty without a model.
    fn node_positions(&self, id: ObjectId, prefix: &str) -> Vec<Vec3>;
    /// Location of the object in the map's object list (carrier of the own effects).
    fn carrier(&self, id: ObjectId) -> Option<usize>;
    /// Effect world with archive and models (wheel dust `.pef`, ring model; the destruction smoke
    /// is also created through it).
    fn assets(&mut self) -> (&mut EffectWorld, &Vfs, &mut ModelCache);
}

/// New `.pef` effect `path` at the node `node` of the object `object` as an own effect
/// (`AttachedEffect_Construct(world, path, 0x7FFFFFFF, 0, 1, 0)` with "keep running invisibly",
/// `AttachToNode`, `GameObject_AddOwnedEffect(e, -1)`); returns the key, `None` without a file.
pub fn add_node_effect(
    world: &mut dyn MovementWorld,
    object: usize,
    node: &str,
    path: &str,
) -> Option<u32> {
    let (effects, vfs, models) = world.assets();
    let effect = effects.new_effect(
        vfs,
        models,
        path,
        UNLIMITED_MS,
        Anchor::Node(node.to_string()),
        true,
    )?;
    Some(effects.add_owned(object, true, OwnedKind::Attached(effect), -1))
}

/// Movement effects of all units of a map.
#[derive(Debug)]
pub struct MovementEffects {
    /// Tick of the simulation up to which the object ticks have been processed.
    steps: u64,
    /// State per unit (ID).
    units: BTreeMap<u32, UnitTrack>,
    /// Wheel track keys per object (ID), one per wheel node; head and trailer separate
    /// (`GameObject+0x7F4`).
    treads: BTreeMap<u32, Vec<u32>>,
    /// Randomness of the wading sound (CRT `rand()` in the original, D-273).
    rng: MsvcRand,
    /// Wading sounds of the ticks since the last [`MovementEffects::take_sounds`].
    sounds: Vec<SoundRequest>,
    /// Objects that received own effects since the last [`MovementEffects::take_new_owners`].
    new_owners: Vec<usize>,
}

impl Default for MovementEffects {
    fn default() -> Self {
        Self {
            steps: 0,
            units: BTreeMap::new(),
            treads: BTreeMap::new(),
            rng: MsvcRand::new(EFFECTS_SEED),
            sounds: Vec::new(),
            new_owners: Vec::new(),
        }
    }
}

impl MovementEffects {
    /// Start over at tick `steps` (loading, restart, change of `ParticlesQuality`): forget counters,
    /// track types and wheel dust lists, release the wheel tracks of the effect world, reset randomness to the
    /// initial value. The caller clears existing tracks together with the effect world (`EffectWorld::clear`).
    pub fn reset(&mut self, effects: &mut EffectWorld, steps: u64) {
        for key in std::mem::take(&mut self.treads).into_values().flatten() {
            effects.release_tread(key);
        }
        self.units.clear();
        self.rng = MsvcRand::new(EFFECTS_SEED);
        self.sounds.clear();
        self.new_owners.clear();
        self.steps = steps;
    }

    /// State of the unit `id`.
    pub fn track(&self, id: ObjectId) -> Option<&UnitTrack> {
        self.units.get(&id.0)
    }

    /// Wheel track keys of the object `id`.
    pub fn tread_keys(&self, id: ObjectId) -> &[u32] {
        self.treads.get(&id.0).map_or(&[], Vec::as_slice)
    }

    /// The wading sounds of the ticks since the last call.
    pub fn take_sounds(&mut self) -> Vec<SoundRequest> {
        std::mem::take(&mut self.sounds)
    }

    /// Locations of the objects that received own effects (wheel dust) since the last call; the
    /// host gives them their rest hull for visibility.
    pub fn take_new_owners(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.new_owners)
    }

    /// Change of owner (`SimEvent::OwnerChanged`): same object under a new ID, state and
    /// wheel tracks move along.
    pub fn rename(&mut self, renamed: &[(u32, u32)]) {
        for &(old, new) in renamed {
            if let Some(t) = self.units.remove(&old) {
                self.units.insert(new, t);
            }
            if let Some(t) = self.treads.remove(&old) {
                self.treads.insert(new, t);
            }
        }
    }

    /// Sync with the simulation after a frame: entries of vanished objects are dropped
    /// (their wheel tracks age out), then the object tick of each unit of the groups that ticked since the
    /// last sync. With `quality == 0` nothing runs, not even a counter
    /// (`PropertyWalk_Tick` does not call the function then).
    pub fn sync(&mut self, sim: &SimHost, quality: u8, world: &mut dyn MovementWorld) {
        let now = sim.state.steps;
        if now < self.steps || now - self.steps > MAX_CATCH_UP {
            self.steps = now;
        }
        self.drop_vanished(sim, world);
        if quality != 0 {
            // After a tick with phase 1, group 0 ticked, with phase 0 group 1; before that the
            // groups alternate.
            let last = usize::from(sim.state.phase != 1);
            for back in (0..now - self.steps).rev() {
                let group = last ^ (back as usize & 1);
                for &id in &sim.state.tick_groups[group] {
                    if let Some(o) = sim.state.object(id, false) {
                        self.tick_object(sim, o, quality, world);
                    }
                }
            }
        }
        self.steps = now;
    }

    fn drop_vanished(&mut self, sim: &SimHost, world: &mut dyn MovementWorld) {
        let exists = |id: u32| sim.state.object(ObjectId(id), false).is_some();
        self.units.retain(|&id, _| exists(id));
        let (effects, _, _) = world.assets();
        self.treads.retain(|&id, keys| {
            if exists(id) {
                return true;
            }
            for &key in keys.iter() {
                effects.release_tread(key);
            }
            false
        });
    }

    /// One object tick (`Unit_UpdateMovementEffects` `0x0053CEE0`) for an object with a walk module
    /// (without an animation lock: otherwise no module tick runs).
    fn tick_object(
        &mut self,
        sim: &SimHost,
        o: &GameObject,
        quality: u8,
        world: &mut dyn MovementWorld,
    ) {
        if !o.has_module(0, false) || o.anim_lock_ms != 0 {
            return;
        }
        let Some(walk) = o
            .module(0)
            .filter(|m| matches!(m.kind, ModuleKind::Walk(_)))
        else {
            return;
        };
        let moving = o.status & status::MOVE != 0 && o.status & NOT_MOVING == 0;
        let mut track = self.units.remove(&o.id.0).unwrap_or_default();
        let started = !track.moving && moving;
        let stopped = track.moving && !moving;
        if track.due(moving) && o.status & EXCLUDED == 0 {
            let pos = Vec3::from(o.world_pos);
            if world.ground_height(pos.x, pos.z) >= world.water_level() {
                if !world.node_positions(o.id, WHEEL_NODE).is_empty() {
                    self.wheels(sim, o, &mut track, started, stopped, quality, world);
                }
                if track.moving
                    && o.status & SIEGE_CREW == 0
                    && o.status & status::ISMOUNTED == 0
                    && world.tracks_at_tile(o.tile.0, o.tile.1)
                {
                    footprints(o, &mut track, pos, quality, world);
                }
            } else if !walk.base.has_flags(WALK_NOTHUMAN) {
                // Ships and boats (`0x20000`) are handled by `ShipStand` (nml-effects).
                self.water(sim, o, &mut track, world);
            }
        }
        self.units.insert(o.id.0, track);
    }

    /// Wheels on land (`0x0053D550…`): wheel dust when starting off at each wheel node (also at the
    /// trailer via MULTIHEAD `0x70B`), stopped when halting (the trailer releases all own
    /// effects), wheel tracks while driving (head and trailer).
    #[allow(clippy::too_many_arguments)]
    fn wheels(
        &mut self,
        sim: &SimHost,
        o: &GameObject,
        track: &mut UnitTrack,
        started: bool,
        stopped: bool,
        quality: u8,
        world: &mut dyn MovementWorld,
    ) {
        let trailer = multipart_trailer(sim, o);
        if started {
            if let Some(index) = world.carrier(o.id) {
                for node in node_names(world, o.id, WHEEL_NODE) {
                    if let Some(key) = add_node_effect(world, index, &node, WHEEL_DUST) {
                        track.dust.push(key);
                        self.new_owners.push(index);
                    }
                }
            }
            if let Some(t) = trailer
                && let Some(index) = world.carrier(t)
            {
                for node in node_names(world, t, WHEEL_NODE) {
                    if add_node_effect(world, index, &node, WHEEL_DUST).is_some() {
                        self.new_owners.push(index);
                    }
                }
            }
        }
        if stopped {
            // List cleared at each halt (D-274); the original lets it grow.
            let dust = std::mem::take(&mut track.dust);
            if let Some(index) = world.carrier(o.id) {
                let (effects, _, _) = world.assets();
                for key in dust {
                    effects.stop_owned(index, key);
                }
            }
            if let Some(t) = trailer
                && let Some(index) = world.carrier(t)
            {
                // `GameObject_ReleaseOwnedEffectsToWorld` (`0x004692D0`): all of them (D-274).
                world.assets().0.release_owned(index);
            }
        }
        if track.moving {
            self.tread_marks(o.id, quality, world);
            if let Some(t) = trailer {
                self.tread_marks(t, quality, world);
            }
        }
    }

    /// `GameObject_UpdateTreadMarks("rad")` (`0x0046E630`): one permanent
    /// `TreadMarkEffect` per wheel node; a point on a tile with `TEX_BITS & 1`, otherwise a gap. Time =
    /// game time of the effects. A wheel track lost in the rebuild is created again.
    fn tread_marks(&mut self, id: ObjectId, quality: u8, world: &mut dyn MovementWorld) {
        let points = world.node_positions(id, WHEEL_NODE);
        let tracks: Vec<bool> = points.iter().map(|p| world.tracks_at(p.x, p.z)).collect();
        let keys = self.treads.entry(id.0).or_default();
        let (effects, _, _) = world.assets();
        let now = effects.now_ms();
        for (i, (&p, &on_track)) in points.iter().zip(&tracks).enumerate() {
            if keys.len() <= i {
                keys.push(effects.add_tread(quality));
            }
            if effects.tread_mut(keys[i]).is_none() {
                keys[i] = effects.add_tread(quality);
            }
            let Some(tread) = effects.tread_mut(keys[i]) else {
                continue;
            };
            if on_track {
                tread.add_point(p, now);
            } else {
                tread.add_gap(now);
            }
        }
    }

    /// Water branch (`0x0053D44F`, without walk flag `0x20000`): when standing, not again until 20 ticks later;
    /// when not diving, a water ring at the grid position at water level in the yaw of the unit;
    /// when not swimming and walking, additionally `SFX_WATERWALK<rand() % 8>` (`0x00553420`, flags `0x11`,
    /// at `GO+0x08`). No fog check and no stealth check (D-269, D-273).
    fn water(
        &mut self,
        sim: &SimHost,
        o: &GameObject,
        track: &mut UnitTrack,
        world: &mut dyn MovementWorld,
    ) {
        if !track.moving {
            track.counter = 20;
        }
        if o.status & status::DIVING != 0 {
            return;
        }
        let grid = grid_to_world(o.grid, sim.rules.map.cell);
        let at = Vec3::new(grid[0], world.water_level(), grid[2]);
        let (effects, vfs, models) = world.assets();
        // No rings without a model (`SurfRing_UpdateVisibility` needs the instance).
        if let Some(model) = models.get(vfs, SURF_RING_MODEL) {
            let bounds = &model.rdo.bounds;
            effects.add_surf_ring(SurfRing::new(
                at,
                o.rotation[1],
                Vec3::from(bounds.center),
                bounds.radius,
            ));
        }
        if o.status & status::SWIMMING == 0 && track.moving {
            self.sounds.push(SoundRequest::Event3d {
                name: format!("SFX_WATERWALK{}", self.rng.rand() % 8),
                flags: WATERWALK_FLAGS,
                position: Vec3::from(o.world_pos),
            });
        }
    }
}

/// Footprints (`0x0053D943`...`0x0053DBC2`): `m_LeftRight` +1 before each track, track type determined
/// once; type != 0 one mark at `GO+0x08` in the yaw, type 0 one per foot node.
fn footprints(
    o: &GameObject,
    track: &mut UnitTrack,
    pos: Vec3,
    quality: u8,
    world: &mut dyn MovementWorld,
) {
    track.left_right = track.left_right.wrapping_add(1);
    let feet = world.node_positions(o.id, FOOT_NODE);
    let kind = *track
        .footprint_kind
        .get_or_insert_with(|| footprint_kind(o, !feet.is_empty()));
    let side = track.left_right & 1;
    let yaw = o.rotation[1];
    let effects = world.assets().0;
    if kind != 0 {
        effects.add_footprint(Footprint::new(pos, yaw, side, kind, quality));
    } else {
        let kind = foot_node_kind(o.guid);
        for p in feet {
            effects.add_footprint(Footprint::new(p, yaw, side, kind, quality));
        }
    }
}

/// Names of the nodes `<prefix>01`, ... belonging to the positions of [`MovementWorld::node_positions`].
fn node_names(world: &dyn MovementWorld, id: ObjectId, prefix: &str) -> Vec<String> {
    (1..=world.node_positions(id, prefix).len())
        .map(|i| format!("{prefix}{i:02}"))
        .collect()
}

/// Per tile (`z * dim_x + x`): does its texture carry `TEX_BITS & 1` ("tracks": sand, snow)?
/// Once per map from the tile list of the `.wld` and the textures of the `.prj` (like
/// `Terrain_GetTextureBitsAt` `0x0042F0F0`).
pub fn track_tiles(scene: &LevelScene) -> Vec<bool> {
    let terrain = &scene.terrain;
    let (dim_x, dim_z) = (terrain.dim_x(), terrain.dim_z());
    (0..dim_z)
        .flat_map(|z| (0..dim_x).map(move |x| (x, z)))
        .map(|(x, z)| {
            terrain
                .tile(x, z)
                .and_then(|guid| scene.project.textures.get(&guid))
                .is_some_and(|t| t.bits & 1 != 0)
        })
        .collect()
}

/// The map for the movement effects ([`crate::game::map::MapView`]): terrain, tile bits,
/// nodes of the drawn models at the position from the simulation, effect world.
pub struct MapMovement<'a> {
    pub scene: &'a LevelScene,
    /// [`track_tiles`] of the map.
    pub track_tiles: &'a [bool],
    pub state: &'a SimState,
    pub entries: &'a [ModelEntry],
    /// Location in the object list -> entry of the renderer.
    pub entry_of: &'a HashMap<usize, usize>,
    /// ID -> location in the object list.
    pub object_of: &'a HashMap<u32, usize>,
    /// Pose clock (`None` = bind pose).
    pub clock_ms: Option<f32>,
    pub effects: &'a mut EffectWorld,
    pub vfs: &'a Vfs,
    pub models: &'a mut ModelCache,
}

impl MovementWorld for MapMovement<'_> {
    fn tracks_at(&self, x: f32, z: f32) -> bool {
        let p = &self.scene.terrain.params;
        // `trunc` like the integer of the grid position; nothing lies left of or above the map.
        if x < 0.0 || z < 0.0 {
            return false;
        }
        self.tracks_at_tile((x / p.size_x) as i32, (z / p.size_z) as i32)
    }

    fn tracks_at_tile(&self, x: i32, z: i32) -> bool {
        let terrain = &self.scene.terrain;
        let (dim_x, dim_z) = (terrain.dim_x() as i32, terrain.dim_z() as i32);
        if !(0..dim_x).contains(&x) || !(0..dim_z).contains(&z) {
            return false;
        }
        self.track_tiles
            .get((z * dim_x + x) as usize)
            .copied()
            .unwrap_or(false)
    }

    fn ground_height(&self, x: f32, z: f32) -> f32 {
        self.scene.terrain.ground_height(x, z)
    }

    fn water_level(&self) -> f32 {
        self.scene.terrain.params.water_level
    }

    /// Nodes in object space from the running pose of the model, into the world with the position and
    /// yaw from the simulation (`GO+0x08`, `rotation[1]`; [`node_frame`]), not with the interpolated
    /// frame position.
    fn node_positions(&self, id: ObjectId, prefix: &str) -> Vec<Vec3> {
        let Some(o) = self.state.object(id, false) else {
            return Vec::new();
        };
        let Some(entry) = self
            .object_of
            .get(&id.0)
            .and_then(|i| self.entry_of.get(i))
            .and_then(|&e| self.entries.get(e))
        else {
            return Vec::new();
        };
        let world = node_frame(o);
        numbered_nodes(&entry.asset, &entry.playback, prefix)
            .iter()
            .filter_map(|name| match self.clock_ms {
                Some(now) => entry.playback.helper_node(&entry.asset, now, name),
                None => entry.playback.bind_helper_node(&entry.asset, name),
            })
            .map(|local| world.transform_point3(local))
            .collect()
    }

    fn carrier(&self, id: ObjectId) -> Option<usize> {
        self.object_of.get(&id.0).copied()
    }

    fn assets(&mut self) -> (&mut EffectWorld, &Vfs, &mut ModelCache) {
        (self.effects, self.vfs, self.models)
    }
}

/// Position of the foot and wheel nodes in the world: `GameObject_GetHelperNodeMapPositionImpl`
/// (`0x0046D190`) takes the node in model space (`ModelInstance_GetHelperNodePosition`
/// `0x0058DAB0`, without instance scale) and rotates it in `GameWorld_LocalOffsetToGridPos`
/// (`0x00479490`) only by the yaw `GO+0x1C0`, then adds the grid position `GO+0x19C…0x1A4`. No
/// tilt, no scale. The yaw addition `GO+0x168` for status `0x5000000` is omitted: the reimplementation
/// never sets those bits.
fn node_frame(o: &GameObject) -> glam::Affine3A {
    object_world_matrix(
        Vec3::from(o.world_pos),
        Vec3::new(0.0, o.rotation[1], 0.0),
        Vec3::ONE,
    )
}

/// Trailer of a front part (`0x70B` -> `m_LinkedHID`), if it is still alive.
fn multipart_trailer(sim: &SimHost, o: &GameObject) -> Option<ObjectId> {
    let ModuleKind::MultiHead(h) = &o.module(multipart::HEAD)?.kind else {
        return None;
    };
    sim.state.object(h.linked, false).map(|t| t.id)
}

#[cfg(test)]
mod tests;
