//! Screen texts of the message bar: the list `MessageBar+0xC` with texts that fade out linearly over their whole
//! duration. Script action `n5` enqueues a text of kind 2 (unused in the shipped data); in the original the
//! headhunters also use the list (kind 1, O-380, in the reimplementation still `Notices`). Without simulation: the
//! host enqueues, ticks with the game time of the message bar and draws with the world.
//!
//! Chat lines as kind 10 (`FUN_00492B00` -> `FUN_00492340`) with their own text, own position and movement, fade-out
//! from 1024, timer `duration/6` and wait list (`MessageBar+0x24/+0x28`, `FUN_00492960(..., 1)`, moving up
//! `FUN_004929F0`). Kinds 0/1/2 stay unchanged.
//!
//! Basis: re/campaign-ui D6.12 (enqueue `FUN_00492A40` -> `FUN_00492450`, `FUN_00492960`; tick `FUN_00494A00` ->
//! `FUN_004925D0`; drawing `FUN_00493B80` -> `FUN_004926D0` -> `FUN_00492F20`); quirks O-373 and O-380.

use super::SCREEN_TEXT_FONT;
use crate::dialog::UiContext;
use crate::draw::DrawCmd;
use crate::text::{GameTags, LayoutParams, TextLayout, draw_runs, layout_flags, layout_text_game};

/// Kinds of the screen texts (`+4`, first argument of `FUN_00492A40`); they determine the position.
pub mod kind {
    /// 0: at (0, 32), left-aligned; several texts of this kind stand side by side in the list.
    pub const TOP: u8 = 0;
    /// 1: centered at y = H - 200 (headhunter, O-380).
    pub const LOW: u8 = 1;
    /// 2: at (0, H/2), left-aligned (`n5`).
    pub const MIDDLE: u8 = 2;
    /// 10: chat line (`FUN_00492B00`), with its own position and movement ([`super::ScreenTexts::push_chat`]). After
    /// the timer the original sets the kind to 0 (`FUN_004925D0`); the entry keeps its position.
    pub const CHAT: u8 = 10;
}

/// Chat lines (`FUN_00492B00`): start at x = 20 ...
pub const CHAT_X: i32 = 20;
/// ... and y = H - 200 (`g_pRenderer+0xA8` when enqueuing).
pub const CHAT_Y_FROM_BOTTOM: i32 = 200;
/// Velocity `(vx, vy)` in 1/65536 px per ms (`FUN_00492340(..., 0, -1500, ...)`, factor `1.5258789e-05`).
pub const CHAT_VELOCITY: [i32; 2] = [0, -1500];
/// Display duration of a chat line in ms (`FUN_0042E980`).
pub const CHAT_MS: i32 = 8000;
/// Fade-out of kind 10: start alpha `1024.0` and step `(1 / duration) * 1280.0` (`FUN_00492340`).
const CHAT_ALPHA: f32 = 1024.0;
const CHAT_FADE: f32 = 1280.0;
/// `1/65536` as `float` (`0x37800000`).
const PER_65536: f32 = 1.525_878_9e-5;

/// Position and movement of a screen text with its own position (kind 10): start (`+0xC`), velocity (`+0x24`) and
/// position of the last tick (`+0x18` = start + elapsed * velocity).
#[derive(Debug, Clone, PartialEq)]
pub struct Motion {
    pub start: [f32; 2],
    pub velocity: [f32; 2],
    pub pos: [f32; 2],
}

/// Layout flags (`0xC6`, `FUN_00492F20`): start font, start color, width, truncate without wrapping; for kind 1
/// plus [`layout_flags::CENTER`].
pub const LAYOUT_FLAGS: u32 = layout_flags::START_FONT
    | layout_flags::START_COLOR
    | layout_flags::WIDTH
    | layout_flags::TRUNCATE;

/// A screen text (`FUN_00492450`, `0x58` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenText {
    /// Kind (`+4`), see [`kind`].
    pub kind: u8,
    /// Text ID; the original copies the text when enqueuing (`FUN_0055FFE0`), the reimplementation reads it when
    /// drawing from the same table.
    pub text: i32,
    /// Base color (`+0x34`); the alpha is set by the tick.
    pub color: u32,
    /// Duration in ms of game time (`+0x4C`).
    pub ms: i32,
    /// Elapsed game time in ms (`+0x44`).
    pub elapsed: i32,
    /// Flag "half time reached" (`+8`), set by the tick.
    pub half: bool,
    /// Opacity of the last tick (top byte of `+0x30`).
    pub alpha: u8,
    /// Fade step per ms (`+0x38 = 255.0 / (float)ms`).
    step: f32,
    /// Start alpha (`+0x3C`): 255, 1024 for kind 10.
    start_alpha: f32,
    /// Own text (`+0x40`, copy when enqueuing): chat lines; otherwise `None` and the text comes from [`Self::text`].
    pub own_text: Option<String>,
    /// Own position and movement (kind 10); otherwise `None` and the position follows the kind when drawing.
    pub motion: Option<Motion>,
    /// Timer (`+0x48`, active with flag bit 4 of `+0x54`): `duration/6` for kind 10, none otherwise.
    timer: Option<i32>,
}

impl ScreenText {
    /// Setup `FUN_00492450`: elapsed 0, flag off, step `255 / ms`. The original does not set the alpha (`+0x30`) in
    /// the setup; for `ms <= 0` it would stay undetermined after the initial tick, the reimplementation takes 0
    /// (invisible; the text ends with the next tick anyway).
    fn new(kind: u8, text: i32, color: u32, ms: i32) -> Self {
        Self {
            kind,
            text,
            color,
            ms,
            elapsed: 0,
            half: false,
            alpha: 0,
            step: 255.0 / ms as f32,
            start_alpha: 255.0,
            own_text: None,
            motion: None,
            timer: None,
        }
    }

    /// Setup `FUN_00492340(10, x, y, 0, vy, color, text, duration, duration/6)` from `FUN_00492B00`: position (`+0xC`
    /// and `+0x18`) = `(x, y)`, velocity `v * 1/65536`, start alpha 1024 and step `(1 / duration) * 1280`, timer
    /// `duration / 6` (integer division), flags 6 (screen text with timer).
    fn chat(text: String, color: u32, ms: i32, at: [i32; 2]) -> Self {
        let start = [at[0] as f32, at[1] as f32];
        Self {
            kind: kind::CHAT,
            text: 0,
            color,
            ms,
            elapsed: 0,
            half: false,
            alpha: 0,
            step: (1.0 / ms as f32) * CHAT_FADE,
            start_alpha: CHAT_ALPHA,
            own_text: Some(text),
            motion: Some(Motion {
                start,
                velocity: CHAT_VELOCITY.map(|v| v as f32 * PER_65536),
                pos: start,
            }),
            timer: Some(ms / 6),
        }
    }

    /// Tick `FUN_004925D0(dt)`. First the timer (only with flag bit 4): `time -= dt`, at `time < 1` it is off, the
    /// kind becomes 0 and the old kind goes as return value to [`ScreenTexts::promote`] (`FUN_004929F0(old kind)`).
    /// Then `elapsed += dt`; from `elapsed >= ms` it is over (`false`, the entry goes). Otherwise it sets the flag
    /// "half" at `elapsed >= ms / 2` (integer division toward 0), the position `start + (float)elapsed * velocity`
    /// and the alpha `ftol(start alpha - elapsed * step)`, clamped to 0...255.
    fn tick(&mut self, dt: i32) -> (bool, Option<u8>) {
        let mut released = None;
        if let Some(t) = self.timer.as_mut() {
            *t = t.wrapping_sub(dt);
            if *t < 1 {
                self.timer = None;
                released = Some(std::mem::replace(&mut self.kind, 0));
            }
        }
        self.elapsed = self.elapsed.wrapping_add(dt);
        if self.elapsed >= self.ms {
            return (false, released);
        }
        if !self.half && self.elapsed >= self.ms / 2 {
            self.half = true;
        }
        if let Some(m) = self.motion.as_mut() {
            let t = self.elapsed as f32;
            m.pos = [
                m.start[0] + t * m.velocity[0],
                m.start[1] + t * m.velocity[1],
            ];
        }
        let alpha = (self.start_alpha - self.elapsed as f32 * self.step) as i32;
        self.alpha = alpha.clamp(0, 255) as u8;
        (true, released)
    }

    /// Draw color: `(color & 0xFFFFFF) | alpha << 24`.
    pub fn draw_color(&self) -> u32 {
        (self.color & 0x00FF_FFFF) | u32::from(self.alpha) << 24
    }
}

/// The list of screen texts in list order (`MessageBar+0xC`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScreenTexts {
    entries: Vec<ScreenText>,
    /// Wait list (`MessageBar+0x24/+0x28`): entries from `FUN_00492960(..., 1)` whose kind was already active.
    waiting: Vec<ScreenText>,
}

impl ScreenTexts {
    /// Empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue `FUN_00492A40` -> `FUN_00492960(new, wait = 0)`: kind 0 always appends. For kind != 0 the **first**
    /// entry of the same kind in the list counts: if it carries the flag "half" (from the last tick), the new one
    /// replaces it at the same place; otherwise the new one is rejected. Without an entry of the same kind it
    /// appends. Every inserted text gets an initial tick with `dt = 0` (`FUN_00492720`), so it stands at alpha 255.
    pub fn push(&mut self, kind: u8, text: i32, color: u32, ms: i32) {
        self.enqueue(ScreenText::new(kind, text, color, ms), false);
    }

    /// Chat line `FUN_00492B00(text, color, duration)` at the screen height `height`: entry of kind 10 at
    /// ([`CHAT_X`], `height` - [`CHAT_Y_FROM_BOTTOM`]) and `FUN_00492960(entry, 1)`. If a chat line is already active
    /// (kind 10, i.e. younger than `duration/6`), the new one waits; otherwise it stands immediately. The replacement
    /// branch ("half") never applies because the kind expires after `duration/6`.
    pub fn push_chat(&mut self, text: String, color: u32, ms: i32, height: i32) {
        let at = [CHAT_X, height.wrapping_sub(CHAT_Y_FROM_BOTTOM)];
        self.enqueue(ScreenText::chat(text, color, ms, at), true);
    }

    /// `FUN_00492960(new, wait)`: for kind != 0 the first active entry of the same kind counts: with the flag "half"
    /// the new one replaces it at the same place (initial tick `FUN_00492720`); otherwise the new one waits (`wait`,
    /// at the back of the wait list) or is rejected. Without such an entry it appends with an initial tick.
    fn enqueue(&mut self, mut new: ScreenText, wait: bool) {
        let same = (new.kind != kind::TOP)
            .then(|| self.entries.iter().position(|e| e.kind == new.kind))
            .flatten();
        match same {
            Some(i) if self.entries[i].half => {
                new.tick(0);
                self.entries[i] = new;
            }
            Some(_) if wait => self.waiting.push(new),
            Some(_) => {}
            None => {
                new.tick(0);
                self.entries.push(new);
            }
        }
    }

    /// `FUN_004929F0(kind)`: for kind != 0, moves the first waiting entry of this kind to the back of the active
    /// list, with an initial tick (`dt = 0`).
    fn promote(&mut self, kind: u8) {
        if kind == 0 {
            return;
        }
        if let Some(i) = self.waiting.iter().position(|e| e.kind == kind) {
            let mut e = self.waiting.remove(i);
            e.tick(0);
            self.entries.push(e);
        }
    }

    /// Tick `FUN_00494A00(dt)` with `dt` ms of game time of the message bar: tick every entry (waiting entries whose
    /// kind a timer frees move up in the process), remove expired ones and let the waiting entries of their kind move
    /// up. The pass follows the list of the original, which reads the successor **before** the tick: an entry that
    /// has moved up is still ticked in this tick, unless the trigger was the last of the list.
    pub fn tick(&mut self, dt: i32) {
        let mut i = 0;
        while i < self.entries.len() {
            let has_next = i + 1 < self.entries.len();
            let (alive, released) = self.entries[i].tick(dt);
            if let Some(k) = released {
                self.promote(k);
            }
            if alive {
                i += 1;
            } else {
                let gone = self.entries.remove(i);
                self.promote(gone.kind);
            }
            if !has_next {
                break;
            }
        }
    }

    /// The waiting entries in sequence (`MessageBar+0x24`).
    pub fn waiting(&self) -> &[ScreenText] {
        &self.waiting
    }

    /// The active chat lines (entries with their own text) in list order, for the control channel.
    pub fn chat_lines(&self) -> impl Iterator<Item = &ScreenText> {
        self.entries.iter().filter(|e| e.own_text.is_some())
    }

    /// The entries in list order.
    pub fn entries(&self) -> &[ScreenText] {
        &self.entries
    }

    /// Empty after loading or restart (like the message bar not in the savegame, re/game-ui N.7.4).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.waiting.clear();
    }

    /// Drawing `FUN_00493B80` (first part, before the messages and also in cinemascope) -> `FUN_004926D0` ->
    /// `FUN_00492F20(x, y, text, color)` per entry in list order:
    /// - position by kind (`FUN_00492A40`): 0 at (0, 32), 1 at (-1, H - 200), 2 at (0, H/2), otherwise like 0;
    /// - layout with [`LAYOUT_FLAGS`] in the font `InfoMessages` (`this+4` of the message bar) to the width
    ///   `W - x`; for `x < 0` additionally centered and `x = 0`;
    /// - with shadow (`FUN_0055FD60(&(x, y), 1, 0)`), color [`ScreenText::draw_color`].
    ///
    /// Text from the text table; if it is missing, the own number (`FUN_0055FFE0`); game tags via `game`. Deviation:
    /// the original computes the position when enqueuing with the screen height at that time, the reimplementation
    /// when drawing (a size change during a text shifts it).
    pub fn draw(&self, ctx: &UiContext<'_>, game: &GameTags<'_>, out: &mut Vec<DrawCmd>) {
        let s = ctx.screen;
        for entry in &self.entries {
            // Kind 10: the position of the last tick (`FUN_004926D0`: `ftol(+0x18)`, `ftol(+0x1C)`), computed when
            // enqueuing.
            let (x, y) = match (&entry.motion, entry.kind) {
                (Some(m), _) => (m.pos[0] as i32, m.pos[1] as i32),
                (None, kind::LOW) => (-1, s.height.wrapping_sub(200)),
                (None, kind::MIDDLE) => (0, s.height / 2),
                (None, _) => (0, 32),
            };
            let (x, flags) = if x < 0 {
                (0, LAYOUT_FLAGS | layout_flags::CENTER)
            } else {
                (x, LAYOUT_FLAGS)
            };
            let text = match &entry.own_text {
                Some(own) => own.clone(),
                None => ctx
                    .texts
                    .get(entry.text)
                    .map(str::to_string)
                    .unwrap_or_else(|| entry.text.to_string()),
            };
            let params = LayoutParams {
                flags,
                width: s.width.wrapping_sub(x),
                font: ctx.fonts.by_name(SCREEN_TEXT_FONT),
                color: entry.draw_color(),
            };
            let mut layout = TextLayout::default();
            layout_text_game(&text, &params, ctx.fonts, &mut layout, None, Some(game));
            draw_runs(&layout, [x, y], true, None, ctx.fonts, out);
        }
    }

    /// The first text of kind `kind` (for the control channel).
    pub fn first_of(&self, kind: u8) -> Option<&ScreenText> {
        self.entries.iter().find(|e| e.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use nml_formats::localisation::{Localisation, parse_localisation};

    use super::*;
    use crate::assets::{Texts, UiAssets};
    use crate::coords::Screen;
    use crate::fonts::FontBook;
    use crate::testutil::{fnt_text, test_vfs, tga_bottom_up};
    use crate::text::{GameText, TradeCost};

    /// No game values.
    struct NoValues;

    impl GameText for NoValues {
        fn player_name(&self, _: i32) -> String {
            String::new()
        }
        fn trade_cost(&self, _: TradeCost) -> i32 {
            0
        }
        fn guid_count(&self, _: i32, _: i32) -> i32 {
            0
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

    /// Font `InfoMessages`: every character 10 px wide, line height 15 (cell 16).
    fn fonts() -> &'static FontBook {
        static FONTS: OnceLock<FontBook> = OnceLock::new();
        FONTS.get_or_init(|| {
            let vfs = test_vfs(&[
                (
                    "data/fonts/Test_15_0_0_0_0.fnt",
                    fnt_text(|_| 10, |_| 15).into_bytes(),
                ),
                (
                    "data/fonts/Test_15_0_0_0_0.tga",
                    tga_bottom_up(256, 256, |_, _| [255, 255, 255, 255]),
                ),
            ]);
            let mut assets = UiAssets::new();
            let loc =
                parse_localisation("Font InfoMessages\nFontSize 15\nTextureFont\nFontName Test\n");
            FontBook::load(&vfs, &loc, 1024, &mut assets, None)
        })
    }

    const SCREEN: Screen = Screen {
        width: 1024,
        height: 768,
    };

    /// The font cells as `(x, y, color)`; texts without a table appear as their number.
    fn drawn(texts: &ScreenTexts) -> Vec<(i32, i32, u32)> {
        let vfs = test_vfs(&[]);
        let mut assets = UiAssets::new();
        let table = Texts::load(&vfs, &Localisation::default());
        let ctx = UiContext {
            vfs: &vfs,
            assets: &mut assets,
            fonts: fonts(),
            texts: &table,
            screen: SCREEN,
        };
        let values = NoValues;
        let mut out = Vec::new();
        texts.draw(&ctx, &GameTags::new(&values, 0), &mut out);
        out.iter()
            .map(|c| match *c {
                DrawCmd::Quad {
                    texture: Some(_),
                    dst,
                    color,
                    ..
                } => (dst[0], dst[1], color),
                ref other => panic!("only font cells expected: {other:?}"),
            })
            .collect()
    }

    /// O-373: kind 2 stands left-aligned at (0, H/2) with shadow (shadow at (x, y + 2) in `color & 0xFF000000`, text
    /// at (x + 2, y)) and fades out linearly over the whole duration: initial tick alpha 255, after 1000 of 2000 ms
    /// `ftol(255 - 1000 * 255/2000)` = 127, gone from 2000 ms.
    #[test]
    fn screen_text_art_2_at_half_height_fades_linearly() {
        let mut texts = ScreenTexts::new();
        texts.push(kind::MIDDLE, 7, 0xFFFFFF, 2_000);
        assert_eq!(texts.entries()[0].alpha, 255, "initial tick with dt = 0");
        // Text "7" (without a table the number): shadow, then text.
        assert_eq!(
            drawn(&texts),
            [(0, 386, 0xFF00_0000), (2, 384, 0xFFFF_FFFF)]
        );
        texts.tick(500);
        assert_eq!(texts.entries()[0].alpha, 191, "255 - 63.75");
        texts.tick(500);
        assert_eq!(texts.entries()[0].alpha, 127, "255 - 127.5");
        assert!(texts.entries()[0].half);
        assert_eq!(
            drawn(&texts),
            [(0, 386, 0x7F00_0000), (2, 384, 0x7FFF_FFFF)]
        );
        texts.tick(999);
        assert_eq!(texts.entries()[0].alpha, 0, "255 - 254.87... -> 0");
        texts.tick(1);
        assert!(texts.entries().is_empty(), "from elapsed >= ms it is over");
        assert!(drawn(&texts).is_empty());
    }

    /// O-373: a new text of the same kind (!= 0) is rejected as long as the old one does not carry the flag "half";
    /// only the tick sets that, at `elapsed >= ms / 2`. After that the new one replaces the old one at the same list
    /// position. Kind 0 always appends; other kinds do not affect each other.
    #[test]
    fn newer_screen_text_dropped_while_old_under_half_time() {
        let mut texts = ScreenTexts::new();
        texts.push(kind::MIDDLE, 1, 0xFFFFFF, 2_001);
        texts.push(kind::LOW, 2, 0x00FF00, 6_000);
        texts.tick(999);
        texts.push(kind::MIDDLE, 3, 0xFFFFFF, 1_000);
        let ids = |t: &ScreenTexts| t.entries().iter().map(|e| e.text).collect::<Vec<_>>();
        assert_eq!(ids(&texts), [1, 2], "999 < 2001/2 = 1000: rejected");
        texts.tick(1);
        assert!(texts.entries()[0].half, "1000 >= 1000 (integer division)");
        texts.push(kind::MIDDLE, 3, 0xFFFFFF, 1_000);
        assert_eq!(ids(&texts), [3, 2], "replaced at the same place");
        assert_eq!(
            (texts.entries()[0].elapsed, texts.entries()[0].alpha),
            (0, 255),
            "new with initial tick"
        );
        assert!(!texts.entries()[0].half);
        // Kind 0 always appends.
        texts.push(kind::TOP, 4, 0xFFFFFF, 1_000);
        texts.push(kind::TOP, 5, 0xFFFFFF, 1_000);
        assert_eq!(ids(&texts), [3, 2, 4, 5]);
        assert_eq!(texts.first_of(kind::MIDDLE).map(|e| e.text), Some(3));
        texts.clear();
        assert!(texts.entries().is_empty());
    }

    /// `FUN_00492A40`: kind 0 at (0, 32) left-aligned, kind 1 centered on the full width at y = H - 200; the color
    /// keeps only its RGB bits. `ms <= 0` ends in the first tick.
    #[test]
    fn screen_text_kinds_0_and_1_positions() {
        let mut texts = ScreenTexts::new();
        texts.push(kind::TOP, 12, 0xAA00_FF00, 1_000);
        texts.push(kind::LOW, 3, 0x0000_FF00, 6_000);
        // "12" at (0, 32); "3" (10 px) centered: (1024 - 10) / 2 = 507.
        assert_eq!(
            drawn(&texts),
            [
                (0, 34, 0xFF00_0000),
                (10, 34, 0xFF00_0000),
                (2, 32, 0xFF00_FF00),
                (12, 32, 0xFF00_FF00),
                (507, 570, 0xFF00_0000),
                (509, 568, 0xFF00_FF00),
            ]
        );
        texts.push(kind::MIDDLE, 9, 0xFFFFFF, 0);
        assert_eq!(texts.entries()[2].alpha, 0, "ms = 0: invisible");
        texts.tick(0);
        assert_eq!(texts.entries().len(), 2, "ms = 0 ends in the first tick");
    }

    /// Position of chat line `i` (whole pixels like `ftol`).
    fn chat_pos(texts: &ScreenTexts, i: usize) -> (i32, i32) {
        let m = texts.entries()[i].motion.as_ref().expect("position");
        (m.pos[0] as i32, m.pos[1] as i32)
    }

    /// Test vector: two chat lines 100 ms apart. The second waits until the timer of the first (8000/6 = 1333 ms)
    /// frees the kind, and then starts at (20, H - 200); the first has meanwhile risen 1333 * 1500/65536 ~ 30 px.
    /// Alpha `1024 - t * 1280/8000`: 255 at 4800 ms, 0 from 6400 ms; gone at 8000 ms.
    #[test]
    fn chat_lines_wait_rise_and_fade_d5b() {
        let mut texts = ScreenTexts::new();
        texts.push_chat("A: one".into(), 0x00FF_0000, CHAT_MS, 768);
        assert_eq!(texts.entries().len(), 1);
        assert_eq!(texts.entries()[0].kind, kind::CHAT);
        assert_eq!(
            texts.entries()[0].alpha,
            255,
            "initial tick: min(1024, 255)"
        );
        assert_eq!(chat_pos(&texts, 0), (20, 568));
        texts.tick(100);
        texts.push_chat("B: two".into(), 0x0000_FF00, CHAT_MS, 768);
        assert_eq!(texts.entries().len(), 1, "the kind is taken: waits");
        assert_eq!(texts.waiting().len(), 1);
        texts.tick(1232);
        assert_eq!(texts.entries().len(), 1, "1332 ms: not free yet");
        texts.tick(1);
        // 1333 ms: timer off, kind of the first 0, the second moves up (no longer ticked, the first was the last of
        // the list).
        assert_eq!(texts.entries().len(), 2);
        assert!(texts.waiting().is_empty());
        assert_eq!(texts.entries()[0].kind, 0);
        assert_eq!(texts.entries()[1].kind, kind::CHAT);
        assert_eq!(texts.entries()[1].elapsed, 0);
        assert_eq!(chat_pos(&texts, 1), (20, 568));
        assert_eq!(
            chat_pos(&texts, 0),
            (20, 537),
            "568 - 1333 * 0.022888 = 537.49"
        );
        // Drawn at the own position, with own text (two characters "A:" ... at x = 20 + shadow).
        let cells = drawn(&texts);
        assert!(cells.contains(&(20, 539, 0xFF00_0000)), "{cells:?}");
        assert!(cells.contains(&(22, 537, 0xFFFF_0000)));
        assert!(cells.contains(&(22, 568, 0xFF00_FF00)));
        // Alpha of the first: 255 at 4800 ms, 0 from 6400 ms.
        texts.tick(4800 - 1333);
        assert_eq!(texts.entries()[0].alpha, 255);
        texts.tick(1);
        assert_eq!(texts.entries()[0].alpha, 255, "1024 - 768.16 = 255.84");
        texts.tick(6400 - 4801);
        assert_eq!(texts.entries()[0].alpha, 0);
        texts.tick(1599);
        assert_eq!(texts.entries()[0].elapsed, 7999);
        texts.tick(1);
        assert_eq!(texts.entries().len(), 1, "gone at 8000 ms");
        assert_eq!(texts.entries()[0].own_text.as_deref(), Some("B: two"));
        assert_eq!(texts.chat_lines().count(), 1);
        texts.clear();
        assert!(texts.entries().is_empty() && texts.waiting().is_empty());
    }

    /// If a chat line arrives when none is younger than 1333 ms, it stands immediately. An entry that has moved up is
    /// ticked in the same tick if the trigger was not the last of the list (the list reads the successor before the
    /// tick), and can itself free the next one; the chat does not touch screen texts of other kinds.
    #[test]
    fn chat_line_after_the_timer_starts_at_once_and_other_kinds_stay() {
        let mut texts = ScreenTexts::new();
        texts.push_chat("1".into(), 0xFFFFFF, CHAT_MS, 768);
        texts.push(kind::MIDDLE, 7, 0xFFFFFF, 20_000);
        texts.push_chat("2".into(), 0xFFFFFF, CHAT_MS, 768);
        texts.push_chat("3".into(), 0xFFFFFF, CHAT_MS, 768);
        assert_eq!(texts.waiting().len(), 2);
        texts.tick(1333);
        // The first was not the last (behind it kind 2): "2" moves up and gets the 1333 ms right along.
        let own = |t: &ScreenTexts| {
            t.entries()
                .iter()
                .map(|e| (e.own_text.clone(), e.elapsed))
                .collect::<Vec<_>>()
        };
        // Its timer expires in the process, so "3" also moves up - as the last, behind the trigger "2", without a tick.
        assert_eq!(
            own(&texts),
            [
                (Some("1".into()), 1333),
                (None, 1333),
                (Some("2".into()), 1333),
                (Some("3".into()), 0)
            ]
        );
        assert_eq!(
            texts.entries()[2].kind,
            0,
            "timer of \"2\" expired in the same tick"
        );
        assert_eq!(texts.entries()[3].kind, kind::CHAT);
        assert!(texts.waiting().is_empty());
        texts.tick(10);
        assert_eq!(texts.entries()[3].elapsed, 10);
        // After 1333 ms without an active chat line a new one stands immediately.
        texts.tick(1333);
        texts.push_chat("4".into(), 0xFFFFFF, CHAT_MS, 600);
        assert!(texts.waiting().is_empty());
        let last = texts.entries().last().unwrap();
        assert_eq!((last.kind, last.elapsed), (kind::CHAT, 0));
        assert_eq!(chat_pos(&texts, texts.entries().len() - 1), (20, 400));
        // Kind 2 ran on untouched.
        assert_eq!(texts.first_of(kind::MIDDLE).map(|e| e.elapsed), Some(2676));
    }

    /// Tags in the chat text take effect when drawing (`FUN_00492F20` -> `FUN_00560FF0`): `[COLOR ...]` recolors the
    /// rest, the tag itself does not appear.
    #[test]
    fn chat_line_draws_tags() {
        let mut texts = ScreenTexts::new();
        texts.push_chat("A: [COLOR 0xFF00FF00]b".into(), 0x00FF_0000, CHAT_MS, 768);
        let cells = drawn(&texts);
        // Shadow and text of "A", ":", " " (drawn empty or not) and "b" in green.
        assert!(cells.contains(&(22, 568, 0xFFFF_0000)), "{cells:?}");
        assert!(
            cells.iter().any(|&(_, y, c)| y == 568 && c == 0xFF00_FF00),
            "{cells:?}"
        );
        assert!(cells.len() <= 8, "the tag draws no characters: {cells:?}");
    }
}
