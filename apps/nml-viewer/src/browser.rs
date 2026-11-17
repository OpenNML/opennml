//! Model browser: page through all models of the archives, play animations, rotate and zoom the model with the
//! mouse; shadows on a flat ground plane (F8); effects of the model with burn intensity and preview switch
//! (-/+, V).

use std::f32::consts::PI;
use std::sync::Arc;

use anyhow::{Result, bail};
use glam::{Affine3A, Mat4, Vec2, Vec3};
use nml_control::{AnimationState, BrowserState, OrbitState, PendingSwitch};
use nml_effects::browser::{DAMAGE_STEP, ModelEffects};
use nml_effects::output::EffectFrame;
use nml_effects::triggers::load_effect_list;
use nml_effects::world::{ObjectFrame, ObjectView};
use nml_formats::hef::DummyEffect;
use nml_formats::hlm::{Heightmap, HlmPoint};
use nml_formats::prj::{Project, parse_project};
use nml_formats::text::decode_cp1252;
use nml_formats::tlm::TextureMap;
use nml_formats::wld::{ObjectLight, TerrainParams};
use nml_rda::{Vfs, normalize_path};
use nml_render::{EntryShadow, ModelEntry, SceneRenderer, ViewParams};

use crate::snapshot::{BROWSER_EFFECT_QUALITY, effect_state, shadow_state};
use nml_world::anim::{AnimationPlayer, Playback, active_sub_objects, idle_animation};
use nml_world::assets::{ModelAsset, ModelCache};
use nml_world::camera::{AspectMode, CameraSetup, aspect_factor, game_projection, look_at_lh};
use nml_world::classes::{
    ObjectShadow, ObjectTypes, ShadowKind, object_shadow, object_shadow_as, type_for_model,
};
use nml_world::shadows::{TileMask, shadow_light_dir};
use nml_world::terrain::Terrain;
use nml_world::visibility::Frustum;

use nml_client::game::overlay::{OverlayLine, OverlayText};

/// Background of the browser (dark gray).
const BACKGROUND: [f32; 3] = [0.18, 0.2, 0.22];

/// Light of the browser: direction and color like `[LIGHT]` of kamp1-1, ambient 0.6.
fn browser_light() -> ObjectLight {
    ObjectLight {
        position: [800.0, 400.0, -1300.0],
        color: [255, 241, 193],
        intensity: 40,
    }
}

/// Project file from which the browser takes the object type of a model (shadow files).
pub const BROWSER_PROJECT: &str = "data/projects/nml-wiese.prj";
/// Tiles per side of the ground plane (of 128 units each).
const GROUND_TILES: u32 = 64;
/// The model stands in the middle of the ground plane.
pub const GROUND_CENTER: Vec3 = Vec3::new(4096.0, 0.0, 4096.0);
/// Shadow light of the browser: `[SHADOW]` of kamp1-1.
const BROWSER_SHADOW_LIGHT: [f32; 3] = [400.0, 1000.0, -500.0];
const BROWSER_SHADOW_INTENSITY: i32 = 70;

/// Flat ground plane `GROUND_TILES²` (terrain color (150, 150, 140)), receiver of the shadows.
pub fn browser_ground() -> Terrain {
    let n = GROUND_TILES as usize;
    let params = TerrainParams {
        dim_x: GROUND_TILES,
        dim_z: GROUND_TILES,
        light_position: [0.0, 1.0, 0.0],
        light_color: [150, 150, 140],
        light_intensity: 100,
        chunk_size: 16,
        ..TerrainParams::default()
    };
    let points = (0..=n)
        .flat_map(|z| {
            (0..=n).map(move |x| HlmPoint {
                position: [x as f32 * 128.0, 0.0, z as f32 * 128.0],
                normal: [0.0, 1.0, 0.0],
                color_guid: None,
            })
        })
        .collect();
    let heightmap = Heightmap {
        verts_x: n + 1,
        verts_z: n + 1,
        points,
    };
    let texture_map = TextureMap {
        dim_x: n,
        dim_z: n,
        guids: vec![0; n * n],
    };
    Terrain::new(&params, &heightmap, &texture_map, &Project::default())
        .expect("flat ground plane is valid")
}

/// Next shadow kind for F8: none -> kind 1 -> kind 2 -> kind 3 -> none.
pub fn next_shadow_kind(kind: ShadowKind) -> ShadowKind {
    match kind {
        ShadowKind::None => ShadowKind::Decal,
        ShadowKind::Decal => ShadowKind::Persistent,
        ShadowKind::Persistent => ShadowKind::PerFrame,
        ShadowKind::PerFrame => ShadowKind::None,
    }
}

/// Name of a shadow kind in the overlay.
pub fn shadow_kind_name(kind: ShadowKind) -> &'static str {
    match kind {
        ShadowKind::None => "none",
        ShadowKind::Decal => "Kind 1 (decal)",
        ShadowKind::Persistent => "Kind 2 (texture, persistent)",
        ShadowKind::PerFrame => "Kind 3 (texture per frame)",
    }
}

/// Shadow of a model in the browser: type `guid` (from the project file) at `quality`, or the kind chosen
/// with F8; a model without a type has no shadow unless a kind is chosen (texture 128^2).
pub fn browser_shadow(
    types: &ObjectTypes,
    project: &Project,
    guid: Option<u32>,
    quality: u8,
    chosen: Option<ShadowKind>,
) -> ObjectShadow {
    let parts = guid.map_or_else(Vec::new, |g| types.parts(g));
    match (chosen, parts.is_empty()) {
        (None, _) => object_shadow(types, project, &parts, quality, false),
        (Some(kind), false) => object_shadow_as(types, project, &parts, kind),
        (Some(kind), true) => ObjectShadow {
            kind,
            texture_size: 128,
            ..ObjectShadow::NONE
        },
    }
}

/// Shadow choice of the browser (CPU side): type of the model, stage, F8 choice and the shadow determined from
/// them. Determined anew at every change so that F8 sequences and the state snapshot do not depend on drawing
/// a frame.
struct ShadowChoice {
    /// Object type of the model (`None`: no type in the project file and `property.ini`).
    guid: Option<u32>,
    /// `shadow_quality` of the settings (kind without an F8 choice).
    quality: u8,
    /// Shadow kind chosen with F8.
    chosen: Option<ShadowKind>,
    /// Shadow of the model at `guid`, `quality` and `chosen`.
    shadow: ObjectShadow,
}

impl ShadowChoice {
    fn new(quality: u8) -> Self {
        Self {
            guid: None,
            quality,
            chosen: None,
            shadow: ObjectShadow::NONE,
        }
    }

    fn refresh(&mut self, types: &ObjectTypes, project: &Project) {
        self.shadow = browser_shadow(types, project, self.guid, self.quality, self.chosen);
    }

    /// New model with type `guid`: the F8 choice is dropped.
    fn select(&mut self, guid: Option<u32>, types: &ObjectTypes, project: &Project) {
        self.guid = guid;
        self.chosen = None;
        self.refresh(types, project);
    }

    /// F8: next kind, starting from the chosen kind (otherwise the kind currently in effect).
    fn step(&mut self, types: &ObjectTypes, project: &Project) {
        self.chosen = Some(next_shadow_kind(self.chosen.unwrap_or(self.shadow.kind)));
        self.refresh(types, project);
    }

    fn set_quality(&mut self, quality: u8, types: &ObjectTypes, project: &Project) {
        self.quality = quality;
        self.refresh(types, project);
    }
}

/// Does the model have a shadow that can be seen on the ground plane (texture or decal)?
/// Kind `Decal` without a decal (missing shadow file, ship with scaffold) draws nothing.
fn has_shadow(shadow: &ObjectShadow) -> bool {
    shadow.kind.is_texture() || !shadow.decals.is_empty()
}

/// All models (`.rdo`, `.rdw`) of the archives, sorted.
pub fn model_paths(vfs: &Vfs) -> Vec<String> {
    let mut paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".rdo") || p.ends_with(".rdw"))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Index of the model `model` in `paths` (comparison by `normalize_path`, so ignoring case and separators);
/// an unknown path is an error.
pub fn find_model(paths: &[String], model: &str) -> Result<usize> {
    let wanted = normalize_path(model);
    match paths.iter().position(|p| *p == wanted) {
        Some(index) => Ok(index),
        None => bail!(
            "model {model} ({wanted}) is not present in the archives ({} models)",
            paths.len()
        ),
    }
}

/// `index` advanced by `delta`, cyclic in `0..len` (`len` > 0).
pub fn step_index(index: usize, len: usize, delta: isize) -> usize {
    (index as isize + delta).rem_euclid(len as isize) as usize
}

/// Next/previous animation for `count` animations; after the last comes the bind pose (`None`), then the first
/// again.
fn next_animation(current: Option<usize>, count: usize, delta: isize) -> Option<usize> {
    let slots = count + 1; // + bind pose
    let slot = current.map_or(count, |i| i.min(count));
    let next = step_index(slot, slots, delta);
    (next < count).then_some(next)
}

/// Switches the animation of `player` (with sub-objects) at time `now_ms`: while running with a 200 ms
/// crossfade, when paused hard (without) - with time standing still the crossfade would stay at 0 and keep
/// showing the old animation while the overlay already names the new one. `false`: switch rejected during the
/// 200 ms lock.
fn switch_animation(player: &mut Playback, next: Option<usize>, now_ms: f32, paused: bool) -> bool {
    if paused {
        player.play_hard(next, now_ms);
        true
    } else {
        player.play(next, now_ms)
    }
}

/// Up/down request in the browser: target relative to the last requested animation (`pending`) or to the
/// running one. If the 200 ms lock rejects the switch, the target is remembered in `pending` (only the last
/// one) and caught up by [`apply_pending`].
// Deviation: The game has no browser; the lock applies as in the original, but a rejected key press is not
// lost.
fn request_animation(
    player: &mut Playback,
    pending: &mut Option<Option<usize>>,
    count: usize,
    delta: isize,
    now_ms: f32,
    paused: bool,
) {
    let base = pending.unwrap_or(player.player().current);
    let next = next_animation(base, count, delta);
    *pending = (!switch_animation(player, next, now_ms, paused)).then_some(next);
}

/// Executes a remembered request as soon as the lock has expired (when paused immediately, hard).
fn apply_pending(
    player: &mut Playback,
    pending: &mut Option<Option<usize>>,
    now_ms: f32,
    paused: bool,
) {
    if let Some(next) = *pending
        && switch_animation(player, next, now_ms, paused)
    {
        *pending = None;
    }
}

/// Run time of the browser on the virtual clock (`clock.rs`): start time on the clock and the own pause of the
/// space bar.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserTime {
    /// Clock time at which the browser time was 0.
    start_ms: f32,
    /// Paused (space bar, `--screenshot`): browser time at the pause.
    paused_at: Option<f32>,
}

impl BrowserTime {
    /// Browser time 0 at clock time `clock_ms`.
    pub fn new(clock_ms: f32) -> Self {
        Self {
            start_ms: clock_ms,
            paused_at: None,
        }
    }

    /// Browser time at clock time `clock_ms` (paused: time of the pause).
    pub fn now_ms(&self, clock_ms: f32) -> f32 {
        self.paused_at.unwrap_or(clock_ms - self.start_ms)
    }

    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Space bar: pause or continue from the same point.
    pub fn toggle_pause(&mut self, clock_ms: f32) {
        self.paused_at = match self.paused_at {
            Some(t) => {
                self.start_ms = clock_ms - t;
                None
            }
            None => Some(self.now_ms(clock_ms)),
        };
    }

    /// Pause at browser time `ms`.
    pub fn pause_at(&mut self, ms: f32) {
        self.paused_at = Some(ms);
    }
}

/// Camera orbiting a point (initially viewed from the front: figures look toward -z).
#[derive(Debug, Clone, PartialEq)]
pub struct Orbit {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Orbit {
    /// Frames a bounding sphere.
    pub fn frame(center: Vec3, radius: f32) -> Self {
        Self {
            target: center,
            distance: (radius * 3.0).max(200.0),
            yaw: PI,
            pitch: 0.35,
        }
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(sy * cp, sp, cy * cp) * self.distance
    }

    pub fn view(&self) -> Mat4 {
        look_at_lh(self.eye(), self.target, Vec3::Y)
    }

    /// Projection like the game (`n = d/16`), field of view 0.8 rad.
    // Deviation: The game has no model browser; 0.8 rad instead of `Fov` 0.1 from camerasetup.ini shows a
    // single model from close up without a strong telephoto effect.
    pub fn projection(&self, aspect_factor: f32) -> Mat4 {
        game_projection(0.8, aspect_factor, self.distance)
    }

    /// Mouse movement in pixels with the left button pressed.
    pub fn drag(&mut self, delta: Vec2) {
        self.yaw += delta.x * 0.01;
        self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.4, 1.4);
    }

    pub fn zoom(&mut self, closer: bool) {
        self.distance = (self.distance * if closer { 0.85 } else { 1.18 }).clamp(10.0, 1.0e6);
    }

    /// Values for the state snapshot (angles in degrees).
    pub fn state(&self) -> OrbitState {
        OrbitState {
            distance: self.distance,
            yaw_deg: self.yaw.to_degrees(),
            pitch_deg: self.pitch.to_degrees(),
        }
    }
}

/// Lines about a model and its running animation.
pub fn describe_model(
    asset: &ModelAsset,
    animation: Option<usize>,
    elapsed_ms: f32,
) -> Vec<String> {
    let rdo = &asset.rdo;
    let mut lines = vec![format!(
        "{} meshes, {} bones, {} materials, hull r = {:.0}",
        rdo.meshes.len(),
        rdo.bones.len(),
        rdo.materials.len(),
        rdo.bounds.radius
    )];
    let total = asset.config.animations.len();
    lines.push(
        match animation.and_then(|i| asset.animations.get(i).map(|a| (i, a))) {
            Some((i, Some(anim))) => {
                let file = asset.config.animations[i].file.as_deref().unwrap_or("-");
                let duration = anim.duration_ms();
                let time = if anim.looping && duration > 0.0 {
                    elapsed_ms % duration
                } else {
                    elapsed_ms.min(duration)
                };
                let labels: Vec<String> = anim
                    .labels
                    .iter()
                    .map(|l| format!("{}@{}", l.name, l.frame))
                    .collect();
                format!(
                    "Animation {}/{total}: {file} | {time:.0}/{duration:.0} ms{} | Time markers: {}",
                    i + 1,
                    if anim.looping { " | Loop" } else { "" },
                    if labels.is_empty() {
                        "-".into()
                    } else {
                        labels.join(", ")
                    }
                )
            }
            Some((i, None)) => format!("Animation {}/{total}: not loadable", i + 1),
            None => format!("Bind pose ({total} animations)"),
        },
    );
    let subs: Vec<&str> = active_sub_objects(asset, animation)
        .iter()
        .map(|s| s.file.as_str())
        .collect();
    if !subs.is_empty() {
        lines.push(format!("Sub-objects: {}", subs.join(", ")));
    }
    lines
}

/// Running animation of `player` at browser time `now_ms` for the state snapshot; `None` = bind pose.
pub fn animation_state(
    asset: &ModelAsset,
    player: &AnimationPlayer,
    now_ms: f32,
) -> Option<AnimationState> {
    let index = player.current?;
    let anim = asset.animations.get(index).and_then(Option::as_ref);
    Some(AnimationState {
        index,
        file: asset
            .config
            .animations
            .get(index)
            .and_then(|a| a.file.clone()),
        elapsed_ms: player.elapsed_ms(now_ms),
        duration_ms: anim.map(|a| a.duration_ms()),
        looping: anim.is_some_and(|a| a.looping),
    })
}

/// The model of the browser as the effects see it: in the middle of the ground plane, without rotation, always
/// visible; helper nodes from the running pose.
struct BrowserObject<'a> {
    entry: Option<&'a ModelEntry>,
    now_ms: f32,
}

impl ObjectView for BrowserObject<'_> {
    fn frame(&self, _object: usize) -> ObjectFrame {
        ObjectFrame {
            position: GROUND_CENTER,
            rotation: Vec3::ZERO,
            visible: true,
        }
    }

    fn node(&self, _object: usize, name: &str) -> Option<Vec3> {
        let entry = self.entry?;
        entry.playback.helper_node(&entry.asset, self.now_ms, name)
    }
}

/// State of the model browser.
pub struct BrowserView {
    pub paths: Vec<String>,
    pub index: usize,
    pub renderer: SceneRenderer,
    pub text: OverlayText,
    pub orbit: Orbit,
    models: ModelCache,
    asset: Option<Arc<ModelAsset>>,
    /// `AspectRatio` from camerasetup.ini (aspect ratio "stretched" as in the map view).
    stretch_ratio: f32,
    time: BrowserTime,
    /// Animation switch rejected during the 200 ms lock (target), caught up later.
    pending: Option<Option<usize>>,
    /// Error message of the most recently chosen model.
    pub status: Option<String>,
    /// Object types and project file for the shadows.
    types: ObjectTypes,
    project: Project,
    /// Ground plane (receiver of the shadows).
    ground: Terrain,
    /// Shadow choice (up to date at once, also before the next `update`).
    pick: ShadowChoice,
    /// Shadow (and ground plane) have to go to the GPU at the next `update`.
    shadow_dirty: bool,
    /// Effects of the model.
    pub effects: ModelEffects,
    /// Chosen sound action (T/Shift+T, index in the actions of the type) and the counter of the command
    /// responses (Enter); both start at 0 for every model.
    pub sound_choice: usize,
    pub responses: i32,
    effect_list: Vec<DummyEffect>,
    /// Fraction of a millisecond of the effect time.
    effect_carry: f64,
    effects_below: EffectFrame,
    effects_above: EffectFrame,
}

impl BrowserView {
    /// Creates the browser at clock time `clock_ms` (browser time 0) and chooses `start` (unknown path: error)
    /// or the first model; shadows at `shadow_quality`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        vfs: &Vfs,
        start: Option<&str>,
        clock_ms: f32,
        shadow_quality: u8,
        screen_width: u32,
    ) -> Result<Self> {
        let paths = model_paths(vfs);
        let index = match start {
            Some(model) => find_model(&paths, model)?,
            None => 0,
        };
        let generator = nml_client::ui_layer::atlas_generator(vfs);
        let text = OverlayText::new(device, format, vfs, screen_width, generator.as_deref())?;
        let light = browser_light();
        let project = match vfs.read(BROWSER_PROJECT) {
            Ok(bytes) => parse_project(&decode_cp1252(&bytes)).unwrap_or_else(|e| {
                log::warn!("{BROWSER_PROJECT}: {e}");
                Project::default()
            }),
            Err(e) => {
                log::warn!("{BROWSER_PROJECT} not readable, models without shadows: {e}");
                Project::default()
            }
        };
        let mut view = Self {
            renderer: SceneRenderer::models_only(device, queue, format, Some(&light), BACKGROUND),
            text,
            orbit: Orbit::frame(Vec3::ZERO, 100.0),
            paths,
            index,
            models: ModelCache::new(),
            asset: None,
            stretch_ratio: CameraSetup::load(vfs).aspect_ratio,
            time: BrowserTime::new(clock_ms),
            pending: None,
            status: None,
            types: ObjectTypes::load(vfs),
            project,
            ground: browser_ground(),
            pick: ShadowChoice::new(shadow_quality),
            shadow_dirty: true,
            effects: ModelEffects::new(),
            sound_choice: 0,
            responses: 0,
            effect_list: load_effect_list(vfs),
            effect_carry: 0.0,
            effects_below: EffectFrame::default(),
            effects_above: EffectFrame::default(),
        };
        view.select(vfs, index, clock_ms);
        Ok(view)
    }

    /// Browser time in milliseconds at clock time `clock_ms` (paused: time of the pause).
    pub fn now_ms(&self, clock_ms: f32) -> f32 {
        self.time.now_ms(clock_ms)
    }

    /// Chooses model `index`, starts its idle animation and frames it.
    pub fn select(&mut self, vfs: &Vfs, index: usize, clock_ms: f32) {
        if self.paths.is_empty() {
            self.status = Some("no models in the archives".into());
            return;
        }
        self.index = index % self.paths.len();
        let path = self.paths[self.index].clone();
        let now = self.now_ms(clock_ms);
        self.pending = None;
        self.sound_choice = 0;
        self.responses = 0;
        let guid = type_for_model(&self.types, &self.project, &path);
        self.pick.select(guid, &self.types, &self.project);
        self.shadow_dirty = true;
        let entries = self.renderer.entries_mut();
        entries.clear();
        let models = &mut self.models;
        match models.get(vfs, &path) {
            Some(asset) => {
                let (center, radius) =
                    (Vec3::from(asset.rdo.bounds.center), asset.rdo.bounds.radius);
                entries.push(ModelEntry {
                    playback: Playback::new(
                        &asset,
                        |file| models.get(vfs, file),
                        idle_animation(&asset),
                        now,
                    ),
                    asset: asset.clone(),
                    world: Affine3A::from_translation(GROUND_CENTER),
                    object: None,
                    look: nml_render::EntryLook::default(),
                    scaffold: None,
                    ship: false,
                    status: 0,
                    resource: false,
                    hidden_meshes: Arc::from([]),
                });
                self.orbit = Orbit::frame(center + GROUND_CENTER, radius);
                let class = self.pick.guid.and_then(|g| self.types.class(g));
                let playback = &entries[0].playback;
                self.effects.load(
                    vfs,
                    models,
                    &self.effect_list,
                    &asset,
                    playback,
                    class.as_ref(),
                );
                self.asset = Some(asset);
                self.status = None;
            }
            None => {
                // Otherwise the effects of the previous model would stay (drawn and reported).
                self.effects.unload();
                self.asset = None;
                self.status = Some(format!("{path}: not loadable (see log)"));
            }
        }
    }

    /// Pages `delta` models forward (cyclic).
    pub fn step_model(&mut self, vfs: &Vfs, delta: isize, clock_ms: f32) {
        if !self.paths.is_empty() {
            self.select(
                vfs,
                step_index(self.index, self.paths.len(), delta),
                clock_ms,
            );
        }
    }

    /// Next/previous animation (200 ms crossfade, when paused none); after the last comes the bind pose. During
    /// the lock the switch is remembered and caught up.
    pub fn step_animation(&mut self, delta: isize, clock_ms: f32) {
        let now = self.now_ms(clock_ms);
        let paused = self.time.paused();
        let Some(entry) = self.renderer.entries_mut().first_mut() else {
            return;
        };
        let count = entry.asset.config.animations.len();
        request_animation(
            &mut entry.playback,
            &mut self.pending,
            count,
            delta,
            now,
            paused,
        );
    }

    /// Pauses and sets the run time to `ms`; the running (or a still remembered) animation starts at 0 (without
    /// crossfade) - for screenshots at a fixed point in time.
    pub fn set_time(&mut self, ms: f32) {
        self.time.pause_at(ms);
        let pending = self.pending.take();
        if let Some(entry) = self.renderer.entries_mut().first_mut() {
            let current = pending.unwrap_or(entry.playback.player().current);
            entry.playback.play_hard(current, 0.0);
        }
    }

    /// Space bar: pause or continue from the same point.
    pub fn toggle_pause(&mut self, clock_ms: f32) {
        self.time.toggle_pause(clock_ms);
    }

    /// Catches up an animation switch remembered during the lock (logic of one frame).
    pub fn apply_pending(&mut self, clock_ms: f32) {
        let now = self.now_ms(clock_ms);
        let paused = self.time.paused();
        if self.pending.is_some()
            && let Some(entry) = self.renderer.entries_mut().first_mut()
        {
            apply_pending(&mut entry.playback, &mut self.pending, now, paused);
        }
    }

    /// Object type of the model (project file) and whether it is a unit (base module walk).
    pub fn sound_type(&self) -> Option<(u32, bool)> {
        let guid = self.pick.guid?;
        let unit = self
            .types
            .class(guid)
            .and_then(|c| c.base)
            .is_some_and(|b| b == nml_world::classes::BaseModule::Walk);
        Some((guid, unit))
    }

    /// F8: next shadow kind (none -> 1 -> 2 -> 3 -> none).
    pub fn step_shadow(&mut self) {
        self.pick.step(&self.types, &self.project);
        self.shadow_dirty = true;
    }

    /// V: next preview switch.
    pub fn step_preview(&mut self, vfs: &Vfs) {
        if let (Some(asset), Some(entry)) = (&self.asset, self.renderer.entries().first()) {
            let next = self.effects.preview().next();
            self.effects
                .set_preview(vfs, &mut self.models, asset, &entry.playback, next);
        }
    }

    /// Control command `spawn_effect` in the model browser (default: middle of the ground plane).
    pub fn spawn_effect(
        &mut self,
        vfs: &Vfs,
        path: &str,
        position: Option<Vec3>,
        scale: f32,
    ) -> Result<(), String> {
        let placement = nml_effects::particles::Placement {
            position: position.unwrap_or(GROUND_CENTER),
            yaw: 0.0,
        };
        self.effects
            .world
            .spawn(
                vfs,
                &mut self.models,
                path,
                nml_effects::attached::UNLIMITED_MS,
                placement,
                None,
                scale,
            )
            .map_err(|e| format!("Effect {path} {e}"))
    }

    /// -/+: burn intensity by `steps` steps of 0.25 (0...3).
    pub fn step_damage(&mut self, steps: i32) {
        self.effects
            .set_damage(self.effects.damage() + steps as f32 * DAMAGE_STEP);
    }

    /// Advances the effects by `dt_ms` (logic step); stands still while the browser is paused (space bar).
    pub fn advance_effects(&mut self, vfs: &Vfs, dt_ms: f32, clock_ms: f32) {
        if !self.time.paused() {
            self.step_effects(vfs, dt_ms, self.now_ms(clock_ms));
        }
    }

    /// Effects by `dt_ms` with poses at browser time `now_ms` (game time in whole milliseconds, carrying over
    /// the fractions).
    pub fn step_effects(&mut self, vfs: &Vfs, dt_ms: f32, now_ms: f32) {
        if self.asset.is_none() {
            return;
        }
        let total = self.effect_carry + f64::from(dt_ms);
        let whole = total.floor();
        self.effect_carry = total - whole;
        let objects = BrowserObject {
            entry: self.renderer.entries().first(),
            now_ms,
        };
        let frustum = Frustum::from_view_proj(self.orbit.projection(1.0) * self.orbit.view());
        self.effects
            .step(whole as u32, &frustum, &objects, vfs, &mut self.models);
    }

    /// `shadow_quality` changed: applies to models without an F8 choice.
    pub fn set_quality(&mut self, quality: u8) {
        self.pick.set_quality(quality, &self.types, &self.project);
        self.shadow_dirty = true;
    }

    /// Pose of the model at clock time `clock_ms` (always animated, independent of F5); after a change of model,
    /// kind or stage it loads the shadows to the GPU (ground plane only with a shadow); the shadow choice itself
    /// is already fixed after the change.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vfs: &Vfs, clock_ms: f32) {
        let now = self.now_ms(clock_ms);
        self.renderer.update(device, queue, vfs, now, true);
        if self.shadow_dirty {
            self.shadow_dirty = false;
            let shadows = self
                .renderer
                .entries()
                .iter()
                .map(|_| EntryShadow {
                    setup: self.pick.shadow.clone(),
                    position: GROUND_CENTER,
                    yaw: 0.0,
                    moving: false,
                })
                .collect();
            let light = (
                shadow_light_dir(BROWSER_SHADOW_LIGHT),
                BROWSER_SHADOW_INTENSITY,
            );
            self.renderer.set_shadows(
                device,
                queue,
                vfs,
                &mut self.models,
                shadows,
                light,
                (128.0, 128.0),
            );
            let ground = has_shadow(&self.pick.shadow).then_some(&self.ground);
            self.renderer.set_shadow_ground(device, queue, ground);
        }
    }

    /// Camera values of a frame `size`: aspect ratio as in the map view, without fog.
    pub fn view_params(&self, size: (u32, u32), aspect: AspectMode, clock_ms: f32) -> ViewParams {
        let factor = aspect_factor(aspect, size.0, size.1, self.stretch_ratio);
        ViewParams {
            view: self.orbit.view(),
            projection: self.orbit.projection(factor),
            fog: None,
            clear: None,
            now_ms: self.now_ms(clock_ms),
        }
    }

    /// Lines of the overlay (without the FPS line).
    pub fn describe(&self, clock_ms: f32) -> Vec<String> {
        let mut lines = vec![format!(
            "Model {}/{}: {}",
            self.index + 1,
            self.paths.len(),
            self.paths.get(self.index).map_or("-", String::as_str)
        )];
        if let Some(status) = &self.status {
            lines.push(status.clone());
        }
        if let (Some(asset), Some(entry)) = (&self.asset, self.renderer.entries().first()) {
            lines.extend(describe_model(
                asset,
                entry.playback.player().current,
                entry.playback.player().elapsed_ms(self.now_ms(clock_ms)),
            ));
        }
        let guid = self
            .pick
            .guid
            .map_or("without type".to_string(), |g| format!("Type {g}"));
        lines.push(format!(
            "Shadow: {} | {guid} | F8: change kind",
            shadow_kind_name(self.pick.shadow.kind)
        ));
        let counts = self.effects.world.counts();
        lines.push(format!(
            "Effects: burn intensity {:.2} ({} hit zones) | Preview: {} | {} effects, {} particles",
            self.effects.damage(),
            self.effects.zone_count(),
            self.effects.preview().name(),
            counts.owned + counts.world,
            counts.particles
        ));
        if self.time.paused() {
            lines.push("paused".into());
        }
        lines
    }

    /// State snapshot of the browser at clock time `clock_ms`.
    pub fn state(&self, clock_ms: f32) -> BrowserState {
        let now = self.now_ms(clock_ms);
        let player = self.renderer.entries().first().map(|e| e.playback.player());
        BrowserState {
            index: self.index,
            count: self.paths.len(),
            path: self.paths.get(self.index).cloned().unwrap_or_default(),
            status: self.status.clone(),
            orbit: self.orbit.state(),
            animation: match (&self.asset, player) {
                (Some(asset), Some(player)) => animation_state(asset, player, now),
                _ => None,
            },
            paused: self.time.paused(),
            crossfade_active: player.is_some_and(|p| p.crossfade_active(now)),
            lock_remaining_ms: player.map_or(0.0, |p| p.lock_remaining_ms(now)),
            pending: self.pending.map(|animation| PendingSwitch { animation }),
            shadow_kind: self.pick.shadow.kind.number(),
            type_guid: self.pick.guid,
            shadows: Some(shadow_state(
                self.pick.quality,
                &self.renderer.shadow_stats(),
            )),
            damage: self.effects.damage(),
            preview: self.effects.preview().name().into(),
            hit_zones: self.effects.zone_count(),
            effects: Some(effect_state(
                BROWSER_EFFECT_QUALITY,
                &self.effects.world.counts(),
                self.renderer.effect_quads(),
                self.renderer.effect_models(),
            )),
            // The actions are known by the sound database; `App::snapshot` fills them in.
            sound_actions: Vec::new(),
            sound_action: None,
        }
    }

    /// Draws the model with its effects (without overlay).
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        params: &ViewParams,
    ) {
        let all = TileMask::all(self.ground.dim_x(), self.ground.dim_z());
        self.renderer
            .prepare_shadows(device, queue, &self.ground, &all, params);
        let objects = BrowserObject {
            entry: self.renderer.entries().first(),
            now_ms: params.now_ms,
        };
        self.effects.world.draw(
            params.view,
            &objects,
            &mut self.effects_below,
            &mut self.effects_above,
        );
        self.renderer.set_effects(
            device,
            queue,
            vfs,
            &mut self.models,
            &self.effects_below,
            &self.effects_above,
        );
        self.renderer
            .render(device, queue, encoder, target, size, params);
    }

    /// Draws the overlay over the existing image (after [`BrowserView::render_scene`]).
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

/// Overlay line of the sound actions in the model browser.
pub fn sound_line(guid: Option<u32>, actions: &[(i32, String)], choice: usize) -> String {
    let Some(guid) = guid else {
        return "Sound: without type".into();
    };
    match actions.get(choice) {
        Some((action, name)) => format!(
            "Sound: action {action} \"{name}\" ({}/{}) | T/Shift+T: action | Enter: play",
            choice + 1,
            actions.len()
        ),
        None => format!("Sound: type {guid} without actions in sounddatabase.dat"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_effects::browser::{MODEL_OBJECT, Preview};
    use nml_formats::object_cfg::{AnimationEntry, ObjectConfig, SubObject};
    use nml_formats::rdm::RdmLayout;
    use nml_formats::rdo::RdoLayout;
    use nml_formats::testutil::{sample_rdm, sample_rdo};
    use nml_world::anim::AnimationPlayer;

    /// Like [`asset`] with the same animation twice (index 0 and 1).
    fn two_animations() -> ModelAsset {
        let mut asset = asset();
        let entry = asset.config.animations[0].clone();
        asset.config.animations.push(entry);
        asset.animations.push(asset.animations[0].clone());
        asset
    }

    fn sub(file: &str, index: i32) -> SubObject {
        SubObject {
            file: file.into(),
            index,
            start: 0,
            looping: true,
        }
    }

    fn asset() -> ModelAsset {
        ModelAsset {
            path: "data/r3d/test/kiste.rdo".into(),
            rdo: sample_rdo(RdoLayout::Standard),
            config: ObjectConfig {
                model_file: None,
                // Tool of the object layer (`INDEX` -1): never shown.
                sub_objects: vec![sub("axt.rdo", -1)],
                animations: vec![AnimationEntry {
                    file: Some("laufen.rdm".into()),
                    guid: None,
                    max_bounding_box: None,
                    sub_objects: vec![sub("hut.rdo", 0)],
                }],
            },
            animations: vec![Some(Arc::new(sample_rdm(RdmLayout::Standard)))],
            material_textures: vec![None, None],
            bone_track_cache: Default::default(),
        }
    }

    #[test]
    fn shadow_kinds_cycle_and_models_without_type_cast_only_on_request() {
        assert_eq!(next_shadow_kind(ShadowKind::None), ShadowKind::Decal);
        assert_eq!(next_shadow_kind(ShadowKind::PerFrame), ShadowKind::None);
        let types = ObjectTypes::default();
        let project = Project::default();
        assert_eq!(
            browser_shadow(&types, &project, None, 2, None),
            ObjectShadow::NONE
        );
        let chosen = browser_shadow(&types, &project, None, 2, Some(ShadowKind::PerFrame));
        assert_eq!(
            (chosen.kind, chosen.texture_size),
            (ShadowKind::PerFrame, 128)
        );
        assert_eq!(
            shadow_kind_name(ShadowKind::Persistent),
            "Kind 2 (texture, persistent)"
        );
        // Ground plane: flat, terrain color (150, 150, 140), model in the middle.
        let ground = browser_ground();
        assert_eq!((ground.dim_x(), ground.dim_z()), (64, 64));
        assert_eq!(ground.vertex(32, 32).position, GROUND_CENTER);
        assert_eq!(ground.vertex(5, 7).lit_color, [150, 150, 140]);
    }

    /// House (1901, building: kind 2 at stage 2) with decal, infantryman (2006: kind 1 at stage 2).
    fn shadow_world() -> (ObjectTypes, Project) {
        let types = ObjectTypes {
            types: nml_formats::property::parse_property_ini(
                "GUID 1901
NAME Haus
VALUEID 100 6 1 16777216
PROPID 3 2
VALUEID 40003 6 1 0
                 GUID 2006
NAME Mann
VALUEID 100 6 1 33558528
PROPID 0
VALUEID 10003 6 1 2048
PROPID 2
",
            ),
            translations: Default::default(),
        };
        let project = parse_project(
            "[OBJECT]
{
OBJ_GUID 1901
OBJ_FILENAME \"h.rdo\"
OBJ_SHADOW_FILENAME \"data\\r3d\\h_shd.rdo\"
}
             [OBJECT]
{
OBJ_GUID 2006
OBJ_FILENAME \"i.rdo\"
OBJ_SHADOW_FILENAME \"data\\r3d\\schatten.rdo\"
}
",
        )
        .unwrap();
        (types, project)
    }

    #[test]
    fn f8_steps_and_model_changes_take_effect_without_a_frame() {
        let (types, project) = shadow_world();
        let mut pick = ShadowChoice::new(2);
        // House at stage 2: kind 2; the state snapshot reports the kind of the new model at once.
        pick.select(Some(1901), &types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::Persistent);
        // Two F8 in a row without a frame drawn in between: kind 3, then none.
        pick.step(&types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::PerFrame);
        pick.step(&types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::None);
        // New model (infantryman, kind 1): choice dropped, the kind is correct at once.
        pick.select(Some(2006), &types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::Decal);
        pick.step(&types, &project);
        pick.step(&types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::PerFrame);
        // Model without a type: no shadow without a choice; F8 continues counting from none -> kind 1.
        pick.select(None, &types, &project);
        assert_eq!(pick.shadow, ObjectShadow::NONE);
        pick.step(&types, &project);
        pick.step(&types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::Persistent);
        // Stage change applies at once to models without a choice, not to chosen kinds.
        pick.set_quality(3, &types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::Persistent);
        pick.select(Some(2006), &types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::PerFrame);
        pick.set_quality(0, &types, &project);
        assert_eq!(pick.shadow.kind, ShadowKind::None);
    }

    #[test]
    fn the_ground_shows_only_for_models_with_a_visible_shadow() {
        let (types, project) = shadow_world();
        let shadow =
            |guid, quality, chosen| browser_shadow(&types, &project, guid, quality, chosen);
        assert!(has_shadow(&shadow(Some(1901), 2, None)));
        // Kind 1 with decal.
        assert!(has_shadow(&shadow(Some(2006), 2, None)));
        // Kind 1 without decal (model without a type, chosen): nothing to see.
        let bare = shadow(None, 2, Some(ShadowKind::Decal));
        assert_eq!(bare.kind, ShadowKind::Decal);
        assert!(!has_shadow(&bare));
        assert!(!has_shadow(&ObjectShadow::NONE));
        assert!(has_shadow(&shadow(None, 2, Some(ShadowKind::PerFrame))));
    }

    #[test]
    fn the_model_object_is_always_visible_in_the_middle_of_the_ground() {
        let object = BrowserObject {
            entry: None,
            now_ms: 0.0,
        };
        let frame = object.frame(MODEL_OBJECT);
        assert_eq!((frame.position, frame.visible), (GROUND_CENTER, true));
        assert_eq!(object.node(MODEL_OBJECT, "feuer01"), None);
        assert_eq!(Preview::default(), Preview::Off);
    }

    #[test]
    fn sound_line_names_the_chosen_action() {
        let actions = vec![(12, "Execute".to_string()), (100, "Select".to_string())];
        assert_eq!(
            sound_line(Some(2215), &actions, 1),
            "Sound: action 100 \"Select\" (2/2) | T/Shift+T: action | Enter: play"
        );
        assert_eq!(
            sound_line(Some(9), &[], 0),
            "Sound: type 9 without actions in sounddatabase.dat"
        );
        assert_eq!(sound_line(None, &actions, 0), "Sound: without type");
    }

    #[test]
    fn describes_meshes_animation_time_and_labels() {
        let lines = describe_model(&asset(), Some(0), 1250.0);
        assert_eq!(lines[0], "3 meshes, 1 bones, 2 materials, hull r = 61");
        assert_eq!(
            lines[1],
            "Animation 1/1: laufen.rdm | 250/1000 ms | Loop | Time markers: EndLabel@30"
        );
        assert_eq!(lines[2], "Sub-objects: hut.rdo");
        // Bind pose: only the object layer without `INDEX` -1 - nothing here.
        assert_eq!(
            describe_model(&asset(), None, 0.0),
            [
                "3 meshes, 1 bones, 2 materials, hull r = 61",
                "Bind pose (1 animations)"
            ]
        );
    }

    #[test]
    fn animation_state_names_file_time_and_duration() {
        let model = asset();
        let player = AnimationPlayer::new(Some(0), 1000.0);
        assert_eq!(
            animation_state(&model, &player, 1250.0),
            Some(AnimationState {
                index: 0,
                file: Some("laufen.rdm".into()),
                elapsed_ms: 250.0,
                duration_ms: Some(1000.0),
                looping: true,
            })
        );
        // Bind pose.
        assert_eq!(
            animation_state(&model, &AnimationPlayer::new(None, 0.0), 10.0),
            None
        );
        // Animation not loadable: without duration.
        let mut broken = asset();
        broken.animations[0] = None;
        let state = animation_state(&broken, &player, 1250.0).unwrap();
        assert_eq!((state.duration_ms, state.looping), (None, false));
    }

    #[test]
    fn orbit_starts_in_front_and_clamps() {
        let mut orbit = Orbit::frame(Vec3::new(0.0, 50.0, 0.0), 60.0);
        assert!(orbit.eye().z < 0.0, "view from the front (-z)");
        assert_eq!(orbit.distance, 200.0);
        orbit.drag(Vec2::new(0.0, 1000.0));
        assert_eq!(orbit.pitch, 1.4);
        let state = orbit.state();
        assert_eq!(state.distance, 200.0);
        assert!(
            (state.yaw_deg - 180.0).abs() < 1e-4
                && (state.pitch_deg - 1.4_f32.to_degrees()).abs() < 1e-4
        );
        orbit.zoom(true);
        assert!((orbit.distance - 170.0).abs() < 1e-3);
        let target = orbit.view().transform_point3(orbit.target);
        assert!(target.x.abs() < 1e-3 && target.z > 0.0);
    }

    #[test]
    fn start_model_is_found_case_and_separator_insensitive() {
        let paths = vec![
            "data/r3d/a.rdo".to_string(),
            "data/r3d/wid/units/b.rdo".to_string(),
        ];
        assert_eq!(find_model(&paths, "data/r3d/a.rdo").unwrap(), 0);
        assert_eq!(find_model(&paths, r"Data\R3D\Wid\Units\B.RDO").unwrap(), 1);
        assert_eq!(
            find_model(&paths, "./data//r3d/wid/units/b.rdo").unwrap(),
            1
        );
        let error = find_model(&paths, "does/not/exist.rdo").unwrap_err();
        assert!(
            format!("{error}").contains("does/not/exist.rdo")
                && format!("{error}").contains("not present"),
            "{error}"
        );
    }

    #[test]
    fn model_index_wraps_at_both_ends() {
        assert_eq!(step_index(0, 5, -1), 4);
        assert_eq!(step_index(4, 5, 1), 0);
        assert_eq!(step_index(2, 5, 10), 2);
        assert_eq!(step_index(1, 5, -10), 1);
        assert_eq!(step_index(3, 5, 1), 4);
    }

    #[test]
    fn switching_while_paused_shows_the_new_animation_at_once() {
        let asset = two_animations();
        let y = |p: &Playback, now: f32| p.player().pose(&asset, now).bones[0].position.y;
        let start = |animation| Playback::new(&asset, |_| None, animation, 0.0);
        // Running: switch to animation 1 at 500 ms, at first still the old pose (y = 55).
        let mut running = start(Some(0));
        assert!(switch_animation(&mut running, Some(1), 500.0, false));
        assert!((y(&running, 500.0) - 55.0).abs() < 1e-3);
        assert!((y(&running, 700.0) - 52.0).abs() < 1e-3);
        // Paused at 500 ms: the new animation at 0 at once (y = 50), no frozen crossfade.
        let mut paused = start(Some(0));
        assert!(switch_animation(&mut paused, Some(1), 500.0, true));
        assert!((y(&paused, 500.0) - 50.0).abs() < 1e-3);
        assert_eq!(*paused.player(), AnimationPlayer::new(Some(1), 500.0));
        // Bind pose (stopping the animation) also jumps at once while running.
        assert!(switch_animation(&mut running, None, 800.0, false));
        assert!((y(&running, 800.0) - 50.0).abs() < 1e-3);
        assert_eq!(running.player().current, None);
    }

    #[test]
    fn rejected_requests_are_remembered_and_applied_after_the_lock() {
        // `request_animation` gets the number of animations as a parameter (`count` = 4) and only counts
        // indices further; the model itself (two animations) is never evaluated here, a nonexistent index
        // would appear as bind pose.
        let asset = two_animations();
        let start = |animation| Playback::new(&asset, |_| None, animation, 0.0);
        let mut player = start(Some(0));
        let mut pending = None;
        // Four animations; down: 0 -> 1 accepted, immediately two more down: only the last target is remembered
        // (relative to the remembered one): 1 -> 2 -> 3.
        request_animation(&mut player, &mut pending, 4, 1, 1000.0, false);
        assert_eq!((player.player().current, pending), (Some(1), None));
        request_animation(&mut player, &mut pending, 4, 1, 1050.0, false);
        assert_eq!((player.player().current, pending), (Some(1), Some(Some(2))));
        request_animation(&mut player, &mut pending, 4, 1, 1100.0, false);
        assert_eq!((player.player().current, pending), (Some(1), Some(Some(3))));
        // Still locked: nothing; executed afterwards.
        apply_pending(&mut player, &mut pending, 1150.0, false);
        assert_eq!((player.player().current, pending), (Some(1), Some(Some(3))));
        apply_pending(&mut player, &mut pending, 1200.0, false);
        assert_eq!((player.player().current, pending), (Some(3), None));
        // The bind pose (stopping) is not subject to the lock.
        request_animation(&mut player, &mut pending, 4, 1, 1250.0, false);
        assert_eq!((player.player().current, pending), (None, None));
        // When paused a remembered request is executed at once, hard.
        let mut player = start(Some(0));
        assert!(player.play(Some(1), 100.0));
        let mut pending = Some(Some(2));
        apply_pending(&mut player, &mut pending, 150.0, true);
        assert_eq!((player.player().current, pending), (Some(2), None));
    }

    #[test]
    fn browser_time_runs_on_the_clock_relative_to_its_start() {
        let mut time = BrowserTime::new(1000.0);
        assert_eq!(time.now_ms(1000.0), 0.0);
        assert_eq!(time.now_ms(1250.0), 250.0);
        // Space bar at clock time 1250: stands at 250 even if the clock keeps running.
        time.toggle_pause(1250.0);
        assert!(time.paused());
        assert_eq!(time.now_ms(5000.0), 250.0);
        // Continue at clock time 5000: at the same point.
        time.toggle_pause(5000.0);
        assert!(!time.paused());
        assert_eq!(time.now_ms(5000.0), 250.0);
        assert_eq!(time.now_ms(5100.0), 350.0);
        // `--screenshot`: fixed point in time independent of the clock.
        time.pause_at(1500.0);
        assert_eq!(time.now_ms(0.0), 1500.0);
    }

    #[test]
    fn bind_pose_follows_the_last_animation() {
        // Three animations: 0, 1, 2, then bind pose (None), then 0 again.
        assert_eq!(next_animation(Some(2), 3, 1), None);
        assert_eq!(next_animation(None, 3, 1), Some(0));
        assert_eq!(next_animation(Some(0), 3, -1), None);
        assert_eq!(next_animation(None, 3, -1), Some(2));
        assert_eq!(next_animation(Some(0), 3, 1), Some(1));
        // Without animations the bind pose remains.
        assert_eq!(next_animation(None, 0, 1), None);
    }
}
