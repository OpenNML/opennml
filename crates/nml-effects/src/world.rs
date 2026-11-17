//! All effects of a scene, managed as in the original (re/effects): own effects of the
//! objects (map `GameObject+0x178`, drawn in object pass `0x88` after the water) and
//! world effects of the EffectManager (`GameWorld+0x3E0`, layers 1 and 2). Game tick every 100 ms
//! of game time (`GameObject_Tick`, `EffectManager_Tick`), frame update per logic step
//! (`GameWorld_BuildRenderListsAndFrameUpdate` `0x0047FC70`).

use std::collections::BTreeMap;

use glam::{Mat4, Vec3};
use nml_rda::Vfs;
use nml_world::assets::ModelCache;
use nml_world::math::object_world_matrix;
use nml_world::rand::MsvcRand;
use nml_world::visibility::Frustum;

use crate::EFFECTS_SEED;
use crate::ambient::{Ambient, AmbientKind};
use crate::attached::{Anchor, AttachedEffect, Carrier, EffectTarget, TICK_MS, UNLIMITED_MS};
use crate::handle::{HandleEffect, HandleSpawn};
use crate::library::EffectLibrary;
use crate::marks::{Footprint, SurfRing, TreadMark};
use crate::output::{DrawCommand, EffectFrame, GroundDecal, GroundMarks, ModelDraw};
use crate::particles::Placement;

/// Error of [`EffectWorld::spawn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// File missing or not readable (cause in the log).
    Unreadable,
    /// File read, but without effect components (e.g. a `.hef` or `.tga`); the effect
    /// would have silently vanished on the next tick.
    NoComponents,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unreadable => "not loadable",
            Self::NoComponents => "contains no effect components (no .pef?)",
        })
    }
}

impl std::error::Error for SpawnError {}

/// Position of an object in a logic step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectFrame {
    pub position: Vec3,
    /// `OBJ_XROT/YROT/ZROT` (radians).
    pub rotation: Vec3,
    /// In the view frustum (`GameObject_UpdateFrustumVisibility`) and not in the fog of war: is
    /// drawn.
    pub visible: bool,
}

/// What the effects need to know about the objects of the scene.
pub trait ObjectView {
    fn frame(&self, object: usize) -> ObjectFrame;
    /// Helper node in object space for the current pose (`ModelInstance_GetHelperNodePosition`).
    fn node(&self, object: usize, name: &str) -> Option<Vec3>;
    /// Terrain height (HEF points).
    fn height(&self, x: f32, z: f32) -> f32 {
        let _ = (x, z);
        0.0
    }

    /// Fog probe: `QueryFogOfWarArea(tile(pos), viewer, 0)` including the view switch
    /// (re/vision 5.2); without a simulation 7 (everything clear).
    fn fog(&self, position: Vec3) -> u8 {
        let _ = position;
        7
    }

    /// `GameWorld_IsWorldPosVisibleToPlayer` without rectangle (`0x0047B7B0`): bit 2 of the fog probe.
    fn fog_visible(&self, position: Vec3) -> bool {
        self.fog(position) & 2 != 0
    }
}

/// An own effect of an object.
#[derive(Debug, Clone, PartialEq)]
pub enum OwnedKind {
    Attached(AttachedEffect),
    /// Fish of the fishing baskets (`PropertyResource_Init` `0x00521340`).
    Fishes(Ambient),
}

#[derive(Debug, Clone, PartialEq)]
struct Owned {
    /// Key of the map (running counter `GameObject+0x170`).
    key: u32,
    /// Time until `Stop` (ms), -1 = none.
    ttl: i32,
    kind: OwnedKind,
}

/// Own effects of an object.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectEffects {
    pub object: usize,
    /// Moving object (modules Walk, Train, MultiPart): advanced every frame, even
    /// off screen, and drawn after the static ones.
    pub moving: bool,
    next_key: u32,
    owned: Vec<Owned>,
    /// `.hef` on the object with its key ([`EffectWorld::add_handle`]).
    handles: Vec<(u32, HandleEffect)>,
}

impl ObjectEffects {
    /// Order of the MFC `CMap` (hash size 17, key `k`): by `(k >> 4) % 17`, within each chain
    /// newest first (`ObjectRenderer_Draw`).
    fn map_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.owned.len()).collect();
        order.sort_by_key(|&i| {
            let k = self.owned[i].key;
            ((k >> 4) % 17, std::cmp::Reverse(k))
        });
        order
    }

    /// Effect for the key.
    pub fn get_mut(&mut self, key: u32) -> Option<&mut OwnedKind> {
        self.owned
            .iter_mut()
            .find(|o| o.key == key)
            .map(|o| &mut o.kind)
    }

    pub fn len(&self) -> usize {
        self.owned.len()
    }

    /// Keys of the own effects (creation order).
    pub fn keys(&self) -> Vec<u32> {
        self.owned.iter().map(|o| o.key).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.owned.is_empty()
    }
}

/// A world effect (EffectManager).
#[derive(Debug, Clone, PartialEq)]
pub enum WorldKind {
    /// With carrier (`Some`: object index) or fixed.
    Attached(AttachedEffect, Option<usize>),
    Ambient(Ambient),
    /// Water ring of a unit (`SurfRing` type 2, layer 1).
    SurfRing(SurfRing),
}

#[derive(Debug, Clone, PartialEq)]
struct WorldEntry {
    kind: WorldKind,
    /// In the render list (since the last game tick).
    listed: bool,
    /// Created via `spawn` (control command `spawn_effect`, `clear_effects`).
    spawned: bool,
    /// Key of a game effect ([`EffectWorld::add_game_effect`]).
    key: Option<u32>,
    /// Target object (`AttachedEffect+0x28`) of a node target ([`EffectTarget::Node`]).
    target_object: Option<usize>,
}

/// `schiff_stehend.pef` (`Unit_UpdateMovementEffects` `0x0053CEE0`; the game never loads a canoe
/// variant).
pub const SHIP_STANDING: &str = "data/effects/schiff_stehend.pef";

/// A watercraft of the map (walk flags `0x20000` + `0x2000`): an object tick runs through
/// when the counter drops below 1 or "moving" changes (`Unit_UpdateMovementEffects`); it creates
/// a `schiff_stehend` as a world effect at the current position only if the ship is
/// **standing** on the water (ground below `WATER_LEVEL`). So immediately in the first tick or on
/// stopping, and then every 20 ticks (2 s); none while moving (wake and bow waves
/// are not reimplemented).
#[derive(Debug, Clone, PartialEq)]
pub struct ShipStand {
    pub object: usize,
    position: Vec3,
    yaw: f32,
    on_water: bool,
    /// The ship is currently moving (input of the simulation, `status::MOVE` in the viewer).
    moving: bool,
    /// `Module+0x13C`: "moving" in the last tick that ran through (constructor 0).
    last_moving: bool,
    /// `Module+0x130` (constructor 0).
    counter: i32,
}

impl ShipStand {
    /// A standing ship (not moving).
    pub fn new(object: usize, position: Vec3, yaw: f32, on_water: bool) -> Self {
        Self {
            object,
            position,
            yaw,
            on_water,
            moving: false,
            last_moving: false,
            counter: 0,
        }
    }

    /// One object tick; `true` = create a `schiff_stehend` now.
    fn tick(&mut self) -> bool {
        self.counter -= 1;
        if self.counter >= 1 && self.moving == self.last_moving {
            return false;
        }
        self.last_moving = self.moving;
        self.counter = 2;
        if !self.on_water || self.moving {
            return false;
        }
        self.counter = 20;
        true
    }
}

/// Counts for the state snapshot and overlay.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EffectCounts {
    /// Own effects of the objects (incl. fishing basket fish).
    pub owned: usize,
    /// World effects of layer 1 with `.pef` (e.g. `schiff_stehend`, `spawn_effect`, HEF).
    pub world: usize,
    /// Surf of the ships (without the water rings of the units, [`EffectWorld::surf_ring_count`]),
    /// fish (layer 2 and fishing baskets), eagles.
    pub surf: usize,
    pub fish: usize,
    pub eagles: usize,
    /// Live particles of all effects.
    pub particles: usize,
    /// Frozen `.pef` effects (200 ms rule).
    pub frozen: usize,
}

/// Ground marks of a texture ([`EffectWorld::mark_counts`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkCount {
    /// Texture path ([`Footprint::texture`]).
    pub texture: String,
    /// All footprint/crater effects with this texture (a pair counts once).
    pub count: usize,
    /// Of these, in the render list and visible.
    pub visible: usize,
}

/// A footprint or crater of layer 4 with a render list.
#[derive(Debug, Clone, PartialEq)]
struct MarkEntry {
    mark: Footprint,
    /// In the render list (visible since the last game tick).
    listed: bool,
}

/// All effects of a scene.
pub struct EffectWorld {
    pub library: EffectLibrary,
    rng: MsvcRand,
    /// Game time (ms).
    now_ms: u32,
    /// Time since the last game tick.
    since_tick_ms: u32,
    /// Sorted by object index.
    objects: Vec<ObjectEffects>,
    world: Vec<WorldEntry>,
    ships: Vec<ShipStand>,
    /// Surf sounds of the game ticks since the last [`EffectWorld::take_surf_sounds`].
    surf_sounds: Vec<SurfSound>,
    /// Next key of a game effect.
    next_key: u32,
    /// Layer 4: footprints and craters (`FootprintEffect`) in creation order.
    marks: Vec<MarkEntry>,
    /// Layer 4: tread marks (`TreadMarkEffect`) by key (= creation order).
    treads: BTreeMap<u32, TreadMark>,
    /// Next key of a tread mark (continues across [`EffectWorld::clear`]).
    next_tread: u32,
}

/// A surf sound (`SurfEffect_Tick` `0x004F4670`): visible surf plays action 200 of its type in every
/// game tick at `OBJ_POSITION` (`PlayObjectSound3D(200, 0x113, ...)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfSound {
    pub guid: u32,
    pub position: Vec3,
}

impl Default for EffectWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectWorld {
    pub fn new() -> Self {
        Self {
            library: EffectLibrary::new(),
            rng: MsvcRand::new(EFFECTS_SEED),
            now_ms: 0,
            since_tick_ms: 0,
            objects: Vec::new(),
            world: Vec::new(),
            ships: Vec::new(),
            surf_sounds: Vec::new(),
            next_key: 0,
            marks: Vec::new(),
            treads: BTreeMap::new(),
            next_tread: 0,
        }
    }

    /// Game time of the effects (ms).
    pub fn now_ms(&self) -> u32 {
        self.now_ms
    }

    /// Random number generator of the effects (constructors of the ambient effects).
    pub fn rng(&mut self) -> &mut MsvcRand {
        &mut self.rng
    }

    /// Effects of an object (created if it has none yet).
    pub fn object_mut(&mut self, object: usize, moving: bool) -> &mut ObjectEffects {
        let index = match self.objects.binary_search_by_key(&object, |o| o.object) {
            Ok(i) => i,
            Err(i) => {
                self.objects.insert(
                    i,
                    ObjectEffects {
                        object,
                        moving,
                        next_key: 0,
                        owned: Vec::new(),
                        handles: Vec::new(),
                    },
                );
                i
            }
        };
        &mut self.objects[index]
    }

    /// Own effects of all objects (by object index).
    pub fn objects(&self) -> &[ObjectEffects] {
        &self.objects
    }

    /// `GameObject_AddOwnedEffect(effect, ttl)`: returns the map key.
    pub fn add_owned(&mut self, object: usize, moving: bool, kind: OwnedKind, ttl: i32) -> u32 {
        let slot = self.object_mut(object, moving);
        let key = slot.next_key;
        slot.next_key += 1;
        slot.owned.push(Owned { key, ttl, kind });
        key
    }

    /// `.hef` on an object (`GameObject_AddHandleEffect` `0x0046CC10`): returns the key
    /// for [`EffectWorld::remove_handle`] (from the same sequence as the game effects, so unique
    /// in the whole world). When removing, the original searches by file name
    /// (`GameObject_RemoveHandleEffect` `0x0046D0D0`); the key also distinguishes two
    /// circles of the same name.
    pub fn add_handle(&mut self, object: usize, moving: bool, handle: HandleEffect) -> u32 {
        let key = self.next_key;
        self.next_key = self.next_key.wrapping_add(1);
        self.object_mut(object, moving).handles.push((key, handle));
        key
    }

    /// `.hef` for the key as long as it is attached to its object ([`EffectWorld::clear`] loses
    /// all of them).
    pub fn handle(&self, key: u32) -> Option<&HandleEffect> {
        self.objects
            .iter()
            .flat_map(|slot| &slot.handles)
            .find(|(k, _)| *k == key)
            .map(|(_, h)| h)
    }

    /// Removes the `.hef` for the key (`GameObject_RemoveHandleEffect` `0x0046D0D0`); world effects already
    /// placed run out. `false` if the key does not (or no longer) exist.
    pub fn remove_handle(&mut self, key: u32) -> bool {
        for slot in &mut self.objects {
            if let Some(i) = slot.handles.iter().position(|(k, _)| *k == key) {
                slot.handles.remove(i);
                return true;
            }
        }
        false
    }

    /// Register a standing ship.
    pub fn add_ship(&mut self, ship: ShipStand) {
        self.ships.push(ship);
    }

    /// Unregister all standing ships (their effects already laid run out).
    pub fn clear_ships(&mut self) {
        self.ships.clear();
    }

    /// Track the position of the object's standing ships (world effects lie at the world position).
    pub fn place_ships(&mut self, object: usize, position: Vec3, yaw: f32) {
        for ship in self.ships.iter_mut().filter(|s| s.object == object) {
            ship.position = position;
            ship.yaw = yaw;
        }
    }

    /// Track the object's ships: current position, yaw, "on the water" and "moving". Objects
    /// without a [`ShipStand`] stay untouched. The world effects only arise in the game tick
    /// ([`EffectWorld::step`]) at the position valid then.
    pub fn update_ship(
        &mut self,
        object: usize,
        position: Vec3,
        yaw: f32,
        on_water: bool,
        moving: bool,
    ) {
        for ship in self.ships.iter_mut().filter(|s| s.object == object) {
            ship.position = position;
            ship.yaw = yaw;
            ship.on_water = on_water;
            ship.moving = moving;
        }
    }

    /// Object indices of the registered ships.
    pub fn ship_objects(&self) -> Vec<usize> {
        self.ships.iter().map(|s| s.object).collect()
    }

    /// Does the object have a registered ship ([`EffectWorld::update_ship`])?
    pub fn has_ship(&self, object: usize) -> bool {
        self.ships.iter().any(|s| s.object == object)
    }

    /// Fixed positions of the world effects (check of the placement).
    #[cfg(test)]
    pub(crate) fn world_positions(&self) -> Vec<Vec3> {
        self.world
            .iter()
            .filter_map(|w| match &w.kind {
                WorldKind::Attached(a, _) => match &a.anchor {
                    Anchor::Fixed(p) => Some(p.position),
                    _ => None,
                },
                WorldKind::Ambient(_) | WorldKind::SurfRing(_) => None,
            })
            .collect()
    }

    /// Append a world effect (`GameWorld_AddEffect(e, 0)`: in the render list only from the next
    /// game tick).
    pub fn add_world(&mut self, kind: WorldKind) {
        self.world.push(WorldEntry {
            kind,
            listed: false,
            spawned: false,
            key: None,
            target_object: None,
        });
    }

    /// New effect `path` at the current game time, started at 0 (`AttachedEffect_Construct`
    /// with "run invisibly" = `run_invisible`); `None` if the file is missing.
    pub fn new_effect(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        path: &str,
        lifetime_ms: i32,
        anchor: Anchor,
        run_invisible: bool,
    ) -> Option<AttachedEffect> {
        let mut effect = self.library.instantiate(vfs, models, path, run_invisible)?;
        effect.start(0, self.now_ms);
        if let Anchor::Fixed(placement) = &anchor {
            effect.set_placement(*placement);
        }
        Some(AttachedEffect::new(effect, lifetime_ms, anchor, 0.0))
    }

    /// Control command `spawn_effect`: world effect at a point or on an object, with
    /// scaling; like a run-time effect of the game it keeps running even when not drawn.
    /// [`EffectWorld::clear_spawned`] removes it again. A missing file or one without
    /// effect components creates nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        path: &str,
        lifetime_ms: i32,
        placement: Placement,
        carrier: Option<usize>,
        scale: f32,
    ) -> Result<(), SpawnError> {
        self.spawn_flagged(
            vfs,
            models,
            path,
            lifetime_ms,
            placement,
            carrier,
            scale,
            true,
        )
    }

    /// Like [`EffectWorld::spawn`], but not covered by [`EffectWorld::clear_spawned`] (the
    /// selected effect of the effect browser).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_unflagged(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        path: &str,
        lifetime_ms: i32,
        placement: Placement,
        carrier: Option<usize>,
        scale: f32,
    ) -> Result<(), SpawnError> {
        self.spawn_flagged(
            vfs,
            models,
            path,
            lifetime_ms,
            placement,
            carrier,
            scale,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_flagged(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        path: &str,
        lifetime_ms: i32,
        placement: Placement,
        carrier: Option<usize>,
        scale: f32,
        spawned: bool,
    ) -> Result<(), SpawnError> {
        let anchor = match carrier {
            Some(_) => Anchor::Object {
                offset: Vec3::ZERO,
                with_rotation: true,
            },
            None => Anchor::Fixed(placement),
        };
        let Some(mut effect) = self.new_effect(vfs, models, path, lifetime_ms, anchor, true) else {
            return Err(SpawnError::Unreadable);
        };
        if effect.effect.components().is_empty() {
            return Err(SpawnError::NoComponents);
        }
        effect.effect.set_scale(scale);
        self.world.push(WorldEntry {
            kind: WorldKind::Attached(effect, carrier),
            listed: false,
            spawned,
            key: None,
            target_object: None,
        });
        Ok(())
    }

    /// Control command `clear_effects`: all effects created via [`EffectWorld::spawn`] (not
    /// those from [`EffectWorld::spawn_unflagged`] and not those of the game).
    pub fn clear_spawned(&mut self) {
        self.world.retain(|w| !w.spawned);
    }

    /// Effect of the game (events of the simulation, projectile paths of the host): `Effect_Start(start_ms)`
    /// (`kugelrauch.pef` with remaining frame time, re/combat-host 2.2), scaling, attachment; keeps running even
    /// when not drawn and survives [`EffectWorld::clear_spawned`]. Returns the key for
    /// [`EffectWorld::move_game_effect`]/[`EffectWorld::remove_game_effect`]; `None` without a file or
    /// without components.
    #[allow(clippy::too_many_arguments)]
    pub fn add_game_effect(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        path: &str,
        anchor: Anchor,
        carrier: Option<usize>,
        scale: f32,
        start_ms: u32,
        lifetime_ms: i32,
    ) -> Option<u32> {
        let mut effect = self.library.instantiate(vfs, models, path, true)?;
        if effect.components().is_empty() {
            return None;
        }
        effect.start(start_ms, self.now_ms);
        if let Anchor::Fixed(p) = &anchor {
            effect.set_placement(*p);
        }
        effect.set_scale(scale);
        let key = self.next_key;
        self.next_key = self.next_key.wrapping_add(1);
        self.world.push(WorldEntry {
            kind: WorldKind::Attached(
                AttachedEffect::new(effect, lifetime_ms, anchor, 0.0),
                carrier,
            ),
            listed: false,
            spawned: false,
            key: Some(key),
            target_object: None,
        });
        Some(key)
    }

    fn game_entry(&mut self, key: u32) -> Option<&mut AttachedEffect> {
        self.world
            .iter_mut()
            .find_map(|w| match (&mut w.kind, w.key) {
                (WorldKind::Attached(a, _), Some(k)) if k == key => Some(a),
                _ => None,
            })
    }

    /// Game effect with key `key` (not yet expired).
    pub fn game_effect(&self, key: u32) -> Option<&AttachedEffect> {
        self.world.iter().find_map(|w| match (&w.kind, w.key) {
            (WorldKind::Attached(a, _), Some(k)) if k == key => Some(a),
            _ => None,
        })
    }

    /// Game effect with key `key` for modification (scaling, `Stop`).
    pub fn game_effect_mut(&mut self, key: u32) -> Option<&mut AttachedEffect> {
        self.game_entry(key)
    }

    /// Move a fixed-anchored game effect (bullet, fire-arrow fire in flight).
    pub fn move_game_effect(&mut self, key: u32, placement: Placement) -> bool {
        let Some(a) = self.game_entry(key) else {
            return false;
        };
        a.anchor = Anchor::Fixed(placement);
        a.effect.set_placement(placement);
        true
    }

    /// Set the target of game effect `key` ([`AttachedEffect::set_target`]); `object` is the
    /// target object of a node target (`AttachedEffect+0x28`), evaluated in [`EffectWorld::step`].
    /// `false` if the key does not (or no longer) exist.
    pub fn set_game_effect_target(
        &mut self,
        key: u32,
        target: EffectTarget,
        object: Option<usize>,
    ) -> bool {
        let Some(w) = self.world.iter_mut().find(|w| w.key == Some(key)) else {
            return false;
        };
        let WorldKind::Attached(a, _) = &mut w.kind else {
            return false;
        };
        a.set_target(target);
        w.target_object = object;
        true
    }

    /// Stop game effect `key` (`AttachedEffect_Stop`): no new particles; the entry
    /// lives until the game tick finds the last particle dead. `false` without the key.
    pub fn stop_game_effect(&mut self, key: u32) -> bool {
        let Some(a) = self.game_entry(key) else {
            return false;
        };
        a.stop();
        true
    }

    /// Remove a game effect (end of the path).
    pub fn remove_game_effect(&mut self, key: u32) -> bool {
        let before = self.world.len();
        self.world.retain(|w| w.key != Some(key));
        self.world.len() != before
    }

    /// Surf sounds of the game ticks since the last call, in tick and creation order.
    pub fn take_surf_sounds(&mut self) -> Vec<SurfSound> {
        std::mem::take(&mut self.surf_sounds)
    }

    /// Remove all effects (rebuild, e.g. after a change of `particles_quality`); the
    /// game time and the random number generator keep running. Fixed-anchored game effects (with
    /// key, without carrier: flying bullets, fire-arrow fire, impacts) stay in place.
    pub fn clear(&mut self) {
        self.objects.clear();
        // Fixed-anchored game effects (flying bullets, fire-arrow fire, impacts) stay;
        // those with a carrier (blood, muzzle flash) are dropped because the carrier slots change.
        self.world
            .retain(|w| w.key.is_some() && matches!(w.kind, WorldKind::Attached(_, None)));
        // The object slots change with the rebuild: forget target objects (the target point
        // stays remembered).
        for w in &mut self.world {
            w.target_object = None;
        }
        self.ships.clear();
        self.surf_sounds.clear();
        // Layer 4 (footprints, craters, tread marks) entirely; the tread mark keys of the host
        // become invalid.
        self.marks.clear();
        self.treads.clear();
        self.since_tick_ms = 0;
    }

    /// Rebuild of the object list without restarting the effects: `owned[old]` names the new slot
    /// of the object that carries on the own effects, `.hef` and the standing ship (same
    /// object, also after a change of owner: `GameWorld_ChangeObjectOwner` `0x00481730` keeps
    /// the `GameObject` and only assigns a new ID); `carriers[old]` the new slot of the
    /// carrier of the world effects, which look it up by its ID like `AttachedEffect_FrameUpdate` `0x004F3A50`
    /// (same ID only). Own effects of a vanished object go stopped, as a world effect at their last
    /// position, into the render list and run out
    /// (`GameObject_Destruct` `0x00470980` -> `GameObject_ReleaseOwnedEffectsToWorld` `0x004692D0`:
    /// `GameWorld_AddEffect(e, 1)`, `Stop`); fishing basket fish, `.hef` and the standing ship
    /// are dropped. World effects with a vanished carrier stay at their last position
    /// (`FrameUpdate` sets the ID to -1) and also forget their target object
    /// (`+0x24 = +0x28 = -1`); a vanished target object is forgotten, the target point stays.
    /// Ambient effects, game time and randomness stay untouched; slots beyond the tables
    /// count as vanished.
    pub fn remap_objects(&mut self, owned: &[Option<usize>], carriers: &[Option<usize>]) {
        let to = |map: &[Option<usize>], o: usize| map.get(o).copied().flatten();
        let mut released = Vec::new();
        let mut slots = Vec::with_capacity(self.objects.len());
        for mut slot in std::mem::take(&mut self.objects) {
            match to(owned, slot.object) {
                Some(object) => {
                    slot.object = object;
                    slots.push(slot);
                }
                None => released.extend(release(slot.owned)),
            }
        }
        slots.sort_by_key(|s| s.object);
        self.objects = slots;
        for w in &mut self.world {
            w.target_object = w.target_object.and_then(|o| to(carriers, o));
            if let WorldKind::Attached(_, carrier @ Some(_)) = &mut w.kind {
                *carrier = carrier.and_then(|o| to(carriers, o));
                if carrier.is_none() {
                    w.target_object = None;
                }
            }
        }
        self.add_released(released);
        self.ships.retain_mut(|s| match to(owned, s.object) {
            Some(object) => {
                s.object = object;
                true
            }
            None => false,
        });
    }

    /// Released own effects go straight into the render list (`GameWorld_AddEffect(e, 1)`).
    fn add_released(&mut self, released: Vec<AttachedEffect>) {
        self.world.extend(released.into_iter().map(|a| WorldEntry {
            kind: WorldKind::Attached(a, None),
            listed: true,
            spawned: false,
            key: None,
            target_object: None,
        }));
    }

    /// `GameObject_ReleaseOwnedEffectsToWorld` (`0x004692D0`): **all** own effects of the
    /// object go stopped, as a world effect at their last position, into the render list and run
    /// out, as on destruction ([`EffectWorld::remap_objects`]); fishing basket fish are dropped,
    /// `.hef` stay. Caller: the trailer of a halting wagon (D-274).
    pub fn release_owned(&mut self, object: usize) {
        let Ok(i) = self.objects.binary_search_by_key(&object, |o| o.object) else {
            return;
        };
        let owned = std::mem::take(&mut self.objects[i].owned);
        let released: Vec<AttachedEffect> = release(owned).collect();
        self.add_released(released);
    }

    /// Own effect `key` of the object `object` (without creating an entry).
    pub fn owned_mut(&mut self, object: usize, key: u32) -> Option<&mut OwnedKind> {
        let i = self
            .objects
            .binary_search_by_key(&object, |o| o.object)
            .ok()?;
        self.objects[i].get_mut(key)
    }

    /// Stop an own effect (`vf+0x0C`, `Stop`): no new particles, it runs out. `false` if it does not
    /// (or no longer) exist.
    pub fn stop_owned(&mut self, object: usize, key: u32) -> bool {
        match self.owned_mut(object, key) {
            Some(OwnedKind::Attached(a)) => {
                a.stop();
                true
            }
            _ => false,
        }
    }

    /// Attach a water ring (`GameWorld_AddEffect(e, 0)`: in the render list only from the next
    /// game tick, layer 1 in creation order with the other world effects).
    pub fn add_surf_ring(&mut self, ring: SurfRing) {
        self.add_world(WorldKind::SurfRing(ring));
    }

    fn carrier(view: &dyn ObjectView, object: usize, node: Option<&str>) -> Carrier {
        let frame = view.frame(object);
        Carrier {
            position: frame.position,
            rotation: frame.rotation,
            node: node.and_then(|n| view.node(object, n)),
        }
    }

    /// Game tick (100 ms): lifetime, deletion, render list, visibility of the
    /// ambient effects (view frustum and fog probe `fog`), standing ships.
    fn tick(
        &mut self,
        frustum: &Frustum,
        fog: &dyn Fn(Vec3) -> u8,
        vfs: &Vfs,
        models: &mut ModelCache,
    ) {
        for slot in &mut self.objects {
            // `GameObject_Tick` (`0x004736E0`).
            slot.owned.retain_mut(|o| {
                let alive = match &mut o.kind {
                    OwnedKind::Attached(a) => a.tick(),
                    OwnedKind::Fishes(f) => {
                        f.tick(frustum, fog);
                        true
                    }
                };
                if alive && o.ttl >= 0 {
                    o.ttl -= TICK_MS as i32;
                    if o.ttl < 1 {
                        if let OwnedKind::Attached(a) = &mut o.kind {
                            a.stop();
                        }
                        o.ttl = -1;
                    }
                }
                alive
            });
        }
        // `EffectManager_Tick` (`0x004F2260`): render list rebuilt from all with return value 2.
        self.world.retain_mut(|w| match &mut w.kind {
            WorldKind::Attached(a, _) => {
                let alive = a.tick();
                w.listed = alive;
                alive
            }
            WorldKind::SurfRing(r) => {
                let (alive, visible) = r.tick(frustum, fog);
                w.listed = alive && visible;
                alive
            }
            WorldKind::Ambient(a) => {
                w.listed = a.tick(frustum, fog);
                if let (true, AmbientKind::Surf(surf)) = (w.listed, &a.kind) {
                    self.surf_sounds.push(SurfSound {
                        guid: surf.guid,
                        position: surf.position,
                    });
                }
                true
            }
        });
        // Layer 4: footprints and craters (`0x004F1B50`), tread marks (`0x004F5EB0`; old points
        // drop out here instead of when drawing; listed by the sight sphere of the hull
        // `0x004F5440`, D-286).
        self.marks.retain_mut(|e| {
            let keep = e.mark.tick(frustum, fog);
            e.listed = keep && e.mark.visible();
            keep
        });
        let now = self.now_ms;
        self.treads.retain(|_, t| {
            t.prune(now);
            if t.finished() {
                return false;
            }
            t.tick(frustum, now, fog);
            true
        });
        // New world effects are in the render list only from the next tick.
        let due: Vec<(Vec3, f32)> = self
            .ships
            .iter_mut()
            .filter_map(|s| s.tick().then_some((s.position, s.yaw)))
            .collect();
        for (position, yaw) in due {
            let anchor = Anchor::Fixed(Placement { position, yaw });
            if let Some(e) = self.new_effect(vfs, models, SHIP_STANDING, UNLIMITED_MS, anchor, true)
            {
                self.add_world(WorldKind::Attached(e, None));
            }
        }
    }

    /// One logic step of `dt_ms` game-time ms: due game ticks, then the
    /// frame update. Own effects run for visible and for moving objects
    /// (simplified for the cells around the camera), world effects of the render list always; their
    /// components only if their position is currently seen (`AttachedEffect_Render` `0x004F3530`
    /// draws only after `IsWorldPosVisibleToPlayer`). The fog probe comes from
    /// [`ObjectView::fog`]. Returns the number of game ticks of this step.
    pub fn step(
        &mut self,
        dt_ms: u32,
        frustum: &Frustum,
        view: &dyn ObjectView,
        vfs: &Vfs,
        models: &mut ModelCache,
    ) -> u32 {
        self.now_ms = self.now_ms.wrapping_add(dt_ms);
        self.since_tick_ms += dt_ms;
        let mut ticks = 0;
        while self.since_tick_ms >= TICK_MS {
            self.since_tick_ms -= TICK_MS;
            self.tick(frustum, &|p| view.fog(p), vfs, models);
            ticks += 1;
        }
        let now = self.now_ms;
        let mut spawns: Vec<(usize, HandleSpawn)> = Vec::new();
        for slot in &mut self.objects {
            let frame = view.frame(slot.object);
            if !frame.visible && !slot.moving {
                continue;
            }
            for o in &mut slot.owned {
                match &mut o.kind {
                    OwnedKind::Attached(a) => {
                        let carrier = Self::carrier(view, slot.object, a.node());
                        let placement = a.placement(Some(&carrier));
                        a.effect
                            .advance(now, placement, frame.visible, &mut self.rng);
                    }
                    OwnedKind::Fishes(f) => {
                        if f.visible {
                            f.frame_update(dt_ms);
                        }
                    }
                }
            }
            for (_, handle) in &mut slot.handles {
                let height = |x: f32, z: f32| view.height(x, z);
                handle.tick(dt_ms as i32, frame.position, &height, f32::MIN, &mut |s| {
                    spawns.push((slot.object, s))
                });
            }
        }
        for (object, s) in spawns {
            let anchor = Anchor::Object {
                offset: s.offset,
                with_rotation: false,
            };
            if let Some(mut effect) =
                self.new_effect(vfs, models, &s.effect, s.lifetime_ms, anchor, true)
            {
                effect.yaw = s.yaw;
                self.world.push(WorldEntry {
                    kind: WorldKind::Attached(effect, Some(object)),
                    listed: false,
                    spawned: false,
                    key: None,
                    target_object: None,
                });
            }
        }
        for w in self.world.iter_mut().filter(|w| w.listed) {
            match &mut w.kind {
                WorldKind::Attached(a, carrier) => {
                    // `AttachedEffect_FrameUpdate` (`0x004F3A50`): position from the carrier, then the
                    // target (`Effect_SetTarget` against the position of the previous frame, an intermediate value),
                    // the rotation of the target object replaces that of the carrier; then advance, with
                    // `Effect_SetPosition` (`0x0057C4E0`) remeasuring against the new position.
                    let carrier = carrier.map(|o| Self::carrier(view, o, a.node()));
                    let mut placement = a.placement(carrier.as_ref());
                    if let (Some(o), Some(EffectTarget::Node { node, follow })) =
                        (w.target_object, &a.target)
                    {
                        let frame = view.frame(o);
                        let z = match view.node(o, node) {
                            Some(local) => {
                                object_world_matrix(frame.position, frame.rotation, Vec3::ONE)
                                    .transform_point3(local)
                            }
                            None => frame.position,
                        };
                        if !*follow {
                            // Without "follow" only once (in the original immediately on setting).
                            w.target_object = None;
                        }
                        a.effect.set_target(z);
                        placement.yaw = frame.rotation.y;
                    }
                    let seen = view.fog_visible(placement.position);
                    a.effect.advance(now, placement, seen, &mut self.rng);
                }
                WorldKind::Ambient(a) => a.frame_update(dt_ms),
                // The ring changes only in the game tick.
                WorldKind::SurfRing(_) => {}
            }
        }
        ticks
    }

    /// Draw commands: `below` = layer 2 (before the water), `above` = own effects of the
    /// visible objects (static before moving, each in map order) and then layer 1
    /// in creation order (world effects with `.pef` only at a currently seen position).
    pub fn draw(
        &self,
        view_matrix: Mat4,
        view: &dyn ObjectView,
        below: &mut EffectFrame,
        above: &mut EffectFrame,
    ) {
        below.clear();
        above.clear();
        let now = self.now_ms;
        for w in self.world.iter().filter(|w| w.listed) {
            if let WorldKind::Ambient(a) = &w.kind
                && a.layer() == 2
            {
                a.draw(now, below);
            }
        }
        for moving in [false, true] {
            for slot in self.objects.iter().filter(|s| s.moving == moving) {
                if !view.frame(slot.object).visible {
                    continue;
                }
                for i in slot.map_order() {
                    match &slot.owned[i].kind {
                        OwnedKind::Attached(a) => a.effect.draw(view_matrix, above),
                        OwnedKind::Fishes(f) => f.draw(now, above),
                    }
                }
            }
        }
        for w in self.world.iter().filter(|w| w.listed) {
            match &w.kind {
                // `AttachedEffect_Render` (`0x004F3530`): only at a currently seen position.
                WorldKind::Attached(a, carrier) => {
                    let carrier = carrier.map(|o| Self::carrier(view, o, a.node()));
                    let position = a.placement(carrier.as_ref()).position;
                    if view.fog_visible(position) {
                        a.effect.draw(view_matrix, above);
                    }
                }
                WorldKind::Ambient(a) if a.layer() == 1 => a.draw(now, above),
                WorldKind::Ambient(_) => {}
                WorldKind::SurfRing(r) => draw_surf_ring(r, above),
            }
        }
    }

    /// Create a footprint or crater (layer 4, `GameWorld_AddEffect(e, 0)`: in the render list only from the next
    /// game tick). World effect without object reference; not in the savegame (D-271).
    pub fn add_footprint(&mut self, f: Footprint) {
        self.marks.push(MarkEntry {
            mark: f,
            listed: false,
        });
    }

    /// All footprints and craters (layer 4) in creation order, listed or not.
    pub fn footprints(&self) -> impl Iterator<Item = &Footprint> {
        self.marks.iter().map(|e| &e.mark)
    }

    /// New tread mark (layer 4, `TreadMarkEffect_Construct` `0x004F5350`); returns its key
    /// for [`EffectWorld::tread_mut`] and [`EffectWorld::release_tread`]. The keys continue across
    /// [`EffectWorld::clear`], old ones stay invalid there.
    pub fn add_tread(&mut self, quality: u8) -> u32 {
        let key = self.next_tread;
        self.next_tread = self.next_tread.wrapping_add(1);
        self.treads.insert(key, TreadMark::new(quality));
        key
    }

    /// Tread mark for the key; `None` after [`EffectWorld::clear`] or after it was deleted.
    pub fn tread_mut(&mut self, key: u32) -> Option<&mut TreadMark> {
        self.treads.get_mut(&key)
    }

    /// Owner of the tread mark gone (`+0x2C`): the game tick deletes it as soon as fewer than two
    /// points are left (`0x004F5EB0`); until then it ages out.
    pub fn release_tread(&mut self, key: u32) {
        if let Some(t) = self.treads.get_mut(&key) {
            t.release();
        }
    }

    /// Ground marks of the frame (layer 4, `EffectManager_RenderLayer(4)` `0x004F21F0`): for each listed
    /// and visible footprint one decal per used mark (`FootprintEffect_Render` `0x004F1B90`)
    /// and the strips of all listed tread marks ([`TreadMark::quads`] with terrain height `height`
    /// and fog probe `fog`; listed by the sight sphere of their hull in the game tick,
    /// [`TreadMark::visible`], D-286). The original draws both classes in the shared
    /// creation sequence of the render list; here they are separate (the renderer first draws the
    /// tread marks, which arise once per wheel on the first start, then the decals).
    pub fn ground_marks(
        &self,
        height: &dyn Fn(f32, f32) -> f32,
        fog: &dyn Fn(Vec3) -> u8,
    ) -> GroundMarks {
        let mut out = GroundMarks::default();
        for e in self.marks.iter().filter(|e| e.listed) {
            let f = &e.mark;
            let texture = Footprint::texture(f.kind());
            let radius = Footprint::radius(f.kind());
            let alpha = f.alpha();
            for (i, &position) in f.quad().iter().enumerate() {
                if f.used() & (1 << i) != 0 {
                    out.decals.push(GroundDecal {
                        texture,
                        position,
                        yaw: f.yaw(),
                        radius,
                        alpha,
                    });
                }
            }
        }
        for t in self.treads.values().filter(|t| t.listed()) {
            out.treads.extend(t.quads(self.now_ms, height, fog));
        }
        out
    }

    /// Footprints and craters per texture (sorted by path): `count` all, `visible` the listed
    /// (visible since the last game tick). A pair counts once.
    pub fn mark_counts(&self) -> Vec<MarkCount> {
        let mut by_texture: BTreeMap<&'static str, (usize, usize)> = BTreeMap::new();
        for e in &self.marks {
            let c = by_texture
                .entry(Footprint::texture(e.mark.kind()))
                .or_default();
            c.0 += 1;
            c.1 += usize::from(e.listed);
        }
        by_texture
            .into_iter()
            .map(|(texture, (count, visible))| MarkCount {
                texture: texture.to_string(),
                count,
                visible,
            })
            .collect()
    }

    /// Points of all tread marks (gaps included).
    pub fn tread_points(&self) -> usize {
        self.treads.values().map(|t| t.points().len()).sum()
    }

    /// Live water rings of the units (`SurfRing`, layer 1; control channel `world_display.surf_rings`).
    /// They do not count in [`EffectCounts::surf`], so that the overlay number "surf"
    /// shows only the ship surf.
    pub fn surf_ring_count(&self) -> usize {
        self.world
            .iter()
            .filter(|w| matches!(w.kind, WorldKind::SurfRing(_)))
            .count()
    }

    /// Counts.
    pub fn counts(&self) -> EffectCounts {
        let mut c = EffectCounts::default();
        let now = self.now_ms;
        let effect = |c: &mut EffectCounts, a: &AttachedEffect| {
            c.particles += a.effect.particle_count();
            c.frozen += usize::from(a.effect.frozen(now));
        };
        let ambient = |c: &mut EffectCounts, a: &Ambient| match a.kind {
            AmbientKind::Surf(_) => c.surf += 1,
            AmbientKind::Fishes(_) => c.fish += 1,
            AmbientKind::Eagle(_) => c.eagles += 1,
        };
        for slot in &self.objects {
            c.owned += slot.owned.len();
            for o in &slot.owned {
                match &o.kind {
                    OwnedKind::Attached(a) => effect(&mut c, a),
                    OwnedKind::Fishes(f) => ambient(&mut c, f),
                }
            }
        }
        for w in &self.world {
            match &w.kind {
                WorldKind::Attached(a, _) => {
                    c.world += 1;
                    effect(&mut c, a);
                }
                WorldKind::Ambient(a) => ambient(&mut c, a),
                // Separate count: [`EffectWorld::surf_ring_count`].
                WorldKind::SurfRing(_) => {}
            }
        }
        c
    }
}

/// Own effects that go to the world: stopped, anchored fixed at their last position
/// (`GameObject_ReleaseOwnedEffectsToWorld` `0x004692D0`); fishing basket fish are dropped.
fn release(owned: Vec<Owned>) -> impl Iterator<Item = AttachedEffect> {
    owned.into_iter().filter_map(|o| match o.kind {
        OwnedKind::Attached(mut a) => {
            a.stop();
            a.anchor = Anchor::Fixed(a.effect.placement());
            Some(a)
        }
        OwnedKind::Fishes(_) => None,
    })
}

/// Draw a water ring (`0x004EE830`, layer 1 after the water): world `S(s, 1, s) * RotY * T`,
/// `Model_RenderTinted(1.5, 1.5, 1.5, alpha)` (`0x0058D6F0`) with `SRCALPHA`/`INVSRCALPHA`, CULL CCW
/// and fog. Lighting, depth writing and alpha test `GREATER` are set by the renderer as for the
/// other model effects.
fn draw_surf_ring(ring: &SurfRing, out: &mut EffectFrame) {
    let rotation = nml_world::math::game_rotation_y(ring.yaw());
    let s = ring.scale();
    let world = glam::Affine3A::from_mat3_translation(
        rotation * glam::Mat3::from_diagonal(Vec3::new(s, 1.0, s)),
        ring.position(),
    );
    out.commands.push(DrawCommand::Model(ModelDraw {
        model: ring.model().to_string(),
        world,
        animation: None,
        tint: Some(1.5),
        alpha: Some(ring.alpha()),
        blend: None,
        cull: true,
        fog: true,
        mesh: None,
        player_color: None,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ambient::Fishes;
    use crate::effect::{Component, EffectInstance, NoAssets};
    use crate::marks::SURF_RING_MODEL;
    use crate::output::DrawCommand;
    use nml_formats::pef::{BillboardDesc, BillboardLayerDesc, Curve, PefComponent, PefFile};
    use nml_world::camera::{game_projection, look_at_lh};

    /// A permanent, lying billboard (64 x 64).
    fn effect() -> EffectInstance {
        let layer = BillboardLayerDesc {
            textures: vec![Some("t.tga".into())],
            duration: 1.0,
            looping: true,
            size: Some([64, 64]),
            x_scale: Curve { values: [1.0; 100] },
            y_scale: Curve { values: [1.0; 100] },
            fading: Curve {
                values: [255.0; 100],
            },
            ..BillboardLayerDesc::default()
        };
        let pef = PefFile {
            movement_time: 1.0,
            components: vec![PefComponent::Billboard(BillboardDesc {
                layers: vec![layer],
                ..BillboardDesc::default()
            })],
        };
        EffectInstance::new("x.pef", &pef, false, &mut NoAssets)
    }

    /// Objects 0 and 1 at (100 * i, 0, 0); `visible` per object.
    struct Scene {
        visible: Vec<bool>,
    }

    impl ObjectView for Scene {
        fn frame(&self, object: usize) -> ObjectFrame {
            ObjectFrame {
                position: Vec3::new(100.0 * object as f32, 0.0, 0.0),
                rotation: Vec3::ZERO,
                visible: self.visible[object],
            }
        }

        fn node(&self, _object: usize, name: &str) -> Option<Vec3> {
            (name == "n01").then_some(Vec3::new(0.0, 50.0, 0.0))
        }
    }

    fn frustum() -> Frustum {
        let view = look_at_lh(Vec3::new(0.0, 500.0, -500.0), Vec3::ZERO, Vec3::Y);
        Frustum::from_view_proj(game_projection(0.8, 1.0, 700.0) * view)
    }

    fn attach(world: &mut EffectWorld, object: usize, moving: bool) -> u32 {
        let mut e = effect();
        e.start(0, world.now_ms());
        let a = AttachedEffect::new(
            e,
            crate::attached::UNLIMITED_MS,
            Anchor::Node("n01".into()),
            0.0,
        );
        world.add_owned(object, moving, OwnedKind::Attached(a), -1)
    }

    /// Archive with `data/effects/ring.pef` and `schiff_stehend.pef` (one billboard layer, 1 s, loop).
    fn vfs() -> (tempfile::TempDir, Vfs) {
        let pef = "EFFECT\r\n{\r\n\tBILLBOARDEFFECT\r\n\t{\r\n\t\tBILLBOARDLAYER\r\n\t\t{\r\n\t\t\tNUMTEX 1\r\n\t\t\tTEXTURE0 \"t.tga\"\r\n\t\t\tLOOP 1\r\n\t\t\tDURATION 1\r\n\t\t}\r\n\t}\r\n}\r\n";
        let hef = "PARTICLEMOVE\r\nARCRANGE 0 360\r\nCOUNT 20\r\nEFFECT data/effects/ring.pef\r\n/PARTICLEMOVE\r\n";
        let files = [
            nml_rda::testutil::TestFile {
                path: BEAM,
                data: BEAM_PEF.as_bytes(),
                compress: false,
            },
            nml_rda::testutil::TestFile {
                path: "data/effects/ring.pef",
                data: pef.as_bytes(),
                compress: false,
            },
            nml_rda::testutil::TestFile {
                path: SHIP_STANDING,
                data: pef.as_bytes(),
                compress: false,
            },
            nml_rda::testutil::TestFile {
                path: "data/effects/circle.hef",
                data: hef.as_bytes(),
                compress: false,
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("NML0.RDA"),
            nml_rda::testutil::build_rda(&files),
        )
        .unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    #[test]
    fn game_effects_start_at_their_time_move_and_end_by_key() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let p = Placement {
            position: Vec3::ZERO,
            yaw: 0.0,
        };
        let path = "data/effects/ring.pef";
        let key = world
            .add_game_effect(
                &vfs,
                &mut models,
                path,
                Anchor::Fixed(p),
                None,
                0.5,
                30,
                UNLIMITED_MS,
            )
            .unwrap();
        assert_eq!(world.game_effect(key).map(|a| a.effect.time_ms()), Some(30));
        assert_eq!(world.game_effect(key).map(|a| a.effect.scale()), Some(0.5));
        let to = Placement {
            position: Vec3::new(1.0, 2.0, 3.0),
            yaw: 1.0,
        };
        assert!(world.move_game_effect(key, to));
        assert_eq!(
            world.game_effect(key).map(|a| a.anchor.clone()),
            Some(Anchor::Fixed(to))
        );
        world.clear_spawned();
        assert_eq!(
            world.counts().world,
            1,
            "game effects stay on clear_effects"
        );
        world.clear();
        assert!(
            world.game_effect(key).is_some(),
            "fixed-anchored game effects survive the rebuild"
        );
        world.game_effect_mut(key).unwrap().effect.set_scale(2.0);
        assert_eq!(world.game_effect(key).map(|a| a.effect.scale()), Some(2.0));
        assert!(world.remove_game_effect(key));
        assert!(world.game_effect_mut(key).is_none());
        assert!(!world.remove_game_effect(key));
        assert_eq!(world.counts().world, 0);
        assert!(
            world
                .add_game_effect(
                    &vfs,
                    &mut models,
                    "data/effects/missing.pef",
                    Anchor::Fixed(p),
                    None,
                    1.0,
                    0,
                    UNLIMITED_MS
                )
                .is_none()
        );
    }

    /// Own effect with key `key` on object `object`.
    fn owned_effect(world: &EffectWorld, object: usize, key: u32) -> Option<&AttachedEffect> {
        let slot = world.objects.iter().find(|s| s.object == object)?;
        slot.owned
            .iter()
            .find(|o| o.key == key)
            .and_then(|o| match &o.kind {
                OwnedKind::Attached(a) => Some(a),
                OwnedKind::Fishes(_) => None,
            })
    }

    /// Carrier of game effect `key`.
    fn carrier_of(world: &EffectWorld, key: u32) -> Option<usize> {
        world.world.iter().find_map(|w| match (&w.kind, w.key) {
            (WorldKind::Attached(_, carrier), Some(k)) if k == key => *carrier,
            _ => None,
        })
    }

    // QUIRK D-250: kept
    #[test]
    fn remapping_keeps_effects_of_surviving_objects_and_releases_vanished_ones() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let scene = Scene {
            visible: vec![true, true],
        };
        attach(&mut world, 0, false);
        let kept = attach(&mut world, 1, false);
        let ring = world.add_handle(1, false, HandleEffect::new("a.hef", Vec::new(), 1.0));
        world.add_ship(ShipStand::new(0, Vec3::ZERO, 0.0, false));
        world.add_ship(ShipStand::new(1, Vec3::ZERO, 0.0, false));
        let path = "data/effects/ring.pef";
        let fixed = Anchor::Fixed(Placement {
            position: Vec3::ZERO,
            yaw: 0.0,
        });
        let mut game = |world: &mut EffectWorld, carrier| {
            world
                .add_game_effect(
                    &vfs,
                    &mut models,
                    path,
                    fixed.clone(),
                    carrier,
                    1.0,
                    0,
                    5000,
                )
                .unwrap()
        };
        let on_0 = game(&mut world, Some(0));
        let on_1 = game(&mut world, Some(1));
        // Drawn first, then the effect time runs (200 ms rule).
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        world.step(150, &frustum(), &scene, &vfs, &mut models);
        let time = owned_effect(&world, 1, kept).unwrap().effect.time_ms();
        assert!(time > 0);
        let before = world.counts().world;

        // Object 0 vanishes, object 1 moves up to slot 0 (carrier with the same ID).
        world.remap_objects(&[None, Some(0)], &[None, Some(0)]);

        // The permanent effect carries on with its progress at the new slot, as do the circle
        // and the ship; nothing starts over.
        assert_eq!(world.objects().len(), 1);
        assert_eq!(world.objects()[0].keys(), [kept]);
        assert_eq!(
            owned_effect(&world, 0, kept).map(|a| a.effect.time_ms()),
            Some(time)
        );
        assert!(world.handle(ring).is_some());
        assert_eq!(world.ship_objects(), [0]);
        assert_eq!(carrier_of(&world, on_1), Some(0));
        // The carrier of `on_0` is gone: the effect stays at its position.
        assert!(world.game_effect(on_0).is_some());
        assert_eq!(carrier_of(&world, on_0), None);
        // The effect of the vanished object goes into the world stopped at its last position (node
        // `n01` above object 0) and runs out.
        assert_eq!(world.counts().world, before + 1);
        let released = world
            .world
            .iter()
            .find_map(|w| match &w.kind {
                WorldKind::Attached(a, None) if w.key.is_none() => Some((a, w.listed)),
                _ => None,
            })
            .unwrap();
        assert!(
            released.1,
            "immediately in the render list (`AddEffect(e, 1)`)"
        );
        assert!(!released.0.effect.is_active());
        assert_eq!(
            released.0.anchor,
            Anchor::Fixed(Placement {
                position: Vec3::new(0.0, 50.0, 0.0),
                yaw: 0.0
            })
        );

        let scene = Scene {
            visible: vec![true],
        };
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(
            owned_effect(&world, 0, kept).map(|a| a.effect.time_ms()),
            Some(time + 100)
        );
    }

    #[test]
    fn visible_surf_reports_a_sound_in_every_tick() {
        use crate::ambient::Surf;
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let mut rng = MsvcRand::new(1);
        let near = Surf::new(4105, "s.rdo", Vec3::ZERO, 0.0, 10.0, &mut rng);
        let far = Surf::new(
            4104,
            "s.rdo",
            Vec3::new(0.0, 0.0, -9000.0),
            0.0,
            10.0,
            &mut rng,
        );
        world.add_world(WorldKind::Ambient(Ambient::new(AmbientKind::Surf(near))));
        world.add_world(WorldKind::Ambient(Ambient::new(AmbientKind::Surf(far))));
        let scene = Scene { visible: vec![] };
        // 250 ms = two game ticks; only the visible surf reports one sound per tick at its
        // `OBJ_POSITION`.
        world.step(250, &frustum(), &scene, &vfs, &mut models);
        let sounds = world.take_surf_sounds();
        assert_eq!(
            sounds,
            [SurfSound {
                guid: 4105,
                position: Vec3::ZERO
            }; 2]
        );
        assert!(world.take_surf_sounds().is_empty(), "collected");
        world.step(50, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.take_surf_sounds().len(), 1);
    }

    #[test]
    fn owned_effects_follow_their_node_and_freeze_off_screen() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        attach(&mut world, 1, false);
        let mut scene = Scene {
            visible: vec![true, true],
        };
        for _ in 0..10 {
            world.step(20, &frustum(), &scene, &vfs, &mut models);
        }
        let OwnedKind::Attached(a) = &world.objects()[0].owned[0].kind else {
            panic!("effect expected");
        };
        assert_eq!(a.effect.placement().position, Vec3::new(100.0, 50.0, 0.0));
        assert_eq!(a.effect.time_ms(), 200);
        // Static object off screen: no more updates, not drawn.
        scene.visible[1] = false;
        for _ in 0..20 {
            world.step(20, &frustum(), &scene, &vfs, &mut models);
        }
        let (mut below, mut above) = (EffectFrame::default(), EffectFrame::default());
        world.draw(Mat4::IDENTITY, &scene, &mut below, &mut above);
        assert_eq!(above.quad_count(), 0);
        assert_eq!(world.counts().frozen, 1);
        scene.visible[1] = true;
        world.step(20, &frustum(), &scene, &vfs, &mut models);
        world.draw(Mat4::IDENTITY, &scene, &mut below, &mut above);
        assert_eq!(above.quad_count(), 1);
    }

    #[test]
    fn objects_draw_static_first_and_their_effects_in_map_order() {
        let mut world = EffectWorld::new();
        let keys: Vec<u32> = (0..18).map(|_| attach(&mut world, 0, true)).collect();
        attach(&mut world, 1, false);
        assert_eq!(keys[17], 17);
        // Keys 0...15 in chain 0 (newest first), 16 and 17 in chain 1.
        let order: Vec<u32> = world.objects()[0]
            .map_order()
            .into_iter()
            .map(|i| world.objects()[0].owned[i].key)
            .collect();
        assert_eq!(order[..3], [15, 14, 13]);
        assert_eq!(order[16..], [17, 16]);
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let scene = Scene {
            visible: vec![true, true],
        };
        world.step(20, &frustum(), &scene, &vfs, &mut models);
        world.step(20, &frustum(), &scene, &vfs, &mut models);
        let (mut below, mut above) = (EffectFrame::default(), EffectFrame::default());
        world.draw(Mat4::IDENTITY, &scene, &mut below, &mut above);
        // First object 1 (static), then the 18 effects of object 0 (moving).
        let DrawCommand::Quads(first) = &above.commands[0] else {
            panic!("quads expected");
        };
        assert_eq!(above.commands.len(), 19);
        assert!(above.vertices[first.vertices.start as usize].position[0] > 0.0);
    }

    #[test]
    fn world_effects_enter_the_render_list_with_the_next_tick_and_end_with_their_lifetime() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let mut e = effect();
        e.start(0, 0);
        let a = AttachedEffect::new(
            e,
            300,
            Anchor::Fixed(Placement {
                position: Vec3::ZERO,
                yaw: 0.0,
            }),
            0.0,
        );
        world.add_world(WorldKind::Attached(a, None));
        let fishes = Fishes::new("f.rdo", Vec3::ZERO, 20.0, world.rng());
        world.add_world(WorldKind::Ambient(Ambient::new(AmbientKind::Fishes(
            fishes,
        ))));
        let scene = Scene { visible: vec![] };
        let (mut below, mut above) = (EffectFrame::default(), EffectFrame::default());
        world.step(50, &frustum(), &scene, &vfs, &mut models);
        world.draw(Mat4::IDENTITY, &scene, &mut below, &mut above);
        assert_eq!((above.quad_count(), below.model_count()), (0, 0));
        world.step(50, &frustum(), &scene, &vfs, &mut models);
        world.draw(Mat4::IDENTITY, &scene, &mut below, &mut above);
        assert_eq!((above.quad_count(), below.model_count()), (1, 5));
        let counts = world.counts();
        assert_eq!((counts.world, counts.fish), (1, 1));
        // Lifetime 300 ms: deleted after the third tick.
        world.step(200, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.counts().world, 0);
        // `spawn` with a missing file creates nothing; `clear_spawned` leaves ambient effects.
        assert_eq!(
            world.spawn(
                &vfs,
                &mut models,
                "data/effects/missing.pef",
                1000,
                Placement {
                    position: Vec3::ZERO,
                    yaw: 0.0
                },
                None,
                1.0
            ),
            Err(SpawnError::Unreadable)
        );
        world.clear_spawned();
        assert_eq!(world.counts().fish, 1);
    }

    #[test]
    fn spawning_a_file_without_effect_components_is_an_error() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let placement = Placement {
            position: Vec3::ZERO,
            yaw: 0.0,
        };
        for spawn in [EffectWorld::spawn, EffectWorld::spawn_unflagged] {
            let result = spawn(
                &mut world,
                &vfs,
                &mut models,
                "data/effects/circle.hef",
                1000,
                placement,
                None,
                1.0,
            );
            assert_eq!(result, Err(SpawnError::NoComponents));
        }
        assert_eq!(world.counts().world, 0);
        assert_eq!(
            SpawnError::NoComponents.to_string(),
            "contains no effect components (no .pef?)"
        );
    }

    #[test]
    fn clear_spawned_keeps_effects_spawned_without_the_flag() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let placement = Placement {
            position: Vec3::ZERO,
            yaw: 0.0,
        };
        let path = "data/effects/ring.pef";
        assert_eq!(
            world.spawn_unflagged(&vfs, &mut models, path, 1000, placement, None, 1.0),
            Ok(())
        );
        assert_eq!(
            world.spawn(&vfs, &mut models, path, 1000, placement, None, 1.0),
            Ok(())
        );
        assert_eq!(world.counts().world, 2);
        world.clear_spawned();
        assert_eq!(world.counts().world, 1);
    }

    /// Record of the first HEF on object 0.
    fn record_of(world: &EffectWorld) -> nml_formats::hef::HefRecord {
        world.objects()[0].handles[0].1.records()[0].clone()
    }

    #[test]
    fn standing_ships_lay_a_new_effect_every_two_seconds() {
        let mut ship = ShipStand::new(0, Vec3::ZERO, 0.0, true);
        let due: Vec<bool> = (0..41).map(|_| ship.tick()).collect();
        let at: Vec<usize> = (0..41).filter(|&i| due[i]).collect();
        assert_eq!(at, [0, 20, 40]);
        let mut dry = ShipStand::new(0, Vec3::ZERO, 0.0, false);
        assert!((0..40).all(|_| !dry.tick()));
    }

    #[test]
    fn handle_effects_spawn_world_effects_around_their_carrier() {
        use nml_formats::hef::HefRecord;
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let record = HefRecord {
            arc: true,
            arc_to: 360,
            duration_or_count: 1000,
            timeout: 10,
            effect: "data/effects/ring.pef".into(),
            effect_duration: 500,
            ..HefRecord::default()
        };
        world.add_handle(0, false, HandleEffect::new("a.hef", vec![record], 100.0));
        let missing = HefRecord {
            effect: "data/effects/missing.pef".into(),
            ..record_of(&world)
        };
        world.add_handle(1, false, HandleEffect::new("b.hef", vec![missing], 100.0));
        let scene = Scene {
            visible: vec![true, true],
        };
        // Per step (20 ms >= TIMEOUT 10) one world effect around object 0; the missing file
        // yields none.
        for _ in 0..5 {
            world.step(20, &frustum(), &scene, &vfs, &mut models);
        }
        assert_eq!(world.counts().world, 5);
        // Lifetime 500 ms: after another 500 ms the first ones are deleted.
        let scene_off = Scene {
            visible: vec![false, false],
        };
        for _ in 0..25 {
            world.step(20, &frustum(), &scene_off, &vfs, &mut models);
        }
        assert_eq!(world.counts().world, 0);
    }

    #[test]
    fn handles_can_be_removed_by_key() {
        use nml_formats::hef::HefRecord;
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let record = HefRecord {
            arc: true,
            arc_to: 360,
            duration_or_count: 1000,
            timeout: 10,
            effect: "data/effects/ring.pef".into(),
            effect_duration: 500,
            ..HefRecord::default()
        };
        // Two circles on object 0, one on object 1: different keys.
        let a = world.add_handle(
            0,
            false,
            HandleEffect::new("a.hef", vec![record.clone()], 1.0),
        );
        let b = world.add_handle(
            0,
            false,
            HandleEffect::new("b.hef", vec![record.clone()], 1.0),
        );
        let c = world.add_handle(1, true, HandleEffect::new("c.hef", vec![record], 1.0));
        assert!(a != b && b != c && a != c);
        assert_eq!(world.handle(b).map(|h| h.path.as_str()), Some("b.hef"));
        // Remove `b`: the others stay, a second removal finds nothing.
        assert!(world.remove_handle(b));
        assert!(!world.remove_handle(b));
        assert!(world.handle(b).is_none());
        assert_eq!(world.handle(a).map(|h| h.path.as_str()), Some("a.hef"));
        assert_eq!(world.handle(c).map(|h| h.path.as_str()), Some("c.hef"));
        // Per step (20 ms >= TIMEOUT 10) only `a` and `c` still set a world effect.
        let scene = Scene {
            visible: vec![true, true],
        };
        world.step(20, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.counts().world, 2);
        assert!(world.remove_handle(a));
        assert!(world.remove_handle(c));
        world.step(20, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.counts().world, 2, "no new ones after removal");
        // `clear` (rebuild) loses all circles: their keys are unknown afterwards.
        let d = world.add_handle(0, false, HandleEffect::new("d.hef", Vec::new(), 1.0));
        world.clear();
        assert!(world.handle(d).is_none());
        assert!(!world.remove_handle(d));
    }

    #[test]
    fn moving_ships_lay_no_standing_effect_and_lay_one_at_once_after_stopping() {
        let mut ship = ShipStand::new(0, Vec3::ZERO, 0.0, true);
        assert!(ship.tick(), "standing: immediately in the first tick");
        ship.moving = true;
        // Moving: the state transition lets the tick run through, but without an effect; afterwards not a
        // single one in 100 ticks.
        assert!((0..100).all(|_| !ship.tick()));
        // Stopping: immediately in the next tick (even if the counter is still running), then every 20.
        ship.moving = false;
        let due: Vec<bool> = (0..41).map(|_| ship.tick()).collect();
        let at: Vec<usize> = (0..41).filter(|&i| due[i]).collect();
        assert_eq!(at, [0, 20, 40]);
    }

    #[test]
    fn standing_effect_appears_at_the_ships_new_position_after_it_stops() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let start = Vec3::new(10.0, 0.0, 20.0);
        let halt = Vec3::new(500.0, 0.0, 700.0);
        world.add_ship(ShipStand::new(3, start, 0.0, true));
        assert!(world.has_ship(3) && !world.has_ship(0));
        let scene = Scene { visible: vec![] };
        // The ship moves from the first tick on: objects without a ship stay untouched, nothing
        // is created.
        world.update_ship(0, halt, 0.0, true, false);
        world.update_ship(3, start, 0.0, true, true);
        world.step(1000, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.counts().world, 0);
        // Stopped at a new position: in the next tick an effect there, not at the start point.
        world.update_ship(3, halt, 1.5, true, false);
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.world_positions(), [halt]);
        // Another one every 2 s, again at the position of the ship (after an offset of one tick).
        world.step(1900, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.world_positions().len(), 1);
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.world_positions(), [halt, halt]);
    }

    #[test]
    fn a_rebuilt_moving_ship_lays_no_ring_when_synced_before_the_first_step() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let scene = Scene { visible: vec![] };
        // Rebuild (`set_particles_quality`): effects gone, ship created anew as "standing"; the
        // viewer syncs it with the simulation before the step (`MapView::sync_ships`).
        world.clear();
        world.add_ship(ShipStand::new(2, Vec3::ZERO, 0.0, true));
        assert_eq!(world.ship_objects(), [2]);
        world.update_ship(2, Vec3::new(5.0, 0.0, 5.0), 0.0, true, true);
        world.step(1000, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.counts().world, 0);
        // Without the sync the first tick lays a ring at once (the bug the sync prevents).
        let mut stale = EffectWorld::new();
        stale.add_ship(ShipStand::new(2, Vec3::ZERO, 0.0, true));
        stale.step(100, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(stale.counts().world, 1);
    }

    /// Beam with target: one emitter (20/s, loop, life 1 s), `MOVEMENTTIME 1`.
    const BEAM: &str = "data/effects/beam.pef";
    const BEAM_PEF: &str = "EFFECT\r\n{\r\n\tMOVEMENTTIME 1.000000\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tTEXTURE \"p.tga\"\r\n\t\tLOOP 1\r\n\t\tDURATION 1\r\n\t\tEMIT 20\r\n\t\tSPEED 10\r\n\t\tACCEL 1\r\n\t\tLIFE 1\r\n\t\tALPHA0 0 255\r\n\t\tALPHA1 0.3 255\r\n\t\tALPHA2 0.6 255\r\n\t\tALPHA3 1 255\r\n\t\tSIZE0 0 4\r\n\t\tSIZE1 0.3 4\r\n\t\tSIZE2 0.6 4\r\n\t\tSIZE3 1 4\r\n\t}\r\n}\r\n";

    /// Objects at (100 * i, 0, 0) with yaw `yaw[i]`, all visible; node `n01` at (0, 50, 0).
    struct Line {
        yaw: Vec<f32>,
    }

    impl ObjectView for Line {
        fn frame(&self, object: usize) -> ObjectFrame {
            ObjectFrame {
                position: Vec3::new(100.0 * object as f32, 0.0, 0.0),
                rotation: Vec3::new(0.0, self.yaw[object], 0.0),
                visible: true,
            }
        }

        fn node(&self, object: usize, name: &str) -> Option<Vec3> {
            assert!(object < self.yaw.len(), "object {object} does not exist");
            (name == "n01").then_some(Vec3::new(0.0, 50.0, 0.0))
        }
    }

    /// Beam as a game effect on object `carrier` (no offset, with object rotation).
    fn beam(world: &mut EffectWorld, vfs: &Vfs, models: &mut ModelCache, carrier: usize) -> u32 {
        let anchor = Anchor::Object {
            offset: Vec3::ZERO,
            with_rotation: true,
        };
        world
            .add_game_effect(
                vfs,
                models,
                BEAM,
                anchor,
                Some(carrier),
                1.0,
                0,
                UNLIMITED_MS,
            )
            .unwrap()
    }

    /// Target object of game effect `key`.
    fn target_object_of(world: &EffectWorld, key: u32) -> Option<usize> {
        world
            .world
            .iter()
            .find(|w| w.key == Some(key))
            .and_then(|w| w.target_object)
    }

    fn node_target(follow: bool) -> EffectTarget {
        EffectTarget::Node {
            node: "n01".into(),
            follow,
        }
    }

    #[test]
    fn flight_is_measured_against_the_new_placement() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let view = Line {
            yaw: vec![0.0, 0.0, 0.0, 0.7],
        };
        // Carrier object 2 at (200, 0, 0), target node `n01` on object 3 = (300, 50, 0).
        let key = beam(&mut world, &vfs, &mut models, 2);
        assert!(world.set_game_effect_target(key, node_target(true), Some(3)));
        assert_eq!(world.game_effect(key).unwrap().effect.target(), None);
        // First step (tick -> render list): target before position; the measurement against (0, 0, 0) is
        // replaced by `Effect_SetPosition` (`0x0057C4E0`) with the carrier position before a particle arises
        // (D-280, not effective in the original).
        world.step(100, &frustum(), &view, &vfs, &mut models);
        let z = Vec3::new(300.0, 50.0, 0.0);
        let carrier = Vec3::new(200.0, 0.0, 0.0);
        let flight = (z - carrier).length() / 128.0;
        let e = &world.game_effect(key).unwrap().effect;
        assert_eq!(e.target(), Some(z));
        assert_eq!(e.flight_s(), flight);
        assert!(e.particle_count() > 0);
        let Component::Particles(p) = &e.components()[0] else {
            panic!("emitter expected");
        };
        assert_eq!(p.desc().life, flight, "life of the particles = flight time");
        // The rotation of the effect is the yaw of the target object.
        assert_eq!(e.placement().position, carrier);
        assert_eq!(e.placement().yaw, 0.7);
        // Second step: again against the carrier position.
        world.step(20, &frustum(), &view, &vfs, &mut models);
        let e = &world.game_effect(key).unwrap().effect;
        assert_eq!(e.flight_s(), flight);
        assert_eq!(target_object_of(&world, key), Some(3));
    }

    // QUIRK D-276, D-279: kept
    #[test]
    fn target_is_sticky_after_the_object_vanishes() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let view = Line {
            yaw: vec![0.0, 0.0],
        };
        let key = beam(&mut world, &vfs, &mut models, 0);
        world.set_game_effect_target(key, node_target(true), Some(1));
        world.step(100, &frustum(), &view, &vfs, &mut models);
        world.step(20, &frustum(), &view, &vfs, &mut models);
        let z = Vec3::new(100.0, 50.0, 0.0);
        let flight = world.game_effect(key).unwrap().effect.flight_s();
        // The target object vanishes: forgotten, the last point and the flight time stay, new
        // particles keep flying there (the scene no longer knows object 1).
        world.remap_objects(&[Some(0), None], &[Some(0), None]);
        assert_eq!(target_object_of(&world, key), None);
        let view = Line { yaw: vec![0.0] };
        for _ in 0..10 {
            world.step(20, &frustum(), &view, &vfs, &mut models);
        }
        let e = &world.game_effect(key).unwrap().effect;
        assert_eq!((e.target(), e.flight_s()), (Some(z), flight));
        assert!(e.particle_count() > 0);
    }

    #[test]
    fn a_vanished_carrier_forgets_the_target_object_and_remap_follows_it() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let gone = beam(&mut world, &vfs, &mut models, 0);
        let kept = beam(&mut world, &vfs, &mut models, 1);
        world.set_game_effect_target(gone, node_target(true), Some(2));
        world.set_game_effect_target(kept, node_target(true), Some(2));
        // Object 0 vanishes, 1 and 2 move up.
        world.remap_objects(&[None, Some(0), Some(1)], &[None, Some(0), Some(1)]);
        assert_eq!(carrier_of(&world, gone), None);
        assert_eq!(
            target_object_of(&world, gone),
            None,
            "`0x004F3A50`: +0x24 = +0x28 = -1"
        );
        assert_eq!(target_object_of(&world, kept), Some(1));
        // The target object is tracked even without a carrier.
        let free = world
            .add_game_effect(
                &vfs,
                &mut models,
                BEAM,
                Anchor::Fixed(Placement {
                    position: Vec3::ZERO,
                    yaw: 0.0,
                }),
                None,
                1.0,
                0,
                UNLIMITED_MS,
            )
            .unwrap();
        world.set_game_effect_target(free, node_target(true), Some(1));
        world.remap_objects(&[Some(1), Some(0)], &[Some(1), Some(0)]);
        assert_eq!(target_object_of(&world, free), Some(0));
        assert_eq!(target_object_of(&world, kept), Some(0));
    }

    #[test]
    fn a_node_target_without_follow_is_read_once() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let view = Line {
            yaw: vec![0.0, 0.0],
        };
        let key = beam(&mut world, &vfs, &mut models, 0);
        world.set_game_effect_target(key, node_target(false), Some(1));
        world.step(100, &frustum(), &view, &vfs, &mut models);
        assert_eq!(
            world.game_effect(key).unwrap().effect.target(),
            Some(Vec3::new(100.0, 50.0, 0.0))
        );
        assert_eq!(target_object_of(&world, key), None);
        let z = Vec3::new(100.0, 50.0, 0.0);
        assert_eq!(
            world.game_effect(key).unwrap().effect.flight_s(),
            z.length() / 128.0
        );
        // The carrier moves to (300, 0, 0): the target stays fixed, the flight time follows the position.
        world.remap_objects(&[Some(0), Some(1)], &[Some(3), Some(1)]);
        let view = Line { yaw: vec![0.0; 4] };
        world.step(20, &frustum(), &view, &vfs, &mut models);
        let carrier = Vec3::new(300.0, 0.0, 0.0);
        let e = &world.game_effect(key).unwrap().effect;
        assert_eq!(e.placement().position, carrier);
        assert_eq!(e.target(), Some(z));
        assert_eq!(e.flight_s(), (z - carrier).length() / 128.0);
    }

    // QUIRK D-277: kept
    #[test]
    fn point_targets_are_set_at_once_and_remeasured_with_every_placement() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let view = Line {
            yaw: vec![0.0, 0.0],
        };
        // Carrier object 1 at (100, 0, 0).
        let key = beam(&mut world, &vfs, &mut models, 1);
        let z = Vec3::new(7680.0, 0.0, 10240.0);
        // Measured at once, before the first position against the map origin: tile (60, 80) -> 100 s
        // (only an intermediate value, no particle yet).
        assert!(world.set_game_effect_target(key, EffectTarget::Point(z), None));
        let a = world.game_effect(key).unwrap();
        assert_eq!(a.target, Some(EffectTarget::Point(z)));
        assert_eq!((a.effect.target(), a.effect.flight_s()), (Some(z), 100.0));
        assert_eq!(a.effect.particle_count(), 0);
        assert!(!world.set_game_effect_target(999, EffectTarget::Point(z), None));
        // First frame: `Effect_SetPosition` (`0x0057C4E0`) remeasures against the carrier position before
        // the first particle arises (D-277, not effective in the original).
        world.step(100, &frustum(), &view, &vfs, &mut models);
        let carrier = Vec3::new(100.0, 0.0, 0.0);
        let e = &world.game_effect(key).unwrap().effect;
        assert_eq!(e.target(), Some(z));
        assert_eq!(e.flight_s(), (z - carrier).length() / 128.0);
        assert!(e.particle_count() > 0);
        // Moving (`move_game_effect`) also remeasures: 256 units before the target -> 2 s.
        let moved = Placement {
            position: Vec3::new(7680.0, 0.0, 9984.0),
            yaw: 0.0,
        };
        assert!(world.move_game_effect(key, moved));
        assert_eq!(world.game_effect(key).unwrap().effect.flight_s(), 2.0);
    }

    #[test]
    fn stopped_game_effect_lives_until_its_last_particle() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let view = Line {
            yaw: vec![0.0, 0.0],
        };
        let key = beam(&mut world, &vfs, &mut models, 0);
        world.set_game_effect_target(key, node_target(true), Some(1));
        for _ in 0..10 {
            world.step(50, &frustum(), &view, &vfs, &mut models);
        }
        assert!(world.stop_game_effect(key));
        assert!(!world.stop_game_effect(999));
        let a = world.game_effect(key).unwrap();
        assert!(!a.effect.is_active() && a.effect.particle_count() > 0);
        // Flight time ~ 0.9 s: after one second the last particle is dead, the entry gone.
        world.step(500, &frustum(), &view, &vfs, &mut models);
        assert!(world.game_effect(key).is_some());
        for _ in 0..10 {
            world.step(100, &frustum(), &view, &vfs, &mut models);
        }
        assert!(world.game_effect(key).is_none());
    }

    /// Scene without objects with a fixed fog probe.
    struct Fogged(u8);

    impl ObjectView for Fogged {
        fn frame(&self, _object: usize) -> ObjectFrame {
            unreachable!("no objects")
        }

        fn node(&self, _object: usize, _name: &str) -> Option<Vec3> {
            None
        }

        fn fog(&self, _position: Vec3) -> u8 {
            self.0
        }
    }

    #[test]
    fn world_effects_and_surf_wait_for_a_seen_position() {
        use crate::ambient::Surf;
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let mut world = EffectWorld::new();
        let mut e = effect();
        e.start(0, 0);
        let fixed = Anchor::Fixed(Placement {
            position: Vec3::ZERO,
            yaw: 0.0,
        });
        let a = AttachedEffect::new(e, UNLIMITED_MS, fixed, 0.0);
        world.add_world(WorldKind::Attached(a, None));
        let surf = Surf::new(4105, "s.rdo", Vec3::ZERO, 0.0, 10.0, world.rng());
        world.add_world(WorldKind::Ambient(Ambient::new(AmbientKind::Surf(surf))));
        let (mut below, mut above) = (EffectFrame::default(), EffectFrame::default());
        // Explored is not enough for the world effect or the surf (drawing and sound); seen is enough for both.
        for (fog, drawn) in [(1, (0, 0, 0)), (3, (1, 2, 1)), (0, (0, 0, 0))] {
            let scene = Fogged(fog);
            world.step(100, &frustum(), &scene, &vfs, &mut models);
            world.draw(Mat4::IDENTITY, &scene, &mut below, &mut above);
            let sounds = world.take_surf_sounds().len();
            assert_eq!(
                (above.quad_count(), above.model_count(), sounds),
                drawn,
                "fog {fog}"
            );
        }
    }

    fn flat(_: f32, _: f32) -> f32 {
        0.0
    }

    fn seen(_: Vec3) -> u8 {
        7
    }

    // QUIRK D-268: kept
    #[test]
    fn footprints_are_listed_from_the_next_tick_and_end_in_the_tick_their_life_runs_out() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let scene = Scene { visible: vec![] };
        let mut world = EffectWorld::new();
        // Pair (type 6): two decals in mark order.
        let f = Footprint::new(Vec3::ZERO, 0.5, 0, 6, 2);
        let quad = *f.quad();
        world.add_footprint(f);
        assert!(world.ground_marks(&flat, &seen).decals.is_empty());
        let texture = Footprint::texture(6).to_string();
        let count = |visible| MarkCount {
            texture: texture.clone(),
            count: 1,
            visible,
        };
        assert_eq!(world.mark_counts(), [count(0)]);
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        let decal = |position, alpha| GroundDecal {
            texture: Footprint::texture(6),
            position,
            yaw: 0.5,
            radius: 12.0,
            alpha,
        };
        assert_eq!(
            world.ground_marks(&flat, &seen).decals,
            [decal(quad[0], 255), decal(quad[1], 255)]
        );
        assert_eq!(world.mark_counts(), [count(1)]);
        // After 200 ticks remainder 0: still listed with alpha 0; gone in the 201st tick.
        world.step(19_900, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(
            world.ground_marks(&flat, &seen).decals,
            [decal(quad[0], 0), decal(quad[1], 0)]
        );
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        assert!(world.ground_marks(&flat, &seen).is_empty());
        assert!(world.mark_counts().is_empty());
    }

    #[test]
    fn mark_counts_group_by_texture() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let scene = Scene { visible: vec![] };
        let mut world = EffectWorld::new();
        world.add_footprint(Footprint::new(Vec3::ZERO, 0.0, 0, 1, 2));
        world.add_footprint(Footprint::new(Vec3::ZERO, 0.0, 1, 1, 2));
        world.add_footprint(Footprint::new(Vec3::new(1e5, 0.0, 0.0), 0.0, 1, 6, 2));
        world.add_footprint(Footprint::new(Vec3::ZERO, 0.0, 0, 0x430, 2));
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        let count = |kind, count, visible| MarkCount {
            texture: Footprint::texture(kind).to_string(),
            count,
            visible,
        };
        assert_eq!(
            world.mark_counts(),
            [count(1, 2, 2), count(0x430, 1, 1), count(6, 1, 0)]
        );
        assert_eq!(world.ground_marks(&flat, &seen).decals.len(), 3);
    }

    #[test]
    fn treads_lose_old_points_in_the_tick_and_end_after_release() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let scene = Scene { visible: vec![] };
        let mut world = EffectWorld::new();
        let key = world.add_tread(2);
        let now = world.now_ms();
        let tread = world.tread_mut(key).unwrap();
        tread.add_point(Vec3::ZERO, now);
        tread.add_point(Vec3::new(100.0, 0.0, 100.0), now);
        assert_eq!(world.tread_points(), 2);
        // Drawn only from the next game tick (`EffectManager_Add(t, 0)`).
        assert!(world.ground_marks(&flat, &seen).treads.is_empty());
        // Exactly the lifetime old: stays; after that the oldest drops, never the last point.
        world.step(20_000, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.tread_points(), 2);
        assert_eq!(world.ground_marks(&flat, &seen).treads.len(), 1);
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        assert_eq!(world.tread_points(), 1);
        assert!(world.ground_marks(&flat, &seen).treads.is_empty());
        // Without owner and with < 2 points: deleted.
        world.release_tread(key);
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        assert!(world.tread_mut(key).is_none());
        assert_eq!(world.tread_points(), 0);
        // Released with two fresh points: stays.
        let key = world.add_tread(2);
        let now = world.now_ms();
        let tread = world.tread_mut(key).unwrap();
        tread.add_point(Vec3::ZERO, now);
        tread.add_point(Vec3::X, now);
        world.release_tread(key);
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        assert!(world.tread_mut(key).is_some());
    }

    // QUIRK D-286: kept
    #[test]
    fn treads_draw_only_while_their_sight_sphere_lists_them() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let scene = Scene { visible: vec![] };
        let mut world = EffectWorld::new();
        let now = world.now_ms();
        let mut add = |points: [Vec3; 2]| {
            let key = world.add_tread(2);
            let t = world.tread_mut(key).unwrap();
            for p in points {
                t.add_point(p, now);
            }
            key
        };
        // Slanted in view; exactly along z in view; end in view, start (min corner) behind the
        // camera.
        let slanted = add([Vec3::new(-50.0, 0.0, -50.0), Vec3::new(50.0, 0.0, 50.0)]);
        add([Vec3::new(0.0, 0.0, -50.0), Vec3::new(0.0, 0.0, 50.0)]);
        add([Vec3::new(0.0, 0.0, -2000.0), Vec3::new(10.0, 0.0, 0.0)]);
        let counted = |world: &EffectWorld| (world.tread_points(), world.mark_counts());
        let before = counted(&world);
        assert_eq!(before, (6, Vec::new()));
        assert!(world.ground_marks(&flat, &seen).treads.is_empty());
        // After the game tick (`0x004F5EB0` -> `0x004F5440`) only the slanted tread mark is drawn.
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        let drawn = world.ground_marks(&flat, &seen).treads;
        let now = world.now_ms();
        let expected = world.tread_mut(slanted).unwrap().quads(now, &flat, &seen);
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn, expected);
        // Counting is not drawing: points and footprint counts stay.
        assert_eq!(counted(&world), before);
        // The view frustum of the tick decides: seen from far away nothing is listed.
        let away = {
            let view = look_at_lh(
                Vec3::new(50_000.0, 500.0, -500.0),
                Vec3::new(50_000.0, 0.0, 0.0),
                Vec3::Y,
            );
            Frustum::from_view_proj(game_projection(0.8, 1.0, 700.0) * view)
        };
        world.step(100, &away, &scene, &vfs, &mut models);
        assert!(world.ground_marks(&flat, &seen).treads.is_empty());
        assert_eq!(counted(&world), before);
    }

    #[test]
    fn surf_rings_draw_as_tinted_models_on_layer_one_and_end_after_fifteen_ticks() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let scene = Scene { visible: vec![] };
        let mut world = EffectWorld::new();
        let position = Vec3::new(10.0, -3.0, 20.0);
        world.add_surf_ring(SurfRing::new(position, 0.0, Vec3::ZERO, 10.0));
        let (mut below, mut above) = (EffectFrame::default(), EffectFrame::default());
        world.draw(Mat4::IDENTITY, &scene, &mut below, &mut above);
        assert_eq!(above.model_count(), 0, "listed only from the next tick");
        // Separate count (control channel `world_display.surf_rings`); the overlay number "surf" counts
        // only the ship surf.
        assert_eq!((world.surf_ring_count(), world.counts().surf), (1, 0));
        let mut scales = Vec::new();
        for _ in 0..16 {
            world.step(100, &frustum(), &scene, &vfs, &mut models);
            world.draw(Mat4::IDENTITY, &scene, &mut below, &mut above);
            assert!(below.commands.is_empty());
            for c in &above.commands {
                let DrawCommand::Model(m) = c else {
                    panic!("only models");
                };
                assert_eq!(m.model, nml_rda::normalize_path(SURF_RING_MODEL));
                assert_eq!(
                    (m.tint, m.blend, m.cull, m.fog, m.animation.is_none()),
                    (Some(1.5), None, true, true, true)
                );
                assert!(m.alpha.is_some_and(|a| a > 0.0 && a <= 0.5));
                assert_eq!(Vec3::from(m.world.translation), position);
                scales.push(m.world.matrix3.x_axis.x);
                assert_eq!(m.world.matrix3.y_axis.y, 1.0);
            }
        }
        assert_eq!(scales.len(), 15);
        assert!((scales[0] - 0.9).abs() < 1e-5 && (scales[14] - 2.3).abs() < 1e-4);
        assert_eq!(world.surf_ring_count(), 0);
    }

    // QUIRK D-274: deviation
    #[test]
    fn release_owned_stops_all_own_effects_into_the_world() {
        let (_dir, vfs) = vfs();
        let mut models = ModelCache::new();
        let scene = Scene {
            visible: vec![true, true],
        };
        let mut world = EffectWorld::new();
        let a = attach(&mut world, 1, true);
        let b = attach(&mut world, 1, true);
        let other = attach(&mut world, 0, true);
        world.step(100, &frustum(), &scene, &vfs, &mut models);
        assert!(world.stop_owned(1, a));
        assert!(!world.stop_owned(1, 99) && !world.stop_owned(5, a));
        let before = world.counts();
        world.release_owned(1);
        // Both effects of object 1 go into the world stopped (node `n01` above object 1).
        let after = world.counts();
        assert_eq!(
            (after.owned, after.world),
            (before.owned - 2, before.world + 2)
        );
        assert_eq!(owned_effect(&world, 1, b), None);
        assert!(owned_effect(&world, 0, other).is_some());
        let released: Vec<_> = world
            .world
            .iter()
            .filter_map(|w| match &w.kind {
                WorldKind::Attached(e, None) => Some((e, w.listed)),
                _ => None,
            })
            .collect();
        assert_eq!(released.len(), 2);
        for (e, listed) in released {
            assert!(listed && !e.effect.is_active());
            assert_eq!(
                e.anchor,
                Anchor::Fixed(Placement {
                    position: Vec3::new(100.0, 50.0, 0.0),
                    yaw: 0.0
                })
            );
        }
        // A second release changes nothing; new own effects get new keys.
        world.release_owned(1);
        assert_eq!(world.counts().world, after.world);
        assert!(attach(&mut world, 1, true) > b);
    }

    #[test]
    fn footprints_list_all_marks_in_creation_order() {
        let mut world = EffectWorld::new();
        world.add_footprint(Footprint::new(Vec3::ZERO, 0.0, 0, 1, 2));
        world.add_footprint(Footprint::new(Vec3::X, 0.0, 1, 6, 2));
        let kinds: Vec<u32> = world.footprints().map(Footprint::kind).collect();
        assert_eq!(kinds, [1, 6]);
    }

    // QUIRK D-271: deviation
    #[test]
    fn clear_empties_layer_four() {
        let mut world = EffectWorld::new();
        world.add_footprint(Footprint::new(Vec3::ZERO, 0.0, 0, 1, 2));
        let key = world.add_tread(2);
        world.tread_mut(key).unwrap().add_point(Vec3::ZERO, 0);
        world.clear();
        assert!(world.mark_counts().is_empty());
        assert_eq!(world.tread_points(), 0);
        assert!(world.tread_mut(key).is_none());
        assert_ne!(world.add_tread(2), key);
    }
}
