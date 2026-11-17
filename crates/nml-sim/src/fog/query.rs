//! Queries of the fog (re/vision 5) for a player `p` including shared vision. The simulation knows no local
//! player (A6): `p ∉ 0…7` sees nothing; host callers pass their viewer and their view switch (`m_FogShowMode`).

use crate::fx::ftol;
use crate::id::PLAYER_SLOTS;
use crate::rules::MapInfo;
use crate::state::{Diplomacy, SimState};

use super::{ByteGrid, Fog};

/// Reading view of fog and diplomacy.
#[derive(Debug, Clone, Copy)]
pub struct FogView<'a> {
    pub fog: &'a Fog,
    pub diplomacy: &'a Diplomacy,
    /// `m_FogShowMode` (`GameWorld+0x498`): 0 = fog on; ORed onto area queries. Only the host sets it; the
    /// simulation always queries with 0.
    pub show_mode: u8,
}

impl<'a> FogView<'a> {
    pub fn new(state: &'a SimState) -> Self {
        Self {
            fog: &state.fog,
            diplomacy: &state.diplomacy,
            show_mode: 0,
        }
    }

    pub fn with_show_mode(self, show_mode: u8) -> Self {
        Self { show_mode, ..self }
    }

    /// Vision mask of `p`: all q with `HasSharedVision(p, q)` (one-sided: p sees what q sees).
    pub fn mask(&self, p: i32) -> u8 {
        (0..PLAYER_SLOTS as i32)
            .filter(|&q| self.diplomacy.shares_vision(p, q))
            .fold(0, |m, q| m | 1 << q)
    }

    /// `GameWorld_GetFogOfWarStateAt(x, z, p)` (`0x0047B140`): 7 cleared, 3 seen, 1 explored,
    /// otherwise 0; outside 0. Without the view switch.
    pub fn state_at(&self, x: i32, z: i32, p: i32) -> u8 {
        let m = self.mask(p);
        if x < 0 {
            return 0;
        }
        let hit = |g: &ByteGrid| g.get(x, z).is_some_and(|b| b & m != 0);
        if hit(&self.fog.cleared) {
            7
        } else if hit(&self.fog.seen) {
            3
        } else if hit(&self.fog.explored) {
            1
        } else {
            0
        }
    }

    /// `GameWorld_QueryFogOfWarArea(x, z, p, r)` (`0x0047B3B0`): [`Self::state_at`] ORed over the square
    /// `[x − r, x + r] × [z − r, z + r]`, plus the view switch.
    pub fn area(&self, x: i32, z: i32, p: i32, r: i32) -> u8 {
        let mut f = 0;
        for zz in z.saturating_sub(r)..=z.saturating_add(r) {
            for xx in x.saturating_sub(r)..=x.saturating_add(r) {
                f |= self.state_at(xx, zz, p);
            }
        }
        f | self.show_mode
    }

    /// `GameWorld_GetFogOfWarStateInRect(rect, p)` (`0x0047B250`): "any bit in the rectangle" per grid in the
    /// ranking 7/3/1/0, without the view switch.
    pub fn rect(&self, rect: [i32; 4], p: i32) -> u8 {
        let m = self.mask(p);
        if self.fog.cleared.any_in_rect(rect, m) {
            7
        } else if self.fog.seen.any_in_rect(rect, m) {
            3
        } else {
            u8::from(self.fog.explored.any_in_rect(rect, m))
        }
    }

    /// Tile of a world position: `ftol(pos / tile width)` (`GameWorld+0x34/+0x3C`), truncated.
    pub fn world_tile(map: &MapInfo, pos: [f32; 3]) -> (i32, i32) {
        (
            ftol(pos[0] / map.cell[0] as f32),
            ftol(pos[2] / map.cell[2] as f32),
        )
    }

    fn near(&self, map: &MapInfo, pos: [f32; 3], p: i32, rect: bool) -> u8 {
        let (kx, kz) = Self::world_tile(map, pos);
        if rect {
            let r = [
                kx.saturating_sub(2),
                kz.saturating_sub(2),
                kx.saturating_add(2),
                kz.saturating_add(2),
            ];
            self.rect(r, p)
        } else {
            self.area(kx, kz, p, 0)
        }
    }

    /// `GameWorld_IsWorldPosVisibleToPlayer(pos, p, rect)` (`0x0047B7B0`): without rectangle
    /// `area(tile, p, 0) & 2` (with view switch), with rectangle `rect(tile ± 2, p) & 2`.
    pub fn world_pos_visible(&self, map: &MapInfo, pos: [f32; 3], p: i32, rect: bool) -> bool {
        self.near(map, pos, p, rect) & 2 != 0
    }

    /// `0x0047B840`: like [`Self::world_pos_visible`] with `& 1` (explored or more).
    pub fn world_pos_explored(&self, map: &MapInfo, pos: [f32; 3], p: i32, rect: bool) -> bool {
        self.near(map, pos, p, rect) & 1 != 0
    }

    /// `GameWorld_IsTileClearedUpByEnemyOf(x, z, p)` (`0x004765F0`): "cleared" (foreground) with
    /// a bit of an enemy of `p`; without shared vision, false outside.
    pub fn cleared_by_enemy_of(&self, x: i32, z: i32, p: i32) -> bool {
        let m = (0..PLAYER_SLOTS as i32)
            .filter(|&q| self.diplomacy.is_enemy(p, q))
            .fold(0u8, |m, q| m | 1 << q);
        self.fog.cleared.get(x, z).is_some_and(|b| b & m != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit as kit;
    use crate::sim::Sim;

    /// Player 0: explored (1, 1); player 1: seen (2, 1); player 0: everything on (3, 1).
    fn fog() -> Fog {
        let mut fog = Fog::new(8, 8);
        fog.explored.or(1, 1, 0b01);
        fog.seen.or(2, 1, 0b10);
        for g in [&mut fog.explored, &mut fog.seen, &mut fog.cleared] {
            g.or(3, 1, 0b01);
        }
        fog
    }

    fn view<'a>(fog: &'a Fog, d: &'a Diplomacy) -> FogView<'a> {
        FogView {
            fog,
            diplomacy: d,
            show_mode: 0,
        }
    }

    #[test]
    fn points_rank_cleared_seen_explored_and_use_shared_vision() {
        let fog = fog();
        let mut d = Diplomacy::default();
        let v = view(&fog, &d);
        let row = [
            v.state_at(1, 1, 0),
            v.state_at(2, 1, 0),
            v.state_at(3, 1, 0),
            v.state_at(4, 1, 0),
        ];
        assert_eq!(row, [1, 0, 7, 0]);
        assert_eq!(v.state_at(2, 1, 1), 3);
        let outside = [
            v.state_at(-1, 1, 0),
            v.state_at(1, 8, 0),
            v.state_at(1, -1, 0),
            v.state_at(1, 1, 8),
            v.state_at(1, 1, -1),
        ];
        assert_eq!(outside, [0; 5]);
        d.set_vision(0, 1, true);
        let v = view(&fog, &d);
        assert_eq!(
            (v.state_at(2, 1, 0), v.state_at(1, 1, 1)),
            (3, 0),
            "one-sided"
        );
    }

    #[test]
    fn areas_or_the_show_mode_but_points_and_rects_do_not() {
        let fog = fog();
        let d = Diplomacy::default();
        let v = view(&fog, &d);
        assert_eq!(
            (v.area(0, 0, 0, 1), v.area(2, 2, 0, 1), v.area(6, 6, 0, 1)),
            (1, 7, 0)
        );
        let off = v.with_show_mode(7);
        assert_eq!(
            (
                off.area(6, 6, 0, 0),
                off.state_at(6, 6, 0),
                off.rect([5, 5, 7, 7], 0)
            ),
            (7, 0, 0)
        );
        let rects = [
            v.rect([0, 0, 1, 1], 0),
            v.rect([-5, -5, 100, 0], 0),
            v.rect([2, 1, 3, 1], 0),
            v.rect([2, 1, 2, 1], 1),
            v.rect([i32::MIN, i32::MIN, i32::MAX, i32::MAX], 0),
        ];
        assert_eq!(rects, [1, 0, 7, 3, 7]);
    }

    #[test]
    fn world_positions_truncate_to_tiles_and_rects_reach_two_tiles() {
        let fog = fog();
        let d = Diplomacy::default();
        let v = view(&fog, &d);
        let map = MapInfo {
            dim_x: 8,
            dim_z: 8,
            cell: [128; 3],
            water_level: 0.0,
        };
        let at = |x: f32, z: f32| [x * 128.0, 0.0, z * 128.0];
        assert!(v.world_pos_visible(&map, [2.0 * 128.0 + 127.9, 0.0, 128.0], 1, false));
        // `ftol` truncates toward zero: -60 lies on tile 0.
        assert_eq!(FogView::world_tile(&map, [-60.0, 0.0, 200.0]), (0, 1));
        assert!(!v.world_pos_visible(&map, at(4.5, 1.5), 1, false));
        assert!(v.world_pos_visible(&map, at(4.5, 1.5), 1, true));
        assert!(v.world_pos_explored(&map, at(5.5, 1.5), 0, true));
        assert!(!v.world_pos_explored(&map, at(5.5, 1.5), 0, false));
        let off = v.with_show_mode(7);
        assert!(off.world_pos_visible(&map, at(7.5, 7.5), 0, false));
        assert!(
            !off.world_pos_visible(&map, at(7.5, 7.5), 0, true),
            "rectangle without switch"
        );
        assert!(!v.world_pos_visible(&map, [f32::NAN, 0.0, f32::INFINITY], 0, true));
    }

    #[test]
    fn enemies_clear_up_without_shared_vision() {
        let fog = fog();
        let mut d = Diplomacy::default();
        let v = view(&fog, &d);
        assert!(v.cleared_by_enemy_of(3, 1, 1) && !v.cleared_by_enemy_of(3, 1, 0));
        assert!(!v.cleared_by_enemy_of(9, 1, 1) && !v.cleared_by_enemy_of(3, 1, 9));
        d.set_enemy(1, 0, false);
        assert!(!view(&fog, &d).cleared_by_enemy_of(3, 1, 1));
    }

    #[test]
    fn shared_vision_counts_in_queries_but_not_in_stamps() {
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        kit::spawn(
            &mut Sim::new(&mut state, &rules),
            kit::COLONIST,
            1,
            (32, 32),
        );
        for _ in 0..2 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        assert_eq!(FogView::new(&state).state_at(32, 32, 0), 0);
        state.diplomacy.set_vision(0, 1, true);
        assert_eq!(FogView::new(&state).state_at(32, 32, 0), 3);
        for _ in 0..3 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        assert_eq!(state.fog.seen.count(1), 0, "stamped only for the owner");
    }
}
