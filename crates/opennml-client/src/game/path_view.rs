//! Overlay of the pathfinding: a layer of the path grid (blocked, regions) or the occupancy grid around the camera and
//! the path preview of a selected unit. The logic here is testable without a window; drawing happens in the viewer
//! (`apps/opennml-viewer/src/debug_ui.rs`).

use glam::{Mat4, Vec3};
use opennml_control::{GRID_OCCUPANCY, SimGridResult, SimPathResult};
use opennml_sim::fx::Fx;
use opennml_sim::id::ObjectId;
use opennml_sim::module::ModuleKind;
use opennml_sim::path::highpath::{HighPath, PlanFailure, plan_high_path};
use opennml_sim::path::{BLOCKED, LAYER_COUNT, layer_name};
use opennml_sim::state::SimState;
use opennml_world::terrain::Terrain;

use crate::game::grid::{OVERLAY_RADIUS, project, world_to_grid};
use crate::game::sim_host::SimHost;

/// Display mode: layers 0-8 or the occupancy grid (9).
pub const OCCUPANCY_MODE: usize = LAYER_COUNT;

/// Key G (forward) or Shift+G (backward): off -> layer 0 ... 8 -> occupancy -> off.
pub fn next_mode(current: Option<usize>, back: bool) -> Option<usize> {
    let order: Vec<Option<usize>> = std::iter::once(None)
        .chain((0..=OCCUPANCY_MODE).map(Some))
        .collect();
    let i = order.iter().position(|&m| m == current).unwrap_or(0);
    let n = order.len();
    order[if back { (i + n - 1) % n } else { (i + 1) % n }]
}

/// Display name of a display mode.
pub fn mode_name(mode: usize) -> String {
    if mode == OCCUPANCY_MODE {
        "Occupancy".into()
    } else {
        format!("Layer {mode} ({})", layer_name(mode))
    }
}

/// State of a tile in the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellMark {
    Blocked,
    Region(u16),
    Occupied,
}

/// Tiles of the section around `center` (in the map), z outside, x inside; free tiles of the
/// occupancy grid are missing.
pub fn overlay_cells(
    state: &SimState,
    mode: usize,
    center: (i32, i32),
) -> Vec<((i32, i32), CellMark)> {
    let grid = &state.path;
    let mut out = Vec::new();
    for z in center.1 - OVERLAY_RADIUS..=center.1 + OVERLAY_RADIUS {
        for x in center.0 - OVERLAY_RADIUS..=center.0 + OVERLAY_RADIUS {
            if !grid.inside(x, z) {
                continue;
            }
            let mark = if mode == OCCUPANCY_MODE {
                if !state.occupancy.test(x, z) {
                    continue;
                }
                CellMark::Occupied
            } else {
                match grid.region_id(mode, x, z) {
                    BLOCKED => CellMark::Blocked,
                    id => CellMark::Region(id),
                }
            };
            out.push(((x, z), mark));
        }
    }
    out
}

/// Fill color (RGBA) of a tile: blocked red, occupied orange, regions in one color per ID.
pub fn cell_color(mark: CellMark) -> [u8; 4] {
    match mark {
        CellMark::Blocked => [220, 30, 30, 110],
        CellMark::Occupied => [255, 140, 0, 110],
        CellMark::Region(id) => {
            // Hue spread from the ID (golden ratio) so that neighboring regions stand out.
            let h = (u32::from(id).wrapping_mul(0x9E37_79B9) >> 24) as f32 / 256.0;
            let rgb = hue_rgb(h);
            [rgb[0], rgb[1], rgb[2], 45]
        }
    }
}

fn hue_rgb(h: f32) -> [u8; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0) % 6.0;
        let v = 1.0 - (k.min(4.0 - k).clamp(0.0, 1.0));
        (v * 255.0) as u8
    };
    [f(5.0), f(3.0), f(1.0)]
}

/// Result of a path planning for the display.
#[derive(Debug, Clone, PartialEq)]
pub struct PathView {
    pub unit: ObjectId,
    pub layer: usize,
    /// Unit and requested goal in fixed point.
    pub from: (i32, i32),
    pub goal: (i32, i32),
    pub allow_override: bool,
    pub result: Result<HighPath, PlanFailure>,
}

impl PathView {
    /// Line for the overlay and the debug window.
    pub fn summary(&self) -> String {
        let fx = |v: i32| Fx(v).to_f32();
        match &self.result {
            Ok(p) => format!(
                "Path {} ({}): {} tiles, {} waypoints, {} expansions, goal ({:.2}, {:.2})",
                self.layer,
                layer_name(self.layer),
                p.tile_path.len(),
                p.points.len(),
                p.expansions,
                fx(p.goal.0),
                fx(p.goal.1)
            ),
            Err(e) => format!("Path {} ({}): {e:?}", self.layer, layer_name(self.layer)),
        }
    }

    /// World points of the line unit -> waypoints -> goal (height from `height`); empty on failure.
    pub fn polyline(&self, cell: f32, height: impl Fn(f32, f32) -> f32) -> Vec<Vec3> {
        let Ok(p) = &self.result else {
            return Vec::new();
        };
        let to_world = |(x, z): (i32, i32)| {
            let (wx, wz) = (Fx(x).to_f32() * cell, Fx(z).to_f32() * cell);
            Vec3::new(wx, height(wx, wz) + 4.0, wz)
        };
        std::iter::once(self.from)
            .chain(p.points.iter().copied())
            .chain(std::iter::once(p.goal))
            .map(to_world)
            .collect()
    }
}

/// `sim_path`: result for the control channel (world values = fixed point / 4096 * cell).
pub fn path_result(view: &PathView, cell: [i32; 3]) -> SimPathResult {
    let world = |(x, z): (i32, i32)| {
        [
            Fx(x).to_f32() * cell[0] as f32,
            Fx(z).to_f32() * cell[2] as f32,
        ]
    };
    let pairs = |v: &[(i32, i32)]| v.iter().map(|&(x, z)| [x, z]).collect::<Vec<_>>();
    let mut r = SimPathResult {
        unit: (!view.unit.is_none()).then_some(view.unit.0),
        layer: view.layer as u8,
        layer_name: layer_name(view.layer).into(),
        from_fx: [view.from.0, view.from.1],
        to_fx: [view.goal.0, view.goal.1],
        failure: None,
        goal_fx: None,
        goal: None,
        start_tile: None,
        tile_path: Vec::new(),
        smoothed: Vec::new(),
        waypoints_fx: Vec::new(),
        waypoints: Vec::new(),
        expansions: 0,
        message: Vec::new(),
    };
    match &view.result {
        Err(e) => r.failure = Some(format!("{e:?}")),
        Ok(p) => {
            r.goal_fx = Some([p.goal.0, p.goal.1]);
            r.goal = Some(world(p.goal));
            r.start_tile = Some([p.start_tile.0, p.start_tile.1]);
            r.tile_path = pairs(&p.tile_path);
            r.smoothed = pairs(&p.smoothed);
            r.waypoints_fx = pairs(&p.points);
            r.waypoints = p.points.iter().map(|&q| world(q)).collect();
            r.expansions = p.expansions;
            r.message = p.message().args;
        }
    }
    r
}

/// `sim_grid`: figures of a layer or the occupancy grid and the cells of `rect`.
pub fn grid_result(state: &SimState, layer: u8, rect: Option<[i32; 4]>) -> SimGridResult {
    let grid = &state.path;
    let mode = usize::from(layer);
    let occupancy = layer == GRID_OCCUPANCY;
    let cell = |x: i32, z: i32| -> i32 {
        if !grid.inside(x, z) {
            -2
        } else if occupancy {
            i32::from(state.occupancy.test(x, z))
        } else {
            match grid.region_id(mode, x, z) {
                BLOCKED => -1,
                id => i32::from(id),
            }
        }
    };
    let rows = rect.map_or_else(Vec::new, |[x0, z0, x1, z1]| {
        (z0..=z1)
            .map(|z| (x0..=x1).map(|x| cell(x, z)).collect())
            .collect()
    });
    SimGridResult {
        layer,
        name: mode_name(mode),
        width: grid.width,
        height: grid.height,
        blocked: if occupancy {
            state.occupancy.count()
        } else {
            grid.blocked_count(mode)
        },
        regions: if occupancy {
            0
        } else {
            grid.region_count(mode)
        },
        counter: if occupancy {
            0
        } else {
            grid.layers[mode].counter
        },
        built: grid.built,
        rect,
        rows,
    }
}

/// Remaining route of a walking unit in pixels ([`walk_route_points`] projects).
pub fn walk_route(
    sim: &SimHost,
    unit: ObjectId,
    view_proj: Mat4,
    size: (u32, u32),
) -> Vec<[f32; 2]> {
    walk_route_points(sim, unit)
        .into_iter()
        .filter_map(|v| project(view_proj, v, size))
        .collect()
}

/// Remaining route of a walking unit in world points: displayed position, then the waypoints of the
/// HighPath list (empty if it is not walking). The line follows the unit's height rule
/// ([`crate::game::sim_host::clamps_to_water`]): for swimmers the waypoints lie at least at water height
/// instead of on the lake bottom, divers stay on the bottom. Display only.
pub fn walk_route_points(sim: &SimHost, unit: ObjectId) -> Vec<Vec3> {
    let Some(o) = sim.state.object(unit, false) else {
        return Vec::new();
    };
    let Some(opennml_sim::module::ModuleKind::Walk(w)) = o.module(0).map(|m| &m.kind) else {
        return Vec::new();
    };
    if o.status & 1 == 0 || w.high_path.is_empty() {
        return Vec::new();
    }
    let cell = sim.rules.map.cell;
    let on_water = crate::game::sim_host::clamps_to_water(o);
    let at = |x: i32, z: i32| {
        let wx = Fx(x).to_f32() * cell[0] as f32;
        let wz = Fx(z).to_f32() * cell[2] as f32;
        let ground = sim.rules.ground_height(wx, wz);
        let y = if on_water {
            ground.max(sim.rules.map.water_level)
        } else {
            ground
        };
        Vec3::new(wx, y, wz)
    };
    std::iter::once(sim.display(o).0)
        .chain(w.high_path.iter().map(|p| at(p.x, p.z)))
        .collect()
}

/// Plans the path of a unit to the fixed-point goal like a FINDTO (`allow_override` = FINDTO
/// `0x100`); `None` if the unit does not exist or has no walk module. Without `layer` the current
/// layer of the walk module is used (it changes e.g. when diving via `0xF00` to 8,
/// re/swim-dive 3.2); `layer` replaces it.
pub fn plan(
    sim: &SimHost,
    unit: ObjectId,
    goal: (i32, i32),
    layer: Option<usize>,
    allow_override: bool,
) -> Option<PathView> {
    let o = sim.state.object(unit, false)?;
    let layer = match layer {
        Some(l) => l,
        None => match &o.module(0)?.kind {
            ModuleKind::Walk(w) => w.layer,
            _ => return None,
        },
    };
    let from = (o.grid.x.0, o.grid.z.0);
    Some(PathView {
        unit,
        layer,
        from,
        goal,
        allow_override,
        result: plan_high_path(&sim.state.path, layer, from, goal, allow_override, 0x201),
    })
}

/// Tile under the camera center (center of the tile overlays).
pub fn camera_tile(sim: &SimHost, camera_target: Vec3) -> (i32, i32) {
    let tile = sim.rules.map.cell[0] as f32;
    (
        (camera_target.x / tile) as i32,
        (camera_target.z / tile) as i32,
    )
}

/// Tiles (x, z) with fill color as quads in pixels: corners from the grid points of the
/// terrain, projected; tiles with a corner behind the camera are dropped.
pub fn tile_quads(
    terrain: &Terrain,
    view_proj: Mat4,
    size: (u32, u32),
    cells: impl IntoIterator<Item = ((i32, i32), [u8; 4])>,
) -> Vec<([[f32; 2]; 4], [u8; 4])> {
    let corner = |x: i32, z: i32| {
        let v = terrain.vertex(x as usize, z as usize).position;
        project(view_proj, v, size)
    };
    cells
        .into_iter()
        .filter_map(|((x, z), color)| {
            Some((
                [
                    corner(x, z)?,
                    corner(x + 1, z)?,
                    corner(x + 1, z + 1)?,
                    corner(x, z + 1)?,
                ],
                color,
            ))
        })
        .collect()
}

/// What the overlay draws per frame: tile quads with color, outlines of the tile path, the
/// line unit -> waypoints -> goal (all in pixels) and the lines of the debug window.
pub type OverlayParts = (
    Vec<([[f32; 2]; 4], [u8; 4])>,
    Vec<[[f32; 2]; 4]>,
    Vec<[f32; 2]>,
    Vec<String>,
);

/// Builds the overlay of a frame. The path preview applies to exactly one selected own unit
/// with a walk module to the terrain point `ground` under the mouse pointer; it is only replanned when
/// the unit, goal or tick changes (`cache`).
#[allow(clippy::too_many_arguments)]
pub fn path_overlay(
    sim: &SimHost,
    terrain: &Terrain,
    camera_target: Vec3,
    view_proj: Mat4,
    size: (u32, u32),
    mode: Option<usize>,
    selection: &[ObjectId],
    ground: Option<Vec3>,
    cache: &mut Option<(u64, PathView)>,
    shown: Option<&PathView>,
) -> OverlayParts {
    let Some(mode) = mode else {
        *cache = None;
        let Some(view) = shown else {
            return Default::default();
        };
        let mut parts = OverlayParts::default();
        draw_path(view, terrain, view_proj, size, &mut parts);
        return parts;
    };
    let cell = sim.rules.map.cell;
    let tile = cell[0] as f32;
    let center = camera_tile(sim, camera_target);
    let cells = tile_quads(
        terrain,
        view_proj,
        size,
        overlay_cells(&sim.state, mode, center)
            .into_iter()
            .map(|(c, mark)| (c, cell_color(mark))),
    );
    let grid = &sim.state.path;
    let mut rows = vec![if mode == OCCUPANCY_MODE {
        format!(
            "{}: {} tiles occupied (G/Shift+G switches)",
            mode_name(mode),
            sim.state.occupancy.count()
        )
    } else {
        format!(
            "{}: {} blocked, {} regions, counter {} (G/Shift+G switches)",
            mode_name(mode),
            grid.blocked_count(mode),
            grid.region_count(mode),
            grid.layers[mode].counter
        )
    }];
    if let Some(p) = ground {
        let (x, z) = ((p.x / tile) as i32, (p.z / tile) as i32);
        let region = (0..LAYER_COUNT)
            .map(|l| match grid.region_id(l, x, z) {
                BLOCKED => "#".to_string(),
                id => id.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(format!(
            "Tile ({x}, {z}): regions L0 to L8 {region}, occupied {}",
            if sim.state.occupancy.test(x, z) {
                "yes"
            } else {
                "no"
            }
        ));
    }
    let own_unit = match selection {
        [id] => sim
            .state
            .object(*id, false)
            .filter(|o| o.owner() == sim.active_player)
            .map(|o| o.id),
        _ => None,
    };
    let preview = match (own_unit, ground) {
        (Some(unit), Some(p)) => {
            let goal = world_to_grid(p, cell);
            let fresh = cache.as_ref().is_some_and(|(steps, v)| {
                *steps == sim.state.steps && v.unit == unit && v.goal == goal
            });
            if !fresh {
                *cache = plan(sim, unit, goal, None, false).map(|v| (sim.state.steps, v));
            }
            cache.as_ref().map(|(_, v)| v)
        }
        _ => {
            *cache = None;
            None
        }
    };
    let mut parts = (cells, Vec::new(), Vec::new(), rows);
    if let Some(v) = preview.or(shown) {
        parts.3.push(v.summary());
        draw_path(v, terrain, view_proj, size, &mut parts);
    }
    parts
}

/// Outlines of the tile path and the line unit -> waypoints -> goal in pixels.
fn draw_path(
    view: &PathView,
    terrain: &Terrain,
    view_proj: Mat4,
    size: (u32, u32),
    parts: &mut OverlayParts,
) {
    let corner = |x: i32, z: i32| {
        let v = terrain.vertex(x as usize, z as usize).position;
        project(view_proj, v, size)
    };
    if let Ok(p) = &view.result {
        parts.1 = p
            .tile_path
            .iter()
            .filter(|&&(x, z)| {
                (0..terrain.dim_x() as i32).contains(&x) && (0..terrain.dim_z() as i32).contains(&z)
            })
            .filter_map(|&(x, z)| {
                Some([
                    corner(x, z)?,
                    corner(x + 1, z)?,
                    corner(x + 1, z + 1)?,
                    corner(x, z + 1)?,
                ])
            })
            .collect();
    }
    parts.2 = view
        .polyline(terrain.params.size_x, |x, z| terrain.height_at(x, z))
        .into_iter()
        .filter_map(|w| project(view_proj, w, size))
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_cycles_through_layers_and_occupancy() {
        assert_eq!(next_mode(None, false), Some(0));
        assert_eq!(next_mode(Some(8), false), Some(OCCUPANCY_MODE));
        assert_eq!(next_mode(Some(OCCUPANCY_MODE), false), None);
        assert_eq!(next_mode(None, true), Some(OCCUPANCY_MODE));
        assert_eq!(next_mode(Some(0), true), None);
        assert_eq!(mode_name(3), "Layer 3 (Land+Forest+Water 1x1)");
        assert_eq!(mode_name(OCCUPANCY_MODE), "Occupancy");
        assert_ne!(
            cell_color(CellMark::Region(4)),
            cell_color(CellMark::Region(5))
        );
        assert_eq!(cell_color(CellMark::Blocked)[0], 220);
    }

    #[test]
    fn cells_are_cut_to_the_map_and_free_occupancy_is_left_out() {
        let rules = opennml_sim::rules::Rules::synthetic("", 16);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        state.path.build_regions(u32::MAX);
        state.occupancy.set(3, 3, true);
        let cells = overlay_cells(&state, 0, (2, 2));
        assert_eq!(cells.len(), 16 * 16);
        assert!(cells.iter().all(|(_, m)| matches!(m, CellMark::Region(_))));
        // Layer 4 (water) is blocked over water.
        assert!(
            overlay_cells(&state, 4, (2, 2))
                .iter()
                .all(|(_, m)| *m == CellMark::Blocked)
        );
        assert_eq!(
            overlay_cells(&state, OCCUPANCY_MODE, (2, 2)),
            vec![((3, 3), CellMark::Occupied)]
        );
        assert_eq!(
            world_to_grid(Vec3::new(1000.0, 0.0, 64.0), [128; 3]),
            (32000, 2048)
        );
        let g = grid_result(&state, 0, Some([14, 0, 16, 0]));
        assert_eq!((g.width, g.blocked, g.regions, g.built), (16, 0, 1, true));
        assert_eq!(g.rows, vec![vec![1, 1, -2]]);
        let o = grid_result(&state, GRID_OCCUPANCY, Some([3, 3, 3, 3]));
        assert_eq!(
            (o.name.as_str(), o.blocked, o.rows.clone()),
            ("Occupancy", 1, vec![vec![1]])
        );
        assert_eq!(grid_result(&state, 4, None).blocked, 256);
    }

    #[test]
    fn a_unit_plans_in_the_current_layer_of_its_walk_module() {
        // A submerged diver moves in layer 8 (re/swim-dive 3.2, `0xF00`); the path preview and `sim_path` use the
        // layer of the walk module, not the start layer of the type.
        use crate::game::sim_host::testkit::{DIVER, host, spawn};
        use opennml_sim::path::unit_layer;
        let mut h = host();
        let d = spawn(&mut h, DIVER, 640.0, 640.0);
        let start = plan(&h, d, (0x8800, 0x8800), None, false).unwrap().layer;
        assert_eq!(start, unit_layer(&h.rules, DIVER).unwrap());
        let o = h.state.object_mut(d, false).unwrap();
        let Some(ModuleKind::Walk(w)) = o.module_mut(0).map(|m| &mut m.kind) else {
            panic!("diver without walk module");
        };
        w.layer = 8;
        assert_eq!(plan(&h, d, (0x8800, 0x8800), None, false).unwrap().layer, 8);
        // An explicit layer still takes precedence.
        assert_eq!(
            plan(&h, d, (0x8800, 0x8800), Some(0), false).unwrap().layer,
            0
        );
    }

    #[test]
    fn path_results_carry_tiles_points_and_the_message() {
        use opennml_sim::path::highpath::plan_high_path;
        let rules = opennml_sim::rules::Rules::synthetic("", 16);
        let mut state = SimState::new(0, 0);
        state.init_map(&rules);
        for z in 0..5 {
            state.path.mark_cell(1, 1, z, true);
        }
        state.path.flush_dirty_rect();
        state.path.build_regions(u32::MAX);
        let (from, goal) = ((0x800, 0x800), (0x2800, 0x800));
        let view = PathView {
            unit: ObjectId::NONE,
            layer: 0,
            from,
            goal,
            allow_override: false,
            result: plan_high_path(&state.path, 0, from, goal, false, 0x201),
        };
        let r = path_result(&view, [128; 3]);
        assert_eq!((r.unit, r.failure.clone()), (None, None));
        assert_eq!(r.waypoints_fx, vec![[0xC00, 0x5C00], [0x2C00, 0x5C00]]);
        assert_eq!(r.waypoints[0], [96.0, 736.0]);
        assert_eq!(r.goal, Some([320.0, 64.0]));
        assert_eq!(r.message.len(), 7 + 4);
        assert_eq!(r.message[3], 0x201);
        assert!(view.summary().contains("2 waypoints"));
        let blocked = PathView {
            result: Err(PlanFailure::StartBlocked),
            ..view
        };
        assert_eq!(
            path_result(&blocked, [128; 3]).failure.as_deref(),
            Some("StartBlocked")
        );
    }

    #[test]
    fn the_route_of_a_swimmer_ends_on_the_water_surface_but_a_diver_stays_on_the_ground() {
        // The remaining-route line led to the lake bottom under the click point.
        use crate::game::sim_host::testkit::{WARRIOR, host, spawn};
        use opennml_sim::module::walk::HighPoint;
        use opennml_sim::object::status;
        let mut h = host();
        h.rules.map.water_level = 5.0; // flat terrain at 0
        let u = spawn(&mut h, WARRIOR, 640.0, 640.0);
        let o = h.state.object_mut(u, false).unwrap();
        o.status |= status::MOVE;
        let Some(ModuleKind::Walk(w)) = o.module_mut(0).map(|m| &mut m.kind) else {
            panic!("warrior without walk module");
        };
        w.high_path.push_back(HighPoint {
            id: 0,
            x: 0x2800,
            z: 0x2800,
        });
        let end = |h: &SimHost| *walk_route_points(h, u).last().unwrap();
        // Land unit: line on the terrain.
        assert_eq!(end(&h), Vec3::new(320.0, 0.0, 320.0));
        // Swimmer: water surface at the same point.
        h.state.object_mut(u, false).unwrap().status |= status::SWIMMING;
        assert_eq!(end(&h), Vec3::new(320.0, 5.0, 320.0));
        // The start is the displayed position and lies at the same height.
        assert_eq!(walk_route_points(&h, u)[0].y, 5.0);
        // Diver: on the bottom, like the unit's rendering.
        h.state.object_mut(u, false).unwrap().status |= status::DIVING;
        assert_eq!(end(&h).y, 0.0);
        // Terrain above the water stays untouched (swimmer at the shore).
        let o = h.state.object_mut(u, false).unwrap();
        o.status &= !status::DIVING;
        h.rules.map.water_level = -100.0;
        assert_eq!(end(&h).y, 0.0);
    }
}
