//! Generator of the random map (`RandomMap_Generate` `0x00546A50`): from seed text, map script and
//! menu values terrain, areas and random number consumption, bit-identical to the load path of the
//! original. See re/random-map.
//!
//! Flow (re/random-map Z.1, Z.6): seed and defines, parse the script (the preprocessor draws `#random`),
//! prepare areas (borders, quantities, `n` copies per `create_PlayerAreas`), then the terrain steps in
//! fixed order (D-328), then the objects (step 13), finally texture number -> GUID (step 14).
//! [`generate_load_path`] omits the object step like the load path of the original (D-341). Normals,
//! light and water alpha (steps 11, 15) are computed by `opennml_world::terrain::Terrain::new` in
//! [`to_scene`].

mod area;
mod checks;
mod geometry;
mod grid;
mod growth;
mod guids;
mod land;
mod level;
mod objects;
mod paint;
mod place;
mod relief;
mod rng;
mod scene;
mod spec;
mod terrain_areas;

use anyhow::{Context, bail};
use opennml_formats::mp_levels::{RandomMapTypes, parse_random_map_types};
use opennml_formats::prj::{Project, parse_project};
use opennml_formats::rnd::{RandomMapScript, parse_random_map_script};
use opennml_formats::text::decode_cp1252;
use opennml_formats::wld::TerrainParams;
use opennml_rda::Vfs;
use opennml_sim::fx::Fx;
use serde::{Deserialize, Serialize};

use crate::area::{Area, finish_area};
use crate::checks::ObjectCtx;
use crate::grid::Grid;
pub use crate::level::{RandomLevel, build_level, is_island_script, level_from_map, rules_input};
pub use crate::objects::MapObject;
use crate::rng::GenRand;
pub use crate::rng::{Phase, RandCounts};
pub use crate::scene::{random_map_label, to_scene};
pub use crate::spec::{
    RandomMapSpec, map_dim, scale_factor, script_and_defines, seed_hash, size_defines,
    split_defines,
};

/// Path of the map types (`FUN_004CC0D0`).
pub const RANDOM_MAP_TYPES_PATH: &str = "data/gifcon/randommaptypes.ini";

/// Tile size of the terrain (`FUN_0055CDD0(dim, dim, 128, 128, 16, 4)`, `SIZE_X`/`SIZE_Z`).
const TILE_SIZE: f32 = 128.0;
/// `CHUNK_SIZE` and `TEXTURE_TILING` from `FUN_0055CDD0(dim, dim, 128, 128, 16, 4)`.
const CHUNK_SIZE: u32 = 16;
const TEXTURE_TILING: u32 = 4;

/// Start position of a player area (`create_PlayerAreas`, Z.6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerArea {
    pub id: i32,
    pub cx: i32,
    pub cz: i32,
    pub min_size: i32,
}

/// Result of the generator. A savegame of a random map carries it whole, so that loading builds scene and rules input
/// without the generator ([`level_from_map`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RandomMap {
    pub spec: RandomMapSpec,
    /// Seed = [`seed_hash`] of the seed text.
    pub hash: u32,
    pub script_path: String,
    /// Defines from the menu (size, game mode, landscape, time of day); the `#define`s of the script
    /// are additionally in `script.defines`.
    pub defines: Vec<String>,
    pub script: RandomMapScript,
    /// Edge length in tiles (64/128/256).
    pub dim: usize,
    /// Terrain of the script with `dim_x`/`dim_z` of the map.
    pub terrain: TerrainParams,
    /// Heights of the grid points `(dim + 1)²`, row by row, x innermost.
    pub heights: Vec<f32>,
    /// Texture number per tile (-1 = none).
    pub tile_numbers: Vec<i32>,
    /// Texture GUID per tile (step 14, Z.6.12); if the project does not know the drawn GUID, the
    /// texture number stands there (D-335, -1 -> `0xFFFFFFFF`).
    pub tiles: Vec<u32>,
    /// Steepness flags per tile (Z.6.10).
    #[serde(with = "serde_bytes")]
    pub steep: Vec<u8>,
    /// Area ID per tile (`G+0x110`).
    pub id_grid: Vec<i32>,
    pub player_areas: Vec<PlayerArea>,
    pub rand_counts: RandCounts,
    /// State of the `rand()` sequence at the end of the generator.
    pub rand_state: u32,
    /// Placed objects in placement order (`RM+0x30`, step 13); empty in the load path.
    pub objects: Vec<MapObject>,
    /// Blocking map at the end of the object step: steepness plus footprints of the placed objects
    /// (D-337, D-349). Only for checksum and reference comparison; scene and rules do not read it.
    /// Empty in the load path.
    #[serde(with = "serde_bytes")]
    pub object_block: Vec<u8>,
    /// Slots 0...7 with a placed object of this owner (`RM+0x50`, `FUN_005473B0`).
    pub slots_used: [bool; 8],
}

/// Checksums of the result fields (FNV-1a 64, start value `0xcbf29ce484222325`, values little-endian;
/// heights as `f32::to_bits`). They record corpus values without putting game data into the repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digests {
    pub heights: u64,
    pub tile_numbers: u64,
    pub tiles: u64,
    pub steep: u64,
    pub id_grid: u64,
    /// Per object in placement order a 24-byte record, little-endian: GUID (u32), x, y, z
    /// (`f32::to_bits`), rotation (`f32::to_bits`), owner (i32).
    pub objects: u64,
    /// Bytes of the own blocking map at the end of the object step (steepness plus footprints of the objects).
    pub object_block: u64,
}

impl Digests {
    /// The five terrain checksums of the load path: heights, texture numbers, tile GUIDs,
    /// steepness, ID grid.
    pub fn terrain(&self) -> [u64; 5] {
        [
            self.heights,
            self.tile_numbers,
            self.tiles,
            self.steep,
            self.id_grid,
        ]
    }
}

/// FNV-1a 64 over the bytes.
fn fnv1a(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

impl RandomMap {
    /// Internal consistency of a map that does not come from the generator (savegame): edge length of the map
    /// size, the terrain layout the generator sets at the end and the lengths of the grids. Only what
    /// would otherwise make [`to_scene`] or the terrain panic or allocate without bound; no plausibility.
    pub fn check(&self) -> anyhow::Result<()> {
        let dim = map_dim(self.spec.size);
        if self.dim != dim {
            bail!(
                "random map: edge length {}, map size {} has {dim}",
                self.dim,
                self.spec.size
            );
        }
        let defaults = TerrainParams::default();
        let t = &self.terrain;
        let layout = (
            (t.dim_x, t.dim_z, t.size_x, t.size_z),
            (t.chunk_size, t.texture_tiling),
            (t.texture_base_size, t.texture_alpha_size),
        );
        let expected = (
            (dim as u32, dim as u32, TILE_SIZE, TILE_SIZE),
            (CHUNK_SIZE, TEXTURE_TILING),
            (defaults.texture_base_size, defaults.texture_alpha_size),
        );
        if layout != expected {
            bail!("random map: terrain layout {layout:?}, expected {expected:?}");
        }
        let tiles = dim * dim;
        for (name, len, want) in [
            ("heights", self.heights.len(), (dim + 1) * (dim + 1)),
            ("texture numbers", self.tile_numbers.len(), tiles),
            ("tile GUIDs", self.tiles.len(), tiles),
            ("steepness", self.steep.len(), tiles),
            ("ID grid", self.id_grid.len(), tiles),
        ] {
            if len != want {
                bail!("random map: {len} {name}, expected {want}");
            }
        }
        Ok(())
    }

    pub fn digests(&self) -> Digests {
        Digests {
            heights: fnv1a(self.heights.iter().flat_map(|h| h.to_bits().to_le_bytes())),
            tile_numbers: fnv1a(self.tile_numbers.iter().flat_map(|n| n.to_le_bytes())),
            tiles: fnv1a(self.tiles.iter().flat_map(|g| g.to_le_bytes())),
            steep: fnv1a(self.steep.iter().copied()),
            id_grid: fnv1a(self.id_grid.iter().flat_map(|id| id.to_le_bytes())),
            objects: fnv1a(self.objects.iter().flat_map(|o| {
                let mut bytes = Vec::with_capacity(24);
                bytes.extend(o.guid.to_le_bytes());
                for p in o.position {
                    bytes.extend(p.to_bits().to_le_bytes());
                }
                bytes.extend(o.rotation_y.to_bits().to_le_bytes());
                bytes.extend(o.owner.to_le_bytes());
                bytes
            })),
            object_block: fnv1a(self.object_block.iter().copied()),
        }
    }
}

/// Reads `data/gifcon/randommaptypes.ini`.
pub fn load_types(vfs: &Vfs) -> anyhow::Result<RandomMapTypes> {
    let bytes = vfs
        .read(RANDOM_MAP_TYPES_PATH)
        .with_context(|| format!("random map: {RANDOM_MAP_TYPES_PATH} not readable"))?;
    Ok(parse_random_map_types(&decode_cp1252(&bytes)))
}

/// Generates the random map from the files of the installation like the restart of the original:
/// terrain, objects (step 13, [`Phase::Objects`]), texture GUIDs. The reimplementation also generates
/// this way when loading a savegame (D-341, deviation).
pub fn generate(
    vfs: &Vfs,
    types: &RandomMapTypes,
    spec: &RandomMapSpec,
) -> anyhow::Result<RandomMap> {
    generate_with(&mut |path| vfs.read(path).ok(), types, spec)
}

/// Load path of the original (`Game_LoadLevelFromSavegame` -> `RandomMap_Generate` without step 13):
/// the terrain result, verified bit-identical against the original; `objects` and
/// `object_block` empty, `slots_used` free, `rand_counts.objects` 0. For `dump --load-path` and the
/// test `digests_per_script`.
pub fn generate_load_path(
    vfs: &Vfs,
    types: &RandomMapTypes,
    spec: &RandomMapSpec,
) -> anyhow::Result<RandomMap> {
    generate_from(&mut |path| vfs.read(path).ok(), types, spec, false)
}

/// [`generate`] with an arbitrary file source (`None` = file missing).
pub(crate) fn generate_with(
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    types: &RandomMapTypes,
    spec: &RandomMapSpec,
) -> anyhow::Result<RandomMap> {
    generate_from(read, types, spec, true)
}

/// Shared flow; `objects` = run step 13 (restart) or not (load path).
fn generate_from(
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    types: &RandomMapTypes,
    spec: &RandomMapSpec,
    objects: bool,
) -> anyhow::Result<RandomMap> {
    let mut generator = Generator::new(read, types, spec)?;
    // The original reads the project already while parsing (`ProjectFile`); if it is missing, it
    // continues silently (D-327). The reimplementation aborts with an error message before the terrain steps.
    let project = read_project(read, &generator.script)
        .with_context(|| format!("random map \"{}\" not generated", spec.seed_text))?;
    generator.terrain_steps()?;
    if objects {
        generator
            .object_step(read, &project)
            .with_context(|| format!("random map \"{}\" not generated", spec.seed_text))?;
    }
    generator.texture_guids(&project)?;
    Ok(generator.finish())
}

/// Project file from `ProjectFile` of the script. If the entry is missing, the name is empty (without
/// two `"`) or the file is not readable or not parsable -> error (D-327).
pub(crate) fn read_project(
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    script: &RandomMapScript,
) -> anyhow::Result<Project> {
    let Some(path) = script.project_file.as_deref() else {
        bail!("random map: the map script names no project file (ProjectFile)");
    };
    let Some(bytes) = read(path) else {
        bail!("random map: project file \"{path}\" not readable");
    };
    parse_project(&decode_cp1252(&bytes))
        .with_context(|| format!("random map: parsing project file \"{path}\""))
}

/// State of a run (the generator, `0x274` bytes, new per map, Z.5.1).
#[derive(Debug, Clone)]
pub(crate) struct Generator {
    spec: RandomMapSpec,
    hash: u32,
    script_path: String,
    defines: Vec<String>,
    script: RandomMapScript,
    dim: usize,
    /// Map factor (`G+0x268`, `FUN_0053FAC0`): computed once in [`Generator::new`], read again in
    /// the object step.
    scale: Fx,
    rand: GenRand,
    grid: Grid,
    land_areas: Vec<Area>,
    player_areas: Vec<Area>,
    terrain_areas: Vec<Area>,
    /// Steepness flags per tile (step 11), empty until then.
    steep: Vec<u8>,
    /// Texture GUID per tile (step 14), empty until then.
    tiles: Vec<u32>,
    /// Result of the object step (step 13), empty until then or in the load path.
    objects: Vec<MapObject>,
    object_block: Vec<u8>,
    slots_used: [bool; 8],
}

impl Generator {
    /// Everything before the terrain steps (`RandomMap_Generate` up to `0x00546F5A`): script path and
    /// defines, `srand(Hash)` (`0x00546AC6`), map factor (`FUN_0053FAC0`), parse the script
    /// (`FUN_00546930`) and finish the areas (end of `FUN_00543CA0`).
    pub(crate) fn new(
        read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
        types: &RandomMapTypes,
        spec: &RandomMapSpec,
    ) -> anyhow::Result<Self> {
        let (script_path, defines) = script_and_defines(spec, types)?;
        let hash = seed_hash(&spec.seed_text);
        let dim = map_dim(spec.size);
        let (_, size_index) = size_defines(dim);
        let scale = scale_factor(size_index);
        let mut rand = GenRand::new(hash);
        // If the main script is missing, the original returns 0 without a message and the start
        // continues on half a level (D-327); the reimplementation reports the error.
        let script =
            parse_random_map_script(&script_path, defines.clone(), read, &mut || rand.rand())
                .with_context(|| format!("random map \"{}\" not generated", spec.seed_text))?;
        for path in &script.missing_includes {
            log::warn!("Random map: include {path} is missing and is ignored");
        }
        let dim_i = dim as i32;
        let finish = |def| finish_area(def, dim_i, dim_i, spec.players, scale);
        let land_areas = script.land_areas.iter().map(finish).collect();
        // Each `create_PlayerAreas` block immediately creates `n` flat copies (`FUN_00540F70`, D-328).
        let player_areas = script
            .player_areas
            .iter()
            .flat_map(|def| {
                let area = finish(def);
                std::iter::repeat_n(area, spec.players as usize)
            })
            .collect();
        let terrain_areas = script.terrain_areas.iter().map(finish).collect();
        Ok(Self {
            spec: spec.clone(),
            hash,
            script_path,
            defines,
            script,
            dim,
            scale,
            rand,
            grid: Grid::new(dim, dim),
            land_areas,
            player_areas,
            terrain_areas,
            steep: Vec::new(),
            tiles: Vec::new(),
            objects: Vec::new(),
            object_block: Vec::new(),
            slots_used: [false; 8],
        })
    }

    /// The terrain steps after parsing in fixed order (`RandomMap_Generate` from
    /// `0x00546F5F`, D-328). The ID grid (step 3, `FUN_0053FA70`) is created by `Grid::new` with 0.
    pub(crate) fn terrain_steps(&mut self) -> anyhow::Result<()> {
        let players = self.spec.players;
        self.rand.set_phase(Phase::LandAreas);
        land::land_areas(
            &mut self.land_areas,
            &mut self.grid,
            &self.script,
            players,
            &mut self.rand,
        )?;
        self.rand.set_phase(Phase::PlayerAreas);
        land::player_areas(
            &mut self.player_areas,
            &mut self.grid,
            &self.script,
            players,
            &mut self.rand,
        )?;
        // Step 6: `BaseTexture` over all still unpainted tiles (`FUN_00541B20`).
        let base = paint::texture_number(&self.script, &self.script.base_texture);
        paint::base_texture(&mut self.grid, base);
        // Step 7: `connect_Players` (`FUN_00543800`) runs over the centers of **all** entries of the
        // player area list (count `G+0x48`, not the number of players), in list order.
        let centers: Vec<(i32, i32)> = self.player_areas.iter().map(|a| (a.cx, a.cz)).collect();
        paint::connect_players(&mut self.grid, &self.script, &centers)?;
        // Step 8: `create_TerrainArea` (`FUN_005466F0`); the player distance takes all entries of the
        // player area list (`G+0x3C`).
        self.rand.set_phase(Phase::TerrainAreas);
        let players = self.player_list();
        terrain_areas::terrain_areas(
            &mut self.terrain_areas,
            &mut self.grid,
            &self.script,
            &players,
            &mut self.rand,
        )?;
        // Steps 9 and 10: heights from the textures (`FUN_00540FD0`), smoothing (`FUN_00540AD0`).
        relief::heights_from_textures(&mut self.grid, &self.script);
        relief::smooth(&mut self.grid, self.script.smooth_factor)?;
        // Step 11: steepness (`Terrain_ComputeSteepTileFlags`); normals and light are computed later
        // by `Terrain::new` from the same positions.
        self.steep = relief::steep_flags(&self.grid, TILE_SIZE, self.script.terrain.path_normal);
        Ok(())
    }

    /// Step 13: objects (`FUN_005468D0`, restart only). Own blocking map as a copy of `steep`
    /// (`RandomMap::steep` stays the pure steepness), fresh rotation table (the
    /// original reads the project anew per generation, `ProjectFile_ParseObjects` `0x004FBE50`), the
    /// blocks via `place::place_objects` with the number of players `G+0x120` (`spec.players`) and the
    /// map factor `G+0x268`. `GroundTexture` paints into the working grid that step 14 reads.
    pub(crate) fn object_step(
        &mut self,
        read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
        project: &Project,
    ) -> anyhow::Result<()> {
        self.rand.set_phase(Phase::Objects);
        let scale = self.scale;
        let areas = self.player_list();
        let mut ctx = ObjectCtx::new(
            &mut self.grid,
            &self.script,
            &areas,
            &self.steep,
            project,
            read,
            TILE_SIZE,
        );
        place::place_objects(&mut ctx, self.spec.players, scale, &mut self.rand)?;
        self.object_block = ctx.block.cells().to_vec();
        self.objects = ctx.objects;
        self.slots_used = ctx.slots_used;
        Ok(())
    }

    /// Step 14: texture number -> GUID (`FUN_00542F60`). Step 12 (object and shadow light into the
    /// engine) is pure rendering.
    pub(crate) fn texture_guids(&mut self, project: &Project) -> anyhow::Result<()> {
        self.rand.set_phase(Phase::TextureGuids);
        self.tiles =
            guids::assign_texture_guids(&self.grid, &self.script, project, &mut self.rand)?;
        Ok(())
    }

    /// Centers, IDs and `minSize` of the player area list in list order.
    fn player_list(&self) -> Vec<PlayerArea> {
        self.player_areas
            .iter()
            .map(|area| PlayerArea {
                id: area.def.id,
                cx: area.cx,
                cz: area.cz,
                min_size: area.def.min_size,
            })
            .collect()
    }

    /// Result from the state of the run.
    pub(crate) fn finish(self) -> RandomMap {
        let mut terrain = self.script.terrain.clone();
        // `FUN_0055CDD0(dim, dim, 128, 128, 16, 4)`; the display sizes of the textures stay at the
        // defaults.
        terrain.dim_x = self.dim as u32;
        terrain.dim_z = self.dim as u32;
        terrain.size_x = TILE_SIZE;
        terrain.size_z = TILE_SIZE;
        terrain.chunk_size = CHUNK_SIZE;
        terrain.texture_tiling = TEXTURE_TILING;
        let player_areas = self.player_list();
        RandomMap {
            hash: self.hash,
            script_path: self.script_path,
            defines: self.defines,
            dim: self.dim,
            terrain,
            heights: self.grid.heights,
            tile_numbers: self.grid.tiles,
            tiles: self.tiles,
            steep: self.steep,
            id_grid: self.grid.id_grid,
            player_areas,
            rand_counts: self.rand.counts(),
            rand_state: self.rand.state(),
            objects: self.objects,
            object_block: self.object_block,
            slots_used: self.slots_used,
            script: self.script,
            spec: self.spec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::tests::{test_spec, test_types};
    use opennml_rda::normalize_path;
    use opennml_world::rand::MsvcRand;
    use std::collections::BTreeMap;

    /// Project file of the test scripts (`ProjectFile "Data\Projects\p.prj"`): textures 101 ... 103.
    const TEST_PRJ: &str = "PROJECTFILE V1.08f
[TEXTURE]
{
TEX_NAME \"Sea\"
TEX_GUID 101
}
[TEXTURE]
{
TEX_NAME \"Land\"
TEX_GUID 102
}
[TEXTURE]
{
TEX_NAME \"Road\"
TEX_GUID 103
}
";

    /// Files in memory, paths normalized as in the VFS; the project file is always included.
    fn files(list: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        list.iter()
            .chain(&[("data/projects/p.prj", TEST_PRJ)])
            .map(|(path, text)| (normalize_path(path), text.as_bytes().to_vec()))
            .collect()
    }

    fn run(files: &BTreeMap<String, Vec<u8>>, spec: &RandomMapSpec) -> anyhow::Result<Generator> {
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        Generator::new(&mut read, &test_types(), spec)
    }

    const SCRIPT: &str = "ProjectFile \"Data\\Projects\\p.prj\"
#include \"missing.inc\"
#random
#chance 50
BaseTexture A
#chance 50
BaseTexture B
#endrandom
#ifdef MAP_SIZE_SMALL
create_LandArea
{
Percent 60
LeftBorder 8
RightBorder 8
TopBorder 8
BottomBorder 8
}
#endif
create_PlayerAreas
{
minSize 3
ScaleToPlayers
}
create_TerrainArea
{
numClumps 2
ScaleClumpsToMapSize
}
";

    #[test]
    fn missing_main_script_is_an_error() {
        let error = run(&files(&[]), &test_spec()).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("maps/a.rnd is missing"), "{text}");
        assert!(text.contains("random map \"ZKA\""), "{text}");
    }

    // QUIRK D-328: kept
    #[test]
    fn generator_prepares_areas_and_counts_draws() {
        let spec = RandomMapSpec {
            players: 3,
            ..test_spec()
        };
        let generator = run(&files(&[("maps/a.rnd", SCRIPT)]), &spec).unwrap();
        assert_eq!(generator.hash, seed_hash("ZKA"));
        assert_eq!(
            generator.defines,
            ["MAP_SIZE_SMALL", "NORMAL", "MEADOW", "DAY"]
        );
        assert_eq!(generator.script.missing_includes, ["maps/missing.inc"]);
        // A `#random` draws exactly once from the seeded sequence.
        let mut plain = MsvcRand::new(seed_hash("ZKA"));
        let roll = plain.rand() % 100;
        assert_eq!(
            generator.script.base_texture,
            if roll < 50 { "A" } else { "B" }
        );
        assert_eq!(generator.rand.counts().preprocessor, 1);
        assert_eq!(generator.rand.state(), plain.state());
        // Areas: borders and quantities computed, `n` copies per player block.
        assert_eq!(generator.land_areas.len(), 1);
        assert_eq!(generator.land_areas[0].def.num_tiles, 1748);
        assert_eq!(generator.player_areas.len(), 3);
        assert!(generator.player_areas.iter().all(|a| a.def.min_size == 9));
        assert_eq!(generator.terrain_areas[0].def.num_clumps, 2);
        let map = generator.finish();
        assert_eq!(map.dim, 64);
        assert_eq!((map.terrain.dim_x, map.terrain.dim_z), (64, 64));
        assert_eq!(map.tile_numbers, vec![-1; 64 * 64]);
        assert_eq!(map.id_grid, vec![0; 64 * 64]);
        assert_eq!(map.heights, vec![0.0; 65 * 65]);
        assert_eq!(map.rand_counts.preprocessor, 1);
        assert_eq!(map.rand_state, plain.state());
    }

    // QUIRK D-328: kept
    #[test]
    fn generate_grows_land_and_player_areas() {
        let files = files(&[("maps/a.rnd", SCRIPT)]);
        let spec = RandomMapSpec {
            players: 3,
            ..test_spec()
        };
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        let map = generate_with(&mut read, &test_types(), &spec).unwrap();
        // Player areas: IDs 1, 2, 3 from the counter, `minSize 3 · 3`.
        let ids: Vec<i32> = map.player_areas.iter().map(|p| p.id).collect();
        assert_eq!(ids, [1, 2, 3]);
        assert!(map.player_areas.iter().all(|p| p.min_size == 9));
        for p in &map.player_areas {
            assert_eq!(map.id_grid[64 * p.cz as usize + p.cx as usize], p.id);
        }
        // Land area (ID 1) and players: 4*64 draws each for the rough border, plus the growth;
        // the angle counts toward the player areas.
        assert!(map.rand_counts.land_areas > 4 * 64);
        assert!(map.rand_counts.player_areas > 1 + 4 * 64);
        // Terrain area without `Replace`/`PutOnto`: target -1, and because the script knows no
        // `#Texture`, everything paints -1 - the first position attempt hits. Per clump x, z, test,
        // rough border 4*64 and the start tile (target `0 / 2`); two clumps.
        assert_eq!(map.rand_counts.terrain_areas, 2 * (3 + 4 * 64 + 2));
        // Land area 60% of the frame area (1748 tiles) grows completely before the players come.
        assert!(map.id_grid.iter().filter(|&&id| id != 0).count() >= 1748);
    }

    /// Script with textures, base texture and a connection on land (D-333).
    const PAINT_SCRIPT: &str = "ProjectFile \"Data\\Projects\\p.prj\"
#Texture SEA
{
GUID 101
}
#Texture LAND
{
GUID 102
GUID 999
}
#Texture ROAD
{
GUID 103
}
BaseTexture SEA
create_PlayerAreas
{
minSize 2
Texture LAND
}
connect_Players
{
Texture ROAD
PutOnto SEA
radius 0
}
";

    // QUIRK D-328: kept
    #[test]
    fn generate_paints_base_texture_and_connections() {
        let files = files(&[("maps/a.rnd", PAINT_SCRIPT)]);
        let spec = RandomMapSpec {
            players: 3,
            ..test_spec()
        };
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        let map = generate_with(&mut read, &test_types(), &spec).unwrap();
        // Numbers in script order: SEA 1, LAND 2, ROAD 3. Nothing stays -1.
        assert!(map.tile_numbers.iter().all(|&n| (1..=3).contains(&n)));
        let count = |n| map.tile_numbers.iter().filter(|&&v| v == n).count();
        assert!(count(2) > 0 && count(1) > 0);
        // The connection paints paths between the three centers onto SEA (ring, every center lies on
        // land): ROAD tiles exist, and each has sea as predecessor, never land.
        assert!(count(3) > 0);
        // The connections draw no randomness.
        assert_eq!(map.rand_counts.terrain_areas, 0);
        // Step 14: SEA and ROAD have a GUID (no draw); LAND draws per tile from
        // [102, 999], and the project does not know 999 -> the number 2 stays (D-335).
        let land_tiles = count(2) as u32;
        assert_eq!(map.rand_counts.texture_guids, land_tiles);
        for (&number, &guid) in map.tile_numbers.iter().zip(&map.tiles) {
            let expected: &[u32] = match number {
                1 => &[101],
                2 => &[102, 2],
                _ => &[103],
            };
            assert!(expected.contains(&guid), "{number} -> {guid}");
        }
        assert!(map.tiles.contains(&102) && map.tiles.contains(&2));
        // Terrain values from `FUN_0055CDD0(dim, dim, 128, 128, 16, 4)`.
        let t = &map.terrain;
        assert_eq!(
            (t.dim_x, t.dim_z, t.size_x, t.size_z),
            (64, 64, 128.0, 128.0)
        );
        assert_eq!((t.chunk_size, t.texture_tiling), (16, 4));
        assert_eq!(
            t.texture_base_size,
            TerrainParams::default().texture_base_size
        );
        let again = generate_with(&mut read, &test_types(), &spec).unwrap();
        assert_eq!(map, again);
        // Without the connection block the rest stays the same, only ROAD is missing and SEA lies there.
        let plain = PAINT_SCRIPT.replace("connect_Players", "#ifdef NEVER\nconnect_Players");
        let plain = format!("{plain}#endif\n");
        let files = files_of(&plain);
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        let without = generate_with(&mut read, &test_types(), &spec).unwrap();
        assert_eq!(without.tile_numbers.iter().filter(|&&v| v == 3).count(), 0);
        assert_eq!(without.rand_state, map.rand_state);
        for (a, b) in map.tile_numbers.iter().zip(&without.tile_numbers) {
            assert!(a == b || (*a == 3 && *b == 1));
        }
    }

    // QUIRK D-327: deviation
    #[test]
    fn missing_or_unreadable_project_is_an_error() {
        let spec = test_spec();
        let message = |script: &str| {
            let files = files_of(script);
            let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
            let error = generate_with(&mut read, &test_types(), &spec).unwrap_err();
            format!("{error:#}")
        };
        let text = message("BaseTexture A\n");
        assert!(text.contains("no project file"), "{text}");
        assert!(text.contains("random map \"ZKA\""), "{text}");
        // Without two `"` the name is empty - not readable.
        let text = message("ProjectFile data/projects/p.prj\n");
        assert!(text.contains("project file \"\" not readable"), "{text}");
        // Slashes start a comment (D-321) - the scripts name paths with `\`.
        let text = message("ProjectFile \"data/projects/p.prj\"\n");
        assert!(text.contains("project file \"\" not readable"), "{text}");
        let text = message("ProjectFile \"Data\\Projects\\missing.prj\"\n");
        assert!(
            text.contains("\"Data\\Projects\\missing.prj\" not readable"),
            "{text}"
        );
        // A `[TEXTURE]` without `TEX_GUID` makes the project file unreadable.
        let files = files(&[
            ("maps/a.rnd", "ProjectFile \"broken.prj\"\n"),
            ("broken.prj", "[TEXTURE]\n{\nTEX_NAME \"x\"\n}\n"),
        ]);
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        let text = format!(
            "{:#}",
            generate_with(&mut read, &test_types(), &spec).unwrap_err()
        );
        assert!(
            text.contains("parsing project file \"broken.prj\""),
            "{text}"
        );
    }

    #[test]
    fn digests_are_fnv1a_over_little_endian_values() {
        assert_eq!(fnv1a([]), 0xcbf2_9ce4_8422_2325);
        // Check value FNV-1a 64 of "a".
        assert_eq!(fnv1a(*b"a"), 0xaf63_dc4c_8601_ec8c);
        let files = files(&[("maps/a.rnd", RELIEF_SCRIPT)]);
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        let map = generate_with(&mut read, &test_types(), &test_spec()).unwrap();
        let digests = map.digests();
        let bytes = |v: Vec<u8>| fnv1a(v);
        assert_eq!(
            digests.heights,
            bytes(
                map.heights
                    .iter()
                    .flat_map(|h| h.to_bits().to_le_bytes())
                    .collect()
            )
        );
        assert_eq!(digests.steep, bytes(map.steep.clone()));
        let mut changed = map.clone();
        changed.tiles[0] ^= 1;
        assert_ne!(changed.digests().tiles, digests.tiles);
        assert_eq!(changed.digests().heights, digests.heights);
    }

    fn files_of(text: &str) -> BTreeMap<String, Vec<u8>> {
        files(&[("maps/a.rnd", text)])
    }

    /// Small map from [`RELIEF_SCRIPT`] (64x64, 2 players) for tests of other modules.
    pub(crate) fn relief_map() -> RandomMap {
        let files = files_of(RELIEF_SCRIPT);
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        generate_with(&mut read, &test_types(), &test_spec()).unwrap()
    }

    /// Textures with heights, mountain in the middle of the map (player area), sea around it.
    const RELIEF_SCRIPT: &str = "ProjectFile \"Data\\Projects\\p.prj\"
SmoothFactor 2
PathNormal 0.9
#Texture SEA
{
HEIGHT -150
GUID 101
}
#Texture MOUNTAIN
{
HEIGHT 600
GUID 102
}
BaseTexture SEA
create_PlayerAreas
{
minSize 3
Texture MOUNTAIN
}
";

    // QUIRK D-328: kept
    #[test]
    fn generate_computes_heights_smoothing_and_steepness() {
        let files = files(&[("maps/a.rnd", RELIEF_SCRIPT)]);
        let spec = RandomMapSpec {
            players: 2,
            ..test_spec()
        };
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        let map = generate_with(&mut read, &test_types(), &spec).unwrap();
        assert_eq!(map.heights.len(), 65 * 65);
        assert_eq!(map.steep.len(), 64 * 64);
        assert_eq!(map.script.smooth_factor, 2);
        // Mountain and sea exist next to each other after two smoothing passes; the flank is steep, the plain is not.
        assert!(map.heights.iter().any(|&h| h > 100.0));
        assert!(map.heights.iter().any(|&h| h < -100.0));
        assert!(map.steep.contains(&1));
        assert!(map.steep.contains(&0));
        assert!(map.steep.iter().all(|&s| s <= 1));
        // Repeatable bit-identically.
        let again = generate_with(&mut read, &test_types(), &spec).unwrap();
        let bits = |m: &RandomMap| m.heights.iter().map(|h| h.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&map), bits(&again));
        assert_eq!(map.steep, again.steep);
    }

    // QUIRK D-328: kept
    #[test]
    fn generation_is_repeatable() {
        let files = files(&[("maps/a.rnd", SCRIPT)]);
        let spec = RandomMapSpec {
            size: 2,
            players: 8,
            ..test_spec()
        };
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        let a = generate_with(&mut read, &test_types(), &spec).unwrap();
        let b = generate_with(&mut read, &test_types(), &spec).unwrap();
        assert_eq!(a, b);
        // Size 2: `MAP_SIZE_LARGE`, the land area is omitted; map factor 4 for the clumps.
        assert!(a.script.land_areas.is_empty());
        assert_eq!(a.defines[0], "MAP_SIZE_LARGE");
        assert_eq!(a.dim, 256);
    }

    // QUIRK D-337, D-349: kept
    #[test]
    fn object_block_is_steep_plus_rects() {
        // The blocking map of the object step is a copy of the steepness plus the footprint rectangles
        // of all placed objects; `RandomMap::steep` stays the pure steepness. `GroundTexture SEA`
        // paints the rectangles before step 14.
        let script = format!(
            "{RELIEF_SCRIPT}#Object CRATE\n{{\nGUID 500\n}}\nplace_Object CRATE\n{{\nnumber 6\nGroundTexture SEA\n}}\n"
        );
        let mut files = files_of(&script);
        let prj = format!(
            "{TEST_PRJ}[OBJECT]\n{{\nOBJ_NAME \"Crate\"\nOBJ_GUID 500\nOBJ_FILENAME \"Data\\crate.rdw\"\n}}\n"
        );
        files.insert("data/projects/p.prj".into(), prj.into_bytes());
        files.insert("data/crate.rdo".into(), crate::place::tests::box_rdo());
        let spec = RandomMapSpec {
            players: 2,
            ..test_spec()
        };
        let mut read = |path: &str| files.get(&normalize_path(path)).cloned();
        let map = generate_with(&mut read, &test_types(), &spec).unwrap();
        let load = generate_from(&mut read, &test_types(), &spec, false).unwrap();
        assert_eq!(map.objects.len(), 6);
        assert!(map.objects.iter().all(|o| o.guid == 500 && o.owner == 8));
        assert!(map.objects.iter().all(|o| o.position[1] == 0.0));
        assert_eq!(map.slots_used, [false; 8]);
        // Load path: no object step.
        assert!(load.objects.is_empty() && load.object_block.is_empty());
        assert_eq!(load.rand_counts.objects, 0);
        assert_eq!(load.digests().objects, fnv1a([]));
        assert!(map.rand_counts.objects >= 12);
        assert_eq!(
            map.rand_counts.terrain()[..4],
            load.rand_counts.terrain()[..4]
        );
        assert_eq!(map.steep, load.steep);
        assert_eq!(map.heights, load.heights);
        // Expected blocking map from the steepness and the rectangles of the objects.
        let geo = crate::geometry::load_geometry(&mut read, "data/crate.rdo").unwrap();

        let mut expected = map.steep.clone();
        let sea = map
            .script
            .textures
            .iter()
            .find(|t| t.name == "SEA")
            .unwrap()
            .number;
        for o in &map.objects {
            let m = opennml_sim::footprint::generator_world_matrix(
                o.position,
                [0.0, o.rotation_y, 0.0],
            );
            let rect = crate::geometry::ground_rect(&geo, &m, 128);
            for (x, z) in crate::checks::tiles_in_map(rect, 64, 64) {
                expected[64 * z + x] = 1;
                assert_eq!(map.tile_numbers[64 * z + x], sea);
            }
        }
        assert_eq!(map.object_block, expected);
        assert_ne!(map.object_block, map.steep);
        assert_ne!(map.digests().object_block, map.digests().steep);
    }
}
