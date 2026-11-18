//! Fog display of the viewer (game host, re/vision 7-10): the alpha table per tile
//! (`GameWorld+0x2040`) with crossfade on the rendering clock, the view switch
//! `m_FogShowMode` (not in the state, re/vision 6.4) and the rules that only the host needs. Queries
//! never change the simulation. "Fog" here always means the fog of war, not the
//! distance fog of the renderer (`--fog`, `FogSettings`).

use opennml_control::SimFogResult;
use opennml_sim::fog::FogView;
use opennml_sim::object::{GameObject, status};
use opennml_sim::state::SimState;
use opennml_world::terrain::FogFactors;

/// `m_FogShowMode` "Fog off" (`0x00476860`: 7).
pub const FOG_OFF: u8 = 7;
/// Target values of the alpha table (`0x00479160`): seen, explored.
const SEEN: u16 = 0xFF;
const EXPLORED: u16 = 0x80;

/// Entry of the alpha table (8 bytes in the original: current, target, timestamp).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AlphaCell {
    cur: u16,
    target: u16,
    stamp: u32,
}

/// Fog display of the active player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FogDisplay {
    width: i32,
    height: i32,
    cells: Vec<AlphaCell>,
    /// `m_FogShowMode`: 0 fog on, [`FOG_OFF`] off (debug, control channel).
    pub show_mode: u8,
    /// Debug overlay (key F, `sim_view`): state 1 explored, 3 seen, 7 cleared.
    pub overlay: Option<u8>,
}

impl FogDisplay {
    pub fn new(width: i32, height: i32) -> Self {
        let (w, h) = (width.max(0), height.max(0));
        Self {
            width: w,
            height: h,
            cells: vec![AlphaCell::default(); w as usize * h as usize],
            show_mode: 0,
            overlay: None,
        }
    }

    /// `FUN_00476860(-1)` (cheat `rainbowring`; the switch lives in the host, deviation from the original):
    /// `m_FogShowMode = ((old != 0) - 1) & 7`, so 0 -> [`FOG_OFF`] and any other value -> 0.
    pub fn toggle_show_mode(&mut self) {
        self.show_mode = if self.show_mode != 0 { 0 } else { FOG_OFF };
    }

    /// Queries with the view switch of the host.
    pub fn view<'a>(&self, state: &'a SimState) -> FogView<'a> {
        FogView::new(state).with_show_mode(self.show_mode)
    }

    /// `GameWorld_UpdateFogVertexAlpha(immediate)` (`0x00479160`), only with fog on: target 0xFF on
    /// tiles seen by the viewer (including shared vision), 0x80 on explored ones, otherwise 0;
    /// "cleared" does not count. `immediate` also sets the current value (level start, loading,
    /// player switch).
    pub fn update_targets(&mut self, state: &SimState, viewer: u8, immediate: bool) {
        if self.show_mode != 0 {
            return;
        }
        let m = FogView::new(state).mask(i32::from(viewer));
        let (seen, explored) = (&state.fog.seen.cells, &state.fog.explored.cells);
        for (i, c) in self.cells.iter_mut().enumerate() {
            let has = |g: &Vec<u8>| g.get(i).is_some_and(|b| b & m != 0);
            c.target = if has(seen) {
                SEEN
            } else if has(explored) {
                EXPLORED
            } else {
                0
            };
            if immediate {
                c.cur = c.target;
            }
        }
    }

    /// `0x00465F60`: current value at tile (x, z). Quirk: row/column 0 and everything from the map size on
    /// yield 0. Advances the crossfade in whole 10 ms steps up to `now_ms` (rendering clock,
    /// `u32` like `DAT_006736D0`).
    pub fn level(&mut self, x: i32, z: i32, now_ms: u32) -> u8 {
        if x < 1 || z < 1 || x >= self.width || z >= self.height {
            return 0;
        }
        let e = &mut self.cells[self.width as usize * z as usize + x as usize];
        if e.cur == e.target {
            e.stamp = now_ms;
        } else {
            let steps = now_ms.wrapping_sub(e.stamp) / 10;
            if steps > 0 {
                if e.cur < e.target {
                    let n = u32::from(e.cur).saturating_add(steps).min(255);
                    e.cur = if n > u32::from(e.target) {
                        e.target
                    } else {
                        n as u16
                    };
                } else {
                    let lowered = u32::from(e.cur) - u32::from(e.cur).min(steps);
                    e.cur = if u32::from(e.target) > lowered {
                        e.target
                    } else {
                        lowered as u16
                    };
                }
                e.stamp = e.stamp.wrapping_add(steps.wrapping_mul(10));
            }
        }
        e.cur as u8
    }

    /// Brightness factor (`FILD`, `FMUL 1/255`).
    pub fn factor(&mut self, x: i32, z: i32, now_ms: u32) -> f32 {
        f32::from(self.level(x, z, now_ms)) * (1.0 / 255.0)
    }

    /// Brightness factor of an object (`GameObject_RenderPass` `0x0046B906`...): 1.0 for objects
    /// of the viewer and with fog off, otherwise the factor at its tile (`+0x328/+0x32C`).
    pub fn object_brightness(&mut self, o: &GameObject, viewer: u8, now_ms: u32) -> f32 {
        self.brightness_at(o.owner(), o.tile, viewer, now_ms)
    }

    /// The same rule for an object of the slot `owner` at the tile `tile` (also for the
    /// fog memory, which draws the model of the object at its tile).
    pub fn brightness_at(&mut self, owner: u8, tile: (i32, i32), viewer: u8, now_ms: u32) -> f32 {
        if owner == viewer || self.show_mode != 0 {
            1.0
        } else {
            self.factor(tile.0, tile.1, now_ms)
        }
    }

    /// Factors of all grid points `(w + 1) x (h + 1)` for terrain and water; with fog off 255
    /// (`TerrainChunk_UpdateVertexColors` checks `m_FogShowMode == 0`).
    pub fn terrain_factors(&mut self, now_ms: u32, out: &mut FogFactors) {
        let (vx, vz) = (self.width as usize + 1, self.height as usize + 1);
        out.verts_x = vx;
        out.values.resize(vx * vz, 0);
        for z in 0..vz {
            for x in 0..vx {
                out.values[z * vx + x] = if self.show_mode != 0 {
                    255
                } else {
                    self.level(x as i32, z as i32, now_ms)
                };
            }
        }
    }
}

/// `GameObject_GetRenderBitsForPlayer(p, force = 1)` (`0x00466BE0`, re/vision 7) with
/// view switch (`QueryFogOfWarArea`):
/// - Tile seen: camouflaged non-buildings (modules 3, `0x29`, `0x32` excepted) get the special bit `0x80` for
///   foreign players, unless cleared with `WAITING4KILLING`; diving ones only for the owner including
///   shared vision; otherwise the render bits;
/// - not seen: active fog memory -> only the shadow from the memory (`(bits & 2) << 11`);
///   nature (module 5) already on explored tiles; otherwise 0.
pub fn render_bits_for(view: &FogView, o: &GameObject, p: i32, memory_active: bool) -> u32 {
    let f = view.area(o.tile.0, o.tile.1, p, 0);
    let owner = i32::from(o.owner());
    if f & 2 != 0 {
        let st = o.status;
        let fixed = [3, 0x29, 0x32].iter().any(|&m| o.has_module(m, false));
        if st & status::MASKED != 0 && !fixed {
            if f & 4 != 0 && st & status::WAITING4KILLING != 0 {
                return o.render_bits;
            }
            return if view.diplomacy.shares_vision(p, owner) {
                o.render_bits
            } else {
                0x80
            };
        }
        if st & status::DIVING != 0 && !view.diplomacy.shares_vision(p, owner) {
            return 0;
        }
        return o.render_bits;
    }
    if memory_active {
        return (o.render_bits & 2) << 11;
    }
    if f & 1 != 0 && o.has_module(5, false) {
        return o.render_bits;
    }
    0
}

/// `0x00467D70` for the viewer `p` (with view switch): buildings, trees and patterns (modules
/// 3, 4, `0x29`, `0x32`) only with a fog memory whose tile is not unexplored; all others,
/// including resources (module 6), with a seen tile (correction to re/vision 9).
pub fn visible_for_selection(
    view: &FogView,
    o: &GameObject,
    p: i32,
    memory_tile: Option<(i32, i32)>,
) -> bool {
    if [3, 4, 0x29, 0x32].iter().any(|&m| o.has_module(m, false)) {
        return memory_tile.is_some_and(|(x, z)| view.area(x, z, p, 0) != 0);
    }
    view.area(o.tile.0, o.tile.1, p, 0) & 2 != 0
}

/// Response of `sim_fog` (read-only).
pub fn fog_result(state: &SimState, player: u8, rect: Option<[i32; 4]>) -> SimFogResult {
    let view = FogView::new(state);
    let p = i32::from(player);
    let m = view.mask(p);
    let f = &state.fog;
    let rows = rect.map_or_else(Vec::new, |[x0, z0, x1, z1]| {
        (z0..=z1)
            .map(|z| {
                (x0..=x1)
                    .map(|x| match f.explored.get(x, z) {
                        Some(_) => i32::from(view.state_at(x, z, p)),
                        None => -2,
                    })
                    .collect()
            })
            .collect()
    });
    SimFogResult {
        player,
        width: f.explored.width,
        height: f.explored.height,
        explored: f.explored.count(m),
        seen: f.seen.count(m),
        cleared: f.cleared.count(m),
        processed: f.processed,
        cycle_player: f.player,
        per_tick: f.per_tick,
        sources: f.sources.len(),
        rect,
        rows,
    }
}

/// Next stage of the fog overlay (key `KeyF`): off -> explored -> seen -> cleared -> off.
pub fn next_overlay(current: Option<u8>) -> Option<u8> {
    match current {
        None => Some(1),
        Some(1) => Some(3),
        Some(3) => Some(7),
        _ => None,
    }
}

/// Tiles of the fog overlay around `center` (+-[`crate::game::grid::OVERLAY_RADIUS`]) whose
/// state for the viewer has at least the level `level` (1/3/7), with fill color.
pub fn overlay_cells(
    state: &SimState,
    viewer: u8,
    level: u8,
    center: (i32, i32),
) -> Vec<((i32, i32), [u8; 4])> {
    let view = FogView::new(state);
    let r = crate::game::grid::OVERLAY_RADIUS;
    let color = match level {
        7 => [255, 60, 60, 90],
        3 => [60, 200, 255, 70],
        _ => [220, 220, 220, 50],
    };
    let mut out = Vec::new();
    for z in center.1 - r..=center.1 + r {
        for x in center.0 - r..=center.0 + r {
            if view.state_at(x, z, i32::from(viewer)) & level == level {
                out.push(((x, z), color));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_sim::fog::Fog;
    use opennml_sim::id::ObjectId;
    use opennml_sim::module::Module;
    use opennml_sim::object::{GameObject, status};

    fn object(slot: u8, modules: &[usize]) -> GameObject {
        let mut o = GameObject::new(ObjectId::new(0, slot, 1), 0);
        o.tile = (1, 1);
        o.render_bits = 3;
        for &m in modules {
            o.modules[m] = Some(Module::new(m as u8));
        }
        o
    }

    fn state(w: i32, h: i32) -> SimState {
        let mut s = SimState::new(0, 0);
        s.fog = Fog::new(w, h);
        s
    }

    #[test]
    fn targets_follow_seen_and_explored_with_shared_vision_but_not_cleared() {
        let mut s = state(4, 4);
        s.fog.explored.or(1, 1, 1);
        s.fog.seen.or(2, 1, 2);
        s.fog.cleared.or(3, 1, 1);
        let mut d = FogDisplay::new(4, 4);
        d.update_targets(&s, 0, true);
        assert_eq!(
            [d.level(1, 1, 0), d.level(2, 1, 0), d.level(3, 1, 0)],
            [0x80, 0, 0]
        );
        s.diplomacy.set_vision(0, 1, true);
        d.update_targets(&s, 0, true);
        assert_eq!(d.level(2, 1, 0), 0xFF);
    }

    #[test]
    fn levels_fade_10_ms_per_step_and_the_border_stays_black() {
        let mut s = state(4, 4);
        s.fog.explored.or(1, 1, 1);
        let mut d = FogDisplay::new(4, 4);
        d.update_targets(&s, 0, true);
        assert_eq!(d.level(1, 1, 5000), 0x80);
        s.fog.seen.or(1, 1, 1);
        d.update_targets(&s, 0, false);
        assert_eq!(d.level(1, 1, 5009), 0x80);
        assert_eq!(d.level(1, 1, 5010), 0x81);
        assert_eq!(d.level(1, 1, 6270), 0xFF, "0x80 -> 0xFF in 127 steps");
        assert_eq!(d.factor(1, 1, 7000), 1.0);
        s.fog.seen.clear();
        s.fog.explored.clear();
        d.update_targets(&s, 0, false);
        assert_eq!(d.level(1, 1, 7100), 0xFF - 10);
        assert_eq!(d.level(1, 1, 20_000), 0);
        // Quirk: row/column 0 and everything from the map size on: black.
        s.fog.seen.fill(1);
        d.update_targets(&s, 0, true);
        assert_eq!(
            [
                d.level(0, 2, 0),
                d.level(2, 0, 0),
                d.level(4, 2, 0),
                d.level(2, 2, 0)
            ],
            [0, 0, 0, 0xFF]
        );
        // Clock overflow: u32 like the original.
        assert_eq!(d.level(2, 2, u32::MAX), 0xFF);
    }

    #[test]
    fn switching_the_player_sets_the_brightness_at_once() {
        let mut s = state(4, 4);
        s.fog.seen.or(2, 2, 1 << 1);
        let mut d = FogDisplay::new(4, 4);
        d.update_targets(&s, 0, true);
        assert_eq!(d.level(2, 2, 100), 0);
        d.update_targets(&s, 1, true);
        assert_eq!(d.level(2, 2, 100), 0xFF, "player switch without crossfade");
    }

    #[test]
    fn fog_off_lights_the_terrain_and_keeps_the_table() {
        let mut s = state(2, 2);
        s.fog.seen.or(1, 1, 1);
        let mut d = FogDisplay::new(2, 2);
        d.update_targets(&s, 0, true);
        let mut f = FogFactors::default();
        d.terrain_factors(0, &mut f);
        assert_eq!(
            (f.verts_x, f.values.clone()),
            (3, vec![0, 0, 0, 0, 0xFF, 0, 0, 0, 0])
        );
        d.show_mode = FOG_OFF;
        s.fog.seen.clear();
        d.update_targets(&s, 0, true);
        d.terrain_factors(0, &mut f);
        assert_eq!(f.values, vec![0xFF; 9], "fog off: factor 1");
        d.show_mode = 0;
        assert_eq!(d.level(1, 1, 0), 0xFF, "the table was not updated");
        assert_eq!(d.view(&s).area(0, 0, 0, 0), 0);
    }

    #[test]
    fn render_bits_follow_get_render_bits_for_player() {
        let mut s = state(4, 4);
        let unit = object(1, &[0]);
        let natur = object(10, &[5]);
        assert_eq!(render_bits_for(&FogView::new(&s), &unit, 0, false), 0);
        s.fog.explored.or(1, 1, 1);
        assert_eq!(
            render_bits_for(&FogView::new(&s), &unit, 0, false),
            0,
            "unit: seen only"
        );
        assert_eq!(
            render_bits_for(&FogView::new(&s), &natur, 0, false),
            3,
            "nature: already explored"
        );
        assert_eq!(
            render_bits_for(&FogView::new(&s), &unit, 0, true),
            0x1000,
            "fog memory active"
        );
        s.fog.seen.or(1, 1, 1);
        assert_eq!(render_bits_for(&FogView::new(&s), &unit, 0, false), 3);
        let mut diving = unit.clone();
        diving.status |= status::DIVING;
        assert_eq!(render_bits_for(&FogView::new(&s), &diving, 0, false), 0);
        assert_eq!(
            render_bits_for(&FogView::new(&s), &diving, 1, false),
            0,
            "the owner does not see the tile"
        );
        s.fog.seen.or(1, 1, 2);
        assert_eq!(render_bits_for(&FogView::new(&s), &diving, 1, false), 3);
        let mut masked = unit.clone();
        masked.status |= status::MASKED;
        assert_eq!(render_bits_for(&FogView::new(&s), &masked, 0, false), 0x80);
        masked.status |= status::WAITING4KILLING;
        s.fog.cleared.or(1, 1, 1);
        assert_eq!(render_bits_for(&FogView::new(&s), &masked, 0, false), 3);
        let mut masked_building = object(1, &[3]);
        masked_building.status |= status::MASKED;
        assert_eq!(
            render_bits_for(&FogView::new(&s), &masked_building, 0, false),
            3
        );
        assert_eq!(
            render_bits_for(
                &FogView::new(&s).with_show_mode(FOG_OFF),
                &object(1, &[0]),
                5,
                false
            ),
            3
        );
    }

    #[test]
    fn own_objects_and_fog_off_have_full_brightness() {
        let mut s = state(4, 4);
        s.fog.explored.or(1, 1, 1);
        let mut d = FogDisplay::new(4, 4);
        d.update_targets(&s, 0, true);
        let own = object(0, &[0]);
        let other = object(1, &[0]);
        assert_eq!(d.object_brightness(&own, 0, 0), 1.0);
        assert_eq!(d.object_brightness(&other, 0, 0), 128.0 * (1.0 / 255.0));
        d.show_mode = FOG_OFF;
        assert_eq!(d.object_brightness(&other, 0, 0), 1.0);
    }

    #[test]
    fn units_need_sight_and_buildings_trees_need_a_memory() {
        let mut s = state(4, 4);
        let unit = object(1, &[0]);
        let resource = object(10, &[6]);
        let building = object(1, &[3]);
        s.fog.explored.or(1, 1, 1);
        assert!(!visible_for_selection(&FogView::new(&s), &unit, 0, None));
        assert!(
            !visible_for_selection(&FogView::new(&s), &resource, 0, None),
            "resources: seen required"
        );
        assert!(
            !visible_for_selection(&FogView::new(&s), &building, 0, None),
            "never without a fog memory"
        );
        assert!(visible_for_selection(
            &FogView::new(&s),
            &building,
            0,
            Some((1, 1))
        ));
        assert!(
            !visible_for_selection(&FogView::new(&s), &building, 0, Some((3, 3))),
            "memory unexplored"
        );
        s.fog.seen.or(1, 1, 1);
        assert!(visible_for_selection(&FogView::new(&s), &unit, 0, None));
        assert!(visible_for_selection(&FogView::new(&s), &resource, 0, None));
        let off = FogView::new(&s).with_show_mode(FOG_OFF);
        assert!(
            visible_for_selection(&off, &object(2, &[0]), 5, None),
            "fog off"
        );
    }

    #[test]
    fn sim_fog_reports_the_grids_of_a_player() {
        let mut s = state(4, 4);
        s.fog.explored.fill(1);
        s.fog.seen.or(1, 1, 1);
        s.fog.seen.or(2, 2, 2);
        s.fog.cleared.or(1, 1, 1);
        s.fog.processed = -1;
        let r = fog_result(&s, 0, Some([-1, 1, 2, 1]));
        assert_eq!(
            (r.width, r.height, r.explored, r.seen, r.cleared),
            (4, 4, 16, 1, 1)
        );
        assert_eq!((r.processed, r.sources), (-1, 0));
        assert_eq!(r.rows, vec![vec![-2, 1, 7, 1]]);
        s.diplomacy.set_vision(0, 1, true);
        assert_eq!(fog_result(&s, 0, None).seen, 2, "shared vision counts");
        let before = opennml_sim::save::checksum(&s);
        let _ = overlay_cells(&s, 0, 3, (1, 1));
        assert_eq!(opennml_sim::save::checksum(&s), before);
        assert_eq!(overlay_cells(&s, 0, 7, (1, 1)).len(), 1);
        assert_eq!(
            [
                next_overlay(None),
                next_overlay(Some(1)),
                next_overlay(Some(3)),
                next_overlay(Some(7))
            ],
            [Some(1), Some(3), Some(7), None]
        );
    }
}
