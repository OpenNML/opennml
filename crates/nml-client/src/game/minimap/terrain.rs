//! Terrain of the minimap (`Minimap_Construct` `0x004E4B40...` with `Minimap_RenderTerrain` `0x004E3790`).
//!
//! The original calculates the terrain only when loading the level into a color buffer and an image (D-21: a later
//! change of light does not change the map). The reimplementation calculates without a GPU in 8 bits per channel
//! (deviation from the original, O-294) and additionally delivers an [`RgbaImage`] for the presentation.

use glam::Vec3;
use image::RgbaImage;
use nml_world::level::LevelScene;
use nml_world::terrain::Terrain;

/// Color of a water tile as `0x00BBGGRR`: (0, 0, `0xC4`) (O-286).
const WATER_COLOR: u32 = 0x00C4_0000;

/// Terrain of the minimap: color per tile and image.
#[derive(Debug, Clone)]
pub struct MinimapTerrain {
    pub dim_x: u32,
    pub dim_z: u32,
    /// Color per tile (index `z * dim_x + x`) as `0x00BBGGRR`, the source of the fog. A tile without a known texture
    /// stays unwritten (0; in the original heap content, D-38).
    pub colors: Vec<u32>,
    /// Image `dim_x x dim_z`, top row = largest z.
    pub image: RgbaImage,
}

impl MinimapTerrain {
    /// Builds the terrain of a loaded level from map, project and the light of the `.wld`.
    ///
    /// Decisive is the light of the `.wld` (`LevelScene::world.terrain`), not `Terrain::params`: the original builds
    /// the map when loading (`World_ParseNextBlock`), before every change of light and before the start light of the
    /// scripts (D-21); `Terrain::params` may already carry a start light when the map is built.
    pub fn of_level(scene: &LevelScene) -> Self {
        let params = &scene.world.terrain;
        let map_colors = |guid: u32| scene.project.textures.get(&guid).map(|t| t.map_color);
        Self::build(
            &scene.terrain,
            &map_colors,
            Vec3::from(params.light_position),
            params.light_intensity,
            params.water_level,
        )
    }

    /// `Minimap_RenderTerrain` (`0x004E3790`), called only when loading (`Minimap_Construct`, D-21).
    ///
    /// - `map_colors`: `TEX_GUID` -> `TEX_MAP_COLOR` (COLORREF `0x00BBGGRR`); `None` = texture unknown
    ///   (`Project_FindTexture` returns null, D-38).
    /// - `light_position`: direction to the light (`Terrain+0x64`); `build` normalizes it like `Vector3_Normalize`
    ///   at the start of the function.
    /// - `intensity`: `LIGHT_INTENSITY` as an integer (`FILD [Terrain+0x74]`).
    ///
    /// For each tile: the original first looks for the texture. If it is missing, it writes the raw pixel `0xFF` in
    /// blue and leaves the color buffer unwritten (D-38). Otherwise the tile is water if all four corners are below
    /// `water_level` (O-286), otherwise land with `shade(map_color, f)` and
    /// `f = dot(n_bar, L_hat) + (1.0 - 0.01 * intensity)`; `n_bar` is the non-normalized mean of the four corner
    /// normals.
    pub fn build(
        terrain: &Terrain,
        map_colors: &impl Fn(u32) -> Option<u32>,
        light_position: Vec3,
        intensity: i32,
        water_level: f32,
    ) -> Self {
        let (dim_x, dim_z) = (terrain.dim_x(), terrain.dim_z());
        let light_dir = light_position.normalize_or_zero();
        // `FILD`, `FMUL [0x0061623C]`, `FSUBR [0x00615714]`: first 1 - 0.01 * I, then added to the dot product.
        let offset = 1.0 - 0.01 * intensity as f32;
        let mut colors = vec![0u32; dim_x * dim_z];
        let mut image = RgbaImage::new(dim_x as u32, dim_z as u32);
        for z in 0..dim_z {
            for x in 0..dim_x {
                let row = (dim_z - 1 - z) as u32;
                let color = terrain.tile(x, z).and_then(map_colors);
                let Some(color) = color else {
                    // Deviation: alpha 255 instead of 0, because the UI pass always blends; without a visible
                    // effect, the original draws the map without blending.
                    image.put_pixel(x as u32, row, image::Rgba([0, 0, 0xFF, 255]));
                    continue;
                };
                let corners = [
                    terrain.vertex(x, z),
                    terrain.vertex(x + 1, z),
                    terrain.vertex(x + 1, z + 1),
                    terrain.vertex(x, z + 1),
                ];
                let (rgb, packed) = if corners.iter().all(|v| v.position.y < water_level) {
                    ([0, 0, 0xC4], WATER_COLOR)
                } else {
                    let sum = ((corners[0].normal + corners[1].normal) + corners[2].normal)
                        + corners[3].normal;
                    let average = sum / 4.0;
                    let f = average.dot(light_dir) + offset;
                    let base = nml_formats::prj::colorref_rgb(color);
                    let rgb = shade(base, f);
                    (
                        rgb,
                        u32::from(rgb[2]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[0]),
                    )
                };
                colors[z * dim_x + x] = packed;
                image.put_pixel(x as u32, row, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
            }
        }
        Self {
            dim_x: dim_x as u32,
            dim_z: dim_z as u32,
            colors,
            image,
        }
    }
}

/// Dampens a base color with the light factor `f` (`0x004E3A26...0x004E3A42`): per channel `ftol(c * f)`; the result
/// is compared unsigned with the original value (`JBE`) and, if it is larger (also for negative `f`), replaced by
/// the original value. So the map never becomes brighter than the texture (D-38).
pub fn shade(base: [u8; 3], f: f32) -> [u8; 3] {
    base.map(|c| {
        let value = (f32::from(c) * f) as i32 as u32;
        if value > u32::from(c) { c } else { value as u8 }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::prj::{Project, ProjectTexture};
    use nml_formats::testutil::flat_heightmap;
    use nml_formats::tlm::TextureMap;
    use nml_formats::wld::TerrainParams;

    /// Flat terrain `dim x dim` at height `height`; tile `(x, z)` carries the texture `guid(x, z)`
    /// (0 = unknown, not in the project).
    fn flat_terrain(dim: u32, height: f32, guid: impl Fn(usize, usize) -> u32) -> Terrain {
        let params = TerrainParams {
            dim_x: dim,
            dim_z: dim,
            chunk_size: dim.min(16),
            ..TerrainParams::default()
        };
        let d = dim as usize;
        let guids: Vec<u32> = (0..d * d).map(|i| guid(i % d, i / d)).collect();
        let mut project = Project::default();
        for &g in guids.iter().filter(|&&g| g != 0) {
            project.textures.insert(
                g,
                ProjectTexture {
                    guid: g,
                    name: String::new(),
                    file: String::new(),
                    bits: 0,
                    group: 0,
                    map_color: 0,
                },
            );
        }
        let map = TextureMap {
            dim_x: d,
            dim_z: d,
            guids,
        };
        Terrain::new(
            &params,
            &flat_heightmap(dim, dim, 128.0, height),
            &map,
            &project,
        )
        .unwrap()
    }

    /// Light from straight above.
    const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

    // QUIRK O-286: kept
    #[test]
    fn water_tile_is_blue_c4() {
        let terrain = flat_terrain(4, -10.0, |_, _| 1);
        let map = MinimapTerrain::build(&terrain, &|_| Some(0x00FF_FFFF), UP, 0, 0.0);
        assert!(map.colors.iter().all(|&c| c == 0x00C4_0000));
        assert!(map.image.pixels().all(|p| p.0 == [0, 0, 0xC4, 255]));
        // If every corner is at the water level, it is land.
        let terrain = flat_terrain(4, 0.0, |_, _| 1);
        let map = MinimapTerrain::build(&terrain, &|_| Some(0x0000_0080), UP, 0, 0.0);
        assert!(map.colors.iter().all(|&c| c == 0x0000_0080));
    }

    // QUIRK D-38: kept
    #[test]
    fn shade_never_brightens_and_keeps_base_for_negative_f() {
        assert_eq!(shade([200, 100, 50], 0.8), [160, 80, 40]);
        assert_eq!(shade([200, 100, 50], 1.3), [200, 100, 50]);
        assert_eq!(shade([200, 100, 50], -0.2), [200, 100, 50]);
        assert_eq!(shade([200, 100, 50], 1.0), [200, 100, 50]);
    }

    // QUIRK D-38: kept
    #[test]
    fn missing_texture_gives_raw_ff_and_leaves_colour_buffer() {
        let terrain = flat_terrain(3, 100.0, |x, _| if x == 1 { 0 } else { 1 });
        let map = MinimapTerrain::build(&terrain, &|_| Some(0x0000_0040), UP, 0, 0.0);
        for z in 0..3usize {
            assert_eq!(map.colors[z * 3 + 1], 0, "color buffer stays unwritten");
            assert_eq!(map.colors[z * 3], 0x0000_0040);
            // The raw pixel: blue with alpha 255 (in the original alpha 0 without blending).
            assert_eq!(map.image.get_pixel(1, 2 - z as u32).0, [0, 0, 0xFF, 255]);
            assert_eq!(map.image.get_pixel(0, 2 - z as u32).0, [0x40, 0, 0, 255]);
        }
        // A GUID that the project knows but `map_colors` does not counts as unknown too.
        let map = MinimapTerrain::build(&terrain, &|_| None, UP, 0, 0.0);
        assert!(map.colors.iter().all(|&c| c == 0));
    }

    // QUIRK O-286: kept
    #[test]
    fn top_row_is_largest_z() {
        // Texture GUID = z + 1, color = 10 * GUID in red.
        let terrain = flat_terrain(4, 100.0, |_, z| z as u32 + 1);
        let map = MinimapTerrain::build(&terrain, &|g| Some(g * 10), UP, 0, 0.0);
        assert_eq!((map.image.width(), map.image.height()), (4, 4));
        for z in 0..4usize {
            for x in 0..4usize {
                assert_eq!(map.colors[z * 4 + x], (z as u32 + 1) * 10);
                assert_eq!(
                    map.image.get_pixel(x as u32, 3 - z as u32).0,
                    [(z as u8 + 1) * 10, 0, 0, 255]
                );
            }
        }
    }

    // QUIRK O-286: kept
    #[test]
    fn light_factor_uses_the_averaged_unnormalised_normals() {
        // Flat terrain: n_bar = (0, 1, 0). Light oblique and given unnormalized: L_hat = (0, 0.6, 0.8),
        // dot = 0.6. Strength 20: f = 0.6 + 0.8 = 1.4 -> above 1, the base color stays.
        let terrain = flat_terrain(2, 100.0, |_, _| 1);
        let light = Vec3::new(0.0, 3.0, 4.0);
        let map = MinimapTerrain::build(&terrain, &|_| Some(0x0064_3C28), light, 20, 0.0);
        assert!(map.colors.iter().all(|&c| c == 0x0064_3C28));
        // Strength 80: f = 0.6 + 0.2 = 0.8 -> (40, 60, 100) * 0.8 = (32, 48, 80).
        let map = MinimapTerrain::build(&terrain, &|_| Some(0x0064_3C28), light, 80, 0.0);
        assert!(map.colors.iter().all(|&c| c == 0x0050_3020));
    }

    /// Tinteba River (64 x 64, light (800, 400, -1300), strength 14, water -150) with the values of a preliminary
    /// check (reader program following the formula of `0x004E3790`). The editions agree in the shipped data.
    // QUIRK D-21, O-286: kept
    #[test]
    fn corpus_minimap_terrain_of_a_level() {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let scene = nml_world::level::load_level(
            &vfs,
            "data/levels/multiplayer/[2players]-tintebariver.lvl",
        )
        .unwrap();
        let map = MinimapTerrain::of_level(&scene);
        assert_eq!((map.dim_x, map.dim_z), (64, 64));
        assert_eq!(map.colors.len(), 64 * 64);
        assert_eq!((map.image.width(), map.image.height()), (64, 64));
        let at = |x: usize, z: usize| map.colors[z * 64 + x];
        assert_eq!(at(10, 10), 0x0022_5971);
        assert_eq!(at(53, 0), 0x00A2_D0E5);
        assert_eq!(at(54, 0), 0x0073_AAC7);
        assert_eq!(at(54, 1), 0x0081_A7B7);
        assert_eq!(at(57, 0), 0x00C4_0000);
        // Count water tiles; no tile without a texture (an entry 0 would otherwise be black in the buffer).
        let water = map.colors.iter().filter(|&&c| c == 0x00C4_0000).count();
        assert_eq!(water, 746);
        assert_eq!(
            scene.terrain.tiles.iter().filter(|t| t.is_none()).count(),
            0
        );
        // Image row 0 is z = 63, the last one z = 0.
        for x in 0..64usize {
            for (row, z) in [(0u32, 63usize), (63, 0)] {
                let c = at(x, z);
                let expected = [c as u8, (c >> 8) as u8, (c >> 16) as u8, 255];
                assert_eq!(map.image.get_pixel(x as u32, row).0, expected, "({x}, {z})");
            }
        }
        // The light of the map comes from the `.wld` (not from the light set later).
        assert_eq!(scene.world.terrain.light_intensity, 14);
        assert_eq!(scene.world.terrain.water_level, -150.0);
    }
}
