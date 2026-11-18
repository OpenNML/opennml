//! Terrain as in the game: grid from `.hlm`, textures per tile from `.tlm`, derived values
//! like `Terrain_ComputeVertexNormal` (`0x0055AD10`), `Terrain_ComputeVertexLighting`
//! (`0x0055B4B0`) and `Terrain_ComputeWaterAlphaAndBorder` (`0x0055A640`); see
//! `docs/formats/hlm.md`.

use anyhow::ensure;
use glam::Vec3;
use opennml_formats::hlm::Heightmap;
use opennml_formats::lightdata::LightBlock;
use opennml_formats::prj::Project;
use opennml_formats::tlm::TextureMap;
use opennml_formats::wld::TerrainParams;

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainVertex {
    /// Exactly the `.hlm` position.
    pub position: Vec3,
    /// Grid point normal of the game: negated, **non-normalized** mean of the adjoining triangle
    /// normals; points up, shorter than 1 on slopes.
    pub normal: Vec3,
    /// `COLOR_DIFFUSE` of the project color of the grid point; `None` = uncolored (white).
    pub color: Option<[u8; 3]>,
    /// Lit vertex color (`TerrainVertex+0x24`).
    pub lit_color: [u8; 3],
    /// Water alpha (`TerrainVertex+0x2C`).
    pub water_alpha: u8,
    /// Grid point at the map edge (`TerrainVertex+0x30`).
    pub is_edge: bool,
}

#[derive(Debug, Clone)]
pub struct Terrain {
    pub params: TerrainParams,
    pub verts_x: usize,
    pub verts_z: usize,
    /// Row by row, `x` inner.
    pub vertices: Vec<TerrainVertex>,
    /// `TEX_GUID` per tile (index `DIM_X * z + x`); `None` if the project file does not know the
    /// GUID (the game then leaves the tile without texture).
    pub tiles: Vec<Option<u32>>,
}

impl Terrain {
    /// Builds the terrain; the sizes of `.hlm` and `.tlm` must match `[TERRAIN]`.
    pub fn new(
        params: &TerrainParams,
        heightmap: &Heightmap,
        texture_map: &TextureMap,
        project: &Project,
    ) -> anyhow::Result<Self> {
        let (dim_x, dim_z) = (params.dim_x as usize, params.dim_z as usize);
        ensure!(
            dim_x >= 1 && dim_z >= 1,
            "[TERRAIN] declares an empty grid ({dim_x} x {dim_z})"
        );
        // `chunk_count` and `chunk_origin` divide by `chunk_size` and `dim_x` respectively.
        let chunk_size = params.chunk_size as usize;
        ensure!(
            chunk_size >= 1 && dim_x.is_multiple_of(chunk_size) && dim_z.is_multiple_of(chunk_size),
            "CHUNK_SIZE {chunk_size} does not divide {dim_x} x {dim_z}"
        );
        ensure!(
            heightmap.verts_x == dim_x + 1 && heightmap.verts_z == dim_z + 1,
            ".hlm has {} x {} grid points, [TERRAIN] requires {} x {}",
            heightmap.verts_x,
            heightmap.verts_z,
            dim_x + 1,
            dim_z + 1
        );
        ensure!(
            texture_map.dim_x == dim_x && texture_map.dim_z == dim_z,
            ".tlm has {} x {} tiles, [TERRAIN] requires {dim_x} x {dim_z}",
            texture_map.dim_x,
            texture_map.dim_z
        );
        ensure!(
            heightmap.points.len() == (dim_x + 1) * (dim_z + 1),
            ".hlm has {} grid points, expected {}",
            heightmap.points.len(),
            (dim_x + 1) * (dim_z + 1)
        );
        ensure!(
            texture_map.guids.len() == dim_x * dim_z,
            ".tlm has {} tile GUIDs, expected {}",
            texture_map.guids.len(),
            dim_x * dim_z
        );
        let verts_x = dim_x + 1;
        let positions: Vec<Vec3> = heightmap
            .points
            .iter()
            .map(|p| Vec3::from(p.position))
            .collect();
        let at = |x: usize, z: usize| positions[z * verts_x + x];
        let mut faces = Vec::with_capacity(dim_x * dim_z);
        for z in 0..dim_z {
            for x in 0..dim_x {
                faces.push(tile_face_normals(
                    at(x, z),
                    at(x, z + 1),
                    at(x + 1, z),
                    at(x + 1, z + 1),
                ));
            }
        }
        let mut vertices = Vec::with_capacity(positions.len());
        for z in 0..=dim_z {
            for x in 0..=dim_x {
                let normal = vertex_normal(&faces, dim_x, dim_z, x, z);
                let color = heightmap
                    .point(x, z)
                    .color_guid
                    .and_then(|g| project.colors.get(&g))
                    .map(|c| c.rgb());
                let position = at(x, z);
                vertices.push(TerrainVertex {
                    position,
                    normal,
                    color,
                    lit_color: [0; 3],
                    water_alpha: water_alpha(position.y, params),
                    is_edge: x == 0 || z == 0 || x == dim_x || z == dim_z,
                });
            }
        }
        let tiles = texture_map
            .guids
            .iter()
            .map(|g| project.textures.contains_key(g).then_some(*g))
            .collect();
        let mut terrain = Self {
            params: params.clone(),
            verts_x,
            verts_z: dim_z + 1,
            vertices,
            tiles,
        };
        terrain.relight();
        Ok(terrain)
    }

    /// Replaces the terrain light and recomputes the vertex colors like
    /// `GameEngine_ApplyLightPreset` (`0x00430C40`): the same function as when loading
    /// (`Terrain_ComputeVertexLighting`), hard switch.
    pub fn set_light(&mut self, light: &LightBlock) {
        self.params.light_position = light.position;
        self.params.light_color = light.color;
        self.params.light_intensity = light.intensity;
        self.relight();
    }

    /// `lit_color` of all grid points from normal, project color and `params.light_*`.
    fn relight(&mut self) {
        let light_dir = Vec3::from(self.params.light_position).normalize_or_zero();
        for v in &mut self.vertices {
            v.lit_color = lit_color(v.normal, v.color, light_dir, &self.params);
        }
    }

    pub fn dim_x(&self) -> usize {
        self.verts_x - 1
    }

    pub fn dim_z(&self) -> usize {
        self.verts_z - 1
    }

    /// Grid point `(x, z)`.
    pub fn vertex(&self, x: usize, z: usize) -> &TerrainVertex {
        &self.vertices[z * self.verts_x + x]
    }

    /// Tiles `(x, z)` with water: at least one corner lies below `WATER_LEVEL`
    /// (`TerrainChunk_DrawWater` `0x005597D0`). Order: z outer, x inner.
    pub fn water_tiles(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let level = self.params.water_level;
        (0..self.dim_z())
            .flat_map(|z| (0..self.dim_x()).map(move |x| (x, z)))
            .filter(move |&(x, z)| {
                [(x, z), (x, z + 1), (x + 1, z), (x + 1, z + 1)]
                    .iter()
                    .any(|&(cx, cz)| self.vertex(cx, cz).position.y < level)
            })
    }

    /// Texture of tile `(x, z)` (inside the map).
    pub fn tile(&self, x: usize, z: usize) -> Option<u32> {
        self.tiles[z * self.dim_x() + x]
    }

    /// Texture of tile `(x, z)`; `None` outside the map.
    pub fn tile_checked(&self, x: isize, z: isize) -> Option<u32> {
        let (x, z) = (usize::try_from(x).ok()?, usize::try_from(z).ok()?);
        (x < self.dim_x() && z < self.dim_z())
            .then(|| self.tile(x, z))
            .flatten()
    }

    /// Terrain height at `(x, z)` (world units), linear on the tile's triangles
    /// (A = `(x,z)`, `(x,z+1)`, `(x+1,z)`; B = `(x,z+1)`, `(x+1,z+1)`, `(x+1,z)`); clamped to the
    /// edge outside the map.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let (dim_x, dim_z) = (self.dim_x(), self.dim_z());
        let fx = (x / self.params.size_x).clamp(0.0, dim_x as f32);
        let fz = (z / self.params.size_z).clamp(0.0, dim_z as f32);
        let tx = (fx as usize).min(dim_x - 1);
        let tz = (fz as usize).min(dim_z - 1);
        let (u, w) = (fx - tx as f32, fz - tz as f32);
        let h = |dx: usize, dz: usize| self.vertex(tx + dx, tz + dz).position.y;
        if u + w <= 1.0 {
            h(0, 0) + u * (h(1, 0) - h(0, 0)) + w * (h(0, 1) - h(0, 0))
        } else {
            h(1, 1) + (1.0 - u) * (h(0, 1) - h(1, 1)) + (1.0 - w) * (h(1, 0) - h(1, 1))
        }
    }
}

impl Terrain {
    /// First intersection of a ray with the terrain inside the map (display and operation, not
    /// the simulation): steps of 32 units up to `max_distance`, then bisection to below 1/16
    /// unit.
    pub fn ray_hit(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Vec3> {
        let dir = direction.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        let p = &self.params;
        let inside = |v: Vec3| (0.0..p.width()).contains(&v.x) && (0.0..p.depth()).contains(&v.z);
        let below = |v: Vec3| inside(v) && v.y <= self.height_at(v.x, v.z);
        const STEP: f32 = 32.0;
        let mut t = 0.0;
        while t <= max_distance {
            let next = t + STEP;
            if below(origin + dir * next) {
                let (mut lo, mut hi) = (t, next);
                for _ in 0..10 {
                    let mid = (lo + hi) * 0.5;
                    if below(origin + dir * mid) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                let hit = origin + dir * hi;
                return Some(Vec3::new(hit.x, self.height_at(hit.x, hit.z), hit.z));
            }
            t = next;
        }
        None
    }

    /// Ground point of the 3D mouse pointer like `FUN_004CCF00` (Cursor3D `0x00671B00`, point
    /// `+0x44`): the terrain hit; if it lies below `WATER_LEVEL` (`ground_height` like
    /// `Terrain_GetHeightAt`), the intersection with the water surface `y = WATER_LEVEL`
    /// instead (line-plane `0x00576F60`, downward only). Without a terrain hit the water surface
    /// applies, provided there is water there (outside the map the height is `WATER_LEVEL`). The
    /// water surface counts only if the intersection lies in front of the origin (camera above
    /// the water level) and within `max_distance`; otherwise the terrain hit or `None` remains.
    /// The overview mode of the original (plane `y = 0`) does not exist in the viewer.
    pub fn cursor_point(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Vec3> {
        let level = self.params.water_level;
        let water = || {
            if direction.y >= 0.0 {
                return None;
            }
            let t = (level - origin.y) / direction.y;
            // NaN and infinity fail the comparison.
            (0.0..=max_distance)
                .contains(&t)
                .then(|| origin + direction * t)
        };
        match self.ray_hit(origin, direction, max_distance) {
            Some(hit) if self.ground_height(hit.x, hit.z) < level => water().or(Some(hit)),
            Some(hit) => Some(hit),
            None => water().filter(|p| self.ground_height(p.x, p.z) <= level),
        }
    }

    /// Ground height like `Terrain_GetHeightAt(x, 0, z)` (`0x0055A940`): outside the map
    /// `WATER_LEVEL`, otherwise the height of the tile (vertical ray, two-sided triangle test
    /// `0x00577440` without distance limit, so any height), rounded down to 0.005.
    pub fn ground_height(&self, x: f32, z: f32) -> f32 {
        let p = &self.params;
        if !(0.0..p.width()).contains(&x) || !(0.0..p.depth()).contains(&z) {
            return p.water_level;
        }
        (self.height_at(x, z) * 200.0).floor() * 0.005
    }
}

/// Face normals of the tile triangles like `Terrain_ComputeTileFaceNormals` (`0x0055AED0`):
/// `normalized((v2 - v0) x (v1 - v0))` for A = `(x,z)`, `(x,z+1)`, `(x+1,z)` and
/// B = `(x,z+1)`, `(x+1,z+1)`, `(x+1,z)`. On level terrain `(0, -1, 0)` (downward).
pub fn tile_face_normals(p00: Vec3, p01: Vec3, p10: Vec3, p11: Vec3) -> (Vec3, Vec3) {
    let a = (p10 - p00).cross(p01 - p00).normalize_or_zero();
    let b = (p10 - p01).cross(p11 - p01).normalize_or_zero();
    (a, b)
}

/// Grid point normal like `Terrain_ComputeVertexNormal` (`0x0055AD10`): sum of the adjoining
/// triangle normals (tile `(x,z)` A, `(x-1,z)` A+B, `(x,z-1)` A+B, `(x-1,z-1)` B; fewer at the
/// edge), negated and divided by the count, **without** normalization.
fn vertex_normal(faces: &[(Vec3, Vec3)], dim_x: usize, dim_z: usize, x: usize, z: usize) -> Vec3 {
    let mut sum = Vec3::ZERO;
    let mut count = 0.0;
    let (x, z) = (x as isize, z as isize);
    for (tx, tz, use_a, use_b) in [
        (x, z, true, false),
        (x - 1, z, true, true),
        (x, z - 1, true, true),
        (x - 1, z - 1, false, true),
    ] {
        if tx < 0 || tz < 0 || tx as usize >= dim_x || tz as usize >= dim_z {
            continue;
        }
        let (a, b) = faces[tz as usize * dim_x + tx as usize];
        if use_a {
            sum += a;
            count += 1.0;
        }
        if use_b {
            sum += b;
            count += 1.0;
        }
    }
    if count == 0.0 { Vec3::Y } else { -sum / count }
}

/// Lit vertex color like `Terrain_ComputeVertexLighting` (`0x0055B4B0`): per channel
/// `clamp(c/255 * f * L, 0, 255)` with `f = n * Lhat + 1 - 0.01 * LIGHT_INTENSITY` (`n` not
/// normalized, `n * Lhat` not clamped at 0), `c` = `COLOR_DIFFUSE` (otherwise 255),
/// `L` = `LIGHT_COLOR`. Do not unify with the object light (re/renderer).
pub fn lit_color(
    normal: Vec3,
    color: Option<[u8; 3]>,
    light_dir: Vec3,
    params: &TerrainParams,
) -> [u8; 3] {
    let f = normal.dot(light_dir) + 1.0 - 0.01 * params.light_intensity as f32;
    let c = color.unwrap_or([255; 3]);
    std::array::from_fn(|k| {
        (c[k] as f32 / 255.0 * f * params.light_color[k] as f32).clamp(0.0, 255.0) as u8
    })
}

/// Water alpha like `Terrain_ComputeWaterAlphaAndBorder` (`0x0055A640`):
/// `clamp(int(255 * sqrt(-(y - WATER_LEVEL) / WATER_MAXDEPTH)), WATER_MINALPHA, WATER_MAXALPHA)`.
/// Above the water the root is NaN; the game then ends up (via `0x80000000`) at
/// `WATER_MINALPHA`, Rust (`NaN as i32` = 0) likewise. Swapped limits are ordered.
pub fn water_alpha(y: f32, params: &TerrainParams) -> u8 {
    let depth = -(y - params.water_level) / params.water_max_depth as f32;
    let raw = (255.0 * depth.sqrt()) as i32;
    let lo = params.water_min_alpha.min(params.water_max_alpha);
    let hi = params.water_min_alpha.max(params.water_max_alpha);
    raw.clamp(lo, hi).clamp(0, 255) as u8
}

/// Brightness factor of the fog of war per grid point (`0x00465F60`, re/vision 8.2)
/// as a byte (255 = 1.0), row by row `verts_x * z + x` over `(dim_x + 1) x (dim_z + 1)` points.
/// Missing points count as 255. Not to be confused with the renderer's distance fog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FogFactors {
    pub verts_x: usize,
    pub values: Vec<u8>,
}

impl FogFactors {
    pub fn get(&self, x: usize, z: usize) -> u8 {
        if x >= self.verts_x {
            return 255;
        }
        self.values
            .get(z * self.verts_x + x)
            .copied()
            .unwrap_or(255)
    }

    /// Color * factor per channel (`TerrainChunk_UpdateVertexColors` `0x00557070`; factor
    /// `value * 1/255` as `f32`, truncated like `ftol` - hypothesis, display only).
    pub fn apply(&self, x: usize, z: usize, color: [u8; 3]) -> [u8; 3] {
        let k = f32::from(self.get(x, z)) * (1.0 / 255.0);
        color.map(|c| (f32::from(c) * k) as u8)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use opennml_formats::prj::{ProjectColor, ProjectTexture};
    use opennml_formats::testutil::flat_heightmap;

    /// Level test terrain `dim x dim` tiles of 128 units; every tile has texture `tile(x, z)`;
    /// project with textures 1 (group 1), 2 (group 2), 3 (group 0) and color 9 (red).
    pub(crate) fn test_terrain(dim: u32, tile: impl Fn(usize, usize) -> u32) -> (Terrain, Project) {
        let params = TerrainParams {
            dim_x: dim,
            dim_z: dim,
            light_position: [0.0, 1.0, 0.0],
            light_color: [200, 200, 200],
            light_intensity: 100,
            chunk_size: dim.min(16),
            ..TerrainParams::default()
        };
        let mut project = Project::default();
        for (guid, group) in [(1, 1), (2, 2), (3, 0)] {
            project.textures.insert(
                guid,
                ProjectTexture {
                    guid,
                    name: format!("t{guid}"),
                    file: format!("data/textures/t{guid}.tga"),
                    bits: 0,
                    group,
                    map_color: 0,
                },
            );
        }
        project.colors.insert(
            9,
            ProjectColor {
                guid: 9,
                name: "red".into(),
                diffuse: 0x0000FF,
            },
        );
        let heightmap = flat_heightmap(dim, dim, 128.0, 0.0);
        let d = dim as usize;
        let texture_map = TextureMap {
            dim_x: d,
            dim_z: d,
            guids: (0..d * d).map(|i| tile(i % d, i / d)).collect(),
        };
        (
            Terrain::new(&params, &heightmap, &texture_map, &project).unwrap(),
            project,
        )
    }

    #[test]
    fn new_rejects_inconsistent_inputs() {
        let params = TerrainParams {
            dim_x: 2,
            dim_z: 2,
            chunk_size: 2,
            ..TerrainParams::default()
        };
        let heightmap = flat_heightmap(2, 2, 128.0, 0.0);
        let texture_map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![1; 4],
        };
        let project = Project::default();
        assert!(Terrain::new(&params, &heightmap, &texture_map, &project).is_ok());

        // Point list does not match verts_x * verts_z.
        let mut short = heightmap.clone();
        short.points.pop();
        assert!(Terrain::new(&params, &short, &texture_map, &project).is_err());
        // Tile list does not match dim_x * dim_z.
        let short_tiles = TextureMap {
            guids: vec![1; 3],
            ..texture_map.clone()
        };
        assert!(Terrain::new(&params, &heightmap, &short_tiles, &project).is_err());
        // Empty grid, CHUNK_SIZE 0 and non-dividing CHUNK_SIZE.
        let empty = TerrainParams {
            dim_x: 0,
            ..params.clone()
        };
        assert!(Terrain::new(&empty, &heightmap, &texture_map, &project).is_err());
        for chunk_size in [0, 3] {
            let bad = TerrainParams {
                chunk_size,
                ..params.clone()
            };
            assert!(Terrain::new(&bad, &heightmap, &texture_map, &project).is_err());
        }
    }

    #[test]
    fn flat_terrain_has_up_normals_and_down_face_normals() {
        let (terrain, _) = test_terrain(4, |_, _| 1);
        for v in &terrain.vertices {
            assert!((v.normal - Vec3::Y).length() < 1e-6, "{:?}", v.normal);
        }
        let p = |x: f32, z: f32| Vec3::new(x * 128.0, 0.0, z * 128.0);
        let (a, b) = tile_face_normals(p(0.0, 0.0), p(0.0, 1.0), p(1.0, 0.0), p(1.0, 1.0));
        assert_eq!((a, b), (-Vec3::Y, -Vec3::Y));
        assert!(terrain.vertex(0, 2).is_edge && terrain.vertex(4, 4).is_edge);
        assert!(!terrain.vertex(2, 2).is_edge);
    }

    #[test]
    fn vertex_normal_averages_without_normalizing() {
        // One point in the middle pulled up: the neighbors get tilted, shorter normals.
        let params = TerrainParams {
            dim_x: 2,
            dim_z: 2,
            chunk_size: 2,
            ..TerrainParams::default()
        };
        let mut heightmap = flat_heightmap(2, 2, 128.0, 0.0);
        heightmap.points[4].position[1] = 128.0;
        let texture_map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![1; 4],
        };
        let terrain = Terrain::new(&params, &heightmap, &texture_map, &Project::default()).unwrap();
        // Corner (0, 0): only triangle A of tile (0, 0), which stays level.
        let corner = terrain.vertex(0, 0).normal;
        assert!((corner - Vec3::Y).length() < 1e-6, "{corner:?}");
        // Edge point (1, 0): three triangles, two of them tilted toward the elevation at (1, 1) ->
        // normal tilts toward -z and is shorter than 1 (~ (-0.19, 0.76, -0.43)).
        let edge = terrain.vertex(1, 0).normal;
        assert!(edge.y > 0.0 && edge.z < 0.0, "{edge:?}");
        assert!(edge.length() < 0.95, "{edge:?}");
        assert!(terrain.tiles.iter().all(Option::is_none)); // GUID 1 unknown
    }

    #[test]
    fn lighting_and_water_alpha_follow_the_game_formulas() {
        let params = TerrainParams {
            light_color: [200, 100, 50],
            light_intensity: 20,
            water_level: 0.0,
            water_max_depth: 600,
            water_min_alpha: 60,
            water_max_alpha: 150,
            ..TerrainParams::default()
        };
        // f = n*L + 1 - 0.2 = 1.8 at n = L = (0, 1, 0); white: 1.8 * L, limited to 255.
        assert_eq!(lit_color(Vec3::Y, None, Vec3::Y, &params), [255, 180, 90]);
        // Color (255, 0, 0): only red remains; light from below: f = -1 + 0.8 < 0 -> black.
        assert_eq!(
            lit_color(Vec3::Y, Some([255, 0, 0]), Vec3::Y, &params),
            [255, 0, 0]
        );
        assert_eq!(lit_color(Vec3::Y, None, -Vec3::Y, &params), [0, 0, 0]);
        // Water alpha: 255*sqrt(300/600) = 180 -> limited to 150; flat -> 60; above water -> 60.
        assert_eq!(water_alpha(-300.0, &params), 150);
        assert_eq!(water_alpha(-2.0, &params), 60);
        assert_eq!(water_alpha(50.0, &params), 60);
        let swapped = TerrainParams {
            water_min_alpha: 200,
            water_max_alpha: 100,
            ..params
        };
        assert_eq!(water_alpha(50.0, &swapped), 100);
    }

    #[test]
    fn colored_vertices_and_tile_lookup() {
        let tile = |x: usize, _| if x == 0 { 1 } else { 99 };
        let (terrain, project) = test_terrain(2, tile);
        assert_eq!(terrain.tile(0, 1), Some(1));
        assert_eq!(terrain.tile(1, 1), None);
        assert_eq!(terrain.tile_checked(-1, 0), None);
        assert_eq!(terrain.tile_checked(2, 0), None);
        assert_eq!(terrain.vertex(1, 1).lit_color, [200, 200, 200]);
        assert!(project.colors.contains_key(&9));

        // Grid point (0, 0) gets project color 9 (COLORREF 0x0000FF = red (255, 0, 0)); (1, 1)
        // stays uncolored. Level terrain, light vertical, LIGHT_INTENSITY 100:
        // f = n*Lhat + 1 - 1 = 1, so channel = c/255 * 1 * 200 -> (200, 0, 0) instead of (200, 200, 200).
        let mut heightmap = flat_heightmap(2, 2, 128.0, 0.0);
        heightmap.points[0].color_guid = Some(9);
        // A color GUID unknown to the project acts like "uncolored".
        heightmap.points[2].color_guid = Some(12345);
        let texture_map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: (0..4).map(|i| tile(i % 2, i / 2)).collect(),
        };
        let colored = Terrain::new(&terrain.params, &heightmap, &texture_map, &project).unwrap();
        assert_eq!(colored.vertex(0, 0).lit_color, [200, 0, 0]);
        assert_eq!(colored.vertex(1, 1).lit_color, [200, 200, 200]);
        assert_eq!(colored.vertex(2, 0).lit_color, [200, 200, 200]);
    }

    #[test]
    fn set_light_recomputes_with_the_load_formula() {
        let (mut terrain, project) = test_terrain(2, |_, _| 1);
        let mut heightmap = flat_heightmap(2, 2, 128.0, 0.0);
        heightmap.points[0].color_guid = Some(9);
        heightmap.points[4].position[1] = 64.0;
        let map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![1; 4],
        };
        terrain = Terrain::new(&terrain.params, &heightmap, &map, &project).unwrap();
        let loaded: Vec<[u8; 3]> = terrain.vertices.iter().map(|v| v.lit_color).collect();
        // Night: f = n*Lhat + 1 - 0.15, color (82, 90, 93); grid point (0, 0) colored red.
        let night = LightBlock {
            position: [0.0, 1.0, 0.0],
            color: [82, 90, 93],
            intensity: 15,
        };
        terrain.set_light(&night);
        assert_eq!(terrain.params.light_color, [82, 90, 93]);
        let v = terrain.vertex(0, 0);
        assert_eq!(
            v.lit_color,
            lit_color(v.normal, Some([255, 0, 0]), Vec3::Y, &terrain.params)
        );
        assert_eq!(terrain.vertex(2, 2).lit_color, [151, 166, 172]);
        // Back to the light from loading: the same colors as when loading.
        terrain.set_light(&LightBlock {
            position: [0.0, 1.0, 0.0],
            color: [200, 200, 200],
            intensity: 100,
        });
        let back: Vec<[u8; 3]> = terrain.vertices.iter().map(|v| v.lit_color).collect();
        assert_eq!(back, loaded);
    }

    #[test]
    fn rays_hit_the_ground_inside_the_map() {
        let (terrain, _) = test_terrain(4, |_, _| 1);
        let hit = terrain
            .ray_hit(
                Vec3::new(100.0, 500.0, 200.0),
                Vec3::new(0.0, -1.0, 0.5),
                5000.0,
            )
            .unwrap();
        assert!(
            (hit.x - 100.0).abs() < 0.01 && (hit.z - 450.0).abs() < 0.1,
            "{hit}"
        );
        assert_eq!(hit.y, 0.0);
        // Upward or outside the map: no hit.
        assert!(
            terrain
                .ray_hit(Vec3::new(100.0, 10.0, 100.0), Vec3::Y, 5000.0)
                .is_none()
        );
        assert!(
            terrain
                .ray_hit(Vec3::new(-100.0, 10.0, 100.0), -Vec3::Y, 5000.0)
                .is_none()
        );
    }

    // QUIRK D-217: kept
    #[test]
    fn the_cursor_point_over_water_lies_on_the_water_surface() {
        let (mut terrain, _) = test_terrain(4, |_, _| 1); // flat at 0
        terrain.params.water_level = 50.0; // everything under water
        let (o, d) = (Vec3::new(100.0, 500.0, 200.0), Vec3::new(0.0, -1.0, 0.5));
        assert_eq!(terrain.ray_hit(o, d, 5000.0).unwrap().z.round(), 450.0); // sea floor
        let p = terrain.cursor_point(o, d, 5000.0).unwrap();
        assert_eq!(p.y, 50.0);
        assert!(
            (p.x - 100.0).abs() < 0.01 && (p.z - 425.0).abs() < 0.01,
            "{p}"
        );
        // Land (water level below the terrain): terrain hit as before.
        terrain.params.water_level = -10.0;
        assert_eq!(
            terrain.cursor_point(o, d, 5000.0),
            terrain.ray_hit(o, d, 5000.0)
        );
    }

    // QUIRK D-217: kept
    #[test]
    fn the_cursor_point_outside_the_map_is_the_water_surface_only_for_downward_rays() {
        let (terrain, _) = test_terrain(4, |_, _| 1); // water level 0 = terrain height
        // Outside the map WATER_LEVEL applies: the downward ray hits the water plane (0x00576F60) ...
        let p = terrain
            .cursor_point(Vec3::new(-100.0, 10.0, 100.0), -Vec3::Y, 5000.0)
            .unwrap();
        assert_eq!(p, Vec3::new(-100.0, 0.0, 100.0));
        // ... upward or horizontal there is no intersection.
        assert!(
            terrain
                .cursor_point(Vec3::new(-100.0, 10.0, 100.0), Vec3::Y, 5000.0)
                .is_none()
        );
        assert!(
            terrain
                .cursor_point(Vec3::new(-100.0, 10.0, 100.0), Vec3::X, 5000.0)
                .is_none()
        );
    }

    #[test]
    fn the_water_plane_counts_only_in_front_of_the_camera_and_within_range() {
        let (mut terrain, _) = test_terrain(4, |_, _| 1); // flat at 0
        terrain.params.water_level = 50.0;
        // Camera below the water level: the intersection would lie behind it (t < 0); the terrain
        // hit remains, not a point above the camera.
        let (o, d) = (Vec3::new(100.0, 20.0, 200.0), -Vec3::Y);
        let ground = terrain.ray_hit(o, d, 5000.0).unwrap();
        assert_eq!(ground.y, 0.0);
        assert_eq!(terrain.cursor_point(o, d, 5000.0), Some(ground));
        // Camera above the water: the intersection lies in front (control case).
        let above = Vec3::new(100.0, 200.0, 200.0);
        assert_eq!(
            terrain.cursor_point(above, d, 5000.0),
            Some(Vec3::new(100.0, 50.0, 200.0))
        );
        // Outside the map, water plane beyond the range: no point.
        let outside = Vec3::new(-100.0, 500.0, 100.0);
        assert_eq!(
            terrain.cursor_point(outside, -Vec3::Y, 5000.0),
            Some(Vec3::new(-100.0, 50.0, 100.0))
        );
        assert_eq!(terrain.cursor_point(outside, -Vec3::Y, 100.0), None);
        // Almost horizontal: the intersection lies at infinity (not finite or beyond the range).
        let flat = Vec3::new(1.0, -1.0e-30, 0.0).normalize();
        assert_eq!(terrain.cursor_point(outside, flat, 5000.0), None);
    }

    #[test]
    fn height_follows_the_triangle_split() {
        let params = TerrainParams {
            dim_x: 1,
            dim_z: 1,
            chunk_size: 1,
            ..TerrainParams::default()
        };
        let mut heightmap = flat_heightmap(1, 1, 128.0, 0.0);
        heightmap.points[3].position[1] = 100.0; // point (1, 1)
        let texture_map = TextureMap {
            dim_x: 1,
            dim_z: 1,
            guids: vec![1],
        };
        let terrain = Terrain::new(&params, &heightmap, &texture_map, &Project::default()).unwrap();
        // Triangle A (x + z <= 1 tile) does not contain (1, 1): height 0.
        assert_eq!(terrain.height_at(32.0, 32.0), 0.0);
        // Triangle B: middle of the diagonal up to (1, 1).
        assert!((terrain.height_at(96.0, 96.0) - 50.0).abs() < 1e-4);
        assert_eq!(terrain.height_at(500.0, 500.0), 100.0);
    }

    #[test]
    fn ground_height_rounds_down_and_is_water_outside_the_map() {
        let params = TerrainParams {
            dim_x: 1,
            dim_z: 1,
            chunk_size: 1,
            water_level: -7.0,
            ..TerrainParams::default()
        };
        let mut heightmap = flat_heightmap(1, 1, 128.0, 0.0);
        heightmap.points[3].position[1] = 100.0;
        heightmap.points[0].position[1] = 3001.0;
        let texture_map = TextureMap {
            dim_x: 1,
            dim_z: 1,
            guids: vec![1],
        };
        let terrain = Terrain::new(&params, &heightmap, &texture_map, &Project::default()).unwrap();
        // Triangle B: height 50.0 exactly; rounded down to 0.005.
        assert!((terrain.ground_height(96.0, 96.0) - 50.0).abs() < 1e-3);
        let h = terrain.ground_height(100.0, 97.0);
        assert!(h <= terrain.height_at(100.0, 97.0) && terrain.height_at(100.0, 97.0) - h < 0.006);
        // Even above 1000 the ray hits (no distance limit).
        assert!(terrain.ground_height(0.0, 0.0) > 3000.0);
        assert_eq!(terrain.ground_height(-1.0, 50.0), -7.0);
        assert_eq!(terrain.ground_height(50.0, 128.0), -7.0);
    }

    #[test]
    fn mismatched_sizes_are_errors() {
        let params = TerrainParams {
            dim_x: 2,
            dim_z: 2,
            chunk_size: 2,
            ..TerrainParams::default()
        };
        let heightmap = flat_heightmap(3, 2, 128.0, 0.0);
        let texture_map = TextureMap {
            dim_x: 2,
            dim_z: 2,
            guids: vec![1; 4],
        };
        assert!(Terrain::new(&params, &heightmap, &texture_map, &Project::default()).is_err());
    }

    #[test]
    fn water_tiles_are_tiles_with_a_corner_below_the_water_level() {
        let params = TerrainParams {
            dim_x: 4,
            dim_z: 2,
            chunk_size: 2,
            water_level: 0.0,
            ..TerrainParams::default()
        };
        let mut heightmap = flat_heightmap(4, 2, 128.0, 10.0);
        // Grid point (2, 1) under water: belongs to the tiles (1, 0), (2, 0), (1, 1), (2, 1).
        heightmap.points[5 + 2].position[1] = -1.0;
        let map = TextureMap {
            dim_x: 4,
            dim_z: 2,
            guids: vec![1; 8],
        };
        let terrain = Terrain::new(&params, &heightmap, &map, &Project::default()).unwrap();
        let tiles: Vec<_> = terrain.water_tiles().collect();
        assert_eq!(tiles, vec![(1, 0), (2, 0), (1, 1), (2, 1)]);
    }

    #[test]
    fn fog_factors_scale_colors_per_channel_and_default_to_one() {
        let f = FogFactors {
            verts_x: 2,
            values: vec![0, 0x80, 0xFF, 1],
        };
        assert_eq!(f.apply(1, 0, [200, 100, 3]), [100, 50, 1]);
        assert_eq!(f.apply(0, 0, [255, 255, 255]), [0, 0, 0]);
        assert_eq!(f.apply(0, 1, [200, 7, 0]), [200, 7, 0]);
        assert_eq!((f.get(5, 5), f.apply(9, 9, [9, 9, 9])), (255, [9, 9, 9]));
    }
}
