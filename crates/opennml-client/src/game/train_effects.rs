//! Brake sparks of the train (re/train-multipart 7, 8 no. 14; quirk D-216): if the locomotive carries status bit 4
//! (braking), `Train_StartBraking` (`0x0052FEE0`) attaches via `0x921`/`0xA21` to the locomotive and its slaves one
//! `lok_funken.pef` each at the nodes `bremse01`, `bremse02`, ... up to the first missing one
//! (`Train_StartBrakeSparks` `0x0052FC60`: the locomotive has six such nodes, tender and wagons only `bremse01` and
//! `bremse02`, because `bremse03` is missing). `Train_StopBrakeSparks` (`0x0052F780`) ends them with the halt; the
//! list `m_Particles` is never emptied in the process (pure bookkeeping of the original, without visible effect).
//!
//! Pure presentation: the simulation knows the brake bit (`Train::status`), nothing more. The host derives the
//! sparks each frame from the state, so that a loaded savegame is also displayed correctly, and attaches them as game
//! effects with a carrier to the nodes (they follow the animation). If the effect world loses them in the complete
//! rebuild (`EffectWorld::clear` keeps only effects without a carrier; change of `ParticlesQuality`), the next sync
//! creates them again. The rebuild of the object list after creating or destroying carries them on with their
//! carrier.

use std::collections::BTreeMap;

use opennml_effects::attached::{Anchor, UNLIMITED_MS};
use opennml_effects::world::EffectWorld;
use opennml_rda::Vfs;
use opennml_sim::id::ObjectId;
use opennml_sim::module::ModuleKind;
use opennml_sim::module::train::{INDEX, status};
use opennml_world::assets::ModelCache;

use crate::game::sim_host::SimHost;

/// Effect of the brake sparks (`Train_StartBrakeSparks`).
pub const BRAKE_SPARKS: &str = "data/effects/lok_funken.pef";
/// Node name before the running number (`sprintf("bremse%02d")`).
pub const BRAKE_NODE: &str = "bremse";

/// Sparks of the train objects of a map.
#[derive(Debug, Default)]
pub struct TrainSparks {
    /// Keys of the spark effects per train object (ID), one entry per node.
    active: BTreeMap<u32, Vec<u32>>,
}

impl TrainSparks {
    /// Removes all sparks (loading, restart).
    pub fn reset(&mut self, effects: &mut EffectWorld) {
        for key in std::mem::take(&mut self.active).into_values().flatten() {
            effects.remove_game_effect(key);
        }
    }

    /// The train objects that spark: every locomotive with the brake bit (status bit 4) and its existing slaves
    /// (`0x921` to tender and wagons), in table order, the locomotive before its slaves.
    pub fn sparking(sim: &SimHost) -> Vec<ObjectId> {
        let mut out = Vec::new();
        for o in sim.state.objects() {
            let Some(ModuleKind::Train(t)) = o.module(INDEX).map(|m| &m.kind) else {
                continue;
            };
            if t.is_slave || t.status & status::BRAKING == 0 {
                continue;
            }
            out.push(o.id);
            out.extend(
                t.slaves
                    .iter()
                    .copied()
                    .filter(|&s| sim.state.object(s, false).is_some()),
            );
        }
        out
    }

    /// Sync with the simulation after a frame: sparks for every object that is to spark and does not have any yet
    /// (`nodes` names the nodes `<prefix>01...` of the object model up to the first missing one, `carrier` the
    /// index of the object in the object list of the map), end of the sparks (`Stop`: the particles die out) for
    /// all that are no longer to spark. Sparks that the effect world lost in the rebuild are created again.
    pub fn sync(
        &mut self,
        sim: &SimHost,
        effects: &mut EffectWorld,
        models: &mut ModelCache,
        vfs: &Vfs,
        nodes: &dyn Fn(ObjectId, &str) -> Vec<String>,
        carrier: &dyn Fn(ObjectId) -> Option<usize>,
    ) {
        let wanted = Self::sparking(sim);
        self.active.retain(|&id, keys| {
            if wanted.iter().any(|w| w.0 == id) {
                return true;
            }
            for key in keys.iter() {
                if let Some(a) = effects.game_effect_mut(*key) {
                    a.stop();
                }
            }
            false
        });
        for id in wanted {
            if let Some(keys) = self.active.get(&id.0) {
                if keys.iter().all(|&k| effects.game_effect(k).is_some()) {
                    continue;
                }
                // Part of it was lost in the rebuild: remove the rest, create anew.
                for &key in keys {
                    effects.remove_game_effect(key);
                }
                self.active.remove(&id.0);
            }
            let Some(carrier) = carrier(id) else {
                continue;
            };
            let keys = nodes(id, BRAKE_NODE)
                .into_iter()
                .filter_map(|node| {
                    effects.add_game_effect(
                        vfs,
                        models,
                        BRAKE_SPARKS,
                        Anchor::Node(node),
                        Some(carrier),
                        1.0,
                        0,
                        UNLIMITED_MS,
                    )
                })
                .collect();
            // An empty list as well (no node, effect not loadable) counts as done.
            self.active.insert(id.0, keys);
        }
    }

    /// Number of spark nodes per object (tests).
    #[cfg(test)]
    fn count(&self, id: ObjectId) -> usize {
        self.active.get(&id.0).map_or(0, Vec::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::sim_host::testkit::*;
    use opennml_rda::testutil::{TestFile, build_rda};

    /// Archive with `lok_funken.pef` (one emitter in a loop).
    fn archive() -> (tempfile::TempDir, Vfs) {
        let pef = "EFFECT\r\n{\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tLOOP 1\r\n\t\tEMIT 10\r\n\t\tLIFE 1\r\n\t\tACCEL 1\r\n\t}\r\n}\r\n";
        let files = [TestFile {
            path: BRAKE_SPARKS,
            data: pef.as_bytes(),
            compress: false,
        }];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    /// Nodes as in the shipped data: the locomotive `bremse01...06`, tender and wagons `bremse01`, `bremse02`.
    fn nodes(host: &SimHost) -> impl Fn(ObjectId, &str) -> Vec<String> + '_ {
        move |id, prefix| {
            let guid = host.state.object(id, false).map_or(0, |o| o.guid);
            let count = if guid == LOCO { 6 } else { 2 };
            (1..=count).map(|i| format!("{prefix}{i:02}")).collect()
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        vfs: Vfs,
        host: SimHost,
        sparks: TrainSparks,
        effects: EffectWorld,
        models: ModelCache,
        loco: ObjectId,
        slaves: Vec<ObjectId>,
    }

    impl Fixture {
        fn new() -> Self {
            let (dir, vfs) = archive();
            let mut host = train_host();
            lay_track(&mut host, (10, 20), (49, 20));
            let loco = spawn_dir(&mut host, LOCO, 0, (12, 20), 0);
            let slaves = match &host
                .state
                .object(loco, false)
                .unwrap()
                .module(INDEX)
                .unwrap()
                .kind
            {
                ModuleKind::Train(t) => t.slaves.clone(),
                _ => panic!("not a train"),
            };
            assert_eq!(slaves.len(), 2);
            Self {
                _dir: dir,
                vfs,
                host,
                sparks: TrainSparks::default(),
                effects: EffectWorld::new(),
                models: ModelCache::new(),
                loco,
                slaves,
            }
        }

        fn set_status(&mut self, bits: u32) {
            let o = self.host.state.object_mut(self.loco, false).unwrap();
            let Some(ModuleKind::Train(t)) = o.module_mut(INDEX).map(|m| &mut m.kind) else {
                panic!("not a train");
            };
            t.status = bits;
        }

        fn sync(&mut self) {
            let nodes = nodes(&self.host);
            self.sparks.sync(
                &self.host,
                &mut self.effects,
                &mut self.models,
                &self.vfs,
                &nodes,
                &|id| Some(id.0 as usize),
            );
        }

        /// Names of the nodes of the running sparks per object.
        fn attached(&self, id: ObjectId) -> Vec<String> {
            self.sparks
                .active
                .get(&id.0)
                .into_iter()
                .flatten()
                .filter_map(|&k| self.effects.game_effect(k))
                .filter_map(|a| a.node().map(str::to_string))
                .collect()
        }
    }

    // QUIRK D-216: kept
    #[test]
    fn brake_sparks_follow_the_braking_bit() {
        let mut f = Fixture::new();
        // Moves without brake bit: no sparks.
        f.set_status(status::FORWARD | status::MOVING);
        f.sync();
        assert_eq!(f.effects.counts().world, 0);
        assert!(TrainSparks::sparking(&f.host).is_empty());
        // Bit 4: six nodes on the locomotive, `bremse01` and `bremse02` on tender and wagons.
        f.set_status(status::FORWARD | status::MOVING | status::BRAKING);
        assert_eq!(
            TrainSparks::sparking(&f.host),
            [f.loco, f.slaves[0], f.slaves[1]]
        );
        let sum = opennml_sim::save::checksum(&f.host.state);
        f.sync();
        assert_eq!(opennml_sim::save::checksum(&f.host.state), sum);
        assert_eq!(f.effects.counts().world, 10);
        assert_eq!(
            f.attached(f.loco),
            [
                "bremse01", "bremse02", "bremse03", "bremse04", "bremse05", "bremse06"
            ]
        );
        for &s in &f.slaves {
            assert_eq!(f.attached(s), ["bremse01", "bremse02"]);
        }
        // Every frame again: no duplicates.
        f.sync();
        f.sync();
        assert_eq!(f.effects.counts().world, 10);
        // Bit off (halt): the sparks end, their particles die out.
        let keys: Vec<u32> = f.sparks.active.values().flatten().copied().collect();
        f.set_status(0);
        f.sync();
        assert_eq!(f.sparks.count(f.loco) + f.sparks.count(f.slaves[0]), 0);
        for key in keys {
            assert!(
                f.effects
                    .game_effect(key)
                    .is_some_and(|a| !a.effect.is_active()),
                "the effect is stopped"
            );
        }
        // Braking again begins new sparks.
        f.set_status(status::BACKWARD | status::MOVING | status::BRAKING);
        f.sync();
        assert_eq!(f.sparks.count(f.loco), 6);
        assert_eq!(f.sparks.count(f.slaves[1]), 2);
    }

    #[test]
    fn rebuilt_effect_worlds_get_their_sparks_back_and_missing_nodes_end_the_row() {
        let mut f = Fixture::new();
        f.set_status(status::FORWARD | status::MOVING | status::BRAKING);
        f.sync();
        let world = f.effects.counts().world;
        assert_eq!(world, 10);
        // The complete rebuild resets the effects with a carrier: the sync creates them anew.
        f.effects.clear();
        assert_eq!(f.effects.counts().world, 0);
        f.sync();
        assert_eq!(f.effects.counts().world, world);
        // A model without `bremse01` (no node) does not spark; a carrier is missing (object not on
        // the map): no sparks in that case either.
        let mut g = Fixture::new();
        g.set_status(status::FORWARD | status::MOVING | status::BRAKING);
        let nodes = |_: ObjectId, _: &str| Vec::<String>::new();
        g.sparks.sync(
            &g.host,
            &mut g.effects,
            &mut g.models,
            &g.vfs,
            &nodes,
            &|_| Some(0),
        );
        assert_eq!(g.effects.counts().world, 0);
        let mut h = Fixture::new();
        h.set_status(status::FORWARD | status::MOVING | status::BRAKING);
        let nodes = nodes_for(&h.host);
        h.sparks.sync(
            &h.host,
            &mut h.effects,
            &mut h.models,
            &h.vfs,
            &nodes,
            &|_| None,
        );
        assert_eq!(h.effects.counts().world, 0);
        // `reset` removes everything.
        f.sparks.reset(&mut f.effects);
        assert_eq!(f.effects.counts().world, 0);
    }

    fn nodes_for(host: &SimHost) -> impl Fn(ObjectId, &str) -> Vec<String> + '_ {
        nodes(host)
    }
}
