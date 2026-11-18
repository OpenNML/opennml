//! Effects of a single model (model browser): permanent effects and locomotive smoke as on the
//! map, plus, without simulation, the preview of the run-time triggers (re/effects):
//! building fire at the hit zones (`GameObject_UpdateDamageFires` `0x0046E590`) with a
//! burn intensity, wake and bow waves, wheel dust, brake sparks, whistle and standing ship.

use glam::Vec3;
use opennml_formats::hef::DummyEffect;
use opennml_rda::Vfs;
use opennml_world::anim::Playback;
use opennml_world::assets::{ModelAsset, ModelCache};
use opennml_world::classes::{TypeClass, module};
use opennml_world::visibility::Frustum;

use crate::attached::{Anchor, UNLIMITED_MS};
use crate::fire::{ZoneFire, zone_scales, zone_step};
use crate::particles::Placement;
use crate::triggers::{
    SHIP_FLAGS, TRAIN_SMOKE, dummy_effects, is_moving, numbered, numbered_nodes,
};
use crate::world::{EffectWorld, ObjectView, OwnedKind, ShipStand, WorldKind};

/// Object index of the model in the effect world of the browser.
pub const MODEL_OBJECT: usize = 0;
/// Largest burn intensity (scaling of the fires) and step size of the keys.
pub const MAX_DAMAGE: f32 = 3.0;
pub const DAMAGE_STEP: f32 = 0.25;
pub use crate::fire::FIRE;

/// Preview switch (key V).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Preview {
    #[default]
    Off,
    /// Moving on water: every 2 ticks `kielwasser`, `bugwellen_links/rechts` at
    /// `kielwasser%02d`, `lwake%02d`, `rwake%02d` (canoes with prefix `Kanu`).
    Wake,
    /// Moving on land: `rad_staubwolke` at `rad%02d`.
    Dust,
    /// Braking: `lok_funken` at `bremse%02d`.
    Sparks,
    /// Starting off: `lok_toeff` at `toeff01`/`toeff02`, repeated every second.
    Whistle,
}

impl Preview {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Wake,
            Self::Wake => Self::Dust,
            Self::Dust => Self::Sparks,
            Self::Sparks => Self::Whistle,
            Self::Whistle => Self::Off,
        }
    }

    /// Name in the overlay and in the state snapshot.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "aus",
            Self::Wake => "Kielwasser",
            Self::Dust => "Radstaub",
            Self::Sparks => "Bremsfunken",
            Self::Whistle => "Pfiff",
        }
    }
}

/// Hit zones `tzone%02d`, then `tzone %02d` with continuing numbering
/// (`GameObject_CollectHitZones` `0x0046E100`): static node positions.
pub fn hit_zones(model: &ModelAsset) -> Vec<Vec3> {
    let still = opennml_world::anim::AnimationPlayer::default();
    let mut zones = Vec::new();
    let mut i = 1;
    for pattern in ["tzone", "tzone "] {
        while let Some(p) = still.helper_node(model, 0.0, &format!("{pattern}{i:02}")) {
            zones.push(p);
            i += 1;
        }
    }
    zones
}

/// Scaling of the fires per zone for the burn intensity: damage value `d = round(b * 1000/3)`, then the
/// calculation of [`zone_scales`] (shared with the map).
pub fn fire_scales(damage: f32, zones: usize) -> Vec<f32> {
    zone_scales((damage * 1000.0 / 3.0).round() as i32, zones)
}

/// `sin` and `cos` of whole degrees `k` (0...180) like the tables of the original (`0x0062CE10` and
/// `0x0062D0E8`, 181 entries each): `4096 * sin(k deg)` truncated toward zero, divided by 4096.
fn table_sin_cos(k: u32) -> (f32, f32) {
    let r = f64::from(k).to_radians();
    let entry = |v: f64| (v * 4096.0) as i32 as f32 / 4096.0;
    (entry(r.sin()), entry(r.cos()))
}

/// World position of a wake node (`GameObject_GetHelperNodeMapPositionImpl` `0x0046D190`
/// -> `0x00479490`): object position plus the node `local` in object space, each component truncated
/// to whole units (`_ftol`), rotated only about Y with `a = -yaw` from the tables of whole
/// degrees (`FUN_00547C40`/`FUN_00547C70`: `a` brought into [-180 degrees, 180 degrees), index
/// `|a >> 12|` = absolute value of the rounded-down degree number, for the sine with the sign of
/// `a`). Without X/Z tilt, without scaling; the height is that of the object position plus node
/// height.
// Deviation: The original calculates in cell coordinates (fixed point 20.12, node * 4096 / cell size);
// this rounding (below cell size/4096 per term) does not apply here.
pub fn wake_node_world(position: Vec3, yaw: f32, local: Vec3) -> Vec3 {
    let (lx, ly, lz) = (local.x.trunc(), local.y.trunc(), local.z.trunc());
    // Yaw in fixed-point degrees (* 4096) like `[obj+0x158]` from the integer orientation.
    let full = 360 * 4096;
    let yaw_fixed = (f64::from(yaw).to_degrees() * 4096.0).round() as i64;
    let a = (-yaw_fixed + 180 * 4096).rem_euclid(full) - 180 * 4096;
    let degrees = a >> 12;
    let (sin, cos) = table_sin_cos(degrees.unsigned_abs() as u32);
    let sin = if a < 0 { -sin } else { sin };
    Vec3::new(
        position.x + lx * cos + lz * sin,
        position.y + ly,
        position.z - lx * sin + lz * cos,
    )
}

/// Effects of the model in the browser.
pub struct ModelEffects {
    pub world: EffectWorld,
    moving: bool,
    zones: Vec<Vec3>,
    fire_keys: Vec<Option<u32>>,
    damage: f32,
    preview: Preview,
    preview_keys: Vec<u32>,
    preview_ticks: u32,
    /// Walk flags `0x20000` + `0x2000` (ship) and `0x10000` (canoe).
    ship: bool,
    canoe: bool,
}

impl Default for ModelEffects {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelEffects {
    pub fn new() -> Self {
        Self {
            world: EffectWorld::new(),
            moving: false,
            zones: Vec::new(),
            fire_keys: Vec::new(),
            damage: 0.0,
            preview: Preview::Off,
            preview_keys: Vec::new(),
            preview_ticks: 0,
            ship: false,
            canoe: false,
        }
    }

    pub fn damage(&self) -> f32 {
        self.damage
    }

    pub fn preview(&self) -> Preview {
        self.preview
    }

    /// Number of hit zones.
    pub fn zone_count(&self) -> usize {
        self.zones.len()
    }

    /// New model with its playback `playback` (class `class`, if a type is known):
    /// all effects new, permanent effects always (regardless of `particles_quality`), locomotive smoke for
    /// trains, burn intensity and preview stay set. Nodes also in sub-objects.
    pub fn load(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        list: &[DummyEffect],
        model: &ModelAsset,
        playback: &Playback,
        class: Option<&TypeClass>,
    ) {
        self.world.clear();
        self.moving = class.is_some_and(is_moving);
        self.ship = class
            .is_some_and(|c| c.has_module(module::WALK) && c.walk_flags & SHIP_FLAGS == SHIP_FLAGS);
        self.canoe = self.ship && class.is_some_and(|c| c.walk_flags & 0x10000 != 0);
        self.zones = hit_zones(model);
        self.fire_keys = vec![None; self.zones.len()];
        self.preview_keys.clear();
        self.preview_ticks = 0;
        for (node, effect) in dummy_effects(list, model, playback) {
            self.attach(vfs, models, Anchor::Node(node), &effect, true);
        }
        if class.is_some_and(|c| c.has_module(module::TRAIN)) {
            for node in numbered_nodes(model, playback, "dampf") {
                self.attach(vfs, models, Anchor::Node(node), TRAIN_SMOKE, true);
            }
        }
        let preview = self.preview;
        self.preview = Preview::Off;
        self.set_preview(vfs, models, model, playback, preview);
    }

    /// Model not loadable: no effects, no hit zones (burn intensity and preview stay
    /// set).
    pub fn unload(&mut self) {
        self.world.clear();
        self.moving = false;
        self.ship = false;
        self.canoe = false;
        self.zones.clear();
        self.fire_keys.clear();
        self.preview_keys.clear();
        self.preview_ticks = 0;
    }

    fn attach(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        anchor: Anchor,
        effect: &str,
        run_invisible: bool,
    ) -> Option<u32> {
        let e = self
            .world
            .new_effect(vfs, models, effect, UNLIMITED_MS, anchor, run_invisible)?;
        Some(
            self.world
                .add_owned(MODEL_OBJECT, self.moving, OwnedKind::Attached(e), -1),
        )
    }

    fn stop_owned(&mut self, key: u32) {
        if let Some(OwnedKind::Attached(a)) = self
            .world
            .object_mut(MODEL_OBJECT, self.moving)
            .get_mut(key)
        {
            a.stop();
        }
    }

    /// Burn intensity 0...3 (rounded to steps of 0.25); takes effect in the next game tick.
    pub fn set_damage(&mut self, damage: f32) {
        self.damage = ((damage.clamp(0.0, MAX_DAMAGE) / DAMAGE_STEP).round()) * DAMAGE_STEP;
    }

    /// Switches the preview: the previous one ends (permanent effects stop), the new one begins.
    pub fn set_preview(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        model: &ModelAsset,
        playback: &Playback,
        preview: Preview,
    ) {
        for key in std::mem::take(&mut self.preview_keys) {
            self.stop_owned(key);
        }
        self.world.clear_ships();
        self.preview = preview;
        self.preview_ticks = 0;
        let prefixes: &[(&str, &str)] = match preview {
            Preview::Dust => &[("rad", "data/effects/rad_staubwolke.pef")],
            Preview::Sparks => &[("bremse", "data/effects/lok_funken.pef")],
            _ => &[],
        };
        for (prefix, effect) in prefixes {
            for node in numbered_nodes(model, playback, prefix) {
                if let Some(key) = self.attach(vfs, models, Anchor::Node(node), effect, true) {
                    self.preview_keys.push(key);
                }
            }
        }
        if preview == Preview::Off && self.ship {
            self.world
                .add_ship(ShipStand::new(MODEL_OBJECT, Vec3::ZERO, 0.0, true));
        }
    }

    /// One logic step; fire and preview in the game tick. `view` provides the nodes (running
    /// playback of the model with sub-objects).
    pub fn step(
        &mut self,
        dt_ms: u32,
        frustum: &Frustum,
        view: &dyn ObjectView,
        vfs: &Vfs,
        models: &mut ModelCache,
    ) {
        // The standing ship places its effects at the world position of the model.
        let frame = view.frame(MODEL_OBJECT);
        self.world
            .place_ships(MODEL_OBJECT, frame.position, frame.rotation.y);
        let ticks = self.world.step(dt_ms, frustum, view, vfs, models);
        for _ in 0..ticks {
            self.tick(view, vfs, models);
        }
    }

    fn tick(&mut self, view: &dyn ObjectView, vfs: &Vfs, models: &mut ModelCache) {
        // `GameObject_UpdateDamageFires`: new fires with scaling 1, in the next tick `s`.
        let scales = fire_scales(self.damage, self.zones.len());
        for (i, s) in scales.into_iter().enumerate() {
            match zone_step(self.fire_keys[i].is_some(), s) {
                ZoneFire::Idle => {}
                ZoneFire::Start => {
                    let anchor = Anchor::Object {
                        offset: self.zones[i],
                        with_rotation: true,
                    };
                    self.fire_keys[i] = self.attach(vfs, models, anchor, FIRE, true);
                }
                ZoneFire::Scale(s) => {
                    if let Some(key) = self.fire_keys[i]
                        && let Some(OwnedKind::Attached(a)) = self
                            .world
                            .object_mut(MODEL_OBJECT, self.moving)
                            .get_mut(key)
                    {
                        a.effect.set_scale(s);
                    }
                }
                ZoneFire::Stop => {
                    if let Some(key) = self.fire_keys[i].take() {
                        self.stop_owned(key);
                    }
                }
            }
        }
        self.preview_ticks += 1;
        let frame = view.frame(MODEL_OBJECT);
        let has = |name: &str| view.node(MODEL_OBJECT, name).is_some();
        match self.preview {
            Preview::Wake if self.preview_ticks % 2 == 1 => {
                let prefix = if self.canoe {
                    "data/effects/Kanu"
                } else {
                    "data/effects/"
                };
                for (node, file) in [
                    ("lwake", "bugwellen_links.pef"),
                    ("rwake", "bugwellen_rechts.pef"),
                    ("kielwasser", "kielwasser.pef"),
                ] {
                    for name in numbered(node, has).into_iter().take(9) {
                        let local = view.node(MODEL_OBJECT, &name).unwrap_or(Vec3::ZERO);
                        let position = wake_node_world(frame.position, frame.rotation.y, local);
                        let anchor = Anchor::Fixed(Placement {
                            position,
                            yaw: frame.rotation.y,
                        });
                        if let Some(e) = self.world.new_effect(
                            vfs,
                            models,
                            &format!("{prefix}{file}"),
                            UNLIMITED_MS,
                            anchor,
                            true,
                        ) {
                            self.world.add_world(WorldKind::Attached(e, None));
                        }
                    }
                }
            }
            Preview::Whistle if self.preview_ticks % 10 == 1 => {
                for node in ["toeff01", "toeff02"] {
                    let anchor = if has(node) {
                        Anchor::Node(node.into())
                    } else {
                        Anchor::Object {
                            offset: Vec3::ZERO,
                            with_rotation: true,
                        }
                    };
                    self.attach(vfs, models, anchor, "data/effects/lok_toeff.pef", true);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_scales_follow_the_damage_steps_of_the_original() {
        // n = 4: step 200; b = 0.3 -> d = 100 -> only zone 0 with s = 1.
        assert_eq!(fire_scales(0.3, 4), [1.0, 0.0, 0.0, 0.0]);
        // b = 3 -> d = 1000: all with 3 (zone 3: 1000 - 600 = 400 -> 3).
        assert_eq!(fire_scales(3.0, 4), [3.0; 4]);
        // b = 1.5 -> d = 500: 3, 3, 1, 0 (500 - 400 = 100).
        assert_eq!(fire_scales(1.5, 4), [3.0, 3.0, 1.0, 0.0]);
        assert_eq!(fire_scales(0.0, 3), [0.0; 3]);
        assert!(fire_scales(1.0, 0).is_empty());
    }

    /// Model with the helper nodes `names` and an archive with `feuer.pef` and
    /// `rad_staubwolke.pef` (one looping emitter each).
    fn setup(names: &[&str]) -> (tempfile::TempDir, Vfs, ModelAsset) {
        use opennml_formats::object_cfg::ObjectConfig;
        use opennml_formats::rdo::{MeshKind, RdoLayout};
        use opennml_formats::testutil::sample_rdo;
        let pef = "EFFECT\r\n{\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
        let files = [
            opennml_rda::testutil::TestFile {
                path: "data/effects/feuer.pef",
                data: pef.as_bytes(),
                compress: false,
            },
            opennml_rda::testutil::TestFile {
                path: "data/effects/rad_staubwolke.pef",
                data: pef.as_bytes(),
                compress: false,
            },
            opennml_rda::testutil::TestFile {
                path: "data/effects/schiff_stehend.pef",
                data: pef.as_bytes(),
                compress: false,
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("NML0.RDA"),
            opennml_rda::testutil::build_rda(&files),
        )
        .unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut rdo = sample_rdo(RdoLayout::Standard);
        let helper = rdo
            .meshes
            .iter()
            .find(|m| m.kind == MeshKind::Helper)
            .unwrap()
            .clone();
        for (i, name) in names.iter().enumerate() {
            let mut mesh = helper.clone();
            mesh.name = (*name).into();
            mesh.position = [i as f32 * 10.0, 50.0, 0.0];
            rdo.meshes.push(mesh);
        }
        let model = ModelAsset {
            path: "m.rdo".into(),
            rdo,
            config: ObjectConfig::default(),
            animations: vec![],
            material_textures: vec![],
            bone_track_cache: Default::default(),
        };
        (dir, vfs, model)
    }

    /// Playback without animation and without sub-objects.
    fn still(model: &ModelAsset) -> Playback {
        Playback::new(model, |_| None, None, 0.0)
    }

    /// The model at the origin, visible.
    struct Origin;

    impl ObjectView for Origin {
        fn frame(&self, _object: usize) -> crate::world::ObjectFrame {
            crate::world::ObjectFrame {
                position: Vec3::ZERO,
                rotation: Vec3::ZERO,
                visible: true,
            }
        }

        fn node(&self, _object: usize, _name: &str) -> Option<Vec3> {
            None
        }
    }

    /// The model at a fixed world position (as in the browser), visible.
    struct At(Vec3);

    impl ObjectView for At {
        fn frame(&self, _object: usize) -> crate::world::ObjectFrame {
            crate::world::ObjectFrame {
                position: self.0,
                rotation: Vec3::ZERO,
                visible: true,
            }
        }

        fn node(&self, _object: usize, _name: &str) -> Option<Vec3> {
            None
        }
    }

    fn frustum() -> Frustum {
        use opennml_world::camera::{game_projection, look_at_lh};
        let view = look_at_lh(Vec3::new(0.0, 500.0, -500.0), Vec3::ZERO, Vec3::Y);
        Frustum::from_view_proj(game_projection(0.8, 1.0, 700.0) * view)
    }

    #[test]
    fn fires_burn_at_the_hit_zones_with_the_damage_and_go_out() {
        let (_dir, vfs, model) = setup(&["tzone01", "tzone02", "tzone 03"]);
        assert_eq!(hit_zones(&model).len(), 3);
        let mut models = ModelCache::new();
        let mut effects = ModelEffects::new();
        effects.load(&vfs, &mut models, &[], &model, &still(&model), None);
        assert_eq!(effects.zone_count(), 3);
        effects.set_damage(3.0);
        let step = |effects: &mut ModelEffects, models: &mut ModelCache| {
            effects.step(100, &frustum(), &Origin, &vfs, models)
        };
        step(&mut effects, &mut models);
        assert_eq!(effects.world.counts().owned, 3);
        // Scaling 3 only in the next tick.
        let scales = |effects: &mut ModelEffects| -> Vec<f32> {
            effects.world.objects()[0]
                .keys()
                .into_iter()
                .filter_map(|k| match effects.world.object_mut(0, false).get_mut(k) {
                    Some(OwnedKind::Attached(a)) => Some(a.effect.scale()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(scales(&mut effects), [1.0; 3]);
        step(&mut effects, &mut models);
        assert_eq!(scales(&mut effects), [3.0; 3]);
        // Off: stopped, deleted after the particles have died out.
        effects.set_damage(0.0);
        for _ in 0..15 {
            step(&mut effects, &mut models);
        }
        assert_eq!(effects.world.counts().owned, 0);
    }

    #[test]
    fn dust_preview_runs_at_the_wheels_until_switched_off() {
        let (_dir, vfs, model) = setup(&["rad01", "rad02"]);
        let mut models = ModelCache::new();
        let mut effects = ModelEffects::new();
        effects.load(&vfs, &mut models, &[], &model, &still(&model), None);
        effects.set_preview(&vfs, &mut models, &model, &still(&model), Preview::Dust);
        assert_eq!(effects.world.counts().owned, 2);
        effects.set_preview(&vfs, &mut models, &model, &still(&model), Preview::Sparks);
        for _ in 0..15 {
            effects.step(100, &frustum(), &Origin, &vfs, &mut models);
        }
        // Without a `bremse` node no sparks; the dust is stopped and deleted.
        assert_eq!(effects.world.counts().owned, 0);
    }

    #[test]
    fn a_standing_ship_lays_its_effect_at_the_position_of_the_model() {
        let (_dir, vfs, model) = setup(&[]);
        let class = TypeClass {
            guid: 1,
            name: "schiff".into(),
            base: None,
            category: 0,
            modules: vec![module::WALK],
            walk_flags: SHIP_FLAGS,
            stand_flags: 0,
            resource_flags: 0,
        };
        let mut models = ModelCache::new();
        let mut effects = ModelEffects::new();
        effects.load(&vfs, &mut models, &[], &model, &still(&model), Some(&class));
        let at = Vec3::new(4096.0, 0.0, 4096.0);
        effects.step(100, &frustum(), &At(at), &vfs, &mut models);
        assert_eq!(effects.world.world_positions(), [at]);
    }

    #[test]
    fn unloading_removes_the_effects_of_the_previous_model() {
        let (_dir, vfs, model) = setup(&["tzone01", "rad01"]);
        let mut models = ModelCache::new();
        let mut effects = ModelEffects::new();
        effects.load(&vfs, &mut models, &[], &model, &still(&model), None);
        effects.set_preview(&vfs, &mut models, &model, &still(&model), Preview::Dust);
        effects.set_damage(3.0);
        effects.step(100, &frustum(), &Origin, &vfs, &mut models);
        assert_eq!(effects.world.counts().owned, 2);
        effects.unload();
        assert_eq!(effects.world.counts().owned, 0);
        assert_eq!(effects.zone_count(), 0);
        // Without a model, further steps create nothing; burn intensity and preview stay.
        effects.step(100, &frustum(), &Origin, &vfs, &mut models);
        assert_eq!(effects.world.counts().owned, 0);
        assert_eq!((effects.damage(), effects.preview()), (3.0, Preview::Dust));
    }

    #[test]
    fn wake_nodes_follow_the_integer_formula_of_the_original() {
        let position = Vec3::new(1000.0, 50.0, 2000.0);
        let local = Vec3::new(100.7, 10.9, -40.2);
        // Yaw 0: only the truncated node coordinates (100, 10, -40).
        assert_eq!(
            wake_node_world(position, 0.0, local),
            Vec3::new(1100.0, 60.0, 1960.0)
        );
        // Yaw 37.5 degrees: a = -37.5 degrees -> index 38 with negative sine, so the table values for 38 degrees
        // (sin 2521/4096, cos 3227/4096); x' = x*cos - z*sin, z' = x*sin + z*cos.
        let (s, c) = (2521.0 / 4096.0, 3227.0 / 4096.0);
        let got = wake_node_world(position, 37.5f32.to_radians(), local);
        let want = Vec3::new(
            1000.0 + 100.0 * c - (-40.0) * s,
            60.0,
            2000.0 + 100.0 * s + (-40.0) * c,
        );
        assert!((got - want).abs().max_element() < 1e-3, "{got} {want}");
        // Yaw -37.5 degrees: a = +37.5 degrees -> index 37 (rounded down), positive sine: rotation by -37 degrees.
        let (s, c) = (2465.0 / 4096.0, 3271.0 / 4096.0);
        let got = wake_node_world(position, (-37.5f32).to_radians(), local);
        let want = Vec3::new(
            1000.0 + 100.0 * c + (-40.0) * s,
            60.0,
            2000.0 - 100.0 * s + (-40.0) * c,
        );
        assert!((got - want).abs().max_element() < 1e-3, "{got} {want}");
        // Whole degrees: the same rotation as the world matrix without tilt (table ~ sin/cos).
        let yaw = 120f32.to_radians();
        let matrix =
            opennml_world::math::object_world_matrix(position, Vec3::new(0.0, yaw, 0.0), Vec3::ONE)
                .transform_point3(local.trunc());
        assert!(
            (wake_node_world(position, yaw, local) - matrix)
                .abs()
                .max_element()
                < 0.1
        );
    }

    #[test]
    fn sine_tables_truncate_like_the_original() {
        // Samples from `0x0062CE10`/`0x0062D0E8`; sin 30 degrees = 2047 and cos 60 degrees = 2048 show the
        // truncation of the floating-point values.
        let raw = |k| {
            let (s, c) = table_sin_cos(k);
            ((s * 4096.0) as i32, (c * 4096.0) as i32)
        };
        assert_eq!(raw(0), (0, 4096));
        assert_eq!(raw(1), (71, 4095));
        assert_eq!(raw(2), (142, 4093));
        assert_eq!(raw(30), (2047, 3547));
        assert_eq!(raw(60), (3547, 2048));
        assert_eq!(raw(90), (4096, 0));
        assert_eq!(raw(91), (4095, -71));
        assert_eq!(raw(180), (0, -4096));
    }

    #[test]
    fn previews_cycle_and_damage_rounds_to_quarters() {
        let mut p = Preview::Off;
        let names: Vec<&str> = (0..5)
            .map(|_| {
                p = p.next();
                p.name()
            })
            .collect();
        assert_eq!(
            names,
            ["Kielwasser", "Radstaub", "Bremsfunken", "Pfiff", "aus"]
        );
        let mut effects = ModelEffects::new();
        effects.set_damage(1.1);
        assert_eq!(effects.damage(), 1.0);
        effects.set_damage(9.0);
        assert_eq!(effects.damage(), 3.0);
        effects.set_damage(-1.0);
        assert_eq!(effects.damage(), 0.0);
    }
}
