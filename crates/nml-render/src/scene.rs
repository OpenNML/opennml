//! Draws a level like `GameEngine_RenderWorld` (`0x00438C60`): terrain and edge skirts,
//! shadows on the terrain, objects, effects below water, water, effects above water, weather.
//! Without terrain and water, the same renderer draws the models of the model browser (with a
//! ground surface for the shadows) and the effect browser.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use glam::{Affine3A, Mat4, Vec3};
use nml_effects::output::{DrawCommand, EffectFrame, ModelDraw};
use nml_formats::wld::ObjectLight;
use nml_rda::Vfs;
use nml_world::anim::{Playback, Pose, idle_animation};
use nml_world::assets::{ModelAsset, ModelCache, load_texture};
use nml_world::camera::ray_sphere;
use nml_world::classes::{SHIP_GUIDS, module};
use nml_world::environment::WeatherKind;
use nml_world::level::{LevelScene, ObjectClass};
use nml_world::markers::player_rgb;
use nml_world::shadows::{Aabb, TileMask, instance_box, shadow_light};
use nml_world::terrain::{FogFactors, Terrain};

use crate::effects::{EffectPass, EffectPipelines};
use crate::markers::{GroundMarker, MarkerGpu, MarkerStats, WorldDecal};
use crate::marks::{GroundMarkGpu, GroundMarkStats};
use crate::objects::{Cut, Instance, InstanceLook, MeshFilter, ModelLibrary, ObjectRenderer};
use crate::pick::{PickDistances, PickFlags, PickPart, PickRay, has_hitbox, pick_object};
use crate::shadows::{EntryShadow, MemoryDecal, ShadowGpu, ShadowStats};
use crate::shared::{DEPTH_FORMAT, FrameUniforms, Shared};
use crate::terrain::TerrainGpu;
use crate::texture::{GpuTexture, upload_rgba, white};
use crate::water::{WaterGpu, water_phases};
use crate::weather::{RAIN_TEXTURE, SNOW_TEXTURE, WeatherGpu};

/// Fog for a frame (`Camera_Apply`): color 0...1, start and end in view-space z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FogSettings {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
}

/// Camera and time of a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewParams {
    pub view: Mat4,
    pub projection: Mat4,
    /// `None` = fog off (game default, `m_EnableFog` = 0).
    pub fog: Option<FogSettings>,
    /// Clear color in 0...1; `None` = the fog color when fog is on, otherwise black (`BackColor` 0) or the
    /// background color of [`SceneRenderer::models_only`]. The map view of a match sets it after
    /// `GameApp_Tick` `0x004CA8EC…0x004CA946` (`GameSession::clear_color`, E-245).
    pub clear: Option<[f32; 3]>,
    /// Time in milliseconds (water animation in 25 ms steps).
    pub now_ms: f32,
}

/// Light 0 from `[LIGHT]` (`World_ApplyObjectLight` `0x0042FFE0`): direction to the light,
/// diffuse `LIGHT_COLOR / 255`, ambient `1 − 0.01 · LIGHT_INTENSITY`.
fn object_light(light: Option<&ObjectLight>) -> ([f32; 4], [f32; 4], [f32; 4]) {
    match light {
        Some(light) => {
            let dir = Vec3::from(light.position).normalize_or_zero();
            let [r, g, b] = light.color.map(|c| c as f32 / 255.0);
            let k = 1.0 - 0.01 * light.intensity as f32;
            ([dir.x, dir.y, dir.z, 0.0], [r, g, b, 1.0], [k, k, k, 1.0])
        }
        // `GameApp_CreateStateBlocks`: diffuse white, direction (0, 0, 1) -> to the light (0, 0, -1).
        None => ([0.0, 0.0, -1.0, 0.0], [1.0; 4], [0.0, 0.0, 0.0, 1.0]),
    }
}

/// A model in the scene: asset, world matrix, playback with sub-instances ([`Playback`],
/// created for this asset).
pub struct ModelEntry {
    pub asset: Arc<ModelAsset>,
    pub world: Affine3A,
    pub playback: Playback,
    /// Index in `LevelScene::objects` (for picking); `None` in the model browser.
    pub object: Option<usize>,
    /// Render bits and construction progress (simulation or object class).
    pub look: EntryLook,
    /// Scaffold of the construction site (`OBJ_SCAFFOLD_FILENAME` of the `.prj`, `ObjectRenderer+0x80`).
    pub scaffold: Option<Arc<ModelAsset>>,
    /// Ship flag `ObjectRenderer+0x74` (constructor `0x00549A1B`: GUID in [`SHIP_GUIDS`] and
    /// a scaffold): the scaffold is drawn along in the model pass. `ObjectRenderer_Draw`
    /// clears it permanently once the object is dead (`GameObject+0x1AC` bit `0x40000`).
    pub ship: bool,
    /// Status word `GameObject+0x1AC` of the simulation (0 without simulation): with `DIED` etc. it blocks
    /// the shadow on the ground ([`nml_world::shadows::receives_shadow`]).
    pub status: u32,
    /// The object has module 6 (resource): without visible render bits (`& 0x65`) it remains pickable with
    /// the hull of the main model (ray test `FUN_0046CEA0`), e.g. the invisible
    /// fishing baskets.
    pub resource: bool,
    /// Hide mask (mesh names, lowercase) of the game logic (`MeshVisibility`); the host resets it at
    /// every animation start.
    pub hidden_meshes: Arc<[String]>,
}

/// Render bits that make an object visible to the ray test (`FUN_0046CEA0`: `& 0x65`).
pub const PICK_VISIBLE_BITS: u32 = 0x65;

/// Render bits `GameObject+0x1C4` and progress `GameObject+0x18` of an entry
/// (re/object-classes, section "Render bits and object passes"): `1` model, `2` shadow, `4` construction
/// phase, `0x20` burning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntryLook {
    pub bits: u32,
    /// `GameObject+0x18`: construction progress 0.1 ... 1, or while burning HP percent * 2.5%.
    pub progress: f32,
    /// Brightness factor of the fog, 1.0 without fog.
    pub brightness: f32,
    /// Opacity of the whole entry, 1.0 = opaque (`None` in the pass); the host draws divers
    /// at 0.6 (re/swim-dive 3.3).
    pub alpha: f32,
    /// Player color `g_PlayerColors[GameObject+0x2D0]` (RGB) for the first `EF-MATERIAL` of the
    /// main model and its sub-objects (`ObjectRenderer_Draw` `0x00549AC0` via
    /// `FUN_005884C0`, [`InstanceLook::player_color`]); `None` = `.rdo` color (model browser).
    pub color: Option<[u8; 3]>,
}

impl Default for EntryLook {
    /// Ordinary object: model and shadow.
    fn default() -> Self {
        Self {
            bits: RENDER_MODEL | RENDER_SHADOW,
            progress: 1.0,
            brightness: 1.0,
            alpha: 1.0,
            color: None,
        }
    }
}

/// Player color of an entry as floating point like `ObjectRenderer::SetColor` (`0x00547EE0`:
/// `+0x9C/+0xA0/+0xA4` = r, g, b / 255).
fn player_color(color: Option<[u8; 3]>) -> Option<[f32; 3]> {
    color.map(|c| c.map(|v| f32::from(v) / 255.0))
}

/// Opacity of an instance (`InstanceLook::alpha`): the layer opacity (carcass, `Material_SetAlpha`)
/// times the opacity of the entry ([`EntryLook::alpha`]). An entry opacity of 1.0 leaves the layer
/// unchanged, so an opaque object stays in the opaque pass (`None`).
fn instance_alpha(layer: Option<f32>, entry: f32) -> Option<f32> {
    if entry == 1.0 {
        return layer;
    }
    Some(layer.map_or(entry, |a| a * entry))
}

/// Render bit "model".
pub const RENDER_MODEL: u32 = 0x1;
/// Render bit "shadow".
pub const RENDER_SHADOW: u32 = 0x2;
/// Render bit "construction phase" (crossfade).
pub const RENDER_BUILD: u32 = 0x4;
/// Render bit "burning".
pub const RENDER_BURNING: u32 = 0x20;
/// Render bit "fading" (carcasses of hunted animals, re/combat-projectiles 15.6).
pub const RENDER_FADE: u32 = 0x40;
/// Render bit "silhouette" of a foreign camouflaged unit (`GameObject_GetRenderBitsForPlayer`
/// `0x00466BE0`, re/world-display W.2): drawn only in the late object pass `RenderObjects(0x88)` after
/// the water, never shadow, own effects or ray test.
pub const RENDER_STEALTH: u32 = 0x80;
/// Opacity of the silhouette (`ModelInstance_Render(0x3DCCCCCD)` in `ObjectRenderer_Draw`
/// `0x0054A42C`): 0.1.
pub const STEALTH_ALPHA: f32 = 0.1;
/// Status bit `MASKED` (`GameObject+0x1AC`, `0x100000`) in [`ModelEntry::status`].
const STATUS_MASKED: u32 = 0x10_0000;

/// Which model a draw layer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerModel {
    /// The object model (`ObjectRenderer+0x78`) with sub-objects.
    Main,
    /// The scaffold (`ObjectRenderer+0x80`).
    Scaffold,
}

/// A draw layer of an entry: model, cut and tint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    pub model: LayerModel,
    /// `None`: whole; otherwise (fraction 0 ... 1 of the hull height, keep the upper part).
    pub cut: Option<(f32, bool)>,
    /// `Material_MakeTinted(r, g, b)`.
    pub tint: Option<[f32; 3]>,
    /// `Material_SetAlpha(a)` (render bit `0x40`).
    pub alpha: Option<f32>,
    /// Silhouette (render bit `0x80`): untinted with opacity [`STEALTH_ALPHA`] in the late
    /// object pass after the water ([`InstanceLook::default_white`]).
    pub stealth: bool,
}

/// Hide mask of an entry as a mesh filter.
fn entry_meshes(hidden: &Arc<[String]>) -> MeshFilter {
    if hidden.is_empty() {
        MeshFilter::All
    } else {
        MeshFilter::Hide(hidden.clone())
    }
}

/// Layer with fraction `f` like `0x0058D7C0` (keep bottom) or `0x0058D8D0` (keep top): drawn only
/// for `f > 0` (or NaN), whole from `f ≥ 1` (and for NaN).
fn cut_layer(model: LayerModel, f: f32, keep_above: bool, tint: Option<[f32; 3]>) -> Option<Layer> {
    if f <= 0.0 {
        return None;
    }
    Some(Layer {
        model,
        cut: (f < 1.0).then_some((f, keep_above)),
        tint,
        alpha: None,
        stealth: false,
    })
}

/// `x` limited to 0 ... 1 like the `FCOMP` sequences in `ObjectRenderer_Draw` (NaN stays NaN).
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// Draw layers of an entry in the order of `ObjectRenderer_Draw` (`0x00549AC0`) with
/// `s` = `GameObject+0x18`: bit `1` the model; bit `4` (construction phase) with a scaffold the model up to
/// `2s − 1` and the scaffold up to `min(2s, 1)` of the hull height (first the scaffold grows, then the
/// building), without a scaffold the model up to `s`; bit `0x20` (burning) the upper part `1 − s` dark
/// (tint 0.2) and below it up to `s` the model. Bit `0x40` (carcass of hunted animals) the whole scaffold
/// and the model with opacity `s`. Bit `0x80` (silhouette) the model with opacity
/// [`STEALTH_ALPHA`] as a layer with [`Layer::stealth`] for the late pass after the water.
/// With ship flag `ship` (`+0x74`), bit `1` draws the whole scaffold first (`0x00549E20`/
/// `0x00549E5F`: `ECX = [EBP+0x80]` before the main model).
pub fn entry_layers(look: EntryLook, has_scaffold: bool, ship: bool) -> Vec<Layer> {
    let s = look.progress;
    let mut layers = Vec::new();
    if look.bits & RENDER_MODEL != 0 {
        if ship && has_scaffold {
            layers.push(Layer {
                model: LayerModel::Scaffold,
                cut: None,
                tint: None,
                alpha: None,
                stealth: false,
            });
        }
        layers.push(Layer {
            model: LayerModel::Main,
            cut: None,
            tint: None,
            alpha: None,
            stealth: false,
        });
    }
    if look.bits & RENDER_BUILD != 0 {
        if has_scaffold {
            layers.extend(cut_layer(
                LayerModel::Main,
                clamp01(2.0 * s - 1.0),
                false,
                None,
            ));
            // `2s ≤ 1` (also NaN via `JP`) -> `2s`, otherwise 1.
            let scaffold = if 2.0 * s > 1.0 { 1.0 } else { 2.0 * s };
            layers.extend(cut_layer(LayerModel::Scaffold, scaffold, false, None));
        } else {
            layers.extend(cut_layer(LayerModel::Main, clamp01(s), false, None));
        }
    }
    if look.bits & RENDER_BURNING != 0 {
        layers.extend(cut_layer(
            LayerModel::Main,
            clamp01(1.0 - s),
            true,
            Some([0.2; 3]),
        ));
        layers.extend(cut_layer(LayerModel::Main, clamp01(s), false, None));
    }
    // Bit 0x40 (`0x0054A0C6…0x0054A1A1`): whole scaffold, then the model with opacity s.
    if look.bits & RENDER_FADE != 0 {
        if has_scaffold {
            layers.push(Layer {
                model: LayerModel::Scaffold,
                cut: None,
                tint: None,
                alpha: None,
                stealth: false,
            });
        }
        layers.push(Layer {
            model: LayerModel::Main,
            cut: None,
            tint: None,
            alpha: Some(s),
            stealth: false,
        });
    }
    // Bit 0x80 (`0x0054A3A2…0x0054A42C`, only in pass `0x88`): the main model with opacity
    // 0.1 (`ModelInstance_Render(0.1)`), without scaffold, cut and tint. Whether the unit is moving
    // (`MOVE`, `0x0054A3E1`) is decided by the host via the render bits.
    if look.bits & RENDER_STEALTH != 0 {
        layers.push(Layer {
            model: LayerModel::Main,
            cut: None,
            tint: None,
            alpha: Some(STEALTH_ALPHA),
            stealth: true,
        });
    }
    layers
}

/// Cut plane of a layer in world space: height from the header hull of the model (`RdoFile+0x9F0`,
/// corners 0 and 7 = `min`/`max`) (keep bottom `min + (max − min)·f` (`0x00584A50`), keep top
/// `max + (min − max)·f` (`0x00585CE0`)), transferred with the world matrix (buildings
/// stand upright: only the height counts).
pub fn layer_cut(asset: &ModelAsset, world: &Affine3A, cut: Option<(f32, bool)>) -> Option<Cut> {
    let (f, keep_above) = cut?;
    let (lo, hi) = (asset.rdo.bounds.min[1], asset.rdo.bounds.max[1]);
    let y = if keep_above {
        (lo - hi) * f + hi
    } else {
        (hi - lo) * f + lo
    };
    Some(Cut {
        height: world.transform_point3(Vec3::new(0.0, y, 0.0)).y,
        keep_above,
    })
}

/// A piece of the build preview (`Cursor3D_RenderWorldModel` `0x004CD130`, for the wall ring
/// `FortCursor_RenderSegments` `0x00450750` per piece): main model of the type at position and rotation
/// after the placement check (world matrix `Ry · T`, without `OBJ_SCALE`), tinted with `(r, g, b)`.
#[derive(Clone)]
pub struct PreviewPiece {
    pub asset: Arc<ModelAsset>,
    pub world: Affine3A,
    /// Valid `[0, 1, 0]`, invalid `[1, 0, 0]`.
    pub tint: [f32; 3],
}

/// An active fog memory (re/vision 9): main model of the type with the remembered world matrix and
/// the remembered render bits/progress (construction sites as a cut), brightness by the object rule
/// (re/vision 8.2: 1.0 for own objects and with "fog off", otherwise the factor at the tile of the
/// image), because the image draws via the object renderer. Special-class decals
/// (gold deposit, fertile soil) like `0x00548B30` with remembered render bit 1 with
/// `k' = trunc(k · 255)`; without shadow decal and texture shadow (deviation, `0x00548840` and
/// `0x00415F70` not reimplemented).
/// The player color (`EntryLook::color`) is that of the object renderer (re/renderer, section "Player color", D-222).
#[derive(Clone)]
pub struct MemoryPiece {
    pub asset: Arc<ModelAsset>,
    pub world: Affine3A,
    pub look: EntryLook,
    /// Remembered position (`GameObject+0x08`) and yaw for the decals.
    pub position: Vec3,
    pub yaw: f32,
    /// Normalized model paths of the special-class decals of the type (empty for other types).
    pub special_decals: Vec<String>,
}

/// All GPU data of a level or the model browser.
pub struct SceneRenderer {
    shared: Shared,
    /// `None` in the model browser ([`SceneRenderer::models_only`]).
    terrain: Option<TerrainGpu>,
    water: Option<WaterGpu>,
    /// `None` in the model browser.
    weather: Option<WeatherGpu>,
    /// Water in detail-texture mode (`TerrainQuality` >= 2).
    water_detail: bool,
    objects: ObjectRenderer,
    /// Build preview: drawn after the objects, without shadows, not pickable.
    preview: ObjectRenderer,
    /// Sub-objects of the preview and memory models (object level without animation) per
    /// model path.
    preview_playbacks: HashMap<String, Playback>,
    /// Active fog memories (fog of war): drawn after the objects, without shadows,
    /// not pickable.
    memories: ObjectRenderer,
    /// Silhouettes of foreign camouflaged units (render bit `0x80`): in the late object pass
    /// `RenderObjects(0x88)` after the water, before the effects above; without shadows, not
    /// pickable.
    stealth: ObjectRenderer,
    library: ModelLibrary,
    entries: Vec<ModelEntry>,
    /// Object index per instance of the last `update` (sub-objects inherit that of their carrier).
    instance_objects: Vec<Option<usize>>,
    /// Instances per entry in the last `update` (model and drawn sub-objects).
    entry_instances: Vec<Range<usize>>,
    /// Instances of the first layer with the main model per entry (model and sub-objects, for the
    /// ray test); empty without a drawn main model.
    entry_models: Vec<Range<usize>>,
    /// Overall hull and hitbox flag per entry in the last `update` (ray test).
    entry_picks: Vec<PickCache>,
    shadows: ShadowGpu,
    /// Ground markers (selection, target marker, object under the mouse).
    markers: MarkerGpu,
    /// Ground marks of layer 4 (footprints, craters, wheel tracks).
    ground_marks: GroundMarkGpu,
    effect_pipelines: EffectPipelines,
    /// Effects of layer 2 (below the water surface, before the water).
    effects_below: EffectPass,
    /// Own effects of the objects (`RenderObjects(0x88)`) and layer 1, after the water.
    effects_above: EffectPass,
    /// World models directly after the objects (rally point flag, re/world-display W.1): replaced
    /// per frame, without shadows and ray test.
    world_models: EffectPass,
    /// The instances are built in the bind pose and the entries unchanged since.
    static_built: bool,
    depth: Option<(wgpu::TextureView, (u32, u32))>,
    light: ([f32; 4], [f32; 4], [f32; 4]),
    water_use_texture: bool,
    /// Clear color without fog (level: black like `BackColor` 0).
    background: [f32; 3],
    /// Keeps the ground textures alive (bind groups refer to their views).
    _ground: HashMap<u32, GpuTexture>,
}

impl SceneRenderer {
    /// Loads terrain, water and all game objects (without ambient effects); each object gets its idle animation
    /// (`idle_animation`). The instances stay in the bind pose until the first `update` with
    /// `animate`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
        vfs: &Vfs,
        scene: &LevelScene,
        models: &mut ModelCache,
    ) -> Self {
        let shared = Shared::new(device);
        let images = scene.ground_textures(vfs);
        let ground: HashMap<u32, GpuTexture> = images
            .iter()
            .map(|(&guid, img)| {
                (
                    guid,
                    upload_rgba(device, queue, img, true, "Ground texture"),
                )
            })
            .collect();
        let white = Arc::new(white(device, queue));
        let terrain = TerrainGpu::new(
            device,
            queue,
            &shared,
            color_format,
            &scene.terrain,
            &scene.project,
            &images,
            &ground,
            &white,
        );
        let params = &scene.world.terrain;
        let water = WaterGpu::new(
            device,
            &shared,
            color_format,
            &scene.terrain,
            ground.get(&params.water_base_texture),
            ground.get(&params.water_refraction_texture),
            &white,
        );
        let weather_texture = |path: &str| match load_texture(vfs, path) {
            Ok(img) => Some(upload_rgba(device, queue, &img, false, "Weather texture")),
            Err(e) => {
                log::warn!("Weather texture {path}: {e:#}");
                None
            }
        };
        let weather = WeatherGpu::new(
            device,
            &shared,
            color_format,
            weather_texture(SNOW_TEXTURE).as_ref(),
            weather_texture(RAIN_TEXTURE).as_ref(),
        );
        let entries = level_entries(vfs, scene, models);
        let mut renderer = Self {
            objects: ObjectRenderer::new(device, &shared, color_format),
            preview: ObjectRenderer::new(device, &shared, color_format),
            preview_playbacks: HashMap::new(),
            memories: ObjectRenderer::new(device, &shared, color_format),
            stealth: ObjectRenderer::new(device, &shared, color_format),
            effect_pipelines: EffectPipelines::new(device, &shared, color_format, &white),
            effects_below: EffectPass::new(device, &shared, color_format),
            effects_above: EffectPass::new(device, &shared, color_format),
            world_models: EffectPass::new(device, &shared, color_format),
            library: ModelLibrary::new(white),
            shadows: ShadowGpu::new(device, &shared, color_format),
            markers: MarkerGpu::new(device, &shared, color_format),
            ground_marks: GroundMarkGpu::new(device, &shared, color_format),
            shared,
            terrain: Some(terrain),
            water,
            weather: Some(weather),
            water_detail: false,
            entries,
            instance_objects: Vec::new(),
            entry_instances: Vec::new(),
            entry_models: Vec::new(),
            entry_picks: Vec::new(),
            static_built: false,
            depth: None,
            light: object_light(scene.object_light.as_ref()),
            water_use_texture: params.water_use_texture,
            background: [0.0; 3],
            _ground: ground,
        };
        renderer.update(device, queue, vfs, 0.0, false);
        renderer
    }

    /// Rebuilds the models of all game objects of the scene (after a change of the object list,
    /// e.g. when the simulation creates or destroys objects). Set the shadows again afterwards with
    /// [`SceneRenderer::set_level_shadows`].
    pub fn set_objects(&mut self, vfs: &Vfs, scene: &LevelScene, models: &mut ModelCache) {
        self.entries = level_entries(vfs, scene, models);
        self.static_built = false;
    }

    /// Models only, without terrain and water (model browser), with light like `[LIGHT]` and a fixed
    /// background color. Entries come via [`SceneRenderer::entries_mut`].
    pub fn models_only(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
        light: Option<&ObjectLight>,
        background: [f32; 3],
    ) -> Self {
        let shared = Shared::new(device);
        let white = Arc::new(white(device, queue));
        Self {
            objects: ObjectRenderer::new(device, &shared, color_format),
            preview: ObjectRenderer::new(device, &shared, color_format),
            preview_playbacks: HashMap::new(),
            memories: ObjectRenderer::new(device, &shared, color_format),
            stealth: ObjectRenderer::new(device, &shared, color_format),
            effect_pipelines: EffectPipelines::new(device, &shared, color_format, &white),
            effects_below: EffectPass::new(device, &shared, color_format),
            effects_above: EffectPass::new(device, &shared, color_format),
            world_models: EffectPass::new(device, &shared, color_format),
            library: ModelLibrary::new(white),
            shadows: ShadowGpu::new(device, &shared, color_format),
            markers: MarkerGpu::new(device, &shared, color_format),
            ground_marks: GroundMarkGpu::new(device, &shared, color_format),
            shared,
            terrain: None,
            water: None,
            weather: None,
            water_detail: false,
            entries: Vec::new(),
            instance_objects: Vec::new(),
            entry_instances: Vec::new(),
            entry_models: Vec::new(),
            entry_picks: Vec::new(),
            static_built: false,
            depth: None,
            light: object_light(light),
            water_use_texture: false,
            background,
            _ground: HashMap::new(),
        }
    }

    /// Takes over the current light of the scene after [`LevelScene::apply_light`]: new vertex colors of
    /// terrain and water, replace light 0 (hard change from the next frame).
    pub fn set_light(&mut self, queue: &wgpu::Queue, scene: &LevelScene) {
        let fog = self.terrain.as_ref().and_then(TerrainGpu::fog);
        if let Some(terrain) = &self.terrain {
            terrain.update_colors(queue, &scene.terrain);
        }
        if let Some(water) = &self.water {
            water.update_colors(queue, &scene.terrain, fog);
        }
        self.light = object_light(scene.object_light.as_ref());
    }

    /// Fog-of-war factors per grid point for terrain and water (`None` = factor 1;
    /// not the distance fog [`FogSettings`]). Written only on change; a later
    /// [`SceneRenderer::set_light`] keeps the factors.
    pub fn set_fog(&mut self, queue: &wgpu::Queue, scene: &LevelScene, fog: Option<&FogFactors>) {
        let Some(terrain) = &mut self.terrain else {
            return;
        };
        if terrain.set_fog(queue, &scene.terrain, fog)
            && let Some(water) = &self.water
        {
            water.update_colors(queue, &scene.terrain, fog);
        }
    }

    /// Weather particles of the frame (`None` = no weather); centers from
    /// `nml_world::weather::Weather::positions`.
    pub fn set_weather(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        kind: Option<WeatherKind>,
        centers: &[[f32; 3]],
    ) {
        if let Some(weather) = &mut self.weather {
            weather.set(device, queue, kind, centers);
        }
    }

    /// Number of drawn weather particles.
    pub fn weather_count(&self) -> u32 {
        self.weather.as_ref().map_or(0, WeatherGpu::count)
    }

    /// Draws water in detail-texture mode (`TerrainQuality` >= 2).
    pub fn set_water_detail(&mut self, detail: bool) {
        self.water_detail = detail;
    }

    /// Shadows of all game objects at `ShadowQuality` `quality` (`ObjectRenderer_InitShadow`;
    /// the viewer determines them anew at every change, as if the map were reloaded).
    pub fn set_level_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        models: &mut ModelCache,
        scene: &LevelScene,
        quality: u8,
    ) {
        let shadows = self
            .entries
            .iter()
            .map(|entry| {
                let index = entry
                    .object
                    .expect("entries of the map have an object index");
                let object = &scene.objects[index];
                let moving = object.parts.iter().any(|&guid| {
                    scene.types.class(guid).is_some_and(|c| {
                        [module::WALK, module::TRAIN, module::MULTI_PART]
                            .iter()
                            .any(|&m| c.has_module(m))
                    })
                });
                EntryShadow {
                    setup: scene.object_shadow(index, quality),
                    position: object.position,
                    yaw: object.rotation.y,
                    moving,
                }
            })
            .collect();
        let p = &scene.world.terrain;
        self.set_shadows(
            device,
            queue,
            vfs,
            models,
            shadows,
            shadow_light(&scene.world),
            (p.size_x, p.size_z),
        );
    }

    /// Shadows per entry (same order as [`SceneRenderer::entries`]) and
    /// shadow light (direction to the light, `SHADOW_INTENSITY`).
    #[allow(clippy::too_many_arguments)]
    pub fn set_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        models: &mut ModelCache,
        shadows: Vec<EntryShadow>,
        light: (Vec3, i32),
        tile_size: (f32, f32),
    ) {
        self.shadows.set(
            device,
            queue,
            &self.shared,
            vfs,
            models,
            &self.entries,
            shadows,
            light,
            tile_size,
        );
    }

    /// Ground surface below the models (model browser), `None` removes it.
    pub fn set_shadow_ground(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        terrain: Option<&Terrain>,
    ) {
        self.shadows.set_ground(device, queue, terrain);
    }

    /// Prepares the shadows of a frame (after [`SceneRenderer::update`], before
    /// [`SceneRenderer::render`]); `visible` = visible tiles of this frame. Ground planes
    /// take the fog factors from [`SceneRenderer::set_fog`].
    pub fn prepare_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        terrain: &Terrain,
        visible: &TileMask,
        params: &ViewParams,
    ) {
        self.shadows.prepare(
            device,
            queue,
            terrain,
            visible,
            params.projection * params.view,
            params.now_ms,
            &self.entries,
            self.objects.instances(),
            &self.entry_instances,
            self.terrain.as_ref().and_then(TerrainGpu::fog),
        );
    }

    /// Ground markers of a frame in draw order (after [`SceneRenderer::update`], before
    /// [`SceneRenderer::render`]); empty = none. `terrain` = terrain of the map.
    pub fn set_markers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        terrain: &Terrain,
        markers: &[GroundMarker],
    ) {
        self.markers.prepare(
            device,
            queue,
            &self.shared,
            vfs,
            terrain,
            &self.entries,
            markers,
        );
    }

    /// Decals without an object of a frame (target point, defense ring; after [`SceneRenderer::update`], before
    /// [`SceneRenderer::render`]); empty = none. They lie in the ground marker pass behind these.
    pub fn set_world_decals(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        terrain: &Terrain,
        decals: &[WorldDecal],
    ) {
        self.markers
            .prepare_world(device, queue, &self.shared, vfs, terrain, decals);
    }

    /// Number of decals without an object of the last [`SceneRenderer::set_world_decals`].
    pub fn world_decal_count(&self) -> usize {
        self.markers.world_decals()
    }

    /// Ground marks of layer 4 of a frame (footprints, craters, wheel tracks; after
    /// [`SceneRenderer::update`], before [`SceneRenderer::render`]); empty = none. `terrain` = terrain
    /// of the map. Drawn after the terrain and before the shadows (step 7).
    pub fn set_ground_marks(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        terrain: &Terrain,
        marks: &nml_effects::output::GroundMarks,
    ) {
        self.ground_marks
            .prepare(device, queue, &self.shared, vfs, terrain, marks);
    }

    /// Counters of the ground marks of the last [`SceneRenderer::set_ground_marks`].
    pub fn ground_mark_stats(&self) -> GroundMarkStats {
        self.ground_marks.stats()
    }

    /// Counters of the markers of the last [`SceneRenderer::set_markers`].
    pub fn marker_stats(&self) -> MarkerStats {
        self.markers.stats()
    }

    /// Effects of a frame: `below` = layer 2 (before the water), `above` = own effects of the
    /// objects and layer 1 (after the water), each in draw order; models from `models`.
    pub fn set_effects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        models: &mut ModelCache,
        below: &EffectFrame,
        above: &EffectFrame,
    ) {
        let pipelines = &mut self.effect_pipelines;
        let library = &mut self.library;
        let shared = &self.shared;
        self.effects_below.set(
            device, queue, shared, vfs, pipelines, library, models, below,
        );
        self.effects_above.set(
            device, queue, shared, vfs, pipelines, library, models, above,
        );
    }

    /// World models of a frame, drawn directly after the objects (before the water): the
    /// rally point flag (`PropertyBuilding_RenderMeetingFlag` `0x00507BB0`). The original draws
    /// it at the end of every `ObjectManager_RenderObjects` (`0x0047C5F0`), i.e. three times in normal
    /// play (masks 2, `0x465`, `0x88`); the reimplementation once (deviation, D-258). The list replaces
    /// that of the last frame; models from `models`, unloadable ones are dropped.
    pub fn set_world_models(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        models: &mut ModelCache,
        draws: &[ModelDraw],
    ) {
        let frame = EffectFrame {
            vertices: Vec::new(),
            commands: draws.iter().cloned().map(DrawCommand::Model).collect(),
        };
        self.world_models.set(
            device,
            queue,
            &self.shared,
            vfs,
            &mut self.effect_pipelines,
            &mut self.library,
            models,
            &frame,
        );
    }

    /// Drawn world models of the last [`SceneRenderer::set_world_models`].
    pub fn world_model_count(&self) -> usize {
        self.world_models.model_count()
    }

    /// Drawn effect quads of the last [`SceneRenderer::set_effects`].
    pub fn effect_quads(&self) -> usize {
        self.effects_below.quad_count() + self.effects_above.quad_count()
    }

    /// Drawn effect models of the last [`SceneRenderer::set_effects`].
    pub fn effect_models(&self) -> usize {
        self.effects_below.model_count() + self.effects_above.model_count()
    }

    /// Moves the shadow of entry `index` to a new position (position `GameObject+0x08`, yaw
    /// `+0x158`, world matrix).
    pub fn follow_shadow(&mut self, index: usize, position: Vec3, yaw: f32, world: Affine3A) {
        self.shadows.follow(index, position, yaw, world);
    }

    /// Counters of the shadows of the last prepared frame.
    pub fn shadow_stats(&self) -> ShadowStats {
        self.shadows.stats()
    }

    pub fn entries(&self) -> &[ModelEntry] {
        &self.entries
    }

    /// Changes the entries (e.g. switching animation); the next `update` rebuilds.
    pub fn entries_mut(&mut self) -> &mut Vec<ModelEntry> {
        self.static_built = false;
        &mut self.entries
    }

    /// Builds the instances of the frame: per entry the model and its drawn
    /// sub-instances (`Playback::active_subs`, same world matrix, own playback). With
    /// `animate` anew per frame with poses at time `now_ms`, otherwise once in the bind pose
    /// (sub-objects of the object level; further calls without `animate` then cost nothing).
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        now_ms: f32,
        animate: bool,
    ) {
        if !animate && self.static_built {
            return;
        }
        let mut instances = Vec::new();
        let mut instance_objects = Vec::new();
        let mut entry_instances = Vec::with_capacity(self.entries.len());
        let mut entry_models = Vec::with_capacity(self.entries.len());
        let mut stealth = Vec::new();
        for entry in &self.entries {
            let first = instances.len();
            let mut main: Option<Range<usize>> = None;
            for layer in entry_layers(entry.look, entry.scaffold.is_some(), entry.ship) {
                if layer.stealth {
                    // Silhouette (`ObjectRenderer_Draw` `0x0054A3A2`): own list for the late
                    // pass `RenderObjects(0x88)` after the water; not in the layers of the
                    // entry, so without shadow, occluder and ray test. The opacity of the
                    // entry has no effect (`ModelInstance_Render(0.1)` fixes it), the
                    // material is untinted (`0x0058A850` with flags 1).
                    let look = InstanceLook {
                        alpha: layer.alpha,
                        brightness: entry.look.brightness,
                        player_color: player_color(entry.look.color),
                        default_white: true,
                        ..InstanceLook::default()
                    };
                    let pose = animate.then(|| entry.playback.player().pose(&entry.asset, now_ms));
                    stealth.push(Instance {
                        model: self
                            .library
                            .model(device, queue, &self.shared, vfs, &entry.asset),
                        world: entry.world,
                        pose,
                        look,
                        meshes: entry_meshes(&entry.hidden_meshes),
                    });
                    for sub in entry.playback.active_subs(animate) {
                        let pose = animate.then(|| sub.player.pose(&sub.asset, now_ms));
                        stealth.push(Instance {
                            model: self
                                .library
                                .model(device, queue, &self.shared, vfs, &sub.asset),
                            world: entry.world,
                            pose,
                            look,
                            meshes: entry_meshes(&entry.hidden_meshes),
                        });
                    }
                    continue;
                }
                // Player color only for the main model and sub-objects (`FUN_005884C0` on the
                // model instance `+0x78`), not for the scaffold (`+0x80`).
                let look = |asset: &ModelAsset| InstanceLook {
                    tint: layer.tint,
                    alpha: instance_alpha(layer.alpha, entry.look.alpha),
                    cut: layer_cut(asset, &entry.world, layer.cut),
                    brightness: entry.look.brightness,
                    player_color: player_color(entry.look.color),
                    ..InstanceLook::default()
                };
                if layer.model == LayerModel::Scaffold {
                    if let Some(scaffold) = &entry.scaffold {
                        instances.push(Instance {
                            model: self
                                .library
                                .model(device, queue, &self.shared, vfs, scaffold),
                            world: entry.world,
                            pose: None,
                            look: InstanceLook {
                                player_color: None,
                                ..look(scaffold)
                            },
                            meshes: MeshFilter::All,
                        });
                        instance_objects.push(entry.object);
                    }
                    continue;
                }
                let pose = animate.then(|| entry.playback.player().pose(&entry.asset, now_ms));
                let model_start = instances.len();
                instances.push(Instance {
                    model: self
                        .library
                        .model(device, queue, &self.shared, vfs, &entry.asset),
                    world: entry.world,
                    pose,
                    look: look(&entry.asset),
                    meshes: entry_meshes(&entry.hidden_meshes),
                });
                instance_objects.push(entry.object);
                for sub in entry.playback.active_subs(animate) {
                    let pose = animate.then(|| sub.player.pose(&sub.asset, now_ms));
                    instances.push(Instance {
                        model: self
                            .library
                            .model(device, queue, &self.shared, vfs, &sub.asset),
                        world: entry.world,
                        pose,
                        look: look(&sub.asset),
                        meshes: entry_meshes(&entry.hidden_meshes),
                    });
                    instance_objects.push(entry.object);
                }
                main.get_or_insert(model_start..instances.len());
            }
            entry_instances.push(first..instances.len());
            entry_models.push(main.unwrap_or(first..first));
        }
        // Overall hull like `ModelInstance_ComputeBoundingBox` with "animated" = animation running
        // (pose of the main model), otherwise the rest hull.
        self.entry_picks = self
            .entries
            .iter()
            .zip(&entry_models)
            .map(|(entry, range)| {
                let pose = instances.get(range.start).filter(|_| !range.is_empty());
                PickCache::of(entry, pose.and_then(|i| i.pose.as_ref()), now_ms)
            })
            .collect();
        self.objects
            .set_instances(device, queue, &self.shared, instances);
        if !stealth.is_empty() || !self.stealth.instances().is_empty() {
            self.stealth
                .set_instances(device, queue, &self.shared, stealth);
        }
        self.instance_objects = instance_objects;
        self.entry_instances = entry_instances;
        self.entry_models = entry_models;
        self.static_built = !animate;
    }

    /// Build preview of the frame (empty = none) like `Cursor3D_RenderWorldModel` (`0x004CD130`):
    /// per piece the main model with the sub-objects of the object level in the bind pose (the
    /// original starts no animation), state `Objects` (light, depth, fog like the
    /// objects), all material groups tinted (`FUN_005884C0` also colors `EF-MATERIAL` beforehand,
    /// then `ModelInstance_RenderTinted` with alpha 1: opaque groups opaque,
    /// transparent ones with their `.rdo` alpha). No shadow, no scaffold, no cut; the
    /// entries and the ray test stay untouched.
    pub fn set_build_preview(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        models: &mut ModelCache,
        pieces: &[PreviewPiece],
    ) {
        if pieces.is_empty() && self.preview.instances().is_empty() {
            return;
        }
        let mut instances = Vec::new();
        for piece in pieces {
            let look = InstanceLook {
                tint: Some(piece.tint),
                ..InstanceLook::default()
            };
            instances.push(Instance {
                model: self
                    .library
                    .model(device, queue, &self.shared, vfs, &piece.asset),
                world: piece.world,
                pose: None,
                look,
                meshes: MeshFilter::All,
            });
            let playback = self
                .preview_playbacks
                .entry(piece.asset.path.clone())
                .or_insert_with(|| {
                    Playback::new(&piece.asset, |file| models.get(vfs, file), None, 0.0)
                });
            for sub in playback.active_subs(false) {
                instances.push(Instance {
                    model: self
                        .library
                        .model(device, queue, &self.shared, vfs, &sub.asset),
                    world: piece.world,
                    pose: None,
                    look,
                    meshes: MeshFilter::All,
                });
            }
        }
        self.preview
            .set_instances(device, queue, &self.shared, instances);
    }

    /// Active fog memories of the frame (empty = none): per piece the layers of
    /// [`entry_layers`] without scaffold with the remembered render bits without the shadow bit, main model and
    /// sub-objects of the object level in the bind pose, cut by the remembered progress and
    /// brightness of the piece; with remembered render bit 1 the special-class decals of the piece with
    /// its brightness (drawn with the special-class decals after
    /// [`SceneRenderer::prepare_shadows`]). The entries, the ray test and the shadows of the
    /// entries stay untouched.
    pub fn set_fog_memories(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        models: &mut ModelCache,
        pieces: &[MemoryPiece],
    ) {
        let decals: Vec<MemoryDecal> = pieces
            .iter()
            .filter(|piece| piece.look.bits & RENDER_MODEL != 0)
            .flat_map(|piece| {
                piece.special_decals.iter().map(|model| MemoryDecal {
                    model: model.clone(),
                    position: piece.position,
                    yaw: piece.yaw,
                    brightness: piece.look.brightness,
                })
            })
            .collect();
        self.shadows
            .set_memory_decals(device, queue, &self.shared, vfs, models, decals);
        if pieces.is_empty() && self.memories.instances().is_empty() {
            return;
        }
        let mut instances = Vec::new();
        for piece in pieces {
            let look = EntryLook {
                bits: piece.look.bits & !RENDER_SHADOW,
                ..piece.look
            };
            for layer in entry_layers(look, false, false) {
                // Memory images carry the bits of the object, never the silhouette of the viewer.
                if layer.model != LayerModel::Main || layer.stealth {
                    continue;
                }
                let instance_look = |asset: &ModelAsset| InstanceLook {
                    tint: layer.tint,
                    alpha: instance_alpha(layer.alpha, piece.look.alpha),
                    cut: layer_cut(asset, &piece.world, layer.cut),
                    brightness: piece.look.brightness,
                    player_color: player_color(piece.look.color),
                    ..InstanceLook::default()
                };
                instances.push(Instance {
                    model: self
                        .library
                        .model(device, queue, &self.shared, vfs, &piece.asset),
                    world: piece.world,
                    pose: None,
                    look: instance_look(&piece.asset),
                    meshes: MeshFilter::All,
                });
                let playback = self
                    .preview_playbacks
                    .entry(piece.asset.path.clone())
                    .or_insert_with(|| {
                        Playback::new(&piece.asset, |file| models.get(vfs, file), None, 0.0)
                    });
                for sub in playback.active_subs(false) {
                    instances.push(Instance {
                        model: self
                            .library
                            .model(device, queue, &self.shared, vfs, &sub.asset),
                        world: piece.world,
                        pose: None,
                        look: instance_look(&sub.asset),
                        meshes: MeshFilter::All,
                    });
                }
            }
        }
        self.memories
            .set_instances(device, queue, &self.shared, instances);
    }

    fn depth_view(&mut self, device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
        if let Some((view, current)) = &self.depth
            && *current == size
        {
            return view.clone();
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = Some((view.clone(), size));
        view
    }

    /// Draws a frame into `target` (size `size`).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        params: &ViewParams,
    ) {
        let depth = self.depth_view(device, size);
        let mut uniforms = FrameUniforms::new(params.view, params.projection);
        (
            uniforms.light_dir,
            uniforms.light_diffuse,
            uniforms.light_ambient,
        ) = self.light;
        let fallback = match params.fog {
            Some(fog) => {
                uniforms.fog_color = [fog.color[0], fog.color[1], fog.color[2], 1.0];
                uniforms.fog = [fog.start, fog.end, 1.0, 0.0];
                fog.color
            }
            None => self.background,
        };
        let clear = params.clear.unwrap_or(fallback);
        let phases = water_phases(params.now_ms);
        uniforms.water = [phases[0], phases[1], phases[2], phases[3]];
        uniforms.water2 = [
            phases[4],
            if self.water_use_texture { 1.0 } else { 0.0 },
            0.0,
            0.0,
        ];
        queue.write_buffer(&self.shared.frame_buffer, 0, bytemuck::bytes_of(&uniforms));
        self.shadows
            .render_occluders(encoder, &self.shared, &self.objects, &self.entry_instances);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Welt"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0] as f64,
                        g: clear[1] as f64,
                        b: clear[2] as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        if let Some(terrain) = &self.terrain {
            terrain.draw(&mut pass, &self.shared);
        }
        // Step 7: ground marks of layer 4 (footprints, craters, wheel tracks) before the shadows.
        self.ground_marks.draw(&mut pass, &self.shared);
        self.shadows.draw(&mut pass, &self.shared);
        // Step 8 of `GameEngine_RenderWorld`: ground markers after the shadows, before the
        // objects (which cover the ring).
        self.markers.draw(&mut pass, &self.shared);
        self.objects.draw(&mut pass, &self.shared);
        // The rally point flag at the end of the object pass (`0x0047C73B…0x0047C788`), once per
        // frame (D-258).
        self.world_models
            .draw(&mut pass, &self.shared, &self.effect_pipelines);
        self.memories.draw(&mut pass, &self.shared);
        // `GameEngine_RenderWorld` calls `Cursor3D_RenderWorldModel` after the object pass.
        self.preview.draw(&mut pass, &self.shared);
        self.effects_below
            .draw(&mut pass, &self.shared, &self.effect_pipelines);
        if let Some(water) = &self.water {
            water.draw(&mut pass, &self.shared, self.water_detail);
        }
        // Step 12 (`RenderObjects(0x88)`, re/renderer): silhouettes of foreign camouflaged units
        // (bit `0x80`) after the water, before the own effects of the objects and layer 1. In the
        // original, silhouette and effects follow per object; a foreign camouflaged unit has no effects.
        self.stealth.draw(&mut pass, &self.shared);
        self.effects_above
            .draw(&mut pass, &self.shared, &self.effect_pipelines);
        if let Some(weather) = &self.weather {
            weather.draw(&mut pass, &self.shared);
        }
    }

    /// Ray test of the entry `entry` like `ObjectRenderer_PickScreenPoint` ([`pick_entry`]) with
    /// the poses of the last `update`.
    pub fn pick_entry(
        &self,
        entry: usize,
        ray: &PickRay,
        flags: PickFlags,
        d: &mut PickDistances,
    ) -> bool {
        let Some(e) = self.entries.get(entry) else {
            return false;
        };
        let instances = self
            .entry_models
            .get(entry)
            .and_then(|r| self.objects.instances().get(r.clone()))
            .unwrap_or(&[]);
        let cache = self
            .entry_picks
            .get(entry)
            .copied()
            .unwrap_or_else(|| PickCache::rest(e));
        pick_entry(e, instances, &cache, ray, flags, d)
    }

    /// Game object under the ray without game logic (map without simulation, model browser): all
    /// entries with an object index in list order with triangle test and shared distances,
    /// the last accepted one wins (like a rank of the click selection `0x0047A210`). Index in
    /// `LevelScene::objects`; `direction` normalized, `origin` the camera position.
    pub fn pick(&self, origin: Vec3, direction: Vec3) -> Option<usize> {
        let ray = PickRay {
            origin,
            direction,
            forward: direction,
        };
        let flags = PickFlags {
            triangles: true,
            only_hitbox: false,
        };
        let mut d = PickDistances::default();
        let mut hit = None;
        for (k, entry) in self.entries.iter().enumerate() {
            if let Some(object) = entry.object
                && self.pick_entry(k, &ray, flags, &mut d)
            {
                hit = Some(object);
            }
        }
        hit
    }

    /// Center of the hull sphere per drawn game object (first layer, world space) in the last
    /// frame; invisible objects (without a layer) are missing.
    pub fn object_centers(&self) -> Vec<(usize, Vec3)> {
        self.entry_instances
            .iter()
            .zip(&self.entries)
            .filter(|(range, _)| !range.is_empty())
            .filter_map(|(range, entry)| {
                let instance = self.objects.instances().get(range.start)?;
                Some((
                    entry.object?,
                    instance.world.transform_point3(instance.model.bounds.0),
                ))
            })
            .collect()
    }

    /// Top of the hull per drawn game object in the last frame (world space): center of the
    /// hull in width, top of the overall hull (`ModelInstance_ComputeBoundingBox`) in
    /// height, at the first layer of the main model; basis of the health bars. Objects without a layer
    /// are missing - except camouflaged ones (`MASKED` in [`ModelEntry::status`]): for a foreign
    /// viewer a camouflaged unit draws only the silhouette or nothing, but its hull remains (at the
    /// world matrix of the entry), because ring and health bar of an already selected unit remain
    /// after camouflaging (`0x0047A490` checks only the fog, re/world-display W.2.1, D-266).
    pub fn object_tops(&self) -> Vec<(usize, Vec3)> {
        self.entries
            .iter()
            .zip(&self.entry_models)
            .zip(&self.entry_picks)
            .filter_map(|((entry, range), pick)| {
                let world = match self.objects.instances().get(range.clone()) {
                    Some([instance, ..]) => instance.world,
                    _ if entry.status & STATUS_MASKED != 0 => entry.world,
                    _ => return None,
                };
                let hull = pick.hull;
                let top = Vec3::new(
                    (hull.min.x + hull.max.x) / 2.0,
                    hull.max.y,
                    (hull.min.z + hull.max.z) / 2.0,
                );
                Some((entry.object?, world.transform_point3(top)))
            })
            .collect()
    }

    /// Number of drawn models (entries, without sub-objects).
    pub fn instance_count(&self) -> usize {
        self.entries.len()
    }
}

/// Hit data of an entry: overall hull in model space (`ModelInstance_ComputeBoundingBox`,
/// [`instance_box`]) and whether the main model has a helper node `hitbox`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickCache {
    pub hull: Aabb,
    pub hitbox: bool,
}

impl PickCache {
    /// With `pose` (pose of the main model while an animation runs at time `now_ms`) the
    /// animated hull (skin meshes via `MAXBOUNDINGBOX` of the animation), otherwise the rest hull.
    pub fn of(entry: &ModelEntry, pose: Option<&Pose>, now_ms: f32) -> Self {
        Self {
            hull: instance_box(&entry.asset, &entry.playback, pose, now_ms),
            hitbox: has_hitbox(&entry.asset),
        }
    }

    /// Rest hull (no running animation).
    pub fn rest(entry: &ModelEntry) -> Self {
        Self::of(entry, None, 0.0)
    }
}

/// Ray test of an entry (`FUN_0046CEA0` -> `ObjectRenderer_PickScreenPoint`, [`pick_object`]):
/// only with visible render bits (`& 0x65`) or module 6 (resource, e.g. the invisible
/// fishing baskets), otherwise never (occupants). The main model is tested with its sub-objects
/// (`ObjectRenderer+0x78`), not the scaffold, and whole - also a construction site whose model
/// is only drawn cut. `instances` = the drawn layer of the main model (with pose);
/// if empty (invisible), the main model in the bind pose at the world matrix of the entry
/// applies. Without a hitbox, a world sphere around the overall hull (radius with the largest
/// axis scale, so never tighter than the sphere in model space) rejects beforehand, before
/// parts are collected.
pub fn pick_entry(
    entry: &ModelEntry,
    instances: &[Instance],
    cache: &PickCache,
    ray: &PickRay,
    flags: PickFlags,
    d: &mut PickDistances,
) -> bool {
    if entry.look.bits & PICK_VISIBLE_BITS == 0 && !entry.resource {
        return false;
    }
    if !cache.hitbox {
        let m = entry.world.matrix3;
        let scale = m
            .x_axis
            .length()
            .max(m.y_axis.length())
            .max(m.z_axis.length());
        let center = entry.world.transform_point3(cache.hull.center());
        let radius = cache.hull.half_extent().length() * scale;
        // Small margin against rounding (the exact check follows in model space).
        if ray_sphere(ray.origin, ray.direction, center, radius * 1.001 + 1e-3).is_none() {
            return false;
        }
    }
    let parts: Vec<PickPart> = if instances.is_empty() {
        vec![PickPart {
            asset: &entry.asset,
            pose: None,
        }]
    } else {
        instances
            .iter()
            .map(|i| PickPart {
                asset: &i.model.asset,
                pose: i.pose.as_ref(),
            })
            .collect()
    };
    let rest_center = || instance_box(&entry.asset, &entry.playback, None, 0.0).center();
    pick_object(
        &parts,
        &entry.world,
        &cache.hull,
        rest_center,
        ray,
        flags,
        d,
    )
}

/// Entries of all game objects with their idle animation (`idle_animation`), their scaffold and the
/// render bits of their class (with simulation the viewer sets them afterwards per frame). Render
/// bits 0 (fishing baskets, `PropertyResource_Init` `0x00521375`): model and shadow are not
/// drawn, only their fishing effect.
fn level_entries(vfs: &Vfs, scene: &LevelScene, models: &mut ModelCache) -> Vec<ModelEntry> {
    let bits = |o: &nml_world::level::PlacedObject| {
        o.parts
            .first()
            .and_then(|&g| scene.types.class(g))
            .map_or(RENDER_MODEL | RENDER_SHADOW, |c| c.render_bits())
    };
    scene
        .objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.class == ObjectClass::Game)
        .filter_map(|(index, object)| {
            let asset = models.get(vfs, &object.model)?;
            let scaffold = scene
                .project
                .objects
                .get(&object.guid)
                .and_then(|k| k.scaffold_model.as_deref())
                .and_then(|path| models.get(vfs, &nml_rda::normalize_path(path)));
            let class = object.parts.first().and_then(|&g| scene.types.class(g));
            Some(ModelEntry {
                look: EntryLook {
                    bits: bits(object),
                    progress: 1.0,
                    brightness: 1.0,
                    alpha: 1.0,
                    // Without simulation the color of the slot `OBJ_COLOR` (8 if not given) with the
                    // lobby color = slot (`World_PlaceObject`, `setup::player_color`).
                    color: Some(player_rgb(object.player.unwrap_or(8))),
                },
                ship: scaffold.is_some() && SHIP_GUIDS.contains(&object.guid),
                status: 0,
                resource: class.is_some_and(|c| c.has_module(module::RESOURCE)),
                hidden_meshes: Arc::from([]),
                scaffold,
                playback: Playback::new(
                    &asset,
                    |file| models.get(vfs, file),
                    idle_animation(&asset),
                    0.0,
                ),
                asset,
                world: object.transform,
                object: Some(index),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CAPTURE_FORMAT, render_to_image};
    use crate::gpu::{BackendChoice, headless_device};
    use crate::objects::animated_model;
    use nml_formats::object_cfg::SubObject;
    use nml_formats::rdo::RdoLayout;
    use nml_formats::testutil::{
        bounds, flat_heightmap, sample_rdo, write_hlm, write_rdo, write_tlm,
    };
    use nml_formats::tlm::TextureMap;
    use nml_rda::testutil::{TestFile, build_rda};
    use nml_world::camera::{AspectMode, CameraSetup, GameCamera, look_at_lh, start_focus};
    use nml_world::level::load_level;

    const LVL: &str = "LEVELFILE V1.08e\r\nPROJECT_FILE \"Data\\Projects\\test.prj\"\r\n";
    const PRJ: &str = "PROJECTFILE V1.08f\r\n[OBJECT]\r\n{\r\n\tOBJ_NAME \"Kiste\"\r\n\tOBJ_GUID 10\r\n\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n}\r\n[TEXTURE]\r\n{\r\n\tTEX_NAME \"Grass\"\r\n\tTEX_GUID 5\r\n\tTEX_FILENAME \"data\\textures\\grass.tga\"\r\n\tTEX_GROUP_ID 0\r\n}\r\n";
    const WLD: &str = "WORLDFILE V1.08e\r\n[TERRAIN]\r\n{\r\n\tTERRAIN_DIM_X 4\r\n\tTERRAIN_DIM_Z 4\r\n\tTERRAIN_SIZE_X 128\r\n\tTERRAIN_SIZE_Z 128\r\n\tLIGHT_POSITION 0 1 0\r\n\tLIGHT_COLOR 255 255 255\r\n\tLIGHT_INTENSITY 100\r\n\tWATER_LEVEL -10\r\n\tCHUNK_SIZE 4\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 10\r\n\tOBJ_POSITION 384 0 256\r\n\tOBJ_SCALE 5 5 5\r\n}\r\n";

    fn layer(model: LayerModel, cut: Option<(f32, bool)>, tint: Option<[f32; 3]>) -> Layer {
        Layer {
            model,
            cut,
            tint,
            alpha: None,
            stealth: false,
        }
    }

    #[test]
    fn the_entry_opacity_joins_the_layer_opacity() {
        // Divers (`EntryLook::alpha` 0.6) draw translucent; opacity 1.0 stays `None`
        // (no transparent pass), the layer opacity of the carcasses is kept.
        assert_eq!(instance_alpha(None, 1.0), None);
        assert_eq!(instance_alpha(None, 0.6), Some(0.6));
        assert_eq!(instance_alpha(Some(0.5), 1.0), Some(0.5));
        assert_eq!(instance_alpha(Some(1.0), 1.0), Some(1.0));
        assert_eq!(instance_alpha(Some(0.5), 0.6), Some(0.5 * 0.6));
        assert_eq!(EntryLook::default().alpha, 1.0);
    }

    #[test]
    fn fading_carcasses_draw_the_model_with_the_progress_as_opacity() {
        // re/combat-projectiles 15.6: render bit 0x40, carcass 0x42 (without model bit 1).
        let look = EntryLook {
            bits: RENDER_FADE | RENDER_SHADOW,
            progress: 0.5,
            brightness: 1.0,
            alpha: 1.0,
            color: None,
        };
        assert_eq!(
            entry_layers(look, false, false),
            vec![Layer {
                model: LayerModel::Main,
                cut: None,
                tint: None,
                alpha: Some(0.5),
                stealth: false,
            }]
        );
        let with_scaffold = entry_layers(look, true, false);
        assert_eq!(with_scaffold.len(), 2);
        assert_eq!(
            (with_scaffold[0].model, with_scaffold[0].alpha),
            (LayerModel::Scaffold, None)
        );
        assert_eq!(with_scaffold[1].alpha, Some(0.5));
    }

    #[test]
    fn construction_sites_grow_the_scaffold_first_then_the_building() {
        let site = |s: f32| EntryLook {
            bits: RENDER_BUILD,
            progress: s,
            brightness: 1.0,
            alpha: 1.0,
            color: None,
        };
        // Without scaffold: the model up to `s` of the height.
        assert_eq!(
            entry_layers(site(0.1), false, false),
            [layer(LayerModel::Main, Some((0.1, false)), None)]
        );
        // With scaffold at 0.1: only the scaffold up to 0.2.
        assert_eq!(
            entry_layers(site(0.1), true, false),
            [layer(LayerModel::Scaffold, Some((0.2, false)), None)]
        );
        // At 0.75: the building up to half, the scaffold whole.
        assert_eq!(
            entry_layers(site(0.75), true, false),
            [
                layer(LayerModel::Main, Some((0.5, false)), None),
                layer(LayerModel::Scaffold, None, None)
            ]
        );
        // Finished (render bits 3): only the model, whole.
        assert_eq!(
            entry_layers(EntryLook::default(), true, false),
            [layer(LayerModel::Main, None, None)]
        );
        // Render bits 0 (fishing basket, occupant): nothing.
        assert!(
            entry_layers(
                EntryLook {
                    bits: 0,
                    progress: 1.0,
                    brightness: 1.0,
                    alpha: 1.0,
                    color: None,
                },
                false,
                false
            )
            .is_empty()
        );
    }

    #[test]
    fn burning_buildings_darken_the_part_above_the_hitpoint_share() {
        let burning = EntryLook {
            bits: RENDER_SHADOW | RENDER_BURNING,
            progress: 0.25,
            brightness: 1.0,
            alpha: 1.0,
            color: None,
        };
        assert_eq!(
            entry_layers(burning, false, false),
            [
                layer(LayerModel::Main, Some((0.75, true)), Some([0.2; 3])),
                layer(LayerModel::Main, Some((0.25, false)), None)
            ]
        );
        // HP 0: completely dark.
        let burnt = EntryLook {
            progress: 0.0,
            ..burning
        };
        assert_eq!(
            entry_layers(burnt, false, false),
            [layer(LayerModel::Main, None, Some([0.2; 3]))]
        );
    }

    #[test]
    fn ships_draw_their_whole_scaffold_before_the_model() {
        // Ship with scaffold: bit 1 draws the scaffold first, then the model, both whole.
        assert_eq!(
            entry_layers(EntryLook::default(), true, true),
            [
                layer(LayerModel::Scaffold, None, None),
                layer(LayerModel::Main, None, None)
            ]
        );
        // Without scaffold no flag; the construction phase (bit 4) stays unchanged.
        assert_eq!(
            entry_layers(EntryLook::default(), false, true),
            [layer(LayerModel::Main, None, None)]
        );
        let site = EntryLook {
            bits: RENDER_BUILD,
            progress: 0.1,
            brightness: 1.0,
            alpha: 1.0,
            color: None,
        };
        assert_eq!(
            entry_layers(site, true, true),
            entry_layers(site, true, false)
        );
    }

    // QUIRK D-263: kept
    #[test]
    fn the_stealth_bit_draws_only_the_main_model_with_a_tenth_alpha() {
        // `ObjectRenderer_Draw` `0x0054A3A2…0x0054A42C` (re/world-display W.2.1, D-263): bit `0x80`
        // draws only the main model (model instance `+0x78` with sub-objects) with
        // `ModelInstance_Render(0.1)`, without scaffold (also for the ship), without cut and tint.
        assert_eq!((RENDER_STEALTH, STEALTH_ALPHA), (0x80, 0.1));
        let look = |bits| EntryLook {
            bits,
            progress: 0.3,
            ..EntryLook::default()
        };
        let ghost = Layer {
            model: LayerModel::Main,
            cut: None,
            tint: None,
            alpha: Some(STEALTH_ALPHA),
            stealth: true,
        };
        assert_eq!(entry_layers(look(RENDER_STEALTH), true, true), [ghost]);
        assert_eq!(entry_layers(look(RENDER_STEALTH), false, false), [ghost]);
        // The other bits draw no silhouette.
        for bits in [
            RENDER_MODEL,
            RENDER_BUILD,
            RENDER_BURNING,
            RENDER_FADE,
            RENDER_SHADOW,
            0,
        ] {
            assert!(
                entry_layers(look(bits), true, true)
                    .iter()
                    .all(|l| !l.stealth),
                "{bits:#x}"
            );
        }
    }

    // QUIRK D-263: kept
    #[test]
    fn stealth_is_no_click_target() {
        // Ray test and mouse target check `& 0x65` (`0x0046CEA0`): the silhouette (only `0x80`) is never
        // a click target, not even with the ray through the middle of the model.
        let asset = crate::objects::animated_model("data/r3d/test/kiste.rdo", false);
        let entry = |bits: u32| ModelEntry {
            playback: Playback::new(&asset, |_| None, None, 0.0),
            asset: asset.clone(),
            world: Affine3A::from_translation(Vec3::new(0.0, 0.0, 100.0)),
            object: Some(1),
            look: EntryLook {
                bits,
                ..EntryLook::default()
            },
            scaffold: None,
            ship: false,
            status: STATUS_MASKED,
            resource: false,
            hidden_meshes: Arc::from([]),
        };
        let ray = PickRay {
            origin: Vec3::new(10.0, 50.0, 0.0),
            direction: Vec3::Z,
            forward: Vec3::Z,
        };
        let flags = PickFlags {
            triangles: true,
            only_hitbox: false,
        };
        let hit = |e: &ModelEntry| {
            let cache = PickCache::rest(e);
            pick_entry(e, &[], &cache, &ray, flags, &mut PickDistances::default())
        };
        assert!(hit(&entry(RENDER_MODEL)), "counter-check: the model is hit");
        assert!(!hit(&entry(RENDER_STEALTH)));
        assert_eq!(PICK_VISIBLE_BITS & RENDER_STEALTH, 0);
    }

    #[test]
    fn hidden_objects_with_a_resource_module_stay_pickable() {
        let asset = crate::objects::animated_model("data/r3d/test/kiste.rdo", false);
        let entry = |object: usize, bits: u32, resource: bool| ModelEntry {
            playback: Playback::new(&asset, |_| None, None, 0.0),
            asset: asset.clone(),
            world: Affine3A::from_translation(Vec3::new(0.0, 0.0, 100.0)),
            object: Some(object),
            look: EntryLook {
                bits,
                progress: 1.0,
                brightness: 1.0,
                alpha: 1.0,
                color: None,
            },
            scaffold: None,
            ship: false,
            status: 0,
            resource,
            hidden_meshes: Arc::from([]),
        };
        let entries = [
            // Fishing basket before the first access: render bits 0, module 6.
            entry(1, 0, true),
            // Occupant: render bits 0, no module 6.
            entry(2, 0, false),
            // Visible object (without a drawn layer: main model in the bind pose).
            entry(3, RENDER_MODEL | RENDER_SHADOW, false),
            // Shadow only (bit 2 does not count): not pickable without module 6.
            entry(4, RENDER_SHADOW, false),
        ];
        // Ray onto the koerper (x 0...20, y 0...100 in z = 0 of the model, normal -z).
        let ray = PickRay {
            origin: Vec3::new(10.0, 50.0, 0.0),
            direction: Vec3::Z,
            forward: Vec3::Z,
        };
        let flags = PickFlags {
            triangles: true,
            only_hitbox: false,
        };
        let hits: Vec<usize> = entries
            .iter()
            .filter(|e| {
                let cache = PickCache::rest(e);
                pick_entry(e, &[], &cache, &ray, flags, &mut PickDistances::default())
            })
            .filter_map(|e| e.object)
            .collect();
        assert_eq!(hits, [1, 3]);
    }

    #[test]
    fn cuts_lie_at_the_share_of_the_head_hull_height_in_world_space() {
        let rdo = sample_rdo(RdoLayout::Standard);
        let (lo, hi) = (rdo.bounds.min[1], rdo.bounds.max[1]);
        let asset = ModelAsset {
            path: "x.rdo".into(),
            rdo,
            config: Default::default(),
            animations: Vec::new(),
            material_textures: Vec::new(),
            bone_track_cache: Default::default(),
        };
        let world = Affine3A::from_scale_rotation_translation(
            Vec3::splat(2.0),
            glam::Quat::from_rotation_y(1.0),
            Vec3::new(5.0, 10.0, 7.0),
        );
        let below = layer_cut(&asset, &world, Some((0.25, false))).unwrap();
        assert!((below.height - (10.0 + 2.0 * (lo + (hi - lo) * 0.25))).abs() < 1e-4);
        assert!(!below.keep_above);
        let above = layer_cut(&asset, &world, Some((0.25, true))).unwrap();
        assert!((above.height - (10.0 + 2.0 * (hi - (hi - lo) * 0.25))).abs() < 1e-4);
        assert!(above.keep_above);
        assert_eq!(layer_cut(&asset, &world, None), None);
    }

    /// Flat, green 4 x 4 terrain; grid point (0, 0) lies 50 deep, only the tile (0, 0)
    /// has water. A crate enlarged five times (`sample_rdo`) stands at (384, 0, 256); without
    /// `[LIGHT]` it gets the default light from the front.
    fn green_level() -> tempfile::TempDir {
        level_files(WLD, PRJ, &[])
    }

    /// Like [`green_level`] with its own `.wld` and `.prj` and further files.
    fn level_files(wld: &str, prj: &str, extra: &[(&str, Vec<u8>)]) -> tempfile::TempDir {
        let mut tga = vec![0u8; 18];
        tga[2] = 2;
        tga[12] = 1;
        tga[14] = 1;
        tga[16] = 24;
        tga.extend_from_slice(&[0, 200, 0]);
        let mut heightmap = flat_heightmap(4, 4, 128.0, 0.0);
        heightmap.points[0].position[1] = -50.0;
        let hlm = write_hlm(&heightmap);
        let kiste = write_rdo(&sample_rdo(RdoLayout::Standard));
        let tlm = write_tlm(&TextureMap {
            dim_x: 4,
            dim_z: 4,
            guids: vec![5; 16],
        });
        // Snow texture: one white, opaque pixel (32 bit).
        let mut snow = vec![0u8; 18];
        snow[2] = 2;
        snow[12] = 1;
        snow[14] = 1;
        snow[16] = 32;
        snow.extend_from_slice(&[255, 255, 255, 255]);
        let mut files = vec![
            TestFile {
                path: "data/levels/t/t.lvl",
                data: LVL.as_bytes(),
                compress: false,
            },
            TestFile {
                path: "data/projects/test.prj",
                data: prj.as_bytes(),
                compress: false,
            },
            TestFile {
                path: "data/levels/t/t.wld",
                data: wld.as_bytes(),
                compress: false,
            },
            TestFile {
                path: "data/levels/t/t.hlm",
                data: &hlm,
                compress: true,
            },
            TestFile {
                path: "data/levels/t/t.tlm",
                data: &tlm,
                compress: true,
            },
            TestFile {
                path: "data/textures/grass.tga",
                data: &tga,
                compress: false,
            },
            TestFile {
                path: "data/r3d/test/kiste.rdo",
                data: &kiste,
                compress: false,
            },
            TestFile {
                path: "data/r3d/nature/summer/snow/snow.tga",
                data: &snow,
                compress: false,
            },
        ];
        files.extend(extra.iter().map(|(path, data)| TestFile {
            path,
            data,
            compress: false,
        }));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        dir
    }

    /// Ground marker in the synthetic level, from above (like
    /// [`shadow_decals_and_textures_render_offscreen`]): ring `±120` around the crate at (384, 256)
    /// with a white, opaque selection texture in red. On the grass this gives texture * color;
    /// where the crate model stands, it covers the ring (pass before the objects). Frame and
    /// line rectangle are prepared and drawn without validation errors. Skipped
    /// without a GPU.
    #[test]
    fn ground_markers_render_between_shadows_and_objects() {
        use nml_formats::prj::SelectionMarker;
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut white = vec![0u8; 18];
        white[2] = 2;
        white[12] = 1;
        white[14] = 1;
        white[16] = 32;
        white.extend_from_slice(&[255, 255, 255, 255]);
        let dir = level_files(WLD, PRJ, &[("data/textures/selektion/ring.tga", white)]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let size = (64, 64);
        // From above (pixel <-> world as in the shadow test) and obliquely from the front (-z).
        let top = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 1500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 3000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let front = ViewParams {
            view: look_at_lh(
                Vec3::new(434.0, 600.0, -400.0),
                Vec3::new(434.0, 100.0, 256.0),
                Vec3::Y,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 300.0, 1.0 / 300.0, 1.0 / 3000.0)),
            ..top
        };
        let ring = GroundMarker {
            entry: 0,
            position: Vec3::new(384.0, 0.0, 256.0),
            yaw: 0.0,
            color: [255, 0, 0, 255],
            selection: SelectionMarker {
                texture: Some("data/textures/selektion/ring.tga".into()),
                radius: 120.0,
                use_bbox: false,
            },
            on_wall: false,
            raise_to_water: false,
        };
        let frame =
            |renderer: &mut SceneRenderer, markers: &[GroundMarker], params: &ViewParams| {
                renderer.set_markers(&device, &queue, &vfs, &scene.terrain, markers);
                render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
                    renderer.render(&device, &queue, encoder, target, size, params);
                })
                .unwrap()
            };
        let grass = |p: [u8; 4]| p[1].abs_diff(200) < 3 && p[0] < 3 && p[2] < 3;
        let red = |p: [u8; 4]| p[0] > 250 && p[1] < 3 && p[2] < 3;
        let plain = frame(&mut renderer, &[], &top);
        let marked = frame(&mut renderer, std::slice::from_ref(&ring), &top);
        assert_eq!(renderer.marker_stats().decals, 1);
        // Ring 264 ... 504 x 136 ... 376: inside texture * color, outside unchanged.
        let world = |i: u32, j: u32| {
            let x = 256.0 + ((i as f32 + 0.5) / 64.0 * 2.0 - 1.0) * 256.0;
            let z = 256.0 + (1.0 - (j as f32 + 0.5) / 64.0 * 2.0) * 256.0;
            (x, z)
        };
        let mut inside_red = 0;
        for j in 0..64 {
            for i in 0..64 {
                let (x, z) = world(i, j);
                let (before, after) = (plain.get_pixel(i, j).0, marked.get_pixel(i, j).0);
                let inside = (270.0..498.0).contains(&x) && (142.0..370.0).contains(&z);
                let outside = !(258.0..510.0).contains(&x) || !(130.0..382.0).contains(&z);
                if outside {
                    assert_eq!(before, after, "outside the ring at ({x}, {z})");
                } else if inside && grass(before) {
                    assert!(red(after), "{after:?} at ({x}, {z})");
                    inside_red += 1;
                }
            }
        }
        assert!(inside_red > 100, "{inside_red}");
        // Oblique: the wall of the crate (x 384 ... 484, height 500) stands in the ring. Where it stands, it is
        // drawn after the ring: the opaque part stays unchanged, the semi-transparent one
        // (`koerper`, alpha 0.5) blends over the red - nowhere pure red.
        let plain_front = frame(&mut renderer, &[], &front);
        let marked_front = frame(&mut renderer, std::slice::from_ref(&ring), &front);
        let (mut model, mut blended, mut changed) = (0, 0, 0);
        for (before, after) in plain_front.pixels().zip(marked_front.pixels()) {
            if !grass(before.0) && before.0 != [0, 0, 0, 255] {
                model += 1;
                assert!(!red(after.0), "ring over the model: {:?}", after.0);
                blended += usize::from(before != after);
            } else if before != after {
                assert!(red(after.0), "{:?}", after.0);
                changed += 1;
            }
        }
        assert!(
            model > 20 && blended > 0 && changed > 50,
            "{model} {blended} {changed}"
        );
        let params = top;
        // Frame over the hull and line rectangle (type without texture).
        let frame_marker = GroundMarker {
            selection: SelectionMarker {
                use_bbox: true,
                ..ring.selection.clone()
            },
            ..ring.clone()
        };
        let lines = GroundMarker {
            selection: SelectionMarker::default(),
            ..ring.clone()
        };
        let both = frame(&mut renderer, &[frame_marker, lines], &params);
        assert_eq!(
            renderer.marker_stats(),
            MarkerStats {
                decals: 1,
                lines: 1
            }
        );
        assert_ne!(both, plain);
        // Without entry (no main model): nothing.
        let orphan = GroundMarker {
            entry: 7,
            ..ring.clone()
        };
        assert_eq!(frame(&mut renderer, &[orphan], &params), plain);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Shadows in the synthetic level, from above (parallel projection onto 512 x 512 around
    /// (256, 256)): crate (type 10 = building with category `0x1000000`) with shadow decal
    /// `shd.rdo` (hull +/-100, black, opaque); shadow light oblique from -z, intensity 128.
    /// The wall enlarged five times (`koerper`, x 384...484, height 500) casts its shadow toward
    /// +z. Skipped without a GPU.
    // QUIRK D-214: kept
    #[test]
    fn shadow_decals_and_textures_render_offscreen() {
        use nml_world::shadows::TileMask;
        const DIED: u32 = 0x4_0000;
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let prj = PRJ.replace(
            "\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n",
            "\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n\tOBJ_SHADOW_FILENAME \"data\\r3d\\test\\shd.rdw\"\r\n",
        );
        assert_ne!(prj, PRJ);
        let wld = WLD.replace(
            "[OBJECT]",
            "[SHADOW]\r\n{\r\n\tSHADOW_LIGHT_POSITION 0 1 -1\r\n\tSHADOW_INTENSITY 128\r\n}\r\n[OBJECT]",
        );
        let mut shd = sample_rdo(RdoLayout::Standard);
        shd.bounds = bounds([-100.0, 0.0, -100.0], [100.0, 1.0, 100.0]);
        shd.materials[0].texture = "shd.tga".into();
        let mut black = vec![0u8; 18];
        black[2] = 2;
        black[12] = 1;
        black[14] = 1;
        black[16] = 32;
        black.extend_from_slice(&[0, 0, 0, 255]);
        let property = "GUID 10\r\nNAME Kiste\r\nVALUEID 100 6 1 16777216\r\nPROPID 3 2\r\n";
        let dir = level_files(
            &wld,
            &prj,
            &[
                ("data/gifcon/property.ini", property.as_bytes().to_vec()),
                ("data/r3d/test/shd.rdo", write_rdo(&shd)),
                ("data/r3d/test/shd.tga", black),
            ],
        );
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        assert_eq!(scene.objects[0].parts, [10]);
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let size = (64, 64);
        let view = |height: f32| ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, height, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let all = TileMask::all(4, 4);
        let frame = |renderer: &mut SceneRenderer, params: &ViewParams| {
            renderer.prepare_shadows(&device, &queue, &scene.terrain, &all, params);
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
                renderer.render(&device, &queue, encoder, target, size, params);
            })
            .unwrap()
        };
        // World point (x, z) -> pixel (view from above, up = +z).
        let at = |img: &image::RgbaImage, x: f32, z: f32| {
            let i = ((x - 256.0) / 256.0 + 1.0) / 2.0 * 64.0;
            let j = (1.0 - (z - 256.0) / 256.0) / 2.0 * 64.0;
            img.get_pixel(i as u32, j as u32).0
        };
        let params = view(500.0);
        let plain = frame(&mut renderer, &params);
        // Without set_level_shadows: no shadow; grass (0, 200, 0).
        assert!(at(&plain, 340.0, 290.0)[1].abs_diff(200) < 3);
        // Level 1: shadow decal (black, opaque) below the object.
        renderer.set_level_shadows(&device, &queue, &vfs, &mut models, &scene, 1);
        let decal = frame(&mut renderer, &params);
        assert!(
            at(&decal, 340.0, 290.0)[1] < 3,
            "{:?}",
            at(&decal, 340.0, 290.0)
        );
        assert_eq!(renderer.shadow_stats().decals, 1);
        assert_eq!(renderer.shadow_stats().kinds, [0, 1, 0, 0]);
        // Dead (`DIED`, `0x00549B23`): no decal anymore for kind 1.
        renderer.entries_mut()[0].status = DIED;
        let dead = frame(&mut renderer, &params);
        assert_eq!(renderer.shadow_stats().decals, 0);
        assert!(at(&dead, 340.0, 290.0)[1].abs_diff(200) < 3);
        renderer.entries_mut()[0].status = 0;
        // Level 2: shadow texture (kind 2) instead of decal; the wall shadow falls toward +z.
        renderer.set_level_shadows(&device, &queue, &vfs, &mut models, &scene, 2);
        let texture = frame(&mut renderer, &params);
        let stats = renderer.shadow_stats();
        assert_eq!(
            (stats.kinds, stats.persistent, stats.rendered),
            ([0, 0, 1, 0], 1, 1)
        );
        assert_eq!(stats.atlas_used, 1.0);
        assert!(at(&texture, 340.0, 290.0)[1].abs_diff(200) < 3);
        // Green * (1 - 128/255) ~ 100 behind the wall, grass in front of and beside it.
        let shadow = at(&texture, 434.0, 400.0);
        assert!(shadow[1].abs_diff(100) < 6, "{shadow:?}");
        assert!(at(&texture, 434.0, 200.0)[1].abs_diff(200) < 3);
        assert!(at(&texture, 300.0, 400.0)[1].abs_diff(200) < 3);
        // Same camera: kind 2 from memory; new camera: re-rendered.
        assert_eq!(frame(&mut renderer, &params), texture);
        assert_eq!(renderer.shadow_stats().rendered, 0);
        frame(&mut renderer, &view(600.0));
        assert_eq!(renderer.shadow_stats().rendered, 1);
        // Prepared but not drawn: the texture does not count as current and is drawn in the
        // next frame.
        renderer.set_level_shadows(&device, &queue, &vfs, &mut models, &scene, 2);
        renderer.prepare_shadows(&device, &queue, &scene.terrain, &all, &params);
        assert_eq!(renderer.shadow_stats().rendered, 1);
        renderer.prepare_shadows(&device, &queue, &scene.terrain, &all, &params);
        assert_eq!(renderer.shadow_stats().rendered, 1);
        assert_eq!(frame(&mut renderer, &params), texture);
        assert_eq!(renderer.shadow_stats().rendered, 1);
        assert_eq!(frame(&mut renderer, &params), texture);
        assert_eq!(renderer.shadow_stats().rendered, 0);
        // Dead with kind 2: no shadow texture on the ground, alive again the same.
        renderer.entries_mut()[0].status = DIED;
        let dead = frame(&mut renderer, &params);
        assert_eq!(renderer.shadow_stats().persistent, 0);
        assert!(at(&dead, 434.0, 400.0)[1].abs_diff(200) < 3);
        renderer.entries_mut()[0].status = 0;
        assert_eq!(frame(&mut renderer, &params), texture);
        // Fewer entries, rendered without a new `prepare`: no crash, no old shadow.
        renderer.set_shadows(
            &device,
            &queue,
            &vfs,
            &mut models,
            Vec::new(),
            (Vec3::Y, 0),
            (256.0, 256.0),
        );
        let bare = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
            renderer.render(&device, &queue, encoder, target, size, &params);
        })
        .unwrap();
        assert_eq!(bare, plain);
        // Level 0: nothing.
        renderer.set_level_shadows(&device, &queue, &vfs, &mut models, &scene, 0);
        assert_eq!(frame(&mut renderer, &params), plain);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// TGA 8 x 8 (32 bit), transparent except for a white, opaque spot in the image rows
    /// and columns 1 ... 2 (file row `r` becomes image row `7 − r`).
    fn spot_tga() -> Vec<u8> {
        let mut tga = vec![0u8; 18];
        tga[2] = 2;
        tga[12] = 8;
        tga[14] = 8;
        tga[16] = 32;
        for r in 0..8 {
            let row = 7 - r;
            for col in 0..8 {
                let on = (1..=2).contains(&row) && (1..=2).contains(&col);
                tga.extend_from_slice(if on { &[255; 4] } else { &[0; 4] });
            }
        }
        tga
    }

    /// Footprints and craters (layer 4, step 7) in the synthetic level from above: crater `0x430`
    /// (`bomb.tga`, r 96) with a spot at the top left of the texture. Yaw 0 places it at (-48, +48)
    /// around the position (up = +z), yaw pi/2 turns it to `(x, z) → (−z, x)` (texture matrix
    /// `[[c, s], [−s, c]]`). Under the crate the mark lies below the shadow decal (black,
    /// opaque): it covers it because the marks are drawn before the shadows. Skipped
    /// without a GPU.
    // QUIRK D-268: kept
    #[test]
    fn footprints_draw_before_shadows_rotated() {
        use nml_effects::marks::Footprint;
        use nml_effects::output::{GroundDecal, GroundMarks};
        use std::f32::consts::FRAC_PI_2;
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let prj = PRJ.replace(
            "\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n",
            "\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n\tOBJ_SHADOW_FILENAME \"data\\r3d\\test\\shd.rdw\"\r\n",
        );
        let mut shd = sample_rdo(RdoLayout::Standard);
        shd.bounds = bounds([-100.0, 0.0, -100.0], [100.0, 1.0, 100.0]);
        shd.materials[0].texture = "shd.tga".into();
        let mut black = vec![0u8; 18];
        black[2] = 2;
        black[12] = 1;
        black[14] = 1;
        black[16] = 32;
        black.extend_from_slice(&[0, 0, 0, 255]);
        let property = "GUID 10\r\nNAME Kiste\r\nVALUEID 100 6 1 16777216\r\nPROPID 3 2\r\n";
        let wld = WLD.replace(
            "[OBJECT]",
            "[SHADOW]\r\n{\r\n\tSHADOW_LIGHT_POSITION 0 1 -1\r\n\tSHADOW_INTENSITY 128\r\n}\r\n[OBJECT]",
        );
        let dir = level_files(
            &wld,
            &prj,
            &[
                ("data/gifcon/property.ini", property.as_bytes().to_vec()),
                ("data/r3d/test/shd.rdo", write_rdo(&shd)),
                ("data/r3d/test/shd.tga", black),
                ("data/r3d/nature/summer/footsteps/bomb.tga", spot_tga()),
            ],
        );
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let size = (64, 64);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let frame = |renderer: &mut SceneRenderer, marks: &GroundMarks| {
            renderer.set_ground_marks(&device, &queue, &vfs, &scene.terrain, marks);
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
                renderer.render(&device, &queue, encoder, target, size, &params);
            })
            .unwrap()
        };
        // World point (x, z) -> pixel (view from above, up = +z).
        let at = |img: &image::RgbaImage, x: f32, z: f32| {
            let i = ((x - 256.0) / 256.0 + 1.0) / 2.0 * 64.0;
            let j = (1.0 - (z - 256.0) / 256.0) / 2.0 * 64.0;
            img.get_pixel(i as u32, j as u32).0
        };
        let grass = |p: [u8; 4]| p[1].abs_diff(200) < 3 && p[0] < 3 && p[2] < 3;
        let white = |p: [u8; 4]| p[0] > 250 && p[1] > 250 && p[2] > 250;
        let crater = |position: Vec3, yaw: f32, alpha: u8| GroundDecal {
            texture: Footprint::texture(0x430),
            position,
            yaw,
            radius: Footprint::radius(0x430),
            alpha,
        };
        let marks = |decals: Vec<GroundDecal>| GroundMarks {
            decals,
            treads: Vec::new(),
        };
        let plain = frame(&mut renderer, &GroundMarks::default());
        assert_eq!(renderer.ground_mark_stats(), GroundMarkStats::default());
        // Free area at the top left (no crate, no shadow): center (192, 320).
        let c = Vec3::new(192.0, 0.0, 320.0);
        let spot = |o: Vec3| (c.x + o.x, c.z + o.z);
        let (up_left, down_left) = (
            spot(Vec3::new(-44.0, 0.0, 44.0)),
            spot(Vec3::new(-44.0, 0.0, -44.0)),
        );
        assert!(grass(at(&plain, up_left.0, up_left.1)));
        assert!(grass(at(&plain, down_left.0, down_left.1)));
        let straight = frame(&mut renderer, &marks(vec![crater(c, 0.0, 255)]));
        assert_eq!(
            renderer.ground_mark_stats(),
            GroundMarkStats {
                decals: 1,
                tread_quads: 0
            }
        );
        let p = at(&straight, up_left.0, up_left.1);
        assert!(white(p), "{p:?}");
        assert!(grass(at(&straight, down_left.0, down_left.1)));
        // Outside the spot the grass remains (the texture is transparent there).
        assert!(grass(at(&straight, c.x + 44.0, c.z - 44.0)));
        // Rotated by pi/2: (-44, 44) -> (-44, -44).
        let turned = frame(&mut renderer, &marks(vec![crater(c, FRAC_PI_2, 255)]));
        assert!(grass(at(&turned, up_left.0, up_left.1)));
        let p = at(&turned, down_left.0, down_left.1);
        assert!(white(p), "{p:?}");
        // Alpha 128: half over the grass (color only from the texture, alpha = texture * vertex).
        let half = frame(&mut renderer, &marks(vec![crater(c, 0.0, 128)]));
        let p = at(&half, up_left.0, up_left.1);
        assert!(p[0].abs_diff(128) < 4 && p[1].abs_diff(228) < 4, "{p:?}");
        // Unloadable texture: dropped, the other mark stays.
        let missing = GroundDecal {
            texture: Footprint::texture(1),
            ..crater(c, 0.0, 255)
        };
        let one = frame(&mut renderer, &marks(vec![missing, crater(c, 0.0, 255)]));
        assert_eq!(renderer.ground_mark_stats().decals, 1);
        assert_eq!(one, straight);
        // Under the crate (384, 256): spot at (340, 300), white without shadow, black with the opaque
        // shadow decal - the mark lies under the shadow.
        let under = marks(vec![crater(Vec3::new(384.0, 0.0, 256.0), 0.0, 255)]);
        let p = at(&frame(&mut renderer, &under), 340.0, 300.0);
        assert!(white(p), "{p:?}");
        renderer.set_level_shadows(&device, &queue, &vfs, &mut models, &scene, 1);
        let all = nml_world::shadows::TileMask::all(4, 4);
        renderer.prepare_shadows(&device, &queue, &scene.terrain, &all, &params);
        let p = at(&frame(&mut renderer, &under), 340.0, 300.0);
        assert!(p[0] < 3 && p[1] < 3 && p[2] < 3, "{p:?}");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Wheel tracks (layer 4) in the synthetic level from above: black strips with alpha `0x60`
    /// over the grass, visible in both driving directions (front face clockwise, CULL CCW),
    /// with alpha per end. Skipped without a GPU.
    #[test]
    fn tread_marks_draw_black_strips() {
        use nml_effects::marks::TreadMark;
        use nml_effects::output::GroundMarks;
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let dir = level_files(WLD, PRJ, &[]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let size = (64, 64);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let frame = |renderer: &mut SceneRenderer, marks: &GroundMarks| {
            renderer.set_ground_marks(&device, &queue, &vfs, &scene.terrain, marks);
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
                renderer.render(&device, &queue, encoder, target, size, &params);
            })
            .unwrap()
        };
        let at = |img: &image::RgbaImage, x: f32, z: f32| {
            let i = ((x - 256.0) / 256.0 + 1.0) / 2.0 * 64.0;
            let j = (1.0 - (z - 256.0) / 256.0) / 2.0 * 64.0;
            img.get_pixel(i as u32, j as u32).0
        };
        let plain = frame(&mut renderer, &GroundMarks::default());
        // Real wheel track (half width 4, one pixel is 8 units): toward +z and toward -z.
        let mut up = TreadMark::new(2);
        up.add_point(Vec3::new(196.0, 0.0, 150.0), 0);
        up.add_point(Vec3::new(196.0, 0.0, 450.0), 0);
        let mut down = TreadMark::new(2);
        down.add_point(Vec3::new(300.0, 0.0, 450.0), 0);
        down.add_point(Vec3::new(300.0, 0.0, 150.0), 0);
        let flat = |_: f32, _: f32| 0.0;
        let seen = |_: Vec3| 7;
        let mut treads = up.quads(0, &flat, &seen);
        treads.extend(down.quads(0, &flat, &seen));
        assert_eq!(treads.len(), 2);
        // Made wider (32 instead of 4) so that whole pixels are covered.
        for q in &mut treads {
            let mid = (q.corners[0] + q.corners[1]) / 2.0;
            let mid_end = (q.corners[2] + q.corners[3]) / 2.0;
            q.corners[0] = mid + (q.corners[0] - mid) * 8.0;
            q.corners[1] = mid + (q.corners[1] - mid) * 8.0;
            q.corners[2] = mid_end + (q.corners[2] - mid_end) * 8.0;
            q.corners[3] = mid_end + (q.corners[3] - mid_end) * 8.0;
        }
        let marks = GroundMarks {
            decals: Vec::new(),
            treads: treads.clone(),
        };
        let striped = frame(&mut renderer, &marks);
        assert_eq!(renderer.ground_mark_stats().tread_quads, 2);
        // Grass * (1 - 0x60/255): 200 * 0.6235 ~ 125.
        for x in [196.0, 300.0] {
            assert!(at(&plain, x, 300.0)[1].abs_diff(200) < 3);
            let p = at(&striped, x, 300.0);
            assert!(
                p[0] < 3 && p[2] < 3 && p[1].abs_diff(125) < 4,
                "{p:?} at x {x}"
            );
        }
        assert_eq!(at(&striped, 250.0, 300.0), at(&plain, 250.0, 300.0));
        // Fade-out toward the end: start 0x60, end 0.
        let mut fading = treads[..1].to_vec();
        fading[0].alpha = [0x60, 0];
        let faded = frame(
            &mut renderer,
            &GroundMarks {
                decals: Vec::new(),
                treads: fading,
            },
        );
        let (start, end) = (at(&faded, 196.0, 160.0)[1], at(&faded, 196.0, 440.0)[1]);
        assert!(start < 135 && end > 190 && start < end, "{start} {end}");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Effect quads in the synthetic level, from above (parallel projection onto 512 x 512 around
    /// (256, 256)): additive in view space, lying flat in world coordinates with alpha blending
    /// and one before the water; white without a texture. Skipped without a GPU.
    #[test]
    fn effect_quads_render_offscreen() {
        use nml_effects::output::{
            EffectBlend, EffectFrame, EffectSpace, EffectVertex, QUAD_CORNERS, QUAD_UVS, d3d_color,
        };
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let dir = green_level();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let size = (64, 64);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        // Quad with half edge length 20 around `center`: in view space (x, y) or lying flat (x, z).
        let quad = |center: Vec3, color: u32, flat: bool| {
            QUAD_CORNERS
                .iter()
                .zip(QUAD_UVS)
                .map(move |(c, uv)| {
                    let corner = if flat {
                        Vec3::new(c[0], 0.0, c[1])
                    } else {
                        Vec3::new(c[0], c[1], 0.0)
                    };
                    EffectVertex {
                        position: (center + corner * 20.0).to_array(),
                        color,
                        specular: 0,
                        uv,
                    }
                })
                .collect::<Vec<_>>()
        };
        let mut above = EffectFrame::default();
        let center = params.view.transform_point3(Vec3::new(256.0, 100.0, 256.0));
        above.push_quads(
            None,
            EffectBlend::Additive,
            EffectSpace::View,
            quad(center, d3d_color(128.0, 255.0, 0.0, 0.0), false),
        );
        above.push_quads(
            Some("data/effects/missing.tga".into()),
            EffectBlend::Alpha,
            EffectSpace::World,
            quad(
                Vec3::new(400.0, 10.0, 400.0),
                d3d_color(255.0, 0.0, 0.0, 255.0),
                true,
            ),
        );
        let mut below = EffectFrame::default();
        below.push_quads(
            None,
            EffectBlend::Alpha,
            EffectSpace::World,
            quad(
                Vec3::new(150.0, 5.0, 400.0),
                d3d_color(255.0, 0.0, 0.0, 0.0),
                true,
            ),
        );
        renderer.set_effects(&device, &queue, &vfs, &mut models, &below, &above);
        assert_eq!(renderer.effect_quads(), 3);
        let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
            renderer.render(&device, &queue, encoder, target, size, &params);
        })
        .unwrap();
        let at = |x: f32, z: f32| {
            let i = ((x - 256.0) / 256.0 + 1.0) / 2.0 * 64.0;
            let j = (1.0 - (z - 256.0) / 256.0) / 2.0 * 64.0;
            img.get_pixel(i as u32, j as u32).0
        };
        let near = |p: [u8; 4], q: [u8; 3]| (0..3).all(|k| p[k].abs_diff(q[k]) <= 2);
        // Additive: grass (0, 200, 0) + red * 128/255.
        assert!(
            near(at(256.0, 256.0), [128, 200, 0]),
            "{:?}",
            at(256.0, 256.0)
        );
        // Missing texture: white * vertex color, opaque.
        assert!(
            near(at(400.0, 400.0), [0, 0, 255]),
            "{:?}",
            at(400.0, 400.0)
        );
        assert!(near(at(150.0, 400.0), [0, 0, 0]), "{:?}", at(150.0, 400.0));
        assert!(near(at(320.0, 150.0), [0, 200, 0]));
        // No effects: only grass again.
        renderer.set_effects(
            &device,
            &queue,
            &vfs,
            &mut models,
            &EffectFrame::default(),
            &EffectFrame::default(),
        );
        assert_eq!(renderer.effect_quads(), 0);
        let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
            renderer.render(&device, &queue, encoder, target, size, &params);
        })
        .unwrap();
        assert!(near(img.get_pixel(32, 32).0, [0, 200, 0]));
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Models in the effect pass, from above onto the green level (default light, ambient 0):
    /// a flat, white square (+/-50) tinted with alpha 0.5 (surf), as a model effect with
    /// `INVSRCALPHA`/`DESTALPHA` and alpha 0 (without alpha test, destination alpha acts as 1) and
    /// like an object (fish). Skipped without a GPU.
    #[test]
    fn effect_models_render_offscreen() {
        use nml_effects::output::{DrawCommand, EffectFrame, ModelDraw};
        use nml_formats::rdo::{Face, MeshKind};
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut quad = sample_rdo(RdoLayout::Standard);
        quad.meshes.truncate(1);
        quad.bones.clear();
        let mesh = &mut quad.meshes[0];
        mesh.kind = MeshKind::Rigid;
        mesh.pivot = [0.0; 3];
        mesh.position = [0.0; 3];
        mesh.positions = vec![
            [-50.0, 0.0, -50.0],
            [-50.0, 0.0, 50.0],
            [50.0, 0.0, -50.0],
            [50.0, 0.0, 50.0],
        ];
        mesh.faces = vec![
            Face {
                vertices: [0, 1, 2],
                material: 0,
            },
            Face {
                vertices: [1, 3, 2],
                material: 0,
            },
        ];
        mesh.uvs = vec![[0.0, 0.0]; 4];
        mesh.uv_faces = Some(vec![[0, 1, 2], [1, 3, 2]]);
        // Normal toward the default light (without `[LIGHT]`: direction to the light (0, 0, -1)).
        mesh.face_normals = vec![[0.0, 0.0, -1.0]; 2];
        mesh.vertex_normals = vec![[0.0, 0.0, -1.0]; 4];
        quad.materials.truncate(1);
        quad.materials[0].name = "Standard".into();
        quad.materials[0].texture = String::new();
        let dir = level_files(WLD, PRJ, &[("data/r3d/test/quad.rdo", write_rdo(&quad))]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let size = (64, 64);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let model = |x: f32, z: f32| ModelDraw {
            model: "data/r3d/test/quad.rdo".into(),
            world: Affine3A::from_translation(Vec3::new(x, 10.0, z)),
            animation: None,
            tint: None,
            alpha: None,
            blend: None,
            cull: false,
            fog: true,
            mesh: None,
            player_color: None,
        };
        let mut above = EffectFrame::default();
        above.commands.push(DrawCommand::Model(ModelDraw {
            tint: Some(2.0),
            alpha: Some(0.5),
            fog: false,
            ..model(150.0, 150.0)
        }));
        above.commands.push(DrawCommand::Model(ModelDraw {
            alpha: Some(0.0),
            blend: Some((6, 7)),
            fog: false,
            ..model(350.0, 350.0)
        }));
        above.commands.push(DrawCommand::Model(model(150.0, 350.0)));
        above.commands.push(DrawCommand::Model(ModelDraw {
            model: "data/r3d/test/missing.rdo".into(),
            ..model(350.0, 150.0)
        }));
        renderer.set_effects(
            &device,
            &queue,
            &vfs,
            &mut models,
            &EffectFrame::default(),
            &above,
        );
        assert_eq!(renderer.effect_models(), 3);
        let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
            renderer.render(&device, &queue, encoder, target, size, &params);
        })
        .unwrap();
        let at = |x: f32, z: f32| {
            let i = ((x - 256.0) / 256.0 + 1.0) / 2.0 * 64.0;
            let j = (1.0 - (z - 256.0) / 256.0) / 2.0 * 64.0;
            img.get_pixel(i as u32, j as u32).0
        };
        let near = |p: [u8; 4], q: [u8; 3]| (0..3).all(|k| p[k].abs_diff(q[k]) <= 3);
        // Tinted (2 -> clamped to 1), alpha 0.5 over grass.
        assert!(
            near(at(150.0, 150.0), [128, 228, 128]),
            "{:?}",
            at(150.0, 150.0)
        );
        // White * (1 - 0) + grass * 1: saturated white.
        assert!(
            near(at(350.0, 350.0), [255, 255, 255]),
            "{:?}",
            at(350.0, 350.0)
        );
        // Like an object: opaque white.
        assert!(
            near(at(150.0, 350.0), [255, 255, 255]),
            "{:?}",
            at(150.0, 350.0)
        );
        // Missing model: nothing.
        assert!(
            near(at(350.0, 150.0), [0, 200, 0]),
            "{:?}",
            at(350.0, 150.0)
        );
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// World models (rally point flag, re/world-display W.1, D-258): its own list directly after
    /// the objects, replaced per frame (drawn once instead of three times), not in the effect counters;
    /// the first `EF-MATERIAL` carries the player color of the order. Flat square from above onto
    /// the green level. Skipped without a GPU.
    #[test]
    fn one_flag_draw_per_frame() {
        use nml_effects::output::ModelDraw;
        use nml_formats::rdo::{Face, MeshKind};
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut quad = sample_rdo(RdoLayout::Standard);
        quad.meshes.truncate(1);
        quad.bones.clear();
        let mesh = &mut quad.meshes[0];
        mesh.kind = MeshKind::Rigid;
        mesh.pivot = [0.0; 3];
        mesh.position = [0.0; 3];
        mesh.positions = vec![
            [-50.0, 0.0, -50.0],
            [-50.0, 0.0, 50.0],
            [50.0, 0.0, -50.0],
            [50.0, 0.0, 50.0],
        ];
        mesh.faces = vec![
            Face {
                vertices: [0, 1, 2],
                material: 0,
            },
            Face {
                vertices: [1, 3, 2],
                material: 0,
            },
        ];
        mesh.uvs = vec![[0.0, 0.0]; 4];
        mesh.uv_faces = Some(vec![[0, 1, 2], [1, 3, 2]]);
        mesh.face_normals = vec![[0.0, 0.0, -1.0]; 2];
        mesh.vertex_normals = vec![[0.0, 0.0, -1.0]; 4];
        quad.materials.truncate(1);
        quad.materials[0].name = "EF-MATERIAL".into();
        quad.materials[0].texture = String::new();
        let dir = level_files(WLD, PRJ, &[("data/r3d/test/flag.rdo", write_rdo(&quad))]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let size = (64, 64);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let flag = ModelDraw {
            model: "data/r3d/test/flag.rdo".into(),
            world: Affine3A::from_translation(Vec3::new(256.0, 10.0, 256.0)),
            animation: None,
            tint: None,
            alpha: None,
            blend: None,
            cull: false,
            fog: true,
            mesh: None,
            player_color: Some([1.0, 0.0, 0.0]),
        };
        // Two frames in a row: the list is replaced, not extended.
        for _ in 0..2 {
            renderer.set_world_models(
                &device,
                &queue,
                &vfs,
                &mut models,
                std::slice::from_ref(&flag),
            );
        }
        assert_eq!(renderer.world_model_count(), 1);
        assert_eq!(renderer.effect_models(), 0);
        let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
            renderer.render(&device, &queue, encoder, target, size, &params);
        })
        .unwrap();
        let p = img.get_pixel(32, 32).0;
        assert!(
            p[0] >= 250 && p[1] <= 3 && p[2] <= 3,
            "player color red instead of the .rdo color: {p:?}"
        );
        renderer.set_world_models(&device, &queue, &vfs, &mut models, &[]);
        assert_eq!(renderer.world_model_count(), 0);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Light change, snow and water in detail mode in the synthetic level, from above
    /// (parallel projection onto 80 x 80 units around (256, 256)). Skipped without a GPU.
    #[test]
    fn light_presets_weather_and_water_detail_render_offscreen() {
        use nml_formats::lightdata::{LightBlock, LightPreset};
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let dir = green_level();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let size = (64, 64);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 40.0, 1.0 / 40.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let frame = |renderer: &mut SceneRenderer| {
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
                renderer.render(&device, &queue, encoder, view, size, &params);
            })
            .unwrap()
            .get_pixel(32, 32)
            .0
        };
        // Grass (0, 200, 0) times vertex color white.
        let day = frame(&mut renderer);
        assert!(day[0] < 3 && day[1].abs_diff(200) < 3, "{day:?}");
        // Terrain light (100, 100, 100), I = 100: f = 1 -> vertex color 100.
        let block = |color| LightBlock {
            position: [0.0, 1.0, 0.0],
            color,
            intensity: 100,
        };
        scene.apply_light(Some(&LightPreset {
            name: "Test".into(),
            terrain: block([100, 100, 100]),
            object: block([10, 10, 10]),
        }));
        renderer.set_light(&queue, &scene);
        let dark = frame(&mut renderer);
        assert!(dark[1].abs_diff(78) < 3, "{dark:?}");
        // Snow above the image center: white additive.
        renderer.set_weather(
            &device,
            &queue,
            Some(WeatherKind::Snow),
            &[[256.0, 100.0, 256.0]],
        );
        assert_eq!(renderer.weather_count(), 1);
        let snow = frame(&mut renderer);
        assert!(snow[0] > 250 && snow[2] > 250, "{snow:?}");
        // Rain without a texture in the archive: nothing drawn; no weather: the terrain again.
        renderer.set_weather(
            &device,
            &queue,
            Some(WeatherKind::Rain),
            &[[256.0, 100.0, 256.0]],
        );
        assert_eq!(frame(&mut renderer), dark);
        assert_eq!(renderer.weather_count(), 0);
        renderer.set_weather(&device, &queue, None, &[]);
        assert_eq!(renderer.weather_count(), 0);
        // Water in detail mode draws without a validation error.
        renderer.set_water_detail(true);
        frame(&mut renderer);
        scene.apply_light(None);
        renderer.set_light(&queue, &scene);
        assert_eq!(frame(&mut renderer), day);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Renders a synthetic level without a window; skipped without a GPU. Checks that all
    /// shaders and pipelines are valid and terrain, object and water are in the image
    /// (front faces correct).
    #[test]
    fn renders_a_synthetic_level_offscreen() {
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let dir = green_level();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let mut camera = GameCamera::new(CameraSetup::default(), &scene.terrain);
        let focus = start_focus(&scene);
        camera.move_to(focus.x, focus.y, &scene.terrain);
        let size = (64, 64);
        let params = ViewParams {
            view: camera.view(),
            projection: camera.projection(AspectMode::Correct, size.0, size.1),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
            renderer.render(&device, &queue, encoder, view, size, &params);
        })
        .unwrap();
        // Image center = camera target on the terrain: texture (0, 200, 0) times vertex color white.
        let center = img.get_pixel(32, 32).0;
        assert!(
            center[0] < 3 && center[1].abs_diff(200) < 3 && center[2] < 3,
            "{center:?}"
        );
        // Corner: no terrain (black background).
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 255]);
        // Without simulation and without `OBJ_COLOR`: player color of slot 8 (light gray).
        assert_eq!(renderer.entries()[0].look.color, Some([0xAC; 3]));
        let pixel = |world: Vec3| {
            let ndc = (params.projection * params.view).project_point3(world);
            let x = ((ndc.x + 1.0) / 2.0 * size.0 as f32) as u32;
            let y = ((1.0 - ndc.y) / 2.0 * size.1 as f32) as u32;
            img.get_pixel(x, y).0
        };
        // Center of the transparent koerper (white, alpha 0.5) in front of the black background. The
        // world matrix scales with `OBJ_SCALE` 5; as in the D3D fixed function without
        // NORMALIZENORMALS, the normal is transformed with the inverse transpose (length 1/5):
        // light frontal 0.2 * white * alpha 0.5 ~ 26.
        let koerper = pixel(Vec3::new(434.0, 300.0, 256.0));
        assert!(
            koerper[..3].iter().all(|c| c.abs_diff(26) <= 3),
            "{koerper:?}"
        );
        // Water (vertex color white, alpha rising toward the deep corner) over green terrain.
        let water = pixel(Vec3::new(16.0, -10.0, 16.0));
        assert!(
            water[0] > 60 && water[0] == water[2] && water[1] > water[0],
            "{water:?}"
        );
    }

    /// Game directory with a sub-object `hut.rdo` far to the right (x ~ 500, outside the screen).
    fn with_sub_object() -> (tempfile::TempDir, Arc<ModelAsset>) {
        let mut hut = sample_rdo(RdoLayout::Standard);
        for mesh in &mut hut.meshes {
            for p in &mut mesh.positions {
                p[0] += 480.0;
            }
            // Move the mesh hulls along (overall hull for the ray test).
            let (mut min, mut max) = (mesh.bounds.min, mesh.bounds.max);
            min[0] += 480.0;
            max[0] += 480.0;
            mesh.bounds = bounds(min, max);
        }
        hut.bounds = bounds([480.0, 0.0, 0.0], [520.0, 120.0, 10.0]);
        let hut = write_rdo(&hut);
        let dir = tempfile::tempdir().unwrap();
        let files = [TestFile {
            path: "data/r3d/test/hut.rdo",
            data: &hut,
            compress: false,
        }];
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let mut asset = Arc::into_inner(animated_model("data/r3d/test/kiste.rdo", false)).unwrap();
        // Object level with `INDEX` 0: drawn without animation.
        asset.config.sub_objects.push(SubObject {
            file: "data/r3d/test/hut.rdo".into(),
            index: 0,
            start: 0,
            looping: true,
        });
        (dir, Arc::new(asset))
    }

    /// Model browser scene without terrain: bind pose, animated (skin bones, node track,
    /// morph frame) and back to the bind pose; plus picking with a sub-object. Skipped
    /// without a GPU.
    #[test]
    fn renders_animated_models_offscreen() {
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (dir, rigid) = with_sub_object();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut models = ModelCache::new();
        let morph = animated_model("data/r3d/test/morph.rdo", true);
        let background = [0.0, 0.0, 1.0];
        let mut renderer =
            SceneRenderer::models_only(&device, &queue, CAPTURE_FORMAT, None, background);
        renderer.entries_mut().extend([
            // A: rigid kiste with node track, skin koerper; object 3; sub-object hut.
            ModelEntry {
                playback: Playback::new(&rigid, |file| models.get(&vfs, file), Some(0), 0.0),
                asset: rigid,
                world: Affine3A::IDENTITY,
                object: Some(3),
                look: EntryLook::default(),
                scaffold: None,
                ship: false,
                status: 0,
                resource: false,
                hidden_meshes: Arc::from([]),
            },
            // B: kiste with morph track, skin koerper; without object index.
            ModelEntry {
                playback: Playback::new(&morph, |file| models.get(&vfs, file), Some(0), 0.0),
                asset: morph,
                world: Affine3A::from_translation(Vec3::new(-25.0, 0.0, 0.0)),
                object: None,
                look: EntryLook::default(),
                scaffold: None,
                ship: false,
                status: 0,
                resource: false,
                hidden_meshes: Arc::from([]),
            },
        ]);
        // Camera at z = -100 looking toward +z, parallel projection onto x in [-40, 40], y in [0, 80]
        // (depth view-space z / 300); pixel (i, j) <-> world ((i + 1/2) / 1.6 - 40, 80 - (j + 1/2) / 1.6).
        let size = (128, 128);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(0.0, 40.0, -100.0),
                Vec3::new(0.0, 40.0, 0.0),
                Vec3::Y,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 40.0, 1.0 / 40.0, 1.0 / 300.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let mut frame = |now_ms: f32, animate: bool| {
            renderer.update(&device, &queue, &vfs, now_ms, animate);
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
                renderer.render(&device, &queue, encoder, view, size, &params);
            })
            .unwrap()
        };
        let rest = frame(0.0, false);
        // 900 ms = 4,320 ticks: bone rotated 81 degrees about y and 9 higher, morph frame y + 27.
        let animated = frame(900.0, true);
        let back = frame(1000.0, false);

        let near = |p: [u8; 4], q: [u8; 3]| (0..3).all(|k| p[k].abs_diff(q[k]) <= 2);
        let blue = [0, 0, 255];
        // Transparent white koerper (alpha 0.5) over blue.
        let koerper = [128, 128, 255];
        // EF-MATERIAL diffuse (0.8; 0.6; 0.4), light frontal.
        let kiste = [204, 153, 102];
        assert!(near(rest.get_pixel(0, 0).0, blue));
        // World (4.1; 50.3): koerper of A in the bind pose, when rotated, only x in 8.4...11.6 is left.
        let p = (70, 47);
        assert!(
            near(rest.get_pixel(p.0, p.1).0, koerper),
            "{:?}",
            rest.get_pixel(p.0, p.1)
        );
        assert!(
            near(animated.get_pixel(p.0, p.1).0, blue),
            "{:?}",
            animated.get_pixel(p.0, p.1)
        );
        // World (-22.2; 32.2): koerper of B in the bind pose, animated the morph frame of the kiste.
        let q = (28, 76);
        assert!(
            near(rest.get_pixel(q.0, q.1).0, koerper),
            "{:?}",
            rest.get_pixel(q.0, q.1)
        );
        assert!(
            near(animated.get_pixel(q.0, q.1).0, kiste),
            "{:?}",
            animated.get_pixel(q.0, q.1)
        );
        // Without `animate`, the bind pose again.
        assert_eq!(back, rest);

        // Two entries, three instances (hut of the object level runs with A in the bind pose); hut
        // counts toward object 3, B is not selectable.
        assert_eq!(renderer.instance_count(), 2);
        assert_eq!(renderer.entries().len(), 2);
        assert_eq!(renderer.objects.instances().len(), 3);
        let pick = |x: f32| renderer.pick(Vec3::new(x, 60.0, -1000.0), Vec3::Z);
        assert_eq!(pick(500.0), Some(3));
        assert_eq!(pick(10.0), Some(3));
        assert_eq!(pick(-70.0), None);

        // Construction site (render bit 4, progress 0.5, without scaffold): A only up to half the height of
        // the header hull (0 ... 120 -> 60); world (4.1; 70) becomes background, (4.1; 50.3) stays koerper.
        let top = (70, 16);
        assert!(near(rest.get_pixel(top.0, top.1).0, koerper));
        renderer.entries_mut()[0].look = EntryLook {
            bits: RENDER_BUILD,
            progress: 0.5,
            brightness: 1.0,
            alpha: 1.0,
            color: None,
        };
        let still = |renderer: &mut SceneRenderer| {
            renderer.update(&device, &queue, &vfs, 0.0, false);
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
                renderer.render(&device, &queue, encoder, view, size, &params);
            })
            .unwrap()
        };
        let site = still(&mut renderer);
        assert!(
            near(site.get_pixel(top.0, top.1).0, blue),
            "{:?}",
            site.get_pixel(top.0, top.1)
        );
        assert!(
            near(site.get_pixel(p.0, p.1).0, koerper),
            "{:?}",
            site.get_pixel(p.0, p.1)
        );
        // Invisible (render bits 0): A is missing entirely and is no longer clickable.
        renderer.entries_mut()[0].look = EntryLook {
            bits: 0,
            progress: 1.0,
            brightness: 1.0,
            alpha: 1.0,
            color: None,
        };
        let hidden = still(&mut renderer);
        assert!(near(hidden.get_pixel(p.0, p.1).0, blue));
        assert_eq!(renderer.pick(Vec3::new(10.0, 60.0, -1000.0), Vec3::Z), None);
        // Burning at 0.5: dark tinted above, as usual below.
        renderer.entries_mut()[0].look = EntryLook {
            bits: RENDER_SHADOW | RENDER_BURNING,
            progress: 0.5,
            brightness: 1.0,
            alpha: 1.0,
            color: None,
        };
        let burning = still(&mut renderer);
        let dark = burning.get_pixel(top.0, top.1).0;
        assert!(!near(dark, koerper) && !near(dark, blue), "{dark:?}");
        assert!(near(burning.get_pixel(p.0, p.1).0, koerper));
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Player color (`ObjectRenderer_Draw` `0x00549AC0` -> `FUN_005884C0`): the first `EF-MATERIAL`
    /// of the main model and its sub-objects appears in the color of the entry instead of the
    /// `.rdo` color; the scaffold stays without. Skipped without a GPU.
    // QUIRK D-39: kept
    #[test]
    fn the_player_colour_reaches_the_model_and_its_sub_objects_but_not_the_scaffold() {
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (dir, rigid) = with_sub_object();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::models_only(&device, &queue, CAPTURE_FORMAT, None, [0.0, 0.0, 1.0]);
        renderer.entries_mut().push(ModelEntry {
            playback: Playback::new(&rigid, |file| models.get(&vfs, file), None, 0.0),
            asset: rigid,
            world: Affine3A::IDENTITY,
            object: Some(0),
            look: EntryLook::default(),
            scaffold: None,
            ship: false,
            status: 0,
            resource: false,
            hidden_meshes: Arc::from([]),
        });
        // Same camera as in `renders_animated_models_offscreen`.
        let size = (128, 128);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(0.0, 40.0, -100.0),
                Vec3::new(0.0, 40.0, 0.0),
                Vec3::Y,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 40.0, 1.0 / 40.0, 1.0 / 300.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let frame = |renderer: &mut SceneRenderer, color: Option<[u8; 3]>| {
            renderer.entries_mut()[0].look.color = color;
            renderer.update(&device, &queue, &vfs, 0.0, false);
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
                renderer.render(&device, &queue, encoder, view, size, &params);
            })
            .unwrap()
        };
        let near = |p: [u8; 4], q: [u8; 3]| (0..3).all(|k| p[k].abs_diff(q[k]) <= 2);
        // Without a color the `.rdo` color of the kiste (0.8; 0.6; 0.4), light frontal, behind the
        // transparent white koerper (alpha 0.5): (c + 255) / 2.
        let behind = |c: [u8; 3]| c.map(|v| ((u16::from(v) + 255) / 2) as u8);
        let plain = frame(&mut renderer, None);
        let kiste: Vec<(u32, u32)> = plain
            .enumerate_pixels()
            .filter(|(_, _, p)| near(p.0, behind([204, 153, 102])))
            .map(|(x, y, _)| (x, y))
            .collect();
        assert!(!kiste.is_empty());
        // With the color of slot 3 (`g_PlayerColors[3]` = C4 18 00).
        let red = [0xC4, 0x18, 0x00];
        let colored = frame(&mut renderer, Some(red));
        for &(x, y) in &kiste {
            assert!(
                near(colored.get_pixel(x, y).0, behind(red)),
                "{:?}",
                colored.get_pixel(x, y)
            );
        }
        // Main model and sub-object hut get the color, the scaffold does not.
        let player = Some([0xC4 as f32 / 255.0, 0x18 as f32 / 255.0, 0.0]);
        let scaffold = animated_model("data/r3d/test/scaffold.rdo", false);
        let entry = &mut renderer.entries_mut()[0];
        entry.scaffold = Some(scaffold);
        entry.ship = true;
        renderer.update(&device, &queue, &vfs, 0.0, false);
        let colors: Vec<Option<[f32; 3]>> = renderer
            .objects
            .instances()
            .iter()
            .map(|i| i.look.player_color)
            .collect();
        assert_eq!(colors, [None, player, player]);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Build preview like `Cursor3D_RenderWorldModel` (`0x004CD130`): the whole model with
    /// sub-objects, opaque groups opaque, transparent ones with their `.rdo` alpha, all
    /// materials multiplicatively tinted (`Material_MakeTinted`), without shadow and without
    /// influence on entries and ray test. Skipped without a GPU.
    #[test]
    fn build_previews_draw_the_whole_model_tinted() {
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (dir, rigid) = with_sub_object();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::models_only(&device, &queue, CAPTURE_FORMAT, None, [0.0, 0.0, 1.0]);
        // Same camera as in `renders_animated_models_offscreen`.
        let size = (128, 128);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(0.0, 40.0, -100.0),
                Vec3::new(0.0, 40.0, 0.0),
                Vec3::Y,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 40.0, 1.0 / 40.0, 1.0 / 300.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let mut frame = |renderer: &mut SceneRenderer, tint: Option<[f32; 3]>| {
            let pieces: Vec<PreviewPiece> = tint
                .map(|tint| PreviewPiece {
                    asset: rigid.clone(),
                    world: Affine3A::IDENTITY,
                    tint,
                })
                .into_iter()
                .collect();
            renderer.set_build_preview(&device, &queue, &vfs, &mut models, &pieces);
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
                renderer.render(&device, &queue, encoder, view, size, &params);
            })
            .unwrap()
        };
        let near = |p: [u8; 4], q: [u8; 3]| (0..3).all(|k| p[k].abs_diff(q[k]) <= 2);
        // World (4.1; 50.3): koerper (white, alpha 0.5) over blue.
        let p = (70, 47);
        let green = frame(&mut renderer, Some([0.0, 1.0, 0.0]));
        assert!(
            near(green.get_pixel(p.0, p.1).0, [0, 128, 128]),
            "{:?}",
            green.get_pixel(p.0, p.1)
        );
        // Model and sub-object hut, both tinted; no entries, nothing pickable.
        assert_eq!(renderer.preview.instances().len(), 2);
        assert!(
            renderer
                .preview
                .instances()
                .iter()
                .all(|i| i.look.tint == Some([0.0, 1.0, 0.0]) && i.pose.is_none())
        );
        assert!(renderer.entries().is_empty());
        assert_eq!(renderer.pick(Vec3::new(10.0, 60.0, -1000.0), Vec3::Z), None);
        let red = frame(&mut renderer, Some([1.0, 0.0, 0.0]));
        assert!(
            near(red.get_pixel(p.0, p.1).0, [128, 0, 128]),
            "{:?}",
            red.get_pixel(p.0, p.1)
        );
        // Without a preview: only the background.
        let none = frame(&mut renderer, None);
        assert!(near(none.get_pixel(p.0, p.1).0, [0, 0, 255]));
        assert!(renderer.preview.instances().is_empty());
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    // QUIRK D-222: kept
    #[test]
    fn fog_memories_draw_the_remembered_look_with_their_brightness() {
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (dir, rigid) = with_sub_object();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::models_only(&device, &queue, CAPTURE_FORMAT, None, [0.0, 0.0, 1.0]);
        let piece = |bits, progress| MemoryPiece {
            asset: rigid.clone(),
            world: Affine3A::IDENTITY,
            look: EntryLook {
                bits,
                progress,
                brightness: 0.5,
                alpha: 1.0,
                color: Some([0x00, 0x73, 0xE6]),
            },
            position: Vec3::ZERO,
            yaw: 0.0,
            special_decals: Vec::new(),
        };
        // Finished: model and sub-object whole, brightness of the image, no shadow, nothing pickable.
        let pieces = [piece(RENDER_MODEL | RENDER_SHADOW, 1.0)];
        renderer.set_fog_memories(&device, &queue, &vfs, &mut models, &pieces);
        let looks: Vec<InstanceLook> = renderer
            .memories
            .instances()
            .iter()
            .map(|i| i.look)
            .collect();
        assert_eq!(looks.len(), 2);
        assert!(
            looks
                .iter()
                .all(|l| l.brightness == 0.5 && l.cut.is_none() && l.tint.is_none())
        );
        // Player color of the piece for model and sub-object (`0x00548B30` via the object renderer).
        let blue = [0.0, 0x73 as f32 / 255.0, 0xE6 as f32 / 255.0];
        assert!(looks.iter().all(|l| l.player_color == Some(blue)));
        assert!(renderer.entries().is_empty() && renderer.objects.instances().is_empty());
        // Construction site without scaffold: cut by the remembered progress; shadow only: nothing.
        let pieces = [piece(RENDER_BUILD, 0.5), piece(RENDER_SHADOW, 1.0)];
        renderer.set_fog_memories(&device, &queue, &vfs, &mut models, &pieces);
        assert_eq!(renderer.memories.instances().len(), 2);
        assert!(
            renderer
                .memories
                .instances()
                .iter()
                .all(|i| i.look.cut.is_some())
        );
        renderer.set_fog_memories(&device, &queue, &vfs, &mut models, &[]);
        assert!(renderer.memories.instances().is_empty());
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Active fog memory of a gold deposit in the synthetic level from above: the special-class
    /// decal (white texture, hull +/-50) lies at the remembered position (150, 150) in
    /// `k' = trunc(k · 255)` (`0x00548B30`), only with remembered render bit 1. The model of the
    /// image stands outside the screen. Skipped without a GPU.
    #[test]
    fn fog_memories_draw_the_special_decal_with_their_brightness() {
        use nml_world::shadows::TileMask;
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut gold = sample_rdo(RdoLayout::Standard);
        gold.bounds = bounds([-50.0, 0.0, -50.0], [50.0, 1.0, 50.0]);
        gold.materials[0].texture = "gold.tga".into();
        let mut white = vec![0u8; 18];
        white[2] = 2;
        white[12] = 1;
        white[14] = 1;
        white[16] = 32;
        white.extend_from_slice(&[255, 255, 255, 255]);
        let dir = level_files(
            WLD,
            PRJ,
            &[
                ("data/r3d/test/gold.rdo", write_rdo(&gold)),
                ("data/r3d/test/gold.tga", white),
            ],
        );
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        // Level 0: the crate of the map casts no shadow.
        renderer.set_level_shadows(&device, &queue, &vfs, &mut models, &scene, 0);
        let asset = models.get(&vfs, "data/r3d/test/kiste.rdo").unwrap();
        let size = (64, 64);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let all = TileMask::all(4, 4);
        let mut frame = |pieces: &[MemoryPiece]| {
            renderer.set_fog_memories(&device, &queue, &vfs, &mut models, pieces);
            renderer.prepare_shadows(&device, &queue, &scene.terrain, &all, &params);
            let specials = renderer.shadow_stats().specials;
            let img = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
                renderer.render(&device, &queue, encoder, target, size, &params);
            })
            .unwrap();
            // World point (150, 150) -> pixel (view from above, up = +z).
            let i = ((150.0 - 256.0) / 256.0 + 1.0) / 2.0 * 64.0;
            let j = (1.0 - (150.0 - 256.0) / 256.0) / 2.0 * 64.0;
            (specials, img.get_pixel(i as u32, j as u32).0)
        };
        let piece = |bits, brightness| MemoryPiece {
            asset: asset.clone(),
            world: Affine3A::from_translation(Vec3::new(5000.0, 0.0, 5000.0)),
            look: EntryLook {
                bits,
                progress: 1.0,
                brightness,
                alpha: 1.0,
                color: None,
            },
            position: Vec3::new(150.0, 0.0, 150.0),
            yaw: 0.0,
            special_decals: vec!["data/r3d/test/gold.rdo".into()],
        };
        let grass = |c: [u8; 4]| c[0] < 3 && c[1].abs_diff(200) < 3 && c[2] < 3;
        let (specials, plain) = frame(&[]);
        assert!(specials == 0 && grass(plain), "{plain:?}");
        // Half bright: 127 = trunc(0.5 * 255); fully bright: white.
        let (specials, half) = frame(&[piece(RENDER_MODEL | RENDER_SHADOW, 0.5)]);
        assert_eq!(specials, 1);
        assert!(half[..3].iter().all(|c| c.abs_diff(127) < 3), "{half:?}");
        let (_, full) = frame(&[piece(RENDER_MODEL | RENDER_SHADOW, 1.0)]);
        assert!(full[..3].iter().all(|&c| c > 252), "{full:?}");
        // Without remembered render bit 1 and without special class: no decal.
        let (specials, hidden) = frame(&[piece(RENDER_SHADOW, 1.0)]);
        assert!(specials == 0 && grass(hidden), "{hidden:?}");
        let plain_piece = MemoryPiece {
            special_decals: Vec::new(),
            ..piece(RENDER_MODEL | RENDER_SHADOW, 1.0)
        };
        let (specials, other) = frame(&[plain_piece]);
        assert!(specials == 0 && grass(other), "{other:?}");
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }

    /// Clear color (E-245): `ViewParams::clear` takes precedence; without it the fog color with fog,
    /// otherwise the background color (model browser blue). Skipped without a GPU.
    #[test]
    fn clear_color_overrides_fog_and_background() {
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let mut renderer =
            SceneRenderer::models_only(&device, &queue, CAPTURE_FORMAT, None, [0.0, 0.0, 1.0]);
        let size = (8, 8);
        let mut pixel = |fog: Option<[f32; 3]>, clear: Option<[f32; 3]>| {
            let params = ViewParams {
                view: Mat4::IDENTITY,
                projection: Mat4::IDENTITY,
                fog: fog.map(|color| FogSettings {
                    color,
                    start: 0.0,
                    end: 1.0,
                }),
                clear,
                now_ms: 0.0,
            };
            let image = render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, view| {
                renderer.render(&device, &queue, encoder, view, size, &params);
            })
            .unwrap();
            let p = image.get_pixel(4, 4).0;
            [p[0], p[1], p[2]]
        };
        assert_eq!(pixel(None, None), [0, 0, 255], "background");
        assert_eq!(pixel(Some([0.0, 1.0, 0.0]), None), [0, 255, 0], "fog color");
        assert_eq!(
            pixel(Some([0.0, 1.0, 0.0]), Some([1.0, 0.0, 0.0])),
            [255, 0, 0]
        );
        assert_eq!(
            pixel(None, Some([0.0; 3])),
            [0, 0, 0],
            "black despite background"
        );
    }

    /// Horizontal, white plate +/-50 around the origin (height 0, material "Standard" without texture,
    /// normal toward the default light), front face seen from above.
    fn plate_rdo(material: &str) -> nml_formats::rdo::RdoFile {
        use nml_formats::rdo::{Face, MeshKind};
        let mut plate = sample_rdo(RdoLayout::Standard);
        plate.meshes.truncate(1);
        plate.bones.clear();
        let mesh = &mut plate.meshes[0];
        mesh.kind = MeshKind::Rigid;
        mesh.pivot = [0.0; 3];
        mesh.position = [0.0; 3];
        mesh.positions = vec![
            [-50.0, 0.0, -50.0],
            [-50.0, 0.0, 50.0],
            [50.0, 0.0, -50.0],
            [50.0, 0.0, 50.0],
        ];
        mesh.faces = vec![
            Face {
                vertices: [0, 1, 2],
                material: 0,
            },
            Face {
                vertices: [1, 3, 2],
                material: 0,
            },
        ];
        mesh.uvs = vec![[0.0, 0.0]; 4];
        mesh.uv_faces = Some(vec![[0, 1, 2], [1, 3, 2]]);
        mesh.face_normals = vec![[0.0, 0.0, -1.0]; 2];
        mesh.vertex_normals = vec![[0.0, 0.0, -1.0]; 4];
        plate.bounds = bounds([-50.0, 0.0, -50.0], [50.0, 1.0, 50.0]);
        plate.materials.truncate(1);
        plate.materials[0].name = material.into();
        plate.materials[0].texture = String::new();
        plate.materials[0].transparency = 0.0;
        plate
    }

    /// Silhouette (render bit `0x80`, re/world-display W.2.1, D-263, D-264) in the synthetic level from
    /// above: the crate (with shadow decal like [`shadow_decals_and_textures_render_offscreen`]) shows
    /// a white plate. Opaque it gives the color `C`; as a silhouette on grass `0.1·C + 0.9·grass`
    /// (white without brightness factor (brightness 0.5) and without the opacity 0.6 of the entry),
    /// without shadow decal and not pickable; over the water `0.1·C + 0.9·(terrain with water)`, i.e.
    /// drawn after the water. A camouflaged unit without a drawn layer keeps its hull top (health bar
    /// of a selected foreign unit, D-266). Skipped without a GPU.
    // QUIRK D-263, D-264, D-266: kept
    #[test]
    fn stealth_layer_is_white_with_a_tenth_alpha_after_water_without_shadow() {
        use nml_world::shadows::TileMask;
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let prj = PRJ.replace(
            "\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n",
            "\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n\tOBJ_SHADOW_FILENAME \"data\\r3d\\test\\shd.rdw\"\r\n",
        );
        assert_ne!(prj, PRJ);
        let wld = WLD.replace(
            "[OBJECT]",
            "[SHADOW]\r\n{\r\n\tSHADOW_LIGHT_POSITION 0 1 -1\r\n\tSHADOW_INTENSITY 128\r\n}\r\n[OBJECT]",
        );
        let mut shd = sample_rdo(RdoLayout::Standard);
        shd.bounds = bounds([-100.0, 0.0, -100.0], [100.0, 1.0, 100.0]);
        shd.materials[0].texture = "shd.tga".into();
        let mut black = vec![0u8; 18];
        black[2] = 2;
        black[12] = 1;
        black[14] = 1;
        black[16] = 32;
        black.extend_from_slice(&[0, 0, 0, 255]);
        let property = "GUID 10\r\nNAME Kiste\r\nVALUEID 100 6 1 16777216\r\nPROPID 3 2\r\n";
        let dir = level_files(
            &wld,
            &prj,
            &[
                ("data/gifcon/property.ini", property.as_bytes().to_vec()),
                ("data/r3d/test/shd.rdo", write_rdo(&shd)),
                ("data/r3d/test/shd.tga", black),
                ("data/r3d/test/plate.rdo", write_rdo(&plate_rdo("Standard"))),
                (
                    "data/r3d/test/plate_ef.rdo",
                    write_rdo(&plate_rdo("EF-MATERIAL")),
                ),
            ],
        );
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/t/t.lvl").unwrap();
        let mut models = ModelCache::new();
        let mut renderer =
            SceneRenderer::new(&device, &queue, CAPTURE_FORMAT, &vfs, &scene, &mut models);
        let plate = models.get(&vfs, "data/r3d/test/plate.rdo").unwrap();
        let plate_ef = models.get(&vfs, "data/r3d/test/plate_ef.rdo").unwrap();
        // Shadow decal at the crate (384, 256) +/-100, away from the plate.
        renderer.set_level_shadows(&device, &queue, &vfs, &mut models, &scene, 1);
        let size = (64, 64);
        let params = ViewParams {
            view: look_at_lh(
                Vec3::new(256.0, 500.0, 256.0),
                Vec3::new(256.0, 0.0, 256.0),
                Vec3::Z,
            ),
            projection: Mat4::from_scale(Vec3::new(1.0 / 256.0, 1.0 / 256.0, 1.0 / 1000.0)),
            fog: None,
            clear: None,
            now_ms: 0.0,
        };
        let all = TileMask::all(4, 4);
        let land = Vec3::new(150.0, 0.0, 400.0);
        // The plate around (40, 40) covers the deep corner of the water tile (0, 0); pixel at (20, 20).
        let water = Vec3::new(40.0, 0.0, 40.0);
        let frame = |renderer: &mut SceneRenderer,
                     asset: &Arc<ModelAsset>,
                     at: Vec3,
                     look: EntryLook,
                     status: u32| {
            let entry = &mut renderer.entries_mut()[0];
            entry.playback = Playback::new(asset, |_| None, None, 0.0);
            entry.asset = asset.clone();
            entry.world = Affine3A::from_translation(at);
            entry.look = look;
            entry.status = status;
            renderer.update(&device, &queue, &vfs, 0.0, false);
            renderer.prepare_shadows(&device, &queue, &scene.terrain, &all, &params);
            render_to_image(&device, &queue, size, CAPTURE_FORMAT, |encoder, target| {
                renderer.render(&device, &queue, encoder, target, size, &params);
            })
            .unwrap()
        };
        let at = |img: &image::RgbaImage, x: f32, z: f32| {
            let i = ((x - 256.0) / 256.0 + 1.0) / 2.0 * 64.0;
            let j = (1.0 - (z - 256.0) / 256.0) / 2.0 * 64.0;
            let p = img.get_pixel(i as u32, j as u32).0;
            [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
        };
        let ghost = |under: [f32; 3], c: [f32; 3]| {
            [0, 1, 2].map(|k| STEALTH_ALPHA * c[k] + (1.0 - STEALTH_ALPHA) * under[k])
        };
        let near = |p: [f32; 3], q: [f32; 3]| (0..3).all(|k| (p[k] - q[k]).abs() <= 3.0);
        let opaque_look = EntryLook::default();
        let stealth = EntryLook {
            bits: RENDER_STEALTH,
            brightness: 0.5,
            alpha: 0.6,
            color: Some([255, 0, 0]),
            ..EntryLook::default()
        };
        // Opaque: color C of the plate, one shadow decal.
        let opaque = frame(&mut renderer, &plate, land, opaque_look, 0);
        let c = at(&opaque, 150.0, 400.0);
        assert_eq!(renderer.shadow_stats().decals, 1);
        // Nothing drawn (bits 0): grass and terrain with water.
        let bare = frame(
            &mut renderer,
            &plate,
            land,
            EntryLook {
                bits: 0,
                ..opaque_look
            },
            0,
        );
        let grass = at(&bare, 150.0, 400.0);
        let under_water = at(&bare, 20.0, 20.0);
        assert!(
            !near(c, grass),
            "counter-check: plate {c:?} on grass {grass:?}"
        );
        assert!(!near(under_water, grass), "water {under_water:?}");
        assert!(!renderer.object_tops().iter().any(|(o, _)| *o == 0));
        // Silhouette on grass: white with 0.1, no shadow decal, no click target.
        let on_land = frame(&mut renderer, &plate, land, stealth, STATUS_MASKED);
        assert!(
            near(at(&on_land, 150.0, 400.0), ghost(grass, c)),
            "{:?} instead of {:?}",
            at(&on_land, 150.0, 400.0),
            ghost(grass, c)
        );
        assert_eq!(renderer.shadow_stats().decals, 0);
        let looks: Vec<InstanceLook> = renderer
            .stealth
            .instances()
            .iter()
            .map(|i| i.look)
            .collect();
        assert_eq!(looks.len(), 1);
        assert_eq!(looks[0].alpha, Some(STEALTH_ALPHA));
        assert!(looks[0].default_white);
        assert_eq!(
            renderer.pick(Vec3::new(150.0, 500.0, 400.0), Vec3::NEG_Y),
            None
        );
        // The overall hull remains the basis of the health bar (D-266).
        assert!(renderer.object_tops().iter().any(|(o, _)| *o == 0));
        // Over the water, with a plate in player color (red; distinguishable from the white water): drawn
        // after the water, so `0.1·C + 0.9·(terrain with water)`.
        let red = EntryLook {
            color: Some([255, 0, 0]),
            ..opaque_look
        };
        let opaque_red = frame(&mut renderer, &plate_ef, land, red, 0);
        let c_red = at(&opaque_red, 150.0, 400.0);
        let red_ghost = EntryLook {
            bits: RENDER_STEALTH,
            ..red
        };
        let on_water = frame(&mut renderer, &plate_ef, water, red_ghost, STATUS_MASKED);
        assert!(
            near(at(&on_water, 20.0, 20.0), ghost(under_water, c_red)),
            "{:?} instead of {:?}",
            at(&on_water, 20.0, 20.0),
            ghost(under_water, c_red)
        );
        // Camouflaged while standing (bits 0): nothing, but the hull remains; uncamouflaged without bits: none.
        let hidden = frame(
            &mut renderer,
            &plate,
            land,
            EntryLook { bits: 0, ..stealth },
            STATUS_MASKED,
        );
        assert_eq!(hidden, bare);
        assert!(renderer.stealth.instances().is_empty());
        assert!(renderer.object_tops().iter().any(|(o, _)| *o == 0));
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }
}
