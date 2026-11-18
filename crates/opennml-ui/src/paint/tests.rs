//! Tests for drawing from the tree: fly-in and fly-out, image table, colors and contents from the state, pass over the
//! live child list.

use super::*;
use crate::coords::Screen;
use crate::dialog::paint_static;
use crate::dialog::tests::{bkg_cmds, color, dialog, dst, texture};
use crate::draw::Blend;
use crate::input::UiInput;
use crate::msg::{Param, code};
use crate::tree::{Broadcast, FrameMode};
use crate::widgets::frame::transition;
use crate::widgets::testkit::{Harness, center};

const S1280: Screen = Screen {
    width: 1280,
    height: 1024,
};

/// `start.dlg` (shipped values for position, duration and bar) with one button.
const START: &str = "FRAME START\n FLAGS MENUFRAME_LEVELBACKGROUND MENUFRAME_DRAWBARL MENUFRAME_MOVE /FLAGS\n \
                     EXTENT -440 212 0 912\n \
                     ZIELPOSITION 40 212\n DURATION 300\n BAREXTENT 0 140 550 225\n \
                     BACKGROUND data/menü/bg.tga\n BUTTON B EXTENT 55 10 345 70 /BUTTON\n/FRAME\n";

fn frame_state(h: &Harness, id: NodeId) -> &frame::FrameState {
    frame::state(&h.tree, id)
}

fn open(h: &mut Harness, id: NodeId, mode: FrameMode) -> bool {
    h.with(|t, _, e| transition(t, id, mode, e))
}

/// Whether `needle` occurs in `hay` as a contiguous sequence.
fn contains_run(hay: &[DrawCmd], needle: &[DrawCmd]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Expected frame decoration of frame `id` in the current state with the offset `offset`.
fn decor(h: &mut Harness, id: NodeId, offset: [i32; 2]) -> Vec<DrawCmd> {
    let (desc, flags, rect) = {
        let node = h.tree.node(id);
        (frame_state(h, id).desc.clone(), node.flags, node.rect)
    };
    let zusatz = [
        Some(h.f.tex("data/menü/zusatz_85x85.tga")),
        Some(h.f.tex("data/menü/zusatz_85x85_gespiegelt.tga")),
    ];
    let mut out = Vec::new();
    paint_frame_decor(
        &desc,
        flags,
        rect,
        offset,
        h.screen,
        zusatz,
        None,
        &h.f.fonts,
        &h.f.assets,
        &mut out,
    );
    out
}

// QUIRK O-23, O-108: kept
#[test]
fn start_flies_in_with_integer_steps() {
    // re/menu-system 12.2 with the values of `start.dlg`: x = ex + rem*(sx-ex)/dur (IDIV), after 0/75/150/299/300 ms
    // -440, -320, -200, 39, 40; bar offset in the first half (rem-half)*(sx-ex)/half, at 75 ms
    // -(225-150)*480/150 = -240, from 150 ms 0. `0x136` exactly in the step that uses up the remaining time.
    let mut h = Harness::new();
    h.now = 1000;
    let frame = h.build(START);
    assert_eq!(
        h.tree.node(frame).rect,
        [-440, 212, 0, 912],
        "built at EXTENT"
    );
    assert!(
        !open(&mut h, frame, FrameMode::Open),
        "not finished at once with MOVE"
    );
    for (t, x, bar, done) in [
        (1000, -440, -480, false),
        (1075, -320, -240, false),
        (1150, -200, 0, false),
        (1299, 39, 0, false),
        (1300, 40, 0, true),
    ] {
        h.now = t;
        let finished = h.tick();
        assert_eq!(h.tree.node(frame).rect, [x, 212, x + 440, 912], "{t}");
        assert_eq!(frame_state(&h, frame).bar, [bar, 0], "{t}");
        assert_eq!(finished == [frame], done, "{t}");
    }
    // End position like static drawing, afterwards no more movement.
    h.now = 1400;
    assert!(h.tick().is_empty());
    assert_eq!(h.tree.node(frame).rect, [40, 212, 480, 912]);
    // Opening with MOVE: 0x15E with BACKGROUND and 2*DURATION = 600 ms (from 1000 ms).
    assert_eq!(
        h.tree.images.visible(1300),
        [("DATA/MENÜ/BG.TGA".to_string(), 127)]
    );

    // Drawn: the child gets its rectangle **before** the tick as clip (`0x0045A51E`), so V = old intersected with new
    // position (O-108). In the first frame (old position -440...0 entirely outside on the left) the frame stays
    // invisible; at 75 ms after a frame at 60 ms (x = -344) it appears clipped to (0, 212, 96, 912), the bar by -240.
    let mut h = Harness::new();
    h.now = 1000;
    let frame = h.build(START);
    open(&mut h, frame, FrameMode::Open);
    h.tick();
    assert!(h.tree.node(frame).visit.is_none(), "first frame: V empty");
    h.now = 1060;
    h.tick();
    assert_eq!(h.tree.node(frame).rect[0], -344);
    h.now = 1075;
    h.tick();
    assert_eq!(
        h.tree.node(frame).visit,
        Some((h.tree.pass, [0, 212, 96, 912]))
    );
    let cmds = h.paint();
    let want = decor(&mut h, frame, [-240, 0]);
    assert!(contains_run(&cmds, &want), "bar with offset");
    let frame_bkg = bkg_cmds(
        &mut h.f,
        "data/menü/menuframe.bkg",
        [-320, 212, 120, 912],
        [0, 212, 96, 912],
        S1280,
    );
    assert!(
        contains_run(&cmds, &frame_bkg),
        "frame at the current position"
    );
}

// QUIRK O-108: kept
#[test]
fn closing_ends_at_extent_with_bar_minus_500() {
    // Closing: back from ZIELPOSITION to EXTENT, bar offset in the second half
    // -((half-rem)*(sx-ex)/half), at the end (0, -500); the frame is still drawn in the frame of the end - with
    // V = old position intersected with new position intersected with clip (the child gets the rectangle before its
    // tick as clip).
    let mut h = Harness::new();
    let frame = h.build(
        "FRAME F FLAGS MENUFRAME_BACKGROUND MENUFRAME_DRAWBARL MENUFRAME_MOVE /FLAGS EXTENT 0 0 400 300 \
         ZIELPOSITION 200 0 DURATION 300 BAREXTENT 0 140 550 225 BACKGROUND data/menü/bg.tga /FRAME\n",
    );
    open(&mut h, frame, FrameMode::Open);
    h.now = 300;
    assert_eq!(h.tick(), [frame]);
    h.now = 1000;
    h.tick();
    assert!(!open(&mut h, frame, FrameMode::Close));
    for (t, x, bar) in [(1000, 200, 0), (1075, 150, 0), (1225, 50, -100)] {
        h.now = t;
        assert!(h.tick().is_empty(), "{t}");
        assert_eq!(h.tree.node(frame).rect[0], x, "{t}");
        assert_eq!(frame_state(&h, frame).bar, [bar, 0], "{t}");
    }
    h.now = 1300;
    assert_eq!(h.tick(), [frame]);
    assert_eq!(h.tree.node(frame).rect, [0, 0, 400, 300]);
    assert_eq!(frame_state(&h, frame).bar, [0, -500]);
    // Still drawn: the frame over the full rectangle (re/menu-system 8.4.2), clip = old intersected with new position
    // = (50, 0, 400, 300).
    let cmds = h.paint();
    let want = bkg_cmds(
        &mut h.f,
        "data/menü/menuframe.bkg",
        [0, 0, 400, 300],
        [50, 0, 400, 300],
        S1280,
    );
    assert!(contains_run(&cmds, &want));
    assert!(contains_run(&cmds, &decor(&mut h, frame, [0, -500])));
    // Fade-out with 600 ms from 1000 ms (0x15F).
    assert_eq!(
        h.tree.images.visible(1300),
        [("DATA/MENÜ/BG.TGA".to_string(), 128)]
    );
}

#[test]
fn duration_zero_ends_in_the_first_tick() {
    // DURATION 0: rem = 0 - dt < 1 already in the first tick, without division; end position ZIELPOSITION.
    let mut h = Harness::new();
    h.now = 50;
    let frame = h.build(
        "FRAME F FLAGS MENUFRAME_MOVE /FLAGS EXTENT -440 212 0 912 ZIELPOSITION 40 212 DURATION 0 /FRAME\n",
    );
    assert!(!open(&mut h, frame, FrameMode::Open));
    assert_eq!(h.tick(), [frame]);
    assert_eq!(h.tree.node(frame).rect, [40, 212, 480, 912]);
    assert!(!frame_state(&h, frame).moving);
    assert_eq!(frame_state(&h, frame).bar, [0, 0]);
    // Without MOVE the transition reports done at once (0x136 synchronous) and moves nothing.
    let mut h = Harness::new();
    let frame = h.build("FRAME F EXTENT 10 10 20 20 /FRAME\n");
    assert!(open(&mut h, frame, FrameMode::Open));
    assert!(h.tick().is_empty());
    assert_eq!(h.tree.node(frame).rect, [10, 10, 20, 20]);
}

// QUIRK O-92: kept
#[test]
fn images_paint_in_cmap_order() {
    // re/menu-system 12.3, O-92: opening without MOVE with MENUFRAME_BACKGROUND -> 0x15E with 500 ms; drawn in CMap
    // order of the uppercased paths: SID (bucket 7), WID (10), PAT (12) - PAT last, so on top -, regardless of the
    // order in which they were opened.
    let mut h = Harness::new();
    let mut frames = Vec::new();
    for name in ["pat", "sid", "wid"] {
        frames.push(h.build(&format!(
            "FRAME {name} FLAGS MENUFRAME_NOFRAME MENUFRAME_BACKGROUND /FLAGS EXTENT 0 0 1 1 \
             BACKGROUND data/menü/background_1024x1024_{name}.tga /FRAME\n"
        )));
    }
    for &f in &frames {
        assert!(open(&mut h, f, FrameMode::Open));
    }
    h.now = 250;
    h.tick();
    let tex =
        |h: &mut Harness, n: &str| h.f.tex(&format!("data/menü/background_1024x1024_{n}.tga"));
    let want = [tex(&mut h, "sid"), tex(&mut h, "wid"), tex(&mut h, "pat")];
    let cmds = h.paint();
    let images: Vec<_> = cmds[..3].iter().map(|c| (texture(c), color(c))).collect();
    // 250*255/500 = 127 -> color 0x7FFFFFFF.
    assert_eq!(images, want.map(|t| (Some(t), 0x7FFF_FFFF)));
    h.now = 500;
    h.tick();
    let cmds = h.paint();
    assert!(cmds[..3].iter().all(|c| color(c) == 0xFFFF_FFFF));
    // An image that is only broadcast to the desktop later in the pass (here by the 0x136 listener in the tick of a
    // frame) is drawn only in the next frame: the original switches the table in `Desktop::Draw` before the children.
    let mover = h.build(
        "FRAME M FLAGS MENUFRAME_NOFRAME MENUFRAME_MOVE /FLAGS EXTENT 0 0 1 1 ZIELPOSITION 0 0 DURATION 10 /FRAME\n",
    );
    open(&mut h, mover, FrameMode::Open);
    h.now = 510;
    let show = Broadcast::ShowImage {
        path: "data/menü/bg.tga".into(),
        ms: 0,
    };
    h.with(|t, _, e| {
        tick_tree(t, e, &mut |t, _, e| {
            let desktop = t.desktop();
            t.broadcast(desktop, &show, e);
        })
    });
    let bg = h.f.tex("data/menü/bg.tga");
    assert!(h.tree.images.contains("data/menü/bg.tga"));
    assert!(h.paint().iter().all(|c| texture(c) != Some(bg)));
    h.now = 511;
    h.tick();
    assert!(h.paint().iter().any(|c| texture(c) == Some(bg)));
}

// QUIRK O-91: kept
#[test]
fn disabled_text_color_with_the_normal_set() {
    // O-91: at run time DISABLE from the file locks at the first query; the button shows set 0
    // (`button_normal.bkg`) and DISABLEDTEXTCOLOR. Static drawing keeps its own path
    // (`button_disabled.bkg`, `DIALOG_DRAW_GOLDEN`).
    const SRC: &str = "FRAME F FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
                       BUTTON B FLAGS MENUBUTTON_TEXT DISABLE /FLAGS TEXT 1 DISABLEDTEXTCOLOR 667D6A41 \
                       EXTENT 0 0 200 60 /BUTTON\n/FRAME\n";
    let mut h = Harness::new();
    h.build(SRC);
    h.tick();
    let cmds = h.paint();
    let normal = h.f.tex("data/menü/buttons_13x13_normal.tga");
    let disabled = h.f.tex("data/menü/buttons_13x13_deaktivated.tga");
    assert_eq!(texture(&cmds[0]), Some(normal));
    assert!(cmds.iter().all(|c| texture(c) != Some(disabled)));
    assert_eq!(color(cmds.last().unwrap()), 0x667D_6A41);
    let d = dialog(SRC);
    let cmds = paint_static(&[&d], &mut h.f.ctx(S1280));
    assert_eq!(texture(&cmds[0]), Some(disabled));
    assert_eq!(color(cmds.last().unwrap()), 0x667D_6A41);
}

/// Frame with client origin (115, 130), a ComboBox with three entries (values 10, 20, 30, code 0 -> 20).
const COMBO: &str = "FRAME DLG EXTENT 100 100 600 500\n\
     COMBOBOX RES FLAGS MENUCOMBOBOX_TEXT /FLAGS TEXT 1 BUTTONEXTENT 50 10 250 50 \
     LISTBOXEXTENT 50 60 250 200 SELECTEDTEXTCOLOR FF00FF00 /COMBOBOX\n/FRAME\n";

fn combo_logic(h: &mut Harness, value: i32) {
    h.rec.allow = true;
    h.rec.answer = Box::new(move |m| {
        if m.element != "res" {
            return -1;
        }
        match m.code {
            code::QUERY_ITEMS => {
                let items = ["A", "B", "C"].map(String::from).to_vec();
                m.params.insert("array".into(), Param::Array(items));
                m.params
                    .insert("indexarray".into(), Param::IndexArray(vec![10, 20, 30]));
                0
            }
            code::QUERY_VALUE => {
                m.params.insert("int".into(), Param::Int(value));
                0
            }
            _ => -1,
        }
    });
}

/// The glyph quads (texture of the font atlas) of a command list.
fn glyphs(h: &Harness, cmds: &[DrawCmd]) -> Vec<DrawCmd> {
    let atlas = h.f.font_atlas();
    cmds.iter()
        .filter(|c| texture(c) == Some(atlas))
        .copied()
        .collect()
}

/// `u0, v0` of glyph `c` in the 16 x 16 grid of the test font.
fn uv_of(c: u8) -> [f32; 2] {
    [f32::from(c % 16) / 16.0, f32::from(c / 16) / 16.0]
}

fn uv0(cmd: &DrawCmd) -> [f32; 2] {
    match *cmd {
        DrawCmd::Quad { uv, .. } => [uv[0], uv[1]],
        DrawCmd::Quad4 { .. } => unreachable!(),
    }
}

#[test]
fn combo_text_comes_from_the_selection() {
    // re/menu-system 8.4.4: ComboBox with 0 <= selection < count shows array[selection] instead of TEXT - code 0 = 20
    // -> row 1 = "B".
    let mut h = Harness::new();
    combo_logic(&mut h, 20);
    h.build(COMBO);
    h.tick();
    let cmds = h.paint();
    let g = glyphs(&h, &cmds);
    assert_eq!(g.len(), 2, "shadow and text of one character");
    assert!(g.iter().all(|c| uv0(c) == uv_of(b'B')));
    // Without a valid selection (value not in indexarray -> -1) the text of the file "AB".
    let mut h = Harness::new();
    combo_logic(&mut h, 99);
    h.build(COMBO);
    h.tick();
    let cmds = h.paint();
    let g = glyphs(&h, &cmds);
    assert_eq!(g.len(), 4);
    assert_eq!(uv0(&g[2]), uv_of(b'A'));
}

// QUIRK O-18: kept
#[test]
fn open_combo_list_draws_its_entries_at_the_moved_client() {
    // Ghidra FUN_00464400: the entries sit at the current client (FUN_00459B30), the text window +0x10C...
    // only supplies the width. After its OnAttach the list was moved by the client origin of the frame
    // (combo.rs), its text window comes from before.
    let mut h = Harness::new();
    combo_logic(&mut h, 20);
    let res = {
        h.build(COMBO);
        h.tree.find("DLGRES").unwrap()
    };
    let at = center(&h.tree, res);
    h.click(at);
    let NodeKind::Button(b) = &h.tree.node(res).kind else {
        unreachable!()
    };
    let NodeKind::ComboPopup(p) = &h.tree.node(b.popup.unwrap()).kind else {
        unreachable!()
    };
    let list = p.list_box.unwrap();
    let NodeKind::ListBox(l) = &h.tree.node(list).kind else {
        unreachable!()
    };
    let client = h.tree.client_rect(list);
    assert_eq!(client, [178, 203, 352, 252]);
    assert_eq!(l.text_window[0], 63, "window before the move");
    assert_eq!(l.text_window[2] - l.text_window[0], client[2] - client[0]);
    h.tick();
    let cmds = h.paint();
    let g = glyphs(&h, &cmds);
    // Per entry shadow (x, y + 2) and text (x + 2, y); the rows move by the layout height 15.
    let starts: Vec<[i32; 2]> = g.iter().map(|c| [dst(c)[0], dst(c)[1]]).collect();
    assert_eq!(
        starts,
        [
            [178, 205],
            [180, 203],
            [178, 220],
            [180, 218],
            [178, 235],
            [180, 233]
        ]
    );
    // The open ComboBox draws no text of its own (O-18); row 1 is the selection.
    assert_eq!(color(&g[3]), 0xFF00_FF00);
}

#[test]
fn listbox_entries_show_selection_bar_and_hover_mix() {
    // re/menu-system 8.4.6: TEXTCOLOR, selection SELECTEDTEXTCOLOR with a bar in front (one row high, without a
    // scrollbar the full client width), hover per byte (MOUSEOVERCOLOR + color)/2; locked everything DISABLEDTEXTCOLOR,
    // bar remains.
    const SRC: &str = "FRAME DLG EXTENT 100 100 600 500\n\
         LISTBOX L EXTENT 10 10 210 102 TEXTCOLOR FF102030 SELECTEDTEXTCOLOR FF00FF00 \
         MOUSEOVERCOLOR 80FFFFFF SELECTIONBARCOLOR 40112233 DISABLEDTEXTCOLOR FF0000AA /LISTBOX\n/FRAME\n";
    let logic = |h: &mut Harness, status: i32| {
        h.rec.answer = Box::new(move |m| {
            if m.element != "l" {
                return -1;
            }
            match m.code {
                code::QUERY_ITEMS => {
                    let items = ["E0", "E1", "E2"].map(String::from).to_vec();
                    m.params.insert("array".into(), Param::Array(items));
                    0
                }
                code::QUERY_VALUE => {
                    m.params.insert("int".into(), Param::Int(1));
                    0
                }
                code::QUERY_STATUS => {
                    m.params.insert("status".into(), Param::Int(status));
                    0
                }
                _ => -1,
            }
        });
    };
    let mut h = Harness::new();
    logic(&mut h, 0);
    h.build(SRC);
    // Mouse over row 2 (client (138, 153, 312, 219), rows of 15).
    h.input(UiInput::Move {
        x: 200,
        y: 153 + 30 + 7,
    });
    h.tick();
    let cmds = h.paint();
    let text_colors: Vec<u32> = glyphs(&h, &cmds)
        .iter()
        .map(color)
        .filter(|c| c & 0x00FF_FFFF != 0)
        .collect();
    let mixed = 0xBF87_8F97; // (80 + FF)/2, (FF + 10)/2, (FF + 20)/2, (FF + 30)/2
    assert_eq!(
        text_colors,
        [
            0xFF10_2030,
            0xFF10_2030,
            0xFF00_FF00,
            0xFF00_FF00,
            mixed,
            mixed
        ]
    );
    let bar = DrawCmd::Quad {
        texture: None,
        dst: [138, 168, 312, 183],
        uv: [0.0; 4],
        color: 0x4011_2233,
        filter: Filter::Point,
        blend: Blend::Alpha,
    };
    assert!(cmds.contains(&bar));
    // Locked (status DISABLE): all entries in DISABLEDTEXTCOLOR, the bar remains.
    let mut h = Harness::new();
    logic(&mut h, common_flags::DISABLE as i32);
    h.build(SRC);
    h.tick();
    let cmds = h.paint();
    assert!(
        glyphs(&h, &cmds)
            .iter()
            .map(color)
            .filter(|c| c & 0x00FF_FFFF != 0)
            .all(|c| c == 0xFF00_00AA)
    );
    assert!(cmds.contains(&bar));
}

// QUIRK O-23: kept
#[test]
fn editfield_caret_blinks_with_the_clock() {
    // re/menu-system 8.4.7: caret only with focus, visible as long as `+0xC8 > now`; if `+0xC8 + 500 < now`, `+0xC8 =
    // now + 500` - in this frame still without caret. Rectangle (C.l + w + 1, C.t, C.l + w + 3, C.t + h).
    let mut h = Harness::new();
    h.build(
        "FRAME F FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
         EDITFIELD E TEXT 1 EXTENT 100 100 400 150 /EDITFIELD\n/FRAME\n",
    );
    let caret = DrawCmd::Quad {
        texture: None,
        dst: [137, 116, 139, 131],
        uv: [0.0; 4],
        color: 0xFFFF_FFFF,
        filter: Filter::Point,
        blend: Blend::Alpha,
    };
    for (t, shown) in [
        (0, false),
        (501, false),
        (502, true),
        (1000, true),
        (1001, false),
        (1501, false),
        (1502, false),
        (1503, true),
    ] {
        h.now = t;
        h.tick();
        assert_eq!(h.paint().contains(&caret), shown, "{t}");
    }
}

#[test]
fn slider_and_radio_follow_their_state() {
    // Slider: thumb at the value of the logic (code 0 -> 3: offset ftol(3 * 364/9) = 121); RadioButton: check mark from
    // the response to code 0 (tile ICONOFFSET + 1).
    let mut h = Harness::new();
    h.rec.answer = Box::new(|m| {
        if m.code == code::QUERY_VALUE {
            let v = if m.element == "s" { 3 } else { 1 };
            m.params.insert("int".into(), Param::Int(v));
            return 0;
        }
        -1
    });
    h.build(
        "FRAME F FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
         SLIDER S EXTENT 0 0 448 100 RESOLUTION 10 THUMBCOLOR FF00FF00 /SLIDER\n\
         RADIOBUTTON R ICONOFFSET 2 EXTENT 0 200 300 251 /RADIOBUTTON\n/FRAME\n",
    );
    h.tick();
    let cmds = h.paint();
    let slider = h.f.tex("data/menü/slider_42x42.tga");
    let thumb: Vec<_> = cmds
        .iter()
        .filter(|c| texture(c) == Some(slider) && color(c) == 0xFF00_FF00)
        .map(dst)
        .collect();
    assert_eq!(thumb, [[21 + 121, 58, 63 + 121, 100]]);
    let radio = h.f.tex("data/menü/buttons_51x51_02.tga");
    let mut want = Vec::new();
    image_quad(
        &h.f.assets,
        radio,
        [51, 51],
        3,
        [0, 200, 51, 251],
        [0, 200, 300, 251],
        0xFFFF_FFFF,
        Filter::Linear,
        &mut want,
    );
    assert!(contains_run(&cmds, &want));
}

#[test]
fn credits_paint_at_the_roll_position() {
    // Credits at the scroll position `_ftol(pos)`: window (C.l, C.t + pos, C.r, C.b + pos); runs of the test file at
    // y = 0, 15, 30 (each 15 high) - at pos = 37.9 only "CD" (30...45) intersects the window and sits at
    // 30 + 2 - 37 = -5 (shadow at -3).
    let mut h = Harness::new();
    h.build(
        "FRAME CREDITS FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
         LABEL SCROLLTEXT CREDITS data/txt/c.txt EXTENT 0 0 1280 1024 /LABEL\n/FRAME\n",
    );
    let id = h.tree.find("CREDITSSCROLLTEXT").unwrap();
    if let NodeKind::Label(s) = &mut h.tree.node_mut(id).kind {
        s.credits.as_mut().unwrap().pos = 37.9;
    }
    h.tick();
    let cmds = h.paint();
    let g = glyphs(&h, &cmds);
    let at: Vec<[i32; 2]> = g.iter().map(|c| [dst(c)[0], dst(c)[1]]).collect();
    assert_eq!(at, [[-2, -3], [8, -3], [0, -5], [10, -5]]);
    assert_eq!(uv0(&g[0]), uv_of(b'C'));
}

#[test]
fn zero_size_screen_paints_and_hits_nothing() {
    // Screen 0 x 0 - no division by 0 when building, in the pass and when drawing; the dispatcher
    // hits nothing.
    let mut h = Harness::new();
    h.screen = Screen {
        width: 0,
        height: 0,
    };
    h.rec.allow = true;
    combo_logic(&mut h, 20);
    h.build(
        "FRAME DLG FLAGS MENUFRAME_DRAWBARL MENUFRAME_DRAWTAGS MENUFRAME_DRAWTITLEL MENUFRAME_MOVE /FLAGS \
         EXTENT -440 212 0 912 ZIELPOSITION 40 212 DURATION 300 BAREXTENT 0 140 550 225 TITLE 2 \
         TITLEEXTENT 0 0 400 100\n\
         COMBOBOX RES FLAGS MENUCOMBOBOX_TEXT /FLAGS BUTTONEXTENT 50 10 250 50 LISTBOXEXTENT 50 60 250 200 \
         /COMBOBOX\n\
         BUTTON OK FLAGS MENUBUTTON_TEXT MENUBUTTON_BORDER /FLAGS TEXT 1 EXTENT 10 60 200 100 /BUTTON\n\
         LISTBOX L EXTENT 10 110 210 202 /LISTBOX\n\
         EDITFIELD E TEXT 1 EXTENT 10 210 300 260 /EDITFIELD\n\
         SLIDER S FLAGS MENUSLIDER_TEXT MENUSLIDER_SCALE /FLAGS TEXT 1 EXTENT 10 270 400 330 /SLIDER\n\
         RADIOBUTTON R FLAGS RADIOBUTTON_TEXT /FLAGS TEXT 1 EXTENT 10 340 200 380 /RADIOBUTTON\n\
         LABEL T FLAGS MENULABEL_AUTOHSCROLL MENULABEL_BACKGROUND /FLAGS TEXT 3 EXTENT 10 390 200 430 /LABEL\n\
         /FRAME\n",
    );
    let frame = h.tree.find("DLG").unwrap();
    open(&mut h, frame, FrameMode::Open);
    for t in [0, 100, 300, 1000] {
        h.now = t;
        h.tick();
        let _ = h.paint();
    }
    h.forget();
    let ok = h.tree.find("DLGOK").unwrap();
    let at = center(&h.tree, ok);
    h.click(at);
    h.click((0, 0));
    assert!(h.rec.msgs.is_empty(), "{:?}", h.rec.msgs);
}

#[test]
fn hidden_frame_still_paints_its_children() {
    // re/menu-system 12.5: HIDE does not act on +0x30 - the frame draws neither border nor decoration, but its
    // children do; they receive no input (the frame is locked).
    let mut h = Harness::new();
    h.rec.allow = true;
    let frame = h.build(
        "FRAME F FLAGS HIDE MENUFRAME_DRAWTAGS /FLAGS EXTENT 100 100 600 500\n\
         BUTTON B EXTENT 10 10 110 50 /BUTTON\n/FRAME\n",
    );
    assert!(h.tree.node(frame).visible && !h.tree.node(frame).enabled);
    h.tick();
    let cmds = h.paint();
    let b = h.tree.find("FB").unwrap();
    let abs = h.tree.abs_rect(b);
    assert_eq!(
        abs,
        [125, 140, 225, 180],
        "in the client of the hidden frame"
    );
    let want = bkg_cmds(&mut h.f, "data/menü/button_normal.bkg", abs, abs, S1280);
    assert_eq!(cmds, want);
    h.forget();
    h.click(center(&h.tree, b));
    assert!(h.rec.msgs.is_empty());
}

#[test]
fn ticks_follow_the_live_child_list() {
    // At 0x0045A515 the successor is read before the visit. If the 0x136 listener builds a new dialog in the tick of a
    // child (appended at the end), the same pass reaches it - unless the triggering child was the last; then only the
    // next one.
    let frame = |name: &str, ms: i32| {
        format!(
            "FRAME {name} FLAGS MENUFRAME_MOVE /FLAGS EXTENT 0 0 100 100 ZIELPOSITION 10 10 DURATION {ms} /FRAME\n"
        )
    };
    for (a_ms, b_ms, same_pass) in [(100, 1000, true), (1000, 100, false)] {
        let mut h = Harness::new();
        let a = h.build(&frame("A", a_ms));
        let b = h.build(&frame("B", b_ms));
        open(&mut h, a, FrameMode::Open);
        open(&mut h, b, FrameMode::Open);
        h.now = 100;
        let new = dialog("FRAME C EXTENT 200 200 300 300 /FRAME\n");
        let mut built = None;
        h.with(|t, _, e| {
            tick_tree(t, e, &mut |t, _, e| {
                built = Some(t.build_dialog(&new, e));
            })
        });
        let c = built.expect("0x136 in the tick");
        let root = h.tree.dialog_root();
        assert_eq!(h.tree.node(root).children.last(), Some(&c));
        let visited_now = |h: &Harness| h.tree.node(c).visit.map(|v| v.0) == Some(h.tree.pass);
        assert_eq!(visited_now(&h), same_pass, "A {a_ms} ms, B {b_ms} ms");
        let menuframe = bkg_cmds(
            &mut h.f,
            "data/menü/menuframe.bkg",
            [200, 200, 300, 300],
            [200, 200, 300, 300],
            S1280,
        );
        assert_eq!(contains_run(&h.paint(), &menuframe), same_pass);
        // In the next pass C is always included.
        h.now = 101;
        h.tick();
        assert!(visited_now(&h));
        assert!(contains_run(&h.paint(), &menuframe));
    }
}

#[test]
fn paint_changes_nothing() {
    // paint_ui is read-only - drawn twice the same, the tree unchanged (queries,
    // scrolling and blinking happen in the pass).
    let mut h = Harness::new();
    combo_logic(&mut h, 20);
    let frame = h.build(START);
    open(&mut h, frame, FrameMode::Open);
    for t in [60, 75] {
        h.now = t;
        h.tick();
    }
    let before = format!("{:?}", h.tree);
    let first = h.paint();
    assert_eq!(h.paint(), first);
    assert_eq!(format!("{:?}", h.tree), before);
    assert!(!first.is_empty());
}
