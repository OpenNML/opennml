//! Triggers in all levels of the original installation: number of objects with permanent effects per
//! base module, locomotive smoke, ships, fishing baskets and ambient effects (counts as in
//! re/effects). Skipped when the installation is missing.

use std::collections::BTreeSet;

use opennml_effects::triggers::{
    Populated, attach_dummies, load_effect_list, populate_level, populate_level_with,
    populate_object,
};
use opennml_effects::world::EffectWorld;
use opennml_rda::Vfs;
use opennml_rda::testutil::corpus_game_dir;
use opennml_world::assets::ModelCache;
use opennml_world::classes::BaseModule;
use opennml_world::level::{ObjectClass, level_paths, load_level};

// QUIRK D-252: kept
#[test]
fn every_level_gets_its_effects() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let list = load_effect_list(&vfs);
    assert_eq!(list.len(), 25);
    let mut models = ModelCache::new();
    let mut total = Populated::default();
    let mut types = [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()];
    let mut levels = 0;
    for path in level_paths(&vfs) {
        let Ok(scene) = load_level(&vfs, &path) else {
            continue;
        };
        levels += 1;
        let mut world = EffectWorld::new();
        let done = populate_level(&mut world, &scene, &list, &vfs, &mut models, 2);
        for k in 0..3 {
            total.dummy_objects[k] += done.dummy_objects[k];
        }
        total.dummy_effects += done.dummy_effects;
        total.trains += done.trains;
        total.ships += done.ships;
        total.baskets += done.baskets;
        total.ambient += done.ambient;
        // Types with permanent effects per base module.
        for object in &scene.objects {
            let Some(class) = object.parts.first().and_then(|&g| scene.types.class(g)) else {
                continue;
            };
            let has = world
                .objects()
                .iter()
                .any(|o| scene.objects[o.object].guid == object.guid && !o.is_empty());
            let slot = match class.base {
                Some(BaseModule::Walk) => 0,
                Some(BaseModule::Building) => 1,
                Some(BaseModule::Natur) => 2,
                _ => continue,
            };
            if has && object.guid != 3505 {
                types[slot].insert(object.guid);
            }
        }
        // With particles_quality 0 only locomotive smoke and fishing baskets.
        let mut low = EffectWorld::new();
        let none = populate_level(&mut low, &scene, &list, &vfs, &mut models, 0);
        assert_eq!(
            (none.dummy_effects, none.ambient, none.ships),
            (0, 0, 0),
            "{path}"
        );
        assert_eq!(none.trains, done.trains, "{path}");

        // Placed individually (rebuild of the object list: new objects) each object gets the same.
        let mut single = EffectWorld::new();
        let mut each = Populated::default();
        for (index, object) in scene.objects.iter().enumerate() {
            if object.class == ObjectClass::Ambient {
                continue;
            }
            let one = populate_object(
                &mut single,
                &scene,
                index,
                &list,
                &vfs,
                &mut models,
                2,
                true,
            );
            for k in 0..3 {
                each.dummy_objects[k] += one.dummy_objects[k];
            }
            each.dummy_effects += one.dummy_effects;
            each.trains += one.trains;
            each.ships += one.ships;
            each.baskets += one.baskets;
        }
        assert_eq!(each, Populated { ambient: 0, ..done }, "{path}");
        assert_eq!(single.counts().owned, world.counts().owned, "{path}");

        // Placed as construction sites the buildings have no permanent effects; the end of construction
        // (`attach_dummies`) gives them exactly those of placement as a finished building.
        let mut sites = EffectWorld::new();
        let early =
            populate_level_with(&mut sites, &scene, &list, &vfs, &mut models, 2, &|_| false);
        assert_eq!(early.dummy_objects[1], 0, "{path}");
        assert_eq!(
            (early.dummy_objects[0], early.dummy_objects[2]),
            (done.dummy_objects[0], done.dummy_objects[2]),
            "{path}"
        );
        let mut finished = 0;
        for (index, object) in scene.objects.iter().enumerate() {
            let building = object
                .parts
                .first()
                .and_then(|&g| scene.types.class(g))
                .is_some_and(|c| c.base == Some(BaseModule::Building));
            if building {
                let added = attach_dummies(&mut sites, &scene, index, &list, &vfs, &mut models);
                finished += added.dummy_effects;
            }
        }
        assert_eq!(early.dummy_effects + finished, done.dummy_effects, "{path}");
        assert_eq!(sites.counts().owned, world.counts().owned, "{path}");
    }
    assert_eq!(levels, 55);
    // Ghost and zombie, 16 building types, the nature types with torches, fire bowls, campfire
    // and fog stone (the wolf stone 4248 appears in no level).
    assert_eq!(total.dummy_objects, [25, 202, 1424]);
    assert_eq!(types.map(|t| t.len()), [2, 16, 8]);
    assert_eq!(total.dummy_effects, 1988);
    // Surf 1,012 + 624 + 195, fish 500 (without the orphaned vicious circle.wld), eagles 54.
    assert_eq!(total.ambient, 2385);
    assert_eq!((total.baskets, total.trains, total.ships), (98, 24, 77));
}
