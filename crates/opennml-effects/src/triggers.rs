//! Triggers of the effects without game simulation (re/effects, section "Triggers"): permanent effects
//! at helper nodes from `effectlist.ini`, locomotive smoke, standing ships, fishing baskets and the
//! ambient effects of the map - as the game creates them when placing the `.wld` objects.

use opennml_formats::hef::{DummyEffect, EFFECT_LIST_PATH, parse_effect_list};
use opennml_formats::text::decode_cp1252;
use opennml_rda::{Vfs, normalize_path};
use opennml_world::anim::{Playback, idle_animation};
use opennml_world::assets::{ModelAsset, ModelCache};
use opennml_world::classes::{BaseModule, TypeClass, module};
use opennml_world::level::{LevelScene, ObjectClass};

use crate::ambient::{
    Ambient, AmbientKind, EAGLE_GUID, Eagle, FISH_BASKET_GUID, FISH_GUID, Fishes, SURF_GUIDS, Surf,
};
use crate::attached::{Anchor, UNLIMITED_MS};
use crate::world::{EffectWorld, OwnedKind, ShipStand, WorldKind};

/// Locomotive smoke at `dampf%02d` (`Train_SetupWagonsAndSmoke` `0x00530220`).
pub const TRAIN_SMOKE: &str = "data/effects/lok_rauch.pef";
/// Walk flags of the watercraft with water effects (`0x20000` + `0x2000`).
pub const SHIP_FLAGS: u32 = 0x22000;

/// Reads `data/effects/effectlist.ini`; if it is missing, there are no permanent effects (warning).
pub fn load_effect_list(vfs: &Vfs) -> Vec<DummyEffect> {
    match vfs.read(EFFECT_LIST_PATH) {
        Ok(bytes) => parse_effect_list(&decode_cp1252(&bytes)),
        Err(e) => {
            log::warn!("{EFFECT_LIST_PATH} not readable, no permanent effects: {e}");
            Vec::new()
        }
    }
}

/// Does the object have a helper node of this name (mesh type 3005, exact match after lowercasing)?
/// The search is done as in the original (`GameObject_GetHelperNodeLocalPosition` `0x0046CFB0` ->
/// `ModelInstance_GetHelperNodePosition` `0x0058DAB0`) first in the model, then recursively in the
/// sub-objects of its playback `playback` ([`Playback::helper_node`]).
pub fn has_helper(model: &ModelAsset, playback: &Playback, name: &str) -> bool {
    playback
        .helper_node(model, playback.player().started_ms, name)
        .is_some()
}

/// Nodes `<prefix>01`, `<prefix>02`, ... up to the first missing one (`sprintf("%s%02d")`); `has`
/// checks whether a node exists.
pub fn numbered(prefix: &str, has: impl Fn(&str) -> bool) -> Vec<String> {
    (1..)
        .map(|i| format!("{prefix}{i:02}").to_lowercase())
        .take_while(|n| has(n))
        .collect()
}

/// [`numbered`] with the helper nodes of the object ([`has_helper`]).
pub fn numbered_nodes(model: &ModelAsset, playback: &Playback, prefix: &str) -> Vec<String> {
    numbered(prefix, |n| has_helper(model, playback, n))
}

/// Playback of an object when placing: the animation of the map view
/// ([`idle_animation`]) with all sub-objects; it determines which sub-objects the
/// node search reaches.
pub fn placement_playback(model: &ModelAsset, vfs: &Vfs, models: &mut ModelCache) -> Playback {
    Playback::new(
        model,
        |file| models.get(vfs, file),
        idle_animation(model),
        0.0,
    )
}

/// Permanent effects of an object (`GameObject_AttachDummyEffects` `0x0046EEA0`): per entry of the
/// list the existing nodes (also in sub-objects, [`has_helper`]); pairs (node,
/// effect). The scaling of the list is not used, as in the original.
pub fn dummy_effects(
    list: &[DummyEffect],
    model: &ModelAsset,
    playback: &Playback,
) -> Vec<(String, String)> {
    list.iter()
        .flat_map(|e| {
            numbered_nodes(model, playback, &e.node)
                .into_iter()
                .map(|node| (node, e.effect.clone()))
        })
        .collect()
}

/// Moving object (modules Walk, Train, MultiPart).
pub fn is_moving(class: &TypeClass) -> bool {
    [module::WALK, module::TRAIN, module::MULTI_PART]
        .iter()
        .any(|&m| class.has_module(m))
}

/// Attaches an effect permanently to a node of the object.
#[allow(clippy::too_many_arguments)]
fn attach_node(
    world: &mut EffectWorld,
    vfs: &Vfs,
    models: &mut ModelCache,
    object: usize,
    moving: bool,
    node: &str,
    effect: &str,
    run_invisible: bool,
) {
    let anchor = Anchor::Node(node.to_string());
    if let Some(e) = world.new_effect(vfs, models, effect, UNLIMITED_MS, anchor, run_invisible) {
        world.add_owned(object, moving, OwnedKind::Attached(e), -1);
    }
}

/// Counts of the setup (corpus test).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Populated {
    /// Objects with permanent effects per base module Walk, Building, Nature.
    pub dummy_objects: [usize; 3],
    pub dummy_effects: usize,
    pub trains: usize,
    pub ships: usize,
    pub baskets: usize,
    pub ambient: usize,
}

/// Creates the effects of a level like the placement of the `.wld` objects
/// (`World_PlaceObject` `0x00440720` and the module inits): ambient effects from
/// `particles_quality` 1 in file order, then [`populate_object`] per object. All buildings
/// count as finished (the `.wld` objects have `0x100`, re/effects, section "Permanent effects").
pub fn populate_level(
    world: &mut EffectWorld,
    scene: &LevelScene,
    list: &[DummyEffect],
    vfs: &Vfs,
    models: &mut ModelCache,
    particles_quality: u8,
) -> Populated {
    populate_level_with(world, scene, list, vfs, models, particles_quality, &|_| {
        true
    })
}

/// [`populate_level`] with the state of the buildings: `finished(i)` says whether object `i` is placed as a finished
/// building (construction sites get their permanent effects only at the end of construction,
/// [`attach_dummies`]).
#[allow(clippy::too_many_arguments)]
pub fn populate_level_with(
    world: &mut EffectWorld,
    scene: &LevelScene,
    list: &[DummyEffect],
    vfs: &Vfs,
    models: &mut ModelCache,
    particles_quality: u8,
    finished: &dyn Fn(usize) -> bool,
) -> Populated {
    let mut done = Populated::default();
    for (index, object) in scene.objects.iter().enumerate() {
        if object.class != ObjectClass::Ambient {
            let one = populate_object(
                world,
                scene,
                index,
                list,
                vfs,
                models,
                particles_quality,
                finished(index),
            );
            done.add(one);
            continue;
        }
        if particles_quality < 1 {
            continue;
        }
        let Some(kind) = scene.project.objects.get(&object.guid) else {
            continue;
        };
        let yaw = object.rotation.y;
        let ambient = if SURF_GUIDS.contains(&object.guid) {
            let model = normalize_path(&kind.model);
            let Some(asset) = models.get(vfs, &model) else {
                continue;
            };
            AmbientKind::Surf(Surf::new(
                object.guid,
                &model,
                object.position,
                yaw,
                asset.rdo.bounds.radius,
                world.rng(),
            ))
        } else if object.guid == FISH_GUID {
            let Some(model) = kind.shadow_model.as_deref().map(normalize_path) else {
                continue;
            };
            let Some(asset) = models.get(vfs, &model) else {
                continue;
            };
            AmbientKind::Fishes(Fishes::new(
                &model,
                object.position,
                asset.rdo.bounds.radius,
                world.rng(),
            ))
        } else if object.guid == EAGLE_GUID {
            let model = normalize_path(&kind.model);
            let Some(asset) = models.get(vfs, &model) else {
                continue;
            };
            AmbientKind::Eagle(Eagle::new(
                &model,
                object.position,
                asset.rdo.bounds.radius,
                world.now_ms(),
            ))
        } else {
            continue;
        };
        world.add_world(WorldKind::Ambient(Ambient::new(ambient)));
        done.ambient += 1;
    }
    done
}

impl Populated {
    fn add(&mut self, other: Populated) {
        for (a, b) in self.dummy_objects.iter_mut().zip(other.dummy_objects) {
            *a += b;
        }
        self.dummy_effects += other.dummy_effects;
        self.trains += other.trains;
        self.ships += other.ships;
        self.baskets += other.baskets;
        self.ambient += other.ambient;
    }
}

/// Effects of a placed object (position `index` in `scene.objects`, not an ambient effect) like
/// its module inits: permanent effects from `particles_quality` 2 (`PropertyWalk_InitBase`
/// `0x00532850`, `PropertyNatur_Init` `0x0051F0E0`; `PropertyBuilding_Init` `0x0050B150` only with
/// `finished`), locomotive smoke always, fishing baskets always, standing ships from 1 (in the game tick). Types
/// without a `property.ini` entry get no effects. An object created in the game (rebuild
/// of the object list) gets the same as a `.wld` object.
#[allow(clippy::too_many_arguments)]
pub fn populate_object(
    world: &mut EffectWorld,
    scene: &LevelScene,
    index: usize,
    list: &[DummyEffect],
    vfs: &Vfs,
    models: &mut ModelCache,
    particles_quality: u8,
    finished: bool,
) -> Populated {
    let mut done = Populated::default();
    let Some(object) = scene.objects.get(index) else {
        return done;
    };
    if object.class == ObjectClass::Ambient {
        return done;
    }
    let kind = scene.project.objects.get(&object.guid);
    let Some(class) = object.parts.first().and_then(|&g| scene.types.class(g)) else {
        return done;
    };
    let moving = is_moving(&class);
    let Some(asset) = models.get(vfs, &object.model) else {
        return done;
    };
    let playback = placement_playback(&asset, vfs, models);
    let building = class.base == Some(BaseModule::Building);
    if particles_quality >= 2 && (finished || !building) {
        done.add(attach_dummies(world, scene, index, list, vfs, models));
    }
    if class.has_module(module::TRAIN) {
        let nodes = numbered_nodes(&asset, &playback, "dampf");
        done.trains += usize::from(!nodes.is_empty());
        for node in nodes {
            attach_node(world, vfs, models, index, moving, &node, TRAIN_SMOKE, true);
        }
    }
    if object.guid == FISH_BASKET_GUID
        && let Some(model) = kind
            .and_then(|k| k.shadow_model.as_deref())
            .map(normalize_path)
        && let Some(fish) = models.get(vfs, &model)
    {
        let fishes = Fishes::new(&model, object.position, fish.rdo.bounds.radius, world.rng());
        let ambient = Ambient::new(AmbientKind::Fishes(fishes));
        world.add_owned(index, moving, OwnedKind::Fishes(ambient), -1);
        done.baskets += 1;
    }
    if particles_quality >= 1
        && class.has_module(module::WALK)
        && class.walk_flags & SHIP_FLAGS == SHIP_FLAGS
    {
        let p = &scene.terrain.params;
        let on_water = scene
            .terrain
            .ground_height(object.position.x, object.position.z)
            < p.water_level;
        world.add_ship(ShipStand::new(
            index,
            object.position,
            object.rotation.y,
            on_water,
        ));
        done.ships += 1;
    }
    done
}

/// Permanent effects of the object at position `index` (`GameObject_AttachDummyEffects` `0x0046EEA0`)
/// for units, buildings and nature, without checking `ParticlesQuality`; also at the end of construction
/// of a building at run time (`PropertyBuilding_OnBuildTick` `0x00508FB0`). Other objects
/// get none.
pub fn attach_dummies(
    world: &mut EffectWorld,
    scene: &LevelScene,
    index: usize,
    list: &[DummyEffect],
    vfs: &Vfs,
    models: &mut ModelCache,
) -> Populated {
    let mut done = Populated::default();
    let Some(object) = scene.objects.get(index) else {
        return done;
    };
    let Some(class) = object.parts.first().and_then(|&g| scene.types.class(g)) else {
        return done;
    };
    let (slot, run_invisible) = match class.base {
        Some(BaseModule::Walk) => (0, true),
        Some(BaseModule::Building) => (1, false),
        Some(BaseModule::Natur) => (2, false),
        _ => return done,
    };
    let Some(asset) = models.get(vfs, &object.model) else {
        return done;
    };
    let playback = placement_playback(&asset, vfs, models);
    let moving = is_moving(&class);
    let effects = dummy_effects(list, &asset, &playback);
    if !effects.is_empty() {
        done.dummy_objects[slot] += 1;
        done.dummy_effects += effects.len();
    }
    for (node, effect) in effects {
        attach_node(
            world,
            vfs,
            models,
            index,
            moving,
            &node,
            &effect,
            run_invisible,
        );
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_formats::object_cfg::ObjectConfig;
    use opennml_formats::rdo::{MeshKind, RdoLayout};
    use opennml_formats::testutil::sample_rdo;

    fn model(helpers: &[&str]) -> ModelAsset {
        let mut rdo = sample_rdo(RdoLayout::Standard);
        let template = rdo
            .meshes
            .iter()
            .find(|m| m.kind == MeshKind::Helper)
            .unwrap()
            .clone();
        rdo.meshes.retain(|m| m.kind != MeshKind::Helper);
        for name in helpers {
            let mut mesh = template.clone();
            mesh.name = (*name).into();
            rdo.meshes.push(mesh);
        }
        ModelAsset {
            path: "m.rdo".into(),
            rdo,
            config: ObjectConfig::default(),
            animations: vec![],
            material_textures: vec![],
            bone_track_cache: Default::default(),
        }
    }

    #[test]
    fn dummy_effects_use_numbered_helper_nodes_until_the_first_gap() {
        let list = parse_effect_list(
            "DUMMYOBJECT Feuer\r\nEFFECT 0.6 data/effects/feuer.pef\r\nDUMMYOBJECT Rauch\r\nEFFECT 1.0 data/effects/rauch_gerb.pef\r\n",
        );
        let m = model(&["feuer01", "feuer02", "feuer04", "rauch01", "Rauch02"]);
        let p = Playback::new(&m, |_| None, None, 0.0);
        assert_eq!(
            dummy_effects(&list, &m, &p),
            [
                ("feuer01".to_string(), "data/effects/feuer.pef".to_string()),
                ("feuer02".to_string(), "data/effects/feuer.pef".to_string()),
                (
                    "rauch01".to_string(),
                    "data/effects/rauch_gerb.pef".to_string()
                ),
            ]
        );
        // Uppercase nodes in the .rdo do not match (exact comparison).
        assert!(!has_helper(&m, &p, "rauch02"));
        assert_eq!(numbered_nodes(&m, &p, "dampf"), Vec::<String>::new());
    }

    #[test]
    fn dummy_effects_find_nodes_of_sub_objects() {
        use opennml_formats::object_cfg::SubObject;
        use std::sync::Arc;
        let list =
            parse_effect_list("DUMMYOBJECT Fackel\r\nEFFECT 1.0 data/effects/fackel.pef\r\n");
        let mut m = model(&["fackel01"]);
        m.config.sub_objects = vec![SubObject {
            file: "halter.rdo".into(),
            index: 0,
            start: 0,
            looping: false,
        }];
        let sub = Arc::new(model(&["fackel02"]));
        let p = Playback::new(&m, |_| Some(sub.clone()), None, 0.0);
        // `fackel02` is only in the sub-object: counted along, as in the original.
        assert_eq!(numbered_nodes(&m, &p, "fackel"), ["fackel01", "fackel02"]);
        assert_eq!(dummy_effects(&list, &m, &p).len(), 2);
        // Without sub-objects only the node of the model.
        let alone = Playback::new(&m, |_| None, None, 0.0);
        assert_eq!(numbered_nodes(&m, &alone, "fackel"), ["fackel01"]);
    }
}
