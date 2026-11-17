//! Visible tiles like `Terrain_UpdateVisibility` (`0x00558370`): quadtree over the terrain
//! (`TerrainQuadtree_Build` `0x0053F350` from `Terrain_BuildChunks`, root `DIM_X` tiles, leaves
//! one tile) against the six planes of the view frustum (`Renderer_ExtractFrustumPlanes`
//! `0x005729D0`), see re/weather-and-light.

use glam::{Mat4, Vec3, Vec4};

use crate::terrain::Terrain;

/// Position of a box relative to the view frustum (`FUN_00571DF0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    Outside,
    Partial,
    Inside,
}

/// Six planes `n * p + d`, positive = inside.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// Planes from `projection * view` (column vectors): row 3 +- row 0, +- row 1, +- row 2,
    /// normalized. As in the original the near plane is `w + z >= 0`, not `z >= 0`.
    pub fn from_view_proj(m: Mat4) -> Self {
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2].map(|p| {
            let length = p.truncate().length();
            if length > 0.0 { p / length } else { p }
        });
        Self { planes }
    }

    /// Sphere against all planes like `0x00571D70`: outside if the distance at one plane is at
    /// most `-r`, fully inside if it is greater than `r` at all of them, otherwise partial.
    pub fn classify_sphere(&self, center: Vec3, radius: f32) -> Containment {
        let mut all_inside = true;
        for plane in &self.planes {
            let d = plane.truncate().dot(center) + plane.w;
            if d <= -radius {
                return Containment::Outside;
            }
            all_inside &= d > radius;
        }
        if all_inside {
            Containment::Inside
        } else {
            Containment::Partial
        }
    }

    /// Eight corners of the box against all planes: no corner inside for one plane -> outside,
    /// all corners inside for all planes -> fully inside, otherwise partial.
    pub fn classify_box(&self, min: Vec3, max: Vec3) -> Containment {
        let corners = [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(min.x, max.y, max.z),
            Vec3::new(max.x, max.y, max.z),
        ];
        let mut all_inside = true;
        for plane in &self.planes {
            let inside = corners
                .iter()
                .filter(|c| plane.truncate().dot(**c) + plane.w > 0.0)
                .count();
            if inside == 0 {
                return Containment::Outside;
            }
            all_inside &= inside == corners.len();
        }
        if all_inside {
            Containment::Inside
        } else {
            Containment::Partial
        }
    }
}

/// Square tile rectangle from `(x0, z0)` with edge length `size` (clipped at the map edge, see
/// [`TileRect::tiles`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRect {
    pub x0: usize,
    pub z0: usize,
    pub size: usize,
}

impl TileRect {
    /// Tiles inside `dim_x x dim_z`, x outer, z inner (like
    /// `Terrain_DrawWeatherParticlesInRect` `0x00559DE0`).
    pub fn tiles(&self, dim_x: usize, dim_z: usize) -> impl Iterator<Item = (usize, usize)> {
        let (x0, z0) = (self.x0, self.z0);
        let x1 = (x0 + self.size).min(dim_x);
        let z1 = (z0 + self.size).min(dim_z);
        (x0..x1).flat_map(move |x| (z0..z1).map(move |z| (x, z)))
    }
}

#[derive(Debug, Clone)]
struct Node {
    rect: TileRect,
    min: Vec3,
    max: Vec3,
    /// Order like `FUN_0053EE60`: (x0, z0), (x0 + h, z0), (x0, z0 + h), (x0 + h, z0 + h).
    children: Vec<usize>,
}

/// Quadtree of the terrain with hull boxes per node.
#[derive(Debug, Clone)]
pub struct TerrainQuadtree {
    nodes: Vec<Node>,
}

impl TerrainQuadtree {
    /// Builds the tree like `TerrainQuadtree_Build(0, 0, DIM_X, 1)`: edge length halved per level
    /// (integer), leaves one tile. y limits like `FUN_00556CE0`: smallest and largest height of
    /// the node's grid points, the maximum at least `WATER_LEVEL`.
    ///
    /// As in the original the root covers `DIM_X x DIM_X` tiles (in the shipped data all maps are
    /// square with power-of-two edges); nodes outside the map are not created.
    pub fn new(terrain: &Terrain) -> Self {
        let mut tree = Self { nodes: Vec::new() };
        tree.build(terrain, 0, 0, terrain.dim_x());
        tree
    }

    fn build(&mut self, terrain: &Terrain, x0: usize, z0: usize, size: usize) -> Option<usize> {
        if x0 >= terrain.dim_x() || z0 >= terrain.dim_z() || size == 0 {
            return None;
        }
        let p = &terrain.params;
        let (mut y_min, mut y_max) = (f32::INFINITY, f32::NEG_INFINITY);
        for x in x0..=(x0 + size).min(terrain.dim_x()) {
            for z in z0..=(z0 + size).min(terrain.dim_z()) {
                let y = terrain.vertex(x, z).position.y;
                y_min = y_min.min(y);
                y_max = y_max.max(y);
            }
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            rect: TileRect { x0, z0, size },
            min: Vec3::new(x0 as f32 * p.size_x, y_min, z0 as f32 * p.size_z),
            max: Vec3::new(
                (x0 + size) as f32 * p.size_x,
                y_max.max(p.water_level),
                (z0 + size) as f32 * p.size_z,
            ),
            children: Vec::new(),
        });
        if size > 1 {
            let h = size / 2;
            let children: Vec<usize> = [(x0, z0), (x0 + h, z0), (x0, z0 + h), (x0 + h, z0 + h)]
                .into_iter()
                .filter_map(|(x, z)| self.build(terrain, x, z, h))
                .collect();
            self.nodes[index].children = children;
        }
        Some(index)
    }

    /// Rectangles of the nodes that `TerrainQuadtree_DrawWeather` (`0x0053F270`) traverses: fully
    /// visible nodes and partially visible leaves, in traversal order.
    pub fn visible_rects(&self, frustum: &Frustum) -> Vec<TileRect> {
        let mut rects = Vec::new();
        if !self.nodes.is_empty() {
            self.visit(0, frustum, &mut rects);
        }
        rects
    }

    fn visit(&self, index: usize, frustum: &Frustum, rects: &mut Vec<TileRect>) {
        let node = &self.nodes[index];
        match frustum.classify_box(node.min, node.max) {
            Containment::Outside => {}
            Containment::Inside => rects.push(node.rect),
            Containment::Partial if node.rect.size == 1 => rects.push(node.rect),
            Containment::Partial => {
                for &child in &node.children {
                    self.visit(child, frustum, rects);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::look_at_lh;
    use crate::terrain::tests::test_terrain;

    /// Vertical parallel projection onto the rectangle `[x0, x1] x [z0, z1]` (view downward).
    fn top_down(x0: f32, x1: f32, z0: f32, z1: f32) -> Mat4 {
        let center = Vec3::new((x0 + x1) / 2.0, 1000.0, (z0 + z1) / 2.0);
        let view = look_at_lh(center, center - Vec3::Y * 1000.0, Vec3::Z);
        let projection = Mat4::from_cols(
            glam::Vec4::new(2.0 / (x1 - x0), 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 2.0 / (z1 - z0), 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 1.0 / 2000.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 0.0, 1.0),
        );
        projection * view
    }

    fn tiles(rects: &[TileRect], dim: usize) -> Vec<(usize, usize)> {
        let mut t: Vec<_> = rects.iter().flat_map(|r| r.tiles(dim, dim)).collect();
        t.sort_unstable();
        t
    }

    #[test]
    fn boxes_are_outside_partial_or_inside() {
        let frustum = Frustum::from_view_proj(top_down(0.0, 100.0, 0.0, 100.0));
        let inside = frustum.classify_box(Vec3::new(10.0, 0.0, 10.0), Vec3::new(20.0, 5.0, 20.0));
        let partial = frustum.classify_box(Vec3::new(90.0, 0.0, 10.0), Vec3::new(110.0, 5.0, 20.0));
        let outside =
            frustum.classify_box(Vec3::new(110.0, 0.0, 10.0), Vec3::new(120.0, 5.0, 20.0));
        assert_eq!(
            (inside, partial, outside),
            (
                Containment::Inside,
                Containment::Partial,
                Containment::Outside
            )
        );
    }

    #[test]
    fn spheres_are_outside_partial_or_inside() {
        let frustum = Frustum::from_view_proj(top_down(0.0, 100.0, 0.0, 100.0));
        let at = |x: f32| frustum.classify_sphere(Vec3::new(x, 0.0, 50.0), 10.0);
        assert_eq!(at(50.0), Containment::Inside);
        assert_eq!(at(95.0), Containment::Partial);
        // Distance exactly -r: outside.
        assert_eq!(at(110.0), Containment::Outside);
        assert_eq!(at(109.0), Containment::Partial);
    }

    /// Original `FUN_00571DF0`: a corner counts as inside only at plane distance > 0; a box
    /// that touches the view frustum only on the plane lies outside.
    #[test]
    fn corners_on_the_plane_are_outside() {
        let frustum = Frustum::from_view_proj(top_down(0.0, 100.0, 0.0, 100.0));
        let touching =
            frustum.classify_box(Vec3::new(100.0, 0.0, 10.0), Vec3::new(110.0, 5.0, 20.0));
        assert_eq!(touching, Containment::Outside);
    }

    #[test]
    fn whole_map_in_view_is_one_rectangle() {
        let (terrain, _) = test_terrain(8, |_, _| 1);
        let tree = TerrainQuadtree::new(&terrain);
        let rects = tree.visible_rects(&Frustum::from_view_proj(top_down(
            -10.0, 1100.0, -10.0, 1100.0,
        )));
        assert_eq!(
            rects,
            [TileRect {
                x0: 0,
                z0: 0,
                size: 8
            }]
        );
    }

    #[test]
    fn partly_visible_map_gives_the_tiles_under_the_view() {
        let (terrain, _) = test_terrain(8, |_, _| 1);
        let tree = TerrainQuadtree::new(&terrain);
        // View onto x in [100, 300], z in [0, 1024]: tile columns 0 (up to 128), 1 and 2 (from 256).
        let rects = tree.visible_rects(&Frustum::from_view_proj(top_down(
            100.0, 300.0, 0.0, 1024.0,
        )));
        let visible = tiles(&rects, 8);
        let expected: Vec<(usize, usize)> =
            (0..3).flat_map(|x| (0..8).map(move |z| (x, z))).collect();
        assert_eq!(visible, expected);
        // Column 0 lies only partially in the image: the traversal starts with leaf (0, 0).
        assert_eq!(
            rects[0],
            TileRect {
                x0: 0,
                z0: 0,
                size: 1
            }
        );
        // Nothing in the image: no tiles.
        let away = Frustum::from_view_proj(top_down(5000.0, 6000.0, 5000.0, 6000.0));
        assert!(tree.visible_rects(&away).is_empty());
    }

    #[test]
    fn node_heights_reach_the_water_level() {
        let (mut terrain, _) = test_terrain(2, |_, _| 1);
        terrain.params.water_level = 50.0;
        let tree = TerrainQuadtree::new(&terrain);
        assert_eq!(tree.nodes.len(), 5);
        assert_eq!((tree.nodes[0].min.y, tree.nodes[0].max.y), (0.0, 50.0));
        assert_eq!(tree.nodes[0].max.x, 256.0);
        assert_eq!(
            tree.nodes[2].rect,
            TileRect {
                x0: 1,
                z0: 0,
                size: 1
            }
        );
    }
}
