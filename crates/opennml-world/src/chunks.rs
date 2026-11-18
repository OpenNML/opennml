//! Textures per rendering block like `TerrainChunk_BuildBaseTexture` (`0x0055CFF0`) and
//! `TerrainChunk_BuildAlphaMaps` (`0x0055D7F0`); rules in `docs/formats/tlm.md`.

use std::collections::{BTreeSet, HashMap};

use image::{GrayImage, Luma, Rgba, RgbaImage};
use opennml_formats::prj::Project;

use crate::terrain::Terrain;

/// A ground texture in the alpha pass with the block-local tiles (`a * C + b`, `a` = x) on
/// which it is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct SplatTexture {
    pub guid: u32,
    pub tiles: Vec<u16>,
}

/// A texture group: alpha map (`TEXTURE_ALPHA_XSIZE x _YSIZE`, row 0 = largest block z) and
/// its textures in drawing order.
#[derive(Debug, Clone, PartialEq)]
pub struct SplatLayer {
    pub group: u32,
    pub alpha: GrayImage,
    pub textures: Vec<SplatTexture>,
}

/// Textures of a block: base texture (`TEXTURE_BASE_XSIZE x _YSIZE`, row 0 = largest block z)
/// and blend layers in drawing order (groups ascending).
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkTextures {
    pub tile_x0: usize,
    pub tile_z0: usize,
    pub base: RgbaImage,
    pub layers: Vec<SplatLayer>,
}

impl Terrain {
    /// Tiles per block edge (`CHUNK_SIZE`).
    pub fn chunk_size(&self) -> usize {
        self.params.chunk_size as usize
    }

    /// Number of blocks (`Terrain_BuildChunks` `0x0055E640`).
    pub fn chunk_count(&self) -> usize {
        let c = self.chunk_size();
        (self.dim_x() / c) * (self.dim_z() / c)
    }

    /// Tile origin of block `chunk`: `x0 = (C*k) mod DIM_X`, `z0 = C * ((C*k) div DIM_X)`.
    pub fn chunk_origin(&self, chunk: usize) -> (usize, usize) {
        let c = self.chunk_size();
        ((c * chunk) % self.dim_x(), c * ((c * chunk) / self.dim_x()))
    }
}

/// `Texture_SampleTexel` (`0x00557750`): column `int(u * width)`, row
/// `height - 1 - int(v * height)` of the image (top first like `decode_tga_game`); held at the edge.
pub fn sample_texel(img: &RgbaImage, u: f32, v: f32) -> [u8; 4] {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return [0, 0, 0, 255];
    }
    let col = ((u * w as f32) as i64).clamp(0, w as i64 - 1) as u32;
    let row = (h as i64 - 1 - (v * h as f32) as i64).clamp(0, h as i64 - 1) as u32;
    img.get_pixel(col, row).0
}

/// Weight of a neighbor tile `(tx, tz)` (block-local) for the point `p` (block-local
/// tile units): `max(0, 1 - |p - center|^2 * 16/49)` (radius 1.75 tiles, `0.3265306`).
fn falloff(p: (f32, f32), tx: isize, tz: isize) -> f32 {
    let dx = p.0 - (tx as f32 + 0.5);
    let dz = p.1 - (tz as f32 + 0.5);
    (1.0 - (dx * dx + dz * dz) * 0.326_530_6).max(0.0)
}

/// Sum of the weights of the 3 x 3 neighbor tiles around `floor(p)` whose texture satisfies `matches`
/// (`Terrain_TextureBlendWeight` `0x0055B760` / `Terrain_GroupBlendWeight` `0x0055B8A0`).
fn blend_weight(
    terrain: &Terrain,
    origin: (usize, usize),
    p: (f32, f32),
    matches: impl Fn(u32) -> bool,
) -> f32 {
    let (bx, bz) = (p.0 as isize, p.1 as isize);
    let mut sum = 0.0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (lx, lz) = (bx + dx, bz + dz);
            let tile = terrain.tile_checked(origin.0 as isize + lx, origin.1 as isize + lz);
            if tile.is_some_and(&matches) {
                sum += falloff(p, lx, lz);
            }
        }
    }
    sum
}

/// Builds base and alpha textures as well as tile lists of a block. If an image is missing from `images`,
/// the texture contributes black (the game would then have loaded no texture).
pub fn build_chunk_textures(
    terrain: &Terrain,
    project: &Project,
    images: &HashMap<u32, RgbaImage>,
    chunk: usize,
) -> ChunkTextures {
    let c = terrain.chunk_size();
    let (x0, z0) = terrain.chunk_origin(chunk);
    let origin = (x0, z0);
    let group_of = |guid: u32| project.textures.get(&guid).map_or(0, |t| t.group);

    // Textures in the block plus a tile border (`TerrainChunk_CollectTextures`), ascending.
    let mut guids = BTreeSet::new();
    for z in z0 as isize - 1..=(z0 + c) as isize {
        for x in x0 as isize - 1..=(x0 + c) as isize {
            guids.extend(terrain.tile_checked(x, z));
        }
    }
    let groups: BTreeSet<u32> = guids.iter().map(|&g| group_of(g)).collect();

    // Base texture: per texel weighted blend of the textures, each sampled once over the block.
    let [bw, bh] = terrain.params.texture_base_size.map(|v| v as usize);
    let mut base = RgbaImage::new(bw as u32, bh as u32);
    for j in 0..bh {
        for i in 0..bw {
            let p = (
                i as f32 * c as f32 / bw as f32,
                j as f32 * c as f32 / bh as f32,
            );
            let weights: Vec<(u32, f32)> = guids
                .iter()
                .map(|&g| (g, blend_weight(terrain, origin, p, |t| t == g)))
                .filter(|&(_, w)| w > 0.0)
                .collect();
            let total: f32 = weights.iter().map(|&(_, w)| w).sum();
            let mut rgb = [0.0f32; 3];
            for &(g, w) in &weights {
                if let Some(img) = images.get(&g) {
                    let texel = sample_texel(img, i as f32 / bw as f32, j as f32 / bh as f32);
                    for (sum, value) in rgb.iter_mut().zip(texel) {
                        *sum += w / total * value as f32;
                    }
                }
            }
            let [r, g, b] = rgb.map(|v| v.clamp(0.0, 255.0) as u8);
            base.put_pixel(i as u32, (bh - 1 - j) as u32, Rgba([r, g, b, 255]));
        }
    }

    // Alpha maps: group weights (group 0 does not count), normalized per texel to sum 1.
    let [aw, ah] = terrain.params.texture_alpha_size.map(|v| v as usize);
    let mut weights: Vec<Vec<f32>> = groups
        .iter()
        .map(|&group| {
            (0..aw * ah)
                .map(|idx| {
                    let (i, j) = (idx % aw, idx / aw);
                    let p = (
                        i as f32 * c as f32 / aw as f32,
                        j as f32 * c as f32 / ah as f32,
                    );
                    if group == 0 {
                        0.0
                    } else {
                        blend_weight(terrain, origin, p, |t| group_of(t) == group)
                    }
                })
                .collect()
        })
        .collect();
    for idx in 0..aw * ah {
        let total: f32 = weights.iter().map(|w| w[idx]).sum();
        if total > 0.0 {
            for w in &mut weights {
                w[idx] /= total;
            }
        }
    }
    let layers = groups
        .iter()
        .zip(weights)
        .map(|(&group, weights)| {
            let mut alpha = GrayImage::new(aw as u32, ah as u32);
            for (idx, w) in weights.iter().enumerate() {
                let (i, j) = (idx % aw, idx / aw);
                let value = if group == 0 {
                    255
                } else {
                    (w * 255.0).clamp(0.0, 255.0) as u8
                };
                alpha.put_pixel(i as u32, (ah - 1 - j) as u32, Luma([value]));
            }
            let textures = guids
                .iter()
                .filter(|&&t| group_of(t) == group)
                .map(|&t| SplatTexture {
                    guid: t,
                    tiles: splat_tiles(terrain, origin, t, &group_of),
                })
                .collect();
            SplatLayer {
                group,
                alpha,
                textures,
            }
        })
        .collect();
    ChunkTextures {
        tile_x0: x0,
        tile_z0: z0,
        base,
        layers,
    }
}

/// Tile list of texture `t` in the alpha pass (`FUN_0055CBF0`).
fn splat_tiles(
    terrain: &Terrain,
    origin: (usize, usize),
    t: u32,
    group_of: &impl Fn(u32) -> u32,
) -> Vec<u16> {
    let c = terrain.chunk_size();
    let gt = group_of(t);
    let near = |x: isize, z: isize| {
        (x - 2..=x + 1).any(|nx| (z - 2..=z + 1).any(|nz| terrain.tile_checked(nx, nz) == Some(t)))
    };
    let mut tiles = Vec::new();
    for a in 0..c {
        for b in 0..c {
            let (x, z) = ((origin.0 + a) as isize, (origin.1 + b) as isize);
            let Some(g) = terrain.tile_checked(x, z) else {
                continue;
            };
            let gk = group_of(g);
            if g == t || (gt != 0 && gk != 0 && gk != gt && near(x, z)) {
                // `CHUNK_SIZE` is limited to 256 in the `.wld` parser, so `a * c + b < 65536`.
                tiles.push((a * c + b) as u16);
            }
        }
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::tests::test_terrain;

    fn solid(rgb: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(2, 2, Rgba([rgb[0], rgb[1], rgb[2], 255]))
    }

    fn images() -> HashMap<u32, RgbaImage> {
        [
            (1, solid([255, 0, 0])),
            (2, solid([0, 0, 255])),
            (3, solid([10, 20, 30])),
        ]
        .into()
    }

    #[test]
    fn sample_texel_reads_bottom_up() {
        let mut img = RgbaImage::new(1, 2);
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255])); // top
        img.put_pixel(0, 1, Rgba([0, 255, 0, 255])); // bottom
        assert_eq!(sample_texel(&img, 0.0, 0.0), [0, 255, 0, 255]);
        assert_eq!(sample_texel(&img, 0.0, 0.99), [255, 0, 0, 255]);
        assert_eq!(sample_texel(&img, 5.0, -3.0), [0, 255, 0, 255]);
    }

    #[test]
    fn chunk_layout_follows_terrain_build_chunks() {
        let (terrain, _) = test_terrain(32, |_, _| 3);
        assert_eq!((terrain.chunk_size(), terrain.chunk_count()), (16, 4));
        assert_eq!(terrain.chunk_origin(1), (16, 0));
        assert_eq!(terrain.chunk_origin(2), (0, 16));
    }

    #[test]
    fn single_group_zero_texture_is_opaque_everywhere() {
        let (terrain, project) = test_terrain(4, |_, _| 3);
        let chunk = build_chunk_textures(&terrain, &project, &images(), 0);
        assert!(chunk.base.pixels().all(|p| p.0 == [10, 20, 30, 255]));
        assert_eq!(chunk.layers.len(), 1);
        let layer = &chunk.layers[0];
        assert_eq!(layer.group, 0);
        assert!(layer.alpha.pixels().all(|p| p.0 == [255]));
        assert_eq!(layer.textures.len(), 1);
        assert_eq!(layer.textures[0].tiles, (0..16).collect::<Vec<u16>>());
    }

    #[test]
    fn groups_blend_and_tile_lists_reach_into_the_neighbour_group() {
        // Left (x < 8) texture 1 (group 1), right texture 2 (group 2).
        let (terrain, project) = test_terrain(16, |x, _| if x < 8 { 1 } else { 2 });
        let chunk = build_chunk_textures(&terrain, &project, &images(), 0);
        let groups: Vec<u32> = chunk.layers.iter().map(|l| l.group).collect();
        assert_eq!(groups, [1, 2]);
        let (left, right) = (&chunk.layers[0], &chunk.layers[1]);
        assert_eq!(left.alpha.get_pixel(0, 5).0, [255]);
        assert_eq!(right.alpha.get_pixel(0, 5).0, [0]);
        assert_eq!(left.alpha.get_pixel(31, 5).0, [0]);
        assert_eq!(right.alpha.get_pixel(31, 5).0, [255]);
        let tiles1 = &left.textures[0].tiles;
        let tiles2 = &right.textures[0].tiles;
        // Texture 1 reaches two tile columns to the right (x - 2 ... x + 1), texture 2 one to the left.
        assert_eq!((tiles1.len(), tiles2.len()), (160, 144));
        assert!(tiles1.contains(&(9 * 16)) && !tiles1.contains(&(10 * 16)));
        assert!(tiles2.contains(&(7 * 16)) && !tiles2.contains(&(6 * 16)));
    }

    #[test]
    fn base_texture_rows_run_bottom_up() {
        // Bottom (z < 8) red, top blue: texel row j = 0 (block z ~ 0) lands in image row H - 1.
        let (terrain, project) = test_terrain(16, |_, z| if z < 8 { 1 } else { 2 });
        let chunk = build_chunk_textures(&terrain, &project, &images(), 0);
        assert_eq!(chunk.base.dimensions(), (8, 8));
        assert_eq!(chunk.base.get_pixel(0, 7).0, [255, 0, 0, 255]);
        assert_eq!(chunk.base.get_pixel(0, 0).0, [0, 0, 255, 255]);
    }
}
