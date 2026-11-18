//! Assembling a level (`Game_StartLevel` `0x004C8590`, order in `docs/formats/lvl.md`):
//! `.lvl` -> project file -> `.wld` -> `.hlm`/`.tlm` -> place objects.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, anyhow};
use glam::{Affine3A, Vec3};
use image::RgbaImage;
use opennml_formats::hlm::{Heightmap, parse_hlm};
use opennml_formats::lvl::{LevelFile, parse_level};
use opennml_formats::prj::{Project, parse_project};
use opennml_formats::text::decode_cp1252;
use opennml_formats::tlm::{TextureMap, parse_tlm};
use opennml_formats::wld::{ObjectLight, World, parse_world};
use opennml_rda::{Vfs, normalize_path};

use crate::assets::load_texture;
use crate::classes::{ObjectTypes, in_no_shadow_area};
use crate::math::object_world_matrix;
use crate::terrain::Terrain;

/// `OBJ_GUID` of the start points: set the start position of the player `OBJ_COLOR`, no object.
pub const START_POINT_GUID: u32 = 4200;
/// Ambient effects (`World_PlaceObject` `0x00440720`): surf 4104-4106, fish 3411, eagle 3408.
pub const AMBIENT_GUIDS: [u32; 5] = [4104, 4105, 4106, 3411, 3408];

/// Kind of a placed object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectClass {
    /// Game object (`GameWorld_CreateObject`).
    Game,
    /// Ambient effect.
    Ambient,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedObject {
    /// Index in `World::objects`.
    pub index: usize,
    pub guid: u32,
    pub class: ObjectClass,
    /// `OBJ_NAME` of the type in the project file.
    pub type_name: String,
    /// `OBJ_NAME` of the object in the `.wld` (script name).
    pub name: Option<String>,
    /// Normalized model path according to the rule of the project file.
    pub model: String,
    /// Player slot (`OBJ_COLOR`), 8 = neutral.
    pub player: Option<i32>,
    /// Types that the game creates for the object (`ObjectTypes::parts`: rider -> rider and
    /// horse); empty if the type is missing from `property.ini` (then without shadow).
    pub parts: Vec<u32>,
    /// The tile of the object lies in a `noshadow...` area of the `.wld`.
    pub no_shadow: bool,
    pub position: Vec3,
    /// Rotation X/Y/Z in radians.
    pub rotation: Vec3,
    /// World matrix in the original coordinate system.
    pub transform: Affine3A,
    /// ID of the simulation object (`opennml_sim::id::ObjectId`) if the map comes from a
    /// simulation.
    pub sim_id: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct LevelScene {
    /// Normalized path of the `.lvl`; for a random map `randommap:<seed>`.
    pub lvl_path: String,
    pub level: LevelFile,
    pub world: World,
    pub project: Project,
    /// Object types (`property.ini`, `translate.ini`).
    pub types: ObjectTypes,
    pub terrain: Terrain,
    /// Objects in `.wld` order (without start points, unknown types, objects outside the
    /// map and types without an existing model).
    pub objects: Vec<PlacedObject>,
    /// Start point per player slot (the last one counts if there are several).
    pub start_points: BTreeMap<i32, Vec3>,
    /// Current object light (light 0): initially `[LIGHT]` of the `.wld`, after
    /// [`LevelScene::apply_light`] that of the preset.
    pub object_light: Option<ObjectLight>,
}

/// All `.lvl` files in the archives (normalized, sorted).
pub fn level_paths(vfs: &Vfs) -> Vec<String> {
    vfs.files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.ends_with(".lvl"))
        .collect()
}

fn read_text(vfs: &Vfs, path: &str) -> anyhow::Result<String> {
    Ok(decode_cp1252(
        &vfs.read(path).with_context(|| format!("reading {path}"))?,
    ))
}

/// The files read for a level before the scene is built. A random map (`opennml-randommap`)
/// fills the same parts from the generator and builds the scene via [`build_scene`].
#[derive(Debug, Clone)]
pub struct LevelSource {
    /// Name of the level in the scene: normalized path of the `.lvl` (or `randommap:<seed>`).
    pub lvl_path: String,
    pub level: LevelFile,
    pub project: Project,
    pub world: World,
    pub heightmap: Heightmap,
    pub texture_map: TextureMap,
}

/// Loads a level with terrain and placed objects: [`read_level_files`], then
/// [`build_scene`].
pub fn load_level(vfs: &Vfs, lvl_path: &str) -> anyhow::Result<LevelScene> {
    build_scene(vfs, read_level_files(vfs, lvl_path)?)
}

/// Reads `.lvl`, project file, `.wld`, `.hlm` and `.tlm` of a level (reading part of
/// [`load_level`]); only here the extension `.lvl` is required.
pub fn read_level_files(vfs: &Vfs, lvl_path: &str) -> anyhow::Result<LevelSource> {
    let lvl_path = normalize_path(lvl_path);
    let base = lvl_path
        .strip_suffix(".lvl")
        .ok_or_else(|| anyhow!("{lvl_path}: not a .lvl file"))?;
    let level =
        parse_level(&read_text(vfs, &lvl_path)?).with_context(|| format!("parsing {lvl_path}"))?;
    let project_path = normalize_path(&level.project_file);
    let project = parse_project(&read_text(vfs, &project_path)?)
        .with_context(|| format!("parsing {project_path}"))?;
    let wld = format!("{base}.wld");
    let world = parse_world(&read_text(vfs, &wld)?).with_context(|| format!("parsing {wld}"))?;
    let (dim_x, dim_z) = (world.terrain.dim_x, world.terrain.dim_z);
    let hlm = format!("{base}.hlm");
    let heightmap = parse_hlm(
        &vfs.read(&hlm).with_context(|| format!("reading {hlm}"))?,
        dim_x,
        dim_z,
    )
    .with_context(|| format!("parsing {hlm}"))?;
    let tlm = format!("{base}.tlm");
    let texture_map = parse_tlm(
        &vfs.read(&tlm).with_context(|| format!("reading {tlm}"))?,
        dim_x,
        dim_z,
    )
    .with_context(|| format!("parsing {tlm}"))?;
    Ok(LevelSource {
        lvl_path,
        level,
        project,
        world,
        heightmap,
        texture_map,
    })
}

/// Builds the scene from the parts that were read (building part of [`load_level`]): terrain, object types,
/// placed objects and start points. Does not check an extension of the name.
pub fn build_scene(vfs: &Vfs, source: LevelSource) -> anyhow::Result<LevelScene> {
    let LevelSource {
        lvl_path,
        level,
        project,
        world,
        heightmap,
        texture_map,
    } = source;
    let terrain = Terrain::new(&world.terrain, &heightmap, &texture_map, &project)?;
    let types = ObjectTypes::load(vfs);
    let (objects, start_points) = place_objects(vfs, &world, &project, &types, &terrain);
    Ok(LevelScene {
        object_light: world.light.clone(),
        lvl_path,
        level,
        world,
        project,
        types,
        terrain,
        objects,
        start_points,
    })
}

/// Objects like `World_PlaceObject` (`0x00440720`) / `GameWorld_CreateObject` (`0x00485F70`).
fn place_objects(
    vfs: &Vfs,
    world: &World,
    project: &Project,
    types: &ObjectTypes,
    terrain: &Terrain,
) -> (Vec<PlacedObject>, BTreeMap<i32, Vec3>) {
    let mut objects = Vec::new();
    let mut start_points = BTreeMap::new();
    let mut without_type = BTreeSet::new();
    let (width, depth) = (terrain.params.width(), terrain.params.depth());
    for (index, object) in world.objects.iter().enumerate() {
        let position = Vec3::from(object.position);
        if object.guid == START_POINT_GUID {
            if let Some(color) = object.color {
                start_points.insert(color, position);
            }
            continue;
        }
        let Some(kind) = project.objects.get(&object.guid) else {
            log::warn!("Object {index}: unknown type {}, skipped", object.guid);
            continue;
        };
        let class = if AMBIENT_GUIDS.contains(&object.guid) {
            ObjectClass::Ambient
        } else {
            ObjectClass::Game
        };
        // `0x004862DC`...`0x00486336`: game objects outside the map are silently dropped.
        let inside = (0.0..=width).contains(&position.x) && (0.0..=depth).contains(&position.z);
        if class == ObjectClass::Game && !inside {
            log::warn!(
                "Object {index} (type {}) lies outside the map, skipped",
                object.guid
            );
            continue;
        }
        let model = normalize_path(&kind.model);
        if !vfs.exists(&model) {
            log::warn!("Object {index}: model {model} missing, skipped");
            continue;
        }
        let parts = types.parts(object.guid);
        if class == ObjectClass::Game && parts.is_empty() {
            without_type.insert(object.guid);
        }
        let p = &terrain.params;
        // `GameObject+0x328/+0x32C` (`GameObject_ApplyPlacement` `0x00473340`).
        let tile = (
            (position.x / p.size_x).floor() as i64,
            (position.z / p.size_z).floor() as i64,
        );
        objects.push(PlacedObject {
            index,
            guid: object.guid,
            class,
            type_name: kind.name.clone(),
            name: object.name.clone(),
            model,
            player: object.color,
            parts,
            no_shadow: in_no_shadow_area(&world.areas, tile),
            position,
            rotation: Vec3::from(object.rotation),
            transform: object_world_matrix(
                position,
                Vec3::from(object.rotation),
                Vec3::from(object.scale),
            ),
            sim_id: None,
        });
    }
    for guid in without_type {
        // `GameWorld_CreateObject` does not create such types; the viewer draws them without
        // shadow (and without effects).
        log::info!("Type {guid} missing in property.ini: drawn without shadow");
    }
    (objects, start_points)
}

impl LevelScene {
    /// Ground textures of the tiles and the water by `TEX_GUID` (decoded top first); ones that cannot
    /// be loaded are logged and left out.
    pub fn ground_textures(&self, vfs: &Vfs) -> HashMap<u32, RgbaImage> {
        let mut guids: Vec<u32> = self.terrain.tiles.iter().flatten().copied().collect();
        let t = &self.world.terrain;
        guids.extend([t.water_base_texture, t.water_refraction_texture]);
        guids.sort_unstable();
        guids.dedup();
        let mut images = HashMap::new();
        for guid in guids {
            let Some(texture) = self.project.textures.get(&guid) else {
                continue;
            };
            match load_texture(vfs, &texture.file) {
                Ok(img) => {
                    images.insert(guid, img);
                }
                Err(e) => log::warn!("Ground texture {guid}: {e:#}"),
            }
        }
        images
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::tests::{game_dir, tga_2x1};
    use opennml_formats::rdo::RdoLayout;
    use opennml_formats::testutil::{flat_heightmap, sample_rdo, write_hlm, write_rdo, write_tlm};
    use opennml_formats::tlm::TextureMap;

    const LVL: &str = "LEVELFILE V1.08e\r\nPROJECT_FILE \"Data\\Projects\\test.prj\"\r\n[PLAYER]\r\n{\r\n\tPLAYER_NAME \"29081 RED\"\r\n\tPLAYER_PLAYER \"SPA\"\r\n\tPLAYER_TYPE 0\r\n\tPLAYER_COLOR 0\r\n}\r\n";
    const PRJ: &str = "PROJECTFILE V1.08f\r\n[OBJECT]\r\n{\r\n\tOBJ_NAME \"Kiste\"\r\n\tOBJ_GUID 10\r\n\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdw\"\r\n\tOBJ_GAME_FILENAME \"\"\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_NAME \"Missing\"\r\n\tOBJ_GUID 11\r\n\tOBJ_FILENAME \"data\\r3d\\test\\missing.rdo\"\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_NAME \"Fish\"\r\n\tOBJ_GUID 3411\r\n\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n}\r\n[TEXTURE]\r\n{\r\n\tTEX_NAME \"Grass\"\r\n\tTEX_GUID 5\r\n\tTEX_FILENAME \"data\\textures\\grass.tga\"\r\n\tTEX_GROUP_ID 1\r\n}\r\n[COLOR]\r\n{\r\n\tCOLOR_NAME \"Red\"\r\n\tCOLOR_GUID 3\r\n\tCOLOR_DIFFUSE 255\r\n}\r\n";
    const WLD: &str = "WORLDFILE V1.08e\r\n[TERRAIN]\r\n{\r\n\tTERRAIN_DIM_X 2\r\n\tTERRAIN_DIM_Z 2\r\n\tTERRAIN_SIZE_X 128\r\n\tTERRAIN_SIZE_Z 128\r\n\tLIGHT_POSITION 0 1 0\r\n\tLIGHT_COLOR 255 255 255\r\n\tLIGHT_INTENSITY 100\r\n\tCHUNK_SIZE 2\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 10\r\n\tOBJ_POSITION 64 0 64\r\n\tOBJ_YROT 1.5\r\n\tOBJ_COLOR 0\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 999\r\n\tOBJ_POSITION 64 0 64\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 10\r\n\tOBJ_POSITION 500 0 64\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 4200\r\n\tOBJ_POSITION 100 0 100\r\n\tOBJ_COLOR 0\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 11\r\n\tOBJ_POSITION 10 0 10\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 3411\r\n\tOBJ_POSITION 900 0 900\r\n}\r\n";

    fn test_level_dir() -> tempfile::TempDir {
        let mut heightmap = flat_heightmap(2, 2, 128.0, 0.0);
        heightmap.points[0].color_guid = Some(3);
        let tlm = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![5, 5, 5, 77],
        };
        game_dir(&[
            ("data/levels/test/test.lvl", LVL.as_bytes().to_vec()),
            ("data/projects/test.prj", PRJ.as_bytes().to_vec()),
            ("data/levels/test/test.wld", WLD.as_bytes().to_vec()),
            ("data/levels/test/test.hlm", write_hlm(&heightmap)),
            ("data/levels/test/test.tlm", write_tlm(&tlm)),
            (
                "data/r3d/test/kiste.rdo",
                write_rdo(&sample_rdo(RdoLayout::Standard)),
            ),
            ("data/textures/grass.tga", tga_2x1()),
        ])
    }

    #[test]
    fn load_level_skips_unknown_and_outside_objects() {
        let dir = test_level_dir();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        assert_eq!(level_paths(&vfs), ["data/levels/test/test.lvl"]);
        let scene = load_level(&vfs, r"Data\Levels\Test\Test.lvl").unwrap();
        assert_eq!(scene.level.players[0].color, 0);
        // Kiste (game object) and fish (ambient effect, without a map check); dropped:
        // unknown type 999, Kiste outside (x = 500 > 256), type 11 without model, start point.
        let guids: Vec<(u32, ObjectClass)> =
            scene.objects.iter().map(|o| (o.guid, o.class)).collect();
        assert_eq!(
            guids,
            [(10, ObjectClass::Game), (3411, ObjectClass::Ambient)]
        );
        let kiste = &scene.objects[0];
        assert_eq!(kiste.model, "data/r3d/test/kiste.rdo");
        assert_eq!(
            (kiste.type_name.as_str(), kiste.player, kiste.index),
            ("Kiste", Some(0), 0)
        );
        let forward = kiste.transform.transform_vector3(-Vec3::Z);
        assert!((forward - Vec3::new(1.5f32.sin(), 0.0, -1.5f32.cos())).length() < 1e-5);
        assert_eq!(scene.start_points[&0], Vec3::new(100.0, 0.0, 100.0));
        // Without property.ini in the archive: no types, no parts (objects stay).
        assert!(scene.types.types.is_empty() && kiste.parts.is_empty());
        assert!(!kiste.no_shadow);
        // Terrain: color 3 = red at grid point (0, 0); tile with GUID 77 without texture.
        assert_eq!(scene.terrain.vertex(0, 0).lit_color, [255, 0, 0]);
        assert_eq!(scene.terrain.vertex(1, 0).lit_color, [255, 255, 255]);
        assert_eq!(scene.terrain.tile(1, 1), None);
        let textures = scene.ground_textures(&vfs);
        assert_eq!(textures.keys().copied().collect::<Vec<_>>(), [5]);
    }

    #[test]
    fn read_and_build_equal_load_level() {
        let dir = test_level_dir();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let path = r"Data\Levels\Test\Test.lvl";
        let source = read_level_files(&vfs, path).unwrap();
        assert_eq!(source.lvl_path, "data/levels/test/test.lvl");
        assert_eq!(source.level.project_file, r"Data\Projects\test.prj");
        assert_eq!((source.texture_map.dim_x, source.heightmap.verts_x), (2, 3));
        let built = build_scene(&vfs, source).unwrap();
        let loaded = load_level(&vfs, path).unwrap();
        // `LevelScene` has no `PartialEq`: comparison via the debug output (all fields).
        assert_eq!(format!("{built:?}"), format!("{loaded:?}"));
        assert_eq!(built.objects.len(), 2);
        // Building the scene checks no extension: a generated map has a different name.
        let mut source = read_level_files(&vfs, path).unwrap();
        source.lvl_path = "randommap:ZKA".to_string();
        assert_eq!(build_scene(&vfs, source).unwrap().lvl_path, "randommap:ZKA");
    }

    #[test]
    fn load_level_reports_missing_files() {
        let dir = game_dir(&[("data/levels/x/x.lvl", LVL.as_bytes().to_vec())]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let error = load_level(&vfs, "data/levels/x/x.lvl").unwrap_err();
        assert!(format!("{error:#}").contains("test.prj"), "{error:#}");
        assert!(load_level(&vfs, "data/levels/x/x.wld").is_err());
    }
}
