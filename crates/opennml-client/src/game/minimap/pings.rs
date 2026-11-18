//! Pings of the minimap (list `GameWorld+0x22F8`; re/game-ui D5.1, N.7.4, O-41, O-219, O-289): addition
//! `GameWorld_Ping(x, z, color, repeats)` (`0x00480C00`), tick and ring in the ping part of `0x00480210`.
//!
//! Host state as in the original (not in the savegame): loading and restart clear the list
//! ([`crate::game::hud::Hud::clear`]). The owner is the `Hud` next to the action stack, because the original keeps
//! both in the world (`GameWorld+0x48`, `+0x22F8`), not in the window of the minimap: without `IsMap`, pings pile up
//! without removal, the stack keeps working.

use opennml_ui::hud::message_bar::ActionStack;

use super::draw::MinimapDot;

/// Repeats of script action 7 (`0x00422E6E PUSH 0xA` before `FUN_0043EBE0`; fixed, not from the argument).
pub const SCRIPT_PING_REPEATS: i32 = 10;
/// Repeats of the signal `0x5C` (`0x0048D4D6...0x0048D539`: `0x00480C00(x, z, color, 1)`).
pub const SIGNAL_PING_REPEATS: i32 = 1;
/// Duration of a repeat: drawn until `t <= 1000` (`CMP ESI, 0x3E8`, `JLE`), then the next one.
pub const PING_MS: i32 = 1000;
/// Radius per millisecond in tiles before the factor `k` (`0x0061C414` = 0.005f).
const RADIUS_PER_MS: f32 = 0.005;
/// Arc between two ring points times radius (`0x0061C410` = 0.62831855f = 2*pi/10).
const POINT_ARC: f32 = 0.628_318_55;
/// Full turn (`0x0061C40C` = 6.2831855f).
const FULL_TURN: f32 = 6.283_185_5;

/// A ping (`{t, repeats, x, z, color}`, `0x0047C260`): clock of the running repeat in ms (game time), remaining
/// repeats, tile (ring around its corner) and color (D3DCOLOR; the ring sets the alpha).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ping {
    pub t_ms: i32,
    pub repeats: i32,
    pub tile: (i32, i32),
    pub color: u32,
    /// Drawn in the last [`MinimapPings::advance`]: the original counts and draws in one pass, and a repeat that just
    /// ended draws nothing in this frame.
    pub drawn: bool,
}

impl Ping {
    /// The points of the ring (`0x00480210`, ping part) in tiles with color including alpha, from angle 0 (point in
    /// +z): radius `r = k * (t * 0.005)` tiles ([`radius_factor`] of the map width `dim_x`); world position
    /// `x = sin(a) * SIZE_X * r + SIZE_X * kx`, `z = cos(a) * SIZE_Z * r + SIZE_Z * kz`; the angle grows by
    /// `0.62831855 / r` as long as it stays below `6.2831855` (a single point for `r = 0`). Alpha
    /// `255 - t * 255 / 1000` (integer). As for the object dots, the tile is world position / tile size
    /// (`0x0047AA60`). `sin`/`cos` calculate in `f64` instead of 80 bits (presentation, deviation from the original).
    pub fn ring_points(&self, dim_x: u32, cell: [i32; 3]) -> Vec<MinimapDot> {
        let r =
            (f64::from(radius_factor(dim_x)) * f64::from(self.t_ms as f32 * RADIUS_PER_MS)) as f32;
        let step = POINT_ARC / r;
        let alpha = (255 - self.t_ms * 255 / PING_MS) as u32;
        let color = alpha << 24 | self.color & 0x00FF_FFFF;
        let (size_x, size_z) = (f64::from(cell[0]), f64::from(cell[2]));
        let base_x = (size_x * f64::from(self.tile.0)) as f32;
        let base_z = (size_z * f64::from(self.tile.1)) as f32;
        let mut out = Vec::new();
        let mut a = 0.0f32;
        loop {
            let (s, c) = f64::from(a).sin_cos();
            let x = (s * size_x * f64::from(r) + f64::from(base_x)) as f32;
            let z = (c * size_z * f64::from(r) + f64::from(base_z)) as f32;
            out.push(MinimapDot {
                tile: [x / cell[0] as f32, z / cell[2] as f32],
                color,
            });
            a += step;
            // `FCOMP`/`JNP`: continue only for `a < 6.2831855` (for `r = 0`, `a` is infinite).
            if a >= FULL_TURN || a.is_nan() {
                break;
            }
        }
        out
    }
}

/// Factor `k` of the end radius (`FUN_0042EBD0`, `+0x23C` = map width): 2/4/8 for 64/128/256, otherwise 1 (O-289).
pub fn radius_factor(dim_x: u32) -> i32 {
    match dim_x {
        64 => 2,
        128 => 4,
        256 => 8,
        _ => 1,
    }
}

/// The ping list with its clock: game time of the session at the last drawn frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MinimapPings {
    list: Vec<Ping>,
    /// Game time of the last [`Self::advance`] (static `0x0067135C` in `0x00480210`); `None` until the first one.
    last_ms: Option<u64>,
}

impl MinimapPings {
    /// `GameWorld_Ping` (`0x00480C00`): first push the location onto the front of the action stack (`0x0047F1A0`,
    /// O-219), then append `{t = 0, repeats, x, z, color}` at the back only if no ping is in the list at the same
    /// tile - whatever its color (O-41).
    pub fn add(&mut self, tile: (i32, i32), color: u32, repeats: i32, stack: &mut ActionStack) {
        stack.push(tile);
        if self.list.iter().any(|p| p.tile == tile) {
            return;
        }
        self.list.push(Ping {
            t_ms: 0,
            repeats,
            tile,
            color,
            drawn: false,
        });
    }

    /// Tick of a frame in which the minimap draws: `dt = now_ms - last` (0 the first time), then per entry
    /// `t' = t + dt`; up to 1000 `t'` stays and the ring is drawn, otherwise `repeats - 1` and at 0 it is gone,
    /// otherwise `t = 0` without a ring (O-289: after a time without drawing, the first frame consumes such a
    /// repeat). Call only from drawn frames; the clock is game time (0 in pause and game menu).
    pub fn advance(&mut self, now_ms: u64) {
        // `FUN_005778A0() - 0x0067135C`; a smaller time (conceivable only after `clear`) counts as 0.
        let dt = self.last_ms.map_or(0, |last| now_ms.saturating_sub(last));
        self.last_ms = Some(now_ms);
        let dt = i32::try_from(dt).unwrap_or(i32::MAX);
        self.list.retain_mut(|p| {
            let t = p.t_ms.saturating_add(dt);
            if t <= PING_MS {
                p.t_ms = t;
                p.drawn = true;
                return true;
            }
            // `repeats - 1`; for < 1 `PtrList_RemoveAt`, otherwise `t = 0` (the surplus time is lost).
            p.repeats -= 1;
            p.t_ms = 0;
            p.drawn = false;
            p.repeats >= 1
        });
    }

    /// The entries in list order.
    pub fn list(&self) -> &[Ping] {
        &self.list
    }

    /// The rings of the last [`Self::advance`] in list order ([`Ping::ring_points`]).
    pub fn dots(&self, dim_x: u32, cell: [i32; 3]) -> Vec<MinimapDot> {
        self.list
            .iter()
            .filter(|p| p.drawn)
            .flat_map(|p| p.ring_points(dim_x, cell))
            .collect()
    }

    /// Clear (loading, restart); the clock starts anew.
    pub fn clear(&mut self) {
        self.list.clear();
        self.last_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::minimap::draw::{MinimapLayout, layout, tests::close};
    use opennml_world::camera::{CameraSetup, GameCamera};

    const CELL: [i32; 3] = [128, 128, 128];
    const RED: u32 = 0xFFFF_0000;
    const BLUE: u32 = 0xFF00_00FF;

    /// Position at 1024 x 768 with theta = 0 (eye at -z of the target) for a map `dim x dim`.
    fn north(dim: u32) -> MinimapLayout {
        let target = glam::Vec3::new(4096.0, 0.0, 4096.0);
        let cam = GameCamera {
            eye: target + glam::Vec3::new(0.0, 8000.0, -8000.0),
            target,
            up: glam::Vec3::Y,
            setup: CameraSetup::default(),
        };
        layout((1024, 768), &cam, (dim, dim))
    }

    /// A ping at (10, 20) with `t` ms via the public clock: addition, first frame (dt 0), second one after `t`.
    fn ping_at(t: u64) -> MinimapPings {
        let mut pings = MinimapPings::default();
        pings.add((10, 20), RED, 5, &mut ActionStack::default());
        pings.advance(1_000);
        pings.advance(1_000 + t);
        pings
    }

    /// Distances of the ring points from the tile corner in screen pixels and the alpha of the ring.
    fn ring_px(pings: &MinimapPings, dim: u32) -> (Vec<f32>, Vec<u32>) {
        let l = north(dim);
        let center = l.tile_to_screen(10.0, 20.0);
        let dots = pings.dots(dim, CELL);
        let dist = dots
            .iter()
            .map(|d| {
                let p = l.tile_to_screen(d.tile[0], d.tile[1]);
                ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt()
            })
            .collect();
        (dist, dots.iter().map(|d| d.color >> 24).collect())
    }

    /// Map 128 (k = 4): t = 500 -> 10 px, alpha 128; t = 1000 -> 20 px, alpha 0; map 96 -> k = 1.
    // QUIRK O-289: kept
    #[test]
    fn ping_radius_and_alpha_m7() {
        assert_eq!([64, 96, 100, 128, 256].map(radius_factor), [2, 1, 1, 4, 8]);
        let (dist, alpha) = ring_px(&ping_at(500), 128);
        assert!(dist.iter().all(|d| (d - 10.0).abs() < 0.01), "{dist:?}");
        assert!(alpha.iter().all(|&a| a == 128), "{alpha:?}");
        // Angular distance (2*pi/10)/r at r = 10 tiles: 100 points, the first in +z.
        assert_eq!(dist.len(), 100);
        let first = ping_at(500).dots(128, CELL)[0];
        close(first.tile, [10.0, 30.0], "angle 0");
        assert_eq!(first.color & 0x00FF_FFFF, RED & 0x00FF_FFFF);
        let (dist, alpha) = ring_px(&ping_at(1000), 128);
        assert!(dist.iter().all(|d| (d - 20.0).abs() < 0.01), "{dist:?}");
        assert!(alpha.iter().all(|&a| a == 0));
        // Map 64: k = 2, so also 10 px (2 px per tile).
        let (dist, _) = ring_px(&ping_at(500), 64);
        assert!(dist.iter().all(|d| (d - 10.0).abs() < 0.01), "{dist:?}");
        // Map 96: k = 1, r = 2.5 tiles = 2.5 * 128/96 px.
        let (dist, _) = ring_px(&ping_at(500), 96);
        assert!(
            dist.iter().all(|d| (d - 2.5 * 128.0 / 96.0).abs() < 0.01),
            "{dist:?}"
        );
        // t = 0: r = 0, step infinite, one point at the corner with full alpha.
        let (dist, alpha) = ring_px(&ping_at(0), 128);
        assert_eq!((dist, alpha), (vec![0.0], vec![255]));
    }

    /// O-41, O-219: the location always goes to the front of the stack, a second ping at the same tile (whatever the
    /// color) is dropped as long as the first one is in the list; afterwards it works again.
    // QUIRK O-41, O-219: kept
    #[test]
    fn second_ping_same_tile_only_stacks() {
        let mut pings = MinimapPings::default();
        let mut stack = ActionStack::default();
        pings.add((5, 5), RED, 1, &mut stack);
        pings.add((5, 5), BLUE, 3, &mut stack);
        pings.add((6, 5), BLUE, 3, &mut stack);
        let tiles: Vec<_> = stack.tiles().iter().copied().collect();
        assert_eq!(tiles, [(6, 5), (5, 5), (5, 5)]);
        let list: Vec<_> = pings
            .list()
            .iter()
            .map(|p| (p.tile, p.color, p.repeats))
            .collect();
        assert_eq!(list, [((5, 5), RED, 1), ((6, 5), BLUE, 3)]);
        assert!(pings.list().iter().all(|p| p.t_ms == 0 && !p.drawn));
        // The one repeat of the first one runs out (t' = 1001 > 1000): gone; the second begins its second one.
        pings.advance(0);
        pings.advance(1_001);
        let list: Vec<_> = pings
            .list()
            .iter()
            .map(|p| (p.tile, p.repeats, p.t_ms))
            .collect();
        assert_eq!(list, [((6, 5), 2, 0)]);
        pings.add((5, 5), BLUE, 1, &mut stack);
        assert_eq!(pings.list().len(), 2, "the tile is free again");
        assert_eq!(stack.tiles().len(), 4);
    }

    /// O-289: the clock counts the game time between two drawn frames. Without drawing (cinemascope, hidden map) the
    /// ping stands still; the first frame afterwards brings the whole time at once and consumes a repeat without a
    /// ring.
    // QUIRK O-289: kept
    #[test]
    fn pings_freeze_when_minimap_hidden() {
        let mut pings = MinimapPings::default();
        pings.add((1, 2), RED, 3, &mut ActionStack::default());
        pings.advance(5_000);
        assert_eq!(
            (pings.list()[0].t_ms, pings.list()[0].drawn),
            (0, true),
            "first frame: dt 0"
        );
        pings.advance(5_400);
        assert_eq!(pings.list()[0].t_ms, 400);
        // Pause and game menu: the same game time, nothing happens.
        pings.advance(5_400);
        assert_eq!(pings.list()[0].t_ms, 400);
        // 3 s without drawing, then a frame: t' = 3400 -> second repeat, no ring in this frame.
        pings.advance(8_400);
        let p = pings.list()[0];
        assert_eq!((p.t_ms, p.repeats, p.drawn), (0, 2, false));
        assert!(pings.dots(128, CELL).is_empty());
        pings.advance(8_450);
        assert_eq!((pings.list()[0].t_ms, pings.list()[0].drawn), (50, true));
        // Loading: list empty, the clock starts anew (a smaller game time does not count backwards).
        pings.clear();
        pings.add((1, 2), RED, 3, &mut ActionStack::default());
        pings.advance(100);
        assert_eq!(pings.list()[0].t_ms, 0);
    }
}
