//! Terrain classification per tile (`GameEngine_ClassifyTileTerrain` `0x00431AB0`, re/path-grid 2): height bands
//! around the water level, steepness from the two triangle normals and the texture bit "impassable". The class
//! depends only on the terrain and is kept in the rules; it is applied on loading and on every release of a tile.

use glam::Vec3;
use opennml_world::terrain::{Terrain, tile_face_normals};

use super::grid::PathGrid;
use crate::occupancy::OccupancyGrid;

/// Bits of a tile class.
pub mod class {
    /// Highest corner height above the water level (`h > W`): layer 4 blocked.
    pub const ABOVE_WATER: u8 = 0x01;
    /// Above the deep-water steepness limit (`W − 0.5615·Y < h`): layers 0-3 by steepness.
    pub const STEEP_CHECKED: u8 = 0x02;
    /// Steep (`−normal·y ≤ PATH_NORMAL`, also NaN).
    pub const STEEP: u8 = 0x04;
    /// Below the wading limit (`W − 0.4395·Y > h`): layers 0 and 2 blocked.
    pub const DEEP: u8 = 0x08;
    /// Under water (`h < W`): layers 1, 3, 4 free.
    pub const UNDER_WATER: u8 = 0x10;
    /// Texture bit 2 "impassable": layers 0-4 blocked.
    pub const IMPASSABLE: u8 = 0x20;
    /// Not nearly flat (averaged normal `≤ 0.999`): occupancy bit on loading.
    pub const NOT_FLAT: u8 = 0x40;
    /// Texture bit 4 "not buildable": occupancy bit on loading.
    pub const NO_BUILD: u8 = 0x80;
}

/// `Fixed_ToFloat(0xFFFFF704)` = -2300/4096 and `Fixed_ToFloat(0xFFFFF8F8)` = -1800/4096 (exact).
const STEEP_LIMIT: f32 = -2300.0 / 4096.0;
const DEEP_LIMIT: f32 = -1800.0 / 4096.0;
/// `0x00618B30`.
const FLAT: f32 = 0.999;

/// `Terrain_GetTileMaxCornerHeight` (`0x0055AC40`): corners (x,z), (x,z+1), (x+1,z), (x+1,z+1).
fn max_corner(t: &Terrain, x: usize, z: usize) -> f32 {
    let mut h = t.vertex(x, z).position.y;
    for (cx, cz) in [(x, z + 1), (x + 1, z), (x + 1, z + 1)] {
        let c = t.vertex(cx, cz).position.y;
        if h < c {
            h = c;
        }
    }
    h
}

/// Class of each tile (z outer, x inner); `texture_bits` supplies `TEX_BITS` for the texture GUID of the tile
/// (unknown -> 0). `Y` = `TERRAIN_SIZE_X` (`g_nWorldCellSizeY`).
pub fn classify_tiles(terrain: &Terrain, texture_bits: impl Fn(Option<u32>) -> u32) -> Vec<u8> {
    let p = &terrain.params;
    let w = p.water_level;
    let y = p.size_x as i32 as f32;
    let steep_limit = w + STEEP_LIMIT * y;
    let deep_limit = w + DEEP_LIMIT * y;
    let (dim_x, dim_z) = (terrain.dim_x(), terrain.dim_z());
    let mut out = Vec::with_capacity(dim_x * dim_z);
    for z in 0..dim_z {
        for x in 0..dim_x {
            let h = max_corner(terrain, x, z);
            let mut c = 0;
            if h > w {
                c |= class::ABOVE_WATER;
            }
            if steep_limit < h {
                c |= class::STEEP_CHECKED;
                let v = |dx: usize, dz: usize| -> Vec3 { terrain.vertex(x + dx, z + dz).position };
                let (a, b) = tile_face_normals(v(0, 0), v(0, 1), v(1, 0), v(1, 1));
                let avg_y = (a.y + b.y) * 0.5;
                let dot = -avg_y;
                // `≤` including NaN (the x87 comparison jumps along on "unordered").
                if dot.is_nan() || dot <= p.path_normal {
                    c |= class::STEEP;
                }
                if dot.is_nan() || dot <= FLAT {
                    c |= class::NOT_FLAT;
                }
            }
            if deep_limit > h {
                c |= class::DEEP;
            }
            if h < w {
                c |= class::UNDER_WATER;
            }
            let bits = texture_bits(terrain.tile(x, z));
            if bits & 2 != 0 {
                c |= class::IMPASSABLE;
            }
            if bits & 4 != 0 {
                c |= class::NO_BUILD;
            }
            out.push(c);
        }
    }
    out
}

/// The write accesses of a tile in the order of the original: (layer mask, blocked).
fn writes(c: u8) -> impl Iterator<Item = (u32, bool)> {
    [
        (c & class::ABOVE_WATER != 0).then_some((0x10, true)),
        (c & class::STEEP_CHECKED != 0).then_some((0x0F, c & class::STEEP != 0)),
        (c & class::DEEP != 0).then_some((0x05, true)),
        (c & class::UNDER_WATER != 0).then_some((0x1A, false)),
        (c & class::IMPASSABLE != 0).then_some((0x1F, true)),
    ]
    .into_iter()
    .flatten()
}

/// Occupancy bit on loading (steps 1-5 of the classification, the last one wins).
fn occupied(c: u8) -> Option<bool> {
    let mut v = None;
    if c & class::ABOVE_WATER != 0 {
        v = Some(false);
    }
    if c & class::STEEP_CHECKED != 0 {
        v = Some(c & class::NOT_FLAT != 0);
    }
    if c & class::UNDER_WATER != 0 {
        v = Some(true);
    }
    if c & class::NO_BUILD != 0 {
        v = Some(true);
    }
    v
}

/// `GameEngine_ClassifyTileTerrain(x, z, false, false)` via `PathPlanner_MarkCell` - so when releasing a tile
/// (`GameEngine_MarkTileForLayers`): writes **all** base layers according to the terrain, blocks of other objects
/// also cancel out (weakness S7), without an occupancy bit.
pub fn reclassify_tile(grid: &mut PathGrid, classes: &[u8], x: i32, z: i32) {
    if !grid.inside(x, z) {
        return;
    }
    let c = classes[(grid.width * z + x) as usize];
    for (mask, blocked) in writes(c) {
        grid.mark_cell(mask, x, z, blocked);
    }
}

/// `GameEngine_ClassifyAllTiles` (`0x00432140`) on loading: classify every tile (raw before the region build), set
/// occupancy bits, flush at the end. The derived layers are computed once across the whole map; that yields the same
/// as the original's flushes on every mask change, because every tile is written and before the region build only
/// the last result counts.
pub fn classify_all(grid: &mut PathGrid, occupancy: &mut OccupancyGrid, classes: &[u8]) {
    assert!(
        !grid.built,
        "classification on loading before the region build"
    );
    for z in 0..grid.height {
        for x in 0..grid.width {
            let c = classes[(grid.width * z + x) as usize];
            for (mask, blocked) in writes(c) {
                grid.set_raw(mask, x, z, blocked);
            }
            if let Some(v) = occupied(c) {
                occupancy.set(x, z, v);
            }
        }
    }
    grid.mark_whole_map_dirty();
    grid.flush_dirty_rect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::BLOCKED;
    use opennml_formats::hlm::{Heightmap, HlmPoint};
    use opennml_formats::prj::Project;
    use opennml_formats::tlm::TextureMap;
    use opennml_formats::wld::TerrainParams;

    /// 4 x 1 tiles: heights of the grid points per column (both rows equal).
    fn strip(heights: [f32; 5], water: f32, guids: [u32; 4]) -> Terrain {
        let params = TerrainParams {
            dim_x: 4,
            dim_z: 1,
            chunk_size: 1,
            water_level: water,
            path_normal: 0.7,
            ..TerrainParams::default()
        };
        let points = (0..2)
            .flat_map(|z| {
                (0..5).map(move |x| HlmPoint {
                    position: [x as f32 * 128.0, heights[x], z as f32 * 128.0],
                    normal: [0.0, 1.0, 0.0],
                    color_guid: None,
                })
            })
            .collect();
        let hm = Heightmap {
            verts_x: 5,
            verts_z: 2,
            points,
        };
        let tm = TextureMap {
            dim_x: 4,
            dim_z: 1,
            guids: guids.to_vec(),
        };
        let mut project = Project::default();
        for g in guids {
            project.textures.insert(
                g,
                opennml_formats::prj::ProjectTexture {
                    guid: g,
                    name: String::new(),
                    file: String::new(),
                    bits: 0,
                    group: 0,
                    map_color: 0,
                },
            );
        }
        Terrain::new(&params, &hm, &tm, &project).unwrap()
    }

    #[test]
    fn height_bands_and_steepness_follow_the_original_thresholds() {
        use class::*;
        // Tile 0 flat above water, 1 steep (height jump 200 over 128), 2 shallow (-50),
        // 3 deep (-100); water at 0.
        let t = strip([10.0, 10.0, 210.0, -50.0, -100.0], 0.0, [1, 2, 3, 4]);
        let bits = |g: Option<u32>| if g == Some(4) { 4 } else { 0 };
        let c = classify_tiles(&t, bits);
        assert_eq!(c[0], ABOVE_WATER | STEEP_CHECKED);
        assert_eq!(c[1], ABOVE_WATER | STEEP_CHECKED | STEEP | NOT_FLAT);
        // Highest corner 210 (tile 2 has 210 on the left): above water and steep.
        assert_eq!(c[2] & (ABOVE_WATER | STEEP), ABOVE_WATER | STEEP);
        // Tile 3: corners -50 and -100 -> shallow (>= W - 56.25), under water, not buildable.
        assert_eq!(
            c[3] & (UNDER_WATER | DEEP | NO_BUILD),
            UNDER_WATER | NO_BUILD
        );
        let mut grid = PathGrid::new(4, 1);
        let mut occ = OccupancyGrid::new(4, 1);
        classify_all(&mut grid, &mut occ, &c);
        let cell = |l: usize, x: i32| grid.layers[l].cell(x, 0) == BLOCKED;
        assert!(!cell(0, 0) && cell(4, 0));
        assert!(cell(0, 1) && cell(3, 1) && cell(4, 1));
        // Shallow: land blocked (steep), swimming layers 1/3 and water free.
        assert!(!cell(1, 3) && !cell(4, 3));
        assert_eq!(occ.cells, [0, 1, 1, 1]);
        assert_eq!(grid.dirty, Default::default());
    }

    #[test]
    fn deep_water_blocks_wading_layers_and_freeing_reclassifies() {
        use class::*;
        let t = strip([-200.0, -200.0, -200.0, -200.0, -200.0], 0.0, [1, 1, 1, 1]);
        let c = classify_tiles(&t, |_| 0);
        assert_eq!(c[0], DEEP | UNDER_WATER);
        let mut grid = PathGrid::new(4, 1);
        let mut occ = OccupancyGrid::new(4, 1);
        classify_all(&mut grid, &mut occ, &c);
        assert_eq!(grid.layers[0].cell(0, 0), BLOCKED);
        assert_eq!(grid.layers[1].cell(0, 0), 0);
        // An object blocks everything, the release restores the terrain.
        grid.mark_cell(0x1F, 2, 0, true);
        grid.mark_cell(0x0F, 2, 0, false);
        reclassify_tile(&mut grid, &c, 2, 0);
        grid.flush_dirty_rect();
        assert_eq!(grid.layers[0].cell(2, 0), BLOCKED);
        assert_eq!(grid.layers[3].cell(2, 0), 0);
        assert_eq!(grid.layers[4].cell(2, 0), 0);
    }
}
