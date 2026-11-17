//! Scene fade of the scripts: `n145` fades in from black, `n146` fades out to black and stays black. A black
//! full-screen rectangle with alpha `255 - ftol(value)` over the world and game windows, under info windows, dialogs
//! and pointer (re/campaign-ui D6.4). Without simulation: the host sets, ticks and draws.
//!
//! Basis: re/campaign-ui D6.6 (set `FUN_004B96A0`, tick and draw `FUN_004BC130`, reset `FUN_004BAA10`); quirks O-362
//! and O-376.

use crate::bkg_paint::fill_rect;
use crate::coords::Screen;
use crate::draw::DrawCmd;

/// States of the fade (`GA+0x19E4`).
pub mod state {
    /// Off: nothing to draw.
    pub const OFF: u8 = 0;
    /// 146 or black: fades out and then stays black.
    pub const OUT: u8 = 1;
    /// 145: fades in and ends in [`OFF`].
    pub const INTO: u8 = 2;
}

/// Scene fade (`GA+0x19E0...+0x19EC`): step per ms `+0x19E0`, state `+0x19E4`, value `+0x19E8` (0 = black,
/// 255 = clear), remaining time `+0x19EC`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneFade {
    pub state: u8,
    pub value: f32,
    pub step: f32,
    pub rest_ms: f32,
}

impl Default for SceneFade {
    /// The state after [`SceneFade::reset`].
    fn default() -> Self {
        Self {
            state: state::OFF,
            value: 255.0,
            step: 0.0,
            rest_ms: 0.0,
        }
    }
}

impl SceneFade {
    /// Sets the fade (`FUN_004B96A0(ms, direction)`): 145 (`into`) with value 0, state 2 and step `255 / ms`; 146
    /// with value 255, state 1 and step `-255 / ms`. For `ms <= 0` the original divides by 0 or by a negative
    /// number and sets the final value in the first tick; here step 0 and remaining time 0, hence the same image.
    pub fn set(&mut self, ms: i32, into: bool) {
        let (value, state, step) = if into {
            (0.0, state::INTO, 255.0)
        } else {
            (255.0, state::OUT, -255.0)
        };
        self.value = value;
        self.state = state;
        if ms > 0 {
            self.step = step / ms as f32;
            self.rest_ms = ms as f32;
        } else {
            self.step = 0.0;
            self.rest_ms = 0.0;
        }
    }

    /// Reset (`FUN_004BAA10`, level start and loading): state 0, value 255, remaining time 0. The original leaves
    /// the step alone; it only takes effect after the next [`SceneFade::set`], which sets it anew, so here 0 as in
    /// [`SceneFade::default`].
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Black without a fade (deviation from the original: loading a savegame after 146 and before 145): state 1,
    /// value 0, remaining time 0; the next tick keeps the final value.
    pub fn set_black(&mut self) {
        self.state = state::OUT;
        self.value = 0.0;
        self.rest_ms = 0.0;
    }

    /// Tick by `dt_ms` program time (`FUN_004BC130`): only for state != 0 `rest -= dt`, `value += dt * step`; at
    /// `rest <= 0` the final value (state 1 -> 0, else 255), and state 2 ends.
    pub fn tick(&mut self, dt_ms: f32) {
        if self.state == state::OFF {
            return;
        }
        self.rest_ms -= dt_ms;
        self.value += dt_ms * self.step;
        if self.rest_ms <= 0.0 {
            self.value = if self.state == state::OUT { 0.0 } else { 255.0 };
            if self.state == state::INTO {
                self.state = state::OFF;
            }
        }
    }

    /// Alpha of the black full-screen rectangle: `255 - ftol(value)` (truncated toward 0, clamped to 0...255); the
    /// original computes `(~ftol(value)) << 24`, which is the same for 0...255.
    pub fn alpha(&self) -> u8 {
        255 - (self.value as i32).clamp(0, 255) as u8
    }

    /// A black full-screen rectangle `(0, 0, W, H)` with [`SceneFade::alpha`] (`alpharect1`), only for state != 0:
    /// in the frame in which 145 ends, the original still draws a rectangle with alpha 0 (invisible, omitted).
    pub fn draw(&self, screen: Screen, out: &mut Vec<DrawCmd>) {
        if self.state == state::OFF {
            return;
        }
        fill_rect(
            [0, 0, screen.width, screen.height],
            u32::from(self.alpha()) << 24,
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::{Blend, Filter};

    const SCREEN: Screen = Screen {
        width: 1024,
        height: 768,
    };

    fn drawn(fade: &SceneFade) -> Vec<DrawCmd> {
        let mut out = Vec::new();
        fade.draw(SCREEN, &mut out);
        out
    }

    /// O-362: `n145;5000` is black immediately; after 1000 ms the value is 51 (step 255/5000 per ms), the alpha 204
    /// and the color `~51 << 24 = 0xCC00_0000`. After 5000 ms the fade ends and draws nothing any more.
    #[test]
    fn fade_145_starts_black_and_is_204_after_1000_ms() {
        let mut fade = SceneFade::default();
        fade.set(5_000, true);
        assert_eq!(
            (fade.state, fade.value, fade.rest_ms),
            (state::INTO, 0.0, 5_000.0)
        );
        assert_eq!(fade.step, 255.0 / 5_000.0);
        assert_eq!(fade.alpha(), 255, "immediately black");
        fade.tick(1_000.0);
        assert_eq!(fade.value as i32, 51);
        assert_eq!(fade.alpha(), 204);
        assert_eq!(
            drawn(&fade),
            [DrawCmd::Quad {
                texture: None,
                dst: [0, 0, 1024, 768],
                uv: [0.0; 4],
                color: 0xCC00_0000,
                filter: Filter::Point,
                blend: Blend::Alpha,
            }]
        );
        fade.tick(3_999.0);
        assert_eq!(fade.state, state::INTO, "1 ms remaining");
        fade.tick(1.0);
        assert_eq!(
            (fade.state, fade.value, fade.alpha()),
            (state::OFF, 255.0, 0)
        );
        assert!(drawn(&fade).is_empty(), "nothing afterwards");
        fade.tick(100.0);
        assert_eq!(fade.value, 255.0, "no tick without a state");
    }

    /// O-362: 146 fades out to black (alpha 0 -> 255) and then stays black in state 1.
    #[test]
    fn fade_146_ends_black_and_stays() {
        let mut fade = SceneFade::default();
        fade.set(2_000, false);
        assert_eq!(
            (fade.state, fade.value, fade.alpha()),
            (state::OUT, 255.0, 0)
        );
        assert_eq!(drawn(&fade).len(), 1, "state 1 draws, here with alpha 0");
        fade.tick(1_000.0);
        assert_eq!(fade.alpha(), 128, "value 127.5 -> 127");
        fade.tick(1_000.0);
        assert_eq!(
            (fade.state, fade.value, fade.alpha()),
            (state::OUT, 0.0, 255)
        );
        for _ in 0..10 {
            fade.tick(500.0);
        }
        assert_eq!((fade.state, fade.alpha()), (state::OUT, 255), "stays black");
        let cmds = drawn(&fade);
        assert!(
            matches!(
                cmds[..],
                [DrawCmd::Quad {
                    color: 0xFF00_0000,
                    dst: [0, 0, 1024, 768],
                    ..
                }]
            ),
            "{cmds:?}"
        );
        // A following 145 fades in from black.
        fade.set(1_000, true);
        assert_eq!(fade.alpha(), 255);
    }

    /// `ms <= 0`: step 0 and remaining time 0, the final value applies after the first tick (also with `dt = 0`).
    #[test]
    fn fade_with_zero_ms_ends_in_the_first_frame() {
        let mut fade = SceneFade::default();
        fade.set(0, true);
        assert_eq!((fade.step, fade.rest_ms), (0.0, 0.0));
        fade.tick(0.0);
        assert_eq!((fade.state, fade.alpha()), (state::OFF, 0));
        assert!(drawn(&fade).is_empty());
        fade.set(-5, false);
        assert_eq!((fade.step, fade.rest_ms), (0.0, 0.0));
        fade.tick(16.0);
        assert_eq!((fade.state, fade.alpha()), (state::OUT, 255));
    }

    /// `FUN_004BAA10` and the deviation: `reset` is the default state (state 0, value 255, nothing drawn);
    /// `set_black` stays black without a fade and keeps it.
    #[test]
    fn reset_and_set_black() {
        let mut fade = SceneFade::default();
        fade.set(3_000, false);
        fade.tick(1_000.0);
        fade.reset();
        assert_eq!(fade, SceneFade::default());
        assert_eq!((fade.state, fade.value), (state::OFF, 255.0));
        assert_eq!(fade.alpha(), 0);
        assert!(drawn(&fade).is_empty());
        fade.set_black();
        assert_eq!((fade.state, fade.alpha()), (state::OUT, 255));
        fade.tick(16.0);
        assert_eq!((fade.state, fade.alpha()), (state::OUT, 255));
        assert_eq!(drawn(&fade).len(), 1);
    }
}
