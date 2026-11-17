//! Level as glTF: terrain per block with baked texture and vertex colors, water surfaces and
//! the placed objects (models in the bind pose, meshes only once per model).

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use glam::{Affine3A, Vec3};
use image::{GrayImage, Rgba, RgbaImage};
use nml_rda::Vfs;
use nml_world::assets::ModelCache;
use nml_world::chunks::{ChunkTextures, build_chunk_textures};
use nml_world::level::{LevelScene, ObjectClass};
use nml_world::terrain::Terrain;
use serde_json::json;

use crate::glb::GltfBuilder;
use crate::mirror;
use crate::model::{MaterialCache, static_parts};
use crate::srgb_to_linear;

/// Settings for [`level_to_glb`].
#[derive(Debug, Clone, PartialEq)]
pub struct LevelExportOptions {
    /// Texels per tile edge of the baked terrain texture.
    pub pixels_per_tile: u32,
    /// Include placed game objects in the output.
    pub objects: bool,
    /// Include water surfaces in the output.
    pub water: bool,
}

impl Default for LevelExportOptions {
    fn default() -> Self {
        Self {
            pixels_per_tile: 16,
            objects: true,
            water: true,
        }
    }
}

/// Bilinear sampling like the linear texture filter (texel centers at `(i + 0.5) / width`);
/// `wrap` = repeat, otherwise clamped at the edge.
fn bilinear(
    (width, height): (u32, u32),
    fetch: impl Fn(u32, u32) -> [f32; 4],
    u: f32,
    v: f32,
    wrap: bool,
) -> [f32; 4] {
    if width == 0 || height == 0 {
        return [0.0, 0.0, 0.0, 255.0];
    }
    let (w, h) = (width as i64, height as i64);
    let x = u * width as f32 - 0.5;
    let y = v * height as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let texel = |xi: i64, yi: i64| {
        let (xi, yi) = if wrap {
            (xi.rem_euclid(w), yi.rem_euclid(h))
        } else {
            (xi.clamp(0, w - 1), yi.clamp(0, h - 1))
        };
        fetch(xi as u32, yi as u32)
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (
        texel(x0, y0),
        texel(x0 + 1, y0),
        texel(x0, y0 + 1),
        texel(x0 + 1, y0 + 1),
    );
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

fn sample_rgba(img: &RgbaImage, u: f32, v: f32, wrap: bool) -> [f32; 4] {
    bilinear(
        img.dimensions(),
        |x, y| img.get_pixel(x, y).0.map(f32::from),
        u,
        v,
        wrap,
    )
}

fn sample_gray(img: &GrayImage, u: f32, v: f32) -> f32 {
    bilinear(
        img.dimensions(),
        |x, y| [f32::from(img.get_pixel(x, y).0[0]); 4],
        u,
        v,
        false,
    )[0]
}

/// Bakes the terrain color of a block without vertex color (which `COLOR_0` carries):
/// base texture (UV set 0), then per group and texture the alpha pass (ground texture tiled
/// with UV set 2, alpha map with UV set 1; `C = C_t*A + C*(1 - A)`), as in
/// `TerrainChunk_Draw` (`0x00558950`). UV formulas from `docs/formats/tlm.md`. Image: row 0 =
/// largest block z, `pixels_per_tile` texels per tile.
pub fn bake_chunk_texture(
    terrain: &Terrain,
    chunk: &ChunkTextures,
    images: &HashMap<u32, RgbaImage>,
    pixels_per_tile: u32,
) -> RgbaImage {
    let c = terrain.chunk_size();
    let cf = c as f32;
    let params = &terrain.params;
    let [base_w, base_h] = params.texture_base_size.map(|v| v as f32);
    let [alpha_w, alpha_h] = params.texture_alpha_size.map(|v| v as f32);
    let tiling = params.texture_tiling as f32;
    // UV set 0/1: half-texel border; the step in v uses the X size, as in the original code.
    let uv = |w: f32, h: f32, fx: f32, fz: f32| {
        (
            1.0 / (2.0 * w) + fx * (1.0 - 1.0 / w) / cf,
            1.0 - (1.0 / (2.0 * h) + fz * (1.0 - 1.0 / w) / cf),
        )
    };
    let tile_sets: Vec<Vec<HashSet<u16>>> = chunk
        .layers
        .iter()
        .map(|layer| {
            layer
                .textures
                .iter()
                .map(|t| t.tiles.iter().copied().collect())
                .collect()
        })
        .collect();
    let size = c as u32 * pixels_per_tile;
    let mut out = RgbaImage::new(size, size);
    for py in 0..size {
        for px in 0..size {
            let fx = (px as f32 + 0.5) / pixels_per_tile as f32;
            let fz = cf - (py as f32 + 0.5) / pixels_per_tile as f32;
            let tile = ((fx as usize).min(c - 1) * c + (fz as usize).min(c - 1)) as u16;
            let (u0, v0) = uv(base_w, base_h, fx, fz);
            let mut color = sample_rgba(&chunk.base, u0, v0, false);
            for (layer, sets) in chunk.layers.iter().zip(&tile_sets) {
                let (u1, v1) = uv(alpha_w, alpha_h, fx, fz);
                let alpha = sample_gray(&layer.alpha, u1, v1) / 255.0;
                for (texture, tiles) in layer.textures.iter().zip(sets) {
                    let Some(img) = images.get(&texture.guid).filter(|_| tiles.contains(&tile))
                    else {
                        continue;
                    };
                    let texel = sample_rgba(img, fx / tiling, 1.0 - fz / tiling, true);
                    for k in 0..3 {
                        color[k] = texel[k] * alpha + color[k] * (1.0 - alpha);
                    }
                }
            }
            let [r, g, b, _] = color.map(|v| v.round().clamp(0.0, 255.0) as u8);
            out.put_pixel(px, py, Rgba([r, g, b, 255]));
        }
    }
    out
}

/// Mesh of a terrain block: `(C + 1)^2` grid points (x outer, z inner like the game's vertex
/// buffer), triangles A/B per tile with reversed winding order, UV onto the baked texture.
fn chunk_mesh(b: &mut GltfBuilder, terrain: &Terrain, chunk: usize, material: usize) -> usize {
    let c = terrain.chunk_size();
    let n = c + 1;
    let (x0, z0) = terrain.chunk_origin(chunk);
    let (mut positions, mut normals, mut colors, mut uvs) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for a in 0..n {
        for bz in 0..n {
            let v = terrain.vertex(x0 + a, z0 + bz);
            positions.push(mirror::point(v.position));
            normals.push(mirror::point(v.normal.normalize_or(Vec3::Y)));
            colors.push(v.lit_color.map(|c| srgb_to_linear(c as f32 / 255.0)));
            uvs.push([a as f32 / c as f32, 1.0 - bz as f32 / c as f32]);
        }
    }
    let mut indices = Vec::with_capacity(c * c * 6);
    for a in 0..c {
        for bz in 0..c {
            let p00 = (a * n + bz) as u32;
            let p01 = p00 + 1;
            let p10 = ((a + 1) * n + bz) as u32;
            let p11 = p10 + 1;
            // A = (x,z),(x,z+1),(x+1,z) and B = (x,z+1),(x+1,z+1),(x+1,z), each as i0, i2, i1.
            indices.extend_from_slice(&[p00, p10, p01, p01, p10, p11]);
        }
    }
    let attributes = json!({
        "POSITION": b.positions(&positions),
        "NORMAL": b.attribute(&normals),
        "COLOR_0": b.attribute(&colors),
        "TEXCOORD_0": b.attribute(&uvs),
    });
    let indices = b.indices(&indices);
    b.mesh(json!({
        "name": format!("Block {chunk}"),
        "primitives": [{ "attributes": attributes, "indices": indices, "material": material, "mode": 4 }],
    }))
}

/// Water: for each tile with a corner below `WATER_LEVEL`, a quad at water height
/// (`TerrainChunk_DrawWater` `0x005597D0`, without waves), color and alpha per grid point
/// (`lit_color`, `water_alpha`), base water texture with UV `grid index / WATER_TEXTURETILING`.
fn water_node(
    b: &mut GltfBuilder,
    scene: &LevelScene,
    images: &HashMap<u32, RgbaImage>,
) -> Option<usize> {
    let terrain = &scene.terrain;
    let params = &scene.world.terrain;
    let tiling = params.water_texture_tiling.max(1) as f32;
    let (mut positions, mut normals, mut colors, mut uvs, mut indices) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (x, z) in terrain.water_tiles() {
        let corners = [(x, z), (x, z + 1), (x + 1, z), (x + 1, z + 1)];
        let first = positions.len() as u32;
        for (cx, cz) in corners {
            let v = terrain.vertex(cx, cz);
            let mut position = v.position;
            position.y = params.water_level;
            positions.push(mirror::point(position));
            normals.push([0.0, 1.0, 0.0]);
            let [r, g, bl] = v.lit_color.map(|c| srgb_to_linear(c as f32 / 255.0));
            colors.push([r, g, bl, v.water_alpha as f32 / 255.0]);
            uvs.push([cx as f32 / tiling, cz as f32 / tiling]);
        }
        // Corners 0 = (x,z), 1 = (x,z+1), 2 = (x+1,z), 3 = (x+1,z+1); like the terrain.
        indices.extend([0, 2, 1, 1, 2, 3].map(|k| first + k));
    }
    if positions.is_empty() {
        return None;
    }
    let mut pbr = json!({ "baseColorFactor": [1.0, 1.0, 1.0, 1.0], "metallicFactor": 0.0, "roughnessFactor": 1.0 });
    if let Some(img) = images
        .get(&params.water_base_texture)
        .filter(|_| params.water_use_texture)
    {
        pbr["baseColorTexture"] = json!({ "index": b.texture_png(img) });
    }
    let material = b.material(json!({
        "name": "Water", "pbrMetallicRoughness": pbr, "alphaMode": "BLEND", "doubleSided": true
    }));
    let attributes = json!({
        "POSITION": b.positions(&positions),
        "NORMAL": b.attribute(&normals),
        "COLOR_0": b.attribute(&colors),
        "TEXCOORD_0": b.attribute(&uvs),
    });
    let indices = b.indices(&indices);
    let mesh = b.mesh(json!({
        "name": "Water",
        "primitives": [{ "attributes": attributes, "indices": indices, "material": material, "mode": 4 }],
    }));
    Some(b.node(json!({ "name": "Water", "mesh": mesh })))
}

/// Placed game objects: one node per object and mesh part with `world matrix * part matrix`.
fn objects_node(b: &mut GltfBuilder, vfs: &Vfs, scene: &LevelScene) -> usize {
    let mut models = ModelCache::new();
    let mut materials = MaterialCache::default();
    let mut parts: HashMap<String, Vec<(usize, Affine3A)>> = HashMap::new();
    let mut children = Vec::new();
    for object in scene
        .objects
        .iter()
        .filter(|o| o.class == ObjectClass::Game)
    {
        if !parts.contains_key(&object.model) {
            let Some(model) = models.get(vfs, &object.model) else {
                continue;
            };
            let list = static_parts(b, &mut materials, vfs, &model);
            parts.insert(object.model.clone(), list);
        }
        for &(mesh, local) in &parts[&object.model] {
            children.push(b.node(json!({
                "name": format!("{} #{}", object.type_name, object.index),
                "mesh": mesh,
                "matrix": mirror::matrix(object.transform * local),
            })));
        }
    }
    let node = b.node(json!({ "name": "Objects" }));
    b.add_children(node, &children);
    node
}

/// Exports a level as `.glb` (terrain, optionally water and objects).
///
/// Deliberate limits (plausible, not faithful to the original; the viewer is the faithful
/// reference):
/// - Materials are unlit PBR with a base color (`metallic 0`, `roughness 1`); glTF does not know
///   the lighting and material model of the original.
/// - Vertex colors (`COLOR_0`) are converted from sRGB to linear because the game computes in
///   sRGB space and glTF requires linear.
/// - Water is a semi-transparent surface with vertex color and alpha, without waves and without
///   texture motion.
/// - Objects stand in the bind pose; for animations, crossfades, pivot offset and helper nodes,
///   the limits of [`crate::model_to_glb`] apply.
pub fn level_to_glb(
    vfs: &Vfs,
    scene: &LevelScene,
    options: &LevelExportOptions,
) -> Result<Vec<u8>> {
    let mut b = GltfBuilder::new();
    let images = scene.ground_textures(vfs);
    let terrain = &scene.terrain;
    let mut chunk_nodes = Vec::new();
    for chunk in 0..terrain.chunk_count() {
        let textures = build_chunk_textures(terrain, &scene.project, &images, chunk);
        let baked = bake_chunk_texture(terrain, &textures, &images, options.pixels_per_tile.max(1));
        let texture = b.texture_png_clamped(&baked);
        let material = b.material(json!({
            "name": format!("Block {chunk}"),
            "pbrMetallicRoughness": {
                "baseColorTexture": { "index": texture }, "metallicFactor": 0.0, "roughnessFactor": 1.0
            },
        }));
        let mesh = chunk_mesh(&mut b, terrain, chunk, material);
        chunk_nodes.push(b.node(json!({ "name": format!("Block {chunk}"), "mesh": mesh })));
    }
    let ground = b.node(json!({ "name": "Terrain" }));
    b.add_children(ground, &chunk_nodes);
    let mut roots = vec![ground];
    if options.water {
        roots.extend(water_node(&mut b, scene, &images));
    }
    if options.objects {
        roots.push(objects_node(&mut b, vfs, scene));
    }
    Ok(b.finish(&roots))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::{game_dir, tga};
    use nml_formats::rdo::RdoLayout;
    use nml_formats::testutil::{flat_heightmap, sample_rdo, write_hlm, write_rdo, write_tlm};
    use nml_formats::tlm::TextureMap;
    use nml_world::level::load_level;

    const LVL: &str = "LEVELFILE V1.08e\r\nPROJECT_FILE \"Data\\Projects\\test.prj\"\r\n[PLAYER]\r\n{\r\n\tPLAYER_NAME \"Spieler\"\r\n\tPLAYER_PLAYER \"SPA\"\r\n\tPLAYER_TYPE 0\r\n\tPLAYER_COLOR 0\r\n}\r\n";
    const PRJ: &str = "PROJECTFILE V1.08f\r\n[OBJECT]\r\n{\r\n\tOBJ_NAME \"Kiste\"\r\n\tOBJ_GUID 10\r\n\tOBJ_FILENAME \"data\\r3d\\test\\kiste.rdo\"\r\n}\r\n[TEXTURE]\r\n{\r\n\tTEX_NAME \"Gras\"\r\n\tTEX_GUID 5\r\n\tTEX_FILENAME \"data\\textures\\gras.tga\"\r\n\tTEX_GROUP_ID 0\r\n}\r\n[TEXTURE]\r\n{\r\n\tTEX_NAME \"Wasser\"\r\n\tTEX_GUID 6\r\n\tTEX_FILENAME \"data\\textures\\wasser.tga\"\r\n}\r\n";
    const WLD: &str = "WORLDFILE V1.08e\r\n[TERRAIN]\r\n{\r\n\tTERRAIN_DIM_X 2\r\n\tTERRAIN_DIM_Z 2\r\n\tTERRAIN_SIZE_X 128\r\n\tTERRAIN_SIZE_Z 128\r\n\tLIGHT_POSITION 0 1 0\r\n\tLIGHT_COLOR 255 255 255\r\n\tLIGHT_INTENSITY 100\r\n\tWATER_LEVEL 0\r\n\tWATER_USETEXTURE 1\r\n\tWATER_BASE_TEXTUREGUID 6\r\n\tWATER_REFRACTION_TEXTUREGUID 6\r\n\tWATER_TEXTURETILING 2\r\n\tWATER_MAXDEPTH 100\r\n\tWATER_MINALPHA 0\r\n\tWATER_MAXALPHA 255\r\n\tCHUNK_SIZE 2\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_GUID 10\r\n\tOBJ_POSITION 64 0 64\r\n\tOBJ_COLOR 0\r\n}\r\n";

    fn level_dir() -> tempfile::TempDir {
        let mut heightmap = flat_heightmap(2, 2, 128.0, 10.0);
        heightmap.points[0].position[1] = -50.0; // corner (0, 0) under water
        game_dir(&[
            ("data/levels/test/test.lvl", LVL.as_bytes().to_vec()),
            ("data/projects/test.prj", PRJ.as_bytes().to_vec()),
            ("data/levels/test/test.wld", WLD.as_bytes().to_vec()),
            ("data/levels/test/test.hlm", write_hlm(&heightmap)),
            (
                "data/levels/test/test.tlm",
                write_tlm(&TextureMap {
                    dim_x: 2,
                    dim_z: 2,
                    guids: vec![5; 4],
                }),
            ),
            (
                "data/r3d/test/kiste.rdo",
                write_rdo(&sample_rdo(RdoLayout::Standard)),
            ),
            ("data/textures/gras.tga", tga([0, 200, 0])),
            ("data/textures/wasser.tga", tga([0, 0, 200])),
        ])
    }

    #[test]
    fn baked_texture_of_a_single_texture_is_that_texture() {
        let dir = level_dir();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/test/test.lvl").unwrap();
        let images = scene.ground_textures(&vfs);
        let chunk = build_chunk_textures(&scene.terrain, &scene.project, &images, 0);
        let baked = bake_chunk_texture(&scene.terrain, &chunk, &images, 4);
        assert_eq!(baked.dimensions(), (8, 8));
        assert!(
            baked.pixels().all(|p| p.0 == [0, 200, 0, 255]),
            "{:?}",
            baked.get_pixel(0, 0)
        );
    }

    #[test]
    fn level_export_contains_ground_water_and_objects() {
        let dir = level_dir();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let scene = load_level(&vfs, "data/levels/test/test.lvl").unwrap();
        let glb = level_to_glb(
            &vfs,
            &scene,
            &LevelExportOptions {
                pixels_per_tile: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let (doc, buffers, _) = gltf::import_slice(&glb).unwrap();
        let roots: Vec<&str> = doc
            .scenes()
            .next()
            .unwrap()
            .nodes()
            .filter_map(|n| n.name())
            .collect();
        assert_eq!(roots, ["Terrain", "Water", "Objects"]);
        let ground = doc.nodes().find(|n| n.name() == Some("Block 0")).unwrap();
        let prim = ground.mesh().unwrap().primitives().next().unwrap();
        let reader = prim.reader(|b| Some(&buffers[b.index()]));
        // 3 x 3 grid points, z mirrored; point (0, 0) is at y = -50.
        let positions: Vec<[f32; 3]> = reader.read_positions().unwrap().collect();
        assert_eq!(positions.len(), 9);
        assert_eq!(positions[0], [0.0, -50.0, 0.0]);
        assert_eq!(positions[1], [0.0, 10.0, -128.0]);
        // Triangles point up (right-handed, counter-clockwise seen from above).
        let indices: Vec<u32> = reader.read_indices().unwrap().into_u32().collect();
        let p = |i: u32| Vec3::from(positions[i as usize]);
        let normal = (p(indices[1]) - p(indices[0])).cross(p(indices[2]) - p(indices[0]));
        assert!(normal.y > 0.0, "{normal:?}");
        // Baked block textures clamp at the edge (no seams at the block boundaries).
        let base_color = prim
            .material()
            .pbr_metallic_roughness()
            .base_color_texture()
            .unwrap();
        let sampler = base_color.texture().sampler();
        assert_eq!(sampler.wrap_s(), gltf::texture::WrappingMode::ClampToEdge);
        assert_eq!(sampler.wrap_t(), gltf::texture::WrappingMode::ClampToEdge);
        // Water: only tile (0, 0) has a corner below the water level.
        let water = doc.nodes().find(|n| n.name() == Some("Water")).unwrap();
        let water_prim = water.mesh().unwrap().primitives().next().unwrap();
        let water_positions: Vec<[f32; 3]> = water_prim
            .reader(|b| Some(&buffers[b.index()]))
            .read_positions()
            .unwrap()
            .collect();
        assert_eq!(water_positions.len(), 4);
        assert!(water_positions.iter().all(|p| p[1] == 0.0));
        let objects = doc.nodes().find(|n| n.name() == Some("Objects")).unwrap();
        assert_eq!(objects.children().count(), 2); // kiste and koerper
    }
}
