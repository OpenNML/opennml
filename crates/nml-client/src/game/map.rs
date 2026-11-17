//! Map view: loading the level, camera, composing the frame (independent of winit).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use glam::{Affine3A, Mat4, Vec2, Vec3};
use nml_control::RandomMapState;
use nml_effects::attached::{Anchor, UNLIMITED_MS};
use nml_effects::output::{DrawCommand, EffectFrame, ModelDraw};
use nml_effects::triggers::{
    attach_dummies, load_effect_list, numbered_nodes, populate_level_with, populate_object,
};
use nml_effects::world::{EffectCounts, EffectWorld, ObjectFrame, ObjectView};
use nml_formats::hef::DummyEffect;
use nml_formats::lightdata::{LightPreset, find_preset};
use nml_formats::rdo::MeshKind;
use nml_rda::Vfs;
use nml_render::pick::{PickDistances, PickFlags, PickPart, PickRay, pick_object};
use nml_render::{
    EntryLook, FogSettings, GroundMarker, MemoryPiece, ModelEntry, PreviewPiece, SceneRenderer,
    ShadowStats, ViewParams,
};
use nml_sim::combat::projectile::{Projectile, ProjectileKind, THROW_MODEL};
use nml_sim::fog::FogView;
use nml_sim::id::ObjectId;
use nml_sim::module::{ModuleKind, building, shoot};
use nml_sim::object::{GameObject, status};
use nml_sim::random_map::{RandomMapRules, RandomMapSpec};
use nml_sim::rules::Rules;
use nml_sim::setup::{MatchSetup, StartPackage};
use nml_sim::state::{SimEvent, SimState};
use nml_world::anim::Playback;
use nml_world::assets::{ModelAsset, ModelCache};
use nml_world::camera::{
    AspectMode, CameraSetup, GameCamera, ray_from_ndc, start_focus, start_focus_of,
};
use nml_world::classes::DecalKind;
use nml_world::environment::{WeatherKind, level_start_environment, load_light_presets};
use nml_world::level::{LevelScene, load_level};
use nml_world::markers::{marker_color, player_rgb};
use nml_world::math::{game_rotation_x, game_rotation_y, mesh_node_matrix, object_world_matrix};
use nml_world::rand::MsvcRand;
use nml_world::shadows::{Aabb, TileMask, instance_box, mesh_box, object_in_view};
use nml_world::terrain::FogFactors;
use nml_world::visibility::{Frustum, TerrainQuadtree};
use nml_world::weather::{WEATHER_SEED, Weather};

use crate::game::ability_effects::AbilityEffects;
use crate::game::building_fires::BuildingFires;
use crate::game::destruction_marks::DestructionMarks;
use crate::game::economy::PlacementPreview;
use crate::game::grid::project;
use crate::game::heal_effects::HealEffects;
use crate::game::input::FreeCamera;
use crate::game::movement_effects::{MapMovement, MovementEffects, track_tiles};
use crate::game::notices::{Notices, SoundRequest};
use crate::game::options::{LEVEL_LIGHT_NAME, LightSetting, ViewOptions, WeatherSetting};
use crate::game::overlay::{OverlayLine, OverlayText};
use crate::game::presentation::{FogValues, ScriptFog};
use crate::game::projectiles::{Flight, Flights, Kind, MeshRef, Phase};
use crate::game::rally_flag::{RallyFlag, flag_draw};
use crate::game::selection::{RightClickTarget, click_target};
use crate::game::sim_host::{
    DisplayAnim, SimHost, WheelRate, map_objects, start_sim_with, viewer_anim,
};
use crate::game::train_effects::TrainSparks;
use crate::game::world_display::{stealth_ids, world_display_state};
use crate::game::world_effects::{Key as WorldKey, WorldEffects, charge_draws, ground_lift};

/// Next weather kind for F7: off -> rain -> snow -> off.
pub fn next_weather(kind: Option<WeatherKind>) -> Option<WeatherKind> {
    match kind {
        None => Some(WeatherKind::Rain),
        Some(WeatherKind::Rain) => Some(WeatherKind::Snow),
        Some(WeatherKind::Snow) => None,
    }
}

/// Light after `step` steps in the order `level`, preset 0, 1, ... (F6 forward,
/// Shift+F6 back); `current` = `None` for the level light.
pub fn step_light(current: Option<usize>, count: usize, step: i32) -> Option<usize> {
    let slots = count as i64 + 1;
    let slot = current.map_or(0, |i| i as i64 + 1);
    let next = (slot + i64::from(step)).rem_euclid(slots);
    (next > 0).then(|| next as usize - 1)
}

/// Index of the preset for a setting (`None` = level light); `start` takes the start light
/// of the level, but **only without a script** (`scripted = false`): a match with a running
/// level script begins with the light of the `.wld` until the script sets it (171). If the
/// start light is not in the list, the level light remains (warning): `GameEngine_ApplyLightPreset`
/// changes nothing for an unknown name. An explicitly requested unknown name, on the other hand, is
/// an error with the known names (`level` first, as in `lights`); explicit settings also apply
/// with a script.
pub fn resolve_light(
    setting: &LightSetting,
    lvl_path: &str,
    presets: &[LightPreset],
    scripted: bool,
) -> Result<Option<usize>> {
    let (name, requested) = match setting {
        LightSetting::Level => return Ok(None),
        LightSetting::Start if scripted => return Ok(None),
        LightSetting::Start => match level_start_environment(lvl_path).light {
            Some(name) => (name, false),
            None => return Ok(None),
        },
        LightSetting::Preset(name) => (name.as_str(), true),
    };
    let Some(preset) = find_preset(presets, name) else {
        if !requested {
            log::warn!(
                "{lvl_path}: start light \"{name}\" is not in the light presets, the level light remains"
            );
            return Ok(None);
        }
        let names: Vec<&str> = std::iter::once(LEVEL_LIGHT_NAME)
            .chain(presets.iter().map(|p| p.name.as_str()))
            .collect();
        bail!(
            "unknown light preset \"{name}\" (known: {})",
            names.join(", ")
        );
    };
    Ok(presets.iter().position(|p| std::ptr::eq(p, preset)))
}

/// Presets from the result of `load_light_presets`: if the `lightdata.ini` is missing or
/// not readable, the list stays empty (warning). `GameEngine_LoadLightPresets` does not abort the
/// start either.
pub fn presets_or_empty(loaded: Result<Vec<LightPreset>>) -> Vec<LightPreset> {
    loaded.unwrap_or_else(|error| {
        log::warn!("Light presets not loaded, the list stays empty: {error:#}");
        Vec::new()
    })
}

/// Weather for a setting; `start` takes the start weather of the level, but **only without a script**
/// (`scripted = false`): with a running level script every level begins without weather until the
/// script sets it (209/210). Explicit settings also apply with a script.
pub fn resolve_weather(
    setting: WeatherSetting,
    lvl_path: &str,
    scripted: bool,
) -> Option<WeatherKind> {
    match setting {
        WeatherSetting::Start if scripted => None,
        WeatherSetting::Start => level_start_environment(lvl_path).weather,
        WeatherSetting::Off => None,
        WeatherSetting::Rain => Some(WeatherKind::Rain),
        WeatherSetting::Snow => Some(WeatherKind::Snow),
    }
}

/// Fog probe of the effects: `QueryFogOfWarArea(tile(pos), active player, 0)` including the
/// view switch (re/vision 5.2, 5.3); without a simulation 7 (everything clear).
fn fog_probe(sim: Option<&SimHost>) -> impl Fn(Vec3) -> u8 + '_ {
    let view = sim.map(|sim| {
        (
            sim.fog.view(&sim.state),
            &sim.rules.map,
            i32::from(sim.active_player),
        )
    });
    move |p: Vec3| match &view {
        Some((view, map, viewer)) => {
            let (x, z) = FogView::world_tile(map, p.to_array());
            view.area(x, z, *viewer, 0)
        }
        None => 7,
    }
}

/// Yaw of the muzzle smoke `kanone_rauch.pef`: yaw of the shooter plus the extra yaw `+0xC8` of the
/// projectile (cannon 0, broadside +/-pi/2; re/combat-host 2.2, re/ships 7).
fn muzzle_smoke_yaw(shooter_yaw: f32, yaw_extra: f32) -> f32 {
    shooter_yaw + yaw_extra
}

/// Position of an own effect at a fixed point (`SimEvent::EffectOwnedAt`): at `pos`, yaw
/// `fpatan(aim.x - pos.x, aim.z - pos.z)` toward the point `aim` (`AttachedEffect_SetPositionAndRotation`
/// in FireGun `0x00526A6F` and FireCannon `0x005265F5`, re/combat-ranged 3.4, 3.5).
fn owned_effect_placement(pos: [f32; 3], aim: [f32; 3]) -> nml_effects::particles::Placement {
    nml_effects::particles::Placement {
        position: Vec3::from(pos),
        yaw: (aim[0] - pos[0]).atan2(aim[2] - pos[2]),
    }
}

/// Muzzle of a projectile: without a node (quartered shooter, `start` is the shot position
/// of the building) `start`; otherwise the animated node position of the shooter, without it `start`.
fn flight_muzzle(node: &str, start: Vec3, node_pos: impl FnOnce() -> Option<Vec3>) -> Vec3 {
    if node.is_empty() {
        return start;
    }
    node_pos().unwrap_or(start)
}

/// Is the ship moving? `Unit_UpdateMovementEffects` (`0x0053CEE0`) gets "moving" from the movement
/// of the object; the reimplementation uses `status::MOVE`, which the simulation's walk sets for the
/// duration of the trip (confirmed on the transport ship, test `ships_leave_a_wake_while_moving`;
/// Ghidra check of the original still open).
fn ship_is_moving(object: &GameObject) -> bool {
    object.status & status::MOVE != 0
}

/// Objects of the map as the effects see them: position from the `.wld`, visibility as in
/// `0x00468C60` with the rest hull of the model and only on currently seen ground
/// (`IsWorldPosVisibleToPlayer` without a rectangle), helper nodes from the running pose.
struct MapObjects<'a> {
    scene: &'a LevelScene,
    entries: &'a [ModelEntry],
    entry_of: &'a HashMap<usize, usize>,
    boxes: &'a HashMap<usize, Aabb>,
    frustum: Frustum,
    /// Clock time of the poses; `None` = bind pose (F5 off).
    now_ms: Option<f32>,
    /// Fog probe ([`fog_probe`]).
    fog: &'a dyn Fn(Vec3) -> u8,
}

/// Do the own effects of an entry with the render bits `bits` and the status `object_status`
/// (`ModelEntry::status`) run and get drawn? In the original, `ObjectRenderer_Draw` draws them via
/// bit 8 of the render bits, which `GameObject_AddOwnedEffect` sets; `GameObject_RenderPass`
/// (`0x0046B890`) forms `mask & (bits | 0x3000300)`. They are dropped only if
/// - the object is stealthed (`MASKED`) and carries no real drawing bit for the viewer (model,
///   construction phase, burning, fading): `GameObject_GetRenderBitsForPlayer` (`0x00466BE0`) then
///   returns only `0x80` (without bit 8; already 0 when standing in the reimplementation,
///   `sim_host::stealth_bits`) - so the effect of a foreign stealthed object (the assassin digging in)
///   stays invisible (re/world-display W.2.2, D-263, D-267), or
/// - the object is quartered (`QUARTERED`, return before drawing in `0x0046B890`).
///
/// All other entries with render bits 0 keep their effects, e.g. the fishing baskets (3505) with
/// their fish effect (`OwnedKind::Fishes`), whose model is never drawn.
pub(crate) fn draws_own_effects(bits: u32, object_status: u32) -> bool {
    use nml_render::scene::{RENDER_BUILD, RENDER_BURNING, RENDER_FADE, RENDER_MODEL};
    if object_status & status::QUARTERED != 0 {
        return false;
    }
    object_status & status::MASKED == 0
        || bits & (RENDER_MODEL | RENDER_BUILD | RENDER_BURNING | RENDER_FADE) != 0
}

impl ObjectView for MapObjects<'_> {
    fn frame(&self, object: usize) -> ObjectFrame {
        let o = &self.scene.objects[object];
        let drawn = self
            .entry_of
            .get(&object)
            .and_then(|&e| self.entries.get(e))
            .is_none_or(|entry| draws_own_effects(entry.look.bits, entry.status));
        ObjectFrame {
            position: o.position,
            rotation: o.rotation,
            visible: drawn
                && self
                    .boxes
                    .get(&object)
                    .is_some_and(|b| object_in_view(&self.frustum, b, o.position))
                && self.fog_visible(o.position),
        }
    }

    fn node(&self, object: usize, name: &str) -> Option<Vec3> {
        let entry = &self.entries[*self.entry_of.get(&object)?];
        match self.now_ms {
            Some(now) => entry.playback.helper_node(&entry.asset, now, name),
            None => entry.playback.bind_helper_node(&entry.asset, name),
        }
    }

    fn height(&self, x: f32, z: f32) -> f32 {
        self.scene.terrain.ground_height(x, z)
    }

    fn fog(&self, position: Vec3) -> u8 {
        (self.fog)(position)
    }
}

/// A loaded level with renderer, camera and overlay font.
pub struct MapView {
    pub scene: LevelScene,
    pub renderer: SceneRenderer,
    pub text: OverlayText,
    pub camera: GameCamera,
    pub free: Option<FreeCamera>,
    /// Light presets from `lightdata.ini`.
    pub presets: Vec<LightPreset>,
    /// Active light: index into `presets`, `None` = level light.
    pub light: Option<usize>,
    /// Active weather.
    pub weather: Option<Weather>,
    /// Distance fog of the scripts (501/502, 505/506): [`MapView::view_params`] evaluates it before the
    /// values of the `.wld` and the view switch; empty without a script action.
    pub script_fog: ScriptFog,
    weather_rng: MsvcRand,
    quadtree: TerrainQuadtree,
    /// Weather particles moved in the last logic step.
    pub weather_moved: usize,
    /// Models (including the shadow and ground plate models).
    models: ModelCache,
    /// `ShadowQuality` of the shadows (F8).
    pub shadow_quality: u8,
    /// Effects of the map.
    pub effects: EffectWorld,
    effect_list: Vec<DummyEffect>,
    /// `ParticlesQuality` when the effects were built.
    pub particles_quality: u8,
    /// Rest hulls of the objects with own effects (visibility).
    effect_boxes: HashMap<usize, Aabb>,
    /// IDs of the buildings placed as a construction site: their permanent effects do not come until the
    /// end of construction ([`MapView::attach_finished_dummies`]).
    dummies_pending: Vec<u32>,
    /// Object index -> entry of the renderer (helper nodes).
    entry_of: HashMap<usize, usize>,
    /// ID of the simulation object -> index in `scene.objects` (lookup only).
    object_of: HashMap<u32, usize>,
    /// Fraction of a millisecond that goes into the next whole game-time millisecond.
    effect_carry: f64,
    effects_below: EffectFrame,
    effects_above: EffectFrame,
    /// Simulation of the map; `None` if its rules are not loadable (objects of the `.wld`).
    pub sim: Option<SimHost>,
    /// Factors of the fog of war per grid point in the last frame (buffer, renewed each frame).
    fog_factors: FogFactors,
    /// Ray test of the fog memories drawn in the last frame (ID of the object).
    memory_picks: BTreeMap<ObjectId, MemoryPick>,
    /// Per model path the drawn sub-objects of the object layer and the rest hull (lookup).
    memory_shapes: HashMap<String, (Vec<Arc<ModelAsset>>, Aabb)>,
    /// Per type the special-class decals of the fog memories ([`memory_special_decals`]).
    memory_decals: HashMap<u32, Vec<String>>,
    /// Per renderer entry the animation start taken over last (`AnimState::starts`) and the
    /// animation shown (for the alarm post it depends on the viewer, [`viewer_anim`]).
    sim_starts: Vec<Option<AnimShown>>,
    /// Messages, floating credits and sound locks of the simulation.
    pub notices: Notices,
    /// Projectile trajectories of the host (arrows, balls); not in the savegame.
    flights: Flights,
    /// Hidden meshes per simulation object (`MeshVisibility`): animation start at which the
    /// mask applies, and the names. Every animation start resets them.
    mesh_masks: BTreeMap<u32, (u32, Vec<String>)>,
    /// Pose clock of the last [`MapView::advance_effects`] (`None` = bind pose), for the muzzle.
    clock_ms: Option<f32>,
    /// Simulation step of the last target tracking of the arrows.
    tracked_step: u64,
    /// Fraction of a game-time millisecond of the trajectories.
    flight_carry: f64,
    /// Model of the arrow mesh per (shooter type, mesh), see [`arrow_mesh`].
    arrow_models: HashMap<(u32, String), Option<MeshRef>>,
    /// Fires of damaged buildings and vehicles (presentation in the object tick of the simulation); not in
    /// the savegame.
    building_fires: BuildingFires,
    /// Brake sparks of the train (presentation according to the brake bit of the locomotive); not in the savegame.
    train_sparks: TrainSparks,
    /// Healing circles and effects on healed targets (presentation according to healing state and
    /// `SimEvent::Healed`); not in the savegame.
    heal_effects: HealEffects,
    /// Evoker hand and staff, beams, banishing spell and aura ring (presentation according to the
    /// module state); not in the savegame.
    ability_effects: AbilityEffects,
    /// Storm, dynamite charge and smithy effect from the state ([`WorldEffects`]).
    world_effects: WorldEffects,
    /// Effects of the temporary upgrades on the object; not in the savegame.
    temp_effects: TempUpgradeEffects,
    /// Footprints, wheel dust, wheel tracks, water ring and wading sound (presentation in the object tick of
    /// the simulation, [`MovementEffects`]); not in the savegame.
    movement_effects: MovementEffects,
    /// Destruction smoke, building and cannon craters for the events of the simulation
    /// ([`DestructionMarks`]); not in the savegame.
    destruction_marks: DestructionMarks,
    /// Per tile `TEX_BITS & 1` (tracks), once per map ([`track_tiles`]).
    track_tiles: Vec<bool>,
    /// Rally point flag of the last [`MapView::set_rally_flag`] (control channel).
    rally_flag: Option<RallyFlag>,
    /// Its draw command in the last frame (position after a rally target, animation time).
    rally_draw: Option<ModelDraw>,
}

/// State of the effect of a temporary upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TempEffect {
    /// Not created yet (object not drawn or just started).
    Pending,
    /// Game effect with this key.
    Shown(u32),
    /// Not loadable; no new attempt until the match is rebuilt.
    Failed,
}

/// Effects of the temporary upgrades (`EFFECT h path` from `tempupgrades.ini`, re/temp-upgrades 3.1,
/// 3.2, 7): `GameObject_ApplyTempUpgradeRecord` (`0x0046D470`) attaches, for the flag `EFFECT` (`0x01`),
/// an `AttachedEffect` with duration `0x7FFFFFFF` and height `h` (flag `HEIGHT`) to the object, and the
/// end (`0x0046FC90`) stops it (`vf+0xC`). The simulation reports both
/// (`SimEvent::TempUpgradeStarted/Ended`); an object destroyed without dying gets
/// no end reported (in the original the effect disappears with the object), so the sync removes
/// its effects itself. After loading and restart the effects arise from the lists of the
/// state; effects lost when the object list is rebuilt are created again by the sync.
#[derive(Debug, Default)]
pub struct TempUpgradeEffects {
    /// (object, GUID) -> effect.
    active: BTreeMap<(u32, u32), TempEffect>,
}

impl TempUpgradeEffects {
    /// Path and height of the effect of a record; `None` without the flag `EFFECT` or without a record.
    pub fn effect_of(rules: &nml_sim::rules::Rules, guid: u32) -> Option<(&str, f32)> {
        use nml_formats::temp_upgrades::flags;
        let rec = rules.temp_upgrade(guid)?;
        if rec.flags & flags::EFFECT == 0 {
            return None;
        }
        let height = if rec.flags & flags::HEIGHT != 0 {
            rec.effect_height as f32
        } else {
            0.0
        };
        Some((rec.effect_path.as_str(), height))
    }

    /// Events of a step in their order: a start with the flag `EFFECT` notes the effect
    /// in advance, an end removes it, a change of owner carries it on under the new ID (the
    /// list stays, K-288). Returns: keys of the effects to stop.
    pub fn handle(&mut self, rules: &nml_sim::rules::Rules, events: &[SimEvent]) -> Vec<u32> {
        let mut stop = Vec::new();
        for event in events {
            match *event {
                SimEvent::TempUpgradeStarted { object, guid }
                    if Self::effect_of(rules, guid).is_some() =>
                {
                    self.active
                        .entry((object.0, guid))
                        .or_insert(TempEffect::Pending);
                }
                SimEvent::TempUpgradeEnded { object, guid } => {
                    if let Some(TempEffect::Shown(key)) = self.active.remove(&(object.0, guid)) {
                        stop.push(key);
                    }
                }
                SimEvent::OwnerChanged { old, new } => self.rename(&[(old.0, new.0)]),
                _ => {}
            }
        }
        stop
    }

    /// Switch IDs (change of owner: old -> new).
    pub fn rename(&mut self, renamed: &[(u32, u32)]) {
        for &(old, new) in renamed {
            let moved: Vec<_> = self
                .active
                .range((old, 0)..=(old, u32::MAX))
                .map(|(&(_, guid), &e)| (guid, e))
                .collect();
            for (guid, e) in moved {
                self.active.remove(&(old, guid));
                self.active.insert((new, guid), e);
            }
        }
    }

    /// Remove entries of vanished objects; returns: keys of their effects.
    pub fn prune(&mut self, state: &nml_sim::state::SimState) -> Vec<u32> {
        let mut gone = Vec::new();
        self.active.retain(|&(object, _), e| {
            let keep = state.object(ObjectId(object), false).is_some();
            if let (false, TempEffect::Shown(key)) = (keep, *e) {
                gone.push(key);
            }
            keep
        });
        gone
    }

    /// After loading or restart: the entries with the flag `EFFECT` from the lists of the state.
    pub fn seed(&mut self, state: &nml_sim::state::SimState) {
        use nml_formats::temp_upgrades::flags;
        for o in state.objects() {
            for e in o
                .temp_upgrades
                .iter()
                .filter(|e| e.flags & flags::EFFECT != 0)
            {
                self.active
                    .entry((o.id.0, e.guid))
                    .or_insert(TempEffect::Pending);
            }
        }
    }

    /// Remove all effects (loading, restart).
    pub fn reset(&mut self, effects: &mut EffectWorld) {
        for e in std::mem::take(&mut self.active).into_values() {
            if let TempEffect::Shown(key) = e {
                effects.remove_game_effect(key);
            }
        }
    }

    /// Noted effects that are not (yet) or no longer in the effect world.
    fn missing(&self, effects: &EffectWorld) -> Vec<(ObjectId, u32)> {
        self.active
            .iter()
            .filter(|(_, e)| match e {
                TempEffect::Pending => true,
                TempEffect::Shown(key) => effects.game_effect(*key).is_none(),
                TempEffect::Failed => false,
            })
            .map(|(&(object, guid), _)| (ObjectId(object), guid))
            .collect()
    }

    /// Result of creating: key or not loadable.
    fn set_shown(&mut self, object: ObjectId, guid: u32, key: Option<u32>) {
        if let Some(e) = self.active.get_mut(&(object.0, guid)) {
            *e = key.map_or(TempEffect::Failed, TempEffect::Shown);
        }
    }

    /// Noted effects (object, GUID) in key order (tests).
    #[cfg(test)]
    fn wanted(&self) -> Vec<(ObjectId, u32)> {
        self.active
            .keys()
            .map(|&(object, guid)| (ObjectId(object), guid))
            .collect()
    }

    /// Set the key of a created effect (tests).
    #[cfg(test)]
    fn set_key(&mut self, object: ObjectId, guid: u32, key: u32) {
        self.set_shown(object, guid, Some(key));
    }
}

/// Ray test of a fog memory (`FUN_00416440` -> `ObjectRenderer_PickScreenPoint` of the
/// image renderer): main model and drawn sub-objects of the object layer in the bind pose at
/// the remembered world matrix, hull = rest hull of the instance (as for objects without animation).
struct MemoryPick {
    /// Main model first.
    parts: Vec<Arc<ModelAsset>>,
    world: Affine3A,
    hull: Aabb,
}

impl MemoryPick {
    fn pick(&self, ray: &PickRay, flags: PickFlags, d: &mut PickDistances) -> bool {
        let parts: Vec<PickPart> = self
            .parts
            .iter()
            .map(|asset| PickPart { asset, pose: None })
            .collect();
        let center = self.hull.center();
        pick_object(&parts, &self.world, &self.hull, || center, ray, flags, d)
    }
}

/// Launch from [`SimEvent::ProjectileLaunched`] (positions in world coordinates).
struct Launch<'a> {
    id: u32,
    kind: ProjectileKind,
    fire: bool,
    shooter: ObjectId,
    /// Type of the shooter (main model of the arrow).
    guid: u32,
    /// Mesh of the projectile model in the shooter model, empty = none.
    model: &'a str,
    /// Helper node of the muzzle.
    node: &'a str,
    start: Vec3,
    aim: Vec3,
    flight_ms: i32,
    yaw_extra: f32,
}

/// Draw commands of the arrows in flight (re/combat-host 2.4): the mesh of the arrow from the model
/// that contains it ([`arrow_mesh`]), at the position and pitch of the trajectory, plus for each ghost
/// copy the same with fading alpha. The dynamite throw draws likewise, but the whole model
/// `dynamit.rdo` (re/headhunter-robbing 7). Only where the active player has seen the position
/// (`fog & 2`); balls are only their effect. World matrix like the original:
/// `RotX(pitch) * RotY(yaw) * T(position)` with row vectors (column form `T * Ry * Rx`), preceded by the
/// node matrix of the mesh (the arrow lies along z in model space, not along the
/// node axis).
fn flight_draws(flights: &Flights, fog: &dyn Fn(Vec3) -> u8, frame: &mut EffectFrame) {
    for f in &flights.list {
        let mesh = match (f.kind, &f.mesh) {
            (Kind::Arrow { .. }, Some(mesh)) => Some(mesh.clone()),
            (Kind::Throw, _) => None,
            _ => continue,
        };
        let Some(model) = &f.model else {
            continue;
        };
        if f.phase == Phase::Done {
            continue;
        }
        let mut draw = |pos: Vec3, pitch: f32, yaw: f32, alpha: Option<f32>| {
            if fog(pos) & 2 == 0 {
                return;
            }
            let pose =
                Affine3A::from_mat3_translation(game_rotation_y(yaw) * game_rotation_x(pitch), pos);
            frame.commands.push(DrawCommand::Model(ModelDraw {
                model: model.path.clone(),
                world: pose * model.node,
                animation: None,
                tint: None,
                alpha,
                blend: None,
                cull: true,
                fog: true,
                mesh: mesh.clone(),
                player_color: None,
            }));
        };
        draw(f.pos, f.pitch, f.yaw, None);
        for g in &f.ghosts {
            draw(g.pos, g.pitch, g.yaw, Some(g.alpha()));
        }
    }
}

/// Trajectory of a simulation projectile after loading or restart (without GPU). Muzzle and
/// broadside yaw are not in the savegame: start = position of the shooter (`start`), yaw 0. Arrow
/// with the first resolved mesh from 110007 of the shooter (the host then looks for the model),
/// ball, throw with the fixed model [`THROW_MODEL`] (re/headhunter-robbing 7); world effects have
/// no trajectory (the host derives storm and charge itself): `None`.
fn resumed_flight(p: &Projectile, state: &SimState) -> Option<Flight> {
    let (start, aim) = (Vec3::from(p.start), Vec3::from(p.target_pos));
    let shooter = state.object(p.shooter, false);
    let f = match p.kind {
        ProjectileKind::Arrow => {
            // As at launch: the first resolved mesh from 110007 of the shooter.
            let mesh = shooter.and_then(|o| match &o.module(shoot::INDEX)?.kind {
                ModuleKind::Shoot(s) => s.models.first().cloned().flatten(),
                _ => None,
            });
            let mut f = Flight::resume_arrow(
                p.id,
                start,
                aim,
                p.flight_ms,
                p.remaining_ms,
                mesh,
                p.flags & 0x40 != 0,
            );
            f.shooter_guid = shooter.map_or(0, |o| o.guid);
            f
        }
        ProjectileKind::CannonBall => {
            Flight::resume_ball(p.id, start, aim, p.flight_ms, p.remaining_ms, 0.0)
        }
        ProjectileKind::Throw => {
            let mut f =
                Flight::resume_throw(p.id, start, aim, p.flight_ms, p.remaining_ms, THROW_MODEL);
            f.shooter_guid = shooter.map_or(0, |o| o.guid);
            f
        }
        ProjectileKind::Dynamite | ProjectileKind::Explosion | ProjectileKind::Storm => {
            return None;
        }
    };
    Some(f)
}

/// Model that contains the mesh `mesh` of the arrow, together with the node matrix of the mesh
/// (`GameObject_ResolveMeshId` `0x00466A30`, re/combat-projectiles 15.6): the main model `main` of the
/// shooter type, otherwise one of its sub-objects (`FUN_005885F0`; for the archer `pfeil` lies
/// in `..._pfeil_wid.rdo`).
fn arrow_mesh(models: &mut ModelCache, vfs: &Vfs, main: &str, mesh: &str) -> Option<MeshRef> {
    let find = |asset: &ModelAsset| {
        asset
            .rdo
            .meshes
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(mesh))
            .map(|m| MeshRef {
                path: asset.path.clone(),
                node: match m.kind {
                    MeshKind::Rigid => mesh_node_matrix(m),
                    _ => Affine3A::IDENTITY,
                },
            })
    };
    let asset = models.get(vfs, main)?;
    if let Some(found) = find(&asset) {
        return Some(found);
    }
    let subs = asset
        .config
        .sub_objects
        .iter()
        .chain(asset.config.animations.iter().flat_map(|a| &a.sub_objects))
        .map(|s| nml_rda::normalize_path(&s.file));
    for path in subs {
        if let Some(found) = models.get(vfs, &path).and_then(|sub| find(&sub)) {
            return Some(found);
        }
    }
    None
}

/// Mask of the object ID if it belongs to the running animation start: every animation start
/// resets the hiding (`MeshVisibility`).
fn mask_for(masks: &BTreeMap<u32, (u32, Vec<String>)>, id: u32, anim_starts: u32) -> &[String] {
    match masks.get(&id) {
        Some((starts, names)) if *starts == anim_starts => names,
        _ => &[],
    }
}

/// Sets the hide mask of an entry to `wanted`; `true` on a change. The renderer
/// rebuilds the entries with an empty mask after every rebuild of the objects; the per-frame sync
/// then sets it again.
fn sync_mask(hidden: &mut Arc<[String]>, wanted: &[String]) -> bool {
    if **hidden == *wanted {
        return false;
    }
    *hidden = Arc::from(wanted);
    true
}

/// Animation start taken over last (`AnimState::starts`) and animation shown of a
/// renderer entry.
type AnimShown = (u32, i32);

/// Object index -> entry of the renderer.
fn entry_index(renderer: &SceneRenderer) -> HashMap<usize, usize> {
    renderer
        .entries()
        .iter()
        .enumerate()
        .filter_map(|(i, e)| Some((e.object?, i)))
        .collect()
}

/// ID of the simulation object -> index in the object list (rebuilt with the list).
fn sim_index(objects: &[nml_world::level::PlacedObject]) -> HashMap<u32, usize> {
    objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| Some((o.sim_id?, i)))
        .collect()
}

/// Ground markers for the objects `ids` in their order (without GPU, see
/// [`MapView::set_ground_markers`]): objects without an entry in the object list, without a drawn
/// main model or without a project type are omitted.
fn ground_marker_list(
    state: &nml_sim::state::SimState,
    objects: &[nml_world::level::PlacedObject],
    project: &nml_formats::prj::Project,
    entry_of: &HashMap<usize, usize>,
    object_of: &HashMap<u32, usize>,
    ids: &[ObjectId],
) -> Vec<GroundMarker> {
    let mut markers = Vec::with_capacity(ids.len());
    for &id in ids {
        let Some(object) = state.object(id, false) else {
            continue;
        };
        let Some(&index) = object_of.get(&id.0) else {
            continue;
        };
        let (Some(&entry), Some(kind), Some(placed)) = (
            entry_of.get(&index),
            project.objects.get(&object.guid),
            objects.get(index),
        ) else {
            continue;
        };
        markers.push(GroundMarker {
            entry,
            position: placed.position,
            yaw: placed.rotation.y,
            color: marker_color(object.color),
            selection: kind.selection.clone(),
            on_wall: object.status & status::ONWALL != 0,
            raise_to_water: kind.on_water || object.status & status::SWIMMING != 0,
        });
    }
    markers
}

/// For each object of the new list, the object of the old list whose presentation (playback,
/// animation start taken over last) it continues after a rebuild: same ID or,
/// after a change of owner `old -> new` (`renamed`, events of the step), the old ID,
/// in each case only with the same model. New objects and objects without an ID: `None`.
fn carry_over(
    old: &[nml_world::level::PlacedObject],
    new: &[nml_world::level::PlacedObject],
    renamed: &[(u32, u32)],
) -> Vec<Option<usize>> {
    let old_index = sim_index(old);
    new.iter()
        .map(|object| {
            let id = object.sim_id?;
            let source = renamed
                .iter()
                .find(|&&(_, to)| to == id)
                .map_or(id, |&(from, _)| from);
            let &index = old_index.get(&source)?;
            (old[index].model == object.model).then_some(index)
        })
        .collect()
}

/// New locations of the effects on rebuild, per old object ([`EffectWorld::remap_objects`]):
/// own effects go with the object that continues it (`sources` from [`carry_over`], also after
/// a change of owner), carriers of world effects only with the same ID. The ambient objects
/// without an ID at the start of both lists keep their location.
fn effect_moves(
    old: &[nml_world::level::PlacedObject],
    new: &[nml_world::level::PlacedObject],
    sources: &[Option<usize>],
) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let mut owned = vec![None; old.len()];
    let mut carriers = vec![None; old.len()];
    let ambient = |o: &nml_world::level::PlacedObject| o.sim_id.is_none();
    for (i, _) in old
        .iter()
        .zip(new)
        .enumerate()
        .take_while(|(_, (a, b))| ambient(a) && ambient(b) && a.guid == b.guid)
    {
        owned[i] = Some(i);
        carriers[i] = Some(i);
    }
    for (to, &source) in sources.iter().enumerate() {
        let Some(from) = source else {
            continue;
        };
        owned[from] = Some(to);
        if old[from].sim_id == new[to].sim_id {
            carriers[from] = Some(to);
        }
    }
    (owned, carriers)
}

/// ID and owner of the game objects of an object list (comparison before the rebuild).
fn sim_keys(objects: &[nml_world::level::PlacedObject]) -> Vec<(Option<u32>, Option<i32>)> {
    objects.iter().map(|o| (o.sim_id, o.player)).collect()
}

/// What a match builds its map from: a fixed map from its `.lvl` or a
/// random map from its map description (`nml_randommap::build_level`; its path `randommap:<Seed>` is not a
/// file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapSource {
    /// Path of the `.lvl`.
    Level(String),
    /// Menu values of the random map.
    Random(RandomMapSpec),
}

impl MapSource {
    /// Map of a savegame or a replay: the map description, otherwise the `.lvl` path.
    pub fn of_start(start: &StartPackage) -> Self {
        match &start.random_map {
            Some(spec) => Self::Random(spec.clone()),
            None => Self::Level(start.level.clone()),
        }
    }

    /// Map of the running match (restart `0x1F`): the description of the random map from the rules,
    /// otherwise the map path `level` of the match.
    pub fn of_match(rules: &Rules, level: &str) -> Self {
        match &rules.random_map {
            Some(random) => Self::Random(random.spec.clone()),
            None => Self::Level(level.to_string()),
        }
    }

    /// Map path like `LevelScene::lvl_path` (random map: `randommap:<Seed>`).
    pub fn label(&self) -> String {
        match self {
            Self::Level(level) => level.clone(),
            Self::Random(spec) => nml_randommap::random_map_label(spec),
        }
    }

    /// Random map (no `.lvl` path)?
    pub fn is_random(&self) -> bool {
        matches!(self, Self::Random(_))
    }
}

/// A level being loaded (steps of [`MapView::load`], so that a loading screen can report the phases):
/// read ([`MapLoad::read`]), simulation started ([`MapLoad::start_sim`]) or set to a savegame,
/// game objects from the simulation ([`MapLoad::place_objects`]), then the renderer
/// ([`MapView::build`]).
pub struct MapLoad {
    pub scene: LevelScene,
    /// Simulation of the map; `None` if its rules are not loadable (objects of the `.wld`).
    pub sim: Option<SimHost>,
    /// Rules input of a random map ([`MapLoad::open`] with [`MapSource::Random`]); [`MapLoad::start_sim`] uses it
    /// to load the rules without a `.lvl`.
    pub random: Option<RandomMapRules>,
    /// State of the random map for the control command `random_map`, passed on to `GameSession::random_map_state`.
    pub random_state: Option<RandomMapState>,
    /// Path as requested and start of loading (log line of [`MapView::build`]).
    level: String,
    started: Instant,
}

/// Rules of a fixed map without a match (as when loading: read `.lvl`, `Rules::load`): for the start package of the
/// host before loading.
pub fn load_rules(vfs: &Vfs, level: &str) -> Result<Rules> {
    let load = MapLoad::read(vfs, level)?;
    Rules::load(vfs, &load.scene).with_context(|| format!("loading rules of map \"{level}\""))
}

/// Rules of a random map without a match: generate the map from its description
/// ([`MapLoad::open`] with [`MapSource::Random`]) and load its rules ([`Rules::load_random`]) - the same calculation
/// as when loading ([`start_sim_with`]), for the start package of the host. The generator thus runs twice on the host
/// (rules, loading); that costs only time, no state.
pub fn load_random_rules(vfs: &Vfs, spec: &RandomMapSpec) -> Result<Rules> {
    let source = MapSource::Random(spec.clone());
    let load = MapLoad::open(vfs, &source)?;
    let random = load.random.context("random map without rules input")?;
    Rules::load_random(vfs, &load.scene, random)
        .with_context(|| format!("{}: loading rules of the random map", source.label()))
}

impl MapLoad {
    /// Reads the level `level`.
    pub fn read(vfs: &Vfs, level: &str) -> Result<Self> {
        let started = Instant::now();
        let scene = load_level(vfs, level).with_context(|| format!("loading level {level}"))?;
        Ok(Self {
            scene,
            sim: None,
            random: None,
            random_state: None,
            level: level.to_string(),
            started,
        })
    }

    /// Builds the map from `source`: a fixed map like [`MapLoad::read`], a random map via
    /// `nml_randommap::build_level` (generator with object step, scene, rules input; on restart the same as when
    /// loading,
    /// deviation D-341).
    pub fn open(vfs: &Vfs, source: &MapSource) -> Result<Self> {
        let spec = match source {
            MapSource::Level(level) => return Self::read(vfs, level),
            MapSource::Random(spec) => spec,
        };
        let started = Instant::now();
        let label = source.label();
        let level = nml_randommap::build_level(vfs, spec)
            .with_context(|| format!("generating random map {label}"))?;
        Ok(Self {
            random_state: Some(crate::random_map_state::random_map_state(&level.map)),
            random: Some(level.rules),
            scene: level.scene,
            sim: None,
            level: label,
            started,
        })
    }

    /// Takes over a fully built scene without a simulation (random map: `nml_randommap::to_scene`);
    /// the log line names its path (`randommap:<Seed>`). Continue with [`MapView::build`].
    pub fn from_scene(scene: LevelScene) -> Self {
        Self {
            level: scene.lvl_path.clone(),
            scene,
            sim: None,
            random: None,
            random_state: None,
            started: Instant::now(),
        }
    }

    /// Starts the simulation ([`start_sim_with`] with speed `game_speed` and the setup from `setup`, which receives the
    /// rules just loaded); a random map with its rules input ([`MapLoad::random`]).
    pub fn start_sim(
        &mut self,
        vfs: &Vfs,
        game_speed: u8,
        setup: impl FnOnce(&Rules) -> anyhow::Result<MatchSetup>,
    ) -> Result<()> {
        self.sim = start_sim_with(vfs, &self.scene, self.random.as_ref(), game_speed, setup)?;
        Ok(())
    }

    /// The game objects come from the simulation (start objects, owner, position).
    pub fn place_objects(&mut self, vfs: &Vfs) {
        if let Some(sim) = &self.sim {
            self.scene.objects = map_objects(&self.scene, &sim.state, |m| vfs.exists(m));
        }
    }
}

/// Start point of the camera at level start like `GameEngine_InitLevelView` (`0x004458B0`), which calls
/// `GameEngine_CenterCameraOnStart(player)` (`0x00439190`) with the local player. A random map
/// (`random`) has no players in its scene: there the local player of the simulation (`SimHost::active_player`, slot 0
/// in free play) and his first object in `scene.objects` (from the simulation, [`MapLoad::place_objects`]:
/// the HQ created at the start; re/random-map Z.10 item 7). The same on the network (`SimHost::net`, after
/// [`SimHost::enter_rounds`]): there the local player is the own slot from the start, also on fixed maps
/// ([`MapView::center_on_start`]). Fixed maps without a network unchanged via
/// [`start_focus`] (first human of the `.lvl`).
pub(crate) fn start_camera_focus(scene: &LevelScene, sim: Option<&SimHost>, random: bool) -> Vec2 {
    match sim.filter(|s| random || s.net) {
        Some(sim) => start_focus_of(scene, i32::from(sim.active_player)),
        None => start_focus(scene),
    }
}

impl MapView {
    /// Loads the level `level`, starts the simulation ([`MapLoad::start_sim`] with speed `game_speed` and the
    /// setup from `setup`, which receives the rules just loaded), builds the GPU data, sets
    /// the camera as at level start (`GameEngine_ResetCamera`, then
    /// `GameEngine_CenterCameraOnStart`) and sets light, weather and `terrain_quality` from `view`.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        vfs: &Vfs,
        level: &str,
        view: &ViewOptions,
        game_speed: u8,
        setup: impl FnOnce(&Rules) -> anyhow::Result<MatchSetup>,
    ) -> Result<Self> {
        let mut load = MapLoad::read(vfs, level)?;
        load.start_sim(vfs, game_speed, setup)?;
        load.place_objects(vfs);
        Self::build(device, queue, format, vfs, load, view)
    }

    /// Last step of loading ([`MapLoad`]): GPU data, camera as at level start, light, weather and
    /// `terrain_quality` from `view`.
    pub fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        vfs: &Vfs,
        load: MapLoad,
        view: &ViewOptions,
    ) -> Result<Self> {
        let MapLoad {
            mut scene,
            sim,
            level,
            started,
            random,
            ..
        } = load;
        let presets = presets_or_empty(load_light_presets(vfs));
        // With a running level script no start tables: light and weather come from the events.
        let scripted = sim.as_ref().is_some_and(SimHost::runs_level_script);
        let light = resolve_light(&view.light, &scene.lvl_path, &presets, scripted)?;
        scene.apply_light(light.map(|i| &presets[i]));
        let mut models = ModelCache::new();
        let mut renderer = SceneRenderer::new(device, queue, format, vfs, &scene, &mut models);
        // `Terrain+0x05` = 0 from `TerrainQuality` 2: water in detail texture mode.
        renderer.set_water_detail(view.terrain_quality >= 2);
        renderer.set_level_shadows(device, queue, vfs, &mut models, &scene, view.shadow_quality);
        let generator = crate::ui_layer::atlas_generator(vfs);
        let text = OverlayText::new(device, format, vfs, view.text_width, generator.as_deref())?;
        let mut camera = GameCamera::new(CameraSetup::load(vfs), &scene.terrain);
        let focus = start_camera_focus(&scene, sim.as_ref(), random.is_some());
        camera.move_to(focus.x, focus.y, &scene.terrain);
        log::info!(
            "{level}: {} objects drawn, loaded in {:.1} s",
            renderer.instance_count(),
            started.elapsed().as_secs_f32()
        );
        let quadtree = TerrainQuadtree::new(&scene.terrain);
        let entry_of = entry_index(&renderer);
        let object_of = sim_index(&scene.objects);
        let sim_starts = vec![None; renderer.entries().len()];
        let track_tiles = track_tiles(&scene);
        let mut map = Self {
            scene,
            renderer,
            text,
            camera,
            free: None,
            presets,
            light,
            weather: None,
            script_fog: ScriptFog::default(),
            weather_rng: MsvcRand::new(WEATHER_SEED),
            quadtree,
            weather_moved: 0,
            models,
            shadow_quality: view.shadow_quality,
            effects: EffectWorld::new(),
            effect_list: load_effect_list(vfs),
            particles_quality: view.particles_quality,
            effect_boxes: HashMap::new(),
            dummies_pending: Vec::new(),
            entry_of,
            object_of,
            effect_carry: 0.0,
            effects_below: EffectFrame::default(),
            effects_above: EffectFrame::default(),
            sim,
            fog_factors: FogFactors::default(),
            memory_picks: BTreeMap::new(),
            memory_shapes: HashMap::new(),
            memory_decals: HashMap::new(),
            sim_starts,
            notices: Notices::load(vfs),
            flights: Flights::default(),
            mesh_masks: BTreeMap::new(),
            clock_ms: None,
            tracked_step: 0,
            flight_carry: 0.0,
            arrow_models: HashMap::new(),
            building_fires: BuildingFires::default(),
            train_sparks: TrainSparks::default(),
            heal_effects: HealEffects::default(),
            ability_effects: AbilityEffects::default(),
            world_effects: WorldEffects::default(),
            temp_effects: TempUpgradeEffects::default(),
            movement_effects: MovementEffects::default(),
            destruction_marks: DestructionMarks::default(),
            track_tiles,
            rally_flag: None,
            rally_draw: None,
        };
        let weather = resolve_weather(view.weather, &map.scene.lvl_path, scripted);
        map.set_weather(weather);
        map.set_particles_quality(vfs, view.particles_quality);
        Ok(map)
    }

    /// Network match: sets the camera to the start of the own slot
    /// ([`start_camera_focus`]) after [`SimHost::enter_rounds`] has set it - at build time ([`Self::build`])
    /// the local player was still the first human of the map. As in the original `GameEngine_CenterCameraOnStart`
    /// (`0x00439190`) with the local player; before `Go` nobody has moved the camera (input locked). Without a
    /// network it has no effect.
    pub fn center_on_start(&mut self) {
        let Some(sim) = self.sim.as_ref().filter(|s| s.net) else {
            return;
        };
        let focus = start_camera_focus(&self.scene, Some(sim), false);
        self.camera.move_to(focus.x, focus.y, &self.scene.terrain);
    }

    /// Applies a light (`None` = level light): hard switch like
    /// `GameEngine_ApplyLightPreset`.
    pub fn set_light(&mut self, queue: &wgpu::Queue, light: Option<usize>) {
        self.light = light.filter(|&i| i < self.presets.len());
        self.scene.apply_light(self.light.map(|i| &self.presets[i]));
        self.renderer.set_light(queue, &self.scene);
    }

    /// Light by name (`level` = `.wld`, otherwise a preset, case-insensitive).
    pub fn set_light_named(&mut self, queue: &wgpu::Queue, name: &str) -> Result<()> {
        let light = if name.eq_ignore_ascii_case(LEVEL_LIGHT_NAME) {
            None
        } else {
            resolve_light(&LightSetting::Preset(name.into()), "", &self.presets, false)?
        };
        self.set_light(queue, light);
        Ok(())
    }

    /// Names in the order of F6: `level`, then the presets of the file.
    pub fn light_names(&self) -> Vec<String> {
        std::iter::once(LEVEL_LIGHT_NAME.to_string())
            .chain(self.presets.iter().map(|p| p.name.clone()))
            .collect()
    }

    /// F6 / Shift+F6.
    pub fn step_light(&mut self, queue: &wgpu::Queue, step: i32) {
        self.set_light(queue, step_light(self.light, self.presets.len(), step));
    }

    /// Name of the active light (`level` or name of the preset).
    pub fn light_name(&self) -> &str {
        self.light
            .map_or(LEVEL_LIGHT_NAME, |i| self.presets[i].name.as_str())
    }

    /// Light setting of a script (171, `GameEngine_ApplyLightPreset`): the preset `name`,
    /// case-insensitive; an unknown name changes nothing (`false`). Unlike
    /// [`MapView::set_light_named`], `level` is not a special name here.
    pub fn set_script_light(&mut self, queue: &wgpu::Queue, name: &str) -> bool {
        let Some(preset) = find_preset(&self.presets, name) else {
            return false;
        };
        let index = self.presets.iter().position(|p| std::ptr::eq(p, preset));
        self.set_light(queue, index);
        true
    }

    /// Action 381 (E-236): only the object light back to the `.wld`, the terrain keeps the setting.
    pub fn restore_object_light(&mut self, queue: &wgpu::Queue) {
        self.scene.restore_object_light();
        self.renderer.set_light(queue, &self.scene);
    }

    /// Fog values that currently apply (output of a crossfade 506): those of the script, otherwise those of the
    /// `.wld` like the camera constructor and `[FOG]` (color `0xFFRRGGBB`, 0 without `[FOG]`; start otherwise
    /// `FOGSTART` of `CameraSetup.ini`; length otherwise 1000 like [`GameCamera::fog_range`]).
    pub fn fog_values(&self) -> FogValues {
        self.script_fog.values.unwrap_or_else(|| {
            FogValues::of_level(
                self.scene.world.fog.as_ref(),
                self.camera.setup.default_fog_start,
            )
        })
    }

    /// Switches the weather like `GameEngine_SetWeather`: particles are created
    /// anew on every switch-on (the weather's random generator keeps running).
    pub fn set_weather(&mut self, kind: Option<WeatherKind>) {
        self.weather =
            kind.map(|kind| Weather::new(kind, &self.scene.terrain, &mut self.weather_rng));
        self.weather_moved = 0;
    }

    /// F7: off -> rain -> snow -> off.
    pub fn cycle_weather(&mut self) {
        self.set_weather(next_weather(self.weather.as_ref().map(Weather::kind)));
    }

    /// Moves the weather particles of the visible tiles by `dt_ms` (logic step).
    pub fn advance_weather(&mut self, view_proj: Mat4, dt_ms: f32) {
        self.weather_moved = match &mut self.weather {
            Some(weather) => {
                let rects = self
                    .quadtree
                    .visible_rects(&Frustum::from_view_proj(view_proj));
                weather.advance(&rects, dt_ms)
            }
            None => 0,
        };
    }

    /// Hands the weather particles of the visible tiles for one frame to the renderer (before
    /// [`MapView::render_world`]; afterwards `renderer.weather_count()` returns their number).
    /// The state after the last logic step is drawn (original: before moving in the
    /// same frame; the difference is one frame).
    pub fn prepare_weather(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &ViewParams,
    ) {
        let centers = self.weather.as_ref().map_or_else(Vec::new, |weather| {
            let frustum = Frustum::from_view_proj(params.projection * params.view);
            weather.positions(&self.quadtree.visible_rects(&frustum))
        });
        let kind = self.weather.as_ref().map(Weather::kind);
        self.renderer.set_weather(device, queue, kind, &centers);
    }

    /// Sets `ShadowQuality` and recomputes the shadows of all objects.
    pub fn set_shadow_quality(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        quality: u8,
    ) {
        self.shadow_quality = quality;
        self.renderer
            .set_level_shadows(device, queue, vfs, &mut self.models, &self.scene, quality);
    }

    /// Sets `ParticlesQuality` and recreates the effects as if the map were reloaded
    /// (ambient effects from 1, permanent effects from 2); game time and randomness of the effects keep running.
    pub fn set_particles_quality(&mut self, vfs: &Vfs, quality: u8) {
        self.particles_quality = quality;
        self.effects.clear();
        // Wheel dust and wheel track keys belong to the old effect world.
        let steps = self.sim.as_ref().map_or(0, |sim| sim.state.steps);
        self.movement_effects.reset(&mut self.effects, steps);
        self.destruction_marks.reset();
        let unfinished: Vec<bool> = (0..self.scene.objects.len())
            .map(|i| self.under_construction(i))
            .collect();
        populate_level_with(
            &mut self.effects,
            &self.scene,
            &self.effect_list,
            vfs,
            &mut self.models,
            quality,
            &|i| !unfinished[i],
        );
        self.dummies_pending.clear();
        if quality >= 2 {
            self.dummies_pending.extend(
                self.scene
                    .objects
                    .iter()
                    .zip(&unfinished)
                    .filter_map(|(o, &u)| o.sim_id.filter(|_| u)),
            );
        }
        self.effect_boxes.clear();
        self.add_effect_boxes(vfs);
    }

    /// Is the object at location `index` a building of the simulation that is still a construction site
    /// (`PropertyBuilding_Init` `0x0050B150` attaches the permanent effects only to finished ones)?
    fn under_construction(&self, index: usize) -> bool {
        let Some(sim) = &self.sim else {
            return false;
        };
        self.scene.objects[index]
            .sim_id
            .and_then(|id| sim.state.object(ObjectId(id), false))
            .and_then(|o| o.module(building::INDEX))
            .is_some_and(|m| {
                matches!(m.kind, ModuleKind::Building(_))
                    && !m.base.has_flags(building::flags::CONSTRUCTED_FINISHED)
            })
    }

    /// Rest hulls of the objects with own effects that do not have one yet.
    fn add_effect_boxes(&mut self, vfs: &Vfs) {
        for slot in self.effects.objects() {
            let object = &self.scene.objects[slot.object];
            if let Some(asset) = self.models.get(vfs, &object.model) {
                self.effect_boxes
                    .entry(slot.object)
                    .or_insert_with(|| mesh_box(&asset, None).unwrap_or(Aabb::ZERO));
            }
        }
    }

    /// Rebuild of the object list without restarting the effects (after creating, destroying and
    /// change of owner): as in the original the own effects hang on the object and keep running
    /// as long as it lives. `sources` names for each new object the old one that continues it
    /// ([`carry_over`]); world effects follow their carrier only with the same ID (they look for it
    /// via the ID, which a change of owner assigns anew). Vanished objects release their
    /// effects ([`EffectWorld::remap_objects`]), new ones get them as on placement
    /// ([`populate_object`]). Ambient effects are at the front of both lists and stay as they are.
    fn carry_effects(
        &mut self,
        vfs: &Vfs,
        old: &[nml_world::level::PlacedObject],
        sources: &[Option<usize>],
    ) {
        let (owned, carriers) = effect_moves(old, &self.scene.objects, sources);
        self.effects.remap_objects(&owned, &carriers);
        self.effect_boxes = std::mem::take(&mut self.effect_boxes)
            .into_iter()
            .filter_map(|(o, b)| Some((owned.get(o).copied().flatten()?, b)))
            .collect();
        let quality = self.particles_quality;
        for index in 0..self.scene.objects.len() {
            let Some(id) = self.scene.objects[index].sim_id else {
                continue;
            };
            if sources.get(index).copied().flatten().is_some() {
                continue;
            }
            let unfinished = self.under_construction(index);
            populate_object(
                &mut self.effects,
                &self.scene,
                index,
                &self.effect_list,
                vfs,
                &mut self.models,
                quality,
                !unfinished,
            );
            if unfinished && quality >= 2 {
                self.dummies_pending.push(id);
            }
        }
        self.add_effect_boxes(vfs);
    }

    /// Permanent effects of the buildings that have been finished since the last call
    /// (`PropertyBuilding_OnBuildTick` `0x00508FB0` calls `GameObject_AttachDummyEffects` at the end of
    /// construction); vanished ones drop out of the list.
    fn attach_finished_dummies(&mut self, vfs: &Vfs) {
        if self.dummies_pending.is_empty() {
            return;
        }
        let mut added = false;
        for id in std::mem::take(&mut self.dummies_pending) {
            let Some(&index) = self.object_of.get(&id) else {
                continue;
            };
            if self.under_construction(index) {
                self.dummies_pending.push(id);
                continue;
            }
            attach_dummies(
                &mut self.effects,
                &self.scene,
                index,
                &self.effect_list,
                vfs,
                &mut self.models,
            );
            added = true;
        }
        if added {
            self.add_effect_boxes(vfs);
        }
    }

    /// Advance the effects by `dt_ms` (logic step, camera of this step): game time in
    /// whole milliseconds with carry of the fractions; poses at clock time `clock_ms`
    /// (`None` = bind pose).
    pub fn advance_effects(
        &mut self,
        vfs: &Vfs,
        view_proj: Mat4,
        dt_ms: f32,
        clock_ms: Option<f32>,
    ) {
        // Before the step, so that freshly (re)created ships - loading, restart, rebuild
        // after creating/destroying - already have position and moving state in the first tick.
        self.sync_ships();
        let total = self.effect_carry + f64::from(dt_ms);
        let whole = total.floor();
        self.effect_carry = total - whole;
        {
            let fog = fog_probe(self.sim.as_ref());
            let objects = MapObjects {
                scene: &self.scene,
                entries: self.renderer.entries(),
                entry_of: &self.entry_of,
                boxes: &self.effect_boxes,
                frustum: Frustum::from_view_proj(view_proj),
                now_ms: clock_ms,
                fog: &fog,
            };
            let frustum = Frustum::from_view_proj(view_proj);
            self.effects
                .step(whole as u32, &frustum, &objects, vfs, &mut self.models);
        }
        self.clock_ms = clock_ms;
        // Trajectories run on game time like the simulation (GameSpeed), not on the clock.
        let factor = self
            .sim
            .as_ref()
            .map_or(1.0, |sim| SimHost::game_time_factor(sim.game_speed));
        let game_dt =
            crate::game::projectiles::game_ms(&mut self.flight_carry, f64::from(dt_ms), factor);
        self.advance_flights(vfs, game_dt);
    }

    /// Advance the projectile trajectories by `dt` ms (whole effect milliseconds): smoke puffs of the ball,
    /// `kugel.pef` from the visible quarter, move effects to the position, remove finished trajectories.
    fn advance_flights(&mut self, vfs: &Vfs, dt: f32) {
        let mut finished = Vec::new();
        for f in &mut self.flights.list {
            let out = f.advance(dt);
            for (pos, start) in out.puffs {
                let p = nml_effects::particles::Placement {
                    position: pos,
                    yaw: 0.0,
                };
                self.effects.add_game_effect(
                    vfs,
                    &mut self.models,
                    "data/effects/kugelrauch.pef",
                    Anchor::Fixed(p),
                    None,
                    1.0,
                    start,
                    UNLIMITED_MS,
                );
            }
            // Ball: `kugel.pef` appears as soon as it becomes visible (first quarter invisible, C-111);
            // so a ball rebuilt after loading also gets its effect.
            if out.ball_visible && f.effect.is_none() {
                let p = nml_effects::particles::Placement {
                    position: f.pos,
                    yaw: f.yaw,
                };
                f.effect = self.effects.add_game_effect(
                    vfs,
                    &mut self.models,
                    "data/effects/kugel.pef",
                    Anchor::Fixed(p),
                    None,
                    1.0,
                    0,
                    UNLIMITED_MS,
                );
            }
            if let Some(key) = f.effect {
                let yaw = match f.kind {
                    Kind::Ball { yaw_extra } => f.yaw + yaw_extra,
                    Kind::Arrow { .. } | Kind::Throw => f.yaw,
                };
                self.effects.move_game_effect(
                    key,
                    nml_effects::particles::Placement {
                        position: f.pos,
                        yaw,
                    },
                );
            }
            if f.done() {
                finished.push(f.effect);
            }
        }
        for key in finished.into_iter().flatten() {
            self.effects.remove_game_effect(key);
        }
        self.flights.list.retain(|f| !f.done());
    }

    /// Hand the effects of a frame to the renderer (before [`MapView::render_world`]).
    pub fn prepare_effects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        params: &ViewParams,
        clock_ms: Option<f32>,
    ) {
        let fog = fog_probe(self.sim.as_ref());
        let objects = MapObjects {
            scene: &self.scene,
            entries: self.renderer.entries(),
            entry_of: &self.entry_of,
            boxes: &self.effect_boxes,
            frustum: Frustum::from_view_proj(params.projection * params.view),
            now_ms: clock_ms,
            fog: &fog,
        };
        self.effects.draw(
            params.view,
            &objects,
            &mut self.effects_below,
            &mut self.effects_above,
        );
        flight_draws(&self.flights, &fog, &mut self.effects_above);
        // Dynamite charges: sync tracks (after loading before the first step), track the target
        // (`0x004F0480`) and draw without a fog check (`0x004F0620`, D-281).
        if let Some(sim) = &self.sim {
            self.world_effects.sync_charges(sim);
            let wanted = WorldEffects::wanted(&sim.state, &sim.rules);
            charge_draws(
                &wanted,
                self.world_effects.charges(),
                &mut self.effects_above,
            );
        }
        self.renderer.set_effects(
            device,
            queue,
            vfs,
            &mut self.models,
            &self.effects_below,
            &self.effects_above,
        );
        // Layer 4 (step 7): footprints, craters, wheel tracks; corners of the wheel tracks at
        // `Terrain_GetHeightAt` (`0x0055A940`).
        let terrain = &self.scene.terrain;
        let marks = self
            .effects
            .ground_marks(&|x, z| terrain.ground_height(x, z), &fog);
        self.renderer
            .set_ground_marks(device, queue, vfs, terrain, &marks);
    }

    /// Control command `spawn_effect`: world effect at `position`, at object `object` (index into the
    /// level's object list like `hovered.index`; follows it) or at the camera target, unlimited
    /// (one-shot effects end by themselves).
    pub fn spawn_effect(
        &mut self,
        vfs: &Vfs,
        path: &str,
        position: Option<Vec3>,
        object: Option<usize>,
        scale: f32,
    ) -> Result<(), String> {
        // The scene omits start points, unknown types and others: `hovered.index` is the index
        // of the level object list, the carrier is the location in the scene.
        let object = object
            .map(|index| scene_position(self.scene.objects.iter().map(|o| o.index), index))
            .transpose()?;
        let position = position.unwrap_or(self.camera.target);
        let placement = nml_effects::particles::Placement { position, yaw: 0.0 };
        if let Some(o) = object {
            self.ensure_effect_box(vfs, o);
        }
        self.effects
            .spawn(
                vfs,
                &mut self.models,
                path,
                nml_effects::attached::UNLIMITED_MS,
                placement,
                object,
                scale,
            )
            .map_err(|e| format!("effect {path} {e}"))
    }

    /// Carrier with a rest hull so that its visibility is known (`o` = location in
    /// `scene.objects`).
    fn ensure_effect_box(&mut self, vfs: &Vfs, o: usize) {
        let model = self.scene.objects[o].model.clone();
        if let Some(asset) = self.models.get(vfs, &model) {
            self.effect_boxes
                .entry(o)
                .or_insert_with(|| mesh_box(&asset, None).unwrap_or(Aabb::ZERO));
        }
    }

    /// Number of projectile trajectories of the host (debug display, control channel).
    pub fn flight_count(&self) -> usize {
        self.flights.len()
    }

    /// Counters of the effects.
    pub fn effect_counts(&self) -> EffectCounts {
        self.effects.counts()
    }

    /// Prepare the shadows of a frame (after [`MapView::update`]): visible tiles and
    /// objects of this frame.
    pub fn prepare_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &ViewParams,
    ) {
        let frustum = Frustum::from_view_proj(params.projection * params.view);
        let visible = TileMask::from_rects(
            &self.quadtree.visible_rects(&frustum),
            self.scene.terrain.dim_x(),
            self.scene.terrain.dim_z(),
        );
        self.renderer
            .prepare_shadows(device, queue, &self.scene.terrain, &visible, params);
    }

    /// Counters of the shadows of the last frame.
    pub fn shadow_stats(&self) -> ShadowStats {
        self.renderer.shadow_stats()
    }

    /// Base module (`None` without a type) and shadow kind of an object (`LevelScene::objects`).
    pub fn object_class(&self, index: usize) -> (Option<&'static str>, u8) {
        let object = &self.scene.objects[index];
        let base = object
            .parts
            .first()
            .and_then(|&g| self.scene.types.class(g))
            .and_then(|c| c.base)
            .map(|b| b.name());
        let kind = self.scene.object_shadow(index, self.shadow_quality).kind;
        (base, kind.number())
    }

    /// Class and shadow kind of an object for the overlay.
    pub fn describe_class(&self, index: usize) -> String {
        let (base, kind) = self.object_class(index);
        format!("{}, shadow kind {kind}", base.unwrap_or("no type"))
    }

    /// Overlay line of the simulation.
    pub fn sim_line(&self) -> String {
        self.sim.as_ref().map_or_else(
            || "Simulation: off (rules not loaded)".to_string(),
            SimHost::status_line,
        )
    }

    /// "Ship stands" effects follow the simulation (`Unit_UpdateMovementEffects`: position from `+0x08`,
    /// water from the ground at that position). "Moving" = `status::MOVE` is an assumption
    /// (Ghidra check open, re/pathfinding). Runs at the start of
    /// [`MapView::advance_effects`]: the order per logic step is simulation -> rebuild
    /// of the objects (recreates the ships as "standing") -> effects; without this sync a
    /// moving ship would create a ring after every rebuild.
    fn sync_ships(&mut self) {
        let Some(sim) = &self.sim else {
            return;
        };
        for o in self.effects.ship_objects() {
            let placed = &self.scene.objects[o];
            let Some(object) = placed
                .sim_id
                .and_then(|id| sim.state.object(ObjectId(id), false))
            else {
                continue;
            };
            let (position, rotation) = sim.display(object);
            let on_water = self.scene.terrain.ground_height(position.x, position.z)
                < self.scene.terrain.params.water_level;
            let moving = ship_is_moving(object);
            self.effects
                .update_ship(o, position, rotation.y, on_water, moving);
        }
    }

    /// Poses of the objects for the time `now_ms` (with `animate`), otherwise bind pose; position
    /// and animations beforehand from the simulation ([`MapView::sync_sim`]).
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        now_ms: f32,
        animate: bool,
    ) {
        self.sync_sim(now_ms);
        self.sync_fog(queue, now_ms);
        self.sync_fog_memories(device, queue, vfs, now_ms);
        self.renderer.update(device, queue, vfs, now_ms, animate);
    }

    /// Active fog memories of the active player (re/vision 9): per memory the
    /// main model of the remembered type with the remembered world matrix, render bits and progress,
    /// for special-class types (gold deposit, fertile soil) additionally their decal at the remembered
    /// position; brightness as for the object (owner of the memory, tile of the memory) at clock time
    /// `now_ms`. Without a simulation none.
    fn sync_fog_memories(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        now_ms: f32,
    ) {
        let mut pieces = Vec::new();
        self.memory_picks.clear();
        if let Some(sim) = &mut self.sim {
            let viewer = sim.active_player;
            for m in sim.memories.active() {
                let Some(snap) = &m.snapshot else {
                    continue;
                };
                let Some(kind) = self.scene.project.objects.get(&snap.guid) else {
                    continue;
                };
                let Some(asset) = self.models.get(vfs, &nml_rda::normalize_path(&kind.model))
                else {
                    continue;
                };
                let brightness =
                    sim.fog
                        .brightness_at(snap.id.slot(), m.tile, viewer, now_ms as u32);
                // Player color of the remembered object: that of the living object, otherwise the one last seen.
                let memory_color = sim
                    .state
                    .object(snap.id, false)
                    .map_or(m.color, |o| o.color);
                let world = object_world_matrix(
                    Vec3::from(snap.world_pos),
                    Vec3::from(snap.rotation),
                    Vec3::from(snap.scale),
                );
                let models = &mut self.models;
                let (subs, hull) =
                    self.memory_shapes
                        .entry(asset.path.clone())
                        .or_insert_with(|| {
                            let playback =
                                Playback::new(&asset, |file| models.get(vfs, file), None, 0.0);
                            let subs = playback
                                .active_subs(false)
                                .map(|sub| sub.asset.clone())
                                .collect();
                            (subs, instance_box(&asset, &playback, None, 0.0))
                        });
                let parts = std::iter::once(asset.clone())
                    .chain(subs.iter().cloned())
                    .collect();
                self.memory_picks.insert(
                    snap.id,
                    MemoryPick {
                        parts,
                        world,
                        hull: *hull,
                    },
                );
                let scene = &self.scene;
                let special_decals = self
                    .memory_decals
                    .entry(snap.guid)
                    .or_insert_with(|| memory_special_decals(scene, snap.guid))
                    .clone();
                pieces.push(MemoryPiece {
                    asset,
                    world,
                    look: EntryLook {
                        bits: snap.render_bits,
                        progress: snap.progress,
                        brightness,
                        alpha: 1.0,
                        color: Some(player_rgb(memory_color)),
                    },
                    position: Vec3::from(snap.world_pos),
                    yaw: snap.rotation[1],
                    special_decals,
                });
            }
        }
        self.renderer
            .set_fog_memories(device, queue, vfs, &mut self.models, &pieces);
    }

    /// Brightness of the fog of war for terrain and water at clock time `now_ms` of the
    /// virtual clock (crossfade `0x00465F60`; stopped clock: no crossfade). Without a
    /// simulation factor 1; the renderer writes only what changed.
    fn sync_fog(&mut self, queue: &wgpu::Queue, now_ms: f32) {
        let fog = match &mut self.sim {
            Some(sim) => {
                sim.fog
                    .terrain_factors(now_ms as u32, &mut self.fog_factors);
                Some(&self.fog_factors)
            }
            None => None,
        };
        self.renderer.set_fog(queue, &self.scene, fog);
    }

    /// Takes over position (interpolated between the previous and last tick) and animations of the
    /// simulation objects into the scene and renderer. A new animation start of the simulation
    /// (`AnimState::starts`) starts the animation at clock time `now_ms` - start time, blended
    /// as requested; a crossfade rejected by the 200 ms lock is repeated in the next frame. Objects
    /// whose module does not yet set the animation keep the idle animation ([`DisplayAnim::Idle`]).
    fn sync_sim(&mut self, now_ms: f32) {
        let Some(sim) = &mut self.sim else {
            return;
        };
        let mut moved = Vec::new();
        for (i, entry) in self.renderer.entries_mut().iter_mut().enumerate() {
            let Some(placed) = entry.object.map(|o| &mut self.scene.objects[o]) else {
                continue;
            };
            let Some(id) = placed.sim_id.map(ObjectId) else {
                continue;
            };
            // Render bits after fog and progress (construction site, burning, invisible fishing baskets,
            // occupants), brightness at the presentation clock; writes only the alpha table.
            let Some(look) = sim.entry_look(id, now_ms as u32) else {
                continue;
            };
            entry.look = look;
            let Some(object) = sim.state.object(id, false) else {
                continue;
            };
            // Hide mask of the game logic (arrow in hand): before the animation sync below,
            // which skips unchanged starts; also sets it again after a rebuild of the objects
            // (the entries start with an empty mask).
            sync_mask(
                &mut entry.hidden_meshes,
                mask_for(&self.mesh_masks, id.0, object.anim.starts),
            );
            let (position, rotation) = sim.display(object);
            if (position, rotation) != (placed.position, placed.rotation) {
                moved.push((i, position, rotation.y));
            }
            // Module 6 keeps invisible objects hittable, hidden ones not (no render list); death deletes
            // the ship flag for good.
            entry.resource = object.has_module(6, false)
                && !crate::game::sim_host::hidden_from_view(&sim.state, object);
            entry.ship &= object.status & nml_sim::object::status::DIED == 0;
            entry.status = object.status;
            placed.position = position;
            placed.rotation = rotation;
            placed.transform =
                object_world_matrix(placed.position, placed.rotation, Vec3::from(object.scale));
            entry.world = placed.transform;
            // Animation for the active player (alarm post per viewer, re/masking 7).
            let DisplayAnim::Sim(anim) =
                viewer_anim(&sim.state, &sim.rules, object, sim.active_player)
            else {
                continue;
            };
            // Speed of the simulation (128 = normal), e.g. walk animation according to speed; the wheels of the
            // train turn according to the locomotive's driving speed (`SimHost::train_wheel_rate`).
            let rate = object.anim.speed / 128.0;
            match sim.train_wheel_rate(object) {
                None => entry.playback.set_rate(rate, now_ms),
                Some(WheelRate::Derived(wheels)) => entry.playback.set_rates(wheels, rate, now_ms),
                Some(WheelRate::Keep) => {}
            }
            // A new start of the simulation or a different choice of the viewer restarts
            // (time and crossfade remain those of the simulation start).
            let starts = (object.anim.starts, anim);
            if self.sim_starts[i] == Some(starts) {
                continue;
            }
            let index = usize::try_from(anim).ok();
            let started = now_ms - object.anim.start_ms as f32;
            let accepted = if object.anim.blend {
                entry.playback.play(index, started)
            } else {
                entry.playback.play_hard(index, started);
                true
            };
            if accepted {
                self.sim_starts[i] = Some(starts);
            }
        }
        // Track the shadows of moving units.
        for (i, position, yaw) in moved {
            let world = self.renderer.entries()[i].world;
            self.renderer.follow_shadow(i, position, yaw, world);
        }
    }

    /// Follows the events of a simulation step: creating, destroying and change of owner
    /// rebuild the object list if IDs or owners of the drawn objects have
    /// changed (models and shadows anew; playback, animation start and effects of
    /// remaining objects carry over via [`carry_over`], new objects get their effects as
    /// on placement). Buildings that have just been finished get their permanent effects.
    /// After that the effects of the simulation start (explosion of a destroyed building) as a
    /// world effect at the object's position and the rank effects (`EffectOnObject`) at the object.
    /// Fishing baskets that have become visible (`SimEvent::Revealed`) are shown by [`MapView::update`]
    /// via their render bits.
    pub fn apply_sim_events(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        events: &[SimEvent],
    ) {
        let lifecycle = events.iter().any(|e| {
            matches!(
                e,
                SimEvent::Created { .. }
                    | SimEvent::Destroyed { .. }
                    | SimEvent::OwnerChanged { .. }
            )
        });
        if lifecycle {
            let renamed: Vec<(u32, u32)> = events
                .iter()
                .filter_map(|e| match e {
                    SimEvent::OwnerChanged { old, new } => Some((old.0, new.0)),
                    _ => None,
                })
                .collect();
            self.movement_effects.rename(&renamed);
            self.refresh_sim_objects(device, queue, vfs, false, &renamed);
        }
        self.attach_finished_dummies(vfs);
        for event in events {
            match event {
                // Smoke and crater of a demolition (`Building_SpawnDestructionSmoke` `0x0046C350`,
                // before its explosion) and crater of a cannonball (`0x004EF7C2`).
                SimEvent::BuildingDestroyed { .. } | SimEvent::Crater { .. } => {
                    self.apply_destruction_marks(vfs, std::slice::from_ref(event));
                }
                SimEvent::Effect { object, path } => {
                    let Some(position) = self.sim.as_ref().and_then(|sim| {
                        let o = sim.state.object(*object, false)?;
                        Some(sim.display(o).0)
                    }) else {
                        continue;
                    };
                    if let Err(e) = self.spawn_effect(vfs, path, Some(position), None, 1.0) {
                        log::warn!("{e}");
                    }
                }
                // Own effect on the object (rank-up): follows it. It starts after the
                // rebuild above so that it already carries the location of the new object list.
                SimEvent::EffectOnObject { object, path } => {
                    let Some(index) = self
                        .object_of
                        .get(&object.0)
                        .map(|&i| self.scene.objects[i].index)
                    else {
                        continue;
                    };
                    if let Err(e) = self.spawn_effect(vfs, path, None, Some(index), 1.0) {
                        log::warn!("{e}");
                    }
                }
                // World effect of the combat simulation (impact, sparks, explosion parts).
                SimEvent::EffectAt {
                    path,
                    pos,
                    yaw,
                    scale,
                    start_ms,
                } => {
                    // Explosion, storm and clearing up sit at ground height (D-237).
                    let lifted =
                        ground_lift(path, *pos, |x, z| self.scene.terrain.ground_height(x, z));
                    // The dynamite explosion at the charge position of the last frame (`0x004F06A0`,
                    // D-282), not at the placement position of the simulation.
                    let position = self
                        .sim
                        .as_ref()
                        .and_then(|sim| self.world_effects.explosion_at(path, *pos, &sim.state))
                        .unwrap_or(Vec3::from(lifted));
                    let placement = nml_effects::particles::Placement {
                        position,
                        yaw: *yaw,
                    };
                    if self
                        .effects
                        .add_game_effect(
                            vfs,
                            &mut self.models,
                            path,
                            Anchor::Fixed(placement),
                            None,
                            *scale,
                            *start_ms,
                            UNLIMITED_MS,
                        )
                        .is_none()
                    {
                        log::warn!("effect {path} not loadable");
                    }
                }
                // Effect on the object (muzzle fire at the helper node, blood at the hit zone).
                SimEvent::EffectAttached {
                    object,
                    path,
                    node,
                    offset,
                } => {
                    // Carrier = location in `scene.objects` (like `spawn_effect` after `scene_position`).
                    let Some(carrier) = self.object_of.get(&object.0).copied() else {
                        continue;
                    };
                    self.ensure_effect_box(vfs, carrier);
                    // Blood switch of the program (`Localisation_IsBloodEnabled`, O-392): `no_blood.pef` instead of
                    // `blood.pef` after the cheat `bloodmode`.
                    let path = crate::game::blood::effect_path(path, crate::game::blood::enabled());
                    let anchor = if node.is_empty() {
                        Anchor::Object {
                            offset: Vec3::from(*offset),
                            with_rotation: false,
                        }
                    } else {
                        Anchor::Node(node.clone())
                    };
                    if self
                        .effects
                        .add_game_effect(
                            vfs,
                            &mut self.models,
                            path,
                            anchor,
                            Some(carrier),
                            1.0,
                            0,
                            UNLIMITED_MS,
                        )
                        .is_none()
                    {
                        log::warn!("effect {path} not loadable");
                    }
                }
                // Own effect of the building at a fixed position (muzzle fire or cannon smoke of a
                // quartered shooter at the shot position).
                SimEvent::EffectOwnedAt {
                    object,
                    path,
                    pos,
                    aim,
                } => {
                    let Some(carrier) = self.object_of.get(&object.0).copied() else {
                        continue;
                    };
                    if self
                        .effects
                        .add_game_effect(
                            vfs,
                            &mut self.models,
                            path,
                            Anchor::Fixed(owned_effect_placement(*pos, *aim)),
                            Some(carrier),
                            1.0,
                            0,
                            UNLIMITED_MS,
                        )
                        .is_none()
                    {
                        log::warn!("effect {path} not loadable");
                    }
                }
                SimEvent::ProjectileLaunched {
                    id,
                    kind,
                    fire,
                    shooter,
                    shooter_guid,
                    model,
                    node,
                    start,
                    target_pos,
                    flight_ms,
                    yaw_extra,
                    ..
                } => {
                    let launch = Launch {
                        id: *id,
                        kind: *kind,
                        fire: *fire,
                        shooter: *shooter,
                        guid: *shooter_guid,
                        model,
                        node,
                        start: Vec3::from(*start),
                        aim: Vec3::from(*target_pos),
                        flight_ms: *flight_ms,
                        yaw_extra: *yaw_extra,
                    };
                    self.launch_flight(vfs, &launch);
                }
                SimEvent::ProjectileEnded { id, hit, .. } => self.end_flight(*id, *hit),
                SimEvent::MeshVisibility {
                    object,
                    names,
                    visible,
                    anim_starts,
                } => {
                    let entry = self
                        .mesh_masks
                        .entry(object.0)
                        .or_insert((*anim_starts, Vec::new()));
                    if entry.0 != *anim_starts {
                        *entry = (*anim_starts, Vec::new());
                    }
                    for n in names.split_whitespace().map(str::to_ascii_lowercase) {
                        entry.1.retain(|x| *x != n);
                        if !visible {
                            entry.1.push(n);
                        }
                    }
                }
                SimEvent::Destroyed { id } => {
                    self.mesh_masks.remove(&id.0);
                }
                _ => {}
            }
        }
        if let Some(sim) = &self.sim {
            for key in self.temp_effects.handle(&sim.rules, events) {
                if let Some(a) = self.effects.game_effect_mut(key) {
                    a.stop();
                }
            }
        }
        self.track_arrows();
        self.sync_building_fires(vfs);
        self.sync_movement_effects(vfs);
        self.sync_train_sparks(vfs);
        self.sync_heal_effects(vfs, events);
        self.sync_ability_effects(vfs);
        self.sync_world_effects(vfs);
        self.sync_temp_upgrade_effects(vfs);
    }

    /// Permanent ability effects ([`AbilityEffects::sync`]): at the nodes of the models that
    /// the renderer holds; the carriers get their rest hull so that their visibility is
    /// known.
    fn sync_ability_effects(&mut self, vfs: &Vfs) {
        let Some(sim) = &self.sim else {
            return;
        };
        let carriers: Vec<usize> = AbilityEffects::wanted(&sim.state, &sim.rules)
            .into_iter()
            .filter_map(|w| self.object_of.get(&w.id.0).copied())
            .collect();
        for c in carriers {
            self.ensure_effect_box(vfs, c);
        }
        let Some(sim) = &self.sim else {
            return;
        };
        let (renderer, entry_of, object_of) = (&self.renderer, &self.entry_of, &self.object_of);
        let nodes = |id: ObjectId, prefix: &str| -> Vec<String> {
            object_of
                .get(&id.0)
                .and_then(|o| entry_of.get(o))
                .map(|&e| &renderer.entries()[e])
                .map_or_else(Vec::new, |entry| {
                    numbered_nodes(&entry.asset, &entry.playback, prefix)
                })
        };
        let carrier = |id: ObjectId| object_of.get(&id.0).copied();
        self.ability_effects.sync(
            sim,
            &mut self.effects,
            &mut self.models,
            vfs,
            &nodes,
            &carrier,
            self.scene.terrain.params.size_x,
        );
    }

    /// Storm and smithy effect ([`WorldEffects::sync`]); the smith gets its rest hull so that
    /// its visibility is known. The tracks of the dynamite charges are created and follow here in the
    /// step path ([`WorldEffects::sync_charges`], after the events: the explosion has already found its
    /// track with the position from before the disappearance); so every `advance_by` counts like a frame
    /// (viewer command `step N`). They are drawn in [`MapView::prepare_effects`].
    fn sync_world_effects(&mut self, vfs: &Vfs) {
        let Some(sim) = &self.sim else {
            return;
        };
        self.world_effects.sync_charges(sim);
        let carriers: Vec<usize> = WorldEffects::wanted(&sim.state, &sim.rules)
            .into_iter()
            .filter_map(|w| match w.key {
                WorldKey::Object(id) => self.object_of.get(&id).copied(),
                WorldKey::Projectile(_) => None,
            })
            .collect();
        for c in carriers {
            self.ensure_effect_box(vfs, c);
        }
        let Some(sim) = &self.sim else {
            return;
        };
        let object_of = &self.object_of;
        let terrain = &self.scene.terrain;
        self.world_effects.sync(
            sim,
            &mut self.effects,
            &mut self.models,
            vfs,
            &|id| object_of.get(&id.0).copied(),
            &|x, z| terrain.ground_height(x, z),
        );
    }

    /// Digits of the dynamite charges after a logic step (`DynamiteEffect_Tick` `0x004F07A0`,
    /// [`WorldEffects::charge_digits`]) as floating numbers at game time `bar_ms` of the message bar (before its
    /// tick of this frame); called after [`Notices::handle`]. The pause advances nothing, so no digit.
    pub fn charge_digits(&mut self, vfs: &Vfs, bar_ms: i64) {
        let Some(sim) = &self.sim else {
            return;
        };
        self.world_effects
            .charge_digits(sim, &mut self.models, vfs, &mut self.notices, bar_ms);
    }

    /// Healing circles and target effects ([`HealEffects::sync`]) with the events of this frame; the
    /// carriers get their rest hull so that their visibility is known.
    fn sync_heal_effects(&mut self, vfs: &Vfs, events: &[SimEvent]) {
        let Some(sim) = &self.sim else {
            return;
        };
        let healed = events.iter().filter_map(|e| match e {
            SimEvent::Healed { target, .. } => Some(*target),
            _ => None,
        });
        let carriers: Vec<usize> = HealEffects::circled(&sim.state, sim.active_player)
            .into_iter()
            .map(|c| c.id)
            .chain(healed)
            .filter_map(|id| self.object_of.get(&id.0).copied())
            .collect();
        for c in carriers {
            self.ensure_effect_box(vfs, c);
        }
        let Some(sim) = &self.sim else {
            return;
        };
        let object_of = &self.object_of;
        self.heal_effects.sync(
            sim,
            events,
            &mut self.effects,
            &mut self.models,
            vfs,
            &|id| object_of.get(&id.0).copied(),
            self.scene.terrain.params.size_x,
        );
    }

    /// Effects of the temporary upgrades ([`TempUpgradeEffects`]): vanished objects lose
    /// their effects, noted ones and ones lost in the rebuild are created at the object (offset `h` in
    /// y, without rotation) as soon as it is drawn.
    fn sync_temp_upgrade_effects(&mut self, vfs: &Vfs) {
        let Some(sim) = &self.sim else {
            return;
        };
        for key in self.temp_effects.prune(&sim.state) {
            self.effects.remove_game_effect(key);
        }
        for (object, guid) in self.temp_effects.missing(&self.effects) {
            let Some(sim) = &self.sim else {
                return;
            };
            let Some((path, height)) =
                TempUpgradeEffects::effect_of(&sim.rules, guid).map(|(p, h)| (p.to_string(), h))
            else {
                continue;
            };
            let Some(carrier) = self.object_of.get(&object.0).copied() else {
                continue;
            };
            self.ensure_effect_box(vfs, carrier);
            let key = self.effects.add_game_effect(
                vfs,
                &mut self.models,
                &path,
                Anchor::Object {
                    offset: Vec3::Y * height,
                    with_rotation: false,
                },
                Some(carrier),
                1.0,
                0,
                UNLIMITED_MS,
            );
            if key.is_none() {
                log::warn!("effect {path} of temporary upgrade {guid} not loadable");
            }
            self.temp_effects.set_shown(object, guid, key);
        }
    }

    /// Brake sparks of the train ([`TrainSparks::sync`]): at the nodes `bremse01…` of the models of the
    /// locomotive and slaves that the renderer holds.
    fn sync_train_sparks(&mut self, vfs: &Vfs) {
        let Some(sim) = &self.sim else {
            return;
        };
        // Carriers with a rest hull so that their visibility is known.
        let carriers: Vec<usize> = TrainSparks::sparking(sim)
            .into_iter()
            .filter_map(|id| self.object_of.get(&id.0).copied())
            .collect();
        for c in carriers {
            self.ensure_effect_box(vfs, c);
        }
        let Some(sim) = &self.sim else {
            return;
        };
        let (renderer, entry_of, object_of) = (&self.renderer, &self.entry_of, &self.object_of);
        let nodes = |id: ObjectId, prefix: &str| -> Vec<String> {
            object_of
                .get(&id.0)
                .and_then(|o| entry_of.get(o))
                .map(|&e| &renderer.entries()[e])
                .map_or_else(Vec::new, |entry| {
                    numbered_nodes(&entry.asset, &entry.playback, prefix)
                })
        };
        let carrier = |id: ObjectId| object_of.get(&id.0).copied();
        self.train_sparks.sync(
            sim,
            &mut self.effects,
            &mut self.models,
            vfs,
            &nodes,
            &carrier,
        );
    }

    /// Fire of the buildings in the object tick of the simulation ([`BuildingFires::sync`]); the sounds are
    /// fetched by [`MapView::take_fire_sounds`].
    fn sync_building_fires(&mut self, vfs: &Vfs) {
        if let Some(sim) = &self.sim {
            self.building_fires
                .sync(sim, &mut self.effects, &mut self.models, vfs);
        }
    }

    /// Sounds 102 of the building fires since the last call (combat sound path of the application).
    pub fn take_fire_sounds(&mut self) -> Vec<SoundRequest> {
        self.building_fires.take_sounds()
    }

    /// Movement effects in the object tick of the simulation ([`MovementEffects::sync`]) after the
    /// rebuild of the objects; objects with new wheel dust get their rest hull. The
    /// wading sounds are fetched by [`MapView::take_movement_sounds`].
    fn sync_movement_effects(&mut self, vfs: &Vfs) {
        let Some(sim) = &self.sim else {
            return;
        };
        let mut world = MapMovement {
            scene: &self.scene,
            track_tiles: &self.track_tiles,
            state: &sim.state,
            entries: self.renderer.entries(),
            entry_of: &self.entry_of,
            object_of: &self.object_of,
            clock_ms: self.clock_ms,
            effects: &mut self.effects,
            vfs,
            models: &mut self.models,
        };
        self.movement_effects
            .sync(sim, self.particles_quality, &mut world);
        for o in self.movement_effects.take_new_owners() {
            self.ensure_effect_box(vfs, o);
        }
    }

    /// Destruction smoke and craters for the events `events` ([`DestructionMarks::apply`]) through
    /// the same access to the map as the movement effects; at every `ParticlesQuality`.
    fn apply_destruction_marks(&mut self, vfs: &Vfs, events: &[SimEvent]) {
        let Some(sim) = &self.sim else {
            return;
        };
        let mut world = MapMovement {
            scene: &self.scene,
            track_tiles: &self.track_tiles,
            state: &sim.state,
            entries: self.renderer.entries(),
            entry_of: &self.entry_of,
            object_of: &self.object_of,
            clock_ms: self.clock_ms,
            effects: &mut self.effects,
            vfs,
            models: &mut self.models,
        };
        self.destruction_marks
            .apply(events, sim, &mut world, self.particles_quality);
    }

    /// Wading sounds since the last call (`SFX_WATERWALK<n>`, sound path of the application).
    pub fn take_movement_sounds(&mut self) -> Vec<SoundRequest> {
        self.movement_effects.take_sounds()
    }

    /// Target tracking of the arrows (C-108): once per simulation step set the target point anew from
    /// the target object, as long as the target object still exists.
    fn track_arrows(&mut self) {
        let Some(sim) = &self.sim else {
            return;
        };
        if self.tracked_step == sim.state.steps {
            return;
        }
        self.tracked_step = sim.state.steps;
        for p in &sim.state.projectiles.list {
            if p.kind != ProjectileKind::Arrow {
                continue;
            }
            let Some(target) = sim.state.object(p.target, false) else {
                continue;
            };
            if let Some(f) = self.flights.get_mut(p.id) {
                f.track(sim.display(target).0);
            }
        }
    }

    /// Start of a trajectory (re/combat-host 2.1/2.2): muzzle at the animated node of the shooter, otherwise
    /// the event position; arrow model = mesh `model` of the shooter model; ball with `kugel.pef`,
    /// `kanone_rauch.pef` at the muzzle; fire arrow with `brandpfeil_feuer.pef`. Dynamite throw
    /// (re/headhunter-robbing 7): arrow trajectory without fire from the node `dynamitposition`
    /// of the headhunter, `model` here is the path of the whole model (`dynamit.rdo`).
    fn launch_flight(&mut self, vfs: &Vfs, l: &Launch) {
        let muzzle = flight_muzzle(l.node, l.start, || {
            let &i = self.object_of.get(&l.shooter.0)?;
            let entry = &self.renderer.entries()[*self.entry_of.get(&i)?];
            let local = match self.clock_ms {
                Some(now) => entry.playback.helper_node(&entry.asset, now, l.node)?,
                None => entry.playback.bind_helper_node(&entry.asset, l.node)?,
            };
            Some(entry.world.transform_point3(local))
        });
        let mesh = (!l.model.is_empty()).then(|| l.model.to_string());
        let mut f = match l.kind {
            ProjectileKind::Arrow => {
                Flight::arrow(l.id, muzzle, l.aim, l.flight_ms as f32, mesh, l.fire)
            }
            ProjectileKind::CannonBall => {
                Flight::ball(l.id, muzzle, l.aim, l.flight_ms as f32, l.yaw_extra)
            }
            ProjectileKind::Throw => {
                Flight::throw(l.id, muzzle, l.aim, l.flight_ms as f32, l.model)
            }
            // World effects have no trajectory and no `ProjectileLaunched`.
            ProjectileKind::Dynamite | ProjectileKind::Explosion | ProjectileKind::Storm => return,
        };
        f.shooter_guid = l.guid;
        // The throw already carries its model; no mesh in the shooter model.
        if f.kind != Kind::Throw {
            self.resolve_arrow_model(vfs, &mut f);
        }
        // The muzzle smoke of the projectile is an own effect of the shooter; a quartered
        // shooter (without a node) is not drawn (`GameObject_RenderPass` `0x0046B8D4`), so neither is its
        // smoke. The visible smoke at the building is reported by `EffectOwnedAt`.
        if l.kind == ProjectileKind::CannonBall && !l.node.is_empty() {
            let shooter_yaw = self
                .sim
                .as_ref()
                .and_then(|sim| {
                    let o = sim.state.object(l.shooter, false)?;
                    Some(sim.display(o).1.y)
                })
                .unwrap_or(0.0);
            let smoke = nml_effects::particles::Placement {
                position: muzzle,
                yaw: muzzle_smoke_yaw(shooter_yaw, l.yaw_extra),
            };
            self.effects.add_game_effect(
                vfs,
                &mut self.models,
                "data/effects/kanone_rauch.pef",
                Anchor::Fixed(smoke),
                None,
                1.0,
                0,
                UNLIMITED_MS,
            );
        }
        self.start_flight_effect(vfs, &mut f);
        self.flights.list.push(f);
    }

    /// Model and node matrix of the arrow mesh of a trajectory (looked up only once per shooter type and mesh);
    /// without a find the arrow stays invisible.
    fn resolve_arrow_model(&mut self, vfs: &Vfs, f: &mut Flight) {
        let Some(mesh) = &f.mesh else {
            return;
        };
        let key = (f.shooter_guid, mesh.clone());
        if !self.arrow_models.contains_key(&key) {
            let found = self
                .scene
                .project
                .objects
                .get(&f.shooter_guid)
                .and_then(|k| {
                    let main = nml_rda::normalize_path(&k.model);
                    arrow_mesh(&mut self.models, vfs, &main, mesh)
                });
            if found.is_none() {
                log::warn!(
                    "Arrow mesh {mesh} of type {} not found - arrow stays invisible",
                    f.shooter_guid
                );
            }
            self.arrow_models.insert(key.clone(), found);
        }
        f.model = self.arrow_models[&key].clone();
    }

    /// Permanent effect of a trajectory on creation: fire arrow fire. The ball gets `kugel.pef` only from
    /// t/4 (C-111, see [`MapView::advance_flights`]).
    fn start_flight_effect(&mut self, vfs: &Vfs, f: &mut Flight) {
        if f.kind != (Kind::Arrow { fire: true }) {
            return;
        }
        let placement = nml_effects::particles::Placement {
            position: f.pos,
            yaw: f.yaw,
        };
        f.effect = self.effects.add_game_effect(
            vfs,
            &mut self.models,
            "data/effects/brandpfeil_feuer.pef",
            Anchor::Fixed(placement),
            None,
            1.0,
            0,
            UNLIMITED_MS,
        );
    }

    /// End of a trajectory: hit -> gone (arrow, ball); arrow missed -> fall and sticking in the ground.
    fn end_flight(&mut self, id: u32, hit: bool) {
        let Some(f) = self.flights.get_mut(id) else {
            return;
        };
        match (f.kind, hit) {
            (Kind::Arrow { .. }, false) => {
                let ground = self.scene.terrain.ground_height(f.pos.x, f.pos.z);
                f.miss(ground);
            }
            _ => f.phase = Phase::Done,
        }
    }

    /// After loading or restart of the match: object list anew, animations from the new state.
    pub fn reset_sim_view(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vfs: &Vfs) {
        // The effects of the old trajectories are firmly anchored and survived the rebuild.
        for key in self.flights.list.drain(..).filter_map(|f| f.effect) {
            self.effects.remove_game_effect(key);
        }
        self.mesh_masks.clear();
        // Fires of the old match gone; the sync begins at the tick of the loaded state.
        let steps = self.sim.as_ref().map_or(0, |sim| sim.state.steps);
        self.building_fires.reset(&mut self.effects, steps);
        self.movement_effects.reset(&mut self.effects, steps);
        self.destruction_marks.reset();
        self.train_sparks.reset(&mut self.effects);
        self.heal_effects.reset(&mut self.effects);
        self.ability_effects.reset(&mut self.effects);
        self.world_effects.reset(&mut self.effects);
        // Charges found are set quietly at the placement position at once; the flag is thereby used up, and
        // a charge placed in the next step follows its target.
        if let Some(sim) = &self.sim {
            self.world_effects.sync_charges(sim);
        }
        self.temp_effects.reset(&mut self.effects);
        if let Some(sim) = &self.sim {
            self.temp_effects.seed(&sim.state);
        }
        self.refresh_sim_objects(device, queue, vfs, true, &[]);
        self.notices.clear();
        self.resume_flights(vfs);
    }

    /// Trajectories from the projectiles of the simulation (after loading/restart, [`resumed_flight`]): arrows
    /// with the mesh of the shooter, fire arrows with fire, balls with their effect via
    /// [`MapView::advance_flights`], throws with the dynamite model.
    fn resume_flights(&mut self, vfs: &Vfs) {
        let Some(sim) = &self.sim else {
            return;
        };
        self.tracked_step = sim.state.steps;
        let flights: Vec<Flight> = sim
            .state
            .projectiles
            .list
            .iter()
            .filter_map(|p| resumed_flight(p, &sim.state))
            .collect();
        for mut f in flights {
            if f.kind != Kind::Throw {
                self.resolve_arrow_model(vfs, &mut f);
            }
            self.start_flight_effect(vfs, &mut f);
            self.flights.list.push(f);
        }
    }

    fn refresh_sim_objects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        force: bool,
        renamed: &[(u32, u32)],
    ) {
        let Some(sim) = &self.sim else {
            return;
        };
        let objects = map_objects(&self.scene, &sim.state, |m| vfs.exists(m));
        if !force && sim_keys(&objects) == sim_keys(&self.scene.objects) {
            return;
        }
        // Playback and animation start per old object; only the lifecycle rebuild carries them
        // on, otherwise every animation (also held death animations) would begin again from the start.
        let old_objects = std::mem::replace(&mut self.scene.objects, objects);
        let mut kept: Vec<Option<(Playback, Option<AnimShown>)>> = Vec::new();
        let sources = if force {
            Vec::new()
        } else {
            kept.resize_with(old_objects.len(), || None);
            let old_entries = std::mem::take(self.renderer.entries_mut());
            for (entry, starts) in old_entries.into_iter().zip(&self.sim_starts) {
                if let Some(slot) = entry.object.and_then(|o| kept.get_mut(o)) {
                    *slot = Some((entry.playback, *starts));
                }
            }
            carry_over(&old_objects, &self.scene.objects, renamed)
        };
        // Construction sites after a change of owner under their new ID.
        for id in &mut self.dummies_pending {
            if let Some(&(_, to)) = renamed.iter().find(|&&(from, _)| from == *id) {
                *id = to;
            }
        }
        self.renderer
            .set_objects(vfs, &self.scene, &mut self.models);
        self.renderer.set_level_shadows(
            device,
            queue,
            vfs,
            &mut self.models,
            &self.scene,
            self.shadow_quality,
        );
        self.entry_of = entry_index(&self.renderer);
        self.object_of = sim_index(&self.scene.objects);
        self.sim_starts = vec![None; self.renderer.entries().len()];
        for (entry, starts) in self
            .renderer
            .entries_mut()
            .iter_mut()
            .zip(&mut self.sim_starts)
        {
            let source = entry.object.and_then(|o| sources.get(o).copied().flatten());
            if let Some((playback, started)) = source.and_then(|s| kept[s].take()) {
                entry.playback = playback;
                *starts = started;
            }
        }
        if force {
            self.set_particles_quality(vfs, self.particles_quality);
        } else {
            self.carry_effects(vfs, &old_objects, &sources);
        }
    }

    /// Camera and fog values of a frame at clock time `now_ms` (water animation).
    pub fn view_params(
        &self,
        size: (u32, u32),
        aspect: AspectMode,
        fog: bool,
        now_ms: f32,
    ) -> ViewParams {
        let (view, projection) = match &self.free {
            Some(free) => (
                free.view(),
                free.projection(self.camera.aspect_factor(aspect, size.0, size.1)),
            ),
            None => (
                self.camera.view(),
                self.camera.projection(aspect, size.0, size.1),
            ),
        };
        // Script fog before the view switch and before the values of the `.wld`: 501/502 switch, 505/506
        // set color, start and length like `[FOG]` (world units behind the camera target, `fog_range`).
        let script = self.script_fog.values.map(|v| v.params());
        let fog = self.fog_enabled(fog).then(|| {
            let params = script.as_ref().or(self.scene.world.fog.as_ref());
            let (start, end) = self.camera.fog_range(params);
            FogSettings {
                color: self.fog_color(),
                start,
                end,
            }
        });
        ViewParams {
            view,
            projection,
            fog,
            clear: None,
            now_ms,
        }
    }

    /// Fog on (`m_EnableFog`, `GA+0xE768`): 501/502 of the scripts before the view switch `view_fog` (default
    /// of the original 0, F3 of the viewer in the reimplementation). Shared for the world fog
    /// ([`MapView::view_params`]) and the clear color in cinemascope (`GameSession::clear_color`).
    pub fn fog_enabled(&self, view_fog: bool) -> bool {
        self.script_fog.enabled.unwrap_or(view_fog)
    }

    /// Fog color from 0 to 1 (`Camera+0x84`): values 505/506 of the scripts before `[FOG]` of the `.wld`, black
    /// without both.
    pub fn fog_color(&self) -> [f32; 3] {
        let script = self.script_fog.values.map(|v| v.params());
        let params = script.as_ref().or(self.scene.world.fog.as_ref());
        params.map_or([0; 3], |f| f.color).map(|c| c as f32 / 255.0)
    }

    /// Build preview of the frame (`None` = none): the main model of the type (`OBJ_FILENAME` of the
    /// `.prj`) at position and rotation after the placement check, valid green `(0, 1, 0)`, invalid red
    /// `(1, 0, 0)` like `BuildCursor_UpdatePlacement` (`0x0044FDB0`, `Cursor3D_SetColor`).
    /// World matrix like `Cursor3D_RenderWorldModel`: only rotation about y and position, without scaling.
    /// Presentation only, the simulation stays untouched.
    pub fn set_build_preview(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        preview: Option<&PlacementPreview>,
    ) {
        self.set_build_previews(
            device,
            queue,
            vfs,
            preview.map(std::slice::from_ref).unwrap_or_default(),
        );
    }

    /// Build preview with several parts (wall ring of the wall cursor of the command bar, each part
    /// with its own verdict like `BuildCursor_CheckWallSegments` `0x00450870`), otherwise like
    /// [`MapView::set_build_preview`]; empty = none.
    pub fn set_build_previews(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        previews: &[PlacementPreview],
    ) {
        let pieces: Vec<PreviewPiece> = previews
            .iter()
            .filter_map(|p| {
                let kind = self.scene.project.objects.get(&p.guid)?;
                let asset = self
                    .models
                    .get(vfs, &nml_rda::normalize_path(&kind.model))?;
                Some(PreviewPiece {
                    asset,
                    world: object_world_matrix(
                        Vec3::from(p.pos),
                        Vec3::new(0.0, p.rot, 0.0),
                        Vec3::ONE,
                    ),
                    tint: if p.valid {
                        [0.0, 1.0, 0.0]
                    } else {
                        [1.0, 0.0, 0.0]
                    },
                })
            })
            .collect();
        self.renderer
            .set_build_preview(device, queue, vfs, &mut self.models, &pieces);
    }

    /// Decals without an object of the frame (target point of the command bar, defense ring) on the
    /// terrain of the map; empty = none. Presentation only.
    pub fn set_world_decals(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        decals: &[nml_render::WorldDecal],
    ) {
        self.renderer
            .set_world_decals(device, queue, vfs, &self.scene.terrain, decals);
    }

    /// Number of decals without an object of the last [`MapView::set_world_decals`].
    pub fn world_decal_count(&self) -> usize {
        self.renderer.world_decal_count()
    }

    /// Ground markers of the frame for the objects `ids` in drawing order
    /// ([`crate::game::markers::ground_marker_objects`]) like `ObjectRenderer_Draw` with render bit
    /// `0x8000000`: position and yaw of the drawn object, player color from the color index
    /// `GameObject+0x2D0`, `OBJ_SELECTION_*` of the type, state `0x2000000` (wall) and raising to
    /// the water level (`OBJ_ONWATER` or `0x10000`). Objects without a drawn main model
    /// are omitted. Presentation only.
    pub fn set_ground_markers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        ids: &[ObjectId],
    ) {
        let markers = self.sim.as_ref().map_or_else(Vec::new, |sim| {
            ground_marker_list(
                &sim.state,
                &self.scene.objects,
                &self.scene.project,
                &self.entry_of,
                &self.object_of,
                ids,
            )
        });
        self.renderer
            .set_markers(device, queue, vfs, &self.scene.terrain, &markers);
    }

    /// Sight ray through the pixel `cursor` (window pixels) for the ray test: from the
    /// camera position (like `ObjectRenderer_PickScreenPoint`, distances from the camera) with the
    /// direction of `GameCamera::ray`, plus the viewing direction (unit rule). Game and free
    /// camera.
    pub fn pick_ray(&self, params: &ViewParams, cursor: (f64, f64), size: (u32, u32)) -> PickRay {
        let ndc = Vec2::new(
            (cursor.0 / size.0.max(1) as f64 * 2.0 - 1.0) as f32,
            (1.0 - cursor.1 / size.1.max(1) as f64 * 2.0) as f32,
        );
        let (_, direction) = ray_from_ndc(params.projection * params.view, ndc);
        let camera = params.view.inverse();
        PickRay {
            origin: camera.w_axis.truncate(),
            direction,
            forward: camera.z_axis.truncate().normalize_or_zero(),
        }
    }

    /// Index (in `scene.objects`) of the game object under the mouse pointer: with a simulation the
    /// click target ([`MapView::pick_target`]), without (objects of the `.wld`) the ray test of all
    /// objects with triangles ([`SceneRenderer::pick`]).
    pub fn hovered(
        &self,
        params: &ViewParams,
        cursor: (f64, f64),
        size: (u32, u32),
    ) -> Option<usize> {
        if self.sim.is_some() {
            let id = self.pick_target(params, cursor, size)?;
            return self.object_of.get(&id.0).copied();
        }
        let ray = self.pick_ray(params, cursor, size);
        self.renderer.pick(ray.origin, ray.direction)
    }

    /// Simulation object under the mouse pointer like the click selection of the original (`0x0047A210`,
    /// [`click_target`] with the ray test [`SceneRenderer::pick_entry`]): for the overlay "under the
    /// mouse", hover ring and left-click. Trees and nature are never a click target.
    pub fn pick_target(
        &self,
        params: &ViewParams,
        cursor: (f64, f64),
        size: (u32, u32),
    ) -> Option<ObjectId> {
        let sim = self.sim.as_ref()?;
        let ray = self.pick_ray(params, cursor, size);
        let objects = self.pick_list();
        let pick = |k, flags, d: &mut _| self.renderer.pick_entry(k, &ray, flags, d);
        click_target(&sim.state, &objects, pick)
    }

    /// Simulation object or fog memory under the mouse pointer like `Input_PickTargetAction`
    /// (`0x00480E80`, [`SimHost::right_click_target`]) for the selection `selection`: for the
    /// hint "Right-click: ..." and the right-click; also hits trees (the tree behind a
    /// town hall via its roof). A rank without an action falls through (tree in front of a foreign
    /// building); last the fog memories drawn in the last frame ([`MemoryPick`]).
    pub fn pick_right_click_target(
        &mut self,
        params: &ViewParams,
        cursor: (f64, f64),
        size: (u32, u32),
        selection: &[ObjectId],
    ) -> Option<RightClickTarget> {
        let ray = self.pick_ray(params, cursor, size);
        let objects = self.pick_list();
        let renderer = &self.renderer;
        let memories = &self.memory_picks;
        let pick = |k, flags, d: &mut _| renderer.pick_entry(k, &ray, flags, d);
        let pick_memory = |id, flags, d: &mut _| {
            memories
                .get(&id)
                .is_some_and(|m: &MemoryPick| m.pick(&ray, flags, d))
        };
        self.sim
            .as_mut()?
            .right_click_target(selection, &objects, pick, pick_memory)
    }

    /// Visible list of the double-click (list `+0x88` of the frame, `0x0048A0C0`): simulation objects
    /// whose model center lies in the window, in drawing order. Approximation of the original
    /// (visibility per cell and view frustum); the fog is checked when adding.
    pub fn visible_sim_objects(&self, params: &ViewParams, size: (u32, u32)) -> Vec<ObjectId> {
        let points = self.sim_screen_points(params, size);
        let (width, height) = (size.0 as f32, size.1 as f32);
        self.pick_list()
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| {
                points.get(id).is_some_and(|p| {
                    (0.0..=width).contains(&p[0]) && (0.0..=height).contains(&p[1])
                })
            })
            .collect()
    }

    /// Render list of both target searches: simulation objects of the renderer entries in
    /// drawing order with the entry index.
    fn pick_list(&self) -> Vec<(ObjectId, usize)> {
        self.renderer
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(k, e)| Some((ObjectId(self.scene.objects[e.object?].sim_id?), k)))
            .collect()
    }

    /// Ground point under the mouse pointer like the 3D pointer of the original (`0x004CCF00`): over water
    /// the water surface, otherwise the terrain (see `Terrain::cursor_point`).
    pub fn ground_point(
        &self,
        params: &ViewParams,
        cursor: (f64, f64),
        size: (u32, u32),
    ) -> Option<Vec3> {
        let ndc = Vec2::new(
            (cursor.0 / size.0.max(1) as f64 * 2.0 - 1.0) as f32,
            (1.0 - cursor.1 / size.1.max(1) as f64 * 2.0) as f32,
        );
        let (origin, direction) = ray_from_ndc(params.projection * params.view, ndc);
        self.scene
            .terrain
            .cursor_point(origin, direction, 100_000.0)
    }

    /// Screen point (physical pixels) of the hull center per drawn simulation object
    /// (`0x00468D70` likewise projects the model center); missing behind the camera.
    pub fn sim_screen_points(
        &self,
        params: &ViewParams,
        size: (u32, u32),
    ) -> HashMap<ObjectId, [f32; 2]> {
        let view_proj = params.projection * params.view;
        self.renderer
            .object_centers()
            .into_iter()
            .filter_map(|(i, center)| {
                let id = ObjectId(self.scene.objects[i].sim_id?);
                Some((id, project(view_proj, center, size)?))
            })
            .collect()
    }

    /// Rally point flag of a frame ([`crate::game::rally_flag::rally_flag`], `None` = none) to the
    /// renderer (after [`MapView::prepare_effects`], before [`MapView::render_world`]): a world model directly
    /// after the objects, once per frame (D-258). The animation time is the clock of the effect world
    /// ([`EffectWorld::now_ms`]); it stands still in pause and game menu (D-261) and runs 1:1 with the frame time
    /// like all effects, not with `(GameSpeed + 10) / 20`. Position per [`MapView::rally_flag_position`].
    pub fn set_rally_flag(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        flag: Option<&RallyFlag>,
    ) {
        self.rally_flag = flag.copied();
        self.rally_draw =
            flag.map(|f| flag_draw(self.rally_flag_position(f), f.color, self.effects.now_ms()));
        let draws: &[ModelDraw] = self.rally_draw.as_slice();
        self.renderer
            .set_world_models(device, queue, vfs, &mut self.models, draws);
    }

    /// The flag of the last frame ([`MapView::set_rally_flag`]).
    pub fn rally_flag(&self) -> Option<&RallyFlag> {
        self.rally_flag.as_ref()
    }

    /// Its draw command in the last frame.
    pub fn rally_flag_draw(&self) -> Option<&ModelDraw> {
        self.rally_draw.as_ref()
    }

    /// State of the world presentation for the control channel (viewer `state.world_display`,
    /// `nml-game` `ui_state.game.world`; [`world_display_state`]): the flag of the last
    /// [`MapView::set_rally_flag`] at its drawn position, marks, wheel tracks and water rings of the
    /// effect world, the silhouettes of the renderer entries of the last frame, the running beams and
    /// the tracks of the dynamite charges. `None` without a simulation. Read-only: the checksum stays the same.
    pub fn world_display(&self) -> Option<nml_control::WorldDisplayState> {
        self.sim.as_ref()?;
        let rally = self.rally_flag.as_ref().map(|f| {
            let drawn = self
                .rally_draw
                .as_ref()
                .map_or(f.position, |d| Vec3::from(d.world.translation));
            (f, drawn)
        });
        let stealth = stealth_ids(self.renderer.entries().iter().map(|e| {
            let id = e
                .object
                .and_then(|i| self.scene.objects.get(i))
                .and_then(|o| o.sim_id);
            (id, e.look.bits)
        }));
        Some(world_display_state(
            &self.effects,
            rally,
            stealth,
            &self.ability_effects.beams(),
            self.world_effects.charges(),
        ))
    }

    /// Position of the flag (`PropertyBuilding_RenderMeetingFlag` `0x00507BB0`): the stored rally point; with a
    /// rally target (`m_MeetingTarget`, W-100) on top of the target - x/z from its world position, y the
    /// upper edge of its hull in the last frame ([`SceneRenderer::object_tops`]; the original takes position + full
    /// hull height). Without a drawn target the stored point applies; nothing is written back.
    fn rally_flag_position(&self, flag: &RallyFlag) -> Vec3 {
        let Some(target) = flag.target else {
            return flag.position;
        };
        let Some(sim) = &self.sim else {
            return flag.position;
        };
        let Some(o) = sim.state.object(target, false) else {
            return flag.position;
        };
        let Some(&index) = self.object_of.get(&target.0) else {
            return flag.position;
        };
        let top = self
            .renderer
            .object_tops()
            .into_iter()
            .find_map(|(i, top)| (i == index).then_some(top));
        let Some(top) = top else {
            return flag.position;
        };
        let (pos, _) = sim.display(o);
        Vec3::new(pos.x, top.y, pos.z)
    }

    /// Upper edge of the hull per drawn simulation object (world space, for health bars and
    /// rank icons, [`SceneRenderer::object_tops`]).
    pub fn sim_top_points(&self) -> HashMap<ObjectId, Vec3> {
        self.renderer
            .object_tops()
            .into_iter()
            .filter_map(|(i, top)| Some((ObjectId(self.scene.objects[i].sim_id?), top)))
            .collect()
    }

    /// Draws the world.
    pub fn render_world(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        params: &ViewParams,
    ) {
        self.renderer
            .render(device, queue, encoder, target, size, params);
    }

    /// Draws the overlay over the existing image (after [`MapView::render_world`]).
    pub fn draw_overlay(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        lines: &[OverlayLine],
    ) {
        self.text.draw(device, queue, encoder, target, size, lines);
    }
}

/// Location of the object with level index `index` (`hovered.index`) in the scene, which
/// omits objects (start points, unknown types, outside the map, missing models).
fn scene_position(
    scene_indices: impl Iterator<Item = usize>,
    index: usize,
) -> Result<usize, String> {
    let mut scene_indices = scene_indices;
    scene_indices
        .position(|i| i == index)
        .ok_or_else(|| format!("object_index {index} does not exist (not an object of the map)"))
}

/// Special-class decals of the type `guid` (normalized model paths, empty for other types): the
/// decals of kind [`DecalKind::Special`] of its shadow. The special class has its decal at every
/// `ShadowQuality` and also in the `noshadow` area (shadow kind 1, re/shadows).
fn memory_special_decals(scene: &LevelScene, guid: u32) -> Vec<String> {
    let parts = scene.types.parts(guid);
    nml_world::classes::object_shadow(&scene.types, &scene.project, &parts, 0, false)
        .decals
        .into_iter()
        .filter(|d| d.kind == DecalKind::Special)
        .map(|d| d.model)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::lightdata::LightBlock;

    #[test]
    fn temp_upgrade_effects_follow_the_events_and_drop_vanished_objects() {
        use crate::game::unit_bar::testkit::{COLONIST, GOVERNOR, host, spawn};
        let mut host = host();
        let governor = spawn(&mut host, GOVERNOR, 0, (10, 10));
        let colonist = spawn(&mut host, COLONIST, 0, (12, 10));
        let mut fx = TempUpgradeEffects::default();
        let started = |object, guid| SimEvent::TempUpgradeStarted { object, guid };
        let ended = |object, guid| SimEvent::TempUpgradeEnded { object, guid };
        // Flag `EFFECT` (0x01): 4450 shows the effect from the record, 4461 has none.
        let stop = fx.handle(
            &host.rules,
            &[started(governor, 4450), started(colonist, 4461)],
        );
        assert!(stop.is_empty());
        assert_eq!(fx.wanted(), [(governor, 4450)]);
        assert_eq!(
            TempUpgradeEffects::effect_of(&host.rules, 4450),
            Some(("data/effects/test_steuern.pef", 0.0))
        );
        assert_eq!(
            TempUpgradeEffects::effect_of(&host.rules, 4451),
            Some(("data/effects/test_bauzeit.pef", 30.0)),
            "flag HEIGHT: height from `EFFECT h`"
        );
        assert_eq!(TempUpgradeEffects::effect_of(&host.rules, 4461), None);
        // Created effect: the end stops it.
        fx.set_key(governor, 4450, 7);
        assert_eq!(fx.handle(&host.rules, &[ended(governor, 4450)]), [7]);
        assert!(fx.wanted().is_empty());
        // Change of owner: the effect moves along with the new ID (K-288, list stays).
        fx.handle(&host.rules, &[started(governor, 4450)]);
        fx.set_key(governor, 4450, 8);
        let renamed = ObjectId(governor.0 + 1);
        fx.rename(&[(governor.0, renamed.0)]);
        assert_eq!(fx.wanted(), [(renamed, 4450)]);
        // Destroyed without dying (no `TempUpgradeEnded`): the sync removes the effect.
        assert_eq!(fx.prune(&host.state), [8]);
        assert!(fx.wanted().is_empty());
        // After loading: entries with `EFFECT` from the state.
        host.state
            .object_mut(colonist, false)
            .unwrap()
            .temp_upgrades
            .push(nml_sim::upgrades::TempUpgradeEntry {
                flags: 0xE7,
                guid: 4451,
                rest_ms: 1000,
            });
        fx.seed(&host.state);
        assert_eq!(fx.wanted(), [(colonist, 4451)]);
    }

    /// A finished scene (random map) loads without a simulation; path of the log line = path of the
    /// scene. With a real map because the scene needs terrain.
    #[test]
    fn from_scene_has_no_sim() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let mut scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
        scene.lvl_path = "randommap:ZKA".into();
        let objects = scene.objects.len();
        let mut load = MapLoad::from_scene(scene);
        assert!(load.sim.is_none());
        assert_eq!(load.level, "randommap:ZKA");
        assert_eq!(load.scene.lvl_path, "randommap:ZKA");
        // Without a simulation `place_objects` leaves the objects of the scene alone.
        load.place_objects(&vfs);
        assert_eq!(load.scene.objects.len(), objects);
    }

    /// Loads `source` like the match start ([`MapLoad::open`], [`MapLoad::start_sim`] with `setup`,
    /// [`MapLoad::place_objects`]).
    fn started_load(
        vfs: &Vfs,
        source: &MapSource,
        setup: impl FnOnce(&Rules) -> anyhow::Result<MatchSetup>,
    ) -> MapLoad {
        let mut load = MapLoad::open(vfs, source).unwrap();
        load.start_sim(vfs, 10, setup).unwrap();
        load.place_objects(vfs);
        load
    }

    /// Random map `seed`, `continental`, meadow, day, NORMAL; `size` 0 = 64-tile map, 1 = 128-tile map.
    fn random_spec(vfs: &Vfs, seed: &str, size: i32, players: i32) -> RandomMapSpec {
        let types = nml_randommap::load_types(vfs).unwrap();
        RandomMapSpec {
            seed_text: seed.into(),
            size,
            players,
            terrain: types
                .map_scripts
                .iter()
                .position(|s| s.to_lowercase().ends_with("/continental.rnd"))
                .unwrap(),
            landscape: types
                .landscape_types
                .iter()
                .position(|l| l == "MEADOW")
                .unwrap(),
            daytime: types.daytime_types.iter().position(|d| d == "DAY").unwrap(),
            game_type: 0,
        }
    }

    /// On a random map (re/random-map Z.10 item 7) the start camera is, as in the original
    /// (`GameEngine_InitLevelView` `0x004458B0` -> `GameEngine_CenterCameraOnStart` `0x00439190` with the local
    /// player), on the HQ of the own slot 0 - the first object of the player -, not in the center of the map.
    /// Lobby as in the reference run: human on 0, computer on 1, rest open.
    #[test]
    fn random_map_start_camera_looks_at_the_own_hq() {
        use nml_sim::setup::{LobbyEntry, SlotKind};
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        for (seed, size, players) in [("ZKA", 1, 4), ("ZKB", 0, 2)] {
            let spec = random_spec(&vfs, seed, size, players);
            let setup = MatchSetup {
                mode: 0,
                lobby: [(SlotKind::Human, "Mensch"), (SlotKind::Ai, "KI")]
                    .into_iter()
                    .zip(0..)
                    .map(|((kind, name), team)| LobbyEntry {
                        kind,
                        name: name.into(),
                        nation: 0,
                        team,
                        color: team,
                        handicap: 0,
                    })
                    .collect(),
                ..MatchSetup::default()
            };
            let load = started_load(&vfs, &MapSource::Random(spec), |_| Ok(setup));
            let sim = load.sim.as_ref().unwrap();
            assert_eq!(sim.active_player, 0, "{seed}: local player");
            // HQ of nation 0 (`GameWorld_GetHeadquartersGuid`: 1300) in slot 0.
            let hq = sim
                .state
                .objects()
                .find(|o| o.owner() == 0 && o.guid == 1300)
                .unwrap_or_else(|| panic!("{seed}: HQ in slot 0"));
            let center = Vec2::new(
                load.scene.terrain.params.width() / 2.0,
                load.scene.terrain.params.depth() / 2.0,
            );
            let hq_at = Vec2::new(hq.world_pos[0], hq.world_pos[2]);
            assert_ne!(hq_at, center, "{seed}: HQ not in the center of the map");
            let focus = start_camera_focus(&load.scene, load.sim.as_ref(), load.random.is_some());
            assert_eq!(focus, hq_at, "{seed}: start camera on the own HQ");
        }
    }

    /// Fixed maps keep their start camera ([`start_focus`] from the `.lvl` with the objects of the
    /// simulation), in free play as in the campaign. The positions were measured before the start camera change
    /// (German edition with patch); other editions only check equality with [`start_focus`].
    #[test]
    fn fixed_map_start_camera_is_unchanged() {
        use nml_rda::testutil::{GameVersion, game_version};
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let german = game_version(&vfs).unwrap_or_else(|e| panic!("{e}")) == GameVersion::German;
        for (level, measured) in [
            (
                "data/levels/multiplayer/[2players]-tintebariver.lvl",
                Vec2::new(1970.5313, 2597.8125),
            ),
            (
                "data/levels/multiplayer/[4players]-exile.lvl",
                Vec2::new(2799.6563, 13667.5625),
            ),
            (
                "data/levels/kampagne1/kamp1-1/kamp1-1.lvl",
                Vec2::new(12197.8125, 4916.5625),
            ),
        ] {
            let load = started_load(&vfs, &MapSource::Level(level.into()), |rules| {
                Ok(MatchSetup::default_for(rules))
            });
            assert!(load.random.is_none() && load.sim.is_some(), "{level}");
            let focus = start_camera_focus(&load.scene, load.sim.as_ref(), load.random.is_some());
            assert_eq!(focus, start_focus(&load.scene), "{level}");
            if german {
                assert_eq!(focus, measured, "{level}");
            }
        }
    }

    /// In the network match the start camera is, as in the original
    /// (`GameEngine_CenterCameraOnStart` `0x00439190` with the local player), at the start of the **own** slot
    /// (`SimHost::enter_rounds`), also on fixed maps; before, the client had it at the start of the first human
    /// of the `.lvl` (slot 0) in unexplored territory. Without a network it stays at [`start_focus`].
    #[test]
    fn net_start_camera_looks_at_the_own_start() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let level = "data/levels/multiplayer/[4players]-exile.lvl";
        let mut load = started_load(&vfs, &MapSource::Level(level.into()), |rules| {
            Ok(MatchSetup::default_for(rules))
        });
        let first_human = start_focus(&load.scene);
        assert_eq!(
            start_camera_focus(&load.scene, load.sim.as_ref(), false),
            first_human,
            "without a network the first human of the .lvl"
        );
        assert_ne!(
            start_focus_of(&load.scene, 1),
            first_human,
            "slot 1 starts elsewhere than slot 0"
        );
        for slot in [1u8, 0] {
            let sim = load.sim.as_mut().unwrap();
            sim.enter_rounds(slot, slot == 0).unwrap();
            assert_eq!(
                start_camera_focus(&load.scene, load.sim.as_ref(), false),
                start_focus_of(&load.scene, i32::from(slot)),
                "slot {slot}"
            );
        }
    }

    /// The gold deposit (3500) brings its special-class decal into the fog memory; fertile
    /// soil (3503) has none, nor do the other types of the map (real types and
    /// project file).
    #[test]
    fn memories_of_special_types_carry_their_special_decal() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
        let gold = memory_special_decals(&scene, 3500);
        assert_eq!(
            gold,
            ["data/r3d/misc/resources/goldvorkommen/goldvorkommen_shd.rdo"]
        );
        // Fertile soil: special class, but without `OBJ_SHADOW_FILENAME` (only a dummy model).
        assert!(scene.types.class(3503).is_some_and(|c| c.is_special()));
        assert!(memory_special_decals(&scene, 3503).is_empty());
        let others: Vec<u32> = scene
            .objects
            .iter()
            .map(|o| o.guid)
            .filter(|&g| scene.types.class(g).is_some_and(|c| !c.is_special()))
            .collect();
        assert!(!others.is_empty());
        for guid in others {
            assert!(memory_special_decals(&scene, guid).is_empty(), "{guid}");
        }
    }

    fn placed(sim_id: Option<u32>, player: Option<i32>) -> nml_world::level::PlacedObject {
        nml_world::level::PlacedObject {
            index: 0,
            guid: 1,
            class: nml_world::level::ObjectClass::Game,
            type_name: "x".into(),
            name: None,
            model: "x.rdo".into(),
            player,
            parts: vec![1],
            no_shadow: false,
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            transform: glam::Affine3A::IDENTITY,
            sim_id,
        }
    }

    #[test]
    fn the_fog_probe_asks_the_active_player_and_is_free_without_a_simulation() {
        assert_eq!(fog_probe(None)(Vec3::new(-1e9, 0.0, 1e9)), 7);
        let mut host = crate::game::economy::testkit::host();
        host.state.fog = nml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        let at = Vec3::new(64.0, 0.0, 64.0);
        assert_eq!(fog_probe(Some(&host))(at), 0, "fog");
        nml_sim::fog::reveal_map(&mut host.state, 1);
        assert_eq!(fog_probe(Some(&host))(at), 0, "only player 1 sees");
        host.active_player = 1;
        assert_eq!(fog_probe(Some(&host))(at) & 2, 2, "seen");
        host.active_player = 0;
        host.fog.show_mode = crate::game::fog_view::FOG_OFF;
        assert_eq!(fog_probe(Some(&host))(at), 7, "fog off");
    }

    #[test]
    fn points_project_to_pixels_and_vanish_behind_the_camera() {
        // Camera at z = 10 looking toward -z; perspective w = -z.
        let view = Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0));
        let proj = Mat4::from_cols(
            glam::Vec4::X,
            glam::Vec4::Y,
            glam::Vec4::new(0.0, 0.0, 1.0, -1.0),
            glam::Vec4::ZERO,
        );
        let vp = proj * view;
        let center = project(vp, Vec3::ZERO, (200, 100)).unwrap();
        assert!((center[0] - 100.0).abs() < 1e-3 && (center[1] - 50.0).abs() < 1e-3);
        let up = project(vp, Vec3::new(0.0, 1.0, 0.0), (200, 100)).unwrap();
        assert!(up[1] < 50.0);
        assert_eq!(project(vp, Vec3::new(0.0, 0.0, 20.0), (200, 100)), None);
    }

    #[test]
    fn the_object_list_is_rebuilt_only_for_new_ids_or_owners() {
        let a = [placed(Some(1), Some(0)), placed(Some(2), Some(1))];
        let mut moved = a.clone();
        moved[0].position = Vec3::ONE;
        assert_eq!(sim_keys(&a), sim_keys(&moved));
        let mut owner = a.clone();
        owner[1].player = Some(3);
        assert_ne!(sim_keys(&a), sim_keys(&owner));
        assert_ne!(sim_keys(&a), sim_keys(&a[..1]));
    }

    #[test]
    fn a_rebuild_carries_the_display_state_of_kept_and_renamed_objects() {
        let old = [
            placed(Some(1), Some(0)),
            placed(Some(2), Some(1)),
            placed(Some(3), Some(0)),
            placed(Some(4), Some(0)),
            placed(None, None),
        ];
        let mut other_model = placed(Some(4), Some(0));
        other_model.model = "y.rdo".into();
        let new = [
            placed(None, None),
            placed(Some(3), Some(0)),
            // Change of owner 2 -> 9: same presentation under a new ID.
            placed(Some(9), Some(2)),
            placed(Some(5), Some(0)),
            other_model,
        ];
        assert_eq!(
            carry_over(&old, &new, &[(2, 9)]),
            vec![None, Some(2), Some(1), None, None]
        );
        // Without the event 9 is a new object.
        assert_eq!(carry_over(&old, &new, &[])[2], None);
    }

    #[test]
    fn effects_move_with_their_object_and_world_effects_with_its_id() {
        let old = [
            placed(None, None),
            placed(Some(1), Some(0)),
            placed(Some(2), Some(1)),
            placed(Some(3), Some(0)),
        ];
        let new = [
            placed(None, None),
            // 1 is destroyed, 3 moves up, 2 changes owner (-> 9), 5 is new.
            placed(Some(3), Some(0)),
            placed(Some(9), Some(2)),
            placed(Some(5), Some(0)),
        ];
        let sources = carry_over(&old, &new, &[(2, 9)]);
        let (owned, carriers) = effect_moves(&old, &new, &sources);
        // Own effects stay on the object, also under a new ID; the ambient object keeps
        // its location.
        assert_eq!(owned, [Some(0), None, Some(2), Some(1)]);
        // World effects look for their carrier via the ID: no longer after the change of owner.
        assert_eq!(carriers, [Some(0), None, None, Some(1)]);
    }

    #[test]
    fn own_effects_stop_only_for_stealthed_and_quartered_objects() {
        // `GameObject_RenderPass` `0x0046B890`: stealthed without a real drawing bit (silhouette `0x80`, shadow
        // only, fog memory shadow `0x1000` or nothing) no own effects; with a
        // drawing bit (owner, reconnaissance) they run.
        use nml_render::scene::RENDER_SHADOW;
        for bits in [0, 0x80, RENDER_SHADOW, 0x1000, 0x80 | RENDER_SHADOW] {
            assert!(!draws_own_effects(bits, status::MASKED), "{bits:#x}");
        }
        for bits in [1, 4, 0x20, 0x40, 1 | RENDER_SHADOW] {
            assert!(draws_own_effects(bits, status::MASKED), "{bits:#x}");
        }
        // Quartered (occupant): never, even with bits.
        for bits in [0, 1, 1 | RENDER_SHADOW] {
            assert!(!draws_own_effects(bits, status::QUARTERED), "{bits:#x}");
        }
    }

    #[test]
    fn objects_without_render_bits_keep_their_own_effects() {
        // Fishing basket (3505, `level_entries`: render bits 0, only the fish effect): neither stealthed
        // nor quartered, the own effects run as before; likewise any other
        // non-stealthed object with arbitrary bits.
        use nml_render::scene::RENDER_SHADOW;
        for bits in [0, RENDER_SHADOW, 0x1000, 1, 1 | RENDER_SHADOW] {
            assert!(draws_own_effects(bits, 0), "{bits:#x}");
            assert!(draws_own_effects(bits, status::MOVE), "{bits:#x}");
        }
    }

    #[test]
    fn sim_ids_map_to_their_position_in_the_object_list() {
        let list = [
            placed(None, None),
            placed(Some(7), Some(0)),
            placed(Some(3), Some(1)),
        ];
        let index = sim_index(&list);
        assert_eq!(index.get(&7), Some(&1));
        assert_eq!(index.get(&3), Some(&2));
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn ground_markers_take_wall_water_and_player_colour_from_the_object() {
        use crate::game::economy::testkit::{COLONIST, TREE, host, spawn};
        use nml_formats::prj::{Project, ProjectObject, SelectionMarker};
        let mut host = host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let enemy = spawn(&mut host, COLONIST, 3, (12, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let unlisted = spawn(&mut host, COLONIST, 0, (16, 10));
        let undrawn = spawn(&mut host, COLONIST, 0, (18, 10));
        host.state.object_mut(worker, false).unwrap().status |= status::SWIMMING;
        host.state.object_mut(enemy, false).unwrap().status |= status::ONWALL;
        let kind = |guid, on_water| ProjectObject {
            guid,
            name: String::new(),
            model: "x.rdo".into(),
            shadow_model: None,
            groundplane_model: None,
            scaffold_model: None,
            on_water,
            default_rotation: [0.0; 3],
            block_path: false,
            random_rot: false,
            selection: SelectionMarker {
                texture: Some("ring.tga".into()),
                radius: f32::from(guid as u16 % 7),
                use_bbox: false,
            },
            upgrades: Vec::new(),
        };
        let mut project = Project::default();
        project.objects.insert(COLONIST, kind(COLONIST, false));
        project.objects.insert(TREE, kind(TREE, true));
        // Object list: an ambient object at the front, then the game objects (without `unlisted`).
        let mut objects = vec![placed(None, None)];
        for (k, id) in [worker, enemy, tree, undrawn].into_iter().enumerate() {
            let mut o = placed(Some(id.0), Some(0));
            o.position = Vec3::new(k as f32, 1.0, 2.0);
            o.rotation = Vec3::new(0.0, 0.5 * k as f32, 0.0);
            objects.push(o);
        }
        let object_of = sim_index(&objects);
        // Renderer entries: `undrawn` (index 4) without a main model.
        let entry_of: HashMap<usize, usize> = [(1, 10), (2, 11), (3, 12)].into_iter().collect();
        let gone = ObjectId::new(
            worker.index() as u16,
            worker.slot(),
            worker.generation() + 1,
        );
        let ids = [tree, gone, unlisted, enemy, undrawn, worker];
        let markers =
            ground_marker_list(&host.state, &objects, &project, &entry_of, &object_of, &ids);
        let got: Vec<_> = markers
            .iter()
            .map(|m| {
                (
                    m.entry,
                    m.position,
                    m.yaw,
                    m.color,
                    m.on_wall,
                    m.raise_to_water,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                // Tree: `OBJ_ONWATER` of the type raises, neutral color.
                (
                    12,
                    Vec3::new(2.0, 1.0, 2.0),
                    1.0,
                    [0xAC, 0xAC, 0xAC, 0xFF],
                    false,
                    true
                ),
                // Enemy on the wall (`0x2000000`): flat square, color of slot 3.
                (
                    11,
                    Vec3::new(1.0, 1.0, 2.0),
                    0.5,
                    [0xC4, 0x18, 0x00, 0xFF],
                    true,
                    false
                ),
                // Own swimmer (`0x10000`): raised, color of slot 0.
                (
                    10,
                    Vec3::new(0.0, 1.0, 2.0),
                    0.0,
                    [0xFF, 0xCE, 0x0D, 0xFF],
                    false,
                    true
                ),
            ]
        );
        assert_eq!(markers[0].selection, project.objects[&TREE].selection);
        assert_eq!(markers[2].selection, project.objects[&COLONIST].selection);
    }

    #[test]
    fn the_mask_applies_to_its_animation_start_and_returns_after_a_rebuild() {
        // `set_objects` rebuilds the entries with an empty mask; the per-frame sync sets it
        // again. A new animation start deletes it (re/combat-host 1.3).
        let mut masks = BTreeMap::new();
        masks.insert(7, (3, vec!["pfeil".to_string()]));
        assert_eq!(mask_for(&masks, 7, 3), ["pfeil".to_string()]);
        assert!(mask_for(&masks, 7, 4).is_empty(), "other start");
        assert!(mask_for(&masks, 8, 3).is_empty(), "other object");

        let mut hidden: Arc<[String]> = Arc::from([]);
        assert!(sync_mask(&mut hidden, mask_for(&masks, 7, 3)));
        assert_eq!(*hidden, ["pfeil".to_string()]);
        assert!(!sync_mask(&mut hidden, mask_for(&masks, 7, 3)), "unchanged");
        // Rebuild of the objects: empty entry, the same mask comes back.
        let mut rebuilt: Arc<[String]> = Arc::from([]);
        assert!(sync_mask(&mut rebuilt, mask_for(&masks, 7, 3)));
        assert_eq!(rebuilt, hidden);
        // New animation start: mask gone.
        assert!(sync_mask(&mut rebuilt, mask_for(&masks, 7, 4)));
        assert!(rebuilt.is_empty());
    }

    #[test]
    fn arrows_draw_their_mesh_from_its_model_with_fading_ghosts_where_seen() {
        use crate::game::projectiles::Ghost;
        let seen = |p: Vec3| if p.x < 100.0 { 7 } else { 0 };
        let mut arrow = Flight::arrow(
            1,
            Vec3::ZERO,
            Vec3::new(500.0, 0.0, 0.0),
            1000.0,
            Some("pfeil".into()),
            false,
        );
        arrow.model = Some(MeshRef {
            path: "data/r3d/pfeil.rdo".into(),
            node: Affine3A::IDENTITY,
        });
        arrow.pos = Vec3::new(10.0, 5.0, 0.0);
        arrow.ghosts.push(Ghost {
            pos: Vec3::new(5.0, 5.0, 0.0),
            pitch: 0.0,
            yaw: 0.0,
            age_ms: 75.0,
        });
        arrow.ghosts.push(Ghost {
            pos: Vec3::new(500.0, 5.0, 0.0),
            pitch: 0.0,
            yaw: 0.0,
            age_ms: 0.0,
        });
        let mut ball = Flight::ball(2, Vec3::ZERO, Vec3::X, 100.0, 0.0);
        ball.model = arrow.model.clone();
        let flights = Flights {
            list: vec![arrow.clone(), ball],
        };
        let mut frame = EffectFrame::default();
        flight_draws(&flights, &seen, &mut frame);
        let models: Vec<&ModelDraw> = frame
            .commands
            .iter()
            .map(|c| match c {
                DrawCommand::Model(m) => m,
                DrawCommand::Quads(_) => panic!("Quads"),
            })
            .collect();
        // Arrow and the seen ghost copy; the unseen copy and the ball not.
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model, "data/r3d/pfeil.rdo");
        assert_eq!(models[0].mesh.as_deref(), Some("pfeil"));
        assert_eq!(models[0].alpha, None);
        assert_eq!(
            models[0].world.translation,
            glam::Vec3A::new(10.0, 5.0, 0.0)
        );
        assert_eq!(models[1].alpha, Some(0.5));
        // Unseen position, finished trajectory, mesh without a found model: nothing.
        let mut frame = EffectFrame::default();
        let mut hidden = arrow.clone();
        hidden.pos = Vec3::new(500.0, 0.0, 0.0);
        hidden.ghosts.clear();
        let mut done = arrow.clone();
        done.phase = Phase::Done;
        let mut unknown = arrow;
        unknown.model = None;
        let flights = Flights {
            list: vec![hidden, done, unknown],
        };
        flight_draws(&flights, &seen, &mut frame);
        assert!(frame.commands.is_empty());
    }

    #[test]
    fn a_throw_draws_the_whole_dynamite_model_with_its_ghosts() {
        // re/headhunter-robbing 7: the whole model (`mesh: None`) at position and pitch of the
        // trajectory like the arrow, ghost trail as with the arrow without fire.
        let seen = |_: Vec3| 2;
        let path = "data/r3d/misc/Tools/dynamit/dynamit.rdo";
        let mut f = Flight::throw(3, Vec3::ZERO, Vec3::new(700.0, 0.0, 0.0), 1000.0, path);
        f.advance(500.0);
        let flights = Flights {
            list: vec![f.clone()],
        };
        let mut frame = EffectFrame::default();
        flight_draws(&flights, &seen, &mut frame);
        let models: Vec<&ModelDraw> = frame
            .commands
            .iter()
            .map(|c| match c {
                DrawCommand::Model(m) => m,
                DrawCommand::Quads(_) => panic!("Quads"),
            })
            .collect();
        assert_eq!(models.len(), 2, "dynamite and a ghost copy");
        let pose = Affine3A::from_mat3_translation(
            game_rotation_y(f.yaw) * game_rotation_x(f.pitch),
            f.pos,
        );
        assert_eq!(models[0].model, nml_rda::normalize_path(path));
        assert_eq!(models[0].mesh, None);
        assert_eq!(models[0].alpha, None);
        assert_eq!(models[0].world, pose);
        assert_eq!(models[1].alpha, Some(f.ghosts[0].alpha()));
    }

    #[test]
    fn a_throw_resumes_after_loading() {
        // `resumed_flight` rebuilds the trajectory from the simulation projectile without GPU.
        use crate::game::testworld::{HQ_SID, HQ_SPA, HUNTER_SID, SID_SHERIFF, hunter_host, spawn};
        use nml_sim::combat::projectile::{THROW_MODEL, launch_throw};
        use nml_sim::command::CommandKind;
        // A hired headhunter (without a target he would go home and vanish, re/headhunter-robbing 3.6).
        let mut host = hunter_host();
        host.state.players[0].resources_fx[0] = 1000 << 12;
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        spawn(&mut host, HQ_SID, 1, (40, 40));
        let victim = spawn(&mut host, SID_SHERIFF, 1, (44, 30));
        host.submit_own(CommandKind::HireHeadhunter {
            art: 2,
            target_player: 1,
            target_guid: SID_SHERIFF as i32,
            building: hq,
        });
        host.step_once();
        let hunter = host.state.headhunters[2].expect("headhunter hired");
        let rules = host.rules.clone();
        let target_pos = host.state.object(victim, false).unwrap().world_pos;
        let id = {
            let mut sim = nml_sim::sim::Sim::new(&mut host.state, &rules);
            launch_throw(&mut sim, hunter, target_pos, 0xD22)
        };
        for _ in 0..4 {
            host.step_once();
        }
        let p = host
            .state
            .projectiles
            .list
            .iter()
            .find(|p| p.id == id)
            .expect("throw is still flying")
            .clone();
        assert!(0 < p.remaining_ms && p.remaining_ms < p.flight_ms, "{p:?}");
        let f = resumed_flight(&p, &host.state).expect("trajectory of the throw");
        assert_eq!(f.kind, Kind::Throw);
        assert_eq!(f.id, id);
        assert_eq!(f.mesh, None);
        assert_eq!(
            f.model.as_ref().map(|m| m.path.as_str()),
            Some(nml_rda::normalize_path(THROW_MODEL).as_str())
        );
        assert_eq!(f.shooter_guid, HUNTER_SID);
        assert_eq!(f.elapsed_ms, (p.flight_ms - p.remaining_ms) as f32);
        assert_eq!(f.start, Vec3::from(p.start));
        assert!((f.pos - f.arrow_pos(f.elapsed_ms)).length() < 1e-3);
        assert!((f.arrow_pos(f.t_ms) - Vec3::from(p.target_pos)).length() < 1e-2);
    }

    /// The arrow of the archer (1402) is not in the main model but in the sub-object
    /// `..._pfeil_wid.rdo` (re/combat-projectiles 15.6, `FUN_005885F0`); its node matrix lays the
    /// node axis y (length of the arrow) along z in model space. An unknown mesh is found
    /// nowhere (real models).
    #[test]
    fn the_arrow_mesh_is_found_in_a_sub_object_of_the_shooter_model() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
        let main = nml_rda::normalize_path(&scene.project.objects[&1402].model);
        let mut models = ModelCache::new();
        let arrow = arrow_mesh(&mut models, &vfs, &main, "pfeil").unwrap();
        assert_eq!(
            arrow.path,
            "data/r3d/wid/units/bogenschuetze/bogenschuetze_pfeil_wid.rdo"
        );
        let along = arrow.node.transform_vector3(Vec3::Y);
        assert!(along.z.abs() > 0.99 && along.x.abs() < 1e-3 && along.y.abs() < 1e-3);
        // A mesh of the main model stays in the main model.
        assert_eq!(
            arrow_mesh(&mut models, &vfs, &main, "tasche").map(|m| m.path),
            Some(main.clone())
        );
        assert_eq!(arrow_mesh(&mut models, &vfs, &main, "nonexistent"), None);
    }

    #[test]
    fn object_index_is_the_level_index_not_the_scene_position() {
        // Level objects 0 and 2 are missing in the scene (e.g. start points).
        let scene = [1, 3, 4];
        assert_eq!(scene_position(scene.into_iter(), 3), Ok(1));
        assert_eq!(scene_position(scene.into_iter(), 1), Ok(0));
        assert!(scene_position(scene.into_iter(), 2).is_err());
        assert!(scene_position(scene.into_iter(), 99999).is_err());
    }

    fn presets() -> Vec<LightPreset> {
        let block = LightBlock {
            position: [0.0, 1.0, 0.0],
            color: [1, 2, 3],
            intensity: 10,
        };
        ["Sommer Tag", "Sommer Nacht", "5"]
            .map(|name| LightPreset {
                name: name.into(),
                terrain: block.clone(),
                object: block.clone(),
            })
            .to_vec()
    }

    #[test]
    fn light_cycles_through_level_and_all_presets() {
        assert_eq!(step_light(None, 3, 1), Some(0));
        assert_eq!(step_light(Some(2), 3, 1), None);
        assert_eq!(step_light(None, 3, -1), Some(2));
        assert_eq!(step_light(Some(0), 3, -1), None);
        assert_eq!(step_light(None, 0, 1), None);
    }

    #[test]
    fn weather_cycles_off_rain_snow() {
        assert_eq!(next_weather(None), Some(WeatherKind::Rain));
        assert_eq!(
            next_weather(Some(WeatherKind::Rain)),
            Some(WeatherKind::Snow)
        );
        assert_eq!(next_weather(Some(WeatherKind::Snow)), None);
    }

    #[test]
    fn unknown_start_light_keeps_the_level_light_but_a_requested_one_is_an_error() {
        let kamp1_2 = "data/levels/kampagne1/kamp1-2/kamp1-2.lvl";
        // Start light "Sommer Nacht" is missing in the (empty) list: level light, no error.
        assert_eq!(
            resolve_light(&LightSetting::Start, kamp1_2, &[], false).unwrap(),
            None
        );
        let error =
            resolve_light(&LightSetting::Preset("Mond".into()), kamp1_2, &[], false).unwrap_err();
        assert!(format!("{error}").contains("(known: level)"), "{error}");
    }

    #[test]
    fn missing_light_presets_file_gives_an_empty_list() {
        assert!(presets_or_empty(Err(anyhow::anyhow!("missing"))).is_empty());
        assert_eq!(presets_or_empty(Ok(presets())).len(), 3);
    }

    #[test]
    fn settings_resolve_against_the_start_table_and_the_presets() {
        let p = presets();
        let kamp1_2 = "data/levels/kampagne1/kamp1-2/kamp1-2.lvl";
        let kamp1_1 = "data/levels/kampagne1/kamp1-1/kamp1-1.lvl";
        assert_eq!(
            resolve_light(&LightSetting::Start, kamp1_2, &p, false).unwrap(),
            Some(1)
        );
        assert_eq!(
            resolve_light(&LightSetting::Start, kamp1_1, &p, false).unwrap(),
            None
        );
        assert_eq!(
            resolve_light(&LightSetting::Level, kamp1_2, &p, false).unwrap(),
            None
        );
        let five = LightSetting::Preset("5".into());
        assert_eq!(resolve_light(&five, kamp1_1, &p, false).unwrap(), Some(2));
        let error =
            resolve_light(&LightSetting::Preset("Mond".into()), kamp1_1, &p, false).unwrap_err();
        assert!(format!("{error}").contains("level, Sommer Tag"), "{error}");
        assert!(format!("{error}").contains("Sommer Nacht, 5"), "{error}");
        let kamp2_5 = "data/levels/kampagne2/kamp2-5/kamp2-5.lvl";
        assert_eq!(
            resolve_weather(WeatherSetting::Start, kamp2_5, false),
            Some(WeatherKind::Rain)
        );
        assert_eq!(resolve_weather(WeatherSetting::Off, kamp2_5, false), None);
        assert_eq!(
            resolve_weather(WeatherSetting::Snow, kamp1_1, false),
            Some(WeatherKind::Snow)
        );
    }

    /// Ships of the simulation report "moving" while the walk runs (`status::MOVE`), and when standing
    /// not: `sync_ships` passes that on to the ship effects (`Unit_UpdateMovementEffects`
    /// `0x0053CEE0`: a ship under way creates no `schiff_stehend`).
    #[test]
    fn ships_leave_a_wake_while_moving() {
        use crate::game::sim_host::testkit::*;
        let mut host = train_host();
        make_coast(&mut host);
        let ship = spawn_dir(&mut host, TRANSPORT, 0, (12, 12), 0);
        let moving = |host: &SimHost| ship_is_moving(host.state.object(ship, false).unwrap());
        assert!(!moving(&host), "standing");
        // Trip east across the water.
        assert!(host.move_units(
            &[ship],
            Vec3::new(40.0 * 128.0 + 64.0, 0.0, 12.0 * 128.0 + 64.0),
            None,
            crate::game::formation_order::FormationOrder::default()
        ));
        let mut seen = false;
        for _ in 0..200 {
            host.step_once();
            if moving(&host) {
                seen = true;
                break;
            }
        }
        assert!(seen, "the ship sets off");
        // Under way the ship advances.
        let x0 = host.state.object(ship, false).unwrap().grid.x.0;
        for _ in 0..10 {
            host.step_once();
        }
        assert!(host.state.object(ship, false).unwrap().grid.x.0 > x0);
        assert!(moving(&host));
        // Arrived, it stands again (`MOVE` off).
        for _ in 0..1500 {
            host.step_once();
            if !moving(&host) {
                break;
            }
        }
        assert!(!moving(&host), "at the destination");
        let o = host.state.object(ship, false).unwrap();
        assert!(
            o.grid.x.0 >> 12 >= 38,
            "destination reached: {}",
            o.grid.x.0 >> 12
        );
    }

    /// The muzzle smoke turns with the shooter: a broadside to starboard (-pi/2) of a ship with
    /// yaw 1 smokes in direction 1 - pi/2, a cannon (extra yaw 0) in the direction of the shooter.
    #[test]
    fn the_muzzle_smoke_turns_with_the_shooter() {
        use std::f32::consts::FRAC_PI_2;
        assert_eq!(muzzle_smoke_yaw(1.0, -FRAC_PI_2), 1.0 - FRAC_PI_2);
        assert_eq!(muzzle_smoke_yaw(1.0, FRAC_PI_2), 1.0 + FRAC_PI_2);
        assert_eq!(muzzle_smoke_yaw(0.75, 0.0), 0.75);
    }

    /// Muzzle fire of a shooter in the tower (`SimEvent::EffectOwnedAt`): fixed at the tower position of the
    /// message, turned toward the target like `fpatan(dx, dz)` of the original.
    #[test]
    fn an_owned_effect_stands_at_the_tower_node_facing_the_target() {
        let at = [1000.0, 495.0, 2000.0];
        let p = owned_effect_placement(at, [2000.0, 0.0, 2000.0]);
        assert_eq!(p.position, Vec3::from(at));
        assert_eq!(p.yaw, std::f32::consts::FRAC_PI_2);
        assert_eq!(owned_effect_placement(at, [1000.0, 0.0, 3000.0]).yaw, 0.0);
    }

    /// A projectile without a node (quartered shooter) starts at `start`, not at the node of the
    /// invisible shooter; with a node at its animated position, without it at `start`.
    #[test]
    fn a_shot_without_node_starts_at_its_start() {
        let start = Vec3::new(1.0, 500.0, 2.0);
        let node = Vec3::new(5.0, 60.0, 6.0);
        assert_eq!(flight_muzzle("", start, || Some(node)), start);
        assert_eq!(flight_muzzle("Muendung01", start, || Some(node)), node);
        assert_eq!(flight_muzzle("Muendung01", start, || None), start);
    }
}
