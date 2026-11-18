//! Healing circle and healing effects (re/healing 7): pure rendering
//! from the state and the events of the simulation, after the pattern of
//! [`crate::game::train_effects::TrainSparks`].
//!
//! - **Healing circle** `healing_circle.hef` (`m_HEFFilename`, 300008) as a handle effect on the healer or
//!   healing building, only for objects of the local player (`GameObject_AddHandleEffect` `0x0046CC10`
//!   from `0x00519580` on becoming ready, `0x0051A29A` on the finished building, `0x0051956C` in
//!   `Init`): as long as `m_ReadyToHeal` and in the teardown animation after it, because only `0x41D`
//!   removes it (`GameObject_RemoveHandleEffect` `0x0046D0D0` in `0x00519B70`, re/healing 3.4/3.5).
//!   Radius `floor(E/4) * SIZE_X` with `E` = sight (attribute `0x12`) of the carrier (re/effects,
//!   section "HEF radius indicators").
//! - **Target effect** `heilen.pef` on the healed target (`AttachedEffect`, duration `0x7FFFFFFF`, scale
//!   `H * 0.0055`) per pair `(healer, target)` from [`SimEvent::Healed`]; it keeps running as long as the
//!   pair is healed in every object tick (double buffer of the area healing), and ends in the first
//!   object tick without healing.
//!
//! The simulation keeps no effect keys. The rebuild of the object list after creating,
//! destroying and change of owner leaves circles and target effects in place
//! ([`EffectWorld::remap_objects`]). If the effect world loses them in a complete rebuild
//! (change of `ParticlesQuality`, [`EffectWorld::clear`]), the next sync recreates them;
//! [`HealEffects::reset`] clears everything when loading and on restart.

use std::collections::BTreeMap;

use glam::Vec3;
use opennml_effects::attached::{Anchor, UNLIMITED_MS};
use opennml_effects::handle::HandleEffect;
use opennml_effects::world::EffectWorld;
use opennml_formats::hef::parse_hef;
use opennml_formats::text::decode_cp1252;
use opennml_rda::{Vfs, normalize_path};
use opennml_sim::id::ObjectId;
use opennml_sim::module::ModuleKind;
use opennml_sim::module::healing::INDEX;
use opennml_sim::object::{GameObject, status};
use opennml_sim::state::{SimEvent, SimState};
use opennml_world::assets::ModelCache;

use crate::game::sim_host::SimHost;

/// Effect on the healed target (`heilen.pef`).
pub const HEAL_EFFECT: &str = "data/effects/heilen.pef";
/// Scale of the target effect per healing point (`H * 0.0055`, re/healing 7).
pub const HEAL_EFFECT_SCALE: f32 = 0.0055;
/// Carrier property `0x12` of the HEF radius: the sight.
const SIGHT: u32 = 18;
/// An object tick is two simulation steps.
const STEPS_PER_TICK: u64 = 2;

/// Target effect of a pair `(healer, target)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TargetEffect {
    /// Key of the game effect (`None`: no carrier drawn yet or `failed`).
    key: Option<u32>,
    /// The effect file is not loadable: no new attempt (counts as done). If only the
    /// carrier is missing (not drawn yet), it stays `false` and the next frame tries again.
    failed: bool,
    /// Healing amount of the last [`SimEvent::Healed`] (scale of the effect on a later attempt).
    points: i32,
    /// Simulation step of the last [`SimEvent::Healed`] of the pair.
    step: u64,
}

/// Healing circles and target effects of a map.
///
/// Deviation: In the original `heilen.pef` hangs on the target as soon as the healer has found it,
/// even if it does not heal for lack of mana (`0x00519D67` ... `0x00519E2B`), and disappears only when it
/// gives up the target - the effect is **target-bound** there. The reimplementation derives it from
/// [`SimEvent::Healed`], which only comes on actual healing - here it is **heal-bound**. The difference is
/// visible only briefly, while the mana is bound (a healer with empty mana and a found target); a
/// separate event "target found" would dilute `Healed`.
#[derive(Debug, Default)]
pub struct HealEffects {
    /// Key of the healing circle per object (ID); `None`: file not loadable (no new
    /// attempt as long as the object is supposed to have a circle).
    circles: BTreeMap<u32, Option<u32>>,
    /// Target effects per `(healer, target)` (IDs).
    targets: BTreeMap<(u32, u32), TargetEffect>,
}

/// A circle that an object is to show: object, HEF path, sight of the carrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Circle {
    pub id: ObjectId,
    pub hef: String,
    pub sight: i32,
}

impl HealEffects {
    /// Remove all circles and target effects (loading, restart).
    pub fn reset(&mut self, effects: &mut EffectWorld) {
        for key in std::mem::take(&mut self.circles).into_values().flatten() {
            effects.remove_handle(key);
        }
        for t in std::mem::take(&mut self.targets).into_values() {
            if let Some(key) = t.key {
                effects.remove_game_effect(key);
            }
        }
    }

    /// The objects with a healing circle for the player `viewer`, in table order: own objects
    /// with module 29, a non-empty HEF name and ready (`m_ReadyToHeal`) or in the
    /// teardown animation (`HEAL` without transition bit `0x80_0000`: `0x21D` clears `ready`, only `0x41D`
    /// the circle).
    pub fn circled(state: &SimState, viewer: u8) -> Vec<Circle> {
        state
            .objects()
            .filter(|o| o.owner() == viewer)
            .filter_map(|o| {
                let ModuleKind::Healing(h) = &o.module(INDEX)?.kind else {
                    return None;
                };
                let tearing_down =
                    o.status & status::HEAL != 0 && o.status & status::ANIM_LOCKED == 0;
                (!h.hef.is_empty() && (h.ready || tearing_down)).then(|| Circle {
                    id: o.id,
                    hef: h.hef.clone(),
                    sight: sight(state, o),
                })
            })
            .collect()
    }

    /// Sync after a frame with the events `events` of its simulation steps: create or remove
    /// circles for the active player, create target effects for every new pair from
    /// [`SimEvent::Healed`] and end pairs without healing for one object tick.
    /// `carrier` names the index of the object in the object list of the map, `cell` the
    /// tile size (`SIZE_X`). Reads only the state.
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &mut self,
        sim: &SimHost,
        events: &[SimEvent],
        effects: &mut EffectWorld,
        models: &mut ModelCache,
        vfs: &Vfs,
        carrier: &dyn Fn(ObjectId) -> Option<usize>,
        cell: f32,
    ) {
        self.sync_circles(sim, effects, vfs, carrier, cell);
        self.sync_targets(sim, events, effects, models, vfs, carrier);
    }

    fn sync_circles(
        &mut self,
        sim: &SimHost,
        effects: &mut EffectWorld,
        vfs: &Vfs,
        carrier: &dyn Fn(ObjectId) -> Option<usize>,
        cell: f32,
    ) {
        let wanted = Self::circled(&sim.state, sim.active_player);
        // End of the circle (`0x41D`, change of owner or viewer, object gone).
        self.circles.retain(|&id, key| {
            if wanted.iter().any(|c| c.id.0 == id) {
                return true;
            }
            if let Some(key) = key {
                effects.remove_handle(*key);
            }
            false
        });
        for c in wanted {
            match self.circles.get(&c.id.0) {
                // File not loadable: no new attempt.
                Some(None) => continue,
                Some(Some(key)) if effects.handle(*key).is_some() => continue,
                _ => {}
            }
            // Not drawn yet: again in the next frame.
            let Some(at) = carrier(c.id) else {
                self.circles.remove(&c.id.0);
                continue;
            };
            let moving = sim
                .state
                .object(c.id, false)
                .is_some_and(|o| o.has_module(opennml_sim::module::walk::INDEX, true));
            let path = normalize_path(&c.hef);
            let key = match vfs.read(&path) {
                Ok(bytes) => {
                    let records = parse_hef(&decode_cp1252(&bytes));
                    // `floor(E/4) * SIZE_X` (re/effects, section "HEF radius indicators").
                    let radius = (c.sight / 4) as f32 * cell;
                    Some(effects.add_handle(at, moving, HandleEffect::new(&path, records, radius)))
                }
                Err(e) => {
                    log::warn!("Healing circle {path} not loadable: {e}");
                    None
                }
            };
            self.circles.insert(c.id.0, key);
        }
    }

    fn sync_targets(
        &mut self,
        sim: &SimHost,
        events: &[SimEvent],
        effects: &mut EffectWorld,
        models: &mut ModelCache,
        vfs: &Vfs,
        carrier: &dyn Fn(ObjectId) -> Option<usize>,
    ) {
        let now = sim.state.steps;
        let mut healed: BTreeMap<(u32, u32), (ObjectId, i32)> = BTreeMap::new();
        for e in events {
            if let SimEvent::Healed {
                healer,
                target,
                points,
            } = e
            {
                healed.insert((healer.0, target.0), (*target, *points));
            }
        }
        for (pair, (target, points)) in healed {
            if let Some(t) = self.targets.get_mut(&pair) {
                let alive = if t.failed {
                    true
                } else {
                    t.key.is_some_and(|k| effects.game_effect(k).is_some())
                };
                if alive {
                    t.step = now;
                    t.points = points;
                    continue;
                }
            }
            // New pair or lost in the rebuild.
            let (key, failed) = spawn_target(effects, models, vfs, carrier(target), points);
            self.targets.insert(
                pair,
                TargetEffect {
                    key,
                    failed,
                    step: now,
                    points,
                },
            );
        }
        // Carrier not yet drawn at the first `Healed`: try again in every frame,
        // as long as the pair does not expire.
        for (&(_, target), t) in self.targets.iter_mut() {
            if t.key.is_some() || t.failed {
                continue;
            }
            let (key, failed) =
                spawn_target(effects, models, vfs, carrier(ObjectId(target)), t.points);
            t.key = key;
            t.failed = failed;
        }
        // One object tick without healing of the pair: remove the effect (`KillOwnedEffect`; also target
        // or healer gone, given up, trance ended).
        self.targets.retain(|_, t| {
            if now.saturating_sub(t.step) < STEPS_PER_TICK {
                return true;
            }
            if let Some(key) = t.key {
                effects.remove_game_effect(key);
            }
            false
        });
    }
}

/// Creates the target effect `heilen.pef` on the carrier. Returns `(key, not loadable)`: without a carrier
/// `(None, false)` (try again later), with a non-loadable file `(None, true)`.
fn spawn_target(
    effects: &mut EffectWorld,
    models: &mut ModelCache,
    vfs: &Vfs,
    carrier: Option<usize>,
    points: i32,
) -> (Option<u32>, bool) {
    let Some(at) = carrier else {
        return (None, false);
    };
    let key = effects.add_game_effect(
        vfs,
        models,
        HEAL_EFFECT,
        Anchor::Object {
            offset: Vec3::ZERO,
            with_rotation: false,
        },
        Some(at),
        points as f32 * HEAL_EFFECT_SCALE,
        0,
        UNLIMITED_MS,
    );
    if key.is_none() {
        log::warn!("effect {HEAL_EFFECT} not loadable");
    }
    (key, key.is_none())
}

/// Sight of the object (`AttributeSet_GetCurrentValue(0x12)` with the upgrades of the owner).
pub(crate) fn sight(state: &SimState, o: &GameObject) -> i32 {
    let percent = opennml_sim::upgrades::type_upgrade_percent(state, o.owner(), o.guid, SIGHT);
    o.attributes.current_value(SIGHT, percent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testworld::{ARCHER, MEDIC, host, spawn};
    use opennml_rda::testutil::{TestFile, build_rda};
    use opennml_sim::message::Message;
    use opennml_sim::messaging::send;
    use opennml_sim::module::healing::msg;
    use opennml_sim::save::checksum;
    use opennml_sim::sim::Sim;

    /// Time mode, one effect per 100 ms around the carrier.
    const HEF: &str = "PARTICLEMOVE\r\nARCRANGE 0 360\r\nDURATION 1000\r\nTIMEOUT 100\r\n\
                       EFFECT data/effects/ring.pef\r\nEFFECTDURATION 500\r\n/PARTICLEMOVE\r\n";
    const PEF: &str = "EFFECT\r\n{\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
    const CIRCLE: &str = "data/effects/healing_circle.hef";

    /// Archive with the healing circle and `heilen.pef`.
    fn archive() -> (tempfile::TempDir, Vfs) {
        let files = [
            TestFile {
                path: CIRCLE,
                data: HEF.as_bytes(),
                compress: false,
            },
            TestFile {
                path: HEAL_EFFECT,
                data: PEF.as_bytes(),
                compress: false,
            },
            TestFile {
                path: "data/effects/ring.pef",
                data: PEF.as_bytes(),
                compress: false,
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        vfs: Vfs,
        host: SimHost,
        heal: HealEffects,
        effects: EffectWorld,
        models: ModelCache,
        /// Whether the carriers are drawn (`carrier` returns `Some`).
        carried: bool,
    }

    impl Fixture {
        fn new() -> Self {
            let (dir, vfs) = archive();
            Self {
                _dir: dir,
                vfs,
                host: host(),
                heal: HealEffects::default(),
                effects: EffectWorld::new(),
                models: ModelCache::new(),
                carried: true,
            }
        }

        /// One frame: sync with the events `events`, checksum unchanged.
        fn sync(&mut self, events: &[SimEvent]) {
            let sum = checksum(&self.host.state);
            let carried = self.carried;
            self.heal.sync(
                &self.host,
                events,
                &mut self.effects,
                &mut self.models,
                &self.vfs,
                &|id| carried.then_some(id.0 as usize),
                128.0,
            );
            assert_eq!(checksum(&self.host.state), sum, "rendering changes nothing");
        }

        /// One simulation step with sync (one step per frame); returns the events.
        fn step(&mut self) -> Vec<SimEvent> {
            let events = opennml_sim::tick::step(&mut self.host.state, &self.host.rules, &[]);
            self.sync(&events);
            events
        }

        fn say(&mut self, id: ObjectId, code: u32) -> i32 {
            let rules = self.host.rules.clone();
            let mut sim = Sim::new(&mut self.host.state, &rules);
            send(&mut sim, id, &Message::new(code, &[]))
        }

        fn ready(&self, id: ObjectId) -> bool {
            let o = self.host.state.object(id, false).unwrap();
            matches!(&o.module(INDEX).unwrap().kind, ModuleKind::Healing(h) if h.ready)
        }

        /// Send a healer with full mana into the trance and step by step until "ready".
        fn entrance(&mut self, id: ObjectId) {
            self.set_fraction(id, opennml_sim::module::spell::MANA, 100_000);
            assert_eq!(self.say(id, msg::BEGIN), -1);
            for _ in 0..200 {
                if self.ready(id) {
                    return;
                }
                self.step();
            }
            panic!("healer not ready");
        }

        fn set_fraction(&mut self, id: ObjectId, attribute: u32, f: i32) {
            self.host
                .state
                .object_mut(id, false)
                .unwrap()
                .attributes
                .set_current_fraction(attribute, f);
        }

        /// Key and radius of the living circles per object.
        fn circles(&self) -> Vec<(u32, f32)> {
            self.heal
                .circles
                .iter()
                .filter_map(|(&id, k)| Some((id, self.effects.handle((*k)?)?.radius)))
                .collect()
        }

        /// Pairs with a living target effect and its scale.
        fn targets(&self) -> Vec<((u32, u32), f32)> {
            self.heal
                .targets
                .iter()
                .filter_map(|(&pair, t)| {
                    Some((pair, self.effects.game_effect(t.key?)?.effect.scale()))
                })
                .collect()
        }
    }

    #[test]
    fn circles_follow_ready_own_healers() {
        let mut f = Fixture::new();
        let own = spawn(&mut f.host, MEDIC, 0, (20, 20));
        let foreign = spawn(&mut f.host, MEDIC, 1, (40, 40));
        f.sync(&[]);
        assert!(f.circles().is_empty(), "not ready: no circle");
        // Build-up animation (`HEAL` with transition bit): no circle yet.
        assert_eq!(f.say(own, msg::BEGIN), -1);
        assert_eq!(f.say(foreign, msg::BEGIN), -1);
        f.step();
        assert!(f.circles().is_empty());
        f.entrance(own);
        // The foreign one belongs to another tick group: ready one step later at most.
        if !f.ready(foreign) {
            f.step();
        }
        assert!(f.ready(foreign));
        // Ready: circle only on the own healer, radius floor(40/4) * 128.
        assert_eq!(f.circles(), [(own.0, 1280.0)]);
        let key = f.heal.circles[&own.0].unwrap();
        assert_eq!(f.effects.handle(key).unwrap().path, CIRCLE);
        // Every frame again: no second circle.
        f.sync(&[]);
        f.sync(&[]);
        assert_eq!(f.heal.circles[&own.0], Some(key));
        // Change of the viewer: the foreign one shows its circle, the own one no longer.
        f.host.active_player = 1;
        f.sync(&[]);
        assert_eq!(f.circles(), [(foreign.0, 1280.0)]);
        assert!(f.effects.handle(key).is_none(), "removed");
        f.host.active_player = 0;
        f.sync(&[]);
        assert_eq!(f.circles().len(), 1);
        // Complete rebuild of the effect world: it loses the circle, the sync creates it anew.
        f.effects.clear();
        f.sync(&[]);
        assert_eq!(f.circles(), [(own.0, 1280.0)]);
        // Rebuild of the object list (the healer stays in its place): the same circle stays.
        let key = f.heal.circles[&own.0].unwrap();
        let at = own.0 as usize;
        let mut kept = vec![None; at + 1];
        kept[at] = Some(at);
        f.effects.remap_objects(&kept, &kept);
        f.sync(&[]);
        assert_eq!(f.heal.circles[&own.0], Some(key));
        // If its slot vanishes, the effect world loses the circle, the sync creates it anew.
        f.effects.remap_objects(&[], &[]);
        assert!(f.effects.handle(key).is_none());
        f.sync(&[]);
        assert_eq!(f.circles(), [(own.0, 1280.0)]);
        // End the trance (`0x21D`): `ready` off, the circle stays in the teardown animation ...
        assert_eq!(f.say(own, msg::END_REQUEST), -1);
        assert!(!f.ready(own));
        f.step();
        assert_eq!(f.circles().len(), 1, "teardown animation");
        // ... until `0x41D`.
        let key = f.heal.circles[&own.0].unwrap();
        for _ in 0..200 {
            if f.host.state.object(own, false).unwrap().status & status::HEAL == 0 {
                break;
            }
            f.step();
        }
        assert_eq!(
            f.host.state.object(own, false).unwrap().status & status::HEAL,
            0
        );
        assert!(f.circles().is_empty(), "gone after 0x41D");
        assert!(f.effects.handle(key).is_none());
        // `reset` also clears an existing circle.
        f.entrance(own);
        assert_eq!(f.circles().len(), 1);
        let key = f.heal.circles[&own.0].unwrap();
        f.heal.reset(&mut f.effects);
        assert!(f.heal.circles.is_empty());
        assert!(f.effects.handle(key).is_none());
    }

    #[test]
    fn a_target_without_carrier_is_retried_in_the_next_frame() {
        let mut f = Fixture::new();
        let medic = spawn(&mut f.host, MEDIC, 0, (20, 20));
        let archer = spawn(&mut f.host, ARCHER, 0, (22, 20));
        f.entrance(medic);
        f.set_fraction(archer, 14, 50_000);
        // The carrier is not yet drawn at the first `Healed`: the pair stays without an
        // effect, but does not count as done.
        f.carried = false;
        let pair = (medic.0, archer.0);
        while !f
            .step()
            .iter()
            .any(|e| matches!(e, SimEvent::Healed { .. }))
        {}
        let t = f.heal.targets[&pair];
        assert!(t.key.is_none() && !t.failed);
        assert_eq!(f.effects.counts().world, 0);
        // The next frame without a new `Healed` draws the carrier: the effect arises with the
        // scale of the healing amount.
        f.carried = true;
        f.sync(&[]);
        assert_eq!(f.targets(), [(pair, 288.0 * HEAL_EFFECT_SCALE)]);
        assert_eq!(f.effects.counts().world, 1);
        // If the pair expires without a carrier, nothing is left behind.
        f.heal.reset(&mut f.effects);
        f.carried = false;
        while !f
            .step()
            .iter()
            .any(|e| matches!(e, SimEvent::Healed { .. }))
        {}
        f.set_fraction(archer, 14, 100_000);
        for _ in 0..4 {
            f.step();
        }
        assert!(f.heal.targets.is_empty());
        assert_eq!(f.effects.counts().world, 0);
    }

    #[test]
    fn target_effects_live_while_healed_events_come() {
        let mut f = Fixture::new();
        let medic = spawn(&mut f.host, MEDIC, 0, (20, 20));
        let archer = spawn(&mut f.host, ARCHER, 0, (22, 20));
        f.entrance(medic);
        const HP: u32 = 14;
        f.set_fraction(archer, HP, 50_000);
        let healed = |events: &[SimEvent]| {
            events
                .iter()
                .filter(|e| matches!(e, SimEvent::Healed { .. }))
                .count()
        };
        // Two object ticks with healing (medic H = 288): one effect on the target, scale 288 * 0.0055.
        let pair = (medic.0, archer.0);
        let mut seen = 0;
        let mut key = None;
        while seen < 2 {
            seen += healed(&f.step());
            assert!(f.heal.targets.len() <= 1);
            if let Some(t) = f.heal.targets.get(&pair) {
                assert!(key.is_none() || key == t.key, "same effect");
                key = t.key;
            }
        }
        let scale = 288.0 * HEAL_EFFECT_SCALE;
        assert_eq!(f.targets(), [(pair, scale)]);
        assert_eq!(f.effects.counts().world, 1);
        // Complete rebuild: lost and recreated with the next healing.
        f.effects.clear();
        while healed(&f.step()) == 0 {}
        assert_eq!(f.targets(), [(pair, scale)]);
        // Target full: no `Healed` in the next object tick, the effect is removed.
        f.set_fraction(archer, HP, 100_000);
        let key = f.heal.targets[&pair].key.unwrap();
        for _ in 0..2 {
            assert_eq!(healed(&f.step()), 0);
        }
        assert!(f.heal.targets.is_empty());
        assert!(f.effects.game_effect(key).is_none());
        assert_eq!(f.effects.counts().world, 0);
        // No leak over further ticks, not even without steps (pause).
        f.sync(&[]);
        for _ in 0..4 {
            f.step();
        }
        assert!(f.heal.targets.is_empty());
        // `reset` removes a running target effect.
        f.set_fraction(archer, HP, 50_000);
        while healed(&f.step()) == 0 {}
        assert_eq!(f.targets().len(), 1);
        f.heal.reset(&mut f.effects);
        assert!(f.heal.targets.is_empty());
        assert_eq!(f.effects.counts().world, 0);
    }
}
