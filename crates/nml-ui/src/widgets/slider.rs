//! `MenuSlider` (vtable `0x0061A938`, re/menu-system 8.3, 12.6; O-93, O-100): value `0 ... RESOLUTION - 1`,
//! dragging, step squares, wheel. A slider without an element name is the internal scroll bar of ListBox and label and
//! reports to its parent through the broadcast `0x140` instead of code 1.

use nml_formats::dlg::SliderDesc;
use nml_formats::dlg::desc::slider_flags;

use super::{Slot, base_mouse, due, int_param, lock, notify, query_head};
use crate::bkg_paint::load_bkg;
use crate::coords::normalize_rect;
use crate::dialog::{SLIDER_HORZ_BKG, SLIDER_VERT_BKG};
use crate::geom::contains;
use crate::msg::{Param, code};
use crate::tree::{Broadcast, Node, NodeId, NodeKind, UiEnv, UiTree};

/// Flag `0x20`: visible only from two steps on (`FUN_004595F0` in `OnAttach` and `0x141`); only the internal
/// scroll bars carry it.
pub(crate) const AUTO_HIDE: u32 = 0x20;
/// Flags of the internal scroll bars: vertical and `AUTO_HIDE` (`0x28`).
pub(crate) const SCROLLBAR_FLAGS: u32 = slider_flags::VERTICAL | AUTO_HIDE;
/// Edge length of the thumb and the step squares in the grid (`slider_42x42.tga`).
const THUMB: i32 = 42;

/// State of a `MenuSlider` (fields from `+0x94`).
#[derive(Debug, Clone)]
pub struct SliderState {
    pub desc: SliderDesc,
    /// `+0xBC`: value; the query sets it unclamped, all other paths clamp to `0 ... R - 1`.
    pub value: i32,
    /// `+0xC0`: number of steps `R` (`RESOLUTION`, a response to code 6 overwrites it).
    pub resolution: i32,
    /// `+0x9C`: dragging is in progress.
    pub dragging: bool,
    /// `+0xD8`: next query time; `None` = never again.
    pub query_at: Option<u64>,
}

impl SliderState {
    /// Constructor `FUN_00465580`: value and steps from the description, query due.
    pub fn new(desc: SliderDesc) -> SliderState {
        SliderState {
            value: desc.cur_value,
            resolution: desc.resolution,
            desc,
            dragging: false,
            query_at: Some(0),
        }
    }
}

/// Node of a slider without a name (rectangle `EXTENT`, flags); `OnAttach` loads the background set.
pub(crate) fn new_node(desc: SliderDesc) -> Node {
    let (rect, flags) = (normalize_rect(desc.extent), desc.flags);
    Node::new(NodeKind::Slider(SliderState::new(desc)), rect, flags)
}

fn state(tree: &UiTree, id: NodeId) -> &SliderState {
    match &tree.node(id).kind {
        NodeKind::Slider(s) => s,
        _ => panic!("widget {id:?} is not a Slider"),
    }
}

fn state_mut(tree: &mut UiTree, id: NodeId) -> &mut SliderState {
    match &mut tree.node_mut(id).kind {
        NodeKind::Slider(s) => s,
        _ => panic!("widget {id:?} is not a Slider"),
    }
}

/// `v` clamped to `0 ... R - 1`; 0 for `R <= 0`.
fn clamp_value(v: i32, r: i32) -> i32 {
    if r <= 0 { 0 } else { v.clamp(0, r - 1) }
}

/// Reporting the value: without an element name the broadcast `{0x140, value}` at the parent, otherwise code 1 with
/// `int`.
fn report(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    let value = state(tree, id).value;
    if tree.node(id).name.is_empty() {
        if let Some(parent) = tree.node(id).parent {
            tree.broadcast(parent, &Broadcast::ScrollFirst(value), env);
        }
    } else {
        notify(
            tree,
            id,
            code::VALUE_CHANGED,
            &[("int", Param::Int(value))],
            env,
        );
    }
}

/// Set the value `FUN_00465250`: clamp, then report - on **every** call, even without a change (O-100).
pub(crate) fn set_value(tree: &mut UiTree, id: NodeId, v: i32, env: &mut UiEnv) {
    let s = state_mut(tree, id);
    s.value = clamp_value(v, s.resolution);
    report(tree, id, env);
}

/// Dragging (slot 18 `0x00465380`, only with `+0x9C`): horizontal `v = ((pt.x - A.l - 42)*(R - 1)) / (A.r - A.l - 84)`,
/// vertical with y, in the grid with `IDIV` (toward 0, no rounding to the nearest step). A denominator of 0 (slider
/// exactly 84 long, an exception in the original) yields 0.
fn drag(tree: &mut UiTree, id: NodeId, pt: [i32; 2], env: &mut UiEnv) {
    let a = tree.abs_rect(id);
    let vertical = tree.node(id).flags & slider_flags::VERTICAL != 0;
    let (p, lo, hi) = if vertical {
        (pt[1], a[1], a[3])
    } else {
        (pt[0], a[0], a[2])
    };
    let r = state(tree, id).resolution;
    let num = (i64::from(p) - i64::from(lo) - i64::from(THUMB)) * (i64::from(r) - 1);
    let den = i64::from(hi) - i64::from(lo) - 2 * i64::from(THUMB);
    let v = num.checked_div(den).unwrap_or(0);
    let v = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    set_value(tree, id, v, env);
}

/// Mouse slots: press (12 `0x004654E0`) in the start square (edge length = thickness) -> `v - 1`, in the end square ->
/// `v + 1`, otherwise dragging begins and slot 18 immediately; movement (18) drags; release (15 `0x00464A40`) and
/// double-click (6 jumps to 15) end dragging without a report; wheel (1 `0x00465180` up: `max(0, v - 1)`, 0
/// `0x004651E0` down: `max(0, min(R - 1, v + 1))`) reports only without a name (`0x140`) - named sliders change
/// only the display with the wheel (O-93). All `true`.
pub(crate) fn mouse_slot(
    tree: &mut UiTree,
    id: NodeId,
    slot: Slot,
    pt: [i32; 2],
    env: &mut UiEnv,
) -> bool {
    match slot {
        Slot::LeftDown => {
            let a = tree.abs_rect(id);
            let vertical = tree.node(id).flags & slider_flags::VERTICAL != 0;
            let s = if vertical {
                a[2].wrapping_sub(a[0])
            } else {
                a[3].wrapping_sub(a[1])
            };
            let v = state(tree, id).value;
            if contains(
                [a[0], a[1], a[0].wrapping_add(s), a[1].wrapping_add(s)],
                pt[0],
                pt[1],
            ) {
                set_value(tree, id, v.wrapping_sub(1), env);
            } else if contains(
                [a[2].wrapping_sub(s), a[3].wrapping_sub(s), a[2], a[3]],
                pt[0],
                pt[1],
            ) {
                set_value(tree, id, v.wrapping_add(1), env);
            } else {
                state_mut(tree, id).dragging = true;
                drag(tree, id, pt, env);
            }
            true
        }
        Slot::Move => {
            if state(tree, id).dragging {
                drag(tree, id, pt, env);
            }
            true
        }
        Slot::LeftUp | Slot::LeftDouble => {
            state_mut(tree, id).dragging = false;
            true
        }
        Slot::WheelUp | Slot::WheelDown => {
            let s = state_mut(tree, id);
            s.value = if slot == Slot::WheelUp {
                s.value.wrapping_sub(1).max(0)
            } else {
                s.value
                    .wrapping_add(1)
                    .min(s.resolution.wrapping_sub(1))
                    .max(0)
            };
            if tree.node(id).name.is_empty() {
                report(tree, id, env);
            }
            true
        }
        _ => base_mouse(slot),
    }
}

/// Query `FUN_00464B60` (from `OnAttach` and `Draw`): codes 9, 5 (lock from the flags), 0 (`int` -> value,
/// unclamped), 6 (`resolution` -> number of steps; overwrites `RESOLUTION` of the `.dlg`).
pub(crate) fn query(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if !due(state(tree, id).query_at, env.now) {
        return;
    }
    state_mut(tree, id).query_at = None;
    let (next, _) = query_head(tree, id, env);
    state_mut(tree, id).query_at = next;
    lock(tree.node_mut(id));
    let (_, msg) = notify(tree, id, code::QUERY_VALUE, &[], env);
    if let Some(v) = int_param(&msg, "int") {
        state_mut(tree, id).value = v;
    }
    let (_, msg) = notify(tree, id, code::QUERY_RESOLUTION, &[], env);
    if let Some(r) = int_param(&msg, "resolution") {
        state_mut(tree, id).resolution = r;
    }
}

/// `OnAttach` `0x00465660`: query, with [`AUTO_HIDE`] visible only for `R >= 2`, then the background set
/// `slider_vert.bkg` (vertical) or `slider_horz.bkg`.
pub(crate) fn on_attach(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    query(tree, id, env);
    let r = state(tree, id).resolution;
    let node = tree.node_mut(id);
    if node.flags & AUTO_HIDE != 0 {
        node.visible = r >= 2;
    }
    let path = if node.flags & slider_flags::VERTICAL != 0 {
        SLIDER_VERT_BKG
    } else {
        SLIDER_HORZ_BKG
    };
    let ui = &mut env.ui;
    node.bkg_sets = vec![load_bkg(ui.vfs, path, ui.assets)];
    node.bkg_set = 0;
}

/// `OnMessage` (slot 19 `0x00464AF0`): only `0x141` -> `R = max(0, steps)`, value clamped (without a report), with
/// [`AUTO_HIDE`] visible only for `R >= 2`; accepted. The slider does not handle `0x12E`.
pub(crate) fn on_message(tree: &mut UiTree, id: NodeId, msg: &Broadcast) -> bool {
    let Broadcast::ScrollRange { value, steps } = *msg else {
        return false;
    };
    let r = steps.max(0);
    let s = state_mut(tree, id);
    s.resolution = r;
    s.value = clamp_value(value, r);
    let node = tree.node_mut(id);
    if node.flags & AUTO_HIDE != 0 {
        node.visible = r >= 2;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{MouseButton, UiInput};
    use crate::widgets::testkit::Harness;

    /// Horizontal slider `A = (100, 100, 400, 142)`, `R = 20` in a frame without a border (client = grid).
    fn dlg(cur: i32) -> String {
        format!(
            "FRAME F FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
             SLIDER S EXTENT 100 100 400 142 CURVALUE {cur} RESOLUTION 20 /SLIDER\n/FRAME\n"
        )
    }

    fn slider(h: &Harness) -> (NodeId, &SliderState) {
        let id = h.tree.find("FS").unwrap();
        match &h.tree.node(id).kind {
            NodeKind::Slider(s) => (id, s),
            _ => unreachable!(),
        }
    }

    fn reported(h: &Harness) -> Vec<i32> {
        h.sent_ints("s", code::VALUE_CHANGED, "int")
    }

    #[test]
    fn drag_value_follows_the_integer_formula() {
        let mut h = Harness::new();
        h.build(&dlg(0));
        h.forget();
        // Point x 250: ((250 - 100 - 42)*19) / (300 - 84) = 2052 / 216 = 9 (divided toward 0).
        h.input(UiInput::Move { x: 250, y: 121 });
        h.input(UiInput::Down(MouseButton::Left));
        assert!(slider(&h).1.dragging);
        assert_eq!(slider(&h).1.value, 9);
        // Keep dragging, also outside (captured): 357 -> 215*19/216 = 18, 358 -> 19, 900 -> clamped 19, left -> 0.
        for x in [357, 358, 900, 145, 10] {
            h.input(UiInput::Move { x, y: 300 });
        }
        assert_eq!(reported(&h), [9, 18, 19, 19, 0, 0]);
        // Releasing ends dragging without a report.
        h.input(UiInput::Up(MouseButton::Left));
        assert!(!slider(&h).1.dragging);
        h.input(UiInput::Move { x: 300, y: 121 });
        assert_eq!(reported(&h).len(), 6);
    }

    #[test]
    fn end_squares_step_by_one() {
        // Thickness 42: start square (100, 100, 142, 142), end square (358, 100, 400, 142).
        let mut h = Harness::new();
        h.build(&dlg(5));
        h.forget();
        h.click((110, 120));
        assert_eq!(slider(&h).1.value, 4);
        h.click((390, 120));
        h.click((390, 120));
        assert_eq!(slider(&h).1.value, 6);
        assert_eq!(reported(&h), [4, 5, 6]);
        assert!(!slider(&h).1.dragging, "step squares do not drag");
    }

    #[test]
    fn slider_reports_code_1_on_every_call() {
        // O-100: code 1 arrives even without a change (clamped at the start).
        let mut h = Harness::new();
        h.build(&dlg(0));
        h.forget();
        h.click((110, 120));
        h.click((110, 120));
        assert_eq!(slider(&h).1.value, 0);
        assert_eq!(reported(&h), [0, 0]);
    }

    #[test]
    fn wheel_on_named_slider_only_moves_the_thumb() {
        // O-93: the wheel moves a named slider without a report and takes the event.
        let mut h = Harness::new();
        h.build(&dlg(18));
        h.input(UiInput::Move { x: 250, y: 121 });
        h.forget();
        assert!(h.input(UiInput::WheelDown));
        assert!(h.input(UiInput::WheelDown));
        assert_eq!(slider(&h).1.value, 19, "clamped to R - 1");
        assert!(h.input(UiInput::WheelUp));
        assert_eq!(slider(&h).1.value, 18);
        assert!(h.rec.msgs.is_empty());
    }

    #[test]
    fn resolution_answer_overrides_the_dlg() {
        // Code 6 `resolution` replaces `RESOLUTION 20`; code 0 `int` sets the value unclamped.
        let mut h = Harness::new();
        h.rec.answer = Box::new(|m| match m.code {
            code::QUERY_RESOLUTION => {
                m.params.insert("resolution".into(), Param::Int(5));
                0
            }
            code::QUERY_VALUE => {
                m.params.insert("int".into(), Param::Int(9));
                0
            }
            _ => -1,
        });
        h.build(&dlg(0));
        assert_eq!(h.codes("s"), [9, 5, 0, 6]);
        let s = slider(&h).1;
        assert_eq!((s.resolution, s.value), (5, 9));
        // Drag all the way to the right: at most R - 1 = 4.
        h.input(UiInput::Move { x: 250, y: 121 });
        h.input(UiInput::Down(MouseButton::Left));
        h.input(UiInput::Move { x: 399, y: 121 });
        assert_eq!(slider(&h).1.value, 4);
    }
}
