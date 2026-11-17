//! ComboBox (button with bit `0x10`) and `ComboPopup` (re/menu-system 8.3, 12.6; O-18, O-104): opening, list under a
//! `ModalBlocker`, taking over the selection and closing.
//!
//! Flow in the original: click -> `ComboPopup` (rectangle `LISTBOXEXTENT`) on the parent of the button;
//! `ComboPopup.OnAttach` attaches a `ModalBlocker` (owner = parent of the button) to the top root and below it a
//! `MenuListBox` with forced `AUTOYSIZE`, shifted by the absolute client origin of the frame. The list queries its
//! entries itself under the name of the ComboBox. A click into the list reports code 4 and broadcasts `0x12C` via the
//! blocker to the button; a click beside it hits the blocker (`0x12D`). The button then marks the popup for deletion;
//! its destructor marks the blocker (`FUN_0045F230`, in the tree `UiTree::free`). Until the next untaken mouse event or
//! frame event, both remain hittable (12.1.8).

use nml_formats::dlg::ListDesc;
use nml_formats::dlg::desc::listbox_flags;

use super::{button, int_param, map_selection, notify};
use crate::geom::offset;
use crate::input::synthetic_move;
use crate::msg::{Param, code};
use crate::tree::{FULL, Node, NodeId, NodeKind, UiEnv, UiTree};

/// State of the `ComboPopup` (`FUN_0045F310`): copy of the list description (`+0x94`), blocker (`+0xC8`) and list
/// (`+0xCC`).
#[derive(Debug, Clone)]
pub struct PopupState {
    pub list: ListDesc,
    pub blocker: Option<NodeId>,
    pub list_box: Option<NodeId>,
}

fn popup_state_mut(tree: &mut UiTree, id: NodeId) -> &mut PopupState {
    match &mut tree.node_mut(id).kind {
        NodeKind::ComboPopup(p) => p,
        _ => panic!("widget {id:?} is not a ComboPopup"),
    }
}

/// Opening in the click `0x0045F047`...`0x0045F0A4`: `+0x10C = 1`, attach a `ComboPopup` with `LISTBOXEXTENT` (in frame
/// client coordinates, like `BUTTONEXTENT`) and the name of the ComboBox to the parent of the button; its `OnAttach`
/// builds blocker and list ([`on_attach`]).
pub(crate) fn open(tree: &mut UiTree, button: NodeId, env: &mut UiEnv) {
    let Some(parent) = tree.node(button).parent else {
        return;
    };
    let state = button::state_mut(tree, button);
    let Some(list) = state.list.clone() else {
        return;
    };
    state.list_open = true;
    let mut popup = Node::new(
        NodeKind::ComboPopup(PopupState {
            list: list.clone(),
            blocker: None,
            list_box: None,
        }),
        crate::coords::normalize_rect(list.extent),
        0,
    );
    popup.dialog = tree.node(button).dialog.clone();
    popup.name = tree.node(button).name.clone();
    let id = tree.add_child(parent, popup, env);
    button::state_mut(tree, button).popup = Some(id);
}

/// `ComboPopup.OnAttach` `FUN_0045F120`: `ModalBlocker` (whole grid, owner = parent of the popup) on the top root;
/// force `LISTBOX_AUTOYSIZE`; attach `MenuListBox` as a child of the blocker (its `OnAttach` queries the entries and
/// sets the height while it is still at the origin - the limit `bottom <= 1024` therefore applies before the shift,
/// O-104); then shift by the absolute client origin of the parent (`FUN_004599A0`) with an artificial mouse movement
/// (`FUN_0045B850`).
pub(crate) fn on_attach(tree: &mut UiTree, popup: NodeId, env: &mut UiEnv) {
    let Some(owner) = tree.node(popup).parent else {
        return;
    };
    let blocker = Node::new(NodeKind::ModalBlocker { owner }, FULL, 0);
    let desktop = tree.desktop();
    let blocker = tree.add_child(desktop, blocker, env);
    popup_state_mut(tree, popup).blocker = Some(blocker);

    let mut desc = popup_state_mut(tree, popup).list.clone();
    desc.flags |= listbox_flags::AUTOYSIZE;
    popup_state_mut(tree, popup).list = desc.clone();
    let mut list = super::listbox::new_node(desc, env);
    list.dialog = tree.node(popup).dialog.clone();
    list.name = tree.node(popup).name.clone();
    let list = tree.add_child(blocker, list, env);
    popup_state_mut(tree, popup).list_box = Some(list);

    let c = tree.client_rect(owner);
    let node = tree.node_mut(list);
    node.rect = offset(node.rect, [c[0], c[1]]);
    synthetic_move(tree, env);
}

/// `0x12C` at the button (`FUN_0045E8A0`, only with an open list): raw row index -> `+0xE4`, timer due (only if it was
/// 0), query immediately (code 0 overwrites the selection with the response of the logic), mark the popup, select set,
/// list closed. The button itself reports no code; the list sent code 4 before.
pub(crate) fn pick(tree: &mut UiTree, button: NodeId, index: i32, env: &mut UiEnv) -> bool {
    let state = button::state_mut(tree, button);
    if !state.list_open {
        return false;
    }
    state.selection = index;
    if state.query_at.is_none() {
        state.query_at = Some(0);
    }
    button::query(tree, button, env);
    close(tree, button);
    true
}

/// `0x12D` at the button (only with an open list): mark the popup, select set, list closed.
pub(crate) fn cancel(tree: &mut UiTree, button: NodeId) -> bool {
    if !button::state_mut(tree, button).list_open {
        return false;
    }
    close(tree, button);
    true
}

/// Common ending of `0x12C`/`0x12D`: `FUN_0045B9E0(popup)` at the parent, `+0x94 = 0`, `FUN_0045E080`,
/// `+0x10C = 0`.
fn close(tree: &mut UiTree, button: NodeId) {
    let state = button::state_mut(tree, button);
    let popup = state.popup.take();
    if let Some(popup) = popup
        && let Some(parent) = tree.node(button).parent
    {
        tree.schedule_delete(parent, popup);
    }
    button::select_bkg_set(tree.node_mut(button));
    button::state_mut(tree, button).list_open = false;
}

/// ComboBox part of the query `FUN_0045E350`: code 2 (`array` -> `+0x110`; `indexarray` -> `+0x114`, cleared before
/// the message, `FUN_0042FDA0(0, -1)` - the entries stay if the logic supplies none), then code 0 (`int` -> mapped via
/// `indexarray`, fields of equal length assumed, otherwise the value itself; not found -1) -> `+0xE4`. If `int` is
/// missing, the original takes a leftover pointer as the value (hypothesis); the reimplementation takes -1.
pub(crate) fn query_items(tree: &mut UiTree, button: NodeId, env: &mut UiEnv) {
    let (_, msg) = notify(tree, button, code::QUERY_ITEMS, &[], env);
    let state = button::state_mut(tree, button);
    if let Some(Param::Array(items)) = msg.params.get("array") {
        state.items = items.clone();
    }
    match msg.params.get("indexarray") {
        Some(Param::IndexArray(map)) => state.index_map = map.clone(),
        _ => state.index_map.clear(),
    }
    let (_, msg) = notify(tree, button, code::QUERY_VALUE, &[], env);
    let value = int_param(&msg, "int").unwrap_or(-1);
    let state = button::state_mut(tree, button);
    state.selection = map_selection(value, state.items.len(), &state.index_map);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;
    use crate::input::{MouseButton, UiInput};
    use crate::widgets::testkit::{Harness, center};
    use crate::widgets::{Slot, Trace};

    /// Frame with client origin (115, 130) (`menuframe.bkg` of the test data: `CLIENTAREA 15 30 15 30`), a ComboBox
    /// and a button next to it.
    const DLG: &str = "FRAME DLG EXTENT 100 100 600 500\n\
         COMBOBOX RES FLAGS MENUCOMBOBOX_TEXT /FLAGS BUTTONEXTENT 50 10 250 50 \
         LISTBOXEXTENT 50 60 250 200 CURSELECTION -1 /COMBOBOX\n\
         BUTTON OK FLAGS MENUBUTTON_TEXT /FLAGS EXTENT 300 10 400 50 /BUTTON\n/FRAME\n";

    /// Logic: three entries with values 10, 20, 30; code 0 returns the value last reported via code 4
    /// (initially 20).
    fn logic(h: &mut Harness) -> Rc<Cell<i32>> {
        let value = Rc::new(Cell::new(20));
        let v = value.clone();
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
                    m.params.insert("int".into(), Param::Int(v.get()));
                    0
                }
                code::ITEM_SELECTED => {
                    if let Some(Param::Int(x)) = m.params.get("int") {
                        v.set(*x);
                    }
                    0
                }
                _ => -1,
            }
        });
        value
    }

    fn button(h: &Harness, id: NodeId) -> &button::ButtonState {
        match &h.tree.node(id).kind {
            NodeKind::Button(s) => s,
            _ => unreachable!(),
        }
    }

    /// Opens the ComboBox `name` by click and returns (button, popup, blocker, list).
    fn open_named(h: &mut Harness, name: &str) -> (NodeId, NodeId, NodeId, NodeId) {
        let res = h.tree.find(name).unwrap();
        let at = center(&h.tree, res);
        h.click(at);
        let popup = button(h, res).popup.expect("popup open");
        let NodeKind::ComboPopup(p) = &h.tree.node(popup).kind else {
            panic!("no popup");
        };
        (res, popup, p.blocker.unwrap(), p.list_box.unwrap())
    }

    fn open_combo(h: &mut Harness) -> (NodeId, NodeId, NodeId, NodeId) {
        open_named(h, "DLGRES")
    }

    /// Center of row `row` of the open list (client top + 15 per row).
    fn row(h: &Harness, list: NodeId, row: i32) -> (i32, i32) {
        let c = h.tree.client_rect(list);
        ((c[0] + c[2]) / 2, c[1] + 15 * row + 7)
    }

    #[test]
    fn combo_click_opens_a_popup_under_a_blocker_in_frame_coordinates() {
        // O-104: list at LISTBOXEXTENT + client origin of the frame, AUTOYSIZE, blocker at the desktop.
        let mut h = Harness::new();
        logic(&mut h);
        h.build(DLG);
        let frame = h.tree.find("DLG").unwrap();
        let (res, popup, blocker, list) = open_combo(&mut h);
        assert!(button(&h, res).list_open);
        assert_eq!(h.tree.node(popup).parent, Some(frame));
        assert_eq!(h.tree.node(frame).children.last(), Some(&popup));
        assert_eq!(h.tree.node(popup).rect, [50, 60, 250, 200]);
        let desktop = h.tree.desktop();
        assert_eq!(h.tree.node(desktop).children.last(), Some(&blocker));
        assert!(
            matches!(h.tree.node(blocker).kind, NodeKind::ModalBlocker { owner } if owner == frame)
        );
        assert_eq!(h.tree.node(blocker).rect, [0, 0, 1280, 1024]);
        assert_eq!(h.tree.node(list).parent, Some(blocker));
        assert_ne!(h.tree.node(list).flags & listbox_flags::AUTOYSIZE, 0);
        // Height at the origin: client (63, 73, 237, 187) with CLIENTAREA 13; 3 rows of 15 + 4 = 49 -> bottom 122,
        // with the border 135; then shifted by (115, 130).
        assert_eq!(h.tree.abs_rect(list), [165, 190, 365, 265]);
        let NodeKind::ListBox(l) = &h.tree.node(list).kind else {
            unreachable!()
        };
        assert_eq!(l.items, ["A", "B", "C"]);
        assert_eq!(l.selection, 1, "code 0 = 20 via indexarray");
        assert!(h.tree.node(list).children.is_empty(), "never a scroll bar");
        // The opening click: click sound, but no code 8; the frame took the release.
        assert!(!h.codes("res").contains(&code::TRIGGERED));
        assert_eq!(h.fx.len(), 1);

        // Limit bottom <= 1024 before the shift: the list then extends below the edge.
        let mut h = Harness::new();
        h.rec.answer = Box::new(|m| {
            if m.code == code::QUERY_ITEMS {
                let items = (0..10).map(|i| i.to_string()).collect();
                m.params.insert("array".into(), Param::Array(items));
                return 0;
            }
            -1
        });
        h.rec.allow = true;
        h.build(
            "FRAME DLG EXTENT 100 100 600 500\n COMBOBOX LOW BUTTONEXTENT 50 10 250 50 \
             LISTBOXEXTENT 50 850 250 900 /COMBOBOX\n/FRAME\n",
        );
        let (_, _, _, list) = open_named(&mut h, "DLGLOW");
        // 863 + 10*15 + 4 = 1017, with border 1030 -> 1024; shifted 1154.
        assert_eq!(h.tree.abs_rect(list), [165, 980, 365, 1154]);
    }

    #[test]
    fn combo_is_invisible_while_open() {
        // O-18: `Draw` skips the button as long as the list is open.
        let mut h = Harness::new();
        logic(&mut h);
        h.build(DLG);
        let res = h.tree.find("DLGRES").unwrap();
        assert!(button::draws_itself(h.tree.node(res)));
        let (res, ..) = open_combo(&mut h);
        assert!(!button::draws_itself(h.tree.node(res)));
        // With an open list the button takes no press (the blocker lies above it anyway).
        assert!(!h.with(|t, _, e| button::mouse_slot(t, res, Slot::LeftDown, e)));
        h.click((50, 50));
        assert!(button::draws_itself(h.tree.node(res)));
    }

    #[test]
    fn combo_list_queries_items_under_the_combo_name() {
        let mut h = Harness::new();
        logic(&mut h);
        h.build(DLG);
        let res = h.tree.find("DLGRES").unwrap();
        // At construction the button itself queries 9, 5, 7, 2, 0 and maps 20 to row 1.
        assert_eq!(h.codes("res"), [9, 5, 7, 2, 0]);
        assert_eq!(button(&h, res).selection, 1);
        assert_eq!(button(&h, res).items, ["A", "B", "C"]);
        h.forget();
        let (_, _, _, list) = open_combo(&mut h);
        // The list queries under the name of the ComboBox: codes 9, 5, 2, 0.
        assert_eq!(h.codes("res"), [9, 5, 2, 0]);
        assert!(h.rec.msgs.iter().all(|m| m.0 == "dlg" && m.1 == "res"));
        assert_eq!(
            (
                h.tree.node(list).dialog.as_str(),
                h.tree.node(list).name.as_str()
            ),
            ("DLG", "RES")
        );
    }

    #[test]
    fn pick_sends_code_4_then_requeries() {
        let mut h = Harness::new();
        logic(&mut h);
        h.build(DLG);
        let (res, popup, _, list) = open_combo(&mut h);
        h.forget();
        let at = row(&h, list, 2);
        h.click(at);
        // Code 4 with the value from indexarray, then the button queries again (9, 5, 7, 2, 0); its selection is the
        // response to code 0, mapped.
        assert_eq!(h.codes("res"), [4, 9, 5, 7, 2, 0]);
        assert_eq!(h.sent_ints("res", code::ITEM_SELECTED, "int"), [30]);
        let s = button(&h, res);
        assert_eq!(s.selection, 2);
        assert!(!s.list_open && s.popup.is_none());
        // The popup is only marked.
        let frame = h.tree.find("DLG").unwrap();
        assert_eq!(h.tree.node(frame).pending_deletes, [popup]);
        assert!(button::draws_itself(h.tree.node(res)));
    }

    #[test]
    fn click_beside_closes_and_is_swallowed() {
        let mut h = Harness::new();
        logic(&mut h);
        h.build(DLG);
        let (res, _, blocker, _) = open_combo(&mut h);
        let ok = h.tree.find("DLGOK").unwrap();
        h.forget();
        h.fx.clear();
        h.tree.trace.clear();
        let at = center(&h.tree, ok);
        h.click(at);
        // The blocker takes the click (0x12D to the frame), the button below gets nothing.
        assert!(
            h.tree
                .trace
                .contains(&Trace::Mouse(blocker, Slot::LeftClick))
        );
        assert!(
            h.tree
                .trace
                .iter()
                .all(|t| !matches!(t, Trace::Mouse(id, _) if *id == ok))
        );
        assert!(h.rec.msgs.is_empty());
        assert!(h.fx.is_empty());
        assert!(!button(&h, res).list_open);
        assert_eq!(h.tree.node(res).bkg_set, button::SET_NORMAL);
        // Right click and double click do not close either, only the left click. Before reopening, the frame event
        // must delete the old blocker - until then it swallows every click.
        h.input(UiInput::Frame);
        let (res, ..) = open_combo(&mut h);
        h.input(UiInput::Move { x: 50, y: 50 });
        h.input(UiInput::Down(MouseButton::Right));
        h.input(UiInput::Up(MouseButton::Right));
        h.input(UiInput::DoubleClick(MouseButton::Left));
        assert!(button(&h, res).list_open);
    }

    #[test]
    fn deleted_popup_stays_hittable_until_the_frame_event() {
        // 12.1.8: the marked popup with blocker and list stays in the tree and hittable until the frame event;
        // afterwards everything is gone, without a panic.
        let mut h = Harness::new();
        logic(&mut h);
        h.build(DLG);
        let (res, popup, blocker, list) = open_combo(&mut h);
        let at = row(&h, list, 0);
        h.click(at);
        assert!(!button(&h, res).list_open);
        for id in [popup, blocker, list] {
            assert!(h.tree.contains(id), "{id:?}");
        }
        // A second click still hits the list: code 4 again, the button no longer takes 0x12C.
        h.forget();
        let at = row(&h, list, 1);
        h.click(at);
        assert_eq!(h.codes("res"), [4]);
        assert_eq!(h.sent_ints("res", code::ITEM_SELECTED, "int"), [20]);
        // The frame event deletes the popup, its destructor the blocker - both in the same pass.
        assert!(!h.input(UiInput::Frame));
        for id in [popup, blocker, list] {
            assert!(!h.tree.contains(id), "{id:?}");
        }
        let desktop = h.tree.desktop();
        assert_eq!(h.tree.node(desktop).children, [h.tree.dialog_root()]);
        assert!(h.tree.node(desktop).pending_deletes.is_empty());
        // The same point now hits the frame: no message.
        h.forget();
        h.click(at);
        assert!(h.rec.msgs.is_empty());
        // Reopening builds anew.
        let (_, popup2, ..) = open_combo(&mut h);
        assert_ne!(popup2, popup);
    }
}
