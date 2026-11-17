//! Tile A* of the original (`GridAStar`, `PathPlanner+0x2C`, re/tile-paths 1):
//! Only what has the same region ID as the A* start cell is passable; step cost 1 also
//! diagonally, heuristic `4 · Manhattan`, OPEN sorted by `f` with ties LIFO, without
//! re-sorting on a better `g` (weakness S2), CLOSED is never reopened (only
//! `PropagateDown`), no node or time limit. The nodes live in an array instead of individually
//! on the heap; orders and ties are those of the original.

use super::grid::LayerView;

/// No node (null pointer of the original).
const NIL: u32 = u32::MAX;

#[derive(Debug, Clone)]
struct Node {
    f: i32,
    h: i32,
    g: i32,
    x: i32,
    z: i32,
    idx: usize,
    parent: u32,
    /// Every neighbor handled via `LinkChildNode` in insertion order (at most 8).
    children: Vec<u32>,
    /// Next node of the OPEN or CLOSED list.
    next: u32,
}

/// Result of a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AStarPath {
    /// Cells from the A* goal (for `PathPlanner_FindTilePath` the start cell of the unit) up to the cell
    /// before the A* start; the A* start itself is missing (root without a parent node).
    pub cells: Vec<(i32, i32)>,
    /// Expansions (`+0x68`).
    pub expansions: u32,
}

/// Search space of a search: one layer (with a temporarily overwritten cell).
struct Search<'a> {
    view: LayerView<'a>,
    width: i32,
    start_region: u16,
    nodes: Vec<Node>,
    /// Head of the OPEN list.
    open: u32,
    open_at: Vec<u32>,
    closed_at: Vec<u32>,
}

impl Search<'_> {
    fn idx(&self, x: i32, z: i32) -> usize {
        (z * self.width + x) as usize
    }

    /// `GridAStar_IsCellInStartRegion` (`0x00406360`): the only passability criterion.
    fn in_start_region(&self, x: i32, z: i32) -> bool {
        self.view.inside(x, z) && self.view.region_id(x, z) == self.start_region
    }

    fn new_node(&mut self, node: Node) -> u32 {
        self.nodes.push(node);
        (self.nodes.len() - 1) as u32
    }

    /// `GridAStar_InsertOpenSorted` (`0x004064D0`): before the first node with `f ≥ f_new`.
    fn insert_open(&mut self, n: u32) {
        let idx = self.nodes[n as usize].idx;
        self.open_at[idx] = n;
        let f = self.nodes[n as usize].f;
        let (mut prev, mut cur) = (NIL, self.open);
        while cur != NIL && self.nodes[cur as usize].f < f {
            prev = cur;
            cur = self.nodes[cur as usize].next;
        }
        self.nodes[n as usize].next = cur;
        if prev == NIL {
            self.open = n;
        } else {
            self.nodes[prev as usize].next = n;
        }
    }

    /// `GridAStar_PopBestOpenNode` (`0x00406460`): head of OPEN to CLOSED.
    fn pop_best(&mut self) -> Option<u32> {
        let n = self.open;
        if n == NIL {
            return None;
        }
        let idx = self.nodes[n as usize].idx;
        self.open = self.nodes[n as usize].next;
        self.open_at[idx] = NIL;
        self.closed_at[idx] = n;
        Some(n)
    }

    /// `GridAStar_ExpandNode8` (`0x00406AA0`): first the diagonals (only if both
    /// orthogonal neighbors are passable), then the four orthogonals.
    fn expand(&mut self, n: u32, goal: (i32, i32)) {
        let (x, z) = (self.nodes[n as usize].x, self.nodes[n as usize].z);
        for (dx, dz) in [(-1, -1), (1, -1), (1, 1), (-1, 1)] {
            if self.in_start_region(x + dx, z + dz)
                && self.in_start_region(x + dx, z)
                && self.in_start_region(x, z + dz)
            {
                self.link(n, x + dx, z + dz, goal);
            }
        }
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            if self.in_start_region(x + dx, z + dz) {
                self.link(n, x + dx, z + dz, goal);
            }
        }
    }

    /// `GridAStar_LinkChildNode` (`0x00406930`) with step cost 1.
    fn link(&mut self, p: u32, nx: i32, nz: i32, goal: (i32, i32)) {
        let g = self.nodes[p as usize].g + 1;
        let i = self.idx(nx, nz);
        let open = self.open_at[i];
        if open != NIL {
            self.nodes[p as usize].children.push(open);
            let n = &mut self.nodes[open as usize];
            if g < n.g {
                // Quirk (S2): `f` changes, OPEN is not re-sorted.
                n.parent = p;
                n.g = g;
                n.f = n.h + g;
            }
            return;
        }
        let closed = self.closed_at[i];
        if closed != NIL {
            self.nodes[p as usize].children.push(closed);
            let n = &mut self.nodes[closed as usize];
            if g < n.g {
                n.f = n.h + g;
                n.parent = p;
                n.g = g;
                self.propagate_down(closed);
            }
            return;
        }
        let h = ((nx - goal.0).abs() + (nz - goal.1).abs()) << 2;
        let n = self.new_node(Node {
            f: h + g,
            h,
            g,
            x: nx,
            z: nz,
            idx: i,
            parent: p,
            children: Vec::new(),
            next: NIL,
        });
        self.insert_open(n);
        self.nodes[p as usize].children.push(n);
    }

    /// `GridAStar_PropagateDown` (`0x00406880`): pass a better `g` on to the children, stack
    /// LIFO; children in OPEN change `f` without re-sorting.
    fn propagate_down(&mut self, n: u32) {
        let mut stack = vec![n];
        while let Some(m) = stack.pop() {
            let g = self.nodes[m as usize].g + 1;
            for k in 0..self.nodes[m as usize].children.len() {
                let c = self.nodes[m as usize].children[k];
                let child = &mut self.nodes[c as usize];
                if g < child.g {
                    child.g = g;
                    child.f = child.h + g;
                    child.parent = m;
                    stack.push(c);
                }
            }
        }
    }
}

/// `GridAStar_FindPath` (`0x00406E90`) with `StartSearch` and `RunSearch`: search from `start` to
/// `goal` in layer `view`. `None` on failure: different regions, start or goal
/// outside, start == goal or OPEN empty.
pub fn find_path(view: LayerView, start: (i32, i32), goal: (i32, i32)) -> Option<AStarPath> {
    if !view.inside(start.0, start.1) || !view.inside(goal.0, goal.1) {
        return None;
    }
    let width = view.width();
    let size = (width * view.height()) as usize;
    let mut s = Search {
        view,
        width,
        start_region: 0,
        nodes: Vec::new(),
        open: NIL,
        open_at: vec![NIL; size],
        closed_at: vec![NIL; size],
    };
    s.start_region = view.region_id(start.0, start.1);
    let goal_idx = s.idx(goal.0, goal.1);
    if view.region_id(goal.0, goal.1) != s.start_region || s.idx(start.0, start.1) == goal_idx {
        return None;
    }
    let h = ((start.0 - goal.0).abs() + (start.1 - goal.1).abs()) << 2;
    let root = s.new_node(Node {
        f: h,
        h,
        g: 0,
        x: start.0,
        z: start.1,
        idx: s.idx(start.0, start.1),
        parent: NIL,
        children: Vec::new(),
        next: NIL,
    });
    s.open = root;
    s.open_at[s.nodes[root as usize].idx] = root;
    let mut expansions = 0u32;
    loop {
        let n = s.pop_best()?;
        if s.nodes[n as usize].idx == goal_idx {
            // Path output of `PathPlanner_FindTilePath`: from the found node via the parents,
            // without the root.
            let mut cells = Vec::new();
            let mut k = n;
            while s.nodes[k as usize].parent != NIL {
                cells.push((s.nodes[k as usize].x, s.nodes[k as usize].z));
                k = s.nodes[k as usize].parent;
            }
            return Some(AStarPath { cells, expansions });
        }
        s.expand(n, goal);
        expansions += 1;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::path::BLOCKED;
    use crate::path::region::RegionLayer;

    /// 16 x 16, region 1, `walls` blocked.
    pub(crate) fn layer(walls: &[(i32, i32)]) -> RegionLayer {
        let mut l = RegionLayer::new(16, 16);
        l.cells.fill(1);
        for &(x, z) in walls {
            l.cells[(z * 16 + x) as usize] = BLOCKED;
        }
        l
    }

    fn view(l: &RegionLayer) -> LayerView<'_> {
        LayerView {
            layer: l,
            patch: None,
        }
    }

    /// Like `PathPlanner_FindTilePath`: search from the goal of the unit to its start.
    fn unit_path(walls: &[(i32, i32)], from: (i32, i32), to: (i32, i32)) -> (Vec<(i32, i32)>, u32) {
        let p = find_path(view(&layer(walls)), to, from).expect("path");
        (p.cells, p.expansions)
    }

    #[test]
    fn free_grids_follow_the_original_neighbour_order_and_lifo_ties() {
        // Test vectors T1 to T4, T6 from the model of the original code (re/tile-paths 6).
        assert_eq!(
            unit_path(&[], (0, 0), (5, 3)),
            (vec![(0, 0), (1, 0), (2, 0), (3, 1), (4, 2)], 5)
        );
        assert_eq!(
            unit_path(&[], (5, 3), (0, 0)),
            (vec![(5, 3), (4, 3), (3, 3), (2, 2), (1, 1)], 5)
        );
        assert_eq!(
            unit_path(&[], (0, 0), (5, 0)),
            (vec![(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)], 5)
        );
        assert_eq!(
            unit_path(&[], (0, 0), (3, 3)),
            (vec![(0, 0), (1, 1), (2, 2)], 3)
        );
        assert_eq!(unit_path(&[], (1, 1), (2, 2)), (vec![(1, 1)], 1));
    }

    #[test]
    fn walls_force_detours_and_block_corner_cutting() {
        // T5: the overestimating heuristic runs greedily along the wall (S1).
        let wall: Vec<_> = (0..6).map(|z| (3, z)).collect();
        assert_eq!(
            unit_path(&wall, (0, 2), (6, 2)),
            (
                vec![
                    (0, 2),
                    (0, 3),
                    (0, 4),
                    (1, 5),
                    (2, 6),
                    (3, 6),
                    (4, 6),
                    (4, 5),
                    (4, 4),
                    (5, 3)
                ],
                27
            )
        );
        // T7: no diagonal past a blocked orthogonal cell.
        assert_eq!(
            unit_path(&[(2, 1)], (1, 1), (2, 2)),
            (vec![(1, 1), (1, 2)], 2)
        );
    }

    #[test]
    fn other_regions_borders_and_equal_cells_fail() {
        let mut l = layer(&[]);
        l.cells[17] = 2;
        assert_eq!(find_path(view(&l), (0, 0), (1, 1)), None);
        assert_eq!(find_path(view(&l), (0, 0), (0, 0)), None);
        assert_eq!(find_path(view(&l), (0, 0), (16, 0)), None);
        // Overwritten cell: a goal in another area becomes reachable.
        let patched = LayerView {
            layer: &l,
            patch: Some(((1, 1), 1)),
        };
        assert!(find_path(patched, (0, 0), (1, 1)).is_some());
        // An enclosed cell: OPEN runs empty.
        let ring = [
            (4, 5),
            (6, 5),
            (5, 4),
            (5, 6),
            (4, 4),
            (6, 4),
            (4, 6),
            (6, 6),
        ];
        assert_eq!(find_path(view(&layer(&ring)), (0, 0), (5, 5)), None);
        // Region 0xFFFF: start and goal blocked, the search runs through blocked cells
        // (quirk; `PlanHighPath` relocates the start beforehand).
        let mut all = layer(&[]);
        all.cells.fill(BLOCKED);
        assert!(find_path(view(&all), (0, 0), (3, 0)).is_some());
    }
}
