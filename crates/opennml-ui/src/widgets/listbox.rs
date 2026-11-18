//! `MenuListBox` (vtable `0x0061A898`, re/menu-system 8.3, 12.6; O-19, O-95, O-104, O-106): entries from the query,
//! selection by click, hover row, nameless scroll bar as a child or `AUTOYSIZE` (ComboBox list).

use opennml_formats::dlg::desc::listbox_flags;
use opennml_formats::dlg::{ListDesc, SliderDesc};

use super::{
    Slot, base_mouse, client_screen, due, idiv, int_param, line_height, lock, map_selection,
    notify, query_head, set_bkg_set, slider,
};
use crate::bkg_paint::load_bkg;
use crate::coords::{normalize_rect, to_screen_x, to_screen_y, to_virtual_x, to_virtual_y};
use crate::dialog::COMBO_FRAME_BKG;
use crate::geom::contains;
use crate::msg::{Param, code};
use crate::tree::{Broadcast, Node, NodeId, NodeKind, UiEnv, UiTree};

/// Sample for the row height for `AUTOYSIZE` (`0x0061A8FC`).
const AUTOSIZE_SAMPLE: &str = "Dy'";
/// Sample for the row height for row count, hover and scroll bar (`0x0061A894`).
const ROW_SAMPLE: &str = "X";
/// Width of the scroll bar in the grid.
const SCROLLBAR_W: i32 = 42;
/// The text window moves in by 50 screen pixels with a scroll bar (`FUN_004638F0`).
const SCROLL_TEXT_INSET: i32 = 50;

/// State of a `MenuListBox` (fields from `+0x94`).
#[derive(Debug, Clone)]
pub struct ListState {
    pub desc: ListDesc,
    /// `+0xC8`: entries (`array`).
    pub items: Vec<String>,
    /// `+0xDC`: values for the entries (`indexarray`); with equal length code 4 reports the value instead of the index.
    pub index_map: Vec<i32>,
    /// `+0xB0`: selected row (-1 = none).
    pub selection: i32,
    /// `+0xFC`: first visible row.
    pub first: i32,
    /// `+0xF4`: row under the mouse (-1 = none).
    pub hover: i32,
    /// `+0x108`: more entries than rows (scroll bar visible, text window narrower).
    pub scrolling: bool,
    /// `+0x10C...+0x118`: text window in screen pixels (client, with scroll bar -50 on the right). `Draw`
    /// (`FUN_00464400`) takes only the layout width from it; the position of the entries comes from the current client
    /// (`0x00464443`). That the ComboBox list sets its window before the shift (`combo.rs`) therefore has no effect.
    pub text_window: [i32; 4],
    /// `+0x100`: toggle of the background set on click (no effect, only one set loaded; O-19).
    pub toggle: bool,
    /// `+0x104`: next query time; `None` = never again.
    pub query_at: Option<u64>,
}

impl ListState {
    /// Constructor `FUN_00464720`: selection = `CURSELECTION`, hover -1, query due.
    pub fn new(desc: ListDesc) -> ListState {
        ListState {
            selection: desc.cur_selection,
            desc,
            items: Vec::new(),
            index_map: Vec::new(),
            first: 0,
            hover: -1,
            scrolling: false,
            text_window: [0; 4],
            toggle: false,
            query_at: Some(0),
        }
    }
}

/// Node of a ListBox (base rectangle `EXTENT`, flags, `comboframe.bkg` as the only set); without a name.
pub(crate) fn new_node(desc: ListDesc, env: &mut UiEnv) -> Node {
    let (rect, flags) = (normalize_rect(desc.extent), desc.flags);
    let mut node = Node::new(NodeKind::ListBox(ListState::new(desc)), rect, flags);
    let ui = &mut env.ui;
    node.bkg_sets = vec![load_bkg(ui.vfs, COMBO_FRAME_BKG, ui.assets)];
    node
}

fn state(tree: &UiTree, id: NodeId) -> &ListState {
    match &tree.node(id).kind {
        NodeKind::ListBox(s) => s,
        _ => panic!("widget {id:?} is not a ListBox"),
    }
}

fn state_mut(tree: &mut UiTree, id: NodeId) -> &mut ListState {
    match &mut tree.node_mut(id).kind {
        NodeKind::ListBox(s) => s,
        _ => panic!("widget {id:?} is not a ListBox"),
    }
}

fn count(s: &ListState) -> i32 {
    i32::try_from(s.items.len()).unwrap_or(i32::MAX)
}

/// Rows in the client: `(C.b - C.t) / h(L"X")` in S.
fn rows(tree: &UiTree, id: NodeId, env: &UiEnv) -> i32 {
    let c = client_screen(tree, id, env);
    idiv(c[3].wrapping_sub(c[1]), line_height(env, ROW_SAMPLE))
}

/// Query `FUN_004638F0` (from `OnAttach`, `Draw` and `0x12E` code 2): codes 9 and 5; code 2 (`array` -> entries,
/// `indexarray` -> values, cleared before the message); text window = client in S; when the count changes, limit the
/// first row to `max(0, count - rows)`, broadcast `{0x141, first row, count - rows + 1}` to the children (only if
/// there are any), scroll bar flag and text window -50 from 2 steps; code 0 (`int` only on a response
/// != -1) -> selection, mapped via `indexarray`.
pub(crate) fn query(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if !due(state(tree, id).query_at, env.now) {
        return;
    }
    state_mut(tree, id).query_at = None;
    let (next, _) = query_head(tree, id, env);
    state_mut(tree, id).query_at = next;
    lock(tree.node_mut(id));

    let old = count(state(tree, id));
    state_mut(tree, id).index_map.clear();
    let (_, msg) = notify(tree, id, code::QUERY_ITEMS, &[], env);
    let s = state_mut(tree, id);
    if let Some(Param::Array(items)) = msg.params.get("array") {
        s.items = items.clone();
    }
    if let Some(Param::IndexArray(map)) = msg.params.get("indexarray") {
        s.index_map = map.clone();
    }
    let window = client_screen(tree, id, env);
    state_mut(tree, id).text_window = window;
    let n = count(state(tree, id));
    if n != old {
        let rows = rows(tree, id, env);
        let steps = n.wrapping_sub(rows).wrapping_add(1);
        let limit = n.wrapping_sub(rows).max(0);
        let s = state_mut(tree, id);
        s.first = s.first.min(limit);
        let first = s.first;
        if !tree.node(id).children.is_empty() {
            tree.broadcast(
                id,
                &Broadcast::ScrollRange {
                    value: first,
                    steps,
                },
                env,
            );
        }
        let s = state_mut(tree, id);
        s.scrolling = steps >= 2;
        if s.scrolling {
            s.text_window[2] = s.text_window[2].wrapping_sub(SCROLL_TEXT_INSET);
        }
    }

    let (r, msg) = notify(tree, id, code::QUERY_VALUE, &[], env);
    let value = if r != -1 {
        int_param(&msg, "int").unwrap_or(-1)
    } else {
        -1
    };
    let s = state_mut(tree, id);
    s.selection = map_selection(value, s.items.len(), &s.index_map);
}

/// `OnAttach` `0x00464120`: query; with `AUTOYSIZE` height for all entries (`h(L"Dy'")*count + 4` in S from the
/// client, into the grid with rounding `FUN_00459570`, extended by `CLIENTAREA`, at most 1024 at the bottom), first
/// row 0, no scroll bar. Otherwise a nameless vertical slider (flags `0x28`, 42 grid units wide at the right client
/// edge) with `RESOLUTION = count - rows + 1`; from 2 steps initial scrolling only if `selection > rows` (not `>=`,
/// O-95) to `selection - rows + 1` and scroll bar flag.
pub(crate) fn on_attach(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    query(tree, id, env);
    let n = count(state(tree, id));
    let screen = env.ui.screen;
    if tree.node(id).flags & listbox_flags::AUTOYSIZE != 0 {
        let h = line_height(env, AUTOSIZE_SAMPLE);
        let c = client_screen(tree, id, env);
        let bottom = c[1].wrapping_add(h.wrapping_mul(n).wrapping_add(4));
        let mut r = [
            to_virtual_x(screen, c[0]),
            to_virtual_y(screen, c[1]),
            to_virtual_x(screen, c[2]),
            to_virtual_y(screen, bottom),
        ];
        let node = tree.node(id);
        if let Some(Some(bkg)) = node.bkg_sets.get(node.bkg_set) {
            let ca = bkg.desc.client_area;
            r[0] = r[0].wrapping_sub(ca[0]);
            r[1] = r[1].wrapping_sub(ca[1]);
            r[2] = r[2].wrapping_add(ca[2]);
            r[3] = r[3].wrapping_add(ca[3]).min(crate::coords::VIRTUAL_H);
        }
        tree.node_mut(id).rect = r;
        let window = client_screen(tree, id, env);
        let s = state_mut(tree, id);
        s.first = 0;
        s.scrolling = false;
        s.text_window = window;
        return;
    }
    let rows = rows(tree, id, env);
    let steps = n.wrapping_sub(rows).wrapping_add(1);
    let c = tree.client_rect(id);
    let (w, h) = (c[2].wrapping_sub(c[0]), c[3].wrapping_sub(c[1]));
    let desc = SliderDesc {
        flags: slider::SCROLLBAR_FLAGS,
        resolution: steps,
        extent: [w.wrapping_sub(SCROLLBAR_W), 0, w, h],
        ..SliderDesc::default()
    };
    tree.add_child(id, slider::new_node(desc), env);
    if steps > 1 {
        let sel = state(tree, id).selection;
        if sel > rows {
            let first = sel.wrapping_sub(rows).wrapping_add(1).max(0);
            state_mut(tree, id).first = first;
            tree.broadcast(
                id,
                &Broadcast::ScrollRange {
                    value: first,
                    steps,
                },
                env,
            );
        }
        state_mut(tree, id).scrolling = true;
    }
}

/// Slot 18 `0x004636E0`: row under the mouse with a fixed row height `h(L"X")` in the full client (also below the
/// scroll bar), in S: `z = (p.y - C.t) / lh`, only `0 <= z < rows` -> `first row + z` (-1 from `count` on). `true`.
fn hover(tree: &mut UiTree, id: NodeId, pt: [i32; 2], env: &UiEnv) -> bool {
    let screen = env.ui.screen;
    let c = client_screen(tree, id, env);
    let p = [to_screen_x(screen, pt[0]), to_screen_y(screen, pt[1])];
    let lh = line_height(env, ROW_SAMPLE);
    let rows = idiv(c[3].wrapping_sub(c[1]), lh);
    let mut hover = -1;
    if contains(c, p[0], p[1]) {
        let z = idiv(p[1].wrapping_sub(c[1]), lh);
        if p[1] >= 0 && z >= 0 && z < rows {
            let s = state(tree, id);
            hover = z.wrapping_add(s.first);
            if hover >= count(s) {
                hover = -1;
            }
        }
    }
    state_mut(tree, id).hover = hover;
    true
}

/// Click (slot 7 `0x004637B0`) and double click (slot 6 `0x00463DB0`): hover anew, the row below becomes the selection
/// (otherwise the old one stays - a click into the void reports it again, O-95); code 4 with `indexarray[selection]`
/// or the selection, with a double click additionally code `0xD`, then `0x12C {selection}` (raw index) at the parent;
/// if the value is negative, none of this. Afterwards both toggle the background set (`+0x100`, `FUN_0045A050`) -
/// without effect, there is only one (O-19).
fn select(tree: &mut UiTree, id: NodeId, pt: [i32; 2], double: bool, env: &mut UiEnv) {
    hover(tree, id, pt, env);
    let s = state_mut(tree, id);
    if s.hover >= 0 && s.hover < count(s) {
        s.selection = s.hover;
    }
    let sel = s.selection;
    let value = if sel < 0 {
        -1
    } else if s.items.len() == s.index_map.len() {
        usize::try_from(sel)
            .ok()
            .and_then(|i| s.index_map.get(i).copied())
            .unwrap_or(-1)
    } else {
        sel
    };
    if value >= 0 {
        notify(
            tree,
            id,
            code::ITEM_SELECTED,
            &[("int", Param::Int(value))],
            env,
        );
        if double {
            notify(tree, id, code::DOUBLE_CLICK, &[], env);
        }
        if let Some(parent) = tree.node(id).parent {
            tree.broadcast(parent, &Broadcast::ListPick(sel), env);
        }
    }
    if tree.contains(id) {
        let s = state_mut(tree, id);
        s.toggle = !s.toggle;
        let set = usize::from(s.toggle);
        set_bkg_set(tree.node_mut(id), set);
    }
}

/// Mouse slots: leave (17 `0x004636A0`) -> hover -1; move (18); click (7); double click (6); wheel (0/1
/// `0x00463D50`/`0x00463CF0`) only to visible children, first `true` wins, `false` without children (falls through).
pub(crate) fn mouse_slot(
    tree: &mut UiTree,
    id: NodeId,
    slot: Slot,
    pt: [i32; 2],
    env: &mut UiEnv,
) -> bool {
    match slot {
        Slot::Leave => {
            state_mut(tree, id).hover = -1;
            true
        }
        Slot::Move => hover(tree, id, pt, env),
        Slot::LeftClick => {
            select(tree, id, pt, false, env);
            true
        }
        Slot::LeftDouble => {
            select(tree, id, pt, true, env);
            true
        }
        Slot::WheelUp | Slot::WheelDown => {
            for child in tree.node(id).children.clone() {
                if tree.contains(child)
                    && tree.node(child).visible
                    && super::mouse_slot(tree, child, slot, pt, env)
                {
                    return true;
                }
            }
            false
        }
        _ => base_mouse(slot),
    }
}

/// `OnMessage` (slot 19 `0x00463CA0`): `0x12E` code 2 -> query due and immediate; `0x140` from the scroll bar -> first
/// row (unchecked, the slider has already limited). No `0x141` reception.
pub(crate) fn on_message(tree: &mut UiTree, id: NodeId, msg: &Broadcast, env: &mut UiEnv) -> bool {
    match msg {
        Broadcast::RequeryItems => {
            state_mut(tree, id).query_at = Some(0);
            query(tree, id, env);
            true
        }
        Broadcast::ScrollFirst(v) => {
            state_mut(tree, id).first = *v;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{MouseButton, UiInput};
    use crate::widgets::slider::SliderState;
    use crate::widgets::testkit::Harness;

    /// ListBox in the frame (client origin 115, 130): absolute (125, 140, 325, 232), client with `comboframe.bkg`
    /// (`CLIENTAREA 13`) (138, 153, 312, 219) = 66 high -> 4 rows of 15.
    const DLG: &str = "FRAME DLG EXTENT 100 100 600 500\n\
         LISTBOX L EXTENT 10 10 210 102 /LISTBOX\n/FRAME\n";

    /// Logic with `n` entries and the response `sel` to code 0 (`None` = no response).
    fn logic(h: &mut Harness, n: usize, sel: Option<i32>) {
        h.rec.answer = Box::new(move |m| {
            if m.element != "l" {
                return -1;
            }
            match m.code {
                code::QUERY_ITEMS => {
                    let items = (0..n).map(|i| format!("E{i}")).collect();
                    m.params.insert("array".into(), Param::Array(items));
                    0
                }
                code::QUERY_VALUE => match sel {
                    Some(v) => {
                        m.params.insert("int".into(), Param::Int(v));
                        0
                    }
                    None => -1,
                },
                _ => -1,
            }
        });
    }

    fn list(h: &Harness) -> (NodeId, &ListState) {
        let id = h.tree.find("DLGL").unwrap();
        match &h.tree.node(id).kind {
            NodeKind::ListBox(s) => (id, s),
            _ => unreachable!(),
        }
    }

    /// Center of row `z` (counted from the first visible one).
    fn row(z: i32) -> (i32, i32) {
        (200, 153 + 15 * z + 7)
    }

    fn scrollbar(h: &Harness, id: NodeId) -> (NodeId, &SliderState) {
        let child = h.tree.node(id).children[0];
        match &h.tree.node(child).kind {
            NodeKind::Slider(s) => (child, s),
            _ => panic!("no scroll bar"),
        }
    }

    // QUIRK O-95: kept
    #[test]
    fn listbox_scrollbar_appears_only_with_more_items_than_rows() {
        let mut h = Harness::new();
        logic(&mut h, 4, None);
        h.build(DLG);
        let (id, s) = list(&h);
        assert!(!s.scrolling);
        assert_eq!(h.tree.node(id).children.len(), 1, "always created");
        let (bar, sb) = scrollbar(&h, id);
        assert_eq!(sb.resolution, 1);
        assert!(!h.tree.node(bar).visible);
        assert_eq!(h.tree.node(bar).name, "");
        // 42 grid units wide at the right edge of the client (174 x 66).
        assert_eq!(h.tree.node(bar).rect, [132, 0, 174, 66]);
        assert_eq!(h.tree.node(bar).flags, 0x28);
        // Without a visible scroll bar the wheel falls through.
        h.input(UiInput::Move { x: 200, y: 170 });
        assert!(!h.input(UiInput::WheelDown));

        let mut h = Harness::new();
        logic(&mut h, 5, None);
        h.build(DLG);
        let (id, s) = list(&h);
        assert!(s.scrolling);
        let (bar, sb) = scrollbar(&h, id);
        assert_eq!(sb.resolution, 2);
        assert!(h.tree.node(bar).visible);
        // The wheel goes to the scroll bar, the nameless slider reports 0x140 to the list.
        h.input(UiInput::Move { x: 200, y: 170 });
        h.forget();
        assert!(h.input(UiInput::WheelDown));
        assert_eq!(list(&h).1.first, 1);
        assert!(h.rec.msgs.is_empty(), "the scroll bar reports no code");
        // Hover with a fixed row height from the first visible row; leaving clears it.
        let (x, y) = row(2);
        h.input(UiInput::Move { x, y });
        assert_eq!(list(&h).1.hover, 3);
        h.input(UiInput::Move { x: 50, y: 50 });
        assert_eq!(list(&h).1.hover, -1);
    }

    // QUIRK O-117: kept
    #[test]
    fn text_window_resets_on_every_query_and_insets_only_on_a_new_count() {
        // `FUN_004638F0`: every query sets the text window to the full client (138, 153, 312, 219); the -50 at the
        // right edge exists only when the count changes (from 2 steps). A query with the same count takes it back, the
        // scroll bar stays.
        let n = std::rc::Rc::new(std::cell::Cell::new(5usize));
        let mut h = Harness::new();
        let items = n.clone();
        h.rec.answer = Box::new(move |m| {
            if m.element != "l" || m.code != code::QUERY_ITEMS {
                return -1;
            }
            let list = (0..items.get()).map(|i| format!("E{i}")).collect();
            m.params.insert("array".into(), Param::Array(list));
            0
        });
        h.build(DLG);
        let (id, s) = list(&h);
        assert!(s.scrolling);
        assert_eq!(s.text_window, [138, 153, 262, 219], "new count: -50");
        let requery = |h: &mut Harness| {
            h.with(|t, _, e| on_message(t, id, &Broadcast::RequeryItems, e));
        };
        requery(&mut h);
        let s = list(&h).1;
        assert!(s.scrolling, "scroll bar stays");
        assert_eq!(
            s.text_window,
            [138, 153, 312, 219],
            "same count: full client"
        );
        n.set(6);
        requery(&mut h);
        assert_eq!(list(&h).1.text_window, [138, 153, 262, 219]);
    }

    // QUIRK O-118: kept
    #[test]
    fn autoysize_writes_the_absolute_client_into_the_relative_rect() {
        // `OnAttach` `0x00464120` with `AUTOYSIZE`: the rectangle arises from the absolute client
        // (`FUN_00459B30` -> `FUN_00459AC0`) and is set with `FUN_0045A210` as a relative rectangle - in the frame
        // (client origin 115, 130) the list is therefore shifted by this origin. Without effect in the shipped data:
        // only ComboBox lists have `AUTOYSIZE`, whose parent (blocker) is at the origin when attached (O-104).
        const AUTO: &str = "FRAME DLG EXTENT 100 100 600 500
             LISTBOX L FLAGS LISTBOX_AUTOYSIZE /FLAGS EXTENT 10 10 210 102 /LISTBOX
/FRAME
";
        let mut h = Harness::new();
        logic(&mut h, 3, None);
        h.build(AUTO);
        let (id, s) = list(&h);
        assert!(!s.scrolling);
        let r = h.tree.node(id).rect;
        // Absolute (125, 140, 325, ...) = client (138, 153, 312) extended by `CLIENTAREA 13`, as a relative rectangle.
        assert_eq!([r[0], r[1], r[2]], [125, 140, 325]);
        assert!(r[3] > r[1]);
        let abs = h.tree.abs_rect(id);
        assert_eq!([abs[0], abs[1], abs[2]], [240, 270, 440], "shifted twice");
    }

    // QUIRK O-95: kept
    #[test]
    fn initial_scroll_only_when_selection_exceeds_rows() {
        // O-95: 10 entries, 4 rows; selection 4 (= rows) does not scroll, selection 5 scrolls to 5 - 4 + 1 = 2.
        let mut h = Harness::new();
        logic(&mut h, 10, Some(4));
        h.build(DLG);
        let (id, s) = list(&h);
        assert_eq!((s.selection, s.first), (4, 0));
        assert_eq!(scrollbar(&h, id).1.value, 0);

        let mut h = Harness::new();
        logic(&mut h, 10, Some(5));
        h.build(DLG);
        let (id, s) = list(&h);
        assert_eq!((s.selection, s.first), (5, 2));
        let (_, sb) = scrollbar(&h, id);
        assert_eq!((sb.value, sb.resolution), (2, 7));
    }

    // QUIRK O-95: kept
    #[test]
    fn click_on_empty_space_repeats_the_old_selection() {
        // O-95: two entries, four rows.
        let mut h = Harness::new();
        logic(&mut h, 2, None);
        h.build(DLG);
        assert_eq!(list(&h).1.selection, -1);
        // Without a selection a click into the void reports nothing.
        h.forget();
        h.click(row(3));
        assert!(h.codes("l").is_empty());
        h.click(row(1));
        assert_eq!(h.sent_ints("l", code::ITEM_SELECTED, "int"), [1]);
        // Click below the last entry: the old selection comes once more.
        h.click(row(3));
        assert_eq!(h.sent_ints("l", code::ITEM_SELECTED, "int"), [1, 1]);
        assert_eq!(list(&h).1.selection, 1);
    }

    // QUIRK O-19: kept
    #[test]
    fn listbox_click_has_no_visible_background_toggle() {
        // O-19 (corrected): the click toggles `+0x100`, but `FUN_0045A050` finds no second set.
        let mut h = Harness::new();
        logic(&mut h, 2, None);
        h.build(DLG);
        let (id, _) = list(&h);
        assert_eq!(h.tree.node(id).bkg_sets.len(), 1);
        h.click(row(0));
        assert!(list(&h).1.toggle);
        assert_eq!(h.tree.node(id).bkg_set, 0);
        h.click(row(1));
        assert!(!list(&h).1.toggle);
        assert_eq!(h.tree.node(id).bkg_set, 0);
    }

    // QUIRK O-106: kept
    #[test]
    fn listbox_double_click_sends_4_4_d() {
        // O-106: sequence 4, 5, 4, 5, 11 - both clicks report code 4, the double click (slot 6 `0x00463DB0`) code 4
        // once more and then `0xD`, each before `0x12C`.
        let mut h = Harness::new();
        logic(&mut h, 3, None);
        h.build(DLG);
        let (x, y) = row(1);
        h.input(UiInput::Move { x, y });
        h.forget();
        for input in [
            UiInput::Down(MouseButton::Left),
            UiInput::Up(MouseButton::Left),
            UiInput::Down(MouseButton::Left),
            UiInput::Up(MouseButton::Left),
            UiInput::DoubleClick(MouseButton::Left),
        ] {
            h.input(input);
        }
        assert_eq!(
            h.codes("l"),
            [
                code::ITEM_SELECTED,
                code::ITEM_SELECTED,
                code::ITEM_SELECTED,
                code::DOUBLE_CLICK
            ]
        );
        assert_eq!(h.sent_ints("l", code::ITEM_SELECTED, "int"), [1, 1, 1]);
    }
}
