//! Fire of damaged buildings on the map (re/combat-host 4.4): in the building tick the original calls
//! `Building_UpdateDamageState` (`0x00504B30`) and with it
//! `GameObject_UpdateDamageFires((50 - p) * 20)` (`p` = HP percent), `0x3C03` after
//! destruction `UpdateDamageFires(0)`. For each hit zone `i`, `GameObject_SetHitZoneFire`
//! (`0x0046E3D0`) sets a `feuer.pef` with the scale `s` (calculation and zone step:
//! `nml_effects::fire`) and plays sound 102 (`0x11`) for `s > 0`.
//!
//! Presentation only: the simulation knows no fire. The host derives the call from the state
//! of the simulation (HP fraction, finished/sleep/death flags, timer `0x3C03`), so that
//! a loaded savegame is also presented correctly, and ticks in the simulation's **object tick**
//! (each object group every 100 ms of game time, not per frame). The fires are fixed-anchored
//! game effects with a key (`EffectWorld::add_game_effect`): they survive the rebuild of the
//! object list, and buildings do not move, so the position `Pos + Zone * RotX * RotY` holds
//! permanently. Visibility (fog, C-116) and advancing are handled by the effect world.
//!
//! Vehicles too (re/ships 7): `PropertyWalk_Tick` calls the same function in the object tick for living objects with
//! the walk flag `0x20000` (`NOTHUMAN`: ships, boats) with
//! `d = (50 - HP%) * 20`, and so does the locomotive (module 33) (re/train-multipart 4.2, D-216). The
//! zones `tzoneNN` belong to the moving model: the fires follow its frame position in every frame
//! ([`BuildingFires::follow`]). A dead vehicle no longer calls anything (condition `!DIED`,
//! **hypothesis**, re/ships 7): its fires keep the state of the tick of death, follow the sinking
//! model and end with the object. The locomotive has no `tzone` and never burns; tender and wagons
//! never call.

use std::collections::BTreeMap;

use glam::Vec3;
use nml_effects::attached::{Anchor, UNLIMITED_MS};
use nml_effects::fire::{
    FIRE, FIRE_SOUND_ACTION, FIRE_SOUND_FLAGS, ZoneFire, zone_scales, zone_step,
};
use nml_effects::particles::Placement;
use nml_effects::world::EffectWorld;
use nml_rda::Vfs;
use nml_sim::id::ObjectId;
use nml_sim::module::ModuleKind;
use nml_sim::module::building::{INDEX, flags, msg};
use nml_sim::module::train;
use nml_sim::object::{GameObject, status};
use nml_world::assets::ModelCache;
use nml_world::math::{game_rotation_x, game_rotation_y};

use crate::game::notices::SoundRequest;
use crate::game::sim_host::SimHost;

/// Attribute 14 hit points.
const HIT_POINTS: u32 = 14;
/// More ticks since the last sync count as a jump (loading, restart): the sync starts
/// over instead of catching up on ticks.
const MAX_CATCH_UP: u64 = 16;

/// What the building tick does to the fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireCall {
    /// No call (unfinished, sleeping, without HP, dead before `0x3C03`, or still locked).
    None,
    /// `UpdateDamageFires(d)` from `Building_UpdateDamageState`.
    Level(i32),
    /// `UpdateDamageFires(0)` from `0x3C03` (destroyed) or `StartBuild` (a construction site again): extinguish all
    /// fires of the building.
    Clear,
}

/// The call that the tick of building `o` currently makes (`PropertyBuilding_Tick`,
/// re/buildings 4.2): a living, finished, awake building with HP calls
/// `UpdateDamageFires((50 - p) * 20)` with `p = fraction / 1000` (the thresholds for `d` are set by
/// `nml_effects::fire::zone_scales`). A destroyed one (`m_IsDead`) extinguishes the fires as soon as the
/// timer `0x3C03` has fired (100 ms after the destruction); before that it calls nothing anymore.
/// An unfinished building (a construction site again after `StartBuild`) extinguishes the fires.
/// While an animation lock is running, no module tick runs.
pub fn fire_call(o: &GameObject) -> FireCall {
    if o.anim_lock_ms != 0 {
        return FireCall::None;
    }
    let Some(module) = o.module(INDEX) else {
        return vehicle_fire_call(o);
    };
    let ModuleKind::Building(b) = &module.kind else {
        return FireCall::None;
    };
    let finished = module.base.has_flags(flags::CONSTRUCTED_FINISHED);
    if o.status & status::DIED != 0 {
        let pending = |code: u32| {
            module
                .base
                .timers_ws
                .iter()
                .chain(&module.base.events)
                .any(|e| e.msg.code == code)
        };
        return if b.is_dead && !pending(msg::CLEAR_FIRES) || !b.is_dead && !finished {
            FireCall::Clear
        } else {
            FireCall::None
        };
    }
    // `PropertyBuilding_StartBuild` (`0x00504840`) calls `UpdateDamageFires(0)`: a building that is
    // a construction site again (also the sleeping one under a tower, `0x1F03`) extinguishes its fires.
    if !finished {
        return FireCall::Clear;
    }
    if !o.attributes.is_present(HIT_POINTS) || b.is_sleeping {
        return FireCall::None;
    }
    let p = o.attributes.current_fraction(HIT_POINTS) / 1000;
    FireCall::Level((50 - p) * 20)
}

/// Walk flag `NOTHUMAN` (`0x20000`): water vehicles and other large vehicles.
const WALK_NOTHUMAN: u32 = 0x2_0000;

/// The call that the object tick of a vehicle makes (`PropertyWalk_Tick` for `0x20000`,
/// `PropertyTrain_Tick` of the locomotive): a living vehicle with HP calls
/// `UpdateDamageFires((50 - p) * 20)`; a dead one nothing anymore (hypothesis, re/ships 7). Tender and
/// wagons (slaves) never call (re/train-multipart 4.2).
fn vehicle_fire_call(o: &GameObject) -> FireCall {
    let walking = o
        .module(0)
        .is_some_and(|m| matches!(m.kind, ModuleKind::Walk(_)) && m.base.has_flags(WALK_NOTHUMAN));
    let loco = o
        .module(train::INDEX)
        .is_some_and(|m| matches!(&m.kind, ModuleKind::Train(t) if !t.is_slave));
    if !(walking || loco) || o.status & status::DIED != 0 || !o.attributes.is_present(HIT_POINTS) {
        return FireCall::None;
    }
    let p = o.attributes.current_fraction(HIT_POINTS) / 1000;
    FireCall::Level((50 - p) * 20)
}

/// Position of a zone fire: `Pos + RotY * (RotX * Zone)`, yaw of the object
/// (`AttachedEffect::placement` with `Anchor::Object { with_rotation: true }`).
fn zone_placement(position: Vec3, rotation: Vec3, zone: Vec3) -> Placement {
    Placement {
        position: position + game_rotation_y(rotation.y) * (game_rotation_x(rotation.x) * zone),
        yaw: rotation.y,
    }
}

/// Fires of the buildings of a map.
#[derive(Debug, Default)]
pub struct BuildingFires {
    /// Tick of the simulation up to which the object ticks have been processed.
    steps: u64,
    /// Key of the zone fire per building (ID) and zone.
    fires: BTreeMap<u32, Vec<Option<u32>>>,
    /// Sounds 102 of the ticks since the last [`BuildingFires::take_sounds`].
    sounds: Vec<SoundRequest>,
}

impl BuildingFires {
    /// Remove all fires and restart the sync at tick `steps` (loading, restart).
    pub fn reset(&mut self, effects: &mut EffectWorld, steps: u64) {
        for key in std::mem::take(&mut self.fires)
            .into_values()
            .flatten()
            .flatten()
        {
            effects.remove_game_effect(key);
        }
        self.sounds.clear();
        self.steps = steps;
    }

    /// The sounds of the ticks since the last call.
    pub fn take_sounds(&mut self) -> Vec<SoundRequest> {
        std::mem::take(&mut self.sounds)
    }

    /// Sync with the simulation after a frame: fires of buildings that no longer exist
    /// die out; then the object tick of each building of the object groups that have ticked since the last
    /// sync (at most once per 100 ms of game time). The state is the one at the end
    /// of the frame (with two ticks per frame it applies to both).
    pub fn sync(
        &mut self,
        sim: &SimHost,
        effects: &mut EffectWorld,
        models: &mut ModelCache,
        vfs: &Vfs,
    ) {
        let now = sim.state.steps;
        if now < self.steps || now - self.steps > MAX_CATCH_UP {
            self.steps = now;
        }
        self.stop_vanished(sim, effects);
        // After a tick with phase 1, group 0 ticked, with phase 0 group 1 (`group_progress_ms`);
        // before that the groups alternate.
        let last = usize::from(sim.state.phase != 1);
        let taken = now - self.steps;
        for back in (0..taken).rev() {
            let group = last ^ (back as usize & 1);
            for &id in &sim.state.tick_groups[group] {
                if let Some(o) = sim.state.object(id, false) {
                    self.tick_object(sim, o, effects, models, vfs);
                }
            }
        }
        self.steps = now;
        self.follow(sim, effects);
    }

    /// The fires of the vehicles follow their frame position (Zone * RotX * RotY of the rendering), every
    /// frame and not only in the object tick: the fire belongs to the moving model
    /// (`AttachToObject`). Buildings do not move and stay where the tick put them.
    pub fn follow(&mut self, sim: &SimHost, effects: &mut EffectWorld) {
        for (&id, keys) in &self.fires {
            let Some(o) = sim.state.object(ObjectId(id), false) else {
                continue;
            };
            if o.module(INDEX).is_some() {
                continue;
            }
            let Some(zones) = sim.rules.hit_zones.get(&o.guid) else {
                continue;
            };
            let (position, rotation) = sim.display(o);
            for (key, zone) in keys.iter().zip(zones) {
                if let Some(key) = key {
                    effects.move_game_effect(
                        *key,
                        zone_placement(position, rotation, Vec3::from(*zone)),
                    );
                }
            }
        }
    }

    /// Fires of buildings without an object die out (`Stop`: the embers vanish, the smoke fades out).
    fn stop_vanished(&mut self, sim: &SimHost, effects: &mut EffectWorld) {
        self.fires.retain(|&id, keys| {
            if sim.state.object(ObjectId(id), false).is_some() {
                return true;
            }
            for key in keys.iter().flatten() {
                if let Some(a) = effects.game_effect_mut(*key) {
                    a.stop();
                }
            }
            false
        });
    }

    /// One object tick of building `o` (`GameObject_UpdateDamageFires`).
    fn tick_object(
        &mut self,
        sim: &SimHost,
        o: &GameObject,
        effects: &mut EffectWorld,
        models: &mut ModelCache,
        vfs: &Vfs,
    ) {
        let d = match fire_call(o) {
            FireCall::None => return,
            FireCall::Level(d) => d,
            FireCall::Clear => 0,
        };
        let zones = sim
            .rules
            .hit_zones
            .get(&o.guid)
            .map_or(&[][..], Vec::as_slice);
        let id = o.id.0;
        if zones.is_empty() || (d < 1 && !self.fires.contains_key(&id)) {
            return;
        }
        let (position, rotation) = sim.display(o);
        let keys = self.fires.entry(id).or_default();
        keys.resize(zones.len(), None);
        for (i, s) in zone_scales(d, zones.len()).into_iter().enumerate() {
            let placement = zone_placement(position, rotation, Vec3::from(zones[i]));
            // A fire that the effect world no longer knows (rebuild) counts as not there.
            let key = keys[i].filter(|&k| effects.game_effect(k).is_some());
            keys[i] = key;
            match (zone_step(key.is_some(), s), key) {
                (ZoneFire::Start, _) => {
                    keys[i] = effects.add_game_effect(
                        vfs,
                        models,
                        FIRE,
                        Anchor::Fixed(placement),
                        None,
                        1.0,
                        0,
                        UNLIMITED_MS,
                    );
                }
                (ZoneFire::Scale(s), Some(key)) => {
                    if let Some(a) = effects.game_effect_mut(key) {
                        a.effect.set_scale(s);
                    }
                    effects.move_game_effect(key, placement);
                }
                (ZoneFire::Stop, Some(key)) => {
                    if let Some(a) = effects.game_effect_mut(key) {
                        a.stop();
                    }
                    keys[i] = None;
                }
                _ => {}
            }
            // Sound 102 on every call with `s > 0` (every object tick and zone), without a fog check
            // like the combat sounds.
            if s > 0.0 {
                self.sounds.push(SoundRequest::Object3d {
                    action: FIRE_SOUND_ACTION,
                    flags: FIRE_SOUND_FLAGS,
                    position: placement.position,
                    guid: o.guid,
                    object: id,
                });
            }
        }
        if keys.iter().all(Option::is_none) {
            self.fires.remove(&id);
        }
    }

    /// Number of zones with fire per building (tests).
    #[cfg(test)]
    fn burning(&self, id: ObjectId) -> usize {
        self.fires
            .get(&id.0)
            .map_or(0, |k| k.iter().flatten().count())
    }

    /// Scales of the fires per zone of the building (tests).
    #[cfg(test)]
    fn scales(&self, id: ObjectId, effects: &EffectWorld) -> Vec<Option<f32>> {
        self.fires.get(&id.0).map_or_else(Vec::new, |keys| {
            keys.iter()
                .map(|k| {
                    k.and_then(|k| effects.game_effect(k))
                        .map(|a| a.effect.scale())
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::economy::testkit::{HOUSE, host, spawn};
    use nml_rda::testutil::{TestFile, build_rda};

    /// Archive with `feuer.pef` (one emitter in a loop).
    fn archive() -> (tempfile::TempDir, Vfs) {
        let pef = "EFFECT\r\n{\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
        let files = [TestFile {
            path: FIRE,
            data: pef.as_bytes(),
            compress: false,
        }];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    /// Four hit zones at a distance of 100 around the center.
    fn zones() -> Vec<[f32; 3]> {
        vec![
            [100.0, 0.0, 0.0],
            [-100.0, 0.0, 0.0],
            [0.0, 0.0, 100.0],
            [0.0, 0.0, -100.0],
        ]
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        vfs: Vfs,
        host: SimHost,
        house: ObjectId,
        fires: BuildingFires,
        effects: EffectWorld,
        models: ModelCache,
    }

    impl Fixture {
        fn new() -> Self {
            let (dir, vfs) = archive();
            let mut host = host();
            host.rules.hit_zones.insert(HOUSE, zones());
            let house = spawn(&mut host, HOUSE, 0, (20, 20));
            Self {
                _dir: dir,
                vfs,
                host,
                house,
                fires: BuildingFires::default(),
                effects: EffectWorld::new(),
                models: ModelCache::new(),
            }
        }

        /// HP fraction in percent.
        fn set_hp(&mut self, percent: i32) {
            let o = self.host.state.object_mut(self.house, false).unwrap();
            o.attributes
                .set_current_fraction(HIT_POINTS, percent * 1000);
        }

        /// One tick of the simulation including sync; returns the sounds of this tick.
        fn step(&mut self) -> Vec<SoundRequest> {
            self.host.step_once();
            self.fires
                .sync(&self.host, &mut self.effects, &mut self.models, &self.vfs);
            self.fires.take_sounds()
        }

        /// Two ticks = one object tick of each group.
        fn tick(&mut self) -> Vec<SoundRequest> {
            let mut sounds = self.step();
            sounds.extend(self.step());
            sounds
        }
    }

    fn is_fire_sound(s: &SoundRequest) -> bool {
        matches!(
            s,
            SoundRequest::Object3d {
                action: 102,
                flags: 0x11,
                ..
            }
        )
    }

    // QUIRK C-117: kept
    #[test]
    fn a_house_at_30_percent_burns_with_its_zones_and_one_at_60_does_not() {
        let mut f = Fixture::new();
        f.set_hp(60);
        let sounds = f.tick();
        assert!(sounds.is_empty());
        assert_eq!(f.fires.burning(f.house), 0);
        assert_eq!(f.effects.counts().world, 0);
        // 30 %: d = 400, n = 4, step 200 -> 400, 200, 0, -200 -> s = 3, 2, 0, 0.
        f.set_hp(30);
        let sounds = f.tick();
        assert_eq!(f.fires.burning(f.house), 2);
        assert_eq!(f.effects.counts().world, 2);
        // New fires with scale 1, `s` applies only in the next tick.
        assert_eq!(
            f.fires.scales(f.house, &f.effects),
            [Some(1.0), Some(1.0), None, None]
        );
        // Sound 102 per zone with `s > 0`, also at the start.
        assert_eq!(sounds.len(), 2);
        assert!(sounds.iter().all(is_fire_sound));
        let sounds = f.tick();
        assert_eq!(
            f.fires.scales(f.house, &f.effects),
            [Some(3.0), Some(2.0), None, None]
        );
        assert_eq!(sounds.len(), 2);
    }

    #[test]
    fn the_fire_follows_the_object_tick_and_not_the_frame() {
        let mut f = Fixture::new();
        f.set_hp(30);
        // The building belongs to one of the two object groups: of two ticks it ticks in one.
        let batches: Vec<usize> = (0..6).map(|_| f.step().len()).collect();
        assert_eq!(
            batches.iter().sum::<usize>(),
            6,
            "3 object ticks * 2 zones: {batches:?}"
        );
        assert!(batches.iter().all(|&n| n == 0 || n == 2), "{batches:?}");
        // Without a new tick no further sound.
        f.fires.sync(&f.host, &mut f.effects, &mut f.models, &f.vfs);
        assert!(f.fires.take_sounds().is_empty());
    }

    #[test]
    fn a_fire_goes_out_when_the_hit_points_recover() {
        let mut f = Fixture::new();
        f.set_hp(30);
        f.tick();
        f.tick();
        let keys: Vec<u32> = f.fires.fires[&f.house.0]
            .iter()
            .flatten()
            .copied()
            .collect();
        // Repaired above 50 %: d < 1 -> `Stop` per zone, no sound.
        f.set_hp(70);
        let sounds = f.tick();
        assert!(sounds.is_empty());
        assert_eq!(f.fires.burning(f.house), 0);
        for key in keys {
            assert!(
                f.effects
                    .game_effect(key)
                    .is_some_and(|a| !a.effect.is_active()),
                "the effect is stopped, the smoke fades out"
            );
        }
    }

    #[test]
    fn a_destroyed_house_puts_its_fires_out_with_the_timer_and_a_vanished_one_stops_them() {
        let mut f = Fixture::new();
        f.set_hp(30);
        f.tick();
        let keys: Vec<u32> = f.fires.fires[&f.house.0]
            .iter()
            .flatten()
            .copied()
            .collect();
        assert_eq!(keys.len(), 2);
        {
            let rules = f.host.rules.clone();
            let mut sim = nml_sim::sim::Sim::new(&mut f.host.state, &rules);
            nml_sim::module::building::destroy::destroy(&mut sim, f.house);
        }
        // Destroyed: no call anymore (no sound in any tick, the house ticks only every second one),
        // the fires stay until `0x3C03` (100 ms).
        let mut sounds = f.step();
        assert_eq!(f.fires.burning(f.house), 2);
        for _ in 0..6 {
            sounds.extend(f.step());
        }
        assert!(sounds.is_empty(), "{sounds:?}");
        assert_eq!(f.fires.burning(f.house), 0);
        for key in keys {
            assert!(
                f.effects
                    .game_effect(key)
                    .is_some_and(|a| !a.effect.is_active())
            );
        }
        // If the object with burning zones vanishes, the fires die out as well.
        let mut g = Fixture::new();
        g.set_hp(30);
        g.tick();
        let key = g.fires.fires[&g.house.0][0].unwrap();
        {
            let rules = g.host.rules.clone();
            let mut sim = nml_sim::sim::Sim::new(&mut g.host.state, &rules);
            nml_sim::world::queue_destroy(&mut sim, g.house);
        }
        for _ in 0..4 {
            g.step();
        }
        assert!(g.host.state.object(g.house, false).is_none());
        assert_eq!(g.fires.burning(g.house), 0);
        assert!(
            g.effects
                .game_effect(key)
                .is_some_and(|a| !a.effect.is_active())
        );
    }

    #[test]
    fn unfinished_and_unzoned_buildings_never_burn_and_reset_removes_all() {
        use crate::game::economy::testkit::site;
        let mut f = Fixture::new();
        let site = site(&mut f.host, HOUSE, 0, (30, 30));
        f.host
            .state
            .object_mut(site, false)
            .unwrap()
            .attributes
            .set_current_fraction(HIT_POINTS, 10_000);
        f.set_hp(100);
        f.tick();
        assert_eq!(f.fires.burning(site), 0);
        assert_eq!(f.fires.burning(f.house), 0);
        // Without hit zones in the model nothing burns.
        f.host.rules.hit_zones.clear();
        f.set_hp(10);
        f.tick();
        assert_eq!(f.fires.burning(f.house), 0);
        f.host.rules.hit_zones.insert(HOUSE, zones());
        f.tick();
        assert_eq!(f.fires.burning(f.house), 4);
        f.fires.reset(&mut f.effects, f.host.state.steps);
        assert_eq!(f.fires.burning(f.house), 0);
        assert_eq!(f.effects.counts().world, 0);
    }

    #[test]
    fn a_building_that_becomes_a_site_again_puts_its_fires_out() {
        let mut f = Fixture::new();
        f.set_hp(30);
        f.tick();
        let keys: Vec<u32> = f.fires.fires[&f.house.0]
            .iter()
            .flatten()
            .copied()
            .collect();
        assert_eq!(keys.len(), 2);
        // `StartBuild` (e.g. via `0x1F03` under a tower): a construction site again.
        {
            let rules = f.host.rules.clone();
            let mut sim = nml_sim::sim::Sim::new(&mut f.host.state, &rules);
            nml_sim::module::building::start_build(&mut sim, f.house);
        }
        let sounds = f.tick();
        assert!(sounds.is_empty());
        assert_eq!(f.fires.burning(f.house), 0);
        for key in keys {
            assert!(
                f.effects
                    .game_effect(key)
                    .is_some_and(|a| !a.effect.is_active())
            );
        }
        // The same for the sleeping building (`0x1F03`: DIED without `m_IsDead`).
        let mut g = Fixture::new();
        g.set_hp(30);
        g.tick();
        assert_eq!(g.fires.burning(g.house), 2);
        {
            let rules = g.host.rules.clone();
            let mut sim = nml_sim::sim::Sim::new(&mut g.host.state, &rules);
            nml_sim::messaging::send(
                &mut sim,
                g.house,
                &nml_sim::message::Message::new(nml_sim::module::building::msg::SET_SLEEPING, &[]),
            );
        }
        g.tick();
        assert_eq!(g.fires.burning(g.house), 0);
    }

    #[test]
    fn a_rebuilt_effect_world_restarts_the_fires_and_the_simulation_stays_untouched() {
        let mut f = Fixture::new();
        f.set_hp(30);
        f.tick();
        let before = nml_sim::save::checksum(&f.host.state);
        f.fires.sync(&f.host, &mut f.effects, &mut f.models, &f.vfs);
        assert_eq!(nml_sim::save::checksum(&f.host.state), before);
        // The world loses its effects (e.g. `clear` on a complete rebuild): no
        // crash, the fire rebuilds itself in the next tick.
        for key in f.fires.fires[&f.house.0]
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>()
        {
            f.effects.remove_game_effect(key);
        }
        f.tick();
        assert_eq!(f.fires.burning(f.house), 2);
        assert_eq!(f.effects.counts().world, 2);
    }

    #[test]
    fn zone_fires_sit_at_the_rotated_zone_positions() {
        let p = zone_placement(
            Vec3::new(1000.0, 0.0, 2000.0),
            Vec3::new(0.0, std::f32::consts::FRAC_PI_2, 0.0),
            Vec3::new(0.0, 10.0, 5.0),
        );
        // (0, 10, 5) * RotY(90 degrees) = (-5, 10, 0), as in `AttachedEffect::placement`.
        assert!((p.position - Vec3::new(995.0, 10.0, 2000.0)).length() < 1e-3);
        assert_eq!(p.yaw, std::f32::consts::FRAC_PI_2);
    }

    /// Ship, locomotive and tender of the test world of train and ships, the ship
    /// with the hit zones `tzone01..04`.
    struct ShipFixture {
        _dir: tempfile::TempDir,
        vfs: Vfs,
        host: SimHost,
        ship: ObjectId,
        loco: ObjectId,
        fires: BuildingFires,
        effects: EffectWorld,
        models: ModelCache,
    }

    impl ShipFixture {
        fn new() -> Self {
            use crate::game::sim_host::testkit::*;
            let (dir, vfs) = archive();
            let mut host = train_host();
            host.rules.hit_zones.insert(TRANSPORT, zones());
            let ship = spawn_dir(&mut host, TRANSPORT, 0, (20, 20), 0);
            let loco = spawn_dir(&mut host, LOCO, 0, (30, 30), 0);
            Self {
                _dir: dir,
                vfs,
                host,
                ship,
                loco,
                fires: BuildingFires::default(),
                effects: EffectWorld::new(),
                models: ModelCache::new(),
            }
        }

        fn set_hp(&mut self, id: ObjectId, percent: i32) {
            let o = self.host.state.object_mut(id, false).unwrap();
            o.attributes
                .set_current_fraction(HIT_POINTS, percent * 1000);
        }

        fn step(&mut self) -> Vec<SoundRequest> {
            self.host.step_once();
            self.fires
                .sync(&self.host, &mut self.effects, &mut self.models, &self.vfs);
            self.fires.take_sounds()
        }

        fn tick(&mut self) -> Vec<SoundRequest> {
            let mut sounds = self.step();
            sounds.extend(self.step());
            sounds
        }

        /// Position of the object's fires per zone (`None` = no fire).
        fn placements(&self, id: ObjectId) -> Vec<Option<nml_effects::particles::Placement>> {
            self.fires.fires.get(&id.0).map_or_else(Vec::new, |keys| {
                keys.iter()
                    .map(|k| {
                        k.and_then(|k| self.effects.game_effect(k))
                            .and_then(|a| match &a.anchor {
                                Anchor::Fixed(p) => Some(*p),
                                _ => None,
                            })
                    })
                    .collect()
            })
        }
    }

    #[test]
    fn burning_ships_carry_their_fires() {
        let mut f = ShipFixture::new();
        // Undamaged: no fire. 30 %: as with the house d = 400 -> scale 3, 2 at the first zones.
        f.set_hp(f.ship, 80);
        assert!(f.tick().is_empty());
        assert_eq!(f.fires.burning(f.ship), 0);
        f.set_hp(f.ship, 30);
        let sounds = f.tick();
        assert_eq!(f.fires.burning(f.ship), 2);
        assert_eq!(f.effects.counts().world, 2);
        assert_eq!(sounds.len(), 2);
        assert!(sounds.iter().all(is_fire_sound));
        f.tick();
        let scales = |f: &ShipFixture| -> Vec<Option<f32>> {
            f.fires.fires[&f.ship.0]
                .iter()
                .map(|k| {
                    k.and_then(|k| f.effects.game_effect(k))
                        .map(|a| a.effect.scale())
                })
                .collect()
        };
        assert_eq!(scales(&f), [Some(3.0), Some(2.0), None, None]);
        // The fires sit at `Pos + Zone * RotX * RotY` of the frame position and follow the ship, also
        // between the object ticks (every frame).
        let at = |f: &ShipFixture| {
            let o = f.host.state.object(f.ship, false).unwrap();
            f.host.display(o)
        };
        let (pos, rot) = at(&f);
        let expected = |pos: Vec3, rot: Vec3| -> Vec<Option<nml_effects::particles::Placement>> {
            zones()
                .iter()
                .enumerate()
                .map(|(i, z)| (i < 2).then(|| zone_placement(pos, rot, Vec3::from(*z))))
                .collect()
        };
        assert_eq!(f.placements(f.ship), expected(pos, rot));
        {
            let o = f.host.state.object_mut(f.ship, false).unwrap();
            o.world_pos[0] += 300.0;
            o.rotation[1] += 0.5;
        }
        f.fires.follow(&f.host, &mut f.effects);
        let (moved, turned) = at(&f);
        assert_ne!((moved, turned), (pos, rot));
        assert_eq!(f.placements(f.ship), expected(moved, turned));
        // Presentation only: the checksum stays.
        let sum = nml_sim::save::checksum(&f.host.state);
        f.fires.follow(&f.host, &mut f.effects);
        assert_eq!(nml_sim::save::checksum(&f.host.state), sum);
        // Dead ship (`DIED`): no call anymore - neither sound nor new scale, even if the
        // hit points keep falling (hypothesis, re/ships 7).
        f.host
            .state
            .object_mut(f.ship, false)
            .unwrap()
            .set_status_flags(status::DIED, true);
        f.set_hp(f.ship, 5);
        let sounds = f.tick();
        assert!(sounds.is_empty(), "{sounds:?}");
        assert_eq!(scales(&f), [Some(3.0), Some(2.0), None, None]);
        // If the ship vanishes, the fires die out.
        let key = f.fires.fires[&f.ship.0][0].unwrap();
        f.host
            .state
            .object_mut(f.ship, false)
            .unwrap()
            .set_status_flags(status::DIED, false);
        {
            let rules = f.host.rules.clone();
            let mut sim = nml_sim::sim::Sim::new(&mut f.host.state, &rules);
            nml_sim::world::queue_destroy(&mut sim, f.ship);
        }
        for _ in 0..4 {
            f.step();
        }
        assert_eq!(f.fires.burning(f.ship), 0);
        assert!(
            f.effects
                .game_effect(key)
                .is_some_and(|a| !a.effect.is_active())
        );
    }

    // QUIRK D-216: kept
    #[test]
    fn trains_never_burn_without_hit_zones_and_slaves_never_call() {
        let mut f = ShipFixture::new();
        // The locomotive calls `UpdateDamageFires` but has no `tzone` (D-216): no fire.
        f.set_hp(f.loco, 10);
        assert!(f.tick().is_empty());
        assert_eq!(f.fires.burning(f.loco), 0);
        assert_eq!(f.effects.counts().world, 0);
        // Tender and wagons as slaves never call it, not even with zones in the model.
        let slaves = match &f
            .host
            .state
            .object(f.loco, false)
            .unwrap()
            .module(nml_sim::module::train::INDEX)
            .unwrap()
            .kind
        {
            ModuleKind::Train(t) => t.slaves.clone(),
            _ => panic!("not a train"),
        };
        f.host
            .rules
            .hit_zones
            .insert(crate::game::sim_host::testkit::TENDER, zones());
        for &s in &slaves {
            f.set_hp(s, 10);
        }
        assert!(f.tick().is_empty());
        for &s in &slaves {
            assert_eq!(f.fires.burning(s), 0);
        }
        assert_eq!(f.effects.counts().world, 0);
    }
}
