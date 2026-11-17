//! Water like `Terrain_RenderWaterPass` (`0x00559570`) / `TerrainChunk_DrawWater` (`0x005597D0`):
//! per tile with a corner below `WATER_LEVEL` a quad at water height; wave and
//! texture movement in the shader, counters from `Terrain_AdvanceWaterAnimation` (`0x00558160`).

use bytemuck::{Pod, Zeroable};
use nml_world::terrain::{FogFactors, Terrain};
use wgpu::util::DeviceExt;

use crate::shared::{ALPHA_BLEND, PipelineDesc, Shared, pipeline, shader};
use crate::texture::GpuTexture;

/// Water vertex: position at water height, color with water alpha, UV, wave.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct WaterVertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
    pub uv: [f32; 2],
    /// `[grid index, 1 at the map edge]`.
    pub wave: [f32; 2],
}

/// Water counters after `now_ms` milliseconds: one step of `Terrain_AdvanceWaterAnimation`
/// (`0x00558160`) every full 25 ms with `+0.05`, `+0.025`, `+0.035`, `+0.025`,
/// `+0.03` (`GameEngine_Update` `0x00445650`, not tied to the game speed).
pub fn water_phases(now_ms: f32) -> [f32; 5] {
    let steps = (now_ms / 25.0).floor().max(0.0);
    [0.05, 0.025, 0.035, 0.025, 0.03].map(|step| step * steps)
}

/// Quads of all water tiles as a triangle list (corners `(x,z), (x,z+1), (x+1,z), (x+1,z+1)`
/// like the strip of the game). Color = lit terrain color times fog factor (`fog`; without it all
/// factors are 1), alpha = water alpha; if the color **with** the factor is 0, alpha 255 (the original reads
/// the finished terrain color `TerrainVertex+0x28`): unexplored water is opaque black.
/// UV = grid index / `WATER_TEXTURETILING`.
pub fn water_vertices(terrain: &Terrain, fog: Option<&FogFactors>) -> Vec<WaterVertex> {
    let p = &terrain.params;
    let tiling = p.water_texture_tiling.max(1) as f32;
    let mut vertices = Vec::new();
    for (x, z) in terrain.water_tiles() {
        let corners = [(x, z), (x, z + 1), (x + 1, z), (x + 1, z + 1)];
        let quad = corners.map(|(cx, cz)| {
            let v = terrain.vertex(cx, cz);
            let color = fog.map_or(v.lit_color, |f| f.apply(cx, cz, v.lit_color));
            let [r, g, b] = color;
            let alpha = if color == [0, 0, 0] {
                255
            } else {
                v.water_alpha
            };
            WaterVertex {
                position: [v.position.x, p.water_level, v.position.z],
                color: [r, g, b, alpha],
                uv: [cx as f32 / tiling, cz as f32 / tiling],
                wave: [
                    (cz * terrain.verts_x + cx) as f32,
                    if v.is_edge { 1.0 } else { 0.0 },
                ],
            }
        });
        vertices.extend([quad[0], quad[1], quad[2], quad[1], quad[3], quad[2]]);
    }
    vertices
}

/// Water of a level on the GPU; `None` if there is nothing to draw.
pub struct WaterGpu {
    pipeline: wgpu::RenderPipeline,
    /// Detail texture mode (`TerrainQuality` >= 2): base water, then refraction, both with `M0`.
    detail_pipelines: [wgpu::RenderPipeline; 2],
    use_texture: bool,
    buffer: wgpu::Buffer,
    vertex_count: u32,
    bind_group: wgpu::BindGroup,
}

const WATER_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Float32x2, 3 => Float32x2];

impl WaterGpu {
    /// `base`/`refraction`: water textures (`WATER_BASE_TEXTUREGUID`, `…_REFRACTION_…`). With
    /// `WATER_USETEXTURE` and a missing texture the game draws **no** water
    /// (`0x005595BD`); without `WATER_USETEXTURE` only vertex colors (`white` as placeholder).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        shared: &Shared,
        color_format: wgpu::TextureFormat,
        terrain: &Terrain,
        base: Option<&GpuTexture>,
        refraction: Option<&GpuTexture>,
        white: &GpuTexture,
    ) -> Option<Self> {
        let (a, b) = if terrain.params.water_use_texture {
            (base?, refraction?)
        } else {
            (white, white)
        };
        let vertices = water_vertices(terrain, None);
        if vertices.is_empty() {
            return None;
        }
        let module = shader(device, "Water", include_str!("shaders/water.wgsl"));
        let layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WaterVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &WATER_ATTRIBUTES,
        };
        let make = |label, fragment| {
            pipeline(
                device,
                &PipelineDesc {
                    label,
                    layouts: &[&shared.frame_layout, &shared.textures_layout],
                    module: &module,
                    vertex: "vs_water",
                    fragment,
                    buffers: std::slice::from_ref(&layout),
                    blend: Some(ALPHA_BLEND),
                    depth_write: false,
                    cull: false,
                    color_format,
                },
            )
        };
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Water"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        Some(Self {
            pipeline: make("Water", "fs_water"),
            detail_pipelines: [
                make("Water Detail Base", "fs_water_detail_base"),
                make("Water Detail Refraction", "fs_water_detail_refraction"),
            ],
            use_texture: terrain.params.water_use_texture,
            buffer,
            vertex_count: vertices.len() as u32,
            bind_group: shared.textures(device, &a.view, &b.view),
        })
    }

    /// Rebuilds the vertex colors (light change, new fog-of-war factors): water
    /// takes the terrain color including the factor.
    pub fn update_colors(&self, queue: &wgpu::Queue, terrain: &Terrain, fog: Option<&FogFactors>) {
        queue.write_buffer(
            &self.buffer,
            0,
            bytemuck::cast_slice(&water_vertices(terrain, fog)),
        );
    }

    /// Draws the water; with `detail` (and `WATER_USETEXTURE`) in detail texture mode in two
    /// layers (`TerrainChunk_DrawWater` `0x00559D10` ...). The original draws the layers per
    /// tile one after another; since water tiles do not overlap, the result is the same.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, shared: &Shared, detail: bool) {
        pass.set_bind_group(0, &shared.frame_bind_group, &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        if detail && self.use_texture {
            for pipeline in &self.detail_pipelines {
                pass.set_pipeline(pipeline);
                pass.draw(0..self.vertex_count, 0..1);
            }
        } else {
            pass.set_pipeline(&self.pipeline);
            pass.draw(0..self.vertex_count, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_formats::prj::Project;
    use nml_formats::testutil::flat_heightmap;
    use nml_formats::tlm::TextureMap;
    use nml_formats::wld::TerrainParams;

    #[test]
    fn only_tiles_with_a_corner_below_the_water_level_get_water() {
        let params = TerrainParams {
            dim_x: 2,
            dim_z: 2,
            chunk_size: 2,
            water_level: 0.0,
            water_texture_tiling: 8,
            water_max_depth: 100,
            water_min_alpha: 10,
            water_max_alpha: 200,
            ..TerrainParams::default()
        };
        let mut heightmap = flat_heightmap(2, 2, 128.0, 20.0);
        heightmap.points[4].position[1] = -25.0; // center (1, 1) under water -> all four tiles
        let map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![1; 4],
        };
        let terrain = Terrain::new(&params, &heightmap, &map, &Project::default()).unwrap();
        let v = water_vertices(&terrain, None);
        assert_eq!(v.len(), 4 * 6);
        assert!(v.iter().all(|w| w.position[1] == 0.0));
        let center = v.iter().find(|w| w.wave[0] == 4.0).unwrap();
        assert_eq!(center.wave[1], 0.0);
        assert_eq!(center.color[3], 127); // 255 * sqrt(25/100)
        assert_eq!(center.uv, [1.0 / 8.0, 1.0 / 8.0]);
        let corner = v.iter().find(|w| w.wave[0] == 0.0).unwrap();
        assert_eq!((corner.wave[1], corner.color[3]), (1.0, 10)); // edge; above water -> MINALPHA
    }

    #[test]
    fn water_takes_the_fog_factor_and_unexplored_water_is_opaque_black() {
        let params = TerrainParams {
            dim_x: 2,
            dim_z: 2,
            chunk_size: 2,
            water_level: 10.0,
            water_max_depth: 100,
            water_min_alpha: 10,
            water_max_alpha: 200,
            light_position: [0.0, 1.0, 0.0],
            light_color: [200, 200, 200],
            light_intensity: 100,
            ..TerrainParams::default()
        };
        let map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![1; 4],
        };
        let heightmap = flat_heightmap(2, 2, 128.0, 0.0);
        let terrain = Terrain::new(&params, &heightmap, &map, &Project::default()).unwrap();
        // Grid point (1, 1) half bright, all others unexplored.
        let mut fog = FogFactors {
            verts_x: 3,
            values: vec![0; 9],
        };
        fog.values[4] = 0x80;
        let lit = water_vertices(&terrain, None);
        let fogged = water_vertices(&terrain, Some(&fog));
        let at = |v: &[WaterVertex], i: f32| *v.iter().find(|w| w.wave[0] == i).unwrap();
        let (center, corner) = (at(&fogged, 4.0), at(&fogged, 0.0));
        let [r, g, b, a] = at(&lit, 4.0).color;
        assert_eq!(center.color, {
            let [r, g, b] = fog.apply(1, 1, [r, g, b]);
            [r, g, b, a]
        });
        assert_ne!(at(&lit, 0.0).color[3], 255);
        assert_eq!(corner.color, [0, 0, 0, 255], "unexplored: opaque black");
    }

    #[test]
    fn phases_advance_in_25_ms_steps() {
        let p = water_phases(1000.0);
        assert!((p[0] - 2.0).abs() < 1e-6 && (p[4] - 1.2).abs() < 1e-6);
        assert_eq!(water_phases(24.9), [0.0; 5]);
        assert_eq!(water_phases(25.0), [0.05, 0.025, 0.035, 0.025, 0.03]);
        assert_eq!(water_phases(49.0), water_phases(25.0));
        assert_eq!(water_phases(1012.0), water_phases(1000.0));
    }

    #[test]
    fn water_pipeline_validates_on_the_gpu() {
        use crate::texture::white;
        use crate::{BackendChoice, CAPTURE_FORMAT, headless_device};
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let params = TerrainParams {
            dim_x: 2,
            dim_z: 2,
            chunk_size: 2,
            water_level: 10.0,
            water_texture_tiling: 8,
            water_use_texture: true,
            ..TerrainParams::default()
        };
        let heightmap = flat_heightmap(2, 2, 128.0, 0.0);
        let map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![1; 4],
        };
        let terrain = Terrain::new(&params, &heightmap, &map, &Project::default()).unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shared = Shared::new(&device);
        let white = white(&device, &queue);
        let water = WaterGpu::new(
            &device,
            &shared,
            CAPTURE_FORMAT,
            &terrain,
            Some(&white),
            Some(&white),
            &white,
        );
        assert!(water.is_some());
        // Missing texture with WATER_USETEXTURE: no water.
        assert!(
            WaterGpu::new(
                &device,
                &shared,
                CAPTURE_FORMAT,
                &terrain,
                None,
                None,
                &white
            )
            .is_none()
        );
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "validation error: {error:?}");
    }
}
