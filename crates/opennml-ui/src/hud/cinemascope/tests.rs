use std::cell::Cell;

use opennml_formats::localisation::parse_localisation;

use super::*;
use crate::assets::{Texts, UiAssets};
use crate::coords::Screen;
use crate::fonts::FontBook;
use crate::testutil::{TestVfs, fnt_text, test_vfs, tga_bottom_up};
use crate::text::{GameText, TradeCost};

const WHITE: u32 = 0xFFFF_FFFF;
const TOP_TGA: &str = "data/icons/top.tga";
const DOWN_TGA: &str = "data/icons/bottom.tga";
const PORTRAIT_TGA: &str = "data/portraits/p1000.tga";
const FRAME_TGA: &str = "data/portraits/f101.tga";
const MISSING_TGA: &str = "data/portraits/missing.tga";

/// `cinemascope.ini` as in the installation (re/campaign-ui D6.1).
const INI: &str = "BARSIZE 170\r\nBARTEXTURETOP 32 32 data/icons/top.tga\r\n\
BARTEXTUREDOWN 32 32 data/icons/bottom.tga\r\nDIALOGAREA 0\r\nPORTRAITPOS 24 876\r\nPORTRAITSIZE 128 128\r\n\
TEXTPOS 170 880\r\nTEXTSIZE 933 124\r\nTEXTFONT DialogText\r\nTEXTCOLOR FFFFFFFF\r\n/DIALOGAREA\r\n";
const PORTRAITS: &str =
    "1000=Hero=data/portraits/p1000.tga\r\n1001=Path=data/portraits/missing.tga\r\n";
const FRAMES: &str = "101=Frame=data/portraits/f101.tga\r\n";

/// Text 20002 (dialog line), 20100 (hint), 30000 with a live counter, 30001 long (wraps).
const TEXTS: &[(i32, &str)] = &[
    (20002, "Hello all!"),
    (20100, "T"),
    (30000, "[GUIDCOUNTER 0 5]"),
    (
        30001,
        "AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA AAAA",
    ),
];

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

struct Rig {
    vfs: TestVfs,
    assets: UiAssets,
    texts: Texts,
    fonts: FontBook,
    data: CinemascopeData,
    values: Values,
    screen: Screen,
}

const fn screen(width: i32, height: i32) -> Screen {
    Screen { width, height }
}

/// `DialogText` and `ShowInfoText`: 10 px advance (also the space), line height 15.
fn rig_with(ini: &str) -> Rig {
    let white = |w, h| tga_bottom_up(w, h, |_, _| [255; 4]);
    let vfs = test_vfs(&[
        (
            "data/fonts/Test_15_0_0_0_0.fnt",
            fnt_text(|_| 10, |_| 15).into_bytes(),
        ),
        ("data/fonts/Test_15_0_0_0_0.tga", white(256, 256)),
        (TOP_TGA, white(32, 32)),
        (DOWN_TGA, white(32, 32)),
        (PORTRAIT_TGA, white(64, 64)),
        (FRAME_TGA, white(64, 64)),
    ]);
    let mut assets = UiAssets::new();
    let loc = parse_localisation(
        "Font DialogText\nFontSize 15\nTextureFont\nFontName Test\n\
Font ShowInfoText\nFontSize 15\nTextureFont\nFontName Test\n",
    );
    let fonts = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
    Rig {
        vfs,
        assets,
        texts: Texts::from_entries(TEXTS),
        fonts,
        data: CinemascopeData::from_texts(ini, PORTRAITS, FRAMES),
        values: Values {
            counter: Cell::new(3),
        },
        screen: screen(1024, 768),
    }
}

fn rig() -> Rig {
    rig_with(INI)
}

impl Rig {
    fn draw(&mut self, view: &CinemascopeView) -> Vec<DrawCmd> {
        let mut ctx = UiContext {
            vfs: &self.vfs,
            assets: &mut self.assets,
            fonts: &self.fonts,
            texts: &self.texts,
            screen: self.screen,
        };
        let tags = GameTags::new(&self.values, 0);
        let mut out = Vec::new();
        view.draw(&self.data, &mut ctx, &tags, &mut out);
        out
    }

    fn send(&self, view: &mut CinemascopeView, kind: HudEventKind, args: &[i32]) -> bool {
        view.message(&HudEvent::with_args(kind, args), &self.data)
    }

    /// Expected commands of a text: layout with `0x46`, font `font`, width `width`, origin `origin`.
    fn text_cmds(
        &self,
        text: &str,
        font: &str,
        width: i32,
        color: u32,
        origin: [i32; 2],
        shadow: bool,
    ) -> Vec<DrawCmd> {
        let layout = self.layout(text, font, width, color);
        let mut out = Vec::new();
        draw_runs(&layout, origin, shadow, None, &self.fonts, &mut out);
        out
    }

    fn layout(&self, text: &str, font: &str, width: i32, color: u32) -> TextLayout {
        let params = LayoutParams {
            flags: TEXT_LAYOUT,
            width,
            font: self.fonts.by_name(font),
            color,
        };
        let tags = GameTags::new(&self.values, 0);
        let mut layout = TextLayout::default();
        layout_text_game(text, &params, &self.fonts, &mut layout, None, Some(&tags));
        layout
    }
}

/// Commands by texture (path; `None` = solid-color area).
fn by_texture<'a>(rig: &Rig, cmds: &'a [DrawCmd], path: Option<&str>) -> Vec<&'a DrawCmd> {
    cmds.iter()
        .filter(|c| {
            let DrawCmd::Quad { texture, .. } = c else {
                unreachable!()
            };
            texture.map(|t| rig.assets.path(t).to_string()).as_deref() == path
        })
        .collect()
}

fn dst(cmd: &DrawCmd) -> [i32; 4] {
    let DrawCmd::Quad { dst, .. } = *cmd else {
        unreachable!()
    };
    dst
}

/// Top-left corner of a command.
fn origin(cmd: &DrawCmd) -> [i32; 2] {
    let [x, y, ..] = dst(cmd);
    [x, y]
}

fn color(cmd: &DrawCmd) -> u32 {
    let DrawCmd::Quad { color, .. } = *cmd else {
        unreachable!()
    };
    color
}

/// Area with bars.
fn with_bars(rig: &Rig) -> CinemascopeView {
    let mut view = CinemascopeView::new();
    assert!(rig.send(&mut view, HudEventKind::Cinemascope, &[128]));
    view
}

// QUIRK O-351: kept
#[test]
fn bars_follow_the_resolution() {
    let mut rig = rig();
    let view = with_bars(&rig);
    assert_eq!(view.bar, 170);
    for (size, top, bottom) in [
        (screen(1024, 768), 127, 128),
        (screen(800, 600), 99, 100),
        (screen(1280, 1024), 170, 170),
    ] {
        rig.screen = size;
        let cmds = rig.draw(&view);
        let bars = by_texture(&rig, &cmds, None);
        assert_eq!(bars.len(), 2, "{size:?}");
        let (b, h) = (size.width, size.height);
        assert_eq!(dst(bars[0]), [0, 0, b, top], "{size:?}");
        assert_eq!(dst(bars[1]), [0, h - bottom, b, h], "{size:?}");
        assert!(bars.iter().all(|c| color(c) == BAR_COLOR));
        // The bars come first.
        assert_eq!(&cmds[..2], &[*bars[0], *bars[1]]);
    }
}

// QUIRK O-351: kept
#[test]
fn edge_tiles_count_and_positions() {
    let mut rig = rig();
    let view = with_bars(&rig);
    let cmds = rig.draw(&view);
    let top = by_texture(&rig, &cmds, Some(TOP_TGA));
    let down = by_texture(&rig, &cmds, Some(DOWN_TGA));
    assert_eq!((top.len(), down.len()), (32, 32));
    for (i, (t, d)) in top.iter().zip(&down).enumerate() {
        let x = 32 * i as i32;
        // At the top the tile hangs below the bar edge, at the bottom it sits on the bar (O-351).
        assert_eq!(dst(t), [x, 127, x + 32, 159]);
        assert_eq!(dst(d), [x, 608, x + 32, 640]);
        let DrawCmd::Quad { color, filter, .. } = **t else {
            unreachable!()
        };
        assert_eq!((color, filter), (WHITE, Filter::Point));
    }
    rig.screen = screen(800, 600);
    let cmds = rig.draw(&view);
    assert_eq!(by_texture(&rig, &cmds, Some(TOP_TGA)).len(), 25);
    // Widescreen: W/32 truncated, 22 px stay free on the right.
    rig.screen = screen(1366, 768);
    let cmds = rig.draw(&view);
    let top = by_texture(&rig, &cmds, Some(TOP_TGA));
    assert_eq!(top.len(), 42);
    assert_eq!(dst(top[41])[0], 1312);
}

// QUIRK O-351: kept
#[test]
fn no_barsize_no_bars() {
    let mut rig = rig_with("BARTEXTURETOP 32 32 data/icons/top.tga\r\n");
    let mut view = with_bars(&rig);
    assert_eq!(view.bar, 0);
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[0, 101, -1, 1000, -1],
    );
    view.tick(500.0);
    assert!(rig.draw(&view).is_empty());
}

// QUIRK O-353: kept
#[test]
fn portrait_rect_is_19_657_to_121_753() {
    let mut rig = rig();
    let mut view = with_bars(&rig);
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[2, 101, -1, 1000, -1],
    );
    view.tick(500.0);
    let cmds = rig.draw(&view);
    let portrait = by_texture(&rig, &cmds, Some(PORTRAIT_TGA));
    assert_eq!(portrait.len(), 1);
    let DrawCmd::Quad {
        dst, uv, filter, ..
    } = *portrait[0]
    else {
        unreachable!()
    };
    assert_eq!(dst, [19, 657, 121, 753]);
    // Whole image, `billboardalpha` (LINEAR).
    assert_eq!((uv, filter), ([0.0, 0.0, 1.0, 1.0], Filter::Linear));
}

// QUIRK O-354: kept
#[test]
fn portrait_fade_is_127_after_250_ms_and_255_after_500() {
    let mut rig = rig();
    let mut view = with_bars(&rig);
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[1, 101, -1, 1000, -1],
    );
    assert_eq!(view.portraits[1].fade, Fade::show());
    view.tick(250.0);
    let cmds = rig.draw(&view);
    assert_eq!(
        color(by_texture(&rig, &cmds, Some(PORTRAIT_TGA))[0]),
        0x7FFF_FFFF
    );
    assert_eq!(view.portraits[1].fade.state, 3);
    view.tick(250.0);
    let cmds = rig.draw(&view);
    assert_eq!(color(by_texture(&rig, &cmds, Some(PORTRAIT_TGA))[0]), WHITE);
    // End of the fade: bit 2 cleared.
    assert_eq!(view.portraits[1].fade.state, 1);
    assert_eq!(view.slots().0, vec![(1, 1000, 101, 255)]);
    // Fade-out: alpha 255 -> 0 in 500 ms, then state 0 and nothing drawn.
    rig.send(&mut view, HudEventKind::PortraitHide, &[1]);
    view.tick(250.0);
    let cmds = rig.draw(&view);
    assert_eq!(
        color(by_texture(&rig, &cmds, Some(PORTRAIT_TGA))[0]),
        0x7FFF_FFFF
    );
    view.tick(250.0);
    assert_eq!(view.portraits[1].fade.state, 0);
    let cmds = rig.draw(&view);
    assert!(by_texture(&rig, &cmds, Some(PORTRAIT_TGA)).is_empty());
}

// QUIRK O-354: kept
#[test]
fn portrait_fade_freezes_without_bars() {
    let rig = rig();
    let mut view = CinemascopeView::new();
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[0, 101, -1, 1000, -1],
    );
    view.tick(250.0);
    view.tick(1000.0);
    // Without bars the portrait fade does not run (O-354).
    assert_eq!(view.portraits[0].fade, Fade::show());
}

// QUIRK O-354: kept
#[test]
fn bar_message_clears_portrait_states_both_ways() {
    let rig = rig();
    for v in [1, 0] {
        let mut view = with_bars(&rig);
        for pos in 0..4 {
            rig.send(
                &mut view,
                HudEventKind::PortraitShow,
                &[pos, 101, -1, 1000, -1],
            );
        }
        view.tick(100.0);
        rig.send(&mut view, HudEventKind::TextShow, &[0, 20002, -1]);
        assert!(rig.send(&mut view, HudEventKind::Cinemascope, &[v]));
        assert_eq!(view.bar, if v == 0 { 0 } else { 170 });
        assert!(view.portraits.iter().all(|s| s.fade.state == 0), "v = {v}");
        // State only: portrait, frame and alpha stay; the text slots stay untouched.
        assert_eq!(
            (view.portraits[2].portrait, view.portraits[2].frame),
            (1000, 101)
        );
        assert_eq!(view.portraits[2].fade.alpha, 100.0 * FADE_RATE);
        assert_eq!(view.texts[0].fade.state, 3);
    }
}

// QUIRK O-354: kept
#[test]
fn hide_on_cleared_slot_fades_the_old_portrait_again() {
    let mut rig = rig();
    let mut view = with_bars(&rig);
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[3, 101, -1, 1000, -1],
    );
    view.tick(500.0);
    rig.send(&mut view, HudEventKind::Cinemascope, &[1]);
    let cmds = rig.draw(&view);
    assert!(by_texture(&rig, &cmds, Some(PORTRAIT_TGA)).is_empty());
    // `{0x1F}` sets state 5 unconditionally: the old portrait fades out for another 500 ms (O-354).
    assert!(rig.send(&mut view, HudEventKind::PortraitHide, &[3]));
    assert_eq!(view.portraits[3].fade.state, 5);
    view.tick(100.0);
    let cmds = rig.draw(&view);
    let portrait = by_texture(&rig, &cmds, Some(PORTRAIT_TGA));
    assert_eq!(portrait.len(), 1);
    assert_eq!(color(portrait[0]) >> 24, 204);
    view.tick(400.0);
    let cmds = rig.draw(&view);
    assert!(by_texture(&rig, &cmds, Some(PORTRAIT_TGA)).is_empty());
}

// QUIRK O-48, O-353: kept
#[test]
fn frame_drawn_before_portrait_in_the_same_rect() {
    let mut rig = rig();
    let mut view = with_bars(&rig);
    // Two slots at the same place (O-48); slot 0 before slot 1, for each: frame before portrait.
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[1, 101, 0x00FF_0000, 1000, 0x0000_FF00],
    );
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[0, 101, -1, 1000, -1],
    );
    view.tick(500.0);
    let cmds = rig.draw(&view);
    let images: Vec<(String, [i32; 4], u32)> = cmds
        .iter()
        .filter_map(|c| {
            let DrawCmd::Quad {
                texture,
                dst,
                color,
                ..
            } = *c
            else {
                unreachable!()
            };
            let path = rig.assets.path(texture?).to_string();
            (path == PORTRAIT_TGA || path == FRAME_TGA).then_some((path, dst, color))
        })
        .collect();
    let rect = [19, 657, 121, 753];
    assert_eq!(
        images,
        vec![
            (FRAME_TGA.to_string(), rect, WHITE),
            (PORTRAIT_TGA.to_string(), rect, WHITE),
            // Frame with the first color, portrait with the second, both with the alpha of the fade.
            (FRAME_TGA.to_string(), rect, 0xFFFF_0000),
            (PORTRAIT_TGA.to_string(), rect, 0xFF00_FF00),
        ]
    );
    // Portraits after the tiles.
    let last_tile = cmds
        .iter()
        .rposition(|c| by_texture(&rig, std::slice::from_ref(c), Some(DOWN_TGA)).len() == 1)
        .unwrap();
    assert_eq!(cmds.len(), last_tile + 5);
}

// QUIRK O-353: kept
// QUIRK O-378: deviation
#[test]
fn unknown_portrait_draws_nothing_and_loads_once() {
    let mut rig = rig();
    let mut view = with_bars(&rig);
    // Unknown frame ID 7 (O-353): only the portrait appears.
    rig.send(&mut view, HudEventKind::PortraitShow, &[0, 7, -1, 1000, -1]);
    // Portrait 1001 with a missing file, unknown portrait 9: only the frame appears.
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[1, 101, -1, 1001, -1],
    );
    rig.send(&mut view, HudEventKind::PortraitShow, &[2, 101, -1, 9, -1]);
    view.tick(500.0);
    let tiles = 64;
    for _ in 0..2 {
        let cmds = rig.draw(&view);
        assert_eq!(by_texture(&rig, &cmds, Some(PORTRAIT_TGA)).len(), 1);
        assert_eq!(by_texture(&rig, &cmds, Some(FRAME_TGA)).len(), 2);
        assert_eq!(cmds.len(), 2 + tiles + 3);
    }
    // One load attempt for the missing file, not one per frame (deviation from the original, O-378).
    let missing = rig.assets.missing().iter().filter(|p| *p == MISSING_TGA);
    assert_eq!(missing.count(), 1);
}

// QUIRK O-48, O-352: kept
#[test]
fn dialog_text_at_136_660_width_746_without_shadow() {
    let mut rig = rig();
    let mut view = CinemascopeView::new();
    rig.send(&mut view, HudEventKind::TextShow, &[0, 30001, -1]);
    rig.send(&mut view, HudEventKind::TextShow, &[1, 30001, -1]);
    view.tick(500.0);
    let cmds = rig.draw(&view);
    let one = rig.text_cmds(TEXTS[3].1, "DialogText", 746, WHITE, [136, 660], false);
    assert!(
        rig.layout(TEXTS[3].1, "DialogText", 746, WHITE).height > 15,
        "wraps"
    );
    assert_eq!(origin(&one[0]), [136, 660]);
    // Slot 0 and slot 1 at the same place (O-48).
    assert_eq!(cmds, [one.clone(), one].concat());
    // Color of the message (-1 = white), not `TEXTCOLOR`.
    assert!(cmds.iter().all(|c| color(c) == WHITE));
}

// QUIRK O-48: kept
#[test]
fn fallback_without_dialogarea_draws_in_pixels() {
    let ini = "BARSIZE 170\r\nBARTEXTURETOP 32 32 data/icons/top.tga\r\n\
BARTEXTUREDOWN 32 32 data/icons/bottom.tga\r\n";
    let mut rig = rig_with(ini);
    assert_eq!(rig.data.area, None);
    let mut view = with_bars(&rig);
    rig.send(
        &mut view,
        HudEventKind::PortraitShow,
        &[3, 101, -1, 1000, -1],
    );
    rig.send(&mut view, HudEventKind::TextShow, &[0, 20002, -1]);
    rig.send(&mut view, HudEventKind::TextShow, &[1, 20002, -1]);
    view.tick(500.0);
    let cmds = rig.draw(&view);
    // Portrait (0, 0)-(170, 170) in pixels, without conversion (index always 0).
    assert_eq!(
        dst(by_texture(&rig, &cmds, Some(PORTRAIT_TGA))[0]),
        [0, 0, 170, 170]
    );
    assert_eq!(
        dst(by_texture(&rig, &cmds, Some(FRAME_TGA))[0]),
        [0, 0, 170, 170]
    );
    // Slot 0 at (170, 0), slot 1 at (170, 768 - 170), width 1024 - 340 = 684, `DialogText`, without shadow.
    let text = [
        rig.text_cmds("Hello all!", DIALOG_FONT, 684, WHITE, [170, 0], false),
        rig.text_cmds("Hello all!", DIALOG_FONT, 684, WHITE, [170, 598], false),
    ]
    .concat();
    assert_eq!(cmds[cmds.len() - text.len()..], text[..]);
    assert_eq!(origin(&text[0]), [170, 0]);
}

// QUIRK E-240, O-355: kept
#[test]
fn text_slots_fade_without_bars() {
    let mut rig = rig();
    let mut view = CinemascopeView::new();
    rig.send(&mut view, HudEventKind::TextShow, &[1, 20002, -1]);
    view.tick(250.0);
    // Without bars the text fade runs and the text stands over the world (O-355).
    let cmds = rig.draw(&view);
    assert!(!cmds.is_empty());
    assert!(cmds.iter().all(|c| color(c) == 0x7FFF_FFFF));
    view.tick(250.0);
    assert_eq!(view.texts[1].fade.state, 1);
    assert_eq!(view.slots().1, vec![(1, 20002, 255)]);
    let cmds = rig.draw(&view);
    assert!(by_texture(&rig, &cmds, None).is_empty(), "no bars");
}

// QUIRK O-356: kept
#[test]
fn tip_slots_2_and_3_with_shadow() {
    let mut rig = rig();
    let mut view = CinemascopeView::new();
    rig.send(&mut view, HudEventKind::TextShow, &[2, 20100, 0x00FF_8040]);
    view.tick(500.0);
    let cmds = rig.draw(&view);
    // Shadow at (x, y + 2) in `color & 0xFF000000`, text at (x + 2, y); width W/2.
    assert_eq!(cmds.len(), 2);
    assert_eq!((origin(&cmds[0]), color(&cmds[0])), ([0, 32], 0xFF00_0000));
    assert_eq!((origin(&cmds[1]), color(&cmds[1])), ([2, 30], 0xFFFF_8040));
    assert_eq!(
        cmds,
        rig.text_cmds("T", INFO_TEXT_FONT, 512, 0xFFFF_8040, [0, 30], true)
    );
    // Slot 3 at (BARSIZE raw, H - text height - 200), independent of the bars (O-356).
    let mut view = CinemascopeView::new();
    rig.send(&mut view, HudEventKind::TextShow, &[3, 20100, -1]);
    view.tick(500.0);
    let cmds = rig.draw(&view);
    let h = rig.layout("T", INFO_TEXT_FONT, 512, WHITE).height;
    assert_eq!(h, 15);
    assert_eq!(origin(&cmds[0]), [170, 768 - 15 - 200 + 2]);
    assert_eq!(origin(&cmds[1]), [172, 553]);
    assert_eq!(
        cmds,
        rig.text_cmds("T", INFO_TEXT_FONT, 512, WHITE, [170, 553], true)
    );
}

// QUIRK O-356: kept
#[test]
fn text_relayouts_every_frame_with_a_live_counter() {
    let mut rig = rig();
    let mut view = CinemascopeView::new();
    rig.send(&mut view, HudEventKind::TextShow, &[0, 30000, -1]);
    view.tick(500.0);
    assert_eq!(rig.draw(&view).len(), 1);
    rig.values.counter.set(12);
    let cmds = rig.draw(&view);
    assert_eq!(
        cmds,
        rig.text_cmds("12", "DialogText", 746, WHITE, [136, 660], false)
    );
}

#[test]
fn text_out_of_range_slot_is_ignored() {
    let rig = rig();
    let mut view = CinemascopeView::new();
    // The original writes behind the fields (O-357); the reimplementation skips the slot, the message counts as
    // handled.
    for pos in [-1, 4, 99] {
        assert!(rig.send(&mut view, HudEventKind::TextShow, &[pos, 20002, -1]));
        assert!(rig.send(&mut view, HudEventKind::TextHide, &[pos]));
        assert!(rig.send(
            &mut view,
            HudEventKind::PortraitShow,
            &[pos, 101, -1, 1000, -1]
        ));
        assert!(rig.send(&mut view, HudEventKind::PortraitHide, &[pos]));
    }
    assert_eq!(view, CinemascopeView::new());
    // The area does not handle other kinds.
    for kind in [
        HudEventKind::StatusBarOnOff,
        HudEventKind::MinimapOnOff,
        HudEventKind::Lock,
    ] {
        assert!(!rig.send(&mut view, kind, &[1]));
    }
}

// QUIRK O-352: kept
#[test]
fn second_color_is_never_drawn() {
    let mut rig = rig();
    let mut view = CinemascopeView::new();
    rig.send(&mut view, HudEventKind::TextShow, &[0, 20002, 0x0012_3456]);
    view.tick(500.0);
    // The second color is faded along but never used (O-352): changed, the area draws the same.
    let before = rig.draw(&view);
    view.texts[0].colors[1] = 0xFFAB_CDEF;
    assert_eq!(rig.draw(&view), before);
    assert!(before.iter().all(|c| color(c) == 0xFF12_3456));
    view.texts[0].colors[0] = 0xFFAB_CDEF;
    assert_ne!(rig.draw(&view), before);
}

#[test]
fn text_hide_jumps_to_255_then_fades() {
    let mut rig = rig();
    let mut view = CinemascopeView::new();
    rig.send(&mut view, HudEventKind::TextShow, &[1, 20002, -1]);
    view.tick(100.0);
    assert_eq!(view.texts[1].fade.alpha_u8(), 51);
    // Closed in the middle of fading in: alpha jumps to 255, state (3 & !2) | 4 = 5.
    rig.send(&mut view, HudEventKind::TextHide, &[1]);
    assert_eq!(view.texts[1].fade.state, 5);
    assert_eq!(view.texts[1].fade.alpha_u8(), 255);
    view.tick(250.0);
    assert!(rig.draw(&view).iter().all(|c| color(c) == 0x7FFF_FFFF));
    view.tick(250.0);
    assert_eq!(view.texts[1].fade.state, 0);
    assert!(rig.draw(&view).is_empty());
    // Closing an empty slot: state 4, nothing drawn, then 0.
    rig.send(&mut view, HudEventKind::TextHide, &[2]);
    assert_eq!(view.texts[2].fade.state, 4);
    assert!(rig.draw(&view).is_empty());
    view.tick(500.0);
    assert_eq!(view.texts[2].fade.state, 0);
}

#[test]
fn unknown_text_id_draws_its_own_number() {
    let mut rig = rig();
    let mut view = CinemascopeView::new();
    // `FUN_00562770` -> `FUN_0055FFE0`: without an entry the own number (`"%d"`, O-241).
    rig.send(&mut view, HudEventKind::TextShow, &[0, 4711, -1]);
    view.tick(500.0);
    assert_eq!(
        rig.draw(&view),
        rig.text_cmds("4711", "DialogText", 746, WHITE, [136, 660], false)
    );
}

#[test]
fn show_now_shows_without_fade() {
    let mut rig = rig();
    let mut view = with_bars(&rig);
    let ev = HudEvent::with_args(HudEventKind::PortraitShow, &[1, 101, -1, 1000, -1]);
    assert!(view.show_now(&ev, &rig.data));
    let ev = HudEvent::with_args(HudEventKind::TextShow, &[3, 20100, -1]);
    assert!(view.show_now(&ev, &rig.data));
    assert_eq!(
        (view.portraits[1].fade.state, view.texts[3].fade.state),
        (1, 1)
    );
    let before = rig.draw(&view);
    view.tick(100.0);
    assert_eq!(rig.draw(&view), before);
    assert_eq!(
        view.slots(),
        (vec![(1, 1000, 101, 255)], vec![(3, 20100, 255)])
    );
}

#[test]
fn load_without_files_gives_empty_parts() {
    let vfs = test_vfs(&[(PORTRAITS_INI_PATH, PORTRAITS.as_bytes().to_vec())]);
    let data = CinemascopeData::load(&vfs);
    assert_eq!(data.ini, CinemascopeIni::default());
    assert_eq!(data.area, None);
    assert_eq!(data.frames.len(), 0);
    assert_eq!(
        data.portraits.get(&1000).map(String::as_str),
        Some(PORTRAIT_TGA)
    );
}
