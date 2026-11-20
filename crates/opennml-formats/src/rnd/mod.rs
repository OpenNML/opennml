//! Map scripts of the random map (`data/randommapscripts/*.rnd`, `*.inc`): line reader with
//! preprocessor (`FUN_005424B0` `0x005424B0`), the `sscanf` reimplementations of the MSVC runtime, the
//! dispatcher (`FUN_00545C10` `0x00545C10`) and the records it creates.
//! See re/random-map Z.5.

pub mod blocks;
pub mod reader;
pub mod scan;

pub use blocks::parse_random_map_script;
pub use reader::{ScriptReader, include_path};

use serde::{Deserialize, Serialize};

use crate::wld::{ObjectLight, ShadowParams, TerrainParams};

/// `#Texture NAME { ... }` (`0x28` bytes, `FUN_005422B0`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureDef {
    /// Texture number: counter `gen+0x10C`, 1, 2, ... in definition order.
    pub number: i32,
    pub name: String,
    pub height: f32,
    /// `GUID` lines; duplicates stay (they weight the draw).
    pub guids: Vec<u32>,
}

/// `#Object NAME { ... }` (`0x20` bytes, `FUN_00541610`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectDef {
    pub name: String,
    pub guids: Vec<u32>,
}

/// Area block `create_PlayerAreas`/`create_LandArea`/`create_TerrainArea` (`0x374` bytes,
/// `FUN_005406D0`, parser `FUN_00543CA0`), **raw**: borders in percent, amounts unscaled. The generator
/// computes borders and amounts (`FUN_0053FBF0`, `FUN_00540810`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AreaDef {
    pub id: i32,
    pub replace: String,
    pub texture: String,
    pub put_onto: String,
    pub min_size: i32,
    pub num_tiles: i32,
    pub percent: i32,
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
    pub border_roughness: i32,
    pub avoid_roughness: i32,
    /// Default 1.
    pub num_clumps: i32,
    pub avoid_other: i32,
    pub avoid_player_areas: bool,
    pub scale_to_players: bool,
    pub scale_to_map_size: bool,
    pub scale_clumps_to_map_size: bool,
    pub share_player_lands: bool,
}

impl Default for AreaDef {
    fn default() -> Self {
        AreaDef {
            id: 0,
            replace: String::new(),
            texture: String::new(),
            put_onto: String::new(),
            min_size: 0,
            num_tiles: 0,
            percent: 0,
            left: 0,
            right: 0,
            top: 0,
            bottom: 0,
            border_roughness: 0,
            avoid_roughness: 0,
            num_clumps: 1,
            avoid_other: 0,
            avoid_player_areas: false,
            scale_to_players: false,
            scale_to_map_size: false,
            scale_clumps_to_map_size: false,
            share_player_lands: false,
        }
    }
}

/// `connect_Players { ... }` (`0x204` bytes, `FUN_0053FA40`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ConnectDef {
    pub texture: String,
    pub put_onto: String,
    pub radius: i32,
}

/// `place_Object NAME { ... }` (`0x328` bytes, `FUN_0053F9A0`), raw: `number`/`numGroups` unscaled.
/// The reader only reads; the generator places the objects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceObjectDef {
    pub name: String,
    /// Default 1.
    pub number: i32,
    /// Default 1.
    pub num_groups: i32,
    pub max_group_radius: i32,
    pub min_group_distance: i32,
    pub for_all_players: bool,
    pub is_player: bool,
    pub put_on_flat: bool,
    pub min_dist_to_players: i32,
    pub max_dist_to_players: i32,
    pub avoid_other: i32,
    pub put_onto: String,
    pub fill: bool,
    pub scale_to_players: bool,
    pub scale_to_map_size: bool,
    pub scale_groups_to_map_size: bool,
    pub no_grid: bool,
    pub avoid_player_areas: bool,
    pub ground_texture: String,
}

impl Default for PlaceObjectDef {
    fn default() -> Self {
        PlaceObjectDef {
            name: String::new(),
            number: 1,
            num_groups: 1,
            max_group_radius: 0,
            min_group_distance: 0,
            for_all_players: false,
            is_player: false,
            put_on_flat: false,
            min_dist_to_players: 0,
            max_dist_to_players: 0,
            avoid_other: 0,
            put_onto: String::new(),
            fill: false,
            scale_to_players: false,
            scale_to_map_size: false,
            scale_groups_to_map_size: false,
            no_grid: false,
            avoid_player_areas: false,
            ground_texture: String::new(),
        }
    }
}

/// Result of the dispatcher: everything the map script including includes defines (the fields of the
/// generator, `0x274` bytes, `gen+0x04 ... +0x26C`, and of the terrain as far as the script writes them).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RandomMapScript {
    /// All defines at the end: those given by the caller and the `#define` lines of the script.
    pub defines: Vec<String>,
    /// `#const NAME VALUE` in reading order.
    pub consts: Vec<(String, String)>,
    /// `ProjectFile "..."` (name between first and last `"`, empty without two `"`; last line counts).
    pub project_file: Option<String>,
    /// Resolved paths of `#include`s that could not be opened (silently ignored).
    pub missing_includes: Vec<String>,
    /// `SmoothFactor`, default 1.
    pub smooth_factor: i32,
    /// `PathNormal`, default 0 (also copied into `terrain.path_normal`).
    pub path_normal: f32,
    /// `BuildinglotXSize`/`YSize`, default 2/2 (parsed, never used).
    pub buildinglot_size: [i32; 2],
    pub buildinglot_texture: String,
    /// `BaseTexture`: name of the base texture.
    pub base_texture: String,
    pub textures: Vec<TextureDef>,
    pub objects: Vec<ObjectDef>,
    pub land_areas: Vec<AreaDef>,
    /// Templates of the `create_PlayerAreas` blocks (the generator creates `n` copies per block, D-328).
    pub player_areas: Vec<AreaDef>,
    pub terrain_areas: Vec<AreaDef>,
    pub connections: Vec<ConnectDef>,
    pub placements: Vec<PlaceObjectDef>,
    /// Terrain of the script: defaults, on top of them `TerrainLight` (`light_*`), `Water` (`water_*`) and
    /// `PathNormal`.
    pub terrain: TerrainParams,
    /// `ObjectLight`; if the block is missing, it stays `None` (D-345).
    pub object_light: Option<ObjectLight>,
    /// `ShadowLight`; if the block is missing, it stays `None` (D-345).
    pub shadow: Option<ShadowParams>,
}

impl Default for RandomMapScript {
    fn default() -> Self {
        RandomMapScript {
            defines: Vec::new(),
            consts: Vec::new(),
            project_file: None,
            missing_includes: Vec::new(),
            smooth_factor: 1,
            path_normal: 0.0,
            buildinglot_size: [2, 2],
            buildinglot_texture: String::new(),
            base_texture: String::new(),
            textures: Vec::new(),
            objects: Vec::new(),
            land_areas: Vec::new(),
            player_areas: Vec::new(),
            terrain_areas: Vec::new(),
            connections: Vec::new(),
            placements: Vec::new(),
            terrain: TerrainParams::default(),
            object_light: None,
            shadow: None,
        }
    }
}
