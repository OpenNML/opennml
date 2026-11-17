//! Minimap at the bottom right: terrain from map and light (`terrain`), fog in a double buffer (`fog`), the draw
//! list with dots and camera frame (`draw`), the pings (`pings`; owner is the `Hud`) and the mouse events
//! ([`Minimap::mouse`]). Without GPU and without `egui`; the `Hud` keeps the corner buttons as children of the
//! node (`nml_ui::hud::toolbar::minimap_buttons`).

use std::cell::Cell;

use nml_ui::assets::UiAssets;
use nml_ui::draw::TextureId;
use nml_ui::geom::contains;
use nml_ui::hud::HudEventKind;

use crate::game::input_gate::InputGate;

pub mod draw;
pub mod fog;
pub mod pings;
pub mod terrain;

pub use draw::{MinimapDot, MinimapFrame, MinimapLayout};
pub use fog::MinimapFog;
pub use pings::{MinimapPings, Ping};
pub use terrain::MinimapTerrain;

/// Fixed-point factor of `Fixed_FromFloat` (`0x00547980`: `FMUL 4096.0f` `0x006285F8`, then `_ftol`).
const FIXED_ONE: f32 = 4096.0;

/// Effect of a mouse event of the minimap ([`Minimap::mouse`]); the host carries it out
/// (`GameSession::minimap_click`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinimapClick {
    /// Left press in the diamond (`0x00449D20` -> `0x004323F0`): camera to `(SIZE_X*mx, 0, SIZE_Z*mz)`, the eye
    /// keeps the offset. `tile` in floating-point tiles.
    Camera { tile: (f32, f32) },
    /// Right release or double click in the diamond with a selection (`0x00449DD0`): move command of the selection
    /// to `x, z` (fixed point `(m * 4096.0f) as i32`, truncated toward 0), `run` for the double click. Whether it
    /// goes out (first selected object own) is checked by the host; the event is consumed either way.
    Move { x: i32, z: i32, run: bool },
}

/// State of the minimap in the host: terrain and fog buffers, plus the status of the two counters of the
/// [`SimHost`] that it has already processed. Host state, not simulation state: neither in the checksum nor in the
/// savegame.
///
/// [`SimHost`]: crate::game::sim_host::SimHost
#[derive(Debug, Clone)]
pub struct Minimap {
    terrain: MinimapTerrain,
    fog: MinimapFog,
    /// Counter of the idle ticks ([`SimHost::fog_idle_ticks`](crate::game::sim_host::SimHost::fog_idle_ticks)) up
    /// to which the fog rows are written.
    seen_idle_ticks: u32,
    /// Counter of the immediate rebuilds ([`SimHost::fog_rebuilds`](crate::game::sim_host::SimHost::fog_rebuilds))
    /// up to which they are carried out.
    seen_rebuilds: u32,
    /// Visible (`Minimap+0x4D`): starts on, event `0x1D` switches it ([`Self::set_visible`]). Not the visibility of
    /// the window node: the node also takes the mouse when the map is hidden (O-291).
    visible: bool,
    /// Map mode (`GameWorld+0x22D4`): 0 all, 1 economy, 2 military ([`draw::dots`]); not in the savegame (as in
    /// the original). Set by the buttons/keys 573 ... 575 (`GameSession::minimap_button`).
    pub mode: u8,
    /// Position of the last drawn frame (`Minimap+0x0C`, the matrix stays until the next frame); set by drawing
    /// (`&self`, hence the cell), clicks calculate with it. `None` before the first frame.
    last_layout: Cell<Option<MinimapLayout>>,
    /// Right press seen in the rectangle (`Minimap+0x70`): only then does the release count (raw kind 4).
    right_down: bool,
}

impl Minimap {
    /// Both buffers first show the unfogged terrain; the counters start at 0 (the host counts since its start, the
    /// first [`Self::sync`] catches up the immediate rebuild of the level start).
    pub fn new(terrain: MinimapTerrain) -> Self {
        let fog = MinimapFog::new(&terrain.image);
        Self {
            terrain,
            fog,
            seen_idle_ticks: 0,
            seen_rebuilds: 0,
            visible: true,
            mode: 0,
            last_layout: Cell::new(None),
            right_down: false,
        }
    }

    /// Visible (`+0x4D`): it is drawn only then (`0x00449C30`); it also needs the node (`IsMap`) and no cinemascope
    /// (`InputGate::minimap_visible`).
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Event `{0x1D, w}` (`+0x4D = (w != 0)`, cinemascope on/off; re/game-ui D5.1). The corner buttons for it are
    /// built by `Hud::route`.
    pub fn set_visible(&mut self, on: bool) {
        self.visible = on;
    }

    /// Position of the last drawn frame ([`draw::draw`]); `None` as long as nothing was drawn.
    pub fn last_layout(&self) -> Option<MinimapLayout> {
        self.last_layout.get()
    }

    /// Screen point -> tile with the last drawn position ([`MinimapLayout::screen_to_tile`]); `None` before the
    /// first frame.
    pub fn screen_to_tile(&self, p: [f32; 2]) -> Option<(f32, f32)> {
        self.last_layout.get()?.screen_to_tile(p)
    }

    /// A mouse event `kind` at `at` (UI pixels) according to re/game-ui D5.1, section "Input" (slot 2 `0x0044A4C0`,
    /// slot 3 `0x0044A630`): `(consumed, effect)`. `rect` is the window rectangle (`PtInRect` on `Window+0x08`),
    /// `selected` "selection not empty" (`0x0047A6F0` in `0x00449DD0`).
    ///
    /// - 1 move: consumed in the rectangle (the pointer 7 is selected by `pointer`).
    /// - 3 left press: lock -> consumed; otherwise camera jump if the point is in the rectangle and in the diamond.
    /// - 2 left release: lock -> consumed; otherwise consumed in the rectangle. `0x13` left double click: consumed in
    ///   the rectangle, without a lock check.
    /// - 5 right press: lock -> consumed (without flag); otherwise consumed in the rectangle with `+0x70 = 1`.
    /// - 4 right release: only with `+0x70` (which falls in the process); lock -> consumed, otherwise
    ///   [`Self::move_order`].
    /// - `0x12` right double click: lock -> consumed; otherwise [`Self::move_order`] running, without a `+0x70` check.
    ///
    /// The lock is `gate.minimap_swallows_clicks()` (`+0x4C`, O-290). None of the branches checks the visibility
    /// (`+0x4D`) (O-291): hidden, the map calculates with the last drawn position.
    pub fn mouse(
        &mut self,
        kind: HudEventKind,
        at: [i32; 2],
        rect: [i32; 4],
        gate: InputGate,
        selected: bool,
    ) -> (bool, Option<MinimapClick>) {
        use HudEventKind as K;
        let locked = gate.minimap_swallows_clicks();
        let in_rect = contains(rect, at[0], at[1]);
        match kind {
            K::MouseMove | K::DoubleLeft => (in_rect, None),
            K::LeftUp => (locked || in_rect, None),
            K::LeftDown if locked => (true, None),
            K::LeftDown => match self.screen_to_tile(at.map(|v| v as f32)) {
                Some(tile) if in_rect => (true, Some(MinimapClick::Camera { tile })),
                _ => (false, None),
            },
            K::RightDown if locked => (true, None),
            K::RightDown => {
                if in_rect {
                    self.right_down = true;
                }
                (in_rect, None)
            }
            K::RightUp => {
                if !std::mem::take(&mut self.right_down) {
                    (false, None)
                } else if locked {
                    (true, None)
                } else {
                    self.move_order(at, in_rect, selected, false)
                }
            }
            K::DoubleRight if locked => (true, None),
            K::DoubleRight => self.move_order(at, in_rect, selected, true),
            _ => (false, None),
        }
    }

    /// `0x00449DD0(x, y, double)`: consumed if the point is in the rectangle and in the diamond and the selection is
    /// not empty; then the command with the target `Fixed_FromFloat(mx)`, `Fixed_FromFloat(mz)`. The ownership check
    /// of the first object follows in the host (without a command the event stays consumed).
    fn move_order(
        &self,
        at: [i32; 2],
        in_rect: bool,
        selected: bool,
        run: bool,
    ) -> (bool, Option<MinimapClick>) {
        match self.screen_to_tile(at.map(|v| v as f32)) {
            Some((mx, mz)) if in_rect && selected => (
                true,
                Some(MinimapClick::Move {
                    x: (mx * FIXED_ONE) as i32,
                    z: (mz * FIXED_ONE) as i32,
                    run,
                }),
            ),
            _ => (false, None),
        }
    }

    /// ID of the front fog image in `assets` under [`draw::FOG_TEXTURE`]. Uploaded only if the image has changed
    /// compared to the one lying there - after a swap of the buffers or an immediate rebuild -, not in every frame:
    /// replacing raises the revision, and the drawer reloads the texture.
    pub fn texture(&self, assets: &mut UiAssets) -> TextureId {
        let front = self.fog.front();
        match assets.find(draw::FOG_TEXTURE) {
            Some(id) if assets.image(id) == front => id,
            _ => assets.insert_image(draw::FOG_TEXTURE, front.clone()),
        }
    }

    pub fn terrain(&self) -> &MinimapTerrain {
        &self.terrain
    }

    /// The fog image that is drawn.
    pub fn fog(&self) -> &MinimapFog {
        &self.fog
    }

    /// Catches up on what the host has counted since the last call.
    ///
    /// - `rebuilds`: counter of the immediate rebuilds. If it has advanced, `0x00479160(1)` makes the buffers
    ///   completely new - only with fog on (`show_mode == 0`), otherwise the original does not call it. After an
    ///   immediate rebuild the idle ticks counted until then are done.
    /// - `idle_ticks`: counter of the idle ticks of the fog cycle. For each counted idle tick the map writes
    ///   [`fog::rows_per_idle_tick`] rows once. The host does not read until after the step; the original reads in the
    ///   middle of the step, before what AI and scripts of the step can still change in sight (a deviation of the
    ///   presentation). If several idle ticks arrive at once (several steps per frame), each reads the state after
    ///   the last.
    /// - `level(x, z)`: fog level of the tile for the local player with the view switch
    ///   (`GameWorld_QueryFogOfWarArea(x, z, -1, 0)`).
    pub fn sync(
        &mut self,
        idle_ticks: u32,
        rebuilds: u32,
        show_mode: u8,
        level: impl Fn(u32, u32) -> u8,
    ) {
        if rebuilds != self.seen_rebuilds {
            self.seen_rebuilds = rebuilds;
            if show_mode == 0 {
                self.fog.rebuild_now(&self.terrain.colors, &level);
                self.seen_idle_ticks = idle_ticks;
                return;
            }
        }
        let pending = idle_ticks.wrapping_sub(self.seen_idle_ticks);
        self.seen_idle_ticks = idle_ticks;
        for _ in 0..pending {
            self.fog.idle_tick(&self.terrain.colors, &level);
        }
    }

    /// [`Self::sync`] with the state of the host.
    pub fn sync_host(&mut self, host: &crate::game::sim_host::SimHost) {
        let view = host.fog.view(&host.state);
        let player = i32::from(host.active_player);
        self.sync(
            host.fog_idle_ticks(),
            host.fog_rebuilds(),
            host.fog.show_mode,
            |x, z| view.area(x as i32, z as i32, player, 0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    /// Terrain 4 x 20 with all tiles identical `0x00225971`; the color buffer lacks no tile.
    fn minimap() -> Minimap {
        let colors = vec![0x0022_5971; 4 * 20];
        let image = RgbaImage::from_pixel(4, 20, Rgba([0x71, 0x59, 0x22, 255]));
        Minimap::new(MinimapTerrain {
            dim_x: 4,
            dim_z: 20,
            colors,
            image,
        })
    }

    fn top_pixel(map: &Minimap) -> [u8; 4] {
        map.fog().front().get_pixel(0, 0).0
    }

    #[test]
    fn rebuild_now_only_with_fog_on() {
        let explored = |_: u32, _: u32| 1;
        // Fog off (`show_mode` 7): the immediate rebuild is skipped, the terrain stays.
        let mut map = minimap();
        map.sync(0, 1, 7, explored);
        assert_eq!(top_pixel(&map), [0x71, 0x59, 0x22, 255]);
        // Fog on: both buffers completely new (explored: half brightness).
        let mut map = minimap();
        map.sync(0, 1, 0, explored);
        assert_eq!(top_pixel(&map), [0x38, 0x2C, 0x11, 255]);
        // The same counter value does not rebuild again.
        map.sync(0, 1, 0, |_, _| 0);
        assert_eq!(top_pixel(&map), [0x38, 0x2C, 0x11, 255]);
    }

    #[test]
    fn rows_follow_the_counted_idle_ticks_only() {
        let mut map = minimap();
        map.sync(0, 0, 0, |_, _| 0);
        assert_eq!(map.fog().next_row(), 0, "no row without an idle tick");
        map.sync(1, 0, 0, |_, _| 0);
        assert_eq!(map.fog().next_row(), 4);
        map.sync(1, 0, 0, |_, _| 0);
        assert_eq!(
            map.fog().next_row(),
            4,
            "the same counter value writes nothing"
        );
        map.sync(3, 0, 0, |_, _| 0);
        assert_eq!(map.fog().next_row(), 12, "two counted idle ticks at once");
    }

    /// Map 128 with theta = 0 at 1024 x 768 as the last drawn position: tile (mx, mz) -> (871 + mx, 743 - mz).
    fn drawn_minimap() -> Minimap {
        let colors = vec![0; 128 * 128];
        let image = RgbaImage::new(128, 128);
        let map = Minimap::new(MinimapTerrain {
            dim_x: 128,
            dim_z: 128,
            colors,
            image,
        });
        map.last_layout.set(Some(MinimapLayout {
            center: [935.0, 679.0],
            theta: 0.0,
            sx: 1.0,
            sz: 1.0,
            dim: [128, 128],
        }));
        map
    }

    /// Rectangle of the window at 1024 x 768 (`coords::minimap_rect`).
    const RECT: [i32; 4] = [848, 592, 1024, 768];
    /// In the diamond (tile (64, 64)), in the rectangle outside the diamond, outside the rectangle.
    const DIAMOND: [i32; 2] = [935, 679];
    const CORNER: [i32; 2] = [850, 600];
    const WORLD: [i32; 2] = [500, 300];

    fn locked() -> InputGate {
        InputGate {
            ui_locked: true,
            ..InputGate::default()
        }
    }

    /// re/game-ui D5.1, section "Input" (slot 2 `0x0044A4C0`, slot 3 `0x0044A630`) without a lock: what is consumed
    /// and which click results.
    #[test]
    fn mouse_follows_the_d5_1_table() {
        use HudEventKind as K;
        let free = InputGate::default();
        let mut m = drawn_minimap();
        let ev = |m: &mut Minimap, kind, at, selected| m.mouse(kind, at, RECT, free, selected);
        // 1 move: consumed in the rectangle, without a click.
        assert_eq!(ev(&mut m, K::MouseMove, CORNER, true), (true, None));
        assert_eq!(ev(&mut m, K::MouseMove, WORLD, true), (false, None));
        // 3 left press: only in the diamond, then camera jump.
        assert_eq!(
            ev(&mut m, K::LeftDown, DIAMOND, false),
            (true, Some(MinimapClick::Camera { tile: (64.0, 64.0) }))
        );
        assert_eq!(ev(&mut m, K::LeftDown, CORNER, true), (false, None));
        assert_eq!(ev(&mut m, K::LeftDown, WORLD, true), (false, None));
        // 2 and `0x13`: consumed in the rectangle without effect.
        for k in [K::LeftUp, K::DoubleLeft] {
            assert_eq!(ev(&mut m, k, CORNER, true), (true, None), "{k:?}");
            assert_eq!(ev(&mut m, k, WORLD, true), (false, None), "{k:?}");
        }
        // 4 without a prior 5 in the rectangle: not consumed.
        assert_eq!(ev(&mut m, K::RightUp, DIAMOND, true), (false, None));
        // 5 outside: not consumed, no flag.
        assert_eq!(ev(&mut m, K::RightDown, WORLD, true), (false, None));
        assert_eq!(ev(&mut m, K::RightUp, DIAMOND, true), (false, None));
        // 5 in the rectangle sets `+0x70`; 4 in the diamond sends the command (fixed point from the
        // floating-point tile).
        assert_eq!(ev(&mut m, K::RightDown, CORNER, true), (true, None));
        let at = [900, 700];
        let want = MinimapClick::Move {
            x: (29.0f32 * 4096.0) as i32,
            z: (43.0f32 * 4096.0) as i32,
            run: false,
        };
        assert_eq!(ev(&mut m, K::RightUp, at, true), (true, Some(want)));
        // The flag is used up.
        assert_eq!(ev(&mut m, K::RightUp, at, true), (false, None));
        // 4 with the flag in the rectangle outside the diamond or without a selection: not consumed
        // (`0x00449DD0` false).
        ev(&mut m, K::RightDown, DIAMOND, true);
        assert_eq!(ev(&mut m, K::RightUp, CORNER, true), (false, None));
        ev(&mut m, K::RightDown, DIAMOND, true);
        assert_eq!(ev(&mut m, K::RightUp, DIAMOND, false), (false, None));
        // `0x12`: without a flag check, running.
        assert_eq!(
            ev(&mut m, K::DoubleRight, DIAMOND, true),
            (
                true,
                Some(MinimapClick::Move {
                    x: 64 << 12,
                    z: 64 << 12,
                    run: true
                })
            )
        );
        assert_eq!(ev(&mut m, K::DoubleRight, WORLD, true), (false, None));
        // The minimap never consumes the wheel.
        assert_eq!(ev(&mut m, K::WheelUp, DIAMOND, true), (false, None));
        // Before the first frame there is no diamond, but the rectangle.
        let mut fresh = drawn_minimap();
        fresh.last_layout.set(None);
        assert_eq!(
            fresh.mouse(K::LeftDown, DIAMOND, RECT, free, true),
            (false, None)
        );
        assert_eq!(
            fresh.mouse(K::LeftUp, DIAMOND, RECT, free, true),
            (true, None)
        );
    }

    /// O-290 (`0x0044A550`/`0x0044A56F` -> `0x0044A5E2`): under the lock 2, 3, 5 and `0x12` are consumed everywhere,
    /// without effect; 5 does not set the flag; 4 only with an already set flag (which falls in the process);
    /// `0x13` and the move as without the lock.
    #[test]
    fn the_lock_takes_clicks_before_the_rect_check() {
        use HudEventKind as K;
        let mut m = drawn_minimap();
        for at in [DIAMOND, CORNER, WORLD] {
            for k in [K::LeftDown, K::LeftUp, K::RightDown, K::DoubleRight] {
                assert_eq!(
                    m.mouse(k, at, RECT, locked(), true),
                    (true, None),
                    "{k:?} {at:?}"
                );
            }
            // 5 under the lock has set no flag.
            assert_eq!(m.mouse(K::RightUp, at, RECT, locked(), true), (false, None));
        }
        assert_eq!(
            m.mouse(K::DoubleLeft, WORLD, RECT, locked(), true),
            (false, None)
        );
        assert_eq!(
            m.mouse(K::DoubleLeft, CORNER, RECT, locked(), true),
            (true, None)
        );
        assert_eq!(
            m.mouse(K::MouseMove, WORLD, RECT, locked(), true),
            (false, None)
        );
        // Flag set before the lock: 4 is consumed (without a command) and uses it up.
        assert_eq!(
            m.mouse(K::RightDown, CORNER, RECT, InputGate::default(), true),
            (true, None)
        );
        assert_eq!(
            m.mouse(K::RightUp, WORLD, RECT, locked(), true),
            (true, None)
        );
        assert_eq!(
            m.mouse(K::RightUp, WORLD, RECT, locked(), true),
            (false, None)
        );
    }

    /// O-291: the hidden map (`+0x4D = 0`) keeps calculating with the last drawn position.
    #[test]
    fn hidden_map_keeps_the_last_layout_for_clicks() {
        let mut m = drawn_minimap();
        m.set_visible(false);
        assert_eq!(
            m.mouse(
                HudEventKind::LeftDown,
                DIAMOND,
                RECT,
                InputGate::default(),
                false
            ),
            (true, Some(MinimapClick::Camera { tile: (64.0, 64.0) }))
        );
    }

    #[test]
    fn a_rebuild_clears_the_pending_idle_ticks_and_the_pass() {
        let mut map = minimap();
        map.sync(2, 0, 0, |_, _| 0);
        assert_eq!(map.fog().next_row(), 8);
        map.sync(4, 1, 0, |_, _| 1);
        assert_eq!(map.fog().next_row(), 0);
        map.sync(5, 1, 0, |_, _| 1);
        assert_eq!(map.fog().next_row(), 4);
    }
}
