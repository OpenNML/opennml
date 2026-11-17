//! Class of an object type and from it the shadow kind like `ObjectRenderer_InitShadow`
//! (`0x005493B0`): modules, module flags and category bits from `data/gifcon/property.ini`, riders
//! from `data/gifcon/translate.ini`, shadow files from the project file; see
//! re/object-classes (mapping section) and re/shadows.

use std::collections::BTreeMap;

use nml_formats::prj::Project;
use nml_formats::property::{
    ObjectType, PROPERTY_PATH, TRANSLATE_PATH, expand_guid, parse_property_ini, parse_translate,
};
use nml_formats::text::decode_cp1252;
use nml_formats::wld::Area;
use nml_rda::{Vfs, normalize_path};

/// Module indices (`PROPID n`), as far as shadows and render bits need them.
pub mod module {
    pub const WALK: u32 = 0;
    pub const STAND: u32 = 2;
    pub const BUILDING: u32 = 3;
    pub const TREE: u32 = 4;
    pub const NATUR: u32 = 5;
    pub const RESOURCE: u32 = 6;
    pub const MULTI_HEAD: u32 = 0x0B;
    pub const TRAIN: u32 = 0x21;
    pub const MULTI_PART: u32 = 0x33;
}

/// Base module of a type (every created type has exactly one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseModule {
    /// Module 0: units, animals, ships, vehicles.
    Walk,
    Building,
    Tree,
    Natur,
    Resource,
    Train,
    MultiPart,
}

impl BaseModule {
    /// Name in the state snapshot and overlay.
    pub fn name(self) -> &'static str {
        match self {
            Self::Walk => "Einheit",
            Self::Building => "Gebäude",
            Self::Tree => "Baum",
            Self::Natur => "Natur",
            Self::Resource => "Rohstoff",
            Self::Train => "Zug",
            Self::MultiPart => "Mehrteilig",
        }
    }
}

/// Shadow kind (`ObjectRenderer+0xC8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShadowKind {
    /// 0: no shadow.
    None,
    /// 1: shadow model decal (`OBJ_SHADOW_FILENAME`).
    Decal,
    /// 2: shadow texture, re-rendered on camera change, becoming visible and shadow quality change (F8),
    /// otherwise taken from the atlas.
    Persistent,
    /// 3: shadow texture per frame.
    PerFrame,
}

impl ShadowKind {
    /// Number as in the original (0-3).
    pub fn number(self) -> u8 {
        self as u8
    }

    /// Shadow texture (kind 2 or 3).
    pub fn is_texture(self) -> bool {
        matches!(self, Self::Persistent | Self::PerFrame)
    }
}

/// GUIDs with ship flag `ObjectRenderer+0x74` (constructor `0x00549A1B`), only if
/// `OBJ_SCAFFOLD_FILENAME` is set.
pub const SHIP_GUIDS: [u32; 6] = [1108, 1109, 1117, 2009, 2010, 3505];

/// Render bit `0x2`: the object goes through the receiver pass (shadow on the ground).
pub const RENDER_SHADOW: u32 = 0x2;

/// Class of an object type from `property.ini`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeClass {
    pub guid: u32,
    /// `NAME` of the type file.
    pub name: String,
    pub base: Option<BaseModule>,
    /// Category bits (`VALUEID 100 & 0xFFFFFF00`, `GameObject+0x330`).
    pub category: u32,
    /// Module indices in ascending order (without repetition).
    pub modules: Vec<u32>,
    /// Flags of module 0 (`VALUEID 10003`).
    pub walk_flags: u32,
    /// Flags of module 2 (`VALUEID 30003`).
    pub stand_flags: u32,
    /// Flags of module 6 (`VALUEID 70002`).
    pub resource_flags: u32,
}

fn flags(object: &ObjectType, index: u32, value: u32) -> u32 {
    object
        .module(index)
        .and_then(|m| m.value_int(value))
        .unwrap_or(0) as u32
}

impl TypeClass {
    pub fn new(object: &ObjectType) -> Self {
        let mut modules: Vec<u32> = object.modules.iter().map(|m| m.index).collect();
        modules.sort_unstable();
        modules.dedup();
        let has = |n| modules.contains(&n);
        let base = [
            (module::WALK, BaseModule::Walk),
            (module::BUILDING, BaseModule::Building),
            (module::TREE, BaseModule::Tree),
            (module::NATUR, BaseModule::Natur),
            (module::RESOURCE, BaseModule::Resource),
            (module::MULTI_PART, BaseModule::MultiPart),
            (module::TRAIN, BaseModule::Train),
        ]
        .into_iter()
        .find(|&(n, _)| has(n))
        .map(|(_, base)| base);
        Self {
            guid: object.guid,
            name: object.name.clone(),
            base,
            category: object.category(),
            walk_flags: flags(object, module::WALK, 10003),
            stand_flags: flags(object, module::STAND, 30003),
            resource_flags: flags(object, module::RESOURCE, 70002),
            modules,
        }
    }

    /// `GameObject_HasModule(n)`.
    pub fn has_module(&self, index: u32) -> bool {
        self.modules.contains(&index)
    }

    /// `GameObject_MatchesClass(m)` (`0x0046B9D0`) on placement: low byte `0xFF` checks
    /// the category bits, otherwise module `m & 0xFF` with its flags (`m & 0xFFFFFF00`).
    pub fn matches(&self, query: u32) -> bool {
        let bits = query & 0xFFFF_FF00;
        match query & 0xFF {
            0xFF => self.category & bits != 0,
            module::WALK => self.has_module(module::WALK) && self.walk_flags & bits != 0,
            module::STAND => self.has_module(module::STAND) && self.stand_flags & bits != 0,
            module::RESOURCE => {
                self.has_module(module::RESOURCE) && self.resource_flags & bits != 0
            }
            _ => false,
        }
    }

    /// Special class `ObjectRenderer+0xD4` = `Q(0x406)`: gold deposit and fertile soil.
    pub fn is_special(&self) -> bool {
        self.matches(0x406)
    }

    /// Render bits after the module inits on placement (re/object-classes, "Modules"
    /// table): `0x3` for all base modules, Stand only without flag `0x200`, Resource only with flag
    /// `0x400`. The only exception in the shipped data: the fishing baskets (3505) keep 0.
    pub fn render_bits(&self) -> u32 {
        let always = [
            module::WALK,
            module::BUILDING,
            module::TREE,
            module::NATUR,
            module::TRAIN,
            module::MULTI_PART,
        ]
        .iter()
        .any(|&n| self.has_module(n));
        let stand = self.has_module(module::STAND) && self.stand_flags & 0x200 == 0;
        let resource = self.has_module(module::RESOURCE) && self.resource_flags & 0x400 != 0;
        if always || stand || resource { 0x3 } else { 0 }
    }

    /// Shadow kind at `ShadowQuality` `quality` (`0x005493B0`, table in re/shadows).
    pub fn shadow_kind(&self, quality: u8, no_shadow: bool) -> ShadowKind {
        if self.is_special() {
            return ShadowKind::Decal;
        }
        if quality == 0 || no_shadow {
            return ShadowKind::None;
        }
        let building = self.has_module(module::BUILDING);
        let nature_bit = self.matches(0x100_00FF);
        match quality {
            1 => ShadowKind::Decal,
            2 if !building => ShadowKind::Decal,
            2 if nature_bit => ShadowKind::Persistent,
            2 => ShadowKind::PerFrame,
            3 if self.matches(0x200_00FF) => ShadowKind::PerFrame,
            3 if nature_bit => ShadowKind::Persistent,
            3 => ShadowKind::Decal,
            _ => ShadowKind::None,
        }
    }

    /// Edge length of the shadow texture (`ObjectRenderer+0xCC`): 128 for buildings, trees and
    /// large vehicles (`Q(0x20000)`), otherwise 64.
    pub fn texture_size(&self) -> u32 {
        if self.has_module(module::BUILDING)
            || self.has_module(module::TREE)
            || self.matches(0x20000)
        {
            128
        } else {
            64
        }
    }
}

/// Type data of a game: `property.ini` and `translate.ini`.
#[derive(Debug, Clone, Default)]
pub struct ObjectTypes {
    pub types: BTreeMap<u32, ObjectType>,
    pub translations: BTreeMap<u32, Vec<u32>>,
}

impl ObjectTypes {
    /// Reads both files; if one is missing, its part stays empty (warning). Without type data
    /// the viewer keeps drawing the objects, only without shadows.
    pub fn load(vfs: &Vfs) -> Self {
        let read = |path: &str| match vfs.read(path) {
            Ok(bytes) => Some(decode_cp1252(&bytes)),
            Err(e) => {
                log::warn!("{path} not readable, object classes missing: {e}");
                None
            }
        };
        Self {
            types: read(PROPERTY_PATH).map_or_else(BTreeMap::new, |t| parse_property_ini(&t)),
            translations: read(TRANSLATE_PATH).map_or_else(BTreeMap::new, |t| parse_translate(&t)),
        }
    }

    /// Types that the game creates for `guid`: translated (rider -> rider + horse), without
    /// types that are missing from `property.ini` (`GameWorld_CreateObject` aborts there).
    pub fn parts(&self, guid: u32) -> Vec<u32> {
        expand_guid(&self.translations, guid)
            .into_iter()
            .filter(|g| self.types.contains_key(g))
            .collect()
    }

    pub fn class(&self, guid: u32) -> Option<TypeClass> {
        self.types.get(&guid).map(TypeClass::new)
    }
}

/// Is tile `(x, z)` in a `noshadow...` area (`GameEngine_IsInNoShadowArea`
/// `0x00431A70`: `PtInRect` with exclusive end `x1 + 1`, `z1 + 1`)?
pub fn in_no_shadow_area(areas: &[Area], tile: (i64, i64)) -> bool {
    areas
        .iter()
        .filter(|a| a.name.starts_with("noshadow"))
        .any(|a| {
            let [x0, z0, x1, z1] = a.rect.map(i64::from);
            (x0..x1 + 1).contains(&tile.0) && (z0..z1 + 1).contains(&tile.1)
        })
}

/// A decal below an object.
#[derive(Debug, Clone, PartialEq)]
pub struct DecalSetup {
    /// Normalized path of the `.rdo` (hull and material 0 texture).
    pub model: String,
    pub kind: DecalKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecalKind {
    /// Shadow decal (`ShadowCaster_DrawShadowDecal`): color `tex * 128/255`.
    Shadow,
    /// Ground plate (`ShadowCaster_DrawGroundplaneDecal`): color `tex * terrain color`.
    Groundplane,
    /// Decal of the special class in the object pass: color `tex * k'` with `k' = trunc(k * 255)/255`,
    /// `k` = brightness factor of the fog (`shadows::special_decal_color`).
    Special,
}

/// Shadow of a placed object at a `ShadowQuality` (result of
/// `ObjectRenderer_InitShadow` for all parts of the object).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectShadow {
    /// Shadow kind of the first part with a shadow texture, otherwise that of the first part.
    pub kind: ShadowKind,
    /// Edge length of the shadow texture (64 or 128).
    pub texture_size: u32,
    /// Raise the receiver to `WATER_LEVEL` (`ObjectRenderer+0x70`).
    pub on_water: bool,
    /// Decals in drawing order (per part: ground plate, then shadow decal).
    pub decals: Vec<DecalSetup>,
}

impl ObjectShadow {
    pub const NONE: Self = Self {
        kind: ShadowKind::None,
        texture_size: 64,
        on_water: false,
        decals: Vec::new(),
    };
}

/// Shadow of an object with the part GUIDs `parts` (see [`ObjectTypes::parts`]). Parts without
/// render bit `0x2` (fishing baskets) cast no shadow. The shadow texture applies to the
/// drawn model as a whole (rider with horse: one texture, like the group in the original).
pub fn object_shadow(
    types: &ObjectTypes,
    project: &Project,
    parts: &[u32],
    quality: u8,
    no_shadow: bool,
) -> ObjectShadow {
    shadow_with(types, project, parts, |class| {
        class.shadow_kind(quality, no_shadow)
    })
}

/// Like [`object_shadow`], but with a given shadow kind for all parts (model browser, F8);
/// the special class keeps its decal.
pub fn object_shadow_as(
    types: &ObjectTypes,
    project: &Project,
    parts: &[u32],
    kind: ShadowKind,
) -> ObjectShadow {
    shadow_with(types, project, parts, |class| {
        if class.is_special() {
            ShadowKind::Decal
        } else {
            kind
        }
    })
}

/// First object type of the project file (smallest GUID with an entry in `property.ini`) whose
/// model is `model` (normalized path); for the model browser.
pub fn type_for_model(types: &ObjectTypes, project: &Project, model: &str) -> Option<u32> {
    let model = normalize_path(model);
    project
        .objects
        .values()
        .filter(|o| types.types.contains_key(&o.guid))
        .find(|o| normalize_path(&o.model) == model)
        .map(|o| o.guid)
}

fn shadow_with(
    types: &ObjectTypes,
    project: &Project,
    parts: &[u32],
    kind_of: impl Fn(&TypeClass) -> ShadowKind,
) -> ObjectShadow {
    let mut shadow = ObjectShadow::NONE;
    let mut first = true;
    for &guid in parts {
        let Some(class) = types.class(guid) else {
            continue;
        };
        if class.render_bits() & RENDER_SHADOW == 0 {
            continue;
        }
        let kind = kind_of(&class);
        let prj = project.objects.get(&guid);
        let file = |f: Option<&String>| f.map(|p| normalize_path(p));
        let shd = file(prj.and_then(|o| o.shadow_model.as_ref()));
        let gp = file(prj.and_then(|o| o.groundplane_model.as_ref()));
        let ship = SHIP_GUIDS.contains(&guid) && prj.is_some_and(|o| o.scaffold_model.is_some());
        let water_walker = class.matches(0x2000) && !class.matches(0x800);
        if first || (kind.is_texture() && !shadow.kind.is_texture()) {
            shadow.kind = kind;
            shadow.texture_size = class.texture_size();
        }
        first = false;
        shadow.on_water |= water_walker || ship;
        let mut push = |model: Option<String>, kind| {
            if let Some(model) = model {
                shadow.decals.push(DecalSetup { model, kind });
            }
        };
        if class.is_special() {
            push(shd, DecalKind::Special);
            continue;
        }
        match kind {
            ShadowKind::None => {}
            ShadowKind::Decal if ship => {}
            ShadowKind::Decal if shd.is_some() && gp.is_some() => push(shd, DecalKind::Groundplane),
            ShadowKind::Decal => push(shd, DecalKind::Shadow),
            ShadowKind::Persistent | ShadowKind::PerFrame => push(gp, DecalKind::Groundplane),
        }
    }
    shadow
}

impl crate::level::LevelScene {
    /// Shadow of the object `object` (index in `objects`) at `ShadowQuality` `quality`.
    pub fn object_shadow(&self, object: usize, quality: u8) -> ObjectShadow {
        let o = &self.objects[object];
        object_shadow(&self.types, &self.project, &o.parts, quality, o.no_shadow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::prj::parse_project;

    const TYPES: &str = "GUID 1901\nNAME ENG_House\nVALUEID 100 6 1 16777216\nPROPID 3 2\nVALUEID 40003 6 1 0\nGUID 1914\nNAME ENG_Gate\nVALUEID 100 6 1 33554432\nPROPID 3\nGUID 2006\nNAME ENG_Infantryman\nVALUEID 100 6 1 33558528\nPROPID 0\nVALUEID 10003 6 1 2048\nPROPID 2\nGUID 1108\nNAME SPA_Ship\nVALUEID 100 6 1 33554432\nPROPID 0\nVALUEID 10003 6 1 139264\nPROPID 2\nGUID 3602\nNAME Deciduous_tree\nVALUEID 100 6 1 33816832\nPROPID 4\nGUID 3707\nNAME Bush\nVALUEID 100 6 1 17039360\nPROPID 5\nVALUEID 60002 6 1 0\nGUID 3500\nNAME Gold_deposit\nVALUEID 100 6 1 1280\nPROPID 6\nVALUEID 70002 6 1 1280\nGUID 3505\nNAME Fish_baskets\nVALUEID 100 6 1 33554432\nPROPID 2\nVALUEID 30003 6 1 768\nPROPID 6\nVALUEID 70002 6 1 0\nGUID 3409\nNAME ANI_Horse\nVALUEID 100 6 1 33554432\nPROPID 0\nVALUEID 10003 6 1 2048\nPROPID 2\n";

    const PRJ: &str = "[OBJECT]\n{\nOBJ_GUID 1901\nOBJ_FILENAME \"h.rdo\"\nOBJ_SHADOW_FILENAME \"data\\\\r3d\\\\h_shd.rdo\"\nOBJ_GROUNDPLANE_FILENAME \"data\\\\r3d\\\\ground\\\\h.rdo\"\n}\n[OBJECT]\n{\nOBJ_GUID 2006\nOBJ_FILENAME \"i.rdo\"\nOBJ_SHADOW_FILENAME \"data\\\\r3d\\\\shadow.rdw\"\n}\n[OBJECT]\n{\nOBJ_GUID 3409\nOBJ_FILENAME \"p.rdo\"\nOBJ_SHADOW_FILENAME \"data\\\\r3d\\\\horse_shadow.rdo\"\n}\n[OBJECT]\n{\nOBJ_GUID 1108\nOBJ_FILENAME \"s.rdo\"\nOBJ_SCAFFOLD_FILENAME \"data\\\\r3d\\\\scaffold.rdo\"\n}\n[OBJECT]\n{\nOBJ_GUID 3500\nOBJ_FILENAME \"g.rdo\"\nOBJ_SHADOW_FILENAME \"data\\\\r3d\\\\gold_shd.rdo\"\n}\n[OBJECT]\n{\nOBJ_GUID 3505\nOBJ_FILENAME \"k.rdo\"\nOBJ_SHADOW_FILENAME \"data\\\\r3d\\\\k_shd.rdo\"\nOBJ_SCAFFOLD_FILENAME \"data\\\\r3d\\\\scaffold.rdo\"\n}\n";

    fn types() -> ObjectTypes {
        ObjectTypes {
            types: parse_property_ini(TYPES),
            translations: parse_translate("2008=2006+3409\n"),
        }
    }

    fn class(guid: u32) -> TypeClass {
        types().class(guid).unwrap()
    }

    // QUIRK D-77: kept
    #[test]
    fn shadow_kind_follows_the_quality_table() {
        let kinds = |guid| [0, 1, 2, 3, 4].map(|q| class(guid).shadow_kind(q, false).number());
        // Building 0x1000000 (house): 2 at SQ 2 and 3.
        assert_eq!(kinds(1901), [0, 1, 2, 2, 0]);
        // Building 0x2000000 (gate): 3 at SQ 2 and 3.
        assert_eq!(kinds(1914), [0, 1, 3, 3, 0]);
        // Unit, tree (0x2000000): decal at SQ 2, per frame at SQ 3.
        assert_eq!(kinds(2006), [0, 1, 1, 3, 0]);
        assert_eq!(kinds(3602), [0, 1, 1, 3, 0]);
        // Nature 0x1000000: permanent at SQ 3.
        assert_eq!(kinds(3707), [0, 1, 1, 2, 0]);
        // Special class: always decal, even at SQ 0 and in the noshadow area.
        assert_eq!(kinds(3500), [1; 5]);
        assert_eq!(class(3500).shadow_kind(2, true), ShadowKind::Decal);
        assert_eq!(class(2006).shadow_kind(2, true), ShadowKind::None);
    }

    #[test]
    fn classes_texture_sizes_and_render_bits() {
        assert_eq!(class(1901).base, Some(BaseModule::Building));
        assert_eq!(class(3707).base, Some(BaseModule::Natur));
        assert_eq!(class(2006).base, Some(BaseModule::Walk));
        assert_eq!(class(3505).base, Some(BaseModule::Resource));
        // 128 for building, tree, large vehicle (flag 0x20000), otherwise 64.
        assert_eq!(
            [1901, 3602, 1108, 2006, 3707].map(|g| class(g).texture_size()),
            [128, 128, 128, 64, 64]
        );
        assert!(class(3500).is_special() && !class(3505).is_special());
        // Fishing baskets: Stand with 0x200, Resource without 0x400 -> no render bits.
        assert_eq!(class(3505).render_bits(), 0);
        assert_eq!(class(3500).render_bits(), 3);
        assert_eq!(class(2006).render_bits(), 3);
    }

    // QUIRK D-75: kept
    #[test]
    fn object_shadow_picks_decals_groundplanes_and_textures() {
        let types = types();
        let project = parse_project(PRJ).unwrap();
        let shadow = |guid: u32, quality| {
            object_shadow(&types, &project, &types.parts(guid), quality, false)
        };
        // House SQ 1: decal with both files -> _shd as ground plate (terrain-lit).
        let house = shadow(1901, 1);
        assert_eq!(house.kind, ShadowKind::Decal);
        assert_eq!(
            house.decals,
            [DecalSetup {
                model: "data/r3d/h_shd.rdo".into(),
                kind: DecalKind::Groundplane
            }]
        );
        // SQ 2: shadow texture 128 and the ground plate from OBJ_GROUNDPLANE_FILENAME.
        let house = shadow(1901, 2);
        assert_eq!(
            (house.kind, house.texture_size),
            (ShadowKind::Persistent, 128)
        );
        assert_eq!(house.decals[0].model, "data/r3d/ground/h.rdo");
        assert!(shadow(1901, 0).decals.is_empty());
        // Infantryman: shadow decal (extension .rdo).
        let unit = shadow(2006, 2);
        assert_eq!(unit.decals[0].model, "data/r3d/shadow.rdo");
        assert_eq!(unit.decals[0].kind, DecalKind::Shadow);
        // Rider 2008 = infantryman + horse: two decals, one object.
        let rider = shadow(2008, 2);
        assert_eq!(rider.decals.len(), 2);
        assert_eq!(rider.decals[1].model, "data/r3d/horse_shadow.rdo");
        // Ship with scaffold: no decal, receiver on the water; per frame at SQ 3.
        let ship = shadow(1108, 2);
        assert!(ship.decals.is_empty() && ship.on_water);
        assert_eq!(shadow(1108, 3).kind, ShadowKind::PerFrame);
        // Gold: special class even at SQ 0; fishing baskets without render bit 2: nothing.
        assert_eq!(shadow(3500, 0).decals[0].kind, DecalKind::Special);
        assert_eq!(shadow(3505, 3), ObjectShadow::NONE);
        // Unknown type: nothing.
        assert_eq!(shadow(4711, 2), ObjectShadow::NONE);
        assert!(types.parts(4711).is_empty());
    }

    #[test]
    fn browser_shadows_force_the_kind_and_find_the_type_of_a_model() {
        let types = types();
        let project = parse_project(PRJ).unwrap();
        let unit = object_shadow_as(&types, &project, &[2006], ShadowKind::PerFrame);
        assert_eq!((unit.kind, unit.texture_size), (ShadowKind::PerFrame, 64));
        assert!(unit.decals.is_empty());
        let house = object_shadow_as(&types, &project, &[1901], ShadowKind::Decal);
        assert_eq!(house.decals[0].kind, DecalKind::Groundplane);
        let gold = object_shadow_as(&types, &project, &[3500], ShadowKind::None);
        assert_eq!(gold.decals[0].kind, DecalKind::Special);
        assert_eq!(type_for_model(&types, &project, "I.RDO"), Some(2006));
        assert_eq!(type_for_model(&types, &project, "x.rdo"), None);
    }

    #[test]
    fn no_shadow_areas_use_an_exclusive_end_after_plus_one() {
        let areas = [
            Area {
                name: "noshadow01".into(),
                rect: [2, 3, 4, 5],
            },
            Area {
                name: "harbor".into(),
                rect: [0, 0, 100, 100],
            },
        ];
        assert!(in_no_shadow_area(&areas, (2, 3)));
        assert!(in_no_shadow_area(&areas, (4, 5)));
        assert!(!in_no_shadow_area(&areas, (5, 5)));
        assert!(!in_no_shadow_area(&areas, (1, 4)));
        assert!(!in_no_shadow_area(&areas, (50, 50)));
    }
}
