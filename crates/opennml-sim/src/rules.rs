//! Immutable rules of a match: types and modules from `property.ini`,
//! configuration values, player setup of the `.lvl`, start objects, variant rules, hit filters,
//! the objects of the `.wld` and the terrain. Not part of the savegame; their checksum is in the
//! start package.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, anyhow};
use opennml_formats::bonus::{BONUS_PATH, BonusTable, parse_bonus_ini};
use opennml_formats::config::{
    ConfigSets, DEFAULT_DAT_PATH, DROPDOWN_PATH, LevelPlayer, LevelSetup, parse_default_dat,
    parse_dropdowns, parse_level_setup,
};
use opennml_formats::gametype_rules::{
    GAMETYPE_DEFEAT_PATH, GAMETYPE_VICTORY_PATH, GameTypeRules, parse_gametype_rules,
};
use opennml_formats::gifcon::{
    DEFAULT_OBJECTS_PATH, DefaultObjects, GUID_GROUPS_PATH, HEROES_PATH, RESOURCE_CONFIG_PATH,
    ResourceConfig, TOOLBAR_PATH, Toolbar, UPGRADES_PATH, VariantRule, parse_default_objects,
    parse_guid_groups, parse_heroes, parse_resource_config, parse_toolbar, parse_variant_rules,
};
use opennml_formats::property::{ObjectType, PROPERTY_PATH, TRANSLATE_PATH};
use opennml_formats::rdo::{MeshKind, RdoMesh};
use opennml_formats::targets::{
    FLOWCONTROL_TARGETS_PATH, PRIORITIES_PATH, TARGETDEFINES_PATH, TARGETS_PATH, TargetEntry,
    parse_target_defines, parse_targets,
};
use opennml_formats::temp_upgrades::{TEMP_UPGRADES_PATH, TempUpgradeRecord, parse_temp_upgrades};
use opennml_formats::text::decode_cp1252;
use opennml_formats::wld::WorldObject;
use opennml_rda::{Vfs, normalize_path};
use opennml_world::assets::ModelCache;
use opennml_world::level::LevelScene;
use opennml_world::shadows::mesh_box;
use opennml_world::terrain::Terrain;

use crate::fx::Fx;
use crate::ki::catalog::KiCatalog;
use crate::path::terrain::classify_tiles;
use crate::random_map::RandomMapRules;
use crate::script::ScriptRules;

/// FNV-1a 64 bit (checksums of the rules and the state).
#[derive(Debug, Clone, Copy)]
pub struct Fnv64(pub u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Self(0xCBF2_9CE4_8422_2325)
    }
}

impl Fnv64 {
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
}

/// Size of the map and cell sizes (`GameWorld+0x34/+0x38/+0x3C`, 128 in the shipped data).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapInfo {
    /// `TERRAIN_DIM_X/Z`.
    pub dim_x: i32,
    pub dim_z: i32,
    /// `SX`, `SY`, `SZ` = `SIZE_X`, `SIZE_X`, `SIZE_Z` (`GameWorld_Init`). Always positive: the
    /// `.wld` reader (`opennml_formats::wld`, `[TERRAIN]`) rejects a tile size <= 0 with an error message
    /// before the simulation divides by it (`footprint`, `placement`, grid position).
    pub cell: [i32; 3],
    /// `WATER_LEVEL`.
    pub water_level: f32,
}

/// Shape of a building type for the footprint (`GameObject_GetFootprintTileRect`,
/// `PropertyBuilding_CarveFreeNodesFromMask`, re/occupancy 5).
#[derive(Debug, Clone, PartialEq)]
pub struct BuildingShape {
    /// Center of the mesh hulls without helper nodes and without animation.
    pub center: [f32; 3],
    /// Half extent of the head hull of the `.rdo`.
    pub half: [f32; 3],
    /// Rest positions of the helper nodes `frei01`, `frei02`, ... up to the first missing one.
    pub free_nodes: Vec<[f32; 3]>,
    /// All helper nodes of the model (mesh type 3005) by file name, the first one per name, with their
    /// rest position in object space (`ModelInstance_GetHelperNodePosition` `0x0058DAB0` without
    /// node track: `R(node) · hull center + position`), e.g. `ausgang`, `farmer01…05`.
    pub helper_nodes: BTreeMap<String, [f32; 3]>,
}

impl BuildingShape {
    /// Rest position of the helper node `name`, looked up like the original: name in lowercase,
    /// then exact comparison with the node names of the file.
    pub fn helper_node(&self, name: &str) -> Option<[f32; 3]> {
        self.helper_nodes.get(&name.to_lowercase()).copied()
    }
}

/// Hull box of a model for snapping the start buildings and the build site check
/// (center, half extent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelBox {
    pub center: [f32; 3],
    pub half: [f32; 3],
}

/// Terrain grid of the `BuildingTileMap` (`m_MapTerrain`, `BuildingTileMap_Init` `0x00490740`,
/// re/combat-targeting 4.2): per tile the mean of the four corner heights in fixed-point tiles
/// (`ftol(f·4096)/cell_y`, sum `/4` toward 0), at least the water level. Only
/// `BuildingTileMap_Init` writes it, hence it belongs to the rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TileTerrain {
    pub width: i32,
    pub height: i32,
    /// Heights, z outer, x inner.
    pub heights: Vec<i32>,
    /// `ftol(WATER_LEVEL·4096)/cell_y` (`0x0042D5B0`).
    pub water: i32,
}

impl TileTerrain {
    /// Built from the terrain; corner points outside count as height 0.0 (`0x005582E0`).
    pub fn build(terrain: &Terrain, map: &MapInfo) -> Self {
        let cy = map.cell[1];
        let water = Fx::from_f32(map.water_level).0 / cy;
        let (vx, vz) = (terrain.dim_x() as i32, terrain.dim_z() as i32);
        let corner = |x: i32, z: i32| {
            let y = if (0..=vx).contains(&x) && (0..=vz).contains(&z) {
                terrain.vertex(x as usize, z as usize).position.y
            } else {
                0.0
            };
            Fx::from_f32(y).0 / cy
        };
        let (w, h) = (map.dim_x.max(0), map.dim_z.max(0));
        let mut heights = Vec::with_capacity((w * h) as usize);
        for z in 0..h {
            for x in 0..w {
                let s = corner(x, z)
                    .wrapping_add(corner(x + 1, z))
                    .wrapping_add(corner(x, z + 1))
                    .wrapping_add(corner(x + 1, z + 1));
                heights.push((s / 4).max(water));
            }
        }
        Self {
            width: w,
            height: h,
            heights,
            water,
        }
    }

    /// Height of the tile `(x, z)`; 0 outside (`BuildingTileMap_GetHeight` `0x00490990`).
    pub fn at(&self, x: i32, z: i32) -> i32 {
        if (0..self.width).contains(&x) && (0..self.height).contains(&z) {
            self.heights[(z * self.width + x) as usize]
        } else {
            0
        }
    }
}

/// Number of tiles whose **highest** of the four corner heights lies strictly below `map.water_level`
/// (`GameEngine_ClassifyAllTiles` `0x00432140`, `0x00431AB0`; comparison in `f32`, corner points
/// `0..=dim`, re/ki-queries 5.16, E-83). The original counts when loading the world; later
/// terrain changes do not count.
fn count_water_tiles(terrain: &Terrain, map: &MapInfo) -> i32 {
    let (w, h) = (map.dim_x.max(0), map.dim_z.max(0));
    let (vx, vz) = (terrain.dim_x() as i32, terrain.dim_z() as i32);
    // Corner points outside count as height 0.0 like in `TileTerrain::build`.
    let high = |x: i32, z: i32| {
        if (0..=vx).contains(&x) && (0..=vz).contains(&z) {
            terrain.vertex(x as usize, z as usize).position.y
        } else {
            0.0
        }
    };
    let mut n = 0i32;
    for z in 0..h {
        for x in 0..w {
            let top = high(x, z)
                .max(high(x + 1, z))
                .max(high(x, z + 1))
                .max(high(x + 1, z + 1));
            if top < map.water_level {
                n += 1;
            }
        }
    }
    n
}

/// Hit zones of a model (`GameObject_CollectHitZones` `0x0046E100`, re/combat-projectiles
/// 15.1): `tzone01`, `tzone02`, ... up to the first gap, then with the same counter `tzone NN`.
/// `nodes`: helper nodes of the file in file order with rest position; the name is searched in
/// lowercase, compared exactly, the first match counts.
pub fn collect_hit_zones(nodes: &[(String, [f32; 3])]) -> Vec<[f32; 3]> {
    let find = |name: &str| nodes.iter().find(|(n, _)| n == name).map(|(_, p)| *p);
    let mut zones = Vec::new();
    let mut k = 1;
    while let Some(p) = find(&format!("tzone{k:02}")) {
        zones.push(p);
        k += 1;
    }
    while let Some(p) = find(&format!("tzone {k:02}")) {
        zones.push(p);
        k += 1;
    }
    zones
}

/// Rest position of a helper node in object space (`ModelInstance_GetHelperNodePosition` `0x0058DAB0`
/// without node track): `R(node) · hull center + position`.
fn helper_rest_position(m: &RdoMesh) -> [f32; 3] {
    (opennml_world::math::game_quat_matrix(m.rotation_wxyz) * glam::Vec3::from(m.bounds.center)
        + glam::Vec3::from(m.position))
    .to_array()
}

/// Walk flags of the mounts: `SADDLE` (`0x8000`, horses) and `SADDLEBOAT` (`0x10000`, canoes).
const MOUNT_FLAGS: u32 = 0x8000 | 0x1_0000;
/// Helper node of the dismount point (text `0x00625338`, re/riding 3.2).
const MOUNT_NODE: &str = "dummy01";
/// Flag `SHIP` of the shoot module (`110001`, `module::shoot::flag::SHIP`).
const SHOOT_SHIP: u32 = 0x2_0000;
/// Coupling nodes of the train vehicles (re/train-multipart 3.1) and their module `CPropertyTrain`.
const HITCH_NODE: &str = "vhinten01";
const COUPLING_NODE: &str = "vvorne01";
const TRAIN_MODULE: u32 = 33;

/// Helper node `name` (lowercase, exact comparison) in the main model `model`.
fn model_node(model: &opennml_world::assets::ModelAsset, name: &str) -> Option<[f32; 3]> {
    model
        .rdo
        .meshes
        .iter()
        .find(|m| m.kind == MeshKind::Helper && m.name == name)
        .map(helper_rest_position)
}

/// Rest position of the helper node `name` of a model, search order of the couplings and the mount
/// (re/riding 13.1, re/train-multipart 3.1): first the helper nodes of the
/// main model, otherwise the sub-objects of **all** animations of the object `.cfg` in file order.
/// After the main model, the original searches in the **running** animation (rendering); in the shipped data
/// all animations of the PRI canoe carry the same sub-object. The animated position (node track) is
/// not read (K-232, like K-8). The third search stage of the original (sub-objects of the object level
/// with `INDEX` != -1, re/riding 13.1) is omitted: no effect in the shipped data (`dummy01` is carried only by
/// 1712, the coupling nodes are in the main model).
fn find_node_in_any_animation(
    cache: &mut ModelCache,
    vfs: &Vfs,
    path: &str,
    name: &str,
) -> Option<[f32; 3]> {
    let model = cache.get(vfs, path)?;
    if let Some(p) = model_node(&model, name) {
        return Some(p);
    }
    model
        .config
        .animations
        .iter()
        .flat_map(|a| &a.sub_objects)
        .map(|s| normalize_path(&s.file))
        .find_map(|sub| cache.get(vfs, &sub).and_then(|s| model_node(&s, name)))
}

/// Rest position of the cannon node `name` with animation `anim` set
/// (`ModelInstance_GetHelperNodePosition` `0x0058DAB0` -> `0x0058BAF0`, re/ships 4.4): first the
/// main model, then the sub-objects of the **one** animation, then the global sub-objects with
/// `INDEX` != -1. The state "sub-object link active" (`ModelInstance_IsSubObjectLinkEnabled`) is
/// assumed to be set (hypothesis re/ships 4.4); the animated position is omitted (K-8).
fn find_cannon_node(
    cache: &mut ModelCache,
    vfs: &Vfs,
    path: &str,
    anim: i32,
    name: &str,
) -> Option<[f32; 3]> {
    let model = cache.get(vfs, path)?;
    if let Some(p) = model_node(&model, name) {
        return Some(p);
    }
    let in_anim = usize::try_from(anim)
        .ok()
        .and_then(|i| model.config.animations.get(i))
        .into_iter()
        .flat_map(|a| &a.sub_objects);
    let global = model.config.sub_objects.iter().filter(|s| s.index != -1);
    in_anim
        .chain(global)
        .map(|s| normalize_path(&s.file))
        .find_map(|sub| cache.get(vfs, &sub).and_then(|s| model_node(&s, name)))
}

/// Name of the cannon node `i` (from 1) of the broadside: `steuercannonNN` (starboard) or
/// `backcannonNN` (port), two digits (`AimShip` `0x005253A0`, re/ships 4.4).
pub fn cannon_node_name(starboard: bool, i: i32) -> String {
    let side = if starboard {
        "steuercannon"
    } else {
        "backcannon"
    };
    format!("{side}{i:02}")
}

/// Coupling nodes of a train vehicle (module 33, `Train_SetupWagonsAndSmoke` `0x00530220`,
/// re/train-multipart 3.1): `vhinten01` (hook to the rear) and `vvorne01` (counterpart at the front), each
/// rest position in object space; a missing node is `None` (the original then takes the zero vector).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrainNodes {
    pub hitch: Option<[f32; 3]>,
    pub coupling: Option<[f32; 3]>,
}

#[derive(Debug, Clone)]
pub struct Rules {
    /// Normalized path of the `.lvl`.
    pub level_path: String,
    /// `property.ini`.
    pub types: BTreeMap<u32, ObjectType>,
    /// `translate.ini` (rider -> rider + horse).
    pub translations: BTreeMap<u32, Vec<u32>>,
    /// `default.dat` (objects and upgrades).
    pub defaults: ConfigSets,
    /// Players, configuration sets and resources of the `.lvl`.
    pub level: LevelSetup,
    pub default_objects: DefaultObjects,
    pub variant_rules: Vec<VariantRule>,
    /// `targets.ini` (variant conditions 2 ... 999 and the right-click commands of [`crate::targeting`]).
    pub targets: Vec<TargetEntry>,
    /// `bonus.ini`: attack bonuses per attack kind and `MOUNTY_BONUS`.
    pub bonus: BonusTable,
    /// `priorities.ini` (target priorities of combat) in file order.
    pub priorities: Vec<TargetEntry>,
    /// `flowcontroltragets.ini` (`GameWorld+0x338`): MY filters of the filter IDs below 1000 that the AI
    /// uses ([`crate::filter::from_id`]), in file order.
    pub flowcontrol_targets: Vec<TargetEntry>,
    /// `gametypevictory.ini` (`GameEngine+0x2CD4`): victory rules per game type and nation
    /// ([`crate::game_end::check_victory_defeat`]).
    pub victory: GameTypeRules,
    /// `gametypedefeat.ini` (`GameEngine+0x2CD0`): defeat rules per game type and nation.
    pub defeat: GameTypeRules,
    /// `toolbar.ini`: build/production menus, button kinds, prerequisites.
    pub toolbar: Toolbar,
    /// `guidgroups.ini`: group GUID -> members (`GameWorld+0x22D8`).
    pub guid_groups: BTreeMap<u32, Vec<u32>>,
    /// `resourcenconfig.ini`: start resources per resource setting.
    pub resource_config: ResourceConfig,
    /// `[OBJECT]` blocks of the `.wld` in file order.
    pub world_objects: Vec<WorldObject>,
    pub map: MapInfo,
    pub terrain: Terrain,
    /// Normalized model path per type (project file).
    pub models: BTreeMap<u32, String>,
    /// Types with `OBJ_BLOCK_PATH == 1` (project file).
    pub block_path: BTreeSet<u32>,
    /// Time markers (marker ID, ms) per animation of every type with a loadable model (for all types,
    /// E-212) in file order (`GameObject_GetAnimLabelTime` `0x004669D0`); a missing
    /// animation gives an empty list.
    pub anim_labels: BTreeMap<u32, Vec<Vec<(u32, i32)>>>,
    /// Hull box (head hull of the `.rdo`, `Model_GetBoundsCenter/HalfExtent`) of all building types
    /// with a loadable model; mandatory for the start buildings from `defaultobjects.ini`.
    pub model_boxes: BTreeMap<u32, ModelBox>,
    /// Shape of all building types with a loadable model.
    pub building_shapes: BTreeMap<u32, BuildingShape>,
    /// Hit zones (rest position, object space) of all types with a loadable model and without module `0x29`
    /// (re/combat-projectiles 15.1); a missing entry means "no zone".
    pub hit_zones: BTreeMap<u32, Vec<[f32; 3]>>,
    /// Model center (`ObjectRenderer_GetBoundsCenter(·, 0)` `0x00548340`: mesh hulls without
    /// helper nodes, without animation) of all types with a loadable model (re/combat-projectiles 15.4).
    pub model_centers: BTreeMap<u32, [f32; 3]>,
    /// Mesh names (as in the file) of model and sub-objects (also those of the animations) of the
    /// types with a shoot module (10): `PropertyShootProjectile_ResolveMainObject`
    /// (`0x005257C0`) resolves the names from 110007 against them (re/combat-projectiles 15.6).
    pub mesh_names: BTreeMap<u32, BTreeSet<String>>,
    /// Rest position (object space) of the helper node `dummy01` of the mounts (walk flag `SADDLE 0x8000`
    /// or `SADDLEBOAT 0x10000`, re/riding 3.2, 13.1): dismount point when dismounting; a missing
    /// entry means "circle search". In the shipped data only 1712 (PRI canoe).
    pub mount_nodes: BTreeMap<u32, [f32; 3]>,
    /// Rest positions of the cannon nodes `steuercannon01…`/`backcannon01…` of the broadside (re/ships 4.4):
    /// key (GUID, animation index, starboard) - the animation is the first number of 110002
    /// (starboard) or 110003 (port) of a shoot module variant with flag `SHIP`; entry `k`
    /// is the rest position of node `k + 1`. Non-empty lists only; a missing entry means no
    /// cannon.
    pub cannon_nodes: BTreeMap<(u32, i32, bool), Vec<[f32; 3]>>,
    /// Coupling nodes `vhinten01`/`vvorne01` of all types with train module 33 (re/train-multipart 3.1).
    pub train_nodes: BTreeMap<u32, TrainNodes>,
    /// Terrain grid of the building map.
    pub tile_terrain: TileTerrain,
    /// Tiles whose highest corner height lies strictly below the water level (`GameEngine+0x5F0`,
    /// `GameEngine_ClassifyAllTiles` `0x00432140`, re/ki-queries 5.16, E-83); counted once when
    /// loading. Derived, hence not in [`Rules::checksum`].
    pub water_tiles: i32,
    /// Tiles of the map, `DIM_X · DIM_Z` (`GameEngine+0x5F4`); derived like `water_tiles`.
    pub tile_count: i32,
    /// Terrain class per tile (`opennml_sim::path::terrain::class`), z outer, x inner.
    pub tile_classes: Vec<u8>,
    /// Checksum over all files read and derived tables.
    pub checksum: u64,
    /// `[UPGRADE]` of the project file (re/upgrades 1.1): the GUIDs of the upgrade list of the catalog.
    pub upgrades: BTreeSet<u32>,
    /// Upgrade GUID -> types whose `OBJ_UPGRADES` names it, ascending by type GUID
    /// (`Project_GetTypesWithUpgrade` `0x004FB740`, re/upgrades 10.1); no types without an entry.
    pub upgrade_types: BTreeMap<u32, Vec<u32>>,
    /// `tempupgrades.ini` (`m_TempUpgradeMap`): records of the temporary upgrades by GUID; with a
    /// duplicate GUID the later one applies (`CMap::SetAt`, re/temp-upgrades 1.1).
    pub temp_upgrades: BTreeMap<u32, TempUpgradeRecord>,
    /// `heroes.ini` (`GameWorld+0x232C`, re/heroes-resources 1.1): hero GUIDs in file order,
    /// duplicates allowed; index = sort key of the selection ([`Rules::hero_index`]).
    pub heroes: Vec<u32>,
    /// AI prototypes of the 280 modules from `nml.db`; the threads of the AI VM arise
    /// from them.
    pub ki: KiCatalog,
    /// Scripts (level and dialog scripts), areas and speech lengths.
    pub scripts: ScriptRules,
    /// Random map: description, slots, island flag, start value of the CRT sequence
    /// and checksum of the generator; `None` for fixed maps and synthetic rules.
    pub random_map: Option<RandomMapRules>,
}

/// Module index Building (`PROPID 3`).
const BUILDING: u32 = 3;

/// Player setup of a random map: per occupied slot ascending
/// `LevelPlayer { slot, name: "", nation: 0, kind: 0, resources: [] }`; level `[RESOURCES]` and
/// slot sets empty, since only `PlayerSetup_ParseLvl` fills them (`GameWorld+0x3C4`, D-364).
fn random_level_setup(random: &RandomMapRules) -> LevelSetup {
    LevelSetup {
        level_resources: BTreeMap::new(),
        slot_configs: BTreeMap::new(),
        players: (0..8u8)
            .filter(|&slot| random.slots[usize::from(slot)])
            .map(|slot| LevelPlayer {
                slot,
                name: String::new(),
                nation: 0,
                kind: 0,
                resources: Vec::new(),
            })
            .collect(),
    }
}

impl Rules {
    /// Loads the rules for a loaded level.
    pub fn load(vfs: &Vfs, scene: &LevelScene) -> anyhow::Result<Self> {
        Self::build(vfs, scene, None)
    }

    /// Rules of a random map without `.lvl` (`GameEngine_CreateRandomMapLevel`
    /// `0x004402F0`): per occupied slot (`random.slots`, ascending) a player without a name, nation 0,
    /// type (`kind`) 0, without resources; no level `[RESOURCES]` and no slot sets (values from
    /// `default.dat`, time-of-day stage 0, D-364); no level script, no dialogs. The checksum runs
    /// over the same shared files as for fixed maps, in place of `.lvl`, `.wld` and `.hlm`
    /// `random.digest` stands at the end.
    pub fn load_random(
        vfs: &Vfs,
        scene: &LevelScene,
        random: RandomMapRules,
    ) -> anyhow::Result<Self> {
        Self::build(vfs, scene, Some(random))
    }

    /// Shared build of [`Rules::load`] and [`Rules::load_random`]: the hash sequence
    /// of fixed maps stays bit-identical; a random map branches only at the place of the `.lvl`
    /// (nothing hashed), at `.wld`/`.hlm` (skipped) and at the scripts (none), and appends
    /// its checksum last.
    fn build(
        vfs: &Vfs,
        scene: &LevelScene,
        random: Option<RandomMapRules>,
    ) -> anyhow::Result<Self> {
        let mut hash = Fnv64::default();
        let mut text = |path: &str| -> anyhow::Result<String> {
            let bytes = vfs.read(path).with_context(|| format!("reading {path}"))?;
            hash.write(path.as_bytes());
            hash.write(&bytes);
            Ok(decode_cp1252(&bytes))
        };
        let dropdowns = parse_dropdowns(&text(DROPDOWN_PATH)?);
        let defaults = parse_default_dat(&text(DEFAULT_DAT_PATH)?, &dropdowns)
            .with_context(|| format!("parsing {DEFAULT_DAT_PATH}"))?;
        let level = match &random {
            None => parse_level_setup(&text(&scene.lvl_path)?, &defaults, &dropdowns)
                .with_context(|| format!("parsing {}", scene.lvl_path))?,
            Some(random) => random_level_setup(random),
        };
        let default_objects = parse_default_objects(&text(DEFAULT_OBJECTS_PATH)?, &dropdowns)
            .with_context(|| format!("parsing {DEFAULT_OBJECTS_PATH}"))?;
        if !default_objects.complete {
            log::warn!("{DEFAULT_OBJECTS_PATH}: ends in an open block or was aborted");
        }
        let variant_rules = parse_variant_rules(&text(UPGRADES_PATH)?);
        let defines = parse_target_defines(&text(TARGETDEFINES_PATH)?);
        let targets = parse_targets(&text(TARGETS_PATH)?, &defines);
        if let Some(reason) = &targets.aborted {
            log::warn!("{TARGETS_PATH}: reading aborted as in the game ({reason})");
        }
        let bonus = parse_bonus_ini(&text(BONUS_PATH)?);
        let priorities = parse_targets(&text(PRIORITIES_PATH)?, &defines);
        if let Some(reason) = &priorities.aborted {
            log::warn!("{PRIORITIES_PATH}: reading aborted as in the game ({reason})");
        }
        let flowcontrol = parse_targets(&text(FLOWCONTROL_TARGETS_PATH)?, &defines);
        if let Some(reason) = &flowcontrol.aborted {
            log::warn!("{FLOWCONTROL_TARGETS_PATH}: reading aborted as in the game ({reason})");
        }
        // Rules for victory and defeat (`GameEngine_LoadGameTypeConditionsIni` `0x0042CE60`).
        let victory = parse_gametype_rules(&text(GAMETYPE_VICTORY_PATH)?);
        let defeat = parse_gametype_rules(&text(GAMETYPE_DEFEAT_PATH)?);
        // Types, translations, project file, world and terrain come from the scene; their files
        // nevertheless go into the checksum. Without `property.ini` or `translate.ini` there would be
        // no types (`ObjectTypes::load` only warns): that is an error, so that the map is still
        // displayed without a simulation.
        text(PROPERTY_PATH)?;
        text(TRANSLATE_PATH)?;
        let toolbar = parse_toolbar(&text(TOOLBAR_PATH)?);
        let guid_groups = parse_guid_groups(&text(GUID_GROUPS_PATH)?);
        let heroes = parse_heroes(&text(HEROES_PATH)?);
        let resource_config = parse_resource_config(&text(RESOURCE_CONFIG_PATH)?);
        let temp_upgrades: BTreeMap<u32, TempUpgradeRecord> =
            parse_temp_upgrades(&text(TEMP_UPGRADES_PATH)?)
                .with_context(|| format!("parsing {TEMP_UPGRADES_PATH}"))?
                .into_iter()
                .map(|r| (r.guid, r))
                .collect();
        // `[UPGRADE]` and `OBJ_UPGRADES` come from the project file of the scene (already hashed).
        let upgrades: BTreeSet<u32> = scene.project.upgrades.keys().copied().collect();
        let mut upgrade_types: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for object in scene.project.objects.values() {
            for &upgrade in &object.upgrades {
                let types = upgrade_types.entry(upgrade).or_default();
                // A GUID that a type names several times counts once for it.
                if types.last() != Some(&object.guid) {
                    types.push(object.guid);
                }
            }
        }
        let base = scene.lvl_path.trim_end_matches(".lvl").to_string();
        let level_files = match &random {
            None => vec![format!("{base}.wld"), format!("{base}.hlm")],
            // The random map has no files; its terrain is in `random.digest`.
            Some(_) => Vec::new(),
        };
        for path in std::iter::once(normalize_path(&scene.level.project_file)).chain(level_files) {
            if let Ok(bytes) = vfs.read(&path) {
                hash.write(path.as_bytes());
                hash.write(&bytes);
            }
        }
        let models: BTreeMap<u32, String> = scene
            .project
            .objects
            .values()
            .map(|o| (o.guid, normalize_path(&o.model)))
            .collect();
        let block_path: BTreeSet<u32> = scene
            .project
            .objects
            .values()
            .filter(|o| o.block_path)
            .map(|o| o.guid)
            .collect();
        hash.write(
            &block_path
                .iter()
                .flat_map(|g| g.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        let p = &scene.world.terrain;
        let map = MapInfo {
            dim_x: p.dim_x as i32,
            dim_z: p.dim_z as i32,
            cell: [p.size_x as i32, p.size_x as i32, p.size_z as i32],
            water_level: p.water_level,
        };
        let mut cache = ModelCache::new();
        let mut anim_labels = BTreeMap::new();
        // Time markers for **all** types with a loadable model (E-212): `GameObject_GetAnimLabelTime`
        // asks the model of every object, and the animation lock 157 with 0 seconds takes the marker 1
        // of the animation of any type.
        for &guid in scene.types.types.keys() {
            let Some(model) = models.get(&guid).and_then(|m| cache.get(vfs, m)) else {
                continue;
            };
            let labels: Vec<Vec<(u32, i32)>> = model
                .animations
                .iter()
                .map(|anim| {
                    anim.as_ref().map_or_else(Vec::new, |a| {
                        a.labels.iter().map(|l| (l.id, l.time_ms as i32)).collect()
                    })
                })
                .collect();
            anim_labels.insert(guid, labels);
        }
        let mut model_boxes = BTreeMap::new();
        for entry in default_objects.start.values().flatten() {
            let guid = entry.guid;
            let is_building = scene
                .types
                .types
                .get(&guid)
                .is_some_and(|t| t.has_module(BUILDING));
            if !is_building || model_boxes.contains_key(&guid) {
                continue;
            }
            let model = models
                .get(&guid)
                .and_then(|m| cache.get(vfs, m))
                .ok_or_else(|| anyhow!("model of the start building {guid} not loadable"))?;
            let b = &model.rdo.bounds;
            let half = [0, 1, 2].map(|i| (b.max[i] - b.min[i]) * 0.5);
            model_boxes.insert(
                guid,
                ModelBox {
                    center: b.center,
                    half,
                },
            );
        }
        let mut building_shapes = BTreeMap::new();
        for (&guid, ty) in &scene.types.types {
            if !ty.has_module(BUILDING) {
                continue;
            }
            let Some(model) = models.get(&guid).and_then(|m| cache.get(vfs, m)) else {
                log::warn!("Building {guid}: model not loadable, without footprint");
                continue;
            };
            let center = mesh_box(&model, None).map_or([0.0; 3], |b| b.center().to_array());
            let b = &model.rdo.bounds;
            let half = [0, 1, 2].map(|i| (b.max[i] - b.min[i]) * 0.5);
            // Head hull also for all other buildings (build site check).
            model_boxes.entry(guid).or_insert(ModelBox {
                center: b.center,
                half,
            });
            let free_nodes = (1..)
                .map_while(|k| {
                    let name = format!("frei{k:02}");
                    model
                        .rdo
                        .meshes
                        .iter()
                        .find(|m| m.kind == MeshKind::Helper && m.name == name)
                        .map(|m| m.position)
                })
                .collect();
            let mut helper_nodes = BTreeMap::new();
            for m in model
                .rdo
                .meshes
                .iter()
                .filter(|m| m.kind == MeshKind::Helper)
            {
                helper_nodes
                    .entry(m.name.clone())
                    .or_insert(helper_rest_position(m));
            }
            building_shapes.insert(
                guid,
                BuildingShape {
                    center,
                    half,
                    free_nodes,
                    helper_nodes,
                },
            );
        }
        for (guid, s) in &building_shapes {
            hash.write(&guid.to_le_bytes());
            for v in s
                .center
                .iter()
                .chain(&s.half)
                .chain(s.free_nodes.iter().flatten())
            {
                hash.write(&v.to_le_bytes());
            }
            for (name, p) in &s.helper_nodes {
                hash.write(name.as_bytes());
                for v in p {
                    hash.write(&v.to_le_bytes());
                }
            }
        }
        let mut hit_zones = BTreeMap::new();
        let mut model_centers = BTreeMap::new();
        let mut mesh_names = BTreeMap::new();
        for (&guid, ty) in &scene.types.types {
            let Some(path) = models.get(&guid) else {
                continue;
            };
            // First copy everything from the main model, then load the sub-objects.
            let Some((nodes, mut names, subs, center)) = cache.get(vfs, path).map(|model| {
                let nodes: Vec<(String, [f32; 3])> = model
                    .rdo
                    .meshes
                    .iter()
                    .filter(|m| m.kind == MeshKind::Helper)
                    .map(|m| (m.name.clone(), helper_rest_position(m)))
                    .collect();
                let names: BTreeSet<String> =
                    model.rdo.meshes.iter().map(|m| m.name.clone()).collect();
                // Sub-objects of the model and of all animations (`FUN_005885F0`, like
                // `opennml_world::anim`).
                let subs: Vec<String> = model
                    .config
                    .sub_objects
                    .iter()
                    .chain(model.config.animations.iter().flat_map(|a| &a.sub_objects))
                    .map(|s| normalize_path(&s.file))
                    .collect();
                let center = mesh_box(&model, None).map_or([0.0; 3], |b| b.center().to_array());
                (nodes, names, subs, center)
            }) else {
                continue;
            };
            model_centers.insert(guid, center);
            if !ty.has_module(0x29) {
                let zones = collect_hit_zones(&nodes);
                if !zones.is_empty() {
                    hit_zones.insert(guid, zones);
                }
            }
            if ty.has_module(10) {
                for sub in subs {
                    if let Some(s) = cache.get(vfs, &sub) {
                        names.extend(s.rdo.meshes.iter().map(|m| m.name.clone()));
                    }
                }
                mesh_names.insert(guid, names);
            }
        }
        let mut mount_nodes = BTreeMap::new();
        for (&guid, ty) in &scene.types.types {
            let walk_flags = ty.module(0).and_then(|m| m.value_int(10003)).unwrap_or(0) as u32;
            if walk_flags & MOUNT_FLAGS == 0 {
                continue;
            }
            if let Some(p) = models
                .get(&guid)
                .and_then(|path| find_node_in_any_animation(&mut cache, vfs, path, MOUNT_NODE))
            {
                mount_nodes.insert(guid, p);
            }
        }
        // Broadside cannons: per shoot module variant with `SHIP` the first animation of
        // 110002 (starboard) and 110003 (port). The same keys from several variants
        // give the same list (the search depends only on model and animation).
        let mut cannon_nodes = BTreeMap::new();
        for (&guid, ty) in &scene.types.types {
            let Some(path) = models.get(&guid) else {
                continue;
            };
            for desc in ty.modules.iter().filter(|m| m.index == 10) {
                let flags = desc.value_int(110001).unwrap_or(0) as u32;
                if flags & SHOOT_SHIP == 0 {
                    continue;
                }
                for (starboard, value) in [(true, 110002), (false, 110003)] {
                    let list = desc.values.get(&value).map_or("", String::as_str);
                    let Some(&anim) = crate::module::stand::parse_int_list(list, &[]).first()
                    else {
                        continue;
                    };
                    let key = (guid, anim, starboard);
                    if cannon_nodes.contains_key(&key) {
                        continue;
                    }
                    let nodes: Vec<[f32; 3]> = (1..)
                        .map_while(|i| {
                            let name = cannon_node_name(starboard, i);
                            find_cannon_node(&mut cache, vfs, path, anim, &name)
                        })
                        .collect();
                    if !nodes.is_empty() {
                        cannon_nodes.insert(key, nodes);
                    }
                }
            }
        }
        // Coupling nodes of all types with a train module.
        let mut train_nodes = BTreeMap::new();
        for (&guid, ty) in &scene.types.types {
            if !ty.has_module(TRAIN_MODULE) {
                continue;
            }
            let mut find = |name: &str| {
                models
                    .get(&guid)
                    .and_then(|path| find_node_in_any_animation(&mut cache, vfs, path, name))
            };
            let hitch = find(HITCH_NODE);
            let coupling = find(COUPLING_NODE);
            train_nodes.insert(guid, TrainNodes { hitch, coupling });
        }
        let tile_terrain = TileTerrain::build(&scene.terrain, &map);
        let water_tiles = count_water_tiles(&scene.terrain, &map);
        let tile_count = map.dim_x.wrapping_mul(map.dim_z);
        let tile_classes = classify_tiles(&scene.terrain, |guid| {
            guid.and_then(|g| scene.project.textures.get(&g))
                .map_or(0, |t| t.bits)
        });
        hash.write(&tile_classes);
        for (guid, anims) in &anim_labels {
            hash.write(&guid.to_le_bytes());
            hash.write(&(anims.len() as u32).to_le_bytes());
            for labels in anims {
                hash.write(&(labels.len() as u32).to_le_bytes());
                for (id, ms) in labels {
                    hash.write(&id.to_le_bytes());
                    hash.write(&ms.to_le_bytes());
                }
            }
        }
        for (guid, b) in &model_boxes {
            hash.write(&guid.to_le_bytes());
            for v in b.center.iter().chain(&b.half) {
                hash.write(&v.to_le_bytes());
            }
        }
        for (guid, zones) in &hit_zones {
            hash.write(&guid.to_le_bytes());
            for v in zones.iter().flatten() {
                hash.write(&v.to_le_bytes());
            }
        }
        for (guid, p) in &mount_nodes {
            hash.write(&guid.to_le_bytes());
            for v in p {
                hash.write(&v.to_le_bytes());
            }
        }
        for ((guid, anim, starboard), nodes) in &cannon_nodes {
            hash.write(&guid.to_le_bytes());
            hash.write(&anim.to_le_bytes());
            hash.write(&[u8::from(*starboard)]);
            for v in nodes.iter().flatten() {
                hash.write(&v.to_le_bytes());
            }
        }
        for (guid, t) in &train_nodes {
            hash.write(&guid.to_le_bytes());
            for node in [t.hitch, t.coupling] {
                hash.write(&[u8::from(node.is_some())]);
                for v in node.iter().flatten() {
                    hash.write(&v.to_le_bytes());
                }
            }
        }
        for (guid, c) in &model_centers {
            hash.write(&guid.to_le_bytes());
            for v in c {
                hash.write(&v.to_le_bytes());
            }
        }
        for (guid, names) in &mesh_names {
            hash.write(&guid.to_le_bytes());
            for n in names {
                hash.write(n.as_bytes());
                hash.write(&[0]);
            }
        }
        for h in &tile_terrain.heights {
            hash.write(&h.to_le_bytes());
        }
        // Last: `nml.db` and the module files are appended to the end of the checksum, then the
        // scripts and speech lengths; their load errors are in `scripts.errors`.
        let ki = KiCatalog::load(vfs, &mut hash)?;
        let scripts = match &random {
            None => ScriptRules::load(vfs, scene, &mut hash),
            // No level script; the seed text thus never forms a file path.
            Some(_) => ScriptRules::default(),
        };
        if let Some(random) = &random {
            hash.write(&random.digest.to_le_bytes());
        }
        Ok(Self {
            level_path: scene.lvl_path.clone(),
            types: scene.types.types.clone(),
            translations: scene.types.translations.clone(),
            defaults,
            level,
            default_objects,
            variant_rules,
            targets: targets.entries,
            bonus,
            priorities: priorities.entries,
            flowcontrol_targets: flowcontrol.entries,
            victory,
            defeat,
            toolbar,
            guid_groups,
            resource_config,
            world_objects: scene.world.objects.clone(),
            map,
            terrain: scene.terrain.clone(),
            models,
            block_path,
            anim_labels,
            model_boxes,
            building_shapes,
            hit_zones,
            model_centers,
            mesh_names,
            mount_nodes,
            cannon_nodes,
            train_nodes,
            tile_terrain,
            water_tiles,
            tile_count,
            tile_classes,
            checksum: hash.0,
            upgrades,
            upgrade_types,
            temp_upgrades,
            heroes,
            ki,
            scripts,
            random_map: random,
        })
    }

    /// `PlayerConfig_GetValue` (`0x0047C850`): value `cfg` of the type `guid` for player slot `slot`:
    /// first the set of the slot (`.lvl`), then `default.dat`; a value 0 falls through. Slots 9
    /// and 10 have no set of their own, slots from 11 return 0.
    pub fn config_value(&self, slot: u8, guid: u32, cfg: u32) -> i32 {
        if slot >= 11 {
            return 0;
        }
        let from_slot = self
            .level
            .slot_configs
            .get(&slot)
            .and_then(|sets| sets.get(&guid))
            .and_then(|set| set.get(&cfg))
            .copied()
            .unwrap_or(0);
        if from_slot != 0 {
            return from_slot;
        }
        self.defaults
            .get(&guid)
            .and_then(|set| set.get(&cfg))
            .copied()
            .unwrap_or(0)
    }

    /// `GameWorld_GetTypeValueSet` (`0x0047C7F0`): the **whole** type value set of the slot (slots from
    /// 8 read set 8, `[NEUTRAL]`), otherwise that from `default.dat`; none -> `None`. Unlike
    /// [`Rules::config_value`] (`PlayerConfig_GetValue`), which falls back to `default.dat` per value
    /// if the slot value is 0: here a value 0 in the set of the slot applies. The set of a slot
    /// is a copy of the `default.dat` set with the changes of the map when reading the `.lvl`,
    /// so missing values exist in it only where `default.dat` has none either. Used for costs
    /// (`Player_GetObjectCosts`, `Player_CanAffordObject`). A negative slot (unchecked in the
    /// original, exception A7) reads only `default.dat`.
    pub fn type_value_set(&self, slot: i32, guid: u32) -> Option<&BTreeMap<u32, i32>> {
        let own = u8::try_from(slot.min(8))
            .ok()
            .and_then(|s| self.level.slot_configs.get(&s))
            .and_then(|sets| sets.get(&guid));
        own.or_else(|| self.defaults.get(&guid))
    }

    /// Time marker 1 of the animation `anim` of the type `guid` in ms (0 without marker or animation).
    pub fn anim_label_time(&self, guid: u32, anim: i32) -> i32 {
        self.anim_label_time_at(guid, anim, 1)
    }

    /// `GameObject_GetAnimLabelTime(anim, label)` (`0x004669D0`): first marker with this ID of the
    /// animation `anim` of the type, in ms; missing animation or marker -> 0.
    pub fn anim_label_time_at(&self, guid: u32, anim: i32, label: i32) -> i32 {
        let Ok(i) = usize::try_from(anim) else {
            return 0;
        };
        self.anim_labels
            .get(&guid)
            .and_then(|a| a.get(i))
            .and_then(|l| l.iter().find(|(id, _)| i64::from(*id) == i64::from(label)))
            .map_or(0, |&(_, ms)| ms)
    }

    /// Set the time markers of the animation `anim` (used by synthetic rules and tests in several
    /// modules); missing animations before it stay empty.
    pub fn set_anim_labels(&mut self, guid: u32, anim: usize, labels: &[(u32, i32)]) {
        let list = self.anim_labels.entry(guid).or_default();
        if list.len() <= anim {
            list.resize(anim + 1, Vec::new());
        }
        list[anim] = labels.to_vec();
    }

    /// First entry of `targets.ini` with this `BUTTONMSG`
    /// (`GameWorld_FindTargetEntryByButtonMsg` `0x0047B420`).
    pub fn target_entry(&self, button_msg: i32) -> Option<&TargetEntry> {
        self.targets.iter().find(|e| e.button_msg == button_msg)
    }

    /// First entry of `flowcontroltragets.ini` with this `BUTTONMSG` (list `GameWorld+0x338`,
    /// search in `FUN_0047E390`).
    pub fn flowcontrol_target(&self, button_msg: i32) -> Option<&TargetEntry> {
        self.flowcontrol_targets
            .iter()
            .find(|e| e.button_msg == button_msg)
    }

    /// Ground height (`Terrain_GetHeightAt` `0x0055A940`).
    pub fn ground_height(&self, x: f32, z: f32) -> f32 {
        self.terrain.ground_height(x, z)
    }

    /// Rules without game files (tests): flat map `dim × dim` tiles of 128 at height 0, water
    /// at -100, types from `property_ini`, variant rule `BIT 0 = 1` as in the shipped data, otherwise empty.
    #[doc(hidden)]
    pub fn synthetic(property_ini: &str, dim: u32) -> Self {
        use opennml_formats::prj::Project;
        use opennml_formats::property::parse_property_ini;
        use opennml_formats::testutil::flat_heightmap;
        use opennml_formats::tlm::TextureMap;
        use opennml_formats::wld::TerrainParams;
        let params = TerrainParams {
            dim_x: dim,
            dim_z: dim,
            chunk_size: dim,
            water_level: -100.0,
            ..TerrainParams::default()
        };
        let texture_map = TextureMap {
            dim_x: dim as usize,
            dim_z: dim as usize,
            guids: vec![0; (dim * dim) as usize],
        };
        let terrain = Terrain::new(
            &params,
            &flat_heightmap(dim, dim, 128.0, 0.0),
            &texture_map,
            &Project::default(),
        )
        .expect("flat terrain");
        let tile_classes = classify_tiles(&terrain, |_| 0);
        let map = MapInfo {
            dim_x: dim as i32,
            dim_z: dim as i32,
            cell: [128; 3],
            water_level: -100.0,
        };
        let tile_terrain = TileTerrain::build(&terrain, &map);
        Self {
            level_path: "synthetisch".into(),
            types: parse_property_ini(property_ini),
            translations: BTreeMap::new(),
            defaults: ConfigSets::new(),
            level: LevelSetup::default(),
            default_objects: DefaultObjects::default(),
            variant_rules: vec![VariantRule {
                bit: 0,
                terms: vec![1],
            }],
            targets: Vec::new(),
            bonus: BonusTable::default(),
            priorities: Vec::new(),
            flowcontrol_targets: Vec::new(),
            // Empty rule tables: no slot wins or loses by rules
            // (`GameTypeRules_Evaluate` without a root is false).
            victory: GameTypeRules::default(),
            defeat: GameTypeRules::default(),
            toolbar: Toolbar::default(),
            guid_groups: BTreeMap::new(),
            resource_config: ResourceConfig::default(),
            world_objects: Vec::new(),
            map,
            terrain,
            models: BTreeMap::new(),
            block_path: BTreeSet::new(),
            anim_labels: BTreeMap::new(),
            model_boxes: BTreeMap::new(),
            building_shapes: BTreeMap::new(),
            hit_zones: BTreeMap::new(),
            model_centers: BTreeMap::new(),
            mesh_names: BTreeMap::new(),
            mount_nodes: BTreeMap::new(),
            cannon_nodes: BTreeMap::new(),
            train_nodes: BTreeMap::new(),
            tile_terrain,
            water_tiles: 0,
            tile_count: dim as i32 * dim as i32,
            tile_classes,
            checksum: 0,
            upgrades: BTreeSet::new(),
            upgrade_types: BTreeMap::new(),
            temp_upgrades: BTreeMap::new(),
            heroes: Vec::new(),
            ki: KiCatalog::default(),
            scripts: ScriptRules::default(),
            random_map: None,
        }
    }

    /// Is `guid` an upgrade? `[UPGRADE]` of the project file **or** an entry in `default.dat`
    /// (re/upgrades 1.1: 2840 and the time-of-day upgrades 4500 onward are in neither of the two).
    /// `default.dat` also lists the types, so the question is only to be asked together with "no known
    /// type".
    pub fn is_upgrade(&self, guid: u32) -> bool {
        self.upgrades.contains(&guid) || self.defaults.contains_key(&guid)
    }

    /// `Project_GetTypesWithUpgrade` (`0x004FB740`): types that the upgrade `guid` concerns,
    /// ascending by type GUID; empty without an entry.
    pub fn types_with_upgrade(&self, guid: u32) -> &[u32] {
        self.upgrade_types.get(&guid).map_or(&[], Vec::as_slice)
    }

    /// Record of the temporary upgrade `guid` from `tempupgrades.ini`.
    pub fn temp_upgrade(&self, guid: u32) -> Option<&TempUpgradeRecord> {
        self.temp_upgrades.get(&guid)
    }

    /// `Player_IsGuidDisabled` (`0x0047DE80`), **level data** part: the whole value set of the slot
    /// (`type_value_set`) contains config 1 ("upgrade is in the level") with 0 ("disabled"). A
    /// missing set or value 1 means free (re/upgrades 10.4). The script lock
    /// (`GameWorld+0x21E0`, action 207) is in the savegame; **all readers** go through
    /// [`crate::world::is_guid_disabled`], which combines both parts.
    pub fn is_guid_disabled(&self, slot: i32, guid: u32) -> bool {
        self.type_value_set(slot, guid)
            .and_then(|set| set.get(&1))
            .is_some_and(|&v| v == 0)
    }

    /// `GameWorld_GetHeroIndex` (`0x0047BD30`, re/heroes-resources 1.3): first index of `guid` in
    /// the hero list, otherwise -1.
    pub fn hero_index(&self, guid: u32) -> i32 {
        self.heroes
            .iter()
            .position(|&g| g == guid)
            .map_or(-1, |i| i as i32)
    }

    /// Time of day of the level (`level_resources[40]`, re/upgrades 3.8); if the value is missing or is
    /// a pointer value (`None`), 0 applies.
    pub fn level_daytime(&self) -> i32 {
        self.level
            .level_resources
            .get(&40)
            .copied()
            .flatten()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_zones_continue_their_count_with_the_spaced_name() {
        let n = |s: &str, x: f32| (s.to_string(), [x, 0.0, 0.0]);
        // tzone01, tzone02, then a gap; continues with "tzone 03" (re/combat-projectiles 15.1).
        let nodes = [
            n("frei01", 9.0),
            n("tzone02", 2.0),
            n("tzone01", 1.0),
            n("tzone 03", 3.0),
            n("tzone 05", 5.0),
        ];
        assert_eq!(
            collect_hit_zones(&nodes),
            vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]]
        );
        // Nodes in uppercase do not match (the name is searched in lowercase).
        assert!(collect_hit_zones(&[n("TZone01", 1.0)]).is_empty());
        assert!(collect_hit_zones(&[]).is_empty());
    }

    #[test]
    fn cannon_node_names_have_two_digits() {
        // Helper nodes of the broadside (re/ships 4.4): `steuercannonNN` (starboard), `backcannonNN`.
        assert_eq!(cannon_node_name(true, 1), "steuercannon01");
        assert_eq!(cannon_node_name(false, 12), "backcannon12");
    }

    // QUIRK W-273: kept
    #[test]
    fn hero_index_is_the_first_position_or_minus_one() {
        // re/heroes-resources 1.3: first index for duplicates, unknown -1; synthetic is empty.
        let mut rules = Rules::synthetic("", 4);
        assert!(rules.heroes.is_empty());
        assert_eq!(rules.hero_index(1200), -1);
        rules.heroes = vec![1200, 1201, 1200, 1112];
        assert_eq!(rules.hero_index(1200), 0);
        assert_eq!(rules.hero_index(1201), 1);
        assert_eq!(rules.hero_index(1112), 3);
        assert_eq!(rules.hero_index(1100), -1);
    }

    #[test]
    fn synthetic_rules_start_without_vehicle_nodes() {
        let rules = Rules::synthetic("", 8);
        assert!(rules.cannon_nodes.is_empty());
        assert!(rules.train_nodes.is_empty());
    }

    #[test]
    fn the_tile_terrain_averages_four_corners_and_keeps_above_water() {
        // Flat terrain at height 0, water -100 -> 0 per tile; water above the terrain raises it.
        let mut rules = Rules::synthetic("", 8);
        let t = TileTerrain::build(&rules.terrain, &rules.map);
        assert_eq!((t.width, t.height, t.water), (8, 8, -3200));
        assert!(t.heights.iter().all(|&h| h == 0));
        assert_eq!((t.at(-1, 0), t.at(8, 0), t.at(7, 7)), (0, 0, 0));
        rules.map.water_level = 64.0;
        let t = TileTerrain::build(&rules.terrain, &rules.map);
        assert_eq!(t.water, 2048);
        assert!(t.heights.iter().all(|&h| h == 2048));
    }

    // QUIRK K-36: kept
    #[test]
    fn config_values_fall_through_slot_and_zero_to_the_defaults() {
        let mut rules = Rules::synthetic("", 16);
        rules.defaults = ConfigSets::from([(1100, BTreeMap::from([(14, 120), (18, 32)]))]);
        rules.level.slot_configs = BTreeMap::from([
            (
                3,
                ConfigSets::from([(1100, BTreeMap::from([(14, 150), (18, 0)]))]),
            ),
            (8, ConfigSets::from([(3409, BTreeMap::from([(14, 70)]))])),
        ]);
        assert_eq!(rules.config_value(3, 1100, 14), 150);
        // 0 in the slot set cannot override the default.
        assert_eq!(rules.config_value(3, 1100, 18), 32);
        assert_eq!(rules.config_value(0, 1100, 14), 120);
        // Trees/resources (9, 10) see only default.dat; from 11 always 0.
        assert_eq!(rules.config_value(9, 3409, 14), 0);
        assert_eq!(rules.config_value(8, 3409, 14), 70);
        assert_eq!(rules.config_value(11, 1100, 14), 0);
        assert_eq!(rules.config_value(3, 4711, 14), 0);
        rules.set_anim_labels(1100, 1, &[(1, 1250)]);
        assert_eq!(rules.anim_label_time(1100, 1), 1250);
        assert_eq!(rules.anim_label_time(1100, -1), 0);
        assert_eq!(rules.anim_label_time(1100, 7), 0);
        assert_eq!(rules.ground_height(100.0, 100.0), 0.0);
        assert_eq!(rules.ground_height(-1.0, 100.0), -100.0);
    }

    #[test]
    fn upgrade_lookups_and_the_level_set_lock() {
        let mut rules = Rules::synthetic("", 8);
        assert!(!rules.is_upgrade(2800));
        assert!(rules.types_with_upgrade(2800).is_empty());
        assert!(rules.temp_upgrade(4450).is_none());
        assert_eq!(rules.level_daytime(), 0);
        rules.upgrades.insert(2800);
        rules.defaults = ConfigSets::from([(2801, BTreeMap::from([(1, 1), (13, 15)]))]);
        rules.upgrade_types.insert(2800, vec![1100, 1102]);
        assert!(rules.is_upgrade(2800) && rules.is_upgrade(2801) && !rules.is_upgrade(2840));
        assert_eq!(rules.types_with_upgrade(2800), [1100, 1102]);
        // Only an explicit value 0 for config 1 in the set of the slot locks; slots from 8 read
        // set 8 (`[NEUTRAL]`), a missing set or value means free.
        rules.level.slot_configs = BTreeMap::from([
            (3, ConfigSets::from([(2800, BTreeMap::from([(1, 0)]))])),
            (8, ConfigSets::from([(2801, BTreeMap::from([(1, 0)]))])),
        ]);
        assert!(rules.is_guid_disabled(3, 2800));
        assert!(!rules.is_guid_disabled(4, 2800));
        assert!(
            !rules.is_guid_disabled(3, 2801),
            "set of the slot without config 1"
        );
        assert!(rules.is_guid_disabled(8, 2801) && rules.is_guid_disabled(10, 2801));
        assert!(!rules.is_guid_disabled(0, 2840));
        rules.level.level_resources = BTreeMap::from([(40, None)]);
        assert_eq!(rules.level_daytime(), 0);
        rules.level.level_resources.insert(40, Some(3));
        assert_eq!(rules.level_daytime(), 3);
    }

    #[test]
    fn anim_labels_answer_every_label_id() {
        let mut r = Rules::synthetic(
            "GUID 1107
NAME SPA_Saebelkaempfer
VALUEID 100 6 1 0
",
            16,
        );
        r.set_anim_labels(1107, 2, &[(1, 1200), (2, 320)]);
        r.set_anim_labels(1107, 0, &[(2, 7)]);
        assert_eq!(r.anim_label_time_at(1107, 2, 1), 1200);
        assert_eq!(r.anim_label_time_at(1107, 2, 2), 320);
        assert_eq!(
            r.anim_label_time_at(1107, 2, 0x16),
            0,
            "missing marker -> 0"
        );
        assert_eq!(
            r.anim_label_time_at(1107, 1, 1),
            0,
            "gap between 0 and 2 is empty"
        );
        assert_eq!(r.anim_label_time_at(1107, 0, 2), 7);
        assert_eq!(r.anim_label_time_at(1107, 9, 1), 0);
        assert_eq!(r.anim_label_time_at(1107, -1, 1), 0);
        assert_eq!(r.anim_label_time(1107, 2), 1200);
    }

    /// Corpus: bonus table, priorities, time markers and rank upgrades from the real data
    /// (re/combat-damage 7.3, re/combat-melee 8.1, re/combat-death 2.4).
    #[test]
    fn corpus_combat_rules_have_bonuses_priorities_labels_and_rank_upgrades() {
        use opennml_rda::Vfs;
        use opennml_rda::testutil::corpus_game_dir;
        use opennml_world::level::load_level;
        let Some(dir) = corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        let scene = load_level(&vfs, "data/levels/multiplayer/[4players]-exile.lvl").unwrap();
        let r = Rules::load(&vfs, &scene).unwrap();
        use opennml_formats::bonus::*;
        assert_eq!(r.bonus.attack_bonus(ART_SWORD_PEEK), 40);
        assert_eq!(r.bonus.attack_bonus(ART_UNIT_BUILDING), -20);
        assert_eq!(r.bonus.attack_bonus(ART_MELEE_MELEEARTILLERY), 25);
        assert_eq!(r.bonus.mounty, [(18, 20)].into_iter().collect());
        assert_eq!(r.priorities.len(), 12);
        assert!(
            r.mesh_names.get(&1402).is_some_and(|n| n.contains("pfeil")),
            "WID_Bogenschuetze: mesh pfeil (110007)"
        );
        assert_eq!(
            (r.tile_terrain.width, r.tile_terrain.height),
            (r.map.dim_x, r.map.dim_z)
        );
        let l = |g, a, id| r.anim_label_time_at(g, a, id);
        // Saber fighter: animation 2 (combat: end 1200, Hit1 320, no Hit2), animation 3 (dying: 1280).
        assert_eq!(
            (l(1107, 2, 1), l(1107, 2, 2), l(1107, 2, 0x16)),
            (1200, 320, 0)
        );
        assert_eq!(l(1107, 3, 1), 1280);
        // Warrior 14: 1283/560/1096; Quirk: Continental fighter 6: end 667 without Hit1.
        assert_eq!(
            (l(1400, 14, 1), l(1400, 14, 2), l(1400, 14, 0x16)),
            (1283, 560, 1096)
        );
        assert_eq!((l(2308, 6, 1), l(2308, 6, 2)), (667, 0));
        assert_eq!(l(2006, 3, 1), 933, "infantryman dies in 933 ms");
        // Rank upgrades (config 114 HP, 119 melee, 116 speed, 121 range).
        let up = |guid: u32, cfg: u32| r.defaults.get(&guid).and_then(|s| s.get(&cfg)).copied();
        assert_eq!((up(4400, 114), up(4400, 119)), (Some(5), Some(5)));
        assert_eq!(
            (up(4404, 114), up(4404, 116), up(4404, 121)),
            (Some(50), Some(20), Some(40))
        );
    }

    #[test]
    fn fnv_hash_matches_the_reference_values() {
        let mut h = Fnv64::default();
        assert_eq!(h.0, 0xcbf2_9ce4_8422_2325);
        h.write(b"a");
        assert_eq!(h.0, 0xaf63_dc4c_8601_ec8c);
        let mut h = Fnv64::default();
        h.write(b"foobar");
        assert_eq!(h.0, 0x8594_4171_f739_67e8);
    }
}
