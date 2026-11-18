//! Effect browser (key N): every `.pef` at the origin over a flat ground plane, one-shot effects
//! restarted with the space bar; `.hef` as a radius preview around a carrier at the origin with an
//! adjustable radius (the carrier property `0x12` belongs to the simulation).

use glam::Vec3;
use opennml_control::EffectBrowserState;
use opennml_effects::attached::UNLIMITED_MS;
use opennml_effects::handle::HandleEffect;
use opennml_effects::output::EffectFrame;
use opennml_effects::particles::Placement;
use opennml_effects::world::{EffectWorld, ObjectFrame, ObjectView, SpawnError};
use opennml_formats::hef::parse_hef;
use opennml_formats::text::decode_cp1252;
use opennml_rda::{Vfs, normalize_path};
use opennml_render::{SceneRenderer, ViewParams};
use opennml_world::assets::ModelCache;
use opennml_world::camera::{AspectMode, CameraSetup, aspect_factor};
use opennml_world::shadows::TileMask;
use opennml_world::terrain::Terrain;
use opennml_world::visibility::Frustum;

use crate::browser::{BrowserTime, GROUND_CENTER, Orbit, browser_ground};
use crate::snapshot::{BROWSER_EFFECT_QUALITY, effect_state};
use opennml_client::game::overlay::{OverlayLine, OverlayText};

/// Background as in the model browser.
const BACKGROUND: [f32; 3] = [0.18, 0.2, 0.22];
/// Base radius of the `.hef` preview (4 tiles) and step size (1 tile).
pub const DEFAULT_RADIUS: f32 = 512.0;
pub const RADIUS_STEP: f32 = 128.0;
/// Object index of the carrier of the `.hef`.
const CARRIER: usize = 0;

/// All `.pef` and `.hef` of the archives, sorted.
pub fn effect_paths(vfs: &Vfs) -> Vec<String> {
    let mut paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".pef") || p.ends_with(".hef"))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Carrier at the origin of the ground plane, always visible, ground at height 0.
struct Carrier;

impl ObjectView for Carrier {
    fn frame(&self, _object: usize) -> ObjectFrame {
        ObjectFrame {
            position: GROUND_CENTER,
            rotation: Vec3::ZERO,
            visible: true,
        }
    }

    fn node(&self, _object: usize, _name: &str) -> Option<Vec3> {
        None
    }
}

/// State of the effect browser.
pub struct EffectBrowser {
    pub paths: Vec<String>,
    pub index: usize,
    pub renderer: SceneRenderer,
    pub text: OverlayText,
    pub orbit: Orbit,
    pub effects: EffectWorld,
    models: ModelCache,
    ground: Terrain,
    stretch_ratio: f32,
    /// Run time of the selected effect (since selection or restart).
    time: BrowserTime,
    /// Radius of the `.hef` preview.
    pub radius: f32,
    /// Error message (file missing, not readable).
    pub status: Option<String>,
    carry: f64,
    below: EffectFrame,
    above: EffectFrame,
}

impl EffectBrowser {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        vfs: &Vfs,
        clock_ms: f32,
        screen_width: u32,
    ) -> anyhow::Result<Self> {
        let generator = opennml_client::ui_layer::atlas_generator(vfs);
        let text = OverlayText::new(device, format, vfs, screen_width, generator.as_deref())?;
        let light = opennml_formats::wld::ObjectLight {
            position: [800.0, 400.0, -1300.0],
            color: [255, 241, 193],
            intensity: 40,
        };
        let mut renderer =
            SceneRenderer::models_only(device, queue, format, Some(&light), BACKGROUND);
        let ground = browser_ground();
        renderer.set_shadow_ground(device, queue, Some(&ground));
        let mut view = Self {
            paths: effect_paths(vfs),
            index: 0,
            renderer,
            text,
            orbit: Orbit::frame(GROUND_CENTER + Vec3::new(0.0, 100.0, 0.0), 200.0),
            effects: EffectWorld::new(),
            models: ModelCache::new(),
            ground,
            stretch_ratio: CameraSetup::load(vfs).aspect_ratio,
            time: BrowserTime::new(clock_ms),
            radius: DEFAULT_RADIUS,
            status: None,
            carry: 0.0,
            below: EffectFrame::default(),
            above: EffectFrame::default(),
        };
        view.select(vfs, 0, clock_ms);
        Ok(view)
    }

    pub fn path(&self) -> &str {
        self.paths.get(self.index).map_or("-", String::as_str)
    }

    fn is_hef(&self) -> bool {
        self.path().ends_with(".hef")
    }

    /// Selects file `index`, restarts it and frames it.
    pub fn select(&mut self, vfs: &Vfs, index: usize, clock_ms: f32) {
        if self.paths.is_empty() {
            self.status = Some("no effects in the archives".into());
            return;
        }
        self.index = index % self.paths.len();
        self.restart(vfs, clock_ms);
        self.frame_camera();
    }

    /// Camera on the selected file: `.hef` with its radius, `.pef` above the origin. Only on file
    /// selection and radius change; the restart (space bar) leaves rotation and zoom alone.
    fn frame_camera(&mut self) {
        self.orbit = if self.is_hef() {
            Orbit::frame(GROUND_CENTER, self.radius + 200.0)
        } else {
            Orbit::frame(GROUND_CENTER + Vec3::new(0.0, 100.0, 0.0), 200.0)
        };
    }

    /// Pages by `delta` files (cyclic).
    pub fn step(&mut self, vfs: &Vfs, delta: isize, clock_ms: f32) {
        if !self.paths.is_empty() {
            let n = self.paths.len() as isize;
            let index = (self.index as isize + delta).rem_euclid(n) as usize;
            self.select(vfs, index, clock_ms);
        }
    }

    /// Space bar: all effects gone, the selected file new (effect time 0); the camera stays.
    pub fn restart(&mut self, vfs: &Vfs, clock_ms: f32) {
        self.effects.clear();
        self.time = BrowserTime::new(clock_ms);
        self.carry = 0.0;
        self.status = None;
        let path = self.path().to_string();
        if self.is_hef() {
            match vfs.read(&path) {
                Ok(bytes) => {
                    let records = parse_hef(&decode_cp1252(&bytes));
                    // The preview does not need the key: the next restart clears the whole
                    // effect world.
                    let _key = self.effects.add_handle(
                        CARRIER,
                        false,
                        HandleEffect::new(&path, records, self.radius),
                    );
                }
                Err(e) => self.status = Some(format!("{path}: {e}")),
            }
        } else {
            let placement = Placement {
                position: GROUND_CENTER,
                yaw: 0.0,
            };
            match self.effects.spawn_unflagged(
                vfs,
                &mut self.models,
                &path,
                UNLIMITED_MS,
                placement,
                None,
                1.0,
            ) {
                Ok(()) => {}
                Err(SpawnError::Unreadable) => {
                    self.status = Some(format!("{path}: not loadable (see log)"));
                }
                Err(e) => self.status = Some(format!("{path}: {e}")),
            }
        }
    }

    /// -/+: radius of the `.hef` preview by `steps` tiles (at least 0), restarted and
    /// re-framed.
    pub fn step_radius(&mut self, vfs: &Vfs, steps: i32, clock_ms: f32) {
        self.radius = (self.radius + steps as f32 * RADIUS_STEP).max(0.0);
        if self.is_hef() {
            self.restart(vfs, clock_ms);
            self.frame_camera();
        }
    }

    /// Control channel command `spawn_effect` (default: center of the ground plane); `clear_effects` removes
    /// such effects again, the selected file stays.
    pub fn spawn_effect(
        &mut self,
        vfs: &Vfs,
        path: &str,
        position: Option<Vec3>,
        scale: f32,
    ) -> Result<(), String> {
        let placement = Placement {
            position: position.unwrap_or(GROUND_CENTER),
            yaw: 0.0,
        };
        self.effects
            .spawn(
                vfs,
                &mut self.models,
                path,
                UNLIMITED_MS,
                placement,
                None,
                scale,
            )
            .map_err(|e| format!("Effect {path} {e}"))
    }

    /// State snapshot at clock `clock_ms`.
    pub fn state(&self, clock_ms: f32) -> EffectBrowserState {
        EffectBrowserState {
            index: self.index,
            count: self.paths.len(),
            path: self.path().to_string(),
            status: self.status.clone(),
            time_ms: self.time_ms(clock_ms),
            radius: self.radius,
            orbit: self.orbit.state(),
            effects: effect_state(
                BROWSER_EFFECT_QUALITY,
                &self.effects.counts(),
                self.renderer.effect_quads(),
                self.renderer.effect_models(),
            ),
        }
    }

    /// Run time of the selected effect at clock `clock_ms`.
    pub fn time_ms(&self, clock_ms: f32) -> f32 {
        self.time.now_ms(clock_ms)
    }

    /// Logic step of `dt_ms` (game time in whole milliseconds with carry).
    pub fn advance(&mut self, vfs: &Vfs, dt_ms: f32) {
        let total = self.carry + f64::from(dt_ms);
        let whole = total.floor();
        self.carry = total - whole;
        let frustum = Frustum::from_view_proj(self.orbit.projection(1.0) * self.orbit.view());
        self.effects
            .step(whole as u32, &frustum, &Carrier, vfs, &mut self.models);
    }

    pub fn view_params(&self, size: (u32, u32), aspect: AspectMode, clock_ms: f32) -> ViewParams {
        let factor = aspect_factor(aspect, size.0, size.1, self.stretch_ratio);
        ViewParams {
            view: self.orbit.view(),
            projection: self.orbit.projection(factor),
            fog: None,
            clear: None,
            now_ms: self.time_ms(clock_ms),
        }
    }

    /// Components of the selected `.pef` (particles, billboard, model).
    fn components(&mut self, vfs: &Vfs) -> Option<[usize; 3]> {
        let path = self.path().to_string();
        let pef = self.effects.library.template(vfs, &path)?;
        Some(pef.counts())
    }

    /// Lines of the overlay (without the FPS line).
    pub fn describe(&mut self, vfs: &Vfs, clock_ms: f32) -> Vec<String> {
        let mut lines = vec![format!(
            "Effect {}/{}: {}",
            self.index + 1,
            self.paths.len(),
            self.path()
        )];
        if let Some(status) = &self.status {
            lines.push(status.clone());
        }
        if self.is_hef() {
            lines.push(format!(
                "Radius preview: {:.0} ({} tiles) | -/+: radius",
                self.radius,
                self.radius / RADIUS_STEP
            ));
        } else if let Some([p, b, o]) = self.components(vfs) {
            lines.push(format!(
                "Components: {p} particles, {b} billboards, {o} models"
            ));
        }
        let counts = self.effects.counts();
        lines.push(format!(
            "Time {:.0} ms | {} effects, {} particles | {} quads, {} models",
            self.time_ms(clock_ms),
            counts.world,
            counts.particles,
            self.renderer.effect_quads(),
            self.renderer.effect_models()
        ));
        lines
    }

    /// Draws ground plane and effects (without overlay).
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
        self.effects
            .draw(params.view, &Carrier, &mut self.below, &mut self.above);
        self.renderer.set_effects(
            device,
            queue,
            vfs,
            &mut self.models,
            &self.below,
            &self.above,
        );
        self.renderer
            .render(device, queue, encoder, target, size, params);
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_rda::testutil::{TestFile, build_rda};

    #[test]
    fn lists_pef_and_hef_files_sorted() {
        let files = [
            TestFile {
                path: "data/effects/B.hef",
                data: b"",
                compress: false,
            },
            TestFile {
                path: "data/effects/a.pef",
                data: b"",
                compress: false,
            },
            TestFile {
                path: "data/effects/c.eff",
                data: b"",
                compress: false,
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert_eq!(
            effect_paths(&vfs),
            ["data/effects/a.pef", "data/effects/b.hef"]
        );
        let carrier = Carrier.frame(CARRIER);
        assert!(carrier.visible && carrier.position == GROUND_CENTER);
    }
}
