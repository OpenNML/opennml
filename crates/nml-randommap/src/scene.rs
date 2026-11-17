//! Scene of the random map without a level file: grid, tile GUIDs and light from the generator, built
//! through the same scene construction as fixed maps (`nml_world::level::build_scene`).
//! `Terrain::new` computes normals, vertex light and water alpha from it (steps 11 and 15).

use nml_formats::hlm::{Heightmap, HlmPoint};
use nml_formats::lvl::LevelFile;
use nml_formats::tlm::TextureMap;
use nml_formats::wld::{World, WorldObject};
use nml_rda::Vfs;
use nml_world::environment::RANDOM_MAP_PREFIX;
use nml_world::level::{LevelScene, LevelSource, build_scene};

use crate::{RandomMap, RandomMapSpec, read_project};

/// Name of the scene: `randommap:<seed text>`. Does not end in `.lvl` and therefore matches no
/// level environment table.
pub fn random_map_label(spec: &RandomMapSpec) -> String {
    format!("{RANDOM_MAP_PREFIX}{}", spec.seed_text)
}

/// Builds the scene of a generated map. Grid points like `Terrain_AllocateGrid` (`0x0055C140`):
/// `(x·SIZE_X, height, z·SIZE_Z)`, normal `(0, 1, 0)`, without color GUID. The world objects are the
/// placed objects of the generator in placement order (`FUN_005474B0`): position with y = 0 (only the
/// world sets the height when snapping onto the terrain; a preview without simulation raises
/// them itself), rotation only about the vertical axis, scale 1, owner as color, without name and
/// configuration. The scene construction turns `TOWN_CENTER` (4200) into the start points per owner. No
/// areas, cameras or fog; object and shadow light from `ObjectLight`/`ShadowLight` of the script (if a
/// block is missing: `None`, D-345). The project is read again via `ProjectFile`.
pub fn to_scene(vfs: &Vfs, map: &RandomMap) -> anyhow::Result<LevelScene> {
    let project = read_project(&mut |path| vfs.read(path).ok(), &map.script)?;
    let terrain = &map.terrain;
    let verts = map.dim + 1;
    let mut points = Vec::with_capacity(verts * verts);
    for z in 0..verts {
        for x in 0..verts {
            points.push(HlmPoint {
                position: [
                    x as f32 * terrain.size_x,
                    map.heights[verts * z + x],
                    z as f32 * terrain.size_z,
                ],
                normal: [0.0, 1.0, 0.0],
                color_guid: None,
            });
        }
    }
    let source = LevelSource {
        lvl_path: random_map_label(&map.spec),
        level: LevelFile {
            project_file: map.script.project_file.clone().unwrap_or_default(),
            players: Vec::new(),
        },
        project,
        world: World {
            terrain: terrain.clone(),
            areas: Vec::new(),
            cameras: Vec::new(),
            light: map.script.object_light.clone(),
            shadow: map.script.shadow.clone(),
            fog: None,
            objects: map
                .objects
                .iter()
                .map(|o| WorldObject {
                    guid: o.guid,
                    position: o.position,
                    scale: [1.0; 3],
                    rotation: [0.0, o.rotation_y, 0.0],
                    color: Some(o.owner),
                    name: None,
                    config: Default::default(),
                })
                .collect(),
        },
        heightmap: Heightmap {
            verts_x: verts,
            verts_z: verts,
            points,
        },
        texture_map: TextureMap {
            dim_x: map.dim,
            dim_z: map.dim,
            guids: map.tiles.clone(),
        },
    };
    build_scene(vfs, source)
}
