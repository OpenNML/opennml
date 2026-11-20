//! World file `.wld`: terrain parameters, areas, cameras, light, fog and placed objects,
//! see `docs/formats/wld.md`. Read like `World_ParseNextBlock` (`0x00441C50`) and
//! `Terrain_ParseTerrainBlock` (`0x0055ED80`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::FormatError;
use crate::config::scan_int;
use crate::prj::{optional, required, text_value};
use crate::text::{Node, parse_blocks};

/// `[TERRAIN]`. Missing keys (never in the shipped data) receive the values of
/// [`TerrainParams::default`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainParams {
    /// Tiles in x/z.
    pub dim_x: u32,
    pub dim_z: u32,
    /// Edge length of a tile in world units (128 in the shipped data).
    pub size_x: f32,
    pub size_z: f32,
    /// Direction **to** the terrain light (used normalized).
    pub light_position: [f32; 3],
    /// R G B 0-255.
    pub light_color: [i32; 3],
    pub light_intensity: i32,
    pub water_level: f32,
    pub water_use_texture: bool,
    pub water_base_texture: u32,
    pub water_refraction_texture: u32,
    pub water_texture_tiling: i32,
    pub water_max_depth: i32,
    pub water_min_alpha: i32,
    pub water_max_alpha: i32,
    pub path_normal: f32,
    /// Tiles per render block (16 in the shipped data).
    pub chunk_size: u32,
    /// Texels of the base texture per block `[x, y]`.
    pub texture_base_size: [u32; 2],
    /// Texels of the alpha maps per block `[x, y]`.
    pub texture_alpha_size: [u32; 2],
    /// Tiles per repetition of the ground texture.
    pub texture_tiling: u32,
}

impl Default for TerrainParams {
    fn default() -> Self {
        Self {
            dim_x: 0,
            dim_z: 0,
            size_x: 128.0,
            size_z: 128.0,
            light_position: [0.0, 1.0, 0.0],
            light_color: [255; 3],
            light_intensity: 100,
            water_level: 0.0,
            water_use_texture: false,
            water_base_texture: 0,
            water_refraction_texture: 0,
            water_texture_tiling: 1,
            water_max_depth: 1,
            water_min_alpha: 0,
            water_max_alpha: 255,
            path_normal: 0.0,
            chunk_size: 16,
            texture_base_size: [8, 8],
            texture_alpha_size: [32, 32],
            texture_tiling: 4,
        }
    }
}

impl TerrainParams {
    /// Map width in world units (`DIM_X * SIZE_X`).
    pub fn width(&self) -> f32 {
        self.dim_x as f32 * self.size_x
    }

    /// Map depth in world units (`DIM_Z * SIZE_Z`).
    pub fn depth(&self) -> f32 {
        self.dim_z as f32 * self.size_z
    }
}

/// `[AREA]`: rectangle in tile coordinates, inclusive.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    /// Lowercase (key for scripts).
    pub name: String,
    /// `x0 z0 x1 z1`.
    pub rect: [i32; 4],
}

/// `[CAMERA]`: named camera position for script sequences.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedCamera {
    pub name: String,
    pub from: [f32; 3],
    pub at: [f32; 3],
    pub roll: f32,
}

/// `[LIGHT]`: light of the objects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectLight {
    /// Direction **to** the light.
    pub position: [f32; 3],
    pub color: [i32; 3],
    pub intensity: i32,
}

/// `[SHADOW]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowParams {
    pub light_position: [f32; 3],
    pub intensity: i32,
}

/// `[FOG]`; `start`/`end` are missing in 2 files.
#[derive(Debug, Clone, PartialEq)]
pub struct FogParams {
    pub color: [i32; 3],
    /// Distance behind the camera target at which the fog begins.
    pub start: Option<f32>,
    /// Length of the transition.
    pub end: Option<f32>,
}

/// `[OBJECT]`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldObject {
    /// Object type (`OBJ_GUID` of the project file).
    pub guid: u32,
    /// World position of the object origin (y is already the final height).
    pub position: [f32; 3],
    /// `OBJ_SCALE`, `(1, 1, 1)` if not given.
    pub scale: [f32; 3],
    /// `OBJ_XROT/YROT/ZROT` in radians.
    pub rotation: [f32; 3],
    /// `OBJ_COLOR`: player slot 0-7, 8 = neutral.
    pub color: Option<i32>,
    /// `OBJ_NAME`, lowercase.
    pub name: Option<String>,
    /// `[CONFIG]`: configuration values of the object (`CONFIG_GUID` -> value), read like
    /// `Level_ParseObjectConfigBlock` (`0x00434ED0`), see [`parse_object_config`].
    pub config: BTreeMap<u32, i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub terrain: TerrainParams,
    pub areas: Vec<Area>,
    pub cameras: Vec<NamedCamera>,
    pub light: Option<ObjectLight>,
    pub shadow: Option<ShadowParams>,
    pub fog: Option<FogParams>,
    pub objects: Vec<WorldObject>,
}

/// Reads a `.wld`. Blocks may appear in any order; `[TERRAIN]` is mandatory
/// (the game creates the grid there). With several `[LIGHT]`/`[SHADOW]`/`[FOG]` the last one counts.
pub fn parse_world(text: &str) -> Result<World, FormatError> {
    let nodes = parse_blocks(text)?;
    let mut terrain = None;
    let mut world = World {
        terrain: TerrainParams::default(),
        areas: Vec::new(),
        cameras: Vec::new(),
        light: None,
        shadow: None,
        fog: None,
        objects: Vec::new(),
    };
    for node in &nodes {
        match node.name().to_ascii_uppercase().as_str() {
            "[TERRAIN]" => terrain = Some(parse_terrain(node)?),
            "[AREA]" => world.areas.push(Area {
                name: text_value(node, "AREA_NAME")?
                    .unwrap_or_default()
                    .to_lowercase(),
                rect: args(node, "AREA_RECT")?.unwrap_or([0; 4]),
            }),
            "[CAMERA]" => world.cameras.push(NamedCamera {
                name: text_value(node, "CAMERA_NAME")?.unwrap_or_default(),
                from: args(node, "CAMERA_FROM")?.unwrap_or([0.0; 3]),
                at: args(node, "CAMERA_AT")?.unwrap_or([0.0; 3]),
                roll: optional(node, "CAMERA_ROLL")?.unwrap_or(0.0),
            }),
            "[LIGHT]" => {
                world.light = Some(ObjectLight {
                    position: args(node, "LIGHT_POSITION")?.unwrap_or([0.0, 1.0, 0.0]),
                    color: args(node, "LIGHT_COLOR")?.unwrap_or([255; 3]),
                    intensity: optional(node, "LIGHT_INTENSITY")?.unwrap_or(100),
                })
            }
            "[SHADOW]" => {
                world.shadow = Some(ShadowParams {
                    light_position: args(node, "SHADOW_LIGHT_POSITION")?.unwrap_or([0.0, 1.0, 0.0]),
                    intensity: optional(node, "SHADOW_INTENSITY")?.unwrap_or(0),
                })
            }
            "[FOG]" => {
                world.fog = Some(FogParams {
                    color: args(node, "FOG_COLOR")?.unwrap_or([0; 3]),
                    start: optional(node, "FOG_START")?,
                    end: optional(node, "FOG_END")?,
                })
            }
            "[OBJECT]" => world.objects.push(parse_object(node)?),
            _ => {} // WORLDFILE and unknown entries
        }
    }
    world.terrain = terrain.ok_or_else(|| FormatError::Invalid("[TERRAIN] missing".into()))?;
    Ok(world)
}

/// The first `N` arguments of a subkey, if present.
fn args<T: std::str::FromStr, const N: usize>(
    node: &Node,
    key: &str,
) -> Result<Option<[T; N]>, FormatError> {
    node.child(key).map(|n| n.parse_args::<T, N>()).transpose()
}

/// Largest accepted edge length of the block textures (`TEXTURE_BASE_*SIZE`, `TEXTURE_ALPHA_*SIZE`).
///
/// Deviation from the original (Rust only, plausibility limit): the values determine the size of the
/// `RgbaImage` and alpha map allocations in `opennml-world`; without an upper limit a broken file could
/// request arbitrary amounts of memory. The shipped data uses 8 and 32.
const MAX_TERRAIN_TEXTURE_SIZE: u32 = 1024;

/// Largest accepted `CHUNK_SIZE`.
///
/// Deviation from the original (Rust only, plausibility limit): guarantees that the tile index
/// `a * C + b < C * C <= 65536` of a block fits in `u16` (see `opennml-world/src/chunks.rs`,
/// tile list). The shipped data always uses 16.
const MAX_CHUNK_SIZE: u32 = 256;

fn parse_terrain(node: &Node) -> Result<TerrainParams, FormatError> {
    let d = TerrainParams::default();
    let t = TerrainParams {
        dim_x: optional(node, "TERRAIN_DIM_X")?.unwrap_or(d.dim_x),
        dim_z: optional(node, "TERRAIN_DIM_Z")?.unwrap_or(d.dim_z),
        size_x: optional::<i32>(node, "TERRAIN_SIZE_X")?.map_or(d.size_x, |v| v as f32),
        size_z: optional::<i32>(node, "TERRAIN_SIZE_Z")?.map_or(d.size_z, |v| v as f32),
        light_position: args(node, "LIGHT_POSITION")?.unwrap_or(d.light_position),
        light_color: args(node, "LIGHT_COLOR")?.unwrap_or(d.light_color),
        light_intensity: optional(node, "LIGHT_INTENSITY")?.unwrap_or(d.light_intensity),
        water_level: optional(node, "WATER_LEVEL")?.unwrap_or(d.water_level),
        water_use_texture: optional::<i32>(node, "WATER_USETEXTURE")?
            .map_or(d.water_use_texture, |v| v != 0),
        water_base_texture: optional(node, "WATER_BASE_TEXTUREGUID")?
            .unwrap_or(d.water_base_texture),
        water_refraction_texture: optional(node, "WATER_REFRACTION_TEXTUREGUID")?
            .unwrap_or(d.water_refraction_texture),
        water_texture_tiling: optional(node, "WATER_TEXTURETILING")?
            .unwrap_or(d.water_texture_tiling),
        water_max_depth: optional(node, "WATER_MAXDEPTH")?.unwrap_or(d.water_max_depth),
        water_min_alpha: optional(node, "WATER_MINALPHA")?.unwrap_or(d.water_min_alpha),
        water_max_alpha: optional(node, "WATER_MAXALPHA")?.unwrap_or(d.water_max_alpha),
        path_normal: optional(node, "PATH_NORMAL")?.unwrap_or(d.path_normal),
        chunk_size: optional(node, "CHUNK_SIZE")?.unwrap_or(d.chunk_size),
        texture_base_size: [
            optional(node, "TEXTURE_BASE_XSIZE")?.unwrap_or(d.texture_base_size[0]),
            optional(node, "TEXTURE_BASE_YSIZE")?.unwrap_or(d.texture_base_size[1]),
        ],
        texture_alpha_size: [
            optional(node, "TEXTURE_ALPHA_XSIZE")?.unwrap_or(d.texture_alpha_size[0]),
            optional(node, "TEXTURE_ALPHA_YSIZE")?.unwrap_or(d.texture_alpha_size[1]),
        ],
        texture_tiling: optional(node, "TEXTURE_TILING")?.unwrap_or(d.texture_tiling),
    };
    let invalid = |m: String| Err(FormatError::Invalid(format!("[TERRAIN]: {m}")));
    if !(1..=4096).contains(&t.dim_x) || !(1..=4096).contains(&t.dim_z) {
        return invalid(format!("grid size {} x {} implausible", t.dim_x, t.dim_z));
    }
    if t.size_x <= 0.0 || t.size_z <= 0.0 {
        return invalid("tile size must be positive".into());
    }
    if t.chunk_size == 0
        || t.chunk_size > MAX_CHUNK_SIZE
        || !t.dim_x.is_multiple_of(t.chunk_size)
        || !t.dim_z.is_multiple_of(t.chunk_size)
    {
        return invalid(format!(
            "CHUNK_SIZE {} is implausible (1 to {MAX_CHUNK_SIZE}) or does not divide {} x {}",
            t.chunk_size, t.dim_x, t.dim_z
        ));
    }
    if t.texture_base_size.contains(&0)
        || t.texture_alpha_size.contains(&0)
        || t.texture_tiling == 0
    {
        return invalid("texture sizes and TEXTURE_TILING must be positive".into());
    }
    if t.texture_base_size
        .iter()
        .chain(&t.texture_alpha_size)
        .any(|&v| v > MAX_TERRAIN_TEXTURE_SIZE)
    {
        return invalid(format!(
            "texture sizes {:?} / {:?} above {MAX_TERRAIN_TEXTURE_SIZE} implausible",
            t.texture_base_size, t.texture_alpha_size
        ));
    }
    Ok(t)
}

fn parse_object(node: &Node) -> Result<WorldObject, FormatError> {
    Ok(WorldObject {
        guid: required(node, "OBJ_GUID")?,
        position: match args(node, "OBJ_POSITION")? {
            Some(p) => p,
            None => {
                return Err(crate::error::syntax(
                    node.line,
                    "[OBJECT]: OBJ_POSITION missing",
                ));
            }
        },
        scale: args(node, "OBJ_SCALE")?.unwrap_or([1.0; 3]),
        rotation: [
            optional(node, "OBJ_XROT")?.unwrap_or(0.0),
            optional(node, "OBJ_YROT")?.unwrap_or(0.0),
            optional(node, "OBJ_ZROT")?.unwrap_or(0.0),
        ],
        color: optional(node, "OBJ_COLOR")?,
        name: text_value(node, "OBJ_NAME")?.map(|n| n.to_lowercase()),
        config: node
            .child("[CONFIG]")
            .map_or_else(BTreeMap::new, parse_object_config),
    })
}

/// `[CONFIG]` of a `.wld` object like `Level_ParseObjectConfigBlock` (`0x00434ED0`): for each
/// `CONFIG_GUID`, `CONFIG_ENABLE` applies (active unless the value is `false`, case-insensitive,
/// first 5 characters only); a value is accepted only if it is active, a number (`%d`) and
/// greater than 0. The switch is not reset per `CONFIG_GUID` (initial value: off).
pub fn parse_object_config(block: &Node) -> BTreeMap<u32, i32> {
    let mut values = BTreeMap::new();
    let mut active = false;
    for entry in block.children_named("CONFIG_GUID") {
        let Ok(id) = entry.parse_arg::<u32>(0) else {
            continue;
        };
        for line in entry.children() {
            match line.name().to_ascii_uppercase().as_str() {
                "CONFIG_ENABLE" => {
                    let text = line.arg(0).map_or("", |v| v.as_str());
                    let head: String = text.chars().take(5).collect();
                    active = !head.eq_ignore_ascii_case("false");
                }
                "CONFIG_VALUE" => {
                    let value = line.arg(0).and_then(|v| scan_int(v.as_str()));
                    if let Some(v) = value.filter(|&v| active && v > 0) {
                        values.insert(id, v);
                    }
                }
                _ => {}
            }
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "WORLDFILE V1.08e\r\n\r\n[TERRAIN]\r\n{\r\n\tTERRAIN_DIM_X 32\r\n\tTERRAIN_DIM_Z 32\r\n\tTERRAIN_SIZE_X 128\r\n\tTERRAIN_SIZE_Z 128\r\n\tLIGHT_POSITION 800.000000 400.000000 -1300.000000\r\n\tLIGHT_COLOR 200 200 200\r\n\tLIGHT_INTENSITY 20\r\n\tWATER_LEVEL -10.000000\r\n\tWATER_USETEXTURE 1\r\n\tWATER_BASE_TEXTUREGUID 34\r\n\tWATER_REFRACTION_TEXTUREGUID 35\r\n\tWATER_TEXTURETILING 8\r\n\tWATER_MAXDEPTH 600\r\n\tWATER_MINALPHA 60\r\n\tWATER_MAXALPHA 150\r\n\tPATH_NORMAL 0.700000\r\n\tBUILD_AREA_NORMAL 0.000000\r\n\tCHUNK_SIZE 16\r\n\tTEXTURE_BASE_XSIZE 8\r\n\tTEXTURE_BASE_YSIZE 8\r\n\tTEXTURE_ALPHA_XSIZE 32\r\n\tTEXTURE_ALPHA_YSIZE 32\r\n\tTEXTURE_TILING 4\r\n}\r\n[AREA]\r\n{\r\n\tAREA_NAME \"Harbor Intro\"\r\n\tAREA_RECT 82 33 84 35\r\n}\r\n[CAMERA]\r\n{\r\n\tCAMERA_NAME \"Start Camera\"\r\n\tCAMERA_FROM -10169.000000 1174.760010 1046.119995\r\n\tCAMERA_AT 6140.220215 221.328003 1515.900024\r\n\tCAMERA_ROLL 0.000000\r\n}\r\n[LIGHT]\r\n{\r\n\tLIGHT_POSITION 800.000000 400.000000 -1300.000000\r\n\tLIGHT_COLOR 255 241 193\r\n\tLIGHT_INTENSITY 40\r\n}\r\n[SHADOW]\r\n{\r\n\tSHADOW_LIGHT_POSITION 400.000000 1000.000000 -500.000000\r\n\tSHADOW_INTENSITY 70\r\n}\r\n[FOG]\r\n{\r\n\tFOG_COLOR 255 230 241\r\n\tFOG_START -8000.000000\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 3639\r\n\tOBJ_POSITION 3776.000000 50.000000 1600.000000\r\n\tOBJ_XROT 0.000000\r\n\tOBJ_YROT 5.170056\r\n\tOBJ_ZROT 0.000000\r\n\tOBJ_COLOR 8\r\n\tOBJ_STAND_ON 0\r\n\t[CONFIG]\r\n\t{\r\n\t\tCONFIG_GUID 26\r\n\t\t{\r\n\t\t\tCONFIG_VALUE\t\"7000\"\r\n\t\t}\r\n\t}\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 4200\r\n\tOBJ_POSITION 1.0 2.0 3.0\r\n\tOBJ_NAME \"Start Red\"\r\n}\r\n";

    #[test]
    fn parses_all_blocks() {
        let w = parse_world(SAMPLE).unwrap();
        let t = &w.terrain;
        assert_eq!((t.dim_x, t.dim_z, t.size_x), (32, 32, 128.0));
        assert_eq!(t.light_color, [200, 200, 200]);
        assert_eq!(
            (t.water_level, t.water_min_alpha, t.water_max_alpha),
            (-10.0, 60, 150)
        );
        assert!(t.water_use_texture);
        assert_eq!((t.water_base_texture, t.water_refraction_texture), (34, 35));
        assert_eq!(
            (t.texture_base_size, t.texture_alpha_size),
            ([8, 8], [32, 32])
        );
        assert_eq!(t.width(), 4096.0);
        assert_eq!(
            w.areas,
            [Area {
                name: "harbor intro".into(),
                rect: [82, 33, 84, 35]
            }]
        );
        assert_eq!(w.cameras[0].name, "Start Camera");
        assert_eq!(w.light.as_ref().unwrap().color, [255, 241, 193]);
        assert_eq!(w.shadow.as_ref().unwrap().intensity, 70);
        let fog = w.fog.as_ref().unwrap();
        assert_eq!((fog.start, fog.end), (Some(-8000.0), None));
        assert_eq!(w.objects.len(), 2);
        let palme = &w.objects[0];
        assert_eq!(palme.guid, 3639);
        assert_eq!(palme.rotation, [0.0, 5.170056, 0.0]);
        assert_eq!((palme.color, palme.scale), (Some(8), [1.0; 3]));
        assert_eq!(w.objects[1].name.as_deref(), Some("start red"));
        assert_eq!(w.objects[1].color, None);
        // Without CONFIG_ENABLE the switch is off: the value 7000 of the palm is dropped.
        assert!(w.objects[0].config.is_empty() && w.objects[1].config.is_empty());
    }

    #[test]
    fn object_config_takes_only_enabled_positive_numbers() {
        let text = "[OBJECT]\n{\nOBJ_GUID 1\nOBJ_POSITION 0 0 0\n[CONFIG]\n{\nCONFIG_GUID 24\n{\nCONFIG_ENABLE TRUE\nCONFIG_VALUE \"500\"\n}\nCONFIG_GUID 25\n{\nCONFIG_VALUE \"9\"\n}\nCONFIG_GUID 26\n{\nCONFIG_ENABLE FALSE\nCONFIG_VALUE \"9\"\n}\nCONFIG_GUID 14\n{\nCONFIG_VALUE \"7\"\n}\nCONFIG_GUID 18\n{\nCONFIG_ENABLE TRUE\nCONFIG_VALUE \"Medium\"\n}\nCONFIG_GUID 17\n{\nCONFIG_ENABLE falseX\nCONFIG_VALUE \"0\"\n}\nCONFIG_GUID 13\n{\nCONFIG_ENABLE no\nCONFIG_VALUE \" -4\"\n}\n}\n}\n";
        let w = parse_world(&format!("{}{text}", SAMPLE.split("[AREA]").next().unwrap())).unwrap();
        // 25 inherits "active" from 24, 14 inherits "off" from 26; text and values <= 0 are dropped.
        assert_eq!(w.objects[0].config, BTreeMap::from([(24, 500), (25, 9)]));
    }

    #[test]
    fn terrain_is_required_and_checked() {
        assert!(matches!(
            parse_world("WORLDFILE V1.08e\n"),
            Err(FormatError::Invalid(_))
        ));
        let uneven = SAMPLE.replace("TERRAIN_DIM_X 32", "TERRAIN_DIM_X 40");
        assert!(matches!(parse_world(&uneven), Err(FormatError::Invalid(_))));
        // Tile size <= 0: the simulation divides by it (`MapInfo::cell`).
        let zero = SAMPLE.replace("TERRAIN_SIZE_X 128", "TERRAIN_SIZE_X 0");
        assert!(matches!(
            parse_world(&zero),
            Err(FormatError::Invalid(m)) if m.contains("tile size must be positive")
        ));
        let negative = SAMPLE.replace("TERRAIN_SIZE_Z 128", "TERRAIN_SIZE_Z -1");
        assert!(matches!(
            parse_world(&negative),
            Err(FormatError::Invalid(m)) if m.contains("tile size must be positive")
        ));

        // Upper limits: each case changes only the checked value, the message shows the cause.
        let base_1025 = SAMPLE.replace("TEXTURE_BASE_XSIZE 8", "TEXTURE_BASE_XSIZE 1025");
        assert!(matches!(
            parse_world(&base_1025),
            Err(FormatError::Invalid(m)) if m.contains("above 1024")
        ));
        let alpha_1025 = SAMPLE.replace("TEXTURE_ALPHA_XSIZE 32", "TEXTURE_ALPHA_XSIZE 1025");
        assert!(matches!(
            parse_world(&alpha_1025),
            Err(FormatError::Invalid(m)) if m.contains("above 1024")
        ));
        let base_100000 = SAMPLE.replace("TEXTURE_BASE_YSIZE 8", "TEXTURE_BASE_YSIZE 100000");
        assert!(matches!(
            parse_world(&base_100000),
            Err(FormatError::Invalid(m)) if m.contains("above 1024")
        ));
        let chunk_512 = SAMPLE
            .replace("TERRAIN_DIM_X 32", "TERRAIN_DIM_X 512")
            .replace("TERRAIN_DIM_Z 32", "TERRAIN_DIM_Z 512")
            .replace("CHUNK_SIZE 16", "CHUNK_SIZE 512");
        assert!(matches!(
            parse_world(&chunk_512),
            Err(FormatError::Invalid(m)) if m.contains("CHUNK_SIZE 512")
        ));

        let tex_edge = SAMPLE
            .replace("TEXTURE_BASE_XSIZE 8", "TEXTURE_BASE_XSIZE 1024")
            .replace("TEXTURE_ALPHA_YSIZE 32", "TEXTURE_ALPHA_YSIZE 1024");
        let w = parse_world(&tex_edge).unwrap();
        assert_eq!(
            (w.terrain.texture_base_size, w.terrain.texture_alpha_size),
            ([1024, 8], [32, 1024])
        );
        let chunk_edge = SAMPLE
            .replace("TERRAIN_DIM_X 32", "TERRAIN_DIM_X 256")
            .replace("TERRAIN_DIM_Z 32", "TERRAIN_DIM_Z 256")
            .replace("CHUNK_SIZE 16", "CHUNK_SIZE 256");
        let w = parse_world(&chunk_edge).unwrap();
        assert_eq!(
            (w.terrain.dim_x, w.terrain.dim_z, w.terrain.chunk_size),
            (256, 256, 256)
        );

        let no_guid = SAMPLE.replace("\tOBJ_GUID 4200\r\n", "");
        assert!(matches!(
            parse_world(&no_guid),
            Err(FormatError::Syntax { .. })
        ));
    }
}
