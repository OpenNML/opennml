//! Drawing from the widget tree (re/menu-system 8.4, 12.2, 12.3, 12.5).
//!
//! The original draws in one pass `Widget_RenderTree` `FUN_0045A420`, which changes the state at the same time: per
//! widget `Tick` (slot 3, fly-in), then - only if visible and with a non-empty `V = abs ∩ clip` - `DrawBackground`
//! and `Draw`, whose beginning issues the queries (`FUN_0045E350` among others), toggles the blinking of the caret,
//! rolls the credits and, at the desktop, advances the image table; afterwards the children over the live child
//! list. The reimplementation splits this into two steps:
//!
//! 1. [`tick_tree`] - everything that changes state, in the order of the original, with listener and effects; each
//!    visited node remembers its `V`.
//! 2. [`paint_ui`] - purely reading: draws exactly the nodes visited in the last [`tick_tree`] with their `V` in
//!    tree order (8.4.12).
//!
//! Deviation: in the original `DrawBackground` comes before the query at the start of `Draw`; if a due query changes
//! `HIDE`, the original draws the background of this one frame still by the old flags, the reimplementation by the
//! new ones.

use nml_formats::dlg::desc::{button_flags, common_flags, frame_flags, label_flags, slider_flags};

use crate::bkg_paint::{LoadedBkg, fill_rect, image_quad, paint_bkg, paint_frame_decor};
use crate::coords::{Screen, to_screen_rect, to_screen_x, to_screen_y};
use crate::dialog::{FONT_BUTTON, UiContext};
use crate::draw::{DrawCmd, Filter, TextureId};
use crate::fonts::FontRef;
use crate::geom::{intersect, offset};
use crate::text::{
    LayoutParams, TextContext, TextLayout, atlas_codes, draw_runs, draw_string, layout_flags,
    layout_text, layout_text_with, measure, text_height,
};
use crate::tree::{FULL, Node, NodeId, NodeKind, UiEnv, UiTree};
use crate::widgets::{self, button, frame, label};

/// Fixed images of the drawing (strings `0x00619429`...`0x0061AAED`).
const ZUSATZ: &str = "data/menü/zusatz_85x85.tga";
const ZUSATZ_MIRRORED: &str = "data/menü/zusatz_85x85_gespiegelt.tga";
/// Arrow of the ComboBox: tile 9 in the 13 x 13 grid (`0x0045EEF1`, `+0x11C`).
const ARROW_TGA: &str = "data/menü/buttons_13x13_normal.tga";
const ARROW_TILE: [i32; 2] = [13, 13];
const ARROW_INDEX: i32 = 9;
/// Slider thumb and scale: tile 6 in the 42 x 42 grid (`0x004655F2`, `+0x94`).
const SLIDER_TGA: &str = "data/menü/slider_42x42.tga";
const SLIDER_TILE: [i32; 2] = [42, 42];
const SLIDER_INDEX: i32 = 6;
/// RadioButton icons in the 51 x 51 grid (`0x00465E02`, `+0x98`).
const RADIO_TGA: &str = "data/menü/buttons_51x51_02.tga";
const RADIO_TILE: [i32; 2] = [51, 51];
/// Title font (frame constructor `0x00461C4D`).
const FONT_TITLE: &str = "MenuTitle";

const WHITE: u32 = 0xFFFF_FFFF;
/// EditField locked: fixed `0xC8A0A0A0` (`0x0045FA1B`, O-75).
const EDIT_DISABLED_COLOR: u32 = 0xC8A0_A0A0;
/// RadioButton locked: `(-(active) & 0x7F7F7F) − 0x7F7F80` = `0xFF808080` (`FUN_00465AA0`).
const RADIO_DISABLED_COLOR: u32 = 0xFF80_8080;
/// Layout switches of Button/ComboBox `0x1C6` (`0x100|0x80|0x40|0x04|0x02`).
const BUTTON_LAYOUT: u32 = layout_flags::CENTER
    | layout_flags::TRUNCATE
    | layout_flags::WIDTH
    | layout_flags::START_COLOR
    | layout_flags::START_FONT;
/// Layout switches of the ListBox entries `0x2C6` (`0x00464616`).
const LIST_LAYOUT: u32 = layout_flags::KEYID
    | layout_flags::TRUNCATE
    | layout_flags::WIDTH
    | layout_flags::START_COLOR
    | layout_flags::START_FONT;
/// Sample for the row height of the ListBox (`0x0061A894`).
const LIST_ROW_SAMPLE: &str = "X";
/// Width of the scrollbar of the ListBox in the grid; the selection bar ends before it (`0x00464571`).
const LIST_SCROLLBAR_W: i32 = 42;
/// Frame flags for which the constructor loads the additional images (`& 0x13E`) or the font `MenuTitle` (`& 0x128`).
const ZUSATZ_FLAGS: u32 = 0x13E;
const TITLE_FONT_FLAGS: u32 = 0x128;
/// Scale of the Slider only at `step > 21.0` (`0x0061A930`, O-74).
const SLIDER_SCALE_MIN_STEP: f32 = 21.0;

// ---------------------------------------------------------------------------------------------------------------
// Pass with state change
// ---------------------------------------------------------------------------------------------------------------

/// Drawing pass without drawing (`FUN_004BCA90` -> `Widget_RenderTree(Desktop, 0, 0, 1280, 1024)`): per node in
/// pre-order `Tick` (Frame: fly-in, [`frame::tick`]), then, if visible (`+0x30`) and with a non-empty
/// `V = abs ∩ clip`, the pre-pass of `Draw` ([`widgets::query`]) and the children with
/// `clip = abs(child) ∩ Client ∩ V` (the rectangle of the child **before** its `Tick`, `0x0045A51E`...`0x0045A52E`;
/// a frame in fly-in thus sees the intersection of its old and new position). Children with an empty `clip` also get
/// their `Tick`.
///
/// The child list is walked live, the successor read **before** the visit (`0x0045A515`): a child appended at the
/// end during the visit is still ticked and drawn in the same pass, unless the visited one was the last. When a
/// movement ends, the frame calls `on_done(tree, frame, env)` (message `0x136`) immediately in its `Tick` - the
/// dialog stack works it off there and may build dialogs or mark them for deletion; marked ones stay in the tree and
/// keep being ticked and drawn (12.1.8).
pub fn tick_tree(
    tree: &mut UiTree,
    env: &mut UiEnv,
    on_done: &mut dyn FnMut(&mut UiTree, NodeId, &mut UiEnv),
) {
    tree.pass = tree.pass.wrapping_add(1);
    tree.pass_now = env.now;
    let desktop = tree.desktop();
    tick_node(tree, desktop, Some(FULL), env, on_done);
}

fn tick_node(
    tree: &mut UiTree,
    id: NodeId,
    clip: Option<[i32; 4]>,
    env: &mut UiEnv,
    on_done: &mut dyn FnMut(&mut UiTree, NodeId, &mut UiEnv),
) {
    if matches!(tree.node(id).kind, NodeKind::Frame(_)) {
        frame::tick(tree, id, env, on_done);
    }
    if !tree.contains(id) || !tree.node(id).visible {
        return;
    }
    let Some(v) = clip.and_then(|c| intersect(tree.abs_rect(id), c)) else {
        return;
    };
    let pass = tree.pass;
    tree.node_mut(id).visit = Some((pass, v));
    widgets::query(tree, id, env);
    if !tree.contains(id) {
        return;
    }
    let inner = intersect(tree.client_rect(id), v);
    // Index instead of `pNext`: this holds only as long as nobody removes children **before** the index from the
    // list during the pass. In the original deletions are only marked during `FUN_0045A420` (12.1.8,
    // `FUN_0045B9E0`); `0x136` may append at the end (dialog stack). A synchronous removal would shift the siblings.
    let mut i = 0;
    while tree.contains(id) {
        let children = &tree.node(id).children;
        let Some(&child) = children.get(i) else {
            break;
        };
        let has_next = i + 1 < children.len();
        if tree.contains(child) {
            let child_clip = inner.and_then(|c| intersect(tree.abs_rect(child), c));
            tick_node(tree, child, child_clip, env, on_done);
        }
        debug_assert!(
            !tree.contains(id) || tree.node(id).children.get(i) == Some(&child),
            "siblings removed synchronously during the drawing pass"
        );
        if !has_next {
            break;
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------------------------

/// Draws the tree purely reading (8.4.12): the nodes visited in the last [`tick_tree`] in tree order, per node
/// `DrawBackground(S(V))` and `Draw(S(V))`, then the children. Desktop: the image table
/// ([`crate::desktop::DesktopImages::paint`]); Frame: border, bars/ornaments/title with the offset of the fly-in;
/// Button: background set, text in the color by lock/mouse/press, ComboBox text from the selection and arrow;
/// ListBox: entries with selection bar and hover blend; Slider: value and scale; RadioButton: check mark;
/// EditField: text and caret; Label: text in the window, image, credits at the roll position. A marked popup or a
/// marked frame is drawn as long as it was visited (12.1.8). `now` is the time of the preceding [`tick_tree`].
pub fn paint_ui(tree: &UiTree, ctx: &mut UiContext, now: u64) -> Vec<DrawCmd> {
    paint_ui_with(tree, ctx, now, None, None)
}

/// Like [`paint_ui`] with menu state for the text layout (`[KEYID n]` in the entries of the ListBox, layout switches
/// `0x2C6`) and the quad of a running video: if `video` is set, the desktop draws this quad instead of its images
/// (`FUN_0045CC80`, vtable slot `0x14` of the menu root; an empty rectangle - window minimized - draws nothing), the
/// children afterwards as usual (a deviation from the original).
pub fn paint_ui_with(
    tree: &UiTree,
    ctx: &mut UiContext,
    now: u64,
    text: Option<&TextContext>,
    video: Option<&DrawCmd>,
) -> Vec<DrawCmd> {
    debug_assert!(
        tree.pass == 0 || tree.pass_now == now,
        "paint_ui at {now} ms after a pass at {} ms",
        tree.pass_now
    );
    let mut p = Painter {
        ctx,
        text,
        video,
        now,
        out: Vec::new(),
    };
    p.node(tree, tree.desktop());
    p.out
}

/// The nodes visited in the last [`tick_tree`] in drawing order with their `V` (grid).
pub(crate) fn visited(tree: &UiTree) -> Vec<(NodeId, [i32; 4])> {
    let mut out = Vec::new();
    collect_visited(tree, tree.desktop(), &mut out);
    out
}

fn collect_visited(tree: &UiTree, id: NodeId, out: &mut Vec<(NodeId, [i32; 4])>) {
    let node = tree.node(id);
    let Some(v) = visit_of(tree, node) else {
        return;
    };
    out.push((id, v));
    for &child in &node.children {
        if tree.contains(child) {
            collect_visited(tree, child, out);
        }
    }
}

fn visit_of(tree: &UiTree, node: &Node) -> Option<[i32; 4]> {
    match node.visit {
        Some((pass, v)) if pass == tree.pass => Some(v),
        _ => None,
    }
}

struct Painter<'p, 'a> {
    ctx: &'p mut UiContext<'a>,
    /// Menu state of the text layout (`[KEYID]`).
    text: Option<&'p TextContext>,
    /// Quad of the running video instead of the desktop images.
    video: Option<&'p DrawCmd>,
    now: u64,
    out: Vec<DrawCmd>,
}

impl Painter<'_, '_> {
    fn screen(&self) -> Screen {
        self.ctx.screen
    }

    fn s(&self, r: [i32; 4]) -> [i32; 4] {
        to_screen_rect(self.ctx.screen, r)
    }

    fn font(&self, name: &str) -> FontRef {
        self.ctx.fonts.by_name(name)
    }

    fn texture(&mut self, path: &str) -> Option<TextureId> {
        let vfs = self.ctx.vfs;
        self.ctx.assets.texture(vfs, path)
    }

    /// Text of an ID (`Texts::display`, O-4); empty without an ID.
    fn text(&self, id: Option<i32>) -> String {
        id.map(|id| self.ctx.texts.display(id)).unwrap_or_default()
    }

    /// `FUN_0045A570` with a background set: `abs` in the grid, `clip` = visible rectangle in pixels.
    fn bkg(&mut self, bkg: Option<&LoadedBkg>, abs: [i32; 4], clip: [i32; 4]) {
        if let Some(bkg) = bkg {
            paint_bkg(
                bkg,
                abs,
                clip,
                self.ctx.screen,
                self.ctx.assets,
                &mut self.out,
            );
        }
    }

    fn quad(
        &mut self,
        path: &str,
        tile: [i32; 2],
        index: i32,
        dst: [i32; 4],
        clip: [i32; 4],
        color: u32,
    ) {
        if let Some(tex) = self.texture(path) {
            image_quad(
                self.ctx.assets,
                tex,
                tile,
                index,
                dst,
                clip,
                color,
                Filter::Linear,
                &mut self.out,
            );
        }
    }

    fn node(&mut self, tree: &UiTree, id: NodeId) {
        let node = tree.node(id);
        let Some(v) = visit_of(tree, node) else {
            return;
        };
        self.widget(tree, id, v);
        for &child in &node.children {
            if tree.contains(child) {
                self.node(tree, child);
            }
        }
    }

    /// `DrawBackground(S(V))` and `Draw(S(V))` per kind.
    fn widget(&mut self, tree: &UiTree, id: NodeId, v: [i32; 4]) {
        let sv = self.s(v);
        let node = tree.node(id);
        match &node.kind {
            NodeKind::Desktop => {
                // `FUN_0045CAF0` draws `levelbackground.bkg` only over the visible world (`FUN_004BA680`; with a
                // video it is invisible); `Draw` the images, with a running video the video instead of them
                // (`FUN_0045CC80`).
                if tree.world_shown && self.video.is_none() {
                    self.bkg(active_set(node), tree.abs_rect(id), sv);
                }
                match self.video {
                    Some(cmd) => {
                        // The video image is always an axis-aligned `Quad` (`crate::video::video_rect`); `Quad4`
                        // does not occur here.
                        let shown = match *cmd {
                            DrawCmd::Quad { dst, .. } => dst[0] < dst[2] && dst[1] < dst[3],
                            DrawCmd::Quad4 { .. } => true,
                        };
                        if shown {
                            self.out.push(*cmd);
                        }
                    }
                    None => {
                        let now = self.now;
                        tree.images.paint(self.ctx, now, &mut self.out);
                    }
                }
            }
            NodeKind::Frame(_) => self.frame(tree, id, sv),
            NodeKind::Button(_) => self.button(tree, id, sv),
            NodeKind::ListBox(_) => self.list(tree, id, sv),
            NodeKind::EditField(_) => self.edit_field(tree, id, sv),
            NodeKind::Slider(_) => self.slider(tree, id, sv),
            NodeKind::Radio(_) => self.radio(tree, id),
            NodeKind::Label(_) => self.label(tree, id, sv),
            // Dialog root, `ComboPopup` and `ModalBlocker` have neither `.bkg` nor their own `Draw`.
            NodeKind::Plain | NodeKind::ComboPopup(_) | NodeKind::ModalBlocker { .. } => {}
        }
    }

    /// Frame (8.4.3): `DrawBackground` `0x00460550` without `HIDE`/`NOFRAME` with the set (`menuframe.bkg` or
    /// `BKGFILE`); `Draw` `FUN_00461520` bars, ornaments and title with the offset `+0x130/+0x134`.
    fn frame(&mut self, tree: &UiTree, id: NodeId, sv: [i32; 4]) {
        let node = tree.node(id);
        let NodeKind::Frame(f) = &node.kind else {
            return;
        };
        let flags = node.flags;
        if flags & (common_flags::HIDE | frame_flags::NOFRAME) == 0 {
            self.bkg(active_set(node), tree.abs_rect(id), sv);
        }
        let zusatz = if flags & ZUSATZ_FLAGS != 0 {
            [self.texture(ZUSATZ), self.texture(ZUSATZ_MIRRORED)]
        } else {
            [None, None]
        };
        let title = (flags & TITLE_FONT_FLAGS != 0 && f.desc.title.is_some())
            .then(|| atlas_codes(&self.text(f.desc.title)));
        let title_font = self.font(FONT_TITLE);
        paint_frame_decor(
            &f.desc,
            flags,
            node.rect,
            f.bar,
            self.ctx.screen,
            zusatz,
            title.as_deref().map(|t| (t, title_font)),
            self.ctx.fonts,
            self.ctx.assets,
            &mut self.out,
        );
    }

    /// Button and ComboBox (8.4.4). `DrawBackground` `FUN_0045DF40`: without `HIDE`; with a loaded active set and
    /// `BORDER` first set 4 (`FUN_0045A050(4)`, only if it exists, otherwise the active one) over the whole
    /// rectangle, then the active one inset by `BORDERSPACE`; otherwise the active one over the whole rectangle.
    /// `Draw` `0x0045EBD0`: open list (O-18) or `HIDE` -> nothing; text only with bit `0x1` - ComboBox with a valid
    /// selection `array[selection]`, otherwise the text of the logic or `TEXT` -, color by [`button::text_color`]
    /// (locked -> `DISABLEDTEXTCOLOR` with an unchanged set, O-91); ComboBox arrow.
    fn button(&mut self, tree: &UiTree, id: NodeId, sv: [i32; 4]) {
        let node = tree.node(id);
        let NodeKind::Button(s) = &node.kind else {
            return;
        };
        let flags = node.flags;
        let abs = tree.abs_rect(id);
        let [l, t, r, b] = abs;
        let [bl, bt, br, bb] = s.desc.border_space;
        if flags & common_flags::HIDE == 0 {
            let active = active_set(node);
            if active.is_some() && flags & button_flags::BORDER != 0 {
                let border = match node.bkg_sets.get(4) {
                    Some(set) => set.as_ref(),
                    None => active,
                };
                self.bkg(border, abs, sv);
                let inset = [
                    l.wrapping_add(bl),
                    t.wrapping_add(bt),
                    r.wrapping_sub(br),
                    b.wrapping_sub(bb),
                ];
                self.bkg(active, inset, sv);
            } else {
                self.bkg(active, abs, sv);
            }
        }
        if s.list_open || flags & common_flags::HIDE != 0 {
            return;
        }
        let combo = flags & button_flags::COMBOBOX != 0;
        if flags & button_flags::TEXT != 0 {
            let selected = usize::try_from(s.selection)
                .ok()
                .filter(|_| combo)
                .and_then(|i| s.items.get(i));
            let text = match selected {
                Some(item) => item.clone(),
                None => s.text.clone().unwrap_or_else(|| self.text(s.desc.text)),
            };
            if !text.is_empty() {
                let sr = self.s(abs);
                let mut layout = TextLayout::default();
                let params = LayoutParams {
                    flags: BUTTON_LAYOUT,
                    width: sr[2].wrapping_sub(sr[0]),
                    font: self.font(FONT_BUTTON),
                    color: button::text_color(node).unwrap_or(s.desc.text_color),
                };
                layout_text(&text, &params, self.ctx.fonts, &mut layout);
                // `0x0045ED56`...`0x0045ED6F`: y = S.t + (S.h - th)/2 with the layout height, C division; with shadow.
                let y =
                    sr[1].wrapping_add(sr[3].wrapping_sub(sr[1]).wrapping_sub(layout.height) / 2);
                draw_runs(
                    &layout,
                    [sr[0], y],
                    true,
                    None,
                    self.ctx.fonts,
                    &mut self.out,
                );
            }
        }
        if combo {
            // Arrow `0x0045EDC9`...`0x0045EE5C`: independent of bit 0x1, rectangle in the grid, clip = `Draw` argument.
            let y1 = t.wrapping_add(bt).wrapping_add(7);
            let x2 = r.wrapping_sub(7).wrapping_sub(br);
            let dst = self.s([x2.wrapping_sub(13), y1, x2, y1.wrapping_add(13)]);
            self.quad(ARROW_TGA, ARROW_TILE, ARROW_INDEX, dst, sv, WHITE);
        }
    }

    /// ListBox (8.4.6, `FUN_00464400`, read from the disassembly): without `HIDE` `comboframe.bkg`, then per row
    /// `i = 0 ... (C.b - C.t)/h(L"X") - 1` the entry `first row + i`. Position from the **current** client `C` in S
    /// (`FUN_00459B30`, `0x00464443`), only the width from the text window `+0x10C...+0x114`
    /// (`0x0046462F`...`0x0046463D`) - the ComboBox list, whose window was set before the move, thus draws at its
    /// moved position. Color: locked `DISABLEDTEXTCOLOR`; otherwise `TEXTCOLOR`, the selection
    /// `SELECTEDTEXTCOLOR`, the row under the mouse per byte `(MOUSEOVERCOLOR + color)/2`. Selection bar (also when
    /// locked) with `SELECTIONBARCOLOR != 0` up to before the scrollbar (`xs(42)`), one row height tall. Text with
    /// shadow (`0x2C6`); `y` advances by the layout height of the entry.
    fn list(&mut self, tree: &UiTree, id: NodeId, sv: [i32; 4]) {
        let node = tree.node(id);
        let NodeKind::ListBox(s) = &node.kind else {
            return;
        };
        if node.flags & common_flags::HIDE != 0 {
            return;
        }
        self.bkg(active_set(node), tree.abs_rect(id), sv);
        let c = self.s(tree.client_rect(id));
        let font = self.font(FONT_BUTTON);
        let lh = text_height(LIST_ROW_SAMPLE, font, self.ctx.fonts);
        let rows = c[3].wrapping_sub(c[1]).checked_div(lh).unwrap_or(0);
        let d = &s.desc;
        let width = s.text_window[2].wrapping_sub(s.text_window[0]);
        let [x, mut y] = [c[0], c[1]];
        for i in 0..rows.max(0) {
            let idx = s.first.wrapping_add(i);
            let Some(item) = usize::try_from(idx).ok().and_then(|k| s.items.get(k)) else {
                continue;
            };
            let color = if !node.enabled {
                d.disabled_text_color
            } else {
                let mut color = d.text_color;
                if idx == s.selection {
                    color = d.selected_text_color;
                }
                if idx == s.hover {
                    color = blend(d.mouse_over_color, color);
                }
                color
            };
            if d.selection_bar_color != 0 && idx == s.selection {
                let sb = if s.scrolling {
                    to_screen_x(self.screen(), LIST_SCROLLBAR_W)
                } else {
                    0
                };
                let r = x.wrapping_add(c[2].wrapping_sub(c[0])).wrapping_sub(sb);
                fill_rect(
                    [x, y, r, y.wrapping_add(lh)],
                    d.selection_bar_color,
                    &mut self.out,
                );
            }
            let mut layout = TextLayout::default();
            let params = LayoutParams {
                flags: LIST_LAYOUT,
                width,
                font,
                color,
            };
            layout_text_with(item, &params, self.ctx.fonts, &mut layout, self.text);
            draw_runs(&layout, [x, y], true, None, self.ctx.fonts, &mut self.out);
            y = y.wrapping_add(layout.height);
        }
    }

    /// EditField (8.4.7, `Draw` `0x0045F9C0`): without `HIDE` `editfield.bkg`, text at `C.lt` via `FUN_00428390` (no
    /// shadow, no cut), color `TEXTCOLOR` (or the color of the logic), locked fixed `0xC8A0A0A0` (O-75); caret, if
    /// the pre-pass has set it: `(C.l + w + 1, C.t, C.l + w + 3, C.t + h)` intersected with `S(V)` in white,
    /// `(w, h)` = size of the text up to the caret.
    fn edit_field(&mut self, tree: &UiTree, id: NodeId, sv: [i32; 4]) {
        let node = tree.node(id);
        let NodeKind::EditField(s) = &node.kind else {
            return;
        };
        if node.flags & common_flags::HIDE != 0 {
            return;
        }
        self.bkg(active_set(node), tree.abs_rect(id), sv);
        let c = self.s(tree.client_rect(id));
        let codes: Vec<u8> = s.current().iter().map(|&u| (u & 0xFF) as u8).collect();
        let color = if node.enabled {
            s.text_color
        } else {
            EDIT_DISABLED_COLOR
        };
        let font = self.font(FONT_BUTTON);
        draw_string(
            &codes,
            [c[0], c[1]],
            color,
            font,
            self.ctx.fonts,
            &mut self.out,
        );
        if s.caret {
            let n = usize::try_from(s.cursor).unwrap_or(0).min(codes.len());
            let [w, h] = measure(&codes[..n], font, self.ctx.fonts);
            let x = c[0].wrapping_add(w);
            let caret = [
                x.wrapping_add(1),
                c[1],
                x.wrapping_add(3),
                c[1].wrapping_add(h),
            ];
            if let Some(r) = intersect(caret, sv) {
                fill_rect(r, WHITE, &mut self.out);
            }
        }
    }

    /// Slider (8.4.8): without `HIDE` `DrawBackground` `0x00464A70` with the strip (42 grid units at the bottom edge
    /// or at the right), `Draw` `FUN_00464D90` at `R >= 2`: scale, thumb at the value, text; color `TEXTCOLOR` when
    /// active, otherwise `DISABLEDTEXTCOLOR`.
    fn slider(&mut self, tree: &UiTree, id: NodeId, sv: [i32; 4]) {
        let node = tree.node(id);
        let NodeKind::Slider(st) = &node.kind else {
            return;
        };
        let flags = node.flags;
        if flags & common_flags::HIDE != 0 {
            return;
        }
        let d = &st.desc;
        let [l, t, r, b] = tree.abs_rect(id);
        let vertical = flags & slider_flags::VERTICAL != 0;
        let strip = if vertical {
            [r.wrapping_sub(42), t, r, b]
        } else {
            [l, b.wrapping_sub(42), r, b]
        };
        self.bkg(active_set(node), strip, sv);
        let resolution = st.resolution;
        if resolution < 2 {
            return;
        }
        // Base and step width as f32 (`0x00464DFD`...`0x00464E60`); offset `ftol(i * step)` with 24-bit x87
        // precision (`FILD; FMUL float; _ftol`) = `f32` product, truncated.
        let (base, length) = if vertical {
            (
                [
                    r.wrapping_sub(42),
                    t.wrapping_add(21),
                    r,
                    t.wrapping_add(63),
                ],
                b.wrapping_sub(t),
            )
        } else {
            (
                [
                    l.wrapping_add(21),
                    b.wrapping_sub(42),
                    l.wrapping_add(63),
                    b,
                ],
                r.wrapping_sub(l),
            )
        };
        let step = length.wrapping_sub(84) as f32 / resolution.wrapping_sub(1) as f32;
        let shifted = |i: i32| {
            let o = (i as f32 * step) as i32;
            if vertical {
                offset(base, [0, o])
            } else {
                offset(base, [o, 0])
            }
        };
        if flags & slider_flags::SCALE != 0 && step > SLIDER_SCALE_MIN_STEP {
            for i in 0..resolution {
                let dst = self.s(shifted(i));
                self.quad(
                    SLIDER_TGA,
                    SLIDER_TILE,
                    SLIDER_INDEX,
                    dst,
                    sv,
                    d.scale_color,
                );
            }
        }
        // Thumb at the value (not limited when drawing).
        let dst = self.s(shifted(st.value));
        self.quad(
            SLIDER_TGA,
            SLIDER_TILE,
            SLIDER_INDEX,
            dst,
            sv,
            d.thumb_color,
        );
        if flags & slider_flags::TEXT != 0 {
            let codes = atlas_codes(&self.text(d.text));
            let font = self.font(FONT_BUTTON);
            let [tw, th] = measure(&codes, font, self.ctx.fonts);
            let center = flags & slider_flags::CENTERTEXT != 0;
            let px = if center { l } else { l.wrapping_add(21) };
            let mut x = to_screen_x(self.screen(), px);
            let y = to_screen_y(self.screen(), t).wrapping_sub(th);
            if center {
                // O-74: grid width minus pixel text width, exact only at width 1280.
                x = x.wrapping_add(r.wrapping_sub(tw).wrapping_sub(l) / 2);
            }
            let color = if node.enabled {
                d.text_color
            } else {
                d.disabled_text_color
            };
            draw_string(&codes, [x, y], color, font, self.ctx.fonts, &mut self.out);
        }
    }

    /// RadioButton (8.4.9): without `HIDE` symbol `ICONOFFSET + check mark` as a square with the pixel height, clip
    /// `S`, color white when active, otherwise `0xFF808080`; text (bit 0x1) at `(S.l + h + 5, S.t + h/2 - th/2)`
    /// always in `TEXTCOLOR` without shadow (O-11).
    fn radio(&mut self, tree: &UiTree, id: NodeId) {
        let node = tree.node(id);
        let NodeKind::Radio(st) = &node.kind else {
            return;
        };
        if node.flags & common_flags::HIDE != 0 {
            return;
        }
        let rd = &st.desc;
        let s = self.s(tree.abs_rect(id));
        let h = s[3].wrapping_sub(s[1]);
        let color = if node.enabled {
            WHITE
        } else {
            RADIO_DISABLED_COLOR
        };
        let dst = [s[0], s[1], s[0].wrapping_add(h), s[3]];
        let index = rd.icon_offset.wrapping_add(i32::from(st.checked));
        self.quad(RADIO_TGA, RADIO_TILE, index, dst, s, color);
        if node.flags & nml_formats::dlg::desc::radio_flags::TEXT != 0 {
            let codes = atlas_codes(&self.text(rd.text));
            let font = self.font(FONT_BUTTON);
            let [_, th] = measure(&codes, font, self.ctx.fonts);
            let x = s[0].wrapping_add(h).wrapping_add(5);
            let y = s[1].wrapping_add(h / 2).wrapping_sub(th / 2);
            draw_string(
                &codes,
                [x, y],
                rd.text_color,
                font,
                self.ctx.fonts,
                &mut self.out,
            );
        }
    }

    /// Label (8.4.5, `Draw` `0x00463120`). Credits (bit `0x8`): the layout at the roll position. Otherwise from
    /// `0x004633AE`: image of the logic (`MENULABEL_TGA`, tile 0 = the whole image - **hypothesis** - stretched to
    /// `S(abs)`); layout [`label::layout_switches`] in the color `+0xA4`, `W` = pixel width (-42 with
    /// `AUTOHSCROLL`); `HIDE` -> end; `MENULABEL_BACKGROUND` fills `S(V)` in `BACKGROUNDCOLOR` (not for value 0, a
    /// deviation from the original); text left-aligned without shadow with the window `(0, y, C.w, C.h + y)` at
    /// `C.lt`.
    fn label(&mut self, tree: &UiTree, id: NodeId, sv: [i32; 4]) {
        let node = tree.node(id);
        let NodeKind::Label(st) = &node.kind else {
            return;
        };
        let flags = node.flags;
        if flags & label_flags::CREDITS != 0 {
            self.credits(tree, id);
            return;
        }
        let s = self.s(tree.abs_rect(id));
        if flags & label_flags::TGA != 0
            && let Some(tga) = &st.tga
            && let Some(tex) = self.texture(tga)
        {
            let img = self.ctx.assets.image(tex);
            let tile = [img.width() as i32, img.height() as i32];
            image_quad(
                self.ctx.assets,
                tex,
                tile,
                0,
                s,
                s,
                WHITE,
                Filter::Linear,
                &mut self.out,
            );
        }
        let mut width = s[2].wrapping_sub(s[0]);
        if flags & label_flags::AUTOHSCROLL != 0 {
            width = width.wrapping_sub(42);
        }
        let mut layout = TextLayout::default();
        let params = LayoutParams {
            flags: label::layout_switches(flags),
            width,
            font: self.font(FONT_BUTTON),
            color: st.color,
        };
        // With menu state so that `[CURLEVEL...]` inserts (`FUN_00560FF0` without a switch check); `[KEYID]` stays
        // empty without 0x200.
        layout_text_with(
            &label::label_text(st, self.ctx.texts),
            &params,
            self.ctx.fonts,
            &mut layout,
            self.text,
        );
        if flags & common_flags::HIDE != 0 {
            return;
        }
        if flags & label_flags::BACKGROUND != 0 {
            fill_rect(sv, st.desc.background_color.unwrap_or(0), &mut self.out);
        }
        let c = self.s(tree.client_rect(id));
        let y = st.window_y;
        let window = [
            0,
            y,
            c[2].wrapping_sub(c[0]),
            c[3].wrapping_sub(c[1]).wrapping_add(y),
        ];
        draw_runs(
            &layout,
            [c[0], c[1]],
            false,
            Some(window),
            self.ctx.fonts,
            &mut self.out,
        );
    }

    /// Credits (`0x004632C2`...`0x00463340`): `FUN_0055FDA0(C.l - 2, C.t + 2, window (C.l, C.t + pos, C.r, C.b + pos),
    /// shadow 1)` with `pos = _ftol(+0x110)`: every run that intersects the window, at `(run.x - 2, run.y + 2 -
    /// pos)`, text with shadow. No background, no `TEXT`; if the credits file is missing, nothing is drawn.
    fn credits(&mut self, tree: &UiTree, id: NodeId) {
        let NodeKind::Label(st) = &tree.node(id).kind else {
            return;
        };
        let Some(roll) = &st.credits else {
            return;
        };
        let c = self.s(tree.client_rect(id));
        let pos = roll.pos as i32;
        let window = [c[0], c[1].wrapping_add(pos), c[2], c[3].wrapping_add(pos)];
        let origin = [c[0].wrapping_sub(2), c[1].wrapping_add(2)];
        draw_runs(
            &roll.layout,
            origin,
            true,
            Some(window),
            self.ctx.fonts,
            &mut self.out,
        );
    }
}

/// The active background set (`+0x70...+0x90`); `None` if it is missing.
fn active_set(node: &Node) -> Option<&LoadedBkg> {
    node.bkg_sets.get(node.bkg_set).and_then(Option::as_ref)
}

/// Hover blend of the ListBox (`0x004644E4`...`0x00464550`): per byte `(a + b) / 2` rounded down.
fn blend(a: u32, b: u32) -> u32 {
    let byte = |v: u32, shift: u32| (v >> shift) & 0xFF;
    [0, 8, 16, 24]
        .into_iter()
        .map(|sh| ((byte(a, sh) + byte(b, sh)) / 2) << sh)
        .fold(0, |acc, v| acc | v)
}

#[cfg(test)]
mod tests;
