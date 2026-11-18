//! Info windows of the campaign: the window of action 168 and the speech window with portrait of action 213, both as
//! widget `FUN_0045DC20` on the desktop. Without simulation: kind and text arrive as [`InfoFrameKind`], game values for
//! the counters via [`GameTags`].
//!
//! Basis: re/campaign-ui D6.5 and D6.17 (actions `0x004434E0`/`0x00443A20`, `OnAttach` `0x0045DCE0`, `Draw`
//! `0x0045D980`); quirks O-358 ... O-360, O-377 and E-248.

use std::cell::OnceCell;

use super::INFO_FRAME_FONT;
use super::cinemascope::{CinemascopeData, draw_portrait};
use crate::bkg_paint::{LoadedBkg, load_bkg, paint_bkg};
use crate::coords::{
    VIRTUAL_W, to_screen_rect, to_screen_x, to_screen_y, to_virtual_x, to_virtual_y,
};
use crate::dialog::UiContext;
use crate::draw::DrawCmd;
use crate::geom::{client_rel, intersect};
use crate::text::{GameTags, LayoutParams, TextLayout, draw_runs, layout_flags, layout_text_game};

/// Background of window 168 (`0x006193EC`): 9-slice `Tooltip01.tga`, `CLIENTAREA 6 6 6 6`.
pub const INFO_FRAME_BKG: &str = "data/gifcon/infoframe.bkg";
/// Background of speech window 213 (`0x00619420`): `CLIENTAREA 15 30 15 30`.
pub const SPEECH_FRAME_BKG: &str = "data/menü/menuframe.bkg";
/// Color of text, portrait and frame (-1).
const WHITE: u32 = 0xFFFF_FFFF;
/// Edge length of the start rectangle of 168 (`(x, y, x + 100, y + 100)`, `0x00443578...0x004435A4`).
const WINDOW_START: i32 = 100;
/// Start rectangle of 213 (`(0, 0x28, 0x258, 0x12C)`, `0x00443A81...0x00443AD9`).
const SPEECH_RECT: [i32; 4] = [0, 40, 600, 300];
/// Portrait size of 213 (`0x64 x 0x64`).
const SPEECH_PORTRAIT: [i32; 2] = [100, 100];
/// Style of 213 without bit 1 (`0x00443AAC`).
const SPEECH_STYLE: u32 = 0x5C;

/// Style bits `+0x94` (D6.5).
pub mod style {
    /// Right-aligned: x = 1280 - width.
    pub const RIGHT: u32 = 0x01;
    /// x = 0.
    pub const LEFT: u32 = 0x02;
    /// Draw the portrait, text shifted right by the portrait width.
    pub const PORTRAIT: u32 = 0x04;
    /// Draw the portrait frame, text shifted right by the portrait width.
    pub const PORTRAIT_FRAME: u32 = 0x08;
    /// Fixed width with wrapping (layout flag `0x40`).
    pub const WRAP: u32 = 0x10;
    /// Keep the given height (otherwise the text height).
    pub const KEEP_HEIGHT: u32 = 0x20;
    /// Load `.bkg` (`FUN_0045BB00`).
    pub const BKG: u32 = 0x40;
    /// Position (0, 0) (sign of the low byte).
    pub const ORIGIN: u32 = 0x80;
    /// Position (0, 0).
    pub const ORIGIN_100: u32 = 0x100;
}

/// Kind of an info window, as the script presentation reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoFrameKind {
    /// 168 `{text, x, y, flag}`: `style` is the flag (1 right edge, 2 x = 0, any other like 0).
    Window { x: i32, y: i32, style: i32 },
    /// 213 `{text, pos, portrait, frame}`: `slot` is the raw pos (bit 1 of the style only with pos 1, E-248).
    Speech {
        slot: i32,
        portrait: i32,
        frame: i32,
    },
}

/// Parameter block of the widget (16 words after `+0x94`, `FUN_0045DBB0`/`FUN_00439800`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InfoFrameParams {
    /// Style `+0x94` ([`style`]).
    pub style: u32,
    /// Start rectangle in the 1280 x 1024 grid.
    pub rect: [i32; 4],
    /// `.bkg` path `+0xA8`.
    pub bkg: &'static str,
    /// Text ID `+0xB0`.
    pub text: i32,
    /// Portrait ID `+0xC4`.
    pub portrait: i32,
    /// Frame ID `+0xC8`.
    pub frame: i32,
    /// Portrait size `+0xCC`/`+0xD0` in the grid.
    pub portrait_size: [i32; 2],
}

impl InfoFrameParams {
    /// The parameters of the actions:
    /// - 168 (`0x004434E0`): style `0x40`, with flag 1 `0x41`, with flag 2 `0x42`, any other flag `0x40`
    ///   (`0x0044355B...0x00443574`); rectangle `(x, y, x + 100, y + 100)`, [`INFO_FRAME_BKG`].
    /// - 213 (`0x00443A20`): style `0x5C`, with pos 1 `0x5D` (`0x00443AAC...0x00443AB6`); rectangle 0/40/600/300,
    ///   portrait 100 x 100, [`SPEECH_FRAME_BKG`].
    pub fn of(kind: &InfoFrameKind, text: i32) -> Self {
        match *kind {
            InfoFrameKind::Window { x, y, style: flag } => Self {
                style: style::BKG
                    | match flag {
                        1 => style::RIGHT,
                        2 => style::LEFT,
                        _ => 0,
                    },
                rect: [
                    x,
                    y,
                    x.wrapping_add(WINDOW_START),
                    y.wrapping_add(WINDOW_START),
                ],
                bkg: INFO_FRAME_BKG,
                text,
                portrait: 0,
                frame: 0,
                portrait_size: [0, 0],
            },
            InfoFrameKind::Speech {
                slot,
                portrait,
                frame,
            } => Self {
                style: SPEECH_STYLE | u32::from(slot == 1),
                rect: SPEECH_RECT,
                bkg: SPEECH_FRAME_BKG,
                text,
                portrait,
                frame,
                portrait_size: SPEECH_PORTRAIT,
            },
        }
    }

    /// With portrait or frame (style `0x0C`).
    fn has_portrait(&self) -> bool {
        self.style & (style::PORTRAIT | style::PORTRAIT_FRAME) != 0
    }

    /// Layout flags: `+0xAC` = 7, `OnAttach` `0x0045DCE0` sets `0x40` with style `0x10` and clears it otherwise. Bit
    /// `0x01` has no effect in the layout (O-377), the reimplementation sets `0x06` or `0x46`.
    fn layout_flags(&self) -> u32 {
        let wrap = if self.style & style::WRAP != 0 {
            layout_flags::WIDTH
        } else {
            0
        };
        layout_flags::START_FONT | layout_flags::START_COLOR | wrap
    }

    /// Layout of the text with width `width` in pixels (`FUN_0055FD00(0)`, `FUN_00562770`): font `InfoFrame`, white.
    /// The text comes via `FUN_0055FFE0`: an unknown ID sets its own number (`"%d"`, O-241).
    fn layout(&self, ctx: &UiContext<'_>, game: &GameTags<'_>, width: i32) -> TextLayout {
        let text = ctx
            .texts
            .get(self.text)
            .map(str::to_string)
            .unwrap_or_else(|| self.text.to_string());
        let params = LayoutParams {
            flags: self.layout_flags(),
            width,
            font: ctx.fonts.by_name(INFO_FRAME_FONT),
            color: WHITE,
        };
        let mut layout = TextLayout::default();
        layout_text_game(&text, &params, ctx.fonts, &mut layout, None, Some(game));
        layout
    }
}

/// An attached info window: parameters, fixed rectangle in the grid and the loaded background.
#[derive(Debug, Clone)]
pub struct InfoFrame {
    pub params: InfoFrameParams,
    /// Rectangle in the grid after `OnAttach`; fixed afterwards (O-358).
    pub rect: [i32; 4],
    /// `.bkg` from the constructor (style `0x40`, `FUN_0045BB00`); `None` = not loaded (`+0x90` = 0).
    bkg: Option<LoadedBkg>,
}

impl InfoFrame {
    /// Attaching (`OnAttach` `0x0045DCE0`): the size is determined once from the text, the conversion between grid and
    /// pixels rounds (`MenuWidget_VirtualToScreenPoint`, `FUN_00459610`):
    /// 1. `w`, `h` from the start rectangle; with a portrait `w -= portrait width`. Layout with width `S(w)`.
    /// 2. Width: without `0x10` the text width, otherwise `S(w)`; height: without `0x20` the text height, otherwise
    ///    `S(h)`; both back into the grid.
    /// 3. With a portrait: width `+=` portrait width, height at least portrait height.
    /// 4. With a loaded `.bkg`: width `+=` `CLIENTAREA` left + right, height `+=` top + bottom.
    /// 5. Position: `0x01` -> x = 1280 - width, `0x02` -> x = 0, `0x80`/`0x100` -> (0, 0), otherwise the corner of the
    ///    start rectangle.
    ///
    /// Result: 168 = text + 12 without wrapping (O-358), 213 = 630 wide, `max(text height, 100) + 60` high, y = 40,
    /// x = 0 or 650 (O-359).
    pub fn attach(params: InfoFrameParams, ctx: &mut UiContext<'_>, game: &GameTags<'_>) -> Self {
        let s = ctx.screen;
        let bkg = if params.style & style::BKG != 0 {
            load_bkg(ctx.vfs, params.bkg, ctx.assets)
        } else {
            None
        };
        let st = params.style;
        let [x0, y0, x1, y1] = params.rect;
        let [portrait_w, portrait_h] = params.portrait_size;
        let mut w = x1.wrapping_sub(x0);
        let h = y1.wrapping_sub(y0);
        if params.has_portrait() {
            w = w.wrapping_sub(portrait_w);
        }
        let (ws, hs) = (to_screen_x(s, w), to_screen_y(s, h));
        let layout = params.layout(ctx, game, ws);
        let mut width = to_virtual_x(
            s,
            if st & style::WRAP != 0 {
                ws
            } else {
                layout.width
            },
        );
        let mut height = to_virtual_y(
            s,
            if st & style::KEEP_HEIGHT != 0 {
                hs
            } else {
                layout.height
            },
        );
        if params.has_portrait() {
            width = width.wrapping_add(portrait_w);
            height = height.max(portrait_h);
        }
        if let Some(bkg) = &bkg {
            let [l, t, r, b] = bkg.desc.client_area;
            width = width.wrapping_add(r).wrapping_add(l);
            height = height.wrapping_add(b).wrapping_add(t);
        }
        let (mut x, mut y) = (x0, y0);
        if st & style::RIGHT != 0 {
            x = VIRTUAL_W.wrapping_sub(width);
        }
        if st & style::LEFT != 0 {
            x = 0;
        }
        if st & (style::ORIGIN | style::ORIGIN_100) != 0 {
            (x, y) = (0, 0);
        }
        Self {
            params,
            rect: [x, y, x.wrapping_add(width), y.wrapping_add(height)],
            bkg,
        }
    }

    /// `CLIENTAREA` of the loaded background.
    fn client_area(&self) -> Option<[i32; 4]> {
        self.bkg.as_ref().map(|b| b.desc.client_area)
    }

    /// Drawing (read-only):
    /// 1. Background with `DrawBackground` `FUN_0045A570` ([`paint_bkg`]): only the nine pieces.
    /// 2. `Draw` `0x0045D980`: inner rectangle (`FUN_00459AC0`); with a portrait (`0x04`/`0x08`) first the frame, then
    ///    the portrait at the top left of the inner rectangle (portrait size), both white.
    /// 3. The text to the right of it, width = inner width - portrait width, font `InfoFrame`, white, without a shadow
    ///    (`FUN_0055FD60(&pos, 0, 0)`), without clipping at the window (D6.17). The layout runs anew every frame:
    ///    counters are live, the window does not grow with them (O-358, O-377).
    pub fn draw(
        &self,
        data: &CinemascopeData,
        ctx: &mut UiContext<'_>,
        game: &GameTags<'_>,
        out: &mut Vec<DrawCmd>,
    ) {
        let s = ctx.screen;
        if let Some(bkg) = &self.bkg {
            // Visible rectangle on the desktop: its own intersected with the screen (only for the tile sequence).
            let clip = intersect(to_screen_rect(s, self.rect), [0, 0, s.width, s.height])
                .unwrap_or([0; 4]);
            paint_bkg(bkg, self.rect, clip, s, ctx.assets, out);
        }
        let [x, y, ..] = self.rect;
        let c = client_rel(self.rect, self.client_area());
        let client = [
            x.wrapping_add(c[0]),
            y.wrapping_add(c[1]),
            x.wrapping_add(c[2]),
            y.wrapping_add(c[3]),
        ];
        let mut w = client[2].wrapping_sub(client[0]);
        let mut left = client[0];
        if self.params.has_portrait() {
            let [pw, ph] = self.params.portrait_size;
            w = w.wrapping_sub(pw);
            left = client[0].wrapping_add(pw);
            let rect = to_screen_rect(
                s,
                [
                    client[0],
                    client[1],
                    client[0].wrapping_add(pw),
                    client[1].wrapping_add(ph),
                ],
            );
            if self.params.style & style::PORTRAIT_FRAME != 0 {
                draw_portrait(ctx, &data.frames, self.params.frame, rect, WHITE, out);
            }
            if self.params.style & style::PORTRAIT != 0 {
                draw_portrait(ctx, &data.portraits, self.params.portrait, rect, WHITE, out);
            }
        }
        let layout = self.params.layout(ctx, game, to_screen_x(s, w));
        let origin = [to_screen_x(s, left), to_screen_y(s, client[1])];
        draw_runs(&layout, origin, false, None, ctx.fonts, out);
    }
}

/// An open info window: text ID as key, parameters and the widget attached at the first drawing.
#[derive(Debug, Clone)]
struct OpenFrame {
    text: i32,
    params: InfoFrameParams,
    /// Measured only at the first drawing, fixed afterwards (deviation: the original measures when attaching,
    /// re/campaign-ui D6.5, section "Implementation").
    frame: OnceCell<InfoFrame>,
}

/// The open info windows in the order of the desktop children: head first.
///
/// Measured windows hold texture IDs of the `UiAssets` with which they were drawn: the list lives with the same UI
/// layer as the rest of the UI; if the layer is rebuilt, the list must be opened again.
#[derive(Debug, Clone, Default)]
pub struct InfoFrames {
    frames: Vec<OpenFrame>,
}

impl InfoFrames {
    /// No open windows.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the window `text`; an open one with the same ID closes first (`FUN_004401C0` as the first act of 168, 213
    /// also closes beforehand). It is attached at the **head** (`Widget_AddChild` `0x0045BA50` with `AddHead`, O-360).
    /// The size arises at the first drawing; equivalent to the original only if the host draws in the frame of opening
    /// (`GameSession::desktop_cmds` in every frame with a match). Measured later with a minimized window, with `step`
    /// with several steps before a frame and in the viewer without `--hud` (re/campaign-ui D6.5, section
    /// "Implementation in the host").
    pub fn open(&mut self, text: i32, params: InfoFrameParams) {
        self.close(text);
        self.frames.insert(
            0,
            OpenFrame {
                text,
                params,
                frame: OnceCell::new(),
            },
        );
    }

    /// Closes the window `text` immediately, without a fade (`FUN_004401C0` -> `FUN_0045A3C0`); it ignores unknown IDs.
    pub fn close(&mut self, text: i32) {
        self.frames.retain(|f| f.text != text);
    }

    /// Draws from head to end: a newer window lies below an older one (O-360). A window not yet measured attaches
    /// itself while doing so ([`InfoFrame::attach`]).
    pub fn draw(
        &self,
        data: &CinemascopeData,
        ctx: &mut UiContext<'_>,
        game: &GameTags<'_>,
        out: &mut Vec<DrawCmd>,
    ) {
        for open in &self.frames {
            let frame = open
                .frame
                .get_or_init(|| InfoFrame::attach(open.params, ctx, game));
            frame.draw(data, ctx, game, out);
        }
    }

    /// Text ID and rectangle (grid) of the windows already measured, head first.
    pub fn rects(&self) -> Vec<(i32, [i32; 4])> {
        self.frames
            .iter()
            .filter_map(|f| f.frame.get().map(|frame| (f.text, frame.rect)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use opennml_formats::localisation::parse_localisation;

    use super::*;
    use crate::assets::{Texts, UiAssets};
    use crate::coords::Screen;
    use crate::fonts::FontBook;
    use crate::testutil::{TestVfs, fnt_text, test_vfs, tga_bottom_up};
    use crate::text::{GameText, TradeCost};

    const TOOLTIP_TGA: &str = "data/gifcon/tooltip.tga";
    const MENUFRAME_TGA: &str = "data/menue/frame.tga";
    const PORTRAIT_TGA: &str = "data/portraits/p1000.tga";
    const FRAME_TGA: &str = "data/portraits/f101.tga";

    /// Texts: 20002 short (10 characters = 100 px), 30000 with a live counter, 30001 long (wraps at 500 px).
    fn texts() -> Texts {
        let long = "AAAA ".repeat(80);
        Texts::from_entries(&[
            (20002, "Hello all!"),
            (30000, "[GUIDCOUNTER 0 5]"),
            (30001, long.trim_end()),
        ])
    }

    /// Game values: only the object counter (`guid_count`) is live.
    struct Values {
        counter: Cell<i32>,
    }

    impl GameText for Values {
        fn player_name(&self, _: i32) -> String {
            String::new()
        }
        fn trade_cost(&self, _: TradeCost) -> i32 {
            0
        }
        fn guid_count(&self, _: i32, _: i32) -> i32 {
            self.counter.get()
        }
        fn kill_count(&self, _: i32, _: i32) -> i32 {
            0
        }
        fn task_status(&self, _: i32) -> Option<bool> {
            None
        }
        fn countdown_seconds(&self, _: i32) -> i32 {
            0
        }
    }

    /// 9-slice frame from a file with `CLIENTAREA`.
    fn nine(file: &str, tile: i32, client: [i32; 4]) -> Vec<u8> {
        let [l, t, r, b] = client;
        format!(
            "FILE {file} {tile} {tile}\r\nTOPLEFT 0 0\r\nTOPMID 0 1\r\nTOPRIGHT 0 2\r\nLEFTMID 0 3\r\n\
             RIGHTMID 0 5\r\nBOTTOMLEFT 0 6\r\nBOTTOMMID 0 7\r\nBOTTOMRIGHT 0 8\r\nFILLTILE 0 4\r\n\
             CLIENTAREA {l} {t} {r} {b}\r\n"
        )
        .into_bytes()
    }

    struct Rig {
        vfs: TestVfs,
        assets: UiAssets,
        texts: Texts,
        fonts: FontBook,
        data: CinemascopeData,
        values: Values,
        screen: Screen,
    }

    /// Font `InfoFrame`: 10 px advance (also the space), line height 15; screen 1280 x 1024 (grid = pixels).
    fn rig() -> Rig {
        let white = |w, h| tga_bottom_up(w, h, |_, _| [255; 4]);
        let vfs = test_vfs(&[
            (
                "data/fonts/Test_15_0_0_0_0.fnt",
                fnt_text(|_| 10, |_| 15).into_bytes(),
            ),
            ("data/fonts/Test_15_0_0_0_0.tga", white(256, 256)),
            (INFO_FRAME_BKG, nine(TOOLTIP_TGA, 16, [6, 6, 6, 6])),
            (TOOLTIP_TGA, white(48, 48)),
            (SPEECH_FRAME_BKG, nine(MENUFRAME_TGA, 32, [15, 30, 15, 30])),
            (MENUFRAME_TGA, white(96, 96)),
            (PORTRAIT_TGA, white(64, 64)),
            (FRAME_TGA, white(64, 64)),
        ]);
        let mut assets = UiAssets::new();
        let loc = parse_localisation("Font InfoFrame\nFontSize 15\nTextureFont\nFontName Test\n");
        let fonts = FontBook::load(&vfs, &loc, 1280, &mut assets, None);
        Rig {
            vfs,
            assets,
            texts: texts(),
            fonts,
            data: CinemascopeData::from_texts(
                "",
                "1000=Hero=data/portraits/p1000.tga\r\n",
                "101=Frame=data/portraits/f101.tga\r\n",
            ),
            values: Values {
                counter: Cell::new(3),
            },
            screen: Screen {
                width: 1280,
                height: 1024,
            },
        }
    }

    impl Rig {
        fn ctx(&mut self) -> UiContext<'_> {
            UiContext {
                vfs: &self.vfs,
                assets: &mut self.assets,
                fonts: &self.fonts,
                texts: &self.texts,
                screen: self.screen,
            }
        }

        fn attach(&mut self, kind: InfoFrameKind, text: i32) -> InfoFrame {
            let values = Values {
                counter: Cell::new(self.values.counter.get()),
            };
            let tags = GameTags::new(&values, 0);
            InfoFrame::attach(InfoFrameParams::of(&kind, text), &mut self.ctx(), &tags)
        }

        fn draw_frame(&mut self, frame: &InfoFrame) -> Vec<DrawCmd> {
            let values = Values {
                counter: Cell::new(self.values.counter.get()),
            };
            let tags = GameTags::new(&values, 0);
            let data = self.data.clone();
            let mut out = Vec::new();
            frame.draw(&data, &mut self.ctx(), &tags, &mut out);
            out
        }

        fn draw_all(&mut self, frames: &InfoFrames) -> Vec<DrawCmd> {
            let values = Values {
                counter: Cell::new(self.values.counter.get()),
            };
            let tags = GameTags::new(&values, 0);
            let data = self.data.clone();
            let mut out = Vec::new();
            frames.draw(&data, &mut self.ctx(), &tags, &mut out);
            out
        }

        /// Layout like the window: flags `flags`, font `InfoFrame`, white.
        fn layout(&self, text: &str, flags: u32, width: i32) -> TextLayout {
            let params = LayoutParams {
                flags,
                width,
                font: self.fonts.by_name(INFO_FRAME_FONT),
                color: WHITE,
            };
            let tags = GameTags::new(&self.values, 0);
            let mut layout = TextLayout::default();
            layout_text_game(text, &params, &self.fonts, &mut layout, None, Some(&tags));
            layout
        }

        fn path(&self, cmd: &DrawCmd) -> Option<String> {
            let DrawCmd::Quad { texture, .. } = cmd else {
                unreachable!()
            };
            texture.map(|t| self.assets.path(t).to_string())
        }

        /// Glyphs of the font `InfoFrame` (commands with their atlas).
        fn glyphs<'a>(&self, cmds: &'a [DrawCmd]) -> Vec<&'a DrawCmd> {
            let atlas = self.fonts.font(self.fonts.by_name(INFO_FRAME_FONT)).atlas;
            cmds.iter()
                .filter(|c| matches!(c, DrawCmd::Quad { texture, .. } if *texture == Some(atlas)))
                .collect()
        }

        /// Commands with the texture `path`.
        fn with_texture<'a>(&self, cmds: &'a [DrawCmd], path: &str) -> Vec<&'a DrawCmd> {
            cmds.iter()
                .filter(|c| self.path(c).as_deref() == Some(path))
                .collect()
        }
    }

    fn dst(cmd: &DrawCmd) -> [i32; 4] {
        let DrawCmd::Quad { dst, .. } = *cmd else {
            unreachable!()
        };
        dst
    }

    const fn window(x: i32, y: i32, flag: i32) -> InfoFrameKind {
        InfoFrameKind::Window { x, y, style: flag }
    }

    const fn speech(slot: i32) -> InfoFrameKind {
        InfoFrameKind::Speech {
            slot,
            portrait: 1000,
            frame: 101,
        }
    }

    // QUIRK E-248: kept
    #[test]
    fn params_follow_the_actions() {
        // 168: style from the flag, start rectangle (x, y, x + 100, y + 100).
        let p = InfoFrameParams::of(&window(1250, 35, 1), 20950);
        assert_eq!((p.style, p.rect), (0x41, [1250, 35, 1350, 135]));
        assert_eq!((p.text, p.portrait_size), (20950, [0, 0]));
        assert_eq!(InfoFrameParams::of(&window(0, 0, 2), 1).style, 0x42);
        assert_eq!(InfoFrameParams::of(&window(0, 0, 0), 1).style, 0x40);
        // 213: 0x5C, with pos 1 0x5D (E-248); rectangle 0/40/600/300, portrait 100 x 100.
        let p = InfoFrameParams::of(&speech(1), 20002);
        assert_eq!((p.style, p.rect), (0x5D, [0, 40, 600, 300]));
        assert_eq!(
            (p.portrait, p.frame, p.portrait_size),
            (1000, 101, [100, 100])
        );
        for slot in [0, 2, 3, -1] {
            assert_eq!(InfoFrameParams::of(&speech(slot), 1).style, 0x5C, "{slot}");
        }
    }

    // QUIRK O-358: kept
    #[test]
    fn frame_168_unknown_flag_is_plain_0x40() {
        // `0x0044355B...0x00443574`: only flags 1 and 2 set a bit; flag 3 acts like flag 0.
        for flag in [3, -1, 7] {
            assert_eq!(
                InfoFrameParams::of(&window(0, 0, flag), 1).style,
                0x40,
                "{flag}"
            );
        }
        let mut rig = rig();
        let three = rig.attach(window(1250, 35, 3), 20002);
        let zero = rig.attach(window(1250, 35, 0), 20002);
        assert_eq!(three.rect, zero.rect);
        assert_eq!(three.rect, [1250, 35, 1250 + 112, 35 + 27]);
    }

    // QUIRK E-248, O-359: kept
    #[test]
    fn frame_213_at_pos_1_is_630_wide_at_x_650_y_40() {
        let mut rig = rig();
        let right = rig.attach(speech(1), 20002);
        assert_eq!(right.rect, [650, 40, 1280, 200]);
        let left = rig.attach(speech(0), 20002);
        assert_eq!(left.rect, [0, 40, 630, 200]);
        // The conversion rounds: at 1024 x 768 the same width (S(500) = 400, back 500).
        rig.screen = Screen {
            width: 1024,
            height: 768,
        };
        assert_eq!(rig.attach(speech(1), 20002).rect[..3], [650, 40, 1280]);
    }

    // QUIRK O-359: kept
    #[test]
    fn frame_213_height_is_max_of_text_and_100_plus_60() {
        let mut rig = rig();
        // One line (15) -> 100 + 60.
        assert_eq!(rig.attach(speech(0), 20002).rect[3] - 40, 160);
        // Long text with wrapping at 500: text height above 100.
        let text_h = rig.layout(rig.texts.get(30001).unwrap(), 0x46, 500).height;
        assert!(text_h > 100, "{text_h}");
        let long = rig.attach(speech(0), 30001);
        assert_eq!(long.rect, [0, 40, 630, 40 + text_h + 60]);
    }

    // QUIRK O-358: kept
    #[test]
    fn frame_168_flag_1_sits_at_the_right_edge() {
        let mut rig = rig();
        // "Hello all!" = 100 px, + 12 for `CLIENTAREA 6 6 6 6`; height 15 + 12.
        assert_eq!(rig.layout("Hello all!", 0x06, 100).width, 100);
        let f = rig.attach(window(1250, 35, 1), 20002);
        assert_eq!(f.rect, [1280 - 112, 35, 1280, 62]);
        // Grid and pixels round: at 1024 x 768, 100 px are 125 grid units (125.5 toward 0), height 15 px -> 20.
        rig.screen = Screen {
            width: 1024,
            height: 768,
        };
        let f = rig.attach(window(1250, 35, 1), 20002);
        assert_eq!(f.rect, [1280 - 137, 35, 1280, 35 + 32]);
    }

    // QUIRK O-358: kept
    #[test]
    fn frame_168_flag_2_sits_at_x_0() {
        let mut rig = rig();
        let f = rig.attach(window(900, 35, 2), 20002);
        assert_eq!(f.rect, [0, 35, 112, 62]);
    }

    #[test]
    fn unknown_text_id_draws_its_own_number() {
        // `FUN_00562770` -> `FUN_0055FFE0`: without an entry `swprintf("%d")` of its own number (O-241).
        let mut rig = rig();
        let f = rig.attach(window(100, 35, 0), 4711);
        assert_eq!(f.rect, [100, 35, 100 + 40 + 12, 62]);
        let cmds = rig.draw_frame(&f);
        assert_eq!(rig.glyphs(&cmds).len(), 4);
    }

    // QUIRK O-358: kept
    #[test]
    fn size_is_fixed_when_the_counter_grows_and_text_overflows() {
        // O-358: measured once with "3"; if the counter grows, the window stays, the text overflows.
        let mut rig = rig();
        let mut frames = InfoFrames::new();
        frames.open(30000, InfoFrameParams::of(&window(100, 35, 0), 30000));
        assert!(
            frames.rects().is_empty(),
            "unmeasured before the first drawing"
        );
        let first = rig.draw_all(&frames);
        assert_eq!(frames.rects(), [(30000, [100, 35, 122, 62])]);
        let glyphs = rig.glyphs(&first);
        assert_eq!(glyphs.len(), 1);
        assert_eq!(dst(glyphs[0])[0], 106);
        rig.values.counter.set(1_234_567);
        let later = rig.draw_all(&frames);
        assert_eq!(frames.rects(), [(30000, [100, 35, 122, 62])]);
        let glyphs = rig.glyphs(&later);
        assert_eq!(glyphs.len(), 7, "layout anew in every frame");
        // No clipping at the window: the last digit ends far to the right of the edge 122.
        assert!(dst(glyphs[6])[0] >= 106 + 60, "{:?}", dst(glyphs[6]));
        // The background stays the same.
        assert_eq!(
            rig.with_texture(&first, TOOLTIP_TGA),
            rig.with_texture(&later, TOOLTIP_TGA)
        );
    }

    // QUIRK O-360: kept
    #[test]
    fn newer_frame_lies_below_older() {
        // O-360: `open` attaches at the head, `draw` draws from the head: the newer window comes first.
        let mut rig = rig();
        let mut frames = InfoFrames::new();
        frames.open(20002, InfoFrameParams::of(&window(100, 35, 0), 20002));
        frames.open(30000, InfoFrameParams::of(&window(110, 40, 0), 30000));
        let cmds = rig.draw_all(&frames);
        let older = rig.attach(window(100, 35, 0), 20002);
        let newer = rig.attach(window(110, 40, 0), 30000);
        let mut expected = rig.draw_frame(&newer);
        expected.extend(rig.draw_frame(&older));
        assert_eq!(cmds, expected);
        assert_eq!(frames.rects(), [(30000, newer.rect), (20002, older.rect)]);
        // The same ID replaces (`FUN_004401C0` before attaching) and moves to the head.
        frames.open(20002, InfoFrameParams::of(&window(500, 35, 0), 20002));
        assert_eq!(frames.rects(), [(30000, newer.rect)]);
        rig.draw_all(&frames);
        let ids: Vec<i32> = frames.rects().iter().map(|&(t, _)| t).collect();
        assert_eq!(ids, [20002, 30000]);
        assert_eq!(frames.rects()[0].1[0], 500);
        // Closing removes immediately; an unknown ID changes nothing.
        frames.close(999);
        frames.close(30000);
        assert_eq!(frames.rects().len(), 1);
        frames.close(20002);
        assert!(rig.draw_all(&frames).is_empty());
    }

    // QUIRK O-359, O-377: kept
    #[test]
    fn frame_before_portrait_and_text_right_of_it_without_shadow() {
        let mut rig = rig();
        let f = rig.attach(speech(0), 20002);
        let cmds = rig.draw_frame(&f);
        // Inner rectangle: (0 + 15, 40 + 30) ... (630 - 15, 200 - 30); portrait 100 x 100 at the top left.
        let frame = rig.with_texture(&cmds, FRAME_TGA);
        let portrait = rig.with_texture(&cmds, PORTRAIT_TGA);
        assert_eq!((frame.len(), portrait.len()), (1, 1));
        assert_eq!(dst(frame[0]), [15, 70, 115, 170]);
        assert_eq!(dst(portrait[0]), [15, 70, 115, 170]);
        let index = |c: &DrawCmd| cmds.iter().position(|x| x == c).unwrap();
        assert!(index(frame[0]) < index(portrait[0]), "frame first");
        let DrawCmd::Quad { color, .. } = *portrait[0] else {
            unreachable!()
        };
        assert_eq!(color, WHITE);
        // The text to the right of it: layout `0x46` with width 600 - 100 = 500 at (115, 70), without a shadow; it
        // comes last.
        let layout = rig.layout("Hello all!", 0x46, 500);
        let mut text = Vec::new();
        draw_runs(&layout, [115, 70], false, None, &rig.fonts, &mut text);
        assert!(!text.is_empty());
        assert_eq!(&cmds[cmds.len() - text.len()..], text.as_slice());
        assert!(index(portrait[0]) < cmds.len() - text.len());
        // The background first.
        assert_eq!(rig.path(&cmds[0]).as_deref(), Some(MENUFRAME_TGA));
        // 168 without a portrait: text at the inner rectangle (106, 41), flags `0x06`.
        let f = rig.attach(window(100, 35, 0), 20002);
        let cmds = rig.draw_frame(&f);
        let layout = rig.layout("Hello all!", 0x06, 100);
        let mut text = Vec::new();
        draw_runs(&layout, [106, 41], false, None, &rig.fonts, &mut text);
        assert_eq!(&cmds[cmds.len() - text.len()..], text.as_slice());
        assert!(rig.with_texture(&cmds, PORTRAIT_TGA).is_empty());
    }

    // QUIRK O-359: kept
    #[test]
    fn bkg_is_infoframe_for_168_and_menuframe_for_213() {
        assert_eq!(InfoFrameParams::of(&window(0, 0, 0), 1).bkg, INFO_FRAME_BKG);
        assert_eq!(InfoFrameParams::of(&speech(0), 1).bkg, SPEECH_FRAME_BKG);
        let mut rig = rig();
        let f = rig.attach(window(100, 35, 0), 20002);
        let cmds = rig.draw_frame(&f);
        let tooltip = rig.with_texture(&cmds, TOOLTIP_TGA);
        // Only the nine pieces in the rectangle (100, 35)-(212, 62), the corners at the corners.
        assert!(!tooltip.is_empty());
        assert!(rig.with_texture(&cmds, MENUFRAME_TGA).is_empty());
        assert_eq!(dst(tooltip[2]), [100, 35, 116, 51], "TOPLEFT");
        let f = rig.attach(speech(1), 20002);
        let cmds = rig.draw_frame(&f);
        assert!(rig.with_texture(&cmds, TOOLTIP_TGA).is_empty());
        let menu = rig.with_texture(&cmds, MENUFRAME_TGA);
        assert_eq!(dst(menu[2]), [650, 40, 682, 72], "TOPLEFT");
    }
}
