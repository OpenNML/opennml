//! Cinemascope area of the game UI: black bars with edge tiles, four portrait slots with frames and four text slots
//! (dialog lines 0/1, tips 2/3), each with fade-in and fade-out. Without simulation: the area receives HUD events
//! (`0x1B`, `0x1E` ... `0x21`) and game values via [`GameTags`].
//!
//! Basis: re/campaign-ui D6.1-D6.3 and D6.17 (object `0x004483D0`, drawing `0x004484F0`, messages `0x00447110`);
//! quirks O-48, O-351 ... O-357, O-378.

use std::collections::BTreeMap;

use nml_formats::cinemascope_ini::{
    CINEMASCOPE_INI_PATH, CinemascopeIni, DialogArea, IconStrip, parse_cinemascope_ini,
};
use nml_formats::portraits_ini::{
    PORTRAIT_FRAMES_INI_PATH, PORTRAITS_INI_PATH, parse_portraits_ini,
};
use nml_formats::text::decode_cp1252;
use nml_rda::Vfs;

use super::{DIALOG_FONT, HudEvent, HudEventKind, INFO_TEXT_FONT};
use crate::bkg_paint::{fill_rect, image_quad};
use crate::coords::{VIRTUAL_H, VIRTUAL_W, game_rect, game_x, game_y};
use crate::dialog::UiContext;
use crate::draw::{Blend, DrawCmd, Filter};
use crate::text::{GameTags, LayoutParams, TextLayout, draw_runs, layout_flags, layout_text_game};

/// Color of the bars: opaque black (vertex color `0xFF000000`, state block `alpharect1`).
pub const BAR_COLOR: u32 = 0xFF00_0000;
/// Diffuse color of the edge tiles (`FUN_004020E0(..., 0xFFFFFFFF, 1.0, 1.0, 0)`).
const TILE_COLOR: u32 = 0xFFFF_FFFF;
/// Duration of every fade (`0x1F4`).
pub const FADE_MS: f32 = 500.0;
/// Fade rate per ms (`0x3F028F5C` = 0.51; fade-out with `0xBF028F5C`).
pub const FADE_RATE: f32 = f32::from_bits(0x3F02_8F5C);
/// Layout flags of all text slots `0x46` (`0x40|0x04|0x02`, `0x00448D7C` and branch `0x00448FA3`).
pub const TEXT_LAYOUT: u32 =
    layout_flags::WIDTH | layout_flags::START_COLOR | layout_flags::START_FONT;
/// Position of the tip in slot 2 in pixels.
const TIP2_POS: [i32; 2] = [0, 30];
/// Distance of the tip in slot 3 from the bottom edge of the screen, above its layout height.
const TIP3_BOTTOM_GAP: i32 = 200;

/// State bits of a slot: 1 = visible, 2 = fading, 4 = ends with the fade.
pub mod state {
    pub const VISIBLE: u8 = 1;
    pub const FADING: u8 = 2;
    pub const CLOSING: u8 = 4;
}

/// Data of the area from the constructor `0x004483D0`: `cinemascope.ini`, `portraits.ini` (`+0x4C`) and
/// `portraitframes.ini` (`+0x50`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CinemascopeData {
    pub ini: CinemascopeIni,
    /// Area 0 ([`CinemascopeIni::area0`]); `None` = pixel branch of the original (D6.17).
    pub area: Option<DialogArea>,
    /// Portrait ID -> image path.
    pub portraits: BTreeMap<i32, String>,
    /// Frame ID -> image path.
    pub frames: BTreeMap<i32, String>,
}

impl CinemascopeData {
    /// Reads the three files; a missing file yields an empty part (with a warning).
    pub fn load(vfs: &Vfs) -> Self {
        let read = |path: &str| match vfs.read(path) {
            Ok(bytes) => decode_cp1252(&bytes),
            Err(e) => {
                log::warn!("Cinemascope: {path} not read: {e:#}");
                String::new()
            }
        };
        Self::from_texts(
            &read(CINEMASCOPE_INI_PATH),
            &read(PORTRAITS_INI_PATH),
            &read(PORTRAIT_FRAMES_INI_PATH),
        )
    }

    /// Like [`CinemascopeData::load`] from the texts of the three files.
    pub fn from_texts(cinemascope_ini: &str, portraits_ini: &str, frames_ini: &str) -> Self {
        let ini = parse_cinemascope_ini(cinemascope_ini);
        Self {
            area: ini.area0(),
            ini,
            portraits: parse_portraits_ini(portraits_ini),
            frames: parse_portraits_ini(frames_ini),
        }
    }
}

/// Fade of a slot (state, remaining time, alpha as `f32`, rate per ms).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fade {
    pub state: u8,
    pub rest_ms: f32,
    pub alpha: f32,
    pub rate: f32,
}

impl Fade {
    /// Fade in (`0x00446F00`, `0x00446FC0`): state 3, 500 ms, alpha 0, +0.51/ms.
    pub fn show() -> Self {
        Self {
            state: state::VISIBLE | state::FADING,
            rest_ms: FADE_MS,
            alpha: 0.0,
            rate: FADE_RATE,
        }
    }

    /// Immediately fully visible without a fade: state 1, alpha 255 (restoring after loading, deviation).
    pub fn shown() -> Self {
        Self {
            state: state::VISIBLE,
            rest_ms: 0.0,
            alpha: 255.0,
            rate: FADE_RATE,
        }
    }

    /// Closes a portrait (`0x00446F60`): state 5 **unconditionally**, also on an emptied slot (O-354); 500 ms,
    /// alpha 255, -0.51/ms.
    pub fn hide_portrait(&mut self) {
        self.state = state::VISIBLE | state::CLOSING;
        self.rest_ms = FADE_MS;
        self.alpha = 255.0;
        self.rate = -FADE_RATE;
    }

    /// Closes a text slot (`0x00447020`): state `(s & !2) | 4`, alpha jumps to 255, -0.51/ms, 500 ms.
    pub fn hide_text(&mut self) {
        self.state = (self.state & !state::FADING) | state::CLOSING;
        self.alpha = 255.0;
        self.rate = -FADE_RATE;
        self.rest_ms = FADE_MS;
    }

    /// One step of the fade (`0x00448B92...0x00448C08`, text `0x00448C81...0x00448D0F`) while `state & 6`:
    /// `alpha += dt * rate`, `rest -= dt`. When the remaining time ends (`rest - dt < 1` in whole ms, i.e. `<= 0`),
    /// state 0 applies with bit 4, otherwise bit 2 is cleared. The original computes `dt` in whole ms; the
    /// reimplementation takes what the host supplies (the viewer passes on the fractional frame time) and computes
    /// `rest < 1` in `f32`, so the end there can come up to 1 ms earlier (without a visible effect).
    pub fn step(&mut self, dt_ms: f32) {
        if self.state & (state::FADING | state::CLOSING) == 0 {
            return;
        }
        self.alpha += dt_ms * self.rate;
        self.rest_ms -= dt_ms;
        if self.rest_ms < 1.0 {
            if self.state & state::CLOSING != 0 {
                self.state = 0;
            } else {
                self.state &= !state::FADING;
            }
        }
    }

    /// Alpha as a byte: `ftol` (truncate toward 0), then clamped to 0...255.
    pub fn alpha_u8(&self) -> u8 {
        (self.alpha as i32).clamp(0, 255) as u8
    }

    /// Bit 1: the slot is drawn.
    pub fn visible(&self) -> bool {
        self.state & state::VISIBLE != 0
    }

    /// Color with the alpha of the fade: `(c & 0x00FF_FFFF) | (a << 24)`; -1 yields white.
    fn tint(&self, color: u32) -> u32 {
        (color & 0x00FF_FFFF) | u32::from(self.alpha_u8()) << 24
    }
}

/// A portrait slot (`+0x54` + `0x24*i`): portrait `+0x5C`, frame `+0x60`, colors `+0x70` (frame) and `+0x74`
/// (portrait).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PortraitSlot {
    pub portrait: i32,
    pub frame: i32,
    /// `[frame, portrait]`.
    pub colors: [u32; 2],
    pub fade: Fade,
}

/// A text slot (`+0xE4` + `0x20*i`): text ID `+0xEC`, colors `+0xFC` and `+0x100`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TextSlot {
    pub text: i32,
    /// `[drawn, never used]`: the second color is faded along but never drawn (O-352).
    pub colors: [u32; 2],
    pub fade: Fade,
}

/// The area (`0x188` bytes, vtable `0x00619900`): bar height `+0x40` (virtual, 0 = no bars) and the slots.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CinemascopeView {
    pub bar: i32,
    pub portraits: [PortraitSlot; 4],
    pub texts: [TextSlot; 4],
}

/// Word `i` of the message (after the kind); missing words count as 0.
fn arg(ev: &HudEvent, i: usize) -> i32 {
    ev.args.get(i).copied().unwrap_or(0)
}

/// Slot `pos` as an index, only for 0...3.
fn slot_index(pos: i32) -> Option<usize> {
    usize::try_from(pos).ok().filter(|&p| p < 4)
}

impl CinemascopeView {
    /// Empty area: no bars, all slots in state 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Message (`0x00447110`, jump table `0x0044717C`):
    /// - `0x1B [v]`: bars `v == 0 ? 0 : BARSIZE`; in both cases all portrait states 0 (`0x00446FA0`).
    /// - `0x1E [slot, frame, color 1, portrait, color 2]` (`0x00446F00`) and `0x1F [slot]` (`0x00446F60`): only for
    ///   slots 0...3.
    /// - `0x20 [slot, text, color]` (`0x00446FC0`) and `0x21 [slot]` (`0x00447020`): the original does not check the
    ///   slot and writes behind the fields; the reimplementation ignores slots outside 0...3 (O-357).
    ///
    /// Return value as in the original: true for `0x1B` and `0x1E` ... `0x21`, otherwise false (`0x1C`/`0x1D` go to
    /// the status bar and the minimap).
    pub fn message(&mut self, ev: &HudEvent, data: &CinemascopeData) -> bool {
        match ev.kind {
            HudEventKind::Cinemascope => {
                self.bar = if arg(ev, 0) == 0 {
                    0
                } else {
                    data.ini.bar_size
                };
                for slot in &mut self.portraits {
                    slot.fade.state = 0;
                }
            }
            HudEventKind::PortraitShow => {
                if let Some(i) = slot_index(arg(ev, 0)) {
                    self.portraits[i] = PortraitSlot {
                        frame: arg(ev, 1),
                        portrait: arg(ev, 3),
                        colors: [arg(ev, 2) as u32, arg(ev, 4) as u32],
                        fade: Fade::show(),
                    };
                }
            }
            HudEventKind::PortraitHide => {
                if let Some(i) = slot_index(arg(ev, 0)) {
                    self.portraits[i].fade.hide_portrait();
                }
            }
            HudEventKind::TextShow => {
                if let Some(i) = slot_index(arg(ev, 0)) {
                    let color = arg(ev, 2) as u32;
                    self.texts[i] = TextSlot {
                        text: arg(ev, 1),
                        colors: [color, color],
                        fade: Fade::show(),
                    };
                }
            }
            HudEventKind::TextHide => {
                if let Some(i) = slot_index(arg(ev, 0)) {
                    self.texts[i].fade.hide_text();
                }
            }
            _ => return false,
        }
        true
    }

    /// Like [`CinemascopeView::message`], but a shown portrait or a shown text stands there fully at once
    /// (state 1, alpha 255, without a fade): restoring after loading (deviation).
    pub fn show_now(&mut self, ev: &HudEvent, data: &CinemascopeData) -> bool {
        let handled = self.message(ev, data);
        if let Some(i) = slot_index(arg(ev, 0)) {
            match ev.kind {
                HudEventKind::PortraitShow => self.portraits[i].fade = Fade::shown(),
                HudEventKind::TextShow => self.texts[i].fade = Fade::shown(),
                _ => {}
            }
        }
        handled
    }

    /// Continues the fades (`dt` in ms; the original computes whole ms, the viewer passes on the fractional frame time,
    /// see [`Fade::step`]). The portraits only with bars (block `+0x40 != 0`, O-354), the text slots always (loop after
    /// the bar block `0x00448C5B`, O-355). The original computes this while drawing with `Timer_GetTimeMs`
    /// (deviation).
    pub fn tick(&mut self, dt_ms: f32) {
        if self.bar != 0 {
            for slot in &mut self.portraits {
                slot.fade.step(dt_ms);
            }
        }
        for slot in &mut self.texts {
            slot.fade.step(dt_ms);
        }
    }

    /// Drawing `0x004484F0` (read-only):
    /// 1. With bars: at the top `game_rect(0, 0, 1280, bar)`, at the bottom `game_rect(0, 1024 - bar, 1280, 1024)`,
    ///    opaque black (O-351).
    /// 2. Edge tiles (`0x004489D5...0x00448B88`): cell 0, white, POINT, unscaled, `(game_x(1280) - game_x(0)) / w`
    ///    pieces from x = 0; at the top at `game_y(BARSIZE)`, at the bottom at `game_y(1024 - BARSIZE) - h` (O-351).
    /// 3. Portrait slots 0...3 in the same rectangle (O-48), first the frame, then the portrait (O-353).
    /// 4. Text slots 0...3 with bit 1, also without bars (O-355).
    pub fn draw(
        &self,
        data: &CinemascopeData,
        ctx: &mut UiContext<'_>,
        game: &GameTags<'_>,
        out: &mut Vec<DrawCmd>,
    ) {
        if self.bar != 0 {
            self.draw_bars(data, ctx, out);
            let rect = self.portrait_rect(data, ctx);
            for slot in self.portraits.iter().filter(|s| s.fade.visible()) {
                draw_portrait(
                    ctx,
                    &data.frames,
                    slot.frame,
                    rect,
                    slot.fade.tint(slot.colors[0]),
                    out,
                );
                draw_portrait(
                    ctx,
                    &data.portraits,
                    slot.portrait,
                    rect,
                    slot.fade.tint(slot.colors[1]),
                    out,
                );
            }
        }
        for (pos, slot) in self.texts.iter().enumerate() {
            if slot.fade.visible() {
                draw_text_slot(data, ctx, game, pos, slot, out);
            }
        }
    }

    /// Bars and edge tiles.
    fn draw_bars(&self, data: &CinemascopeData, ctx: &mut UiContext<'_>, out: &mut Vec<DrawCmd>) {
        let s = ctx.screen;
        fill_rect(game_rect(s, [0, 0, VIRTUAL_W, self.bar]), BAR_COLOR, out);
        fill_rect(
            game_rect(s, [0, VIRTUAL_H - self.bar, VIRTUAL_W, VIRTUAL_H]),
            BAR_COLOR,
            out,
        );
        // The tiles compute with `+0x3C` (BARSIZE); with bars that is the same number as `+0x40`.
        let bar_size = data.ini.bar_size;
        if let Some(strip) = &data.ini.bar_top {
            draw_tiles(ctx, strip, game_y(s, bar_size), out);
        }
        if let Some(strip) = &data.ini.bar_down {
            let y = game_y(s, VIRTUAL_H - bar_size).wrapping_sub(strip.tile[1]);
            draw_tiles(ctx, strip, y, out);
        }
    }

    /// Portrait rectangle `FUN_004477E0(0)`: with an area `game_rect(PORTRAITPOS, PORTRAITPOS + PORTRAITSIZE)`, without
    /// an area `(0, 0, BARSIZE, BARSIZE)` in pixels (index always 0, no conversion; D6.17).
    fn portrait_rect(&self, data: &CinemascopeData, ctx: &UiContext<'_>) -> [i32; 4] {
        match &data.area {
            Some(area) => {
                let [x, y] = area.portrait_pos;
                let [w, h] = area.portrait_size;
                game_rect(ctx.screen, [x, y, x.wrapping_add(w), y.wrapping_add(h)])
            }
            None => {
                let bar = data.ini.bar_size;
                [0, 0, bar, bar]
            }
        }
    }

    /// Occupied slots for the control channel (bit 1): portraits as `(slot, portrait, frame, alpha)`, texts as
    /// `(slot, text, alpha)`.
    #[allow(clippy::type_complexity)]
    pub fn slots(&self) -> (Vec<(usize, i32, i32, u8)>, Vec<(usize, i32, u8)>) {
        let portraits = self
            .portraits
            .iter()
            .enumerate()
            .filter(|(_, s)| s.fade.visible())
            .map(|(i, s)| (i, s.portrait, s.frame, s.fade.alpha_u8()))
            .collect();
        let texts = self
            .texts
            .iter()
            .enumerate()
            .filter(|(_, s)| s.fade.visible())
            .map(|(i, s)| (i, s.text, s.fade.alpha_u8()))
            .collect();
        (portraits, texts)
    }
}

/// A row of edge tiles at `y`: `FUN_004020E0(pt, 0, -1, 1.0, 1.0, 0)` per tile, x = 0, w, 2w ... Nothing without an
/// image or with a tile width <= 0 (the original divides by the width).
fn draw_tiles(ctx: &mut UiContext<'_>, strip: &IconStrip, y: i32, out: &mut Vec<DrawCmd>) {
    let [w, h] = strip.tile;
    if w <= 0 {
        return;
    }
    let Some(texture) = ctx.assets.texture(ctx.vfs, &strip.path) else {
        return;
    };
    let s = ctx.screen;
    let count = (game_x(s, VIRTUAL_W) - game_x(s, 0)) / w;
    for i in 0..count {
        let x = i * w;
        let dst = [x, y, x.wrapping_add(w), y.wrapping_add(h)];
        image_quad(
            ctx.assets,
            texture,
            strip.tile,
            0,
            dst,
            dst,
            TILE_COLOR,
            Filter::Point,
            out,
        );
    }
}

/// `FUN_004FA210(id, rect, color)`: the whole image (UV 0...1) in the rectangle, state block `billboardalpha` (LINEAR).
/// An unknown ID silently draws nothing (O-353). A missing file likewise; the original reloads it every frame, the
/// reimplementation tries once (deviation from the original; [`crate::assets::UiAssets::texture`] remembers the
/// failure, O-378). The info frames also draw their portrait this way ([`super::info_frame`]).
pub(super) fn draw_portrait(
    ctx: &mut UiContext<'_>,
    set: &BTreeMap<i32, String>,
    id: i32,
    rect: [i32; 4],
    color: u32,
    out: &mut Vec<DrawCmd>,
) {
    let Some(path) = set.get(&id) else {
        return;
    };
    let Some(texture) = ctx.assets.texture(ctx.vfs, path) else {
        return;
    };
    out.push(DrawCmd::Quad {
        texture: Some(texture),
        dst: rect,
        uv: [0.0, 0.0, 1.0, 1.0],
        color,
        filter: Filter::Linear,
        blend: Blend::Alpha,
    });
}

/// A text slot (`0x00448D11...0x0044908D`), layout in every frame with [`TEXT_LAYOUT`] and the first color:
/// - 0/1 with an area: font `TEXTFONT` unchanged, origin `(game_x(TEXTPOS.x), game_y(TEXTPOS.y))`, width
///   `game_x(TEXTSIZE.x)`, without a shadow; both slots at the same position (O-48).
/// - 0/1 without an area (D6.17, pixels with `bar` = raw value BARSIZE): slot 0 at `(bar, 0)`, slot 1 at
///   `(bar, H - bar)`, width `W - 2*bar`, font `DialogText`, without a shadow.
/// - 2/3: `ShowInfoText`, width W/2, with a shadow; slot 2 at (0, 30), slot 3 at (BARSIZE, H - layout height - 200)
///   (O-356).
///
/// The text comes via `FUN_00562770` -> `FUN_0055FFE0`: an unknown ID sets its own number (`"%d"`, O-241).
fn draw_text_slot(
    data: &CinemascopeData,
    ctx: &mut UiContext<'_>,
    game: &GameTags<'_>,
    pos: usize,
    slot: &TextSlot,
    out: &mut Vec<DrawCmd>,
) {
    let s = ctx.screen;
    let bar = data.ini.bar_size;
    let text = ctx
        .texts
        .get(slot.text)
        .map(str::to_string)
        .unwrap_or_else(|| slot.text.to_string());
    let color = slot.fade.tint(slot.colors[0]);
    let lay = |font: &str, width: i32| {
        let params = LayoutParams {
            flags: TEXT_LAYOUT,
            width,
            font: ctx.fonts.by_name(font),
            color,
        };
        let mut layout = TextLayout::default();
        layout_text_game(&text, &params, ctx.fonts, &mut layout, None, Some(game));
        layout
    };
    if pos < 2 {
        let (layout, origin) = match &data.area {
            Some(area) => {
                let width = game_x(s, area.text_size[0]);
                let origin = [game_x(s, area.text_pos[0]), game_y(s, area.text_pos[1])];
                (lay(&area.text_font, width), origin)
            }
            None => {
                let width = s.width.wrapping_sub(bar.wrapping_mul(2));
                let y = if pos == 0 {
                    0
                } else {
                    s.height.wrapping_sub(bar)
                };
                (lay(DIALOG_FONT, width), [bar, y])
            }
        };
        draw_runs(&layout, origin, false, None, ctx.fonts, out);
    } else {
        let layout = lay(INFO_TEXT_FONT, s.width / 2);
        let origin = if pos == 2 {
            TIP2_POS
        } else {
            [
                bar,
                s.height
                    .wrapping_sub(layout.height)
                    .wrapping_sub(TIP3_BOTTOM_GAP),
            ]
        };
        draw_runs(&layout, origin, true, None, ctx.fonts, out);
    }
}

#[cfg(test)]
mod tests;
