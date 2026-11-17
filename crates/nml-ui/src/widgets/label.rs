//! `MenuLabel` (vtable `0x0061A7F0`, re/menu-system 8.4.5, 12.6; O-89, O-105): text, text array, image, vertical
//! scroll bar (`AUTOHSCROLL`) and the credits (`CREDITS`) with scrolling, image changes and end.

use nml_formats::dlg::desc::label_flags;
use nml_formats::dlg::{LabelDesc, SliderDesc};

use super::{
    Slot, base_mouse, button_font, client_screen, due, int_param, line_height, lock, notify,
    query_head, shared_param, slider, text_param,
};
use crate::assets::Texts;
use crate::coords::to_screen_rect;
use crate::dialog::{CREDITS_COLOR, FONT_CREDITS, LABEL_LAYOUT, read_credits};
use crate::msg::{Param, SharedText, code};
use crate::text::{LayoutParams, TextLayout, layout_flags, layout_text};
use crate::tree::{Broadcast, NodeId, NodeKind, UiEnv, UiTree};

/// Credits: minimum duration in ms (`FUN_00462AF0`: `max(120000, music length)`).
const CREDITS_MIN_MS: i32 = 120_000;
/// Fade-in and fade-out of the credits images in ms (`0x15E`/`0x15F {2, 500, name}`).
const CREDITS_FADE_MS: u32 = 500;
/// Scroll bar: 42 grid units wide, 32 grid units beyond the label at top and bottom; wrap width -42 S.
const SCROLLBAR_W: i32 = 42;
const SCROLLBAR_OVERHANG: i32 = 32;

/// Scrolling credits (`FUN_00462AF0`, `Draw` `0x00463120`...`0x00463392`).
#[derive(Debug, Clone, PartialEq)]
pub struct CreditsRoll {
    /// `+0x110`: scroll position in screen pixels (drawn truncated, `_ftol`).
    pub pos: f64,
    /// `+0x120`: end = total height of the layout; after the end broadcast `end^2` (O-105).
    pub end: f64,
    /// `+0x118`: pixels per ms = height / duration.
    pub speed: f64,
    /// Index of the next image change in [`Self::images`] (lists `+0x130`/`+0x14C`).
    pub next_image: usize,
    /// `+0x128`: position from which (strictly greater) the next image comes; without further height `end^2`.
    pub switch_at: f64,
    /// Image changes in file order: name and the height laid out up to then.
    pub images: Vec<(String, i32)>,
    /// `+0x168`: the image shown.
    pub shown: Option<String>,
    /// `+0x190`: time of the last step (ms of the program clock).
    pub last: u64,
    /// `+0x16C`: the laid-out layout (font "Credits", `0xC8FFFFFF`, width of the client in S).
    pub layout: TextLayout,
}

/// State of a `MenuLabel` (fields from `+0x94`).
#[derive(Debug, Clone)]
pub struct LabelState {
    pub desc: LabelDesc,
    /// `+0x98`: text of the logic (code 7 `text`, as a copy); `None` -> `TEXT` of the file.
    pub text: Option<String>,
    /// `+0x98` as pointer: shared buffer of the logic (code 7 `text` as [`SharedText`]), read when drawing. Takes
    /// precedence over [`Self::text`].
    pub shared: Option<SharedText>,
    /// `+0xA4`: text color (`TEXTCOLOR`, response `color`).
    pub color: u32,
    /// Image of the logic (`tga` in the response to code 5, only with `MENULABEL_TGA`).
    pub tga: Option<String>,
    /// `+0xE0`: text array of the logic (code 2 `array`, only with `MENULABEL_TEXTARRAY`).
    pub items: Vec<String>,
    /// Top edge of the text window `+0xD0` in S (`v * h(L"O")` after `0x140`; window `(0, y, C.w, C.h + y)`).
    pub window_y: i32,
    /// `+0x94`: next query time; `None` = never again.
    pub query_at: Option<u64>,
    /// Credits (bit `0x8`), once `OnAttach` has read the file.
    pub credits: Option<CreditsRoll>,
}

impl LabelState {
    /// Text of the logic: the shared buffer at the present time, otherwise the copy.
    pub fn logic_text(&self) -> Option<String> {
        match &self.shared {
            Some(shared) => Some(shared.get()),
            None => self.text.clone(),
        }
    }

    /// Constructor `FUN_00462F90`: color from the file, query due.
    pub fn new(desc: LabelDesc) -> LabelState {
        LabelState {
            color: desc.text_color,
            desc,
            text: None,
            shared: None,
            tga: None,
            items: Vec::new(),
            window_y: 0,
            query_at: Some(0),
            credits: None,
        }
    }
}

fn state(tree: &UiTree, id: NodeId) -> &LabelState {
    match &tree.node(id).kind {
        NodeKind::Label(s) => s,
        _ => panic!("widget {id:?} is not a Label"),
    }
}

fn state_mut(tree: &mut UiTree, id: NodeId) -> &mut LabelState {
    match &mut tree.node_mut(id).kind {
        NodeKind::Label(s) => s,
        _ => panic!("widget {id:?} is not a Label"),
    }
}

/// Query `FUN_00462490` (from `OnAttach` and `Draw`): codes 9, 5 (with `MENULABEL_TGA` from the same response
/// `tga` -> image), lock from the flags; with `TEXTARRAY` code 2 (`array`, cleared beforehand), otherwise code 7
/// (`text`, `color`).
pub(crate) fn query(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if !due(state(tree, id).query_at, env.now) {
        return;
    }
    state_mut(tree, id).query_at = None;
    let (next, status) = query_head(tree, id, env);
    state_mut(tree, id).query_at = next;
    let flags = tree.node(id).flags;
    if flags & label_flags::TGA != 0
        && let Some(tga) = text_param(&status, "tga")
    {
        state_mut(tree, id).tga = Some(tga);
    }
    lock(tree.node_mut(id));
    if flags & label_flags::TEXTARRAY != 0 {
        state_mut(tree, id).items.clear();
        let (_, msg) = notify(tree, id, code::QUERY_ITEMS, &[], env);
        if let Some(Param::Array(items)) = msg.params.get("array") {
            state_mut(tree, id).items = items.clone();
        }
    } else {
        let (_, msg) = notify(tree, id, code::QUERY_TEXT, &[], env);
        let s = state_mut(tree, id);
        // `+0x98 = *text` (`0x004626E4`): the original remembers the pointer. A shared buffer stays shared, an
        // ordinary text is copied.
        if let Some(shared) = shared_param(&msg, "text") {
            s.shared = Some(shared);
            s.text = None;
        } else if let Some(text) = text_param(&msg, "text") {
            s.text = Some(text);
            s.shared = None;
        }
        if let Some(color) = int_param(&msg, "color") {
            s.color = color as u32;
        }
    }
}

/// The laid-out text (`FUN_00462980`): with `TEXTARRAY` the text array concatenated (`wcscat`), otherwise the text of
/// the logic or `TEXT` of the file.
pub(crate) fn label_text(s: &LabelState, texts: &Texts) -> String {
    if s.desc.flags & label_flags::TEXTARRAY != 0 {
        return s.items.concat();
    }
    s.logic_text()
        .unwrap_or_else(|| s.desc.text.map(|t| texts.display(t)).unwrap_or_default())
}

/// Layout switches of the label (`FUN_00462980`): `0x46`; `NOLINEBREAK` sets `0x80` (truncate instead of wrap) only
/// **without** `TEXTARRAY` (`if ((flags & 2) == 0) { if (flags & 0x10) |= 0x80; ... }`).
pub(crate) fn layout_switches(flags: u32) -> u32 {
    if flags & label_flags::TEXTARRAY == 0 && flags & label_flags::NOLINEBREAK != 0 {
        LABEL_LAYOUT | layout_flags::TRUNCATE
    } else {
        LABEL_LAYOUT
    }
}

/// `OnAttach` `0x004634B0`: delete all children immediately; with credits only [`start_credits`]. Otherwise query,
/// window top 0 and with `AUTOHSCROLL` a nameless **vertical** slider (flags `0x28`, `RESOLUTION = H / h(L"D")`
/// with `H` = height of the layout - one scrolls until the last line is at the top), 42 grid units wide at the right
/// client edge and 32 grid units beyond the label at top and bottom; only with `h > 0` and `H > 0`.
pub(crate) fn on_attach(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    tree.remove_children_now(id, env);
    if tree.node(id).flags & label_flags::CREDITS != 0 {
        start_credits(tree, id, env);
        return;
    }
    query(tree, id, env);
    state_mut(tree, id).window_y = 0;
    let flags = tree.node(id).flags;
    if flags & label_flags::AUTOHSCROLL == 0 {
        return;
    }
    // Layout `FUN_00462980`: switches `0x46` (+`0x80` with `NOLINEBREAK` without `TEXTARRAY`), width `S(abs)` - 42.
    let abs = to_screen_rect(env.ui.screen, tree.abs_rect(id));
    let params = LayoutParams {
        flags: layout_switches(flags),
        width: abs[2].wrapping_sub(abs[0]).wrapping_sub(SCROLLBAR_W),
        font: button_font(env),
        color: state(tree, id).color,
    };
    let mut layout = TextLayout::default();
    layout_text(
        &label_text(state(tree, id), env.ui.texts),
        &params,
        env.ui.fonts,
        &mut layout,
    );
    let total = layout.height;
    let lh = line_height(env, "D");
    if lh <= 0 || total <= 0 {
        return;
    }
    let c = tree.client_rect(id);
    let (w, h) = (c[2].wrapping_sub(c[0]), c[3].wrapping_sub(c[1]));
    let desc = SliderDesc {
        flags: slider::SCROLLBAR_FLAGS,
        resolution: total / lh,
        extent: [
            w.wrapping_sub(SCROLLBAR_W),
            -SCROLLBAR_OVERHANG,
            w,
            h.wrapping_add(SCROLLBAR_OVERHANG),
        ],
        ..SliderDesc::default()
    };
    tree.add_child(id, slider::new_node(desc), env);
}

/// Attaching in the file state (static drawing, [`super::attach_static`]): the credits as when attaching (layout,
/// first image), otherwise only the query - without scroll bar.
pub(crate) fn attach_static(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if tree.node(id).flags & label_flags::CREDITS != 0 {
        start_credits(tree, id, env);
    } else {
        query(tree, id, env);
    }
}

/// Credits at attach (`FUN_00462AF0`): lay out the file line by line (layout call `0x46` per line in "Credits",
/// `0xC8FFFFFF`, width of the client in S; empty line `" [CR]"`), at each `[CHANGEBACKGROUND ...]` remember the height
/// laid out so far. Then broadcast `0x162` from the root (music length, 0 without a response), duration
/// `max(120000, length)`, scroll position 0, end = height, speed = height / duration, next change at
/// `height^2` (32-bit product); if there are image changes, the first comes **immediately** (`0x15E {500 ms}` from
/// the root) and the next change lies at the height of the second. If the file is missing, nothing happens
/// (`GameFile_Open`).
/// Destructor `FUN_004622A0` (re/menu-system 8.4.5): with the credits bit (`+0x9C & 8`) broadcast
/// `0x15F {2, 500, +0x168}` from the top root (`FUN_00459420`) - the shown image of the credits fades out. The
/// destructor runs when the children of the frame are torn down (`FUN_00459860`), while frame and label are still
/// attached; the broadcast thus reaches the desktop. The condition is only the bit: even without a readable credits
/// file or without image changes the broadcast goes out with an empty `+0x168` (the desktop finds no entry for it and
/// loads nothing, no effect). Called by [`UiTree`] on deletion, before the children of the label fall.
pub(crate) fn destroyed(tree: &mut UiTree, flags: u32, state: &LabelState, env: &mut UiEnv) {
    if flags & label_flags::CREDITS == 0 {
        return;
    }
    let path = state
        .credits
        .as_ref()
        .and_then(|roll| roll.shown.clone())
        .unwrap_or_default();
    let desktop = tree.desktop();
    tree.broadcast(
        desktop,
        &Broadcast::HideImage {
            path,
            ms: CREDITS_FADE_MS,
        },
        env,
    );
}

fn start_credits(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    let Some(path) = state(tree, id).desc.credits.clone() else {
        return;
    };
    let Some(file) = read_credits(env.ui.vfs, &path) else {
        return;
    };
    let c = client_screen(tree, id, env);
    let params = LayoutParams {
        flags: LABEL_LAYOUT,
        width: c[2].wrapping_sub(c[0]),
        font: env.ui.fonts.by_name(FONT_CREDITS),
        color: CREDITS_COLOR,
    };
    let mut layout = TextLayout::default();
    let mut images = Vec::new();
    let mut backgrounds = file.backgrounds.iter().peekable();
    for (i, line) in file.lines.iter().enumerate() {
        while let Some((_, name)) = backgrounds.next_if(|(at, _)| *at == i) {
            images.push((name.clone(), layout.height));
        }
        layout_text(line, &params, env.ui.fonts, &mut layout);
    }
    images.extend(backgrounds.map(|(_, name)| (name.clone(), layout.height)));
    let height = layout.height;

    let desktop = tree.desktop();
    tree.take_reply();
    tree.broadcast(desktop, &Broadcast::MusicLength, env);
    let music = tree.take_reply().unwrap_or(0);
    let duration = if music > CREDITS_MIN_MS - 1 {
        music
    } else {
        CREDITS_MIN_MS
    };
    let mut roll = CreditsRoll {
        pos: 0.0,
        end: f64::from(height),
        speed: f64::from(height) / f64::from(duration),
        next_image: 0,
        switch_at: f64::from(height.wrapping_mul(height)),
        images,
        shown: None,
        last: env.now,
        layout,
    };
    let mut first = None;
    if let Some((name, at)) = roll.images.first().cloned() {
        roll.switch_at = f64::from(at);
        roll.shown = Some(name.clone());
        roll.next_image = 1;
        if let Some((_, at)) = roll.images.get(1) {
            roll.switch_at = f64::from(*at);
        }
        first = Some(name);
    }
    state_mut(tree, id).credits = Some(roll);
    if let Some(path) = first {
        tree.broadcast(
            desktop,
            &Broadcast::ShowImage {
                path,
                ms: CREDITS_FADE_MS,
            },
            env,
        );
    }
}

/// Pre-pass of `Draw` (`0x00463120`): with credits one scroll step, otherwise the query. Scroll step:
/// `pos += (now - last) * speed`; if `pos` is strictly greater than the next change and there is still an image,
/// `0x15F` for the shown one and `0x15E` for the next (500 ms each, from the root), next change at the height of the
/// one after next or `end^2`. If afterwards `pos > end`, broadcast `0x17E` (like ESC) from the root and
/// `end = end^2` - the end comes only once (O-105). Drawing happens between the change and the end, but changes
/// nothing.
///
/// Deviation: the original computes `dt * speed + pos` in the 80-bit register of the x87 FPU and rounds only when
/// storing; the reimplementation computes in `f64` (fixed order, deterministic).
pub(crate) fn before_draw(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if state(tree, id).credits.is_none() {
        if tree.node(id).flags & label_flags::CREDITS == 0 {
            query(tree, id, env);
        }
        return;
    }
    let now = env.now;
    let mut switch = None;
    let mut ended = false;
    if let Some(roll) = state_mut(tree, id).credits.as_mut() {
        let dt = now.saturating_sub(roll.last) as f64;
        roll.last = now;
        roll.pos += dt * roll.speed;
        if roll.pos > roll.switch_at && roll.next_image < roll.images.len() {
            let old = roll.shown.take();
            let new = roll.images[roll.next_image].0.clone();
            roll.shown = Some(new.clone());
            roll.next_image += 1;
            roll.switch_at = match roll.images.get(roll.next_image) {
                Some((_, at)) => f64::from(*at),
                None => roll.end * roll.end,
            };
            switch = Some((old, new));
        }
        if roll.pos > roll.end {
            roll.end *= roll.end;
            ended = true;
        }
    }
    let desktop = tree.desktop();
    if let Some((old, new)) = switch {
        tree.broadcast(
            desktop,
            &Broadcast::HideImage {
                path: old.unwrap_or_default(),
                ms: CREDITS_FADE_MS,
            },
            env,
        );
        tree.broadcast(
            desktop,
            &Broadcast::ShowImage {
                path: new,
                ms: CREDITS_FADE_MS,
            },
            env,
        );
    }
    if ended {
        tree.broadcast(desktop, &Broadcast::Escape, env);
    }
}

/// Mouse slots: wheel (slots 1/0 `0x00461EE0`/`0x00461F30`) to **all** children without a visibility check (unlike
/// the ListBox), first `true` wins, `false` without children; the others like the base.
pub(crate) fn mouse_slot(
    tree: &mut UiTree,
    id: NodeId,
    slot: Slot,
    pt: [i32; 2],
    env: &mut UiEnv,
) -> bool {
    if !matches!(slot, Slot::WheelUp | Slot::WheelDown) {
        return base_mouse(slot);
    }
    for child in tree.node(id).children.clone() {
        if tree.contains(child) && super::mouse_slot(tree, child, slot, pt, env) {
            return true;
        }
    }
    false
}

/// `OnMessage` `0x00461E20`: `0x140` from the scroll bar -> window by `v * h(L"O")` (line by line); `0x12E` code 2 ->
/// `OnAttach` again (text, scroll bar anew, window at top); code 5 -> query at the next `Draw`; each taken.
pub(crate) fn on_message(tree: &mut UiTree, id: NodeId, msg: &Broadcast, env: &mut UiEnv) -> bool {
    match msg {
        Broadcast::ScrollFirst(v) => {
            let lh = line_height(env, "O");
            state_mut(tree, id).window_y = v.wrapping_mul(lh);
            true
        }
        Broadcast::RequeryItems => {
            state_mut(tree, id).query_at = Some(0);
            on_attach(tree, id, env);
            true
        }
        Broadcast::Requery => {
            state_mut(tree, id).query_at = Some(0);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::UiInput;
    use crate::msg::UiEffect;
    use crate::widgets::Trace;
    use crate::widgets::slider::SliderState;
    use crate::widgets::testkit::Harness;

    /// Credits of the test data (`data/txt/c.txt`): image `bg2`, empty line, `AB\u{E4}`, image `bg`, `CD`; plus an
    /// `OK` with `CHECKESCAPE` as in `credits.dlg`.
    const CREDITS: &str = "FRAME CREDITS FLAGS MENUFRAME_NOFRAME MENUFRAME_MUSIC /FLAGS EXTENT 0 0 1280 1024\n\
         LABEL SCROLLTEXT CREDITS data/txt/c.txt EXTENT 0 0 1280 1024 /LABEL\n\
         BUTTON OK FLAGS MENUBUTTON_CHECKESCAPE /FLAGS EXTENT 0 0 10 10 /BUTTON\n/FRAME\n";

    const BG2: &str = "data/menü/bg2.tga";
    const BG: &str = "data/menü/bg.tga";

    /// Heights of the credits layout after each line (layout calls like `FUN_00462AF0`, 1280 pixels wide).
    fn heights(h: &mut Harness) -> Vec<i32> {
        h.with(|_, _, env| {
            let params = LayoutParams {
                flags: LABEL_LAYOUT,
                width: 1280,
                font: env.ui.fonts.by_name(FONT_CREDITS),
                color: CREDITS_COLOR,
            };
            let mut layout = TextLayout::default();
            [" [CR]", "AB\u{E4}", "CD"]
                .iter()
                .map(|line| {
                    layout_text(line, &params, env.ui.fonts, &mut layout);
                    layout.height
                })
                .collect()
        })
    }

    fn roll(h: &Harness) -> &CreditsRoll {
        let id = h.tree.find("CREDITSSCROLLTEXT").unwrap();
        match &h.tree.node(id).kind {
            NodeKind::Label(s) => s.credits.as_ref().expect("credits"),
            _ => unreachable!(),
        }
    }

    fn tick(h: &mut Harness, now: u64) {
        h.now = now;
        let id = h.tree.find("CREDITSSCROLLTEXT").unwrap();
        h.query(id);
    }

    fn desktop_msgs(h: &Harness) -> Vec<Broadcast> {
        h.tree
            .trace
            .iter()
            .filter_map(|t| match t {
                Trace::Desktop(b) => Some(b.clone()),
                _ => None,
            })
            .collect()
    }

    fn show(path: &str) -> Broadcast {
        Broadcast::ShowImage {
            path: path.into(),
            ms: 500,
        }
    }

    #[test]
    fn credits_roll_with_the_clock_and_switch_images() {
        // O-89: scrolling with the program clock, image change at the remembered height; duration max(120000,
        // music length).
        let mut h = Harness::new();
        let hs = heights(&mut h);
        let (h2, total) = (hs[1], hs[2]);
        assert!(0 < h2 && h2 < total);
        h.tree.set_music_length(240_000);
        h.now = 1000;
        h.build(CREDITS);
        let r = roll(&h);
        assert_eq!(
            r.images,
            [(BG2.to_string(), 0), (BG.to_string(), h2)],
            "height before the first or after two lines"
        );
        assert_eq!(r.end, f64::from(total));
        assert_eq!(r.speed, f64::from(total) / 240_000.0);
        assert_eq!((r.pos, r.last), (0.0, 1000));
        // The first image comes immediately; the next change lies at the height of the second.
        assert_eq!(r.shown.as_deref(), Some(BG2));
        assert_eq!((r.next_image, r.switch_at), (1, f64::from(h2)));
        // The frame (`MUSIC` without a path) reports the music first, then the label asks for the length.
        let music = Broadcast::Music {
            path: String::new(),
            looping: true,
        };
        assert_eq!(desktop_msgs(&h), [music, Broadcast::MusicLength, show(BG2)]);
        // The labels run without a query: no code for the credits label.
        assert!(h.codes("scrolltext").is_empty());

        let at = f64::from(h2) * 240_000.0 / f64::from(total);
        h.tree.trace.clear();
        tick(&mut h, 1000);
        assert_eq!(roll(&h).pos, 0.0, "dt = 0");
        let before = 1000 + at.floor() as u64 - 10;
        tick(&mut h, before);
        assert!(roll(&h).pos < f64::from(h2));
        assert_eq!(roll(&h).shown.as_deref(), Some(BG2));
        let after = 1000 + at.ceil() as u64 + 10;
        tick(&mut h, after);
        let r = roll(&h);
        assert!(r.pos > f64::from(h2));
        assert!((r.pos - (after - 1000) as f64 * r.speed).abs() < 1e-9);
        assert_eq!(r.shown.as_deref(), Some(BG));
        assert_eq!(r.next_image, 2);
        assert_eq!(r.switch_at, r.end * r.end, "no further image");
        assert_eq!(
            desktop_msgs(&h),
            [
                Broadcast::HideImage {
                    path: BG2.into(),
                    ms: 500
                },
                show(BG)
            ]
        );
        // Without music the minimum duration of 120000 ms applies.
        let mut h = Harness::new();
        h.build(CREDITS);
        assert_eq!(roll(&h).speed, f64::from(total) / 120_000.0);
    }

    #[test]
    fn credits_end_broadcasts_escape_once() {
        // O-105: `pos > end` -> broadcast 0x17E from the root (taken by the `OK` with CHECKESCAPE), then `end^2`.
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(CREDITS);
        let total = roll(&h).end;
        h.forget();
        tick(&mut h, 60_000);
        assert!(h.codes("ok").is_empty());
        tick(&mut h, 120_001);
        assert_eq!(h.codes("ok"), [code::TRIGGERED]);
        assert_eq!(roll(&h).end, total * total);
        tick(&mut h, 200_000);
        tick(&mut h, 240_000);
        assert_eq!(h.codes("ok"), [code::TRIGGERED], "only once");
        assert!(
            !h.fx.iter().any(|e| matches!(e, UiEffect::PlaySfx { .. })),
            "like ESC: without click sound"
        );
    }

    #[test]
    fn deleting_the_credits_label_fades_out_its_image() {
        // Destructor `FUN_004622A0`: with the credits bit broadcast `0x15F {2, 500, +0x168}` from the top root (the
        // frame is still attached to the desktop while the children are torn down). Without it, the shown image of
        // the credits would remain fully in the image table after closing (`OPTIONS` after the credits).
        let mut h = Harness::new();
        h.build(CREDITS);
        assert_eq!(roll(&h).shown.as_deref(), Some(BG2));
        let frame = h.tree.find("CREDITS").unwrap();
        let root = h.tree.dialog_root();
        h.tree.schedule_delete(root, frame);
        h.tree.trace.clear();
        h.now = 5000;
        h.input(UiInput::Frame);
        assert!(!h.tree.contains(frame));
        assert_eq!(
            desktop_msgs(&h),
            [Broadcast::HideImage {
                path: BG2.into(),
                ms: 500
            }]
        );
        // An ordinary label reports nothing on deletion.
        let frame = h.build(
            "FRAME BACK FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
             LABEL T EXTENT 0 0 80 40 /LABEL\n/FRAME\n",
        );
        h.tree.schedule_delete(root, frame);
        h.tree.trace.clear();
        h.input(UiInput::Frame);
        assert!(!h.tree.contains(frame));
        assert!(desktop_msgs(&h).is_empty());
        // Credits bit without a readable file (no image shown): the broadcast goes out with an empty name as in the
        // original (`+0x168` empty); the desktop creates no entry for it.
        let frame = h.build(
            "FRAME NOFILE FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024
             LABEL T CREDITS data/txt/missing.txt EXTENT 0 0 80 40 /LABEL
/FRAME
",
        );
        let label = h.tree.find("NOFILET").unwrap();
        let NodeKind::Label(s) = &h.tree.node(label).kind else {
            unreachable!()
        };
        assert!(s.credits.as_ref().is_none_or(|r| r.shown.is_none()));
        h.tree.schedule_delete(root, frame);
        h.tree.trace.clear();
        h.input(UiInput::Frame);
        assert!(!h.tree.contains(frame));
        assert_eq!(
            desktop_msgs(&h),
            [Broadcast::HideImage {
                path: String::new(),
                ms: 500
            }]
        );
        assert!(!h.tree.images.contains(""));
    }

    #[test]
    fn textarray_ignores_nolinebreak() {
        // FUN_00462980: `0x80` (truncate) only without TEXTARRAY. With TEXTARRAY "AB CD" wraps in 80 - 42 = 38 pixels
        // (two lines -> scroll bar with RESOLUTION 2); truncated it would be one line (RESOLUTION 1, invisible).
        assert_eq!(
            layout_switches(label_flags::NOLINEBREAK),
            LABEL_LAYOUT | layout_flags::TRUNCATE
        );
        assert_eq!(
            layout_switches(label_flags::NOLINEBREAK | label_flags::TEXTARRAY),
            LABEL_LAYOUT
        );
        let mut h = Harness::new();
        h.rec.answer = Box::new(|m| {
            if m.code == code::QUERY_ITEMS {
                let items = ["AB ", "CD"].map(String::from).to_vec();
                m.params.insert("array".into(), Param::Array(items));
                return 0;
            }
            -1
        });
        h.build(
            "FRAME BACK FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
             LABEL T FLAGS MENULABEL_AUTOHSCROLL MENULABEL_TEXTARRAY MENULABEL_NOLINEBREAK /FLAGS \
             EXTENT 0 0 80 40 /LABEL\n/FRAME\n",
        );
        let label = h.tree.find("BACKT").unwrap();
        let bar = h.tree.node(label).children[0];
        let NodeKind::Slider(SliderState { resolution, .. }) = h.tree.node(bar).kind else {
            panic!("no scroll bar");
        };
        assert_eq!(resolution, 2);
        assert!(h.tree.node(bar).visible);
    }

    #[test]
    fn wheel_falls_through_the_modal_blocker() {
        // O-93: under a modal dialog the wheel reaches the label with `AUTOHSCROLL`; its nameless scroll bar reports
        // `0x140`, the window advances by one line `h(L"O")`.
        let mut h = Harness::new();
        h.build(
            "FRAME BACK FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
             LABEL T FLAGS MENULABEL_AUTOHSCROLL /FLAGS TEXT 3 EXTENT 0 0 80 40 /LABEL\n/FRAME\n",
        );
        let label = h.tree.find("BACKT").unwrap();
        // "AB CD" in 80 - 42 = 38 pixels: two lines, 30 high -> RESOLUTION 30 / 15 = 2, visible.
        let bar = h.tree.node(label).children[0];
        let NodeKind::Slider(SliderState { resolution, .. }) = h.tree.node(bar).kind else {
            panic!("no scroll bar");
        };
        assert_eq!(resolution, 2);
        assert!(h.tree.node(bar).visible);
        assert_eq!(h.tree.node(bar).rect, [38, -32, 80, 72]);
        h.build(
            "FRAME MSGBOX FLAGS MENUFRAME_MODAL /FLAGS EXTENT 700 600 900 800\n\
             BUTTON YES EXTENT 0 0 50 50 /BUTTON\n/FRAME\n",
        );
        h.input(UiInput::Move { x: 10, y: 10 });
        h.forget();
        assert!(h.input(UiInput::WheelDown));
        let NodeKind::Label(s) = &h.tree.node(label).kind else {
            unreachable!()
        };
        assert_eq!(s.window_y, 15);
        assert!(h.rec.msgs.is_empty());
        // Wheel up scrolls back.
        h.input(UiInput::WheelUp);
        let NodeKind::Label(s) = &h.tree.node(label).kind else {
            unreachable!()
        };
        assert_eq!(s.window_y, 0);
    }
}
