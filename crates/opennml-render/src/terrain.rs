//! Terrain on the GPU like `TerrainChunk_Create` (`0x0055DFB0`) and `TerrainChunk_Draw`
//! (`0x00558950`): one vertex buffer per chunk with three UV sets, base texture and alpha maps;
//! plus the black edge skirts (`TerrainChunk_DrawEdgeSkirts` `0x00558F10`).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use image::RgbaImage;
use opennml_formats::prj::Project;
use opennml_world::chunks::{ChunkTextures, build_chunk_textures};
use opennml_world::terrain::{FogFactors, Terrain};
use wgpu::util::DeviceExt;

use crate::shared::{ALPHA_BLEND, PipelineDesc, Shared, pipeline, shader};
use crate::texture::{GpuTexture, upload_gray, upload_rgba};

/// Terrain vertex (FVF `0x342`: position, color, three UV sets; stride `0x28`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TerrainVertexGpu {
    pub position: [f32; 3],
    /// Lit vertex color RGBA (alpha 255).
    pub color: [u8; 4],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    pub uv2: [f32; 2],
}

/// Edge skirt vertex (position, color).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SkirtVertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
}

/// Vertices of a chunk, x outer, z inner (`a · n + b`), UV formulas from `docs/formats/tlm.md`;
/// the step in v uses the X size of the map, as in the code.
///
/// The color is the lit color times the fog factor of the grid point
/// (`TerrainChunk_UpdateVertexColors` `0x00557070`); without factors (model browser, map without
/// simulation) it is 1.
pub fn chunk_vertices(
    terrain: &Terrain,
    chunk: usize,
    fog: Option<&FogFactors>,
) -> Vec<TerrainVertexGpu> {
    let c = terrain.chunk_size();
    let n = c + 1;
    let (x0, z0) = terrain.chunk_origin(chunk);
    let p = &terrain.params;
    let [wb, hb] = p.texture_base_size.map(|v| v as f32);
    let [wa, ha] = p.texture_alpha_size.map(|v| v as f32);
    let tiling = p.texture_tiling as f32;
    let step = (n - 1) as f32;
    let mut vertices = Vec::with_capacity(n * n);
    for a in 0..n {
        for b in 0..n {
            let v = terrain.vertex(x0 + a, z0 + b);
            let (af, bf) = (a as f32, b as f32);
            let [r, g, bl] = fog.map_or(v.lit_color, |f| f.apply(x0 + a, z0 + b, v.lit_color));
            vertices.push(TerrainVertexGpu {
                position: v.position.to_array(),
                color: [r, g, bl, 255],
                uv0: [
                    1.0 / (2.0 * wb) + af * (1.0 - 1.0 / wb) / step,
                    1.0 - (1.0 / (2.0 * hb) + bf * (1.0 - 1.0 / wb) / step),
                ],
                uv1: [
                    1.0 / (2.0 * wa) + af * (1.0 - 1.0 / wa) / step,
                    1.0 - (1.0 / (2.0 * ha) + bf * (1.0 - 1.0 / wa) / step),
                ],
                uv2: [af / tiling, 1.0 - bf / tiling],
            });
        }
    }
    vertices
}

/// Two triangles per tile from the triangle strip of the game: `(x,z), (x,z+1), (x+1,z)` and
/// `(x,z+1), (x+1,z+1), (x+1,z)` (front face clockwise from above). `tile = a · C + b`.
pub fn tile_indices(chunk_size: usize, tile: usize) -> [u32; 6] {
    let n = chunk_size + 1;
    let (a, b) = (tile / chunk_size, tile % chunk_size);
    let p00 = (a * n + b) as u32;
    let p01 = p00 + 1;
    let p10 = ((a + 1) * n + b) as u32;
    let p11 = p10 + 1;
    [p00, p01, p10, p01, p11, p10]
}

/// Edge skirts: vertical quads along all four map edges from `max(height, WATER_LEVEL)`
/// down to `y = −500`, black (`0xFF000000`, independent of the fog of war).
pub fn skirt_vertices(terrain: &Terrain) -> Vec<SkirtVertex> {
    let water = terrain.params.water_level;
    let (dx, dz) = (terrain.dim_x(), terrain.dim_z());
    let mut edge: Vec<(usize, usize)> = Vec::new();
    edge.extend((0..=dx).map(|x| (x, 0)));
    edge.extend((1..=dz).map(|z| (dx, z)));
    edge.extend((0..dx).rev().map(|x| (x, dz)));
    edge.extend((0..dz).rev().map(|z| (0, z)));
    let black = [0, 0, 0, 255];
    let mut vertices = Vec::new();
    for pair in edge.windows(2) {
        let [(ax, az), (bx, bz)] = [pair[0], pair[1]];
        let (pa, pb) = (
            terrain.vertex(ax, az).position,
            terrain.vertex(bx, bz).position,
        );
        let top = |p: glam::Vec3| [p.x, p.y.max(water), p.z];
        let bottom = |p: glam::Vec3| [p.x, -500.0, p.z];
        for position in [
            top(pa),
            bottom(pa),
            top(pb),
            bottom(pa),
            bottom(pb),
            top(pb),
        ] {
            vertices.push(SkirtVertex {
                position,
                color: black,
            });
        }
    }
    vertices
}

struct SplatDraw {
    bind_group: wgpu::BindGroup,
    first_index: u32,
    index_count: u32,
}

struct ChunkGpu {
    vertex_buffer: wgpu::Buffer,
    base_bind_group: wgpu::BindGroup,
    splat_indices: Option<wgpu::Buffer>,
    splats: Vec<SplatDraw>,
}

/// Terrain of a level on the GPU.
///
/// Deviation: only the alpha-map mode is implemented; the detail-texture mode of the original
/// (TerrainQuality >= 2) is not implemented.
pub struct TerrainGpu {
    base_pipeline: wgpu::RenderPipeline,
    splat_pipeline: wgpu::RenderPipeline,
    skirt_pipeline: wgpu::RenderPipeline,
    tile_indices: wgpu::Buffer,
    tile_index_count: u32,
    chunks: Vec<ChunkGpu>,
    skirt_buffer: wgpu::Buffer,
    skirt_vertex_count: u32,
    /// Most recently written factors of the fog of war (`None` = factor 1).
    fog: Option<FogFactors>,
    _textures: Vec<GpuTexture>,
}

const TERRAIN_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Unorm8x4, 2 => Float32x2, 3 => Float32x2, 4 => Float32x2
];
const SKIRT_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4];

impl TerrainGpu {
    /// Builds all chunks; `ground` are the ground textures by `TEX_GUID` (with mip levels),
    /// `white` serves as the second texture of the base pass.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shared: &Shared,
        color_format: wgpu::TextureFormat,
        terrain: &Terrain,
        project: &Project,
        images: &HashMap<u32, RgbaImage>,
        ground: &HashMap<u32, GpuTexture>,
        white: &GpuTexture,
    ) -> Self {
        let module = shader(device, "Terrain", include_str!("shaders/terrain.wgsl"));
        let layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TerrainVertexGpu>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &TERRAIN_ATTRIBUTES,
        };
        let layouts = [&shared.frame_layout, &shared.textures_layout];
        let desc = |label, fragment, blend, depth_write| PipelineDesc {
            label,
            layouts: &layouts,
            module: &module,
            vertex: "vs_terrain",
            fragment,
            buffers: std::slice::from_ref(&layout),
            blend,
            depth_write,
            cull: true,
            color_format,
        };
        let base_pipeline = pipeline(device, &desc("TerrainSplat Base", "fs_base", None, true));
        let splat_pipeline = pipeline(
            device,
            &desc("TerrainSplat Alpha", "fs_splat", Some(ALPHA_BLEND), false),
        );
        let skirt_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SkirtVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &SKIRT_ATTRIBUTES,
        };
        let skirt_pipeline = pipeline(
            device,
            &PipelineDesc {
                label: "TerrainTiles Edge",
                layouts: &[&shared.frame_layout],
                module: &module,
                vertex: "vs_skirt",
                fragment: "fs_skirt",
                buffers: std::slice::from_ref(&skirt_layout),
                blend: None,
                depth_write: true,
                // Deviation from the state block `TerrainTiles` (CULLMODE CCW): the winding order
                // of the original strip in `TerrainChunk_DrawEdgeSkirts` (0x00558F10) is not
                // determined; both sides are drawn so that no visible skirt is lost.
                cull: false,
                color_format,
            },
        );

        let c = terrain.chunk_size();
        let all_tiles: Vec<u32> = (0..c * c).flat_map(|t| tile_indices(c, t)).collect();
        let tile_indices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Tile indices"),
            contents: bytemuck::cast_slice(&all_tiles),
            usage: wgpu::BufferUsages::INDEX,
        });
        let mut textures = Vec::new();
        let mut chunks = Vec::with_capacity(terrain.chunk_count());
        for chunk in 0..terrain.chunk_count() {
            let built: ChunkTextures = build_chunk_textures(terrain, project, images, chunk);
            let vertices = chunk_vertices(terrain, chunk, None);
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Chunk"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let base = upload_rgba(device, queue, &built.base, false, "Base texture");
            let base_bind_group = shared.textures(device, &base.view, &white.view);
            textures.push(base);
            let mut indices = Vec::new();
            let mut splats = Vec::new();
            for layer in &built.layers {
                let alpha = upload_gray(device, queue, &layer.alpha, "Alpha map");
                for texture in &layer.textures {
                    let Some(ground_texture) = ground.get(&texture.guid) else {
                        continue;
                    };
                    if texture.tiles.is_empty() {
                        continue;
                    }
                    let first_index = indices.len() as u32;
                    for &tile in &texture.tiles {
                        indices.extend_from_slice(&tile_indices(c, tile as usize));
                    }
                    splats.push(SplatDraw {
                        bind_group: shared.textures(device, &ground_texture.view, &alpha.view),
                        first_index,
                        index_count: indices.len() as u32 - first_index,
                    });
                }
                textures.push(alpha);
            }
            let splat_indices = (!indices.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Alpha pass"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                })
            });
            chunks.push(ChunkGpu {
                vertex_buffer,
                base_bind_group,
                splat_indices,
                splats,
            });
        }
        let skirt = skirt_vertices(terrain);
        let skirt_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Edge skirts"),
            contents: bytemuck::cast_slice(&skirt),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            base_pipeline,
            splat_pipeline,
            skirt_pipeline,
            tile_indices: tile_indices_buffer,
            tile_index_count: all_tiles.len() as u32,
            chunks,
            skirt_buffer,
            skirt_vertex_count: skirt.len() as u32,
            fog: None,
            _textures: textures,
        }
    }

    /// Rewrites the vertex buffers of all chunks (light change, `Terrain_UploadLitVertexColors`
    /// `0x00557920`), with the most recently set fog factors.
    pub fn update_colors(&self, queue: &wgpu::Queue, terrain: &Terrain) {
        for index in 0..self.chunks.len() {
            self.write_chunk(queue, terrain, index);
        }
    }

    /// Most recently set factors of the fog of war.
    pub fn fog(&self) -> Option<&FogFactors> {
        self.fog.as_ref()
    }

    /// New factors of the fog of war (`TerrainChunk_UpdateVertexColors` `0x00557070`):
    /// rewrites only the chunks in whose `(c + 1)²` grid points a factor has changed.
    /// The edge skirts stay black. Returns whether a factor has changed.
    pub fn set_fog(
        &mut self,
        queue: &wgpu::Queue,
        terrain: &Terrain,
        fog: Option<&FogFactors>,
    ) -> bool {
        if self.fog.as_ref() == fog {
            return false;
        }
        let factor = |f: Option<&FogFactors>, x, z| f.map_or(255, |f| f.get(x, z));
        let n = terrain.chunk_size() + 1;
        let old = self.fog.as_ref();
        let changed: Vec<usize> = (0..self.chunks.len())
            .filter(|&index| {
                let (x0, z0) = terrain.chunk_origin(index);
                (0..n).any(|a| {
                    (0..n).any(|b| factor(old, x0 + a, z0 + b) != factor(fog, x0 + a, z0 + b))
                })
            })
            .collect();
        match (fog, &mut self.fog) {
            (Some(new), Some(stored)) => stored.clone_from(new),
            (new, stored) => *stored = new.cloned(),
        }
        for index in changed {
            self.write_chunk(queue, terrain, index);
        }
        true
    }

    fn write_chunk(&self, queue: &wgpu::Queue, terrain: &Terrain, index: usize) {
        let vertices = chunk_vertices(terrain, index, self.fog.as_ref());
        queue.write_buffer(
            &self.chunks[index].vertex_buffer,
            0,
            bytemuck::cast_slice(&vertices),
        );
    }

    /// Draws terrain (base pass, then alpha pass per chunk) and edge skirts. All tiles
    /// are drawn (the original only culls invisible ones via quadtree).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shared: &Shared) {
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        for chunk in &self.chunks {
            pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
            pass.set_pipeline(&self.base_pipeline);
            pass.set_bind_group(1, &chunk.base_bind_group, &[]);
            pass.set_index_buffer(self.tile_indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.tile_index_count, 0, 0..1);
            if let Some(indices) = &chunk.splat_indices {
                pass.set_pipeline(&self.splat_pipeline);
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                for splat in &chunk.splats {
                    pass.set_bind_group(1, &splat.bind_group, &[]);
                    pass.draw_indexed(
                        splat.first_index..splat.first_index + splat.index_count,
                        0,
                        0..1,
                    );
                }
            }
        }
        pass.set_pipeline(&self.skirt_pipeline);
        pass.set_vertex_buffer(0, self.skirt_buffer.slice(..));
        pass.draw(0..self.skirt_vertex_count, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_formats::prj::Project;
    use opennml_formats::testutil::flat_heightmap;
    use opennml_formats::tlm::TextureMap;
    use opennml_formats::wld::TerrainParams;

    fn terrain(dim: u32) -> Terrain {
        let params = TerrainParams {
            dim_x: dim,
            dim_z: dim,
            chunk_size: 2,
            texture_base_size: [8, 8],
            texture_alpha_size: [32, 32],
            texture_tiling: 4,
            ..TerrainParams::default()
        };
        let d = dim as usize;
        let map = TextureMap {
            dim_x: d,
            dim_z: d,
            guids: vec![1; d * d],
        };
        Terrain::new(
            &params,
            &flat_heightmap(dim, dim, 128.0, 0.0),
            &map,
            &Project::default(),
        )
        .unwrap()
    }

    #[test]
    fn chunk_vertices_follow_the_documented_uv_formulas() {
        let t = terrain(4);
        let v = chunk_vertices(&t, 3, None); // Chunk (x0, z0) = (2, 2)
        assert_eq!(v.len(), 9);
        assert_eq!(v[0].position, [256.0, 0.0, 256.0]);
        assert_eq!(v[1].position, [256.0, 0.0, 384.0]); // b = 1 → z + 1
        // Set 0: a = 0 hits the center of texel 0, a = C the center of texel W - 1.
        assert!((v[0].uv0[0] - 1.0 / 16.0).abs() < 1e-6);
        assert!((v[6].uv0[0] - (1.0 - 1.0 / 16.0)).abs() < 1e-6);
        assert!((v[0].uv0[1] - (1.0 - 1.0 / 16.0)).abs() < 1e-6);
        // Set 2: u = a / T, v = 1 - b / T.
        assert_eq!(v[8].uv2, [0.5, 0.5]);
        assert_eq!(v[0].color, [255, 255, 255, 255]);
    }

    #[test]
    fn tiles_use_the_strip_triangles_of_the_game() {
        // C = 2, n = 3: tile 1 = (a 0, b 1): (0,1), (0,2), (1,1) / (0,2), (1,2), (1,1).
        assert_eq!(tile_indices(2, 1), [1, 2, 4, 2, 5, 4]);
        // Both triangles clockwise from above (top view, +x right, +z up).
        let p = |i: u32| ((i / 3) as f32, (i % 3) as f32);
        let idx = tile_indices(2, 0);
        for tri in idx.chunks(3) {
            let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
            let cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            assert!(cross < 0.0, "{tri:?} not clockwise");
        }
    }

    #[test]
    fn skirts_cover_all_four_edges() {
        let t = terrain(2);
        let v = skirt_vertices(&t);
        assert_eq!(v.len(), 8 * 6); // perimeter of 8 tile edges, 2 triangles each
        assert!(
            v.iter()
                .all(|s| s.position[1] == 0.0 || s.position[1] == -500.0)
        );
    }

    #[test]
    fn fog_factors_darken_terrain_vertices() {
        let terrain = terrain(4);
        let n = terrain.verts_x;
        let mut fog = FogFactors {
            verts_x: n,
            values: vec![0; n * terrain.verts_z],
        };
        fog.values[n + 1] = 0x80;
        let lit = chunk_vertices(&terrain, 0, None);
        let dark = chunk_vertices(&terrain, 0, Some(&fog));
        // Chunk 0: vertex (a, b) at index a * (c + 1) + b, grid point (x0 + a, z0 + b).
        let c1 = terrain.chunk_size() + 1;
        assert_eq!(dark[0].color, [0, 0, 0, 255]);
        let [r, g, b, _] = lit[c1 + 1].color;
        let [er, eg, eb] = fog.apply(1, 1, [r, g, b]);
        assert_eq!(dark[c1 + 1].color, [er, eg, eb, 255]);
    }

    #[test]
    fn terrain_pipelines_and_buffers_validate_on_the_gpu() {
        use crate::{BackendChoice, CAPTURE_FORMAT, headless_device};
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shared = Shared::new(&device);
        let white = upload_rgba(
            &device,
            &queue,
            &RgbaImage::from_pixel(1, 1, image::Rgba([255; 4])),
            false,
            "White",
        );
        let t = terrain(4);
        let _gpu = TerrainGpu::new(
            &device,
            &queue,
            &shared,
            CAPTURE_FORMAT,
            &t,
            &Project::default(),
            &HashMap::new(),
            &HashMap::new(),
            &white,
        );
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }
}
