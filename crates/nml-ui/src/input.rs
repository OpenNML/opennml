//! Input of the menu UI (re/menu-system 3.1, 3.2 with the corrections 12.4, 4, 12.7, 12.8): mouse events,
//! double-click detection, the dispatcher `Widget_DispatchInput` (`FUN_0045B360`) and the keyboard path
//! `FUN_004C6050` without a match.
//!
//! The host supplies mouse values in screen pixels; the dispatcher converts them per event into the 1280 x 1024
//! grid. An event that no widget takes processes the scheduled deletions (`FUN_004B8E70`, 12.1.8);
//! the frame event [`UiInput::Frame`] (raw event 6) does the same, which the host sends once per frame after all
//! mouse events.

use crate::coords::{Screen, VIRTUAL_H, VIRTUAL_W};
use crate::geom::{contains, intersect};
use crate::keys::chord_name;
use crate::tree::{Broadcast, NodeId, UiEnv, UiTree};
use crate::widgets::{self, Slot};

/// Mouse button of the menu UI (the middle one does not reach widgets, 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

/// An input to the menu UI; mouse values in screen pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiInput {
    /// Mouse movement (raw kind 1 -> widget event 3); the host sends it only when the position has changed.
    Move { x: i32, y: i32 },
    /// Press (raw kind 3/5 -> event 4/8).
    Down(MouseButton),
    /// Release (raw kind 2/4 -> event 5/9).
    Up(MouseButton),
    /// Double-click (raw kind `0x13`/`0x12` -> event 11/10), after the corresponding `Up` (12.8, [`ClickTimer`]).
    DoubleClick(MouseButton),
    /// Wheel up (raw kind `0x2D` -> event 13).
    WheelUp,
    /// Wheel down (raw kind `0x2E` -> event 14).
    WheelDown,
    /// Key event: all held keys in press order (VK) and the character (cp1252, 12.7).
    Key { held: Vec<u8>, ch: Option<u16> },
    /// Frame raw event 6: not a widget event, only processes the scheduled deletions.
    Frame,
}

impl UiInput {
    /// Number of the widget event (`FUN_004B8E70`, table 3.1; key `0xF`); `Frame` has none.
    pub fn widget_event(&self) -> Option<u8> {
        Some(match self {
            UiInput::Move { .. } => 3,
            UiInput::Down(MouseButton::Left) => 4,
            UiInput::Up(MouseButton::Left) => 5,
            UiInput::Down(MouseButton::Right) => 8,
            UiInput::Up(MouseButton::Right) => 9,
            UiInput::DoubleClick(MouseButton::Right) => 10,
            UiInput::DoubleClick(MouseButton::Left) => 11,
            UiInput::WheelUp => 13,
            UiInput::WheelDown => 14,
            UiInput::Key { .. } => 0xF,
            UiInput::Frame => return None,
        })
    }
}

/// Double-click detection of the mouse source `FUN_004145B0` (12.8): per button the time of the last release
/// (`+0x4C` left, `+0x54` right).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClickTimer {
    last_release: [Option<u64>; 2],
}

impl ClickTimer {
    /// Release of `b` at time `now`: a double-click if less than `timeout_ms` has passed since the last release of
    /// the same button (unsigned); the time is reset on **every** release. The host then
    /// sends `DoubleClick(b)` after `Up(b)`. Before the first release there is no double-click (in the
    /// original `GetTickCount` is then far larger than the interval).
    pub fn release(&mut self, b: MouseButton, now: u64, timeout_ms: u32) -> bool {
        let slot = &mut self.last_release[b as usize];
        let double = matches!(*slot, Some(t) if now.wrapping_sub(t) < u64::from(timeout_ms));
        *slot = Some(now);
        double
    }
}

/// Position of the pointer in screen pixels; press, release and wheel act at this point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
}

/// Widget event in the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ev<'k> {
    /// Event 3.
    Move,
    /// Event 4.
    LeftDown,
    /// Event 5.
    LeftUp,
    /// Event 8.
    RightDown,
    /// Event 9.
    RightUp,
    /// Event 10.
    RightDouble,
    /// Event 11.
    LeftDouble,
    /// Event 13.
    WheelUp,
    /// Event 14.
    WheelDown,
    /// Event `0xF` with the key object.
    Key { held: &'k [u8], ch: Option<u16> },
}

/// Mouse input to the tree (`FUN_004B8E70` -> `Widget_DispatchInput` at the desktop). `Move` sets `pointer` first.
/// Returns whether a widget took the event; otherwise (and with `Frame`) the scheduled deletions are
/// processed. `Key` goes to [`dispatch_key`] (without deletions, like `FUN_004C6050`).
///
/// The conversion into the grid follows the dispatcher (`0x0045B3AA`-`0x0045B3F1`): `x' = (1280*x) / W`,
/// `y' = (1024*y) / H`, `IDIV` toward 0, without a rounding addend. With a screen width or height of 0 (the
/// original divided by 0) nothing is hit.
pub fn dispatch_mouse(
    tree: &mut UiTree,
    pointer: &mut Pointer,
    input: &UiInput,
    env: &mut UiEnv,
) -> bool {
    let ev = match input {
        UiInput::Move { x, y } => {
            *pointer = Pointer { x: *x, y: *y };
            Some(Ev::Move)
        }
        UiInput::Down(MouseButton::Left) => Some(Ev::LeftDown),
        UiInput::Up(MouseButton::Left) => Some(Ev::LeftUp),
        UiInput::Down(MouseButton::Right) => Some(Ev::RightDown),
        UiInput::Up(MouseButton::Right) => Some(Ev::RightUp),
        UiInput::DoubleClick(MouseButton::Left) => Some(Ev::LeftDouble),
        UiInput::DoubleClick(MouseButton::Right) => Some(Ev::RightDouble),
        UiInput::WheelUp => Some(Ev::WheelUp),
        UiInput::WheelDown => Some(Ev::WheelDown),
        UiInput::Key { held, ch } => return dispatch_key(tree, held, *ch, env),
        UiInput::Frame => None,
    };
    let pt = raster_point(*pointer, env.ui.screen);
    if pt.is_some() {
        tree.pointer = pt;
    }
    let taken = match (ev, pt) {
        (Some(ev), Some(pt)) => {
            let desktop = tree.desktop();
            dispatch(tree, desktop, ev, pt, env)
        }
        _ => false,
    };
    if !taken {
        tree.run_deletes(env);
    }
    taken
}

/// Key event without a match (`FUN_004C6050`, 12.7): if the combination is exactly `ESCAPE`, broadcast `0x17E` to the
/// desktop; exactly `RETURN` -> `0x17F` (Shift+ESC is not ESC, O-14). Not taken, or any other key:
/// widget event `0xF` along the focus path (no Tab or arrow navigation, 4). ESC without an acceptor has no
/// effect; the game menu (while a game is running) is not handled here.
pub fn dispatch_key(tree: &mut UiTree, held: &[u8], ch: Option<u16>, env: &mut UiEnv) -> bool {
    let name = chord_name(held);
    let desktop = tree.desktop();
    let mut taken = false;
    if name == "ESCAPE" {
        taken = tree.broadcast(desktop, &Broadcast::Escape, env);
    }
    if name == "RETURN" {
        taken = tree.broadcast(desktop, &Broadcast::Enter, env);
    }
    if !taken {
        taken = dispatch(tree, desktop, Ev::Key { held, ch }, [0, 0], env);
    }
    taken
}

/// Artificial mouse movement at the current pointer position from the topmost root (`FUN_0045B850` after
/// moving a widget: event 3 directly to `Widget_DispatchInput`, without deletions). Without a known
/// pointer position (no mouse event yet, screen 0) nothing happens.
pub(crate) fn synthetic_move(tree: &mut UiTree, env: &mut UiEnv) {
    if let Some(pt) = tree.pointer {
        let desktop = tree.desktop();
        dispatch(tree, desktop, Ev::Move, pt, env);
    }
}

/// Screen -> grid like in the dispatcher; `None` for a screen size of 0.
fn raster_point(p: Pointer, s: Screen) -> Option<[i32; 2]> {
    if s.width == 0 || s.height == 0 {
        return None;
    }
    let conv = |v: i32, full: i32, size: i32| {
        let r = i64::from(full) * i64::from(v) / i64::from(size);
        r.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    Some([
        conv(p.x, VIRTUAL_W, s.width),
        conv(p.y, VIRTUAL_H, s.height),
    ])
}

/// `Widget_DispatchInput` `FUN_0045B360` (disassembly read, 12.4):
///
/// 1. Invisible or locked -> `false`, even before the children.
/// 2. Children from top to bottom (end of the list first, predecessor read before the visit): a key only to the
///    child with focus; event 5 only if widget **and** child are captured; otherwise, if the widget is captured, only
///    captured children; if it is not captured, children with `pt` inside the child rectangle intersected with the
///    client area of the widget, or captured children. The first child that takes the event ends the search.
/// 3. Key: slot 22 if the widget itself has focus.
/// 4. Enter: without "mouse over" and `pt` in the own rectangle -> all leave (`FUN_00459B80` from the root),
///    `hover = 1`, slot 16, then `left pressed = captured`. Leave only when captured and `pt` outside: `hover = 0`,
///    slot 17. The return values of both slots do not count.
/// 5. Event by kind; with event 5 slot 7 (click) runs only with "left pressed" and `pt` in the rectangle, its
///    return value is discarded, slot 15 is returned (O-21 corrected).
///
/// Not reproduced: `+0x4D` (accepts mouse) is 1 for every widget, and the pixel test `ALPHATEST`
/// (`FUN_00459E20`) only has an effect with a set that carries `ALPHATEST` - in the shipped data only
/// `button_border.bkg` (set 4), which is never active (O-11).
fn dispatch(tree: &mut UiTree, id: NodeId, ev: Ev, pt: [i32; 2], env: &mut UiEnv) -> bool {
    if !tree.contains(id) {
        return false;
    }
    let node = tree.node(id);
    if !node.visible || !node.enabled {
        return false;
    }

    // 0x0045B400: start at the end of the list, `pPrev` before the visit (0x0045B412). A slot can delete widgets
    // immediately (`MenuLabel.OnAttach` via a broadcast): a child that no longer exists ends the search without a
    // panic.
    let mut cur = node.children.last().copied();
    while let Some(child) = cur {
        if !tree.contains(id) {
            return false;
        }
        if !tree.contains(child) {
            break;
        }
        let children = &tree.node(id).children;
        let pos = children.iter().position(|&c| c == child);
        let prev = pos.and_then(|p| p.checked_sub(1)).map(|p| children[p]);
        let eligible = match ev {
            Ev::Key { .. } => tree.node(child).focused,
            _ => {
                let parent_captured = tree.node(id).captured;
                let child_captured = tree.node(child).captured;
                if ev == Ev::LeftUp {
                    parent_captured && child_captured
                } else if parent_captured {
                    child_captured
                } else {
                    intersect(tree.abs_rect(child), tree.client_rect(id))
                        .is_some_and(|r| contains(r, pt[0], pt[1]))
                        || child_captured
                }
            }
        };
        if eligible && dispatch(tree, child, ev, pt, env) {
            return true;
        }
        cur = prev;
    }
    if !tree.contains(id) {
        return false;
    }

    if let Ev::Key { held, ch } = ev {
        return tree.node(id).focused && widgets::key_slot(tree, id, held, ch, env);
    }

    let rect = tree.abs_rect(id);
    let inside = contains(rect, pt[0], pt[1]);
    if !tree.node(id).hover {
        if inside {
            let desktop = tree.desktop();
            leave_all(tree, desktop, pt, env);
            tree.node_mut(id).hover = true;
            widgets::mouse_slot(tree, id, Slot::Enter, pt, env);
            let node = tree.node_mut(id);
            node.pressed_left = node.captured;
        }
    } else if tree.node(id).captured && !inside {
        tree.node_mut(id).hover = false;
        widgets::mouse_slot(tree, id, Slot::Leave, pt, env);
    }

    match ev {
        Ev::Move => widgets::mouse_slot(tree, id, Slot::Move, pt, env),
        Ev::LeftDown => {
            tree.node_mut(id).pressed_left = true;
            capture(tree, id);
            widgets::mouse_slot(tree, id, Slot::LeftDown, pt, env)
        }
        Ev::LeftUp => {
            if tree.node(id).pressed_left && contains(tree.abs_rect(id), pt[0], pt[1]) {
                // 0x0045B687: return value discarded.
                widgets::mouse_slot(tree, id, Slot::LeftClick, pt, env);
            }
            tree.node_mut(id).pressed_left = false;
            let desktop = tree.desktop();
            release_all(tree, desktop);
            widgets::mouse_slot(tree, id, Slot::LeftUp, pt, env)
        }
        Ev::RightDown => {
            tree.node_mut(id).pressed_right = true;
            widgets::mouse_slot(tree, id, Slot::RightDown, pt, env)
        }
        Ev::RightUp => {
            if tree.node(id).pressed_right && contains(tree.abs_rect(id), pt[0], pt[1]) {
                widgets::mouse_slot(tree, id, Slot::RightClick, pt, env);
            }
            tree.node_mut(id).pressed_right = false;
            widgets::mouse_slot(tree, id, Slot::RightUp, pt, env)
        }
        Ev::RightDouble => widgets::mouse_slot(tree, id, Slot::RightDouble, pt, env),
        Ev::LeftDouble => widgets::mouse_slot(tree, id, Slot::LeftDouble, pt, env),
        Ev::WheelUp => widgets::mouse_slot(tree, id, Slot::WheelUp, pt, env),
        Ev::WheelDown => widgets::mouse_slot(tree, id, Slot::WheelDown, pt, env),
        Ev::Key { .. } => unreachable!("key handled above"),
    }
}

/// `FUN_00459B80`: every widget with "mouse over" leaves (slot 17) and loses the press flags; pre-order,
/// children from the head.
fn leave_all(tree: &mut UiTree, id: NodeId, pt: [i32; 2], env: &mut UiEnv) {
    if tree.node(id).hover {
        tree.node_mut(id).hover = false;
        widgets::mouse_slot(tree, id, Slot::Leave, pt, env);
        let node = tree.node_mut(id);
        node.pressed_left = false;
        node.pressed_right = false;
    }
    for child in tree.node(id).children.clone() {
        if tree.contains(child) {
            leave_all(tree, child, pt, env);
        }
    }
}

/// `FUN_0045B150`: release the capture in the whole tree, then set it for the widget and all ancestors.
fn capture(tree: &mut UiTree, id: NodeId) {
    let desktop = tree.desktop();
    release_all(tree, desktop);
    let mut cur = Some(id);
    while let Some(c) = cur {
        let node = tree.node_mut(c);
        node.captured = true;
        cur = node.parent;
    }
}

/// `FUN_004598A0`: release the capture in the subtree.
fn release_all(tree: &mut UiTree, id: NodeId) {
    for child in tree.node(id).children.clone() {
        if tree.contains(child) {
            release_all(tree, child);
        }
    }
    tree.node_mut(id).captured = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::{UiEffect, code};
    use crate::tree::NodeKind;
    use crate::widgets::Trace;
    use crate::widgets::testkit::{Harness, center};

    /// Synthetic dialog: two buttons, `OK` with `CHECKENTER`, `CANCEL` with `CHECKESCAPE`.
    const DLG: &str = "FRAME DLG EXTENT 100 100 600 500\n\
         BUTTON OK FLAGS MENUBUTTON_TEXT MENUBUTTON_CHECKENTER /FLAGS EXTENT 10 10 110 50 /BUTTON\n\
         BUTTON CANCEL FLAGS MENUBUTTON_TEXT MENUBUTTON_CHECKESCAPE /FLAGS EXTENT 200 10 300 50 /BUTTON\n\
         /FRAME\n";

    fn click_sfx() -> UiEffect {
        UiEffect::PlaySfx {
            event: "SFX_KLICK".into(),
            flags: 0x14,
            category: 1,
        }
    }

    fn triggered(h: &Harness) -> Vec<(String, String)> {
        h.rec
            .msgs
            .iter()
            .filter(|m| m.2 == code::TRIGGERED)
            .map(|m| (m.0.clone(), m.1.clone()))
            .collect()
    }

    fn pair(d: &str, e: &str) -> (String, String) {
        (d.to_string(), e.to_string())
    }

    #[test]
    fn click_sends_code_8_with_one_click_sound() {
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(DLG);
        let ok = h.tree.find("DLGOK").unwrap();
        let (x, y) = center(&h.tree, ok);
        assert!(h.input(UiInput::Move { x, y }));
        assert!(h.input(UiInput::Down(MouseButton::Left)));
        assert!(triggered(&h).is_empty() && h.fx.is_empty());
        assert!(h.input(UiInput::Up(MouseButton::Left)));
        assert_eq!(triggered(&h), [pair("dlg", "ok")]);
        assert_eq!(h.fx, [click_sfx()]);
        // The capture is released in the whole tree after the release.
        assert!(!h.tree.node(h.tree.desktop()).captured);
        assert!(!h.tree.node(ok).captured);
    }

    #[test]
    fn click_needs_press_and_release_inside() {
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(DLG);
        let ok = h.tree.find("DLGOK").unwrap();
        let cancel = h.tree.find("DLGCANCEL").unwrap();
        let (x, y) = center(&h.tree, ok);
        h.input(UiInput::Move { x, y });
        h.input(UiInput::Down(MouseButton::Left));
        // Capture: button and all ancestors.
        let frame = h.tree.find("DLG").unwrap();
        for id in [ok, frame, h.tree.dialog_root(), h.tree.desktop()] {
            assert!(h.tree.node(id).captured, "{id:?}");
        }
        // Dragging onto the other button while pressed: the capture keeps the events with the first one.
        let (cx, cy) = center(&h.tree, cancel);
        h.tree.trace.clear();
        assert!(h.input(UiInput::Move { x: cx, y: cy }));
        assert!(!h.tree.node(cancel).hover);
        assert!(!h.tree.node(ok).hover);
        assert!(
            h.tree
                .trace
                .iter()
                .all(|t| !matches!(t, Trace::Mouse(id, _) if *id == cancel))
        );
        assert!(h.tree.trace.contains(&Trace::Mouse(ok, Slot::Leave)));
        // Release outside: the button takes it (slot 15), but without a click.
        h.tree.trace.clear();
        assert!(h.input(UiInput::Up(MouseButton::Left)));
        assert!(h.tree.trace.contains(&Trace::Mouse(ok, Slot::LeftUp)));
        assert!(!h.tree.trace.contains(&Trace::Mouse(ok, Slot::LeftClick)));
        assert!(triggered(&h).is_empty());
        assert!(h.fx.is_empty());
        // Press outside, release on the button: likewise no click.
        h.input(UiInput::Move { x: 50, y: 50 });
        h.input(UiInput::Down(MouseButton::Left));
        h.input(UiInput::Move { x, y });
        h.input(UiInput::Up(MouseButton::Left));
        assert!(triggered(&h).is_empty());
    }

    #[test]
    fn release_return_of_slot_7_is_ignored() {
        // O-21 corrected: a refused click (transition running, `0x12F` != 0) does not fall through.
        let mut h = Harness::new();
        h.rec.allow = false;
        h.build(DLG);
        let ok = h.tree.find("DLGOK").unwrap();
        let (x, y) = center(&h.tree, ok);
        h.input(UiInput::Move { x, y });
        h.input(UiInput::Down(MouseButton::Left));
        h.tree.trace.clear();
        assert!(h.input(UiInput::Up(MouseButton::Left)));
        assert_eq!(
            h.tree.trace,
            [
                Trace::Mouse(ok, Slot::LeftClick),
                Trace::Mouse(ok, Slot::LeftUp)
            ]
        );
        assert!(triggered(&h).is_empty());
        assert!(h.fx.is_empty());
    }

    #[test]
    fn escape_triggers_the_first_attached_checkescape_button_without_sound() {
        // O-14, O-15, O-16: two buttons with CHECKESCAPE; the one attached first wins, even hidden and under
        // a modal dialog; no click sound.
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(
            "FRAME BACK EXTENT 0 0 400 300\n\
             BUTTON QUIT FLAGS HIDE MENUBUTTON_CHECKESCAPE /FLAGS EXTENT 0 0 50 50 /BUTTON\n/FRAME\n",
        );
        h.build(
            "FRAME MSGBOX FLAGS MENUFRAME_MODAL /FLAGS EXTENT 100 100 500 400\n\
             BUTTON NO FLAGS MENUBUTTON_CHECKESCAPE /FLAGS EXTENT 0 0 50 50 /BUTTON\n/FRAME\n",
        );
        let quit = h.tree.find("BACKQUIT").unwrap();
        assert!(!h.tree.node(quit).enabled, "HIDE locks");
        assert!(h.key(&[0x1B], Some(0x1B)));
        assert_eq!(triggered(&h), [pair("back", "quit")]);
        assert!(h.fx.is_empty());
        // ENTER without a CHECKENTER button: no acceptor, the event goes to the focus path (nobody takes it there).
        h.rec.msgs.clear();
        assert!(!h.key(&[0x0D], Some(0x0D)));
        assert!(triggered(&h).is_empty());
        // Without permission (`0x12F` != 0) nobody takes ESC.
        h.rec.allow = false;
        assert!(!h.key(&[0x1B], Some(0x1B)));
        assert!(triggered(&h).is_empty());
    }

    #[test]
    fn enter_triggers_the_checkenter_button() {
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(DLG);
        assert!(h.key(&[0x0D], Some(0x0D)));
        assert_eq!(triggered(&h), [pair("dlg", "ok")]);
        h.rec.msgs.clear();
        assert!(h.key(&[0x1B], Some(0x1B)));
        assert_eq!(triggered(&h), [pair("dlg", "cancel")]);
        assert!(h.fx.is_empty());
    }

    #[test]
    fn escape_with_shift_is_not_escape() {
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(DLG);
        h.tree.trace.clear();
        assert!(!h.key(&[0x10, 0x1B], Some(0x1B)));
        assert!(!h.key(&[0xA0, 0x0D], Some(0x0D)));
        assert!(triggered(&h).is_empty());
        // No broadcast: only the widget event 0xF came (the desktop has no focus, so nothing).
        assert!(h.tree.trace.is_empty());
        // Counter-check: ESC alone triggers.
        assert!(h.key(&[0x1B], Some(0x1B)));
        assert_eq!(triggered(&h), [pair("dlg", "cancel")]);
    }

    #[test]
    fn double_click_sequence_is_4_5_4_5_11() {
        // O-106, 12.8: mouse source with `DoubleClickTimeout` 200 ms; the second click is a full click.
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(DLG);
        let ok = h.tree.find("DLGOK").unwrap();
        let (x, y) = center(&h.tree, ok);
        h.input(UiInput::Move { x, y });
        let mut timer = ClickTimer::default();
        let mut events = Vec::new();
        h.tree.trace.clear();
        // Release at 10 ms and 150 ms: only the second lies within 200 ms.
        for up in [10, 150] {
            for input in [
                UiInput::Down(MouseButton::Left),
                UiInput::Up(MouseButton::Left),
            ] {
                events.push(input.widget_event().unwrap());
                h.input(input);
            }
            if timer.release(MouseButton::Left, up, 200) {
                let input = UiInput::DoubleClick(MouseButton::Left);
                events.push(input.widget_event().unwrap());
                assert!(h.input(input), "base slot 6 takes the double-click");
            }
        }
        assert_eq!(events, [4, 5, 4, 5, 11]);
        let slots: Vec<Slot> = h
            .tree
            .trace
            .iter()
            .filter_map(|t| match t {
                Trace::Mouse(id, s) if *id == ok => Some(*s),
                _ => None,
            })
            .collect();
        assert_eq!(
            slots,
            [
                Slot::LeftDown,
                Slot::LeftClick,
                Slot::LeftUp,
                Slot::LeftDown,
                Slot::LeftClick,
                Slot::LeftUp,
                Slot::LeftDouble
            ]
        );
        assert_eq!(triggered(&h), [pair("dlg", "ok"), pair("dlg", "ok")]);
        assert_eq!(h.fx, [click_sfx(), click_sfx()]);
    }

    #[test]
    fn click_timer_restarts_on_every_release() {
        let mut t = ClickTimer::default();
        assert!(!t.release(MouseButton::Left, 0, 200), "first release");
        assert!(t.release(MouseButton::Left, 199, 200));
        assert!(
            t.release(MouseButton::Left, 398, 200),
            "time reset on every release"
        );
        assert!(
            !t.release(MouseButton::Left, 598, 200),
            "exactly the interval is not a double-click"
        );
        // Right counts separately.
        assert!(!t.release(MouseButton::Right, 600, 200));
        assert!(t.release(MouseButton::Right, 700, 200));
        assert!(!t.release(MouseButton::Left, 650, 0), "interval 0: never");
    }

    #[test]
    fn wheel_falls_through_buttons() {
        let mut h = Harness::new();
        h.build(DLG);
        let ok = h.tree.find("DLGOK").unwrap();
        let (x, y) = center(&h.tree, ok);
        h.input(UiInput::Move { x, y });
        h.tree.trace.clear();
        assert!(!h.input(UiInput::WheelUp));
        assert!(!h.input(UiInput::WheelDown));
        // Button, frame, dialog root and desktop received the wheel and let it through.
        assert_eq!(h.tree.trace.first(), Some(&Trace::Mouse(ok, Slot::WheelUp)));
        // Not taken -> scheduled deletions run.
        let frame = h.tree.find("DLG").unwrap();
        let root = h.tree.dialog_root();
        h.tree.schedule_delete(root, frame);
        assert!(!h.input(UiInput::WheelUp));
        assert!(!h.tree.contains(frame));
    }

    #[test]
    fn taken_events_keep_scheduled_deletes_until_the_frame_event() {
        let mut h = Harness::new();
        h.build(DLG);
        let frame = h.tree.find("DLG").unwrap();
        let root = h.tree.dialog_root();
        h.tree.schedule_delete(root, frame);
        // The visible dialog root takes every movement (base slot); nothing is deleted.
        assert!(h.input(UiInput::Move { x: 5, y: 5 }));
        assert!(h.tree.contains(frame));
        assert!(!h.input(UiInput::Frame));
        assert!(!h.tree.contains(frame));
    }

    #[test]
    fn focus_key_goes_only_to_the_focused_path() {
        let mut h = Harness::new();
        h.build(DLG);
        let ok = h.tree.find("DLGOK").unwrap();
        let cancel = h.tree.find("DLGCANCEL").unwrap();
        let frame = h.tree.find("DLG").unwrap();
        // Without focus a key reaches no widget.
        h.tree.trace.clear();
        assert!(!h.key(&[0x41], Some(u16::from(b'a'))));
        assert!(h.tree.trace.is_empty());
        // With focus on CANCEL: slot 22 from the inside out only along the chain, never OK.
        h.tree.set_focus(cancel);
        assert!(!h.key(&[0x41], Some(u16::from(b'a'))));
        let desktop = h.tree.desktop();
        let root = h.tree.dialog_root();
        assert_eq!(
            h.tree.trace,
            [
                Trace::Key(cancel),
                Trace::Key(frame),
                Trace::Key(root),
                Trace::Key(desktop)
            ]
        );
        assert!(!h.tree.trace.contains(&Trace::Key(ok)));
        // Arrow keys and Tab do not move the focus.
        h.key(&[0x09], None);
        h.key(&[0x27], None);
        assert!(h.tree.node(cancel).focused && !h.tree.node(ok).focused);
        // A locked widget on the path stops the key.
        h.tree.node_mut(frame).enabled = false;
        h.tree.trace.clear();
        h.key(&[0x41], None);
        assert_eq!(h.tree.trace, [Trace::Key(root), Trace::Key(desktop)]);
    }

    #[test]
    fn hidden_dialog_root_and_zero_screen_hit_nothing() {
        let mut h = Harness::new();
        h.build(DLG);
        let root = h.tree.dialog_root();
        h.tree.node_mut(root).visible = false;
        h.tree.trace.clear();
        assert!(!h.input(UiInput::Move { x: 300, y: 300 }));
        let desktop = h.tree.desktop();
        assert!(
            h.tree
                .trace
                .iter()
                .all(|t| matches!(t, Trace::Mouse(id, _) if *id == desktop))
        );
        h.tree.node_mut(root).visible = true;
        h.screen = crate::coords::Screen {
            width: 0,
            height: 0,
        };
        h.tree.trace.clear();
        assert!(!h.input(UiInput::Move { x: 300, y: 300 }));
        assert!(h.tree.trace.is_empty());
    }

    #[test]
    fn pointer_is_converted_with_integer_division() {
        // 1024 x 768: x' = 1280*x/1024, y' = 1024*y/768, divided toward 0, without the rounding addend of
        // `to_virtual_x/y` (FUN_00459570/FUN_00459610).
        let s = Screen {
            width: 1024,
            height: 768,
        };
        assert_eq!(raster_point(Pointer { x: 99, y: 104 }, s), Some([123, 138]));
        assert_eq!(
            [
                crate::coords::to_virtual_x(s, 99),
                crate::coords::to_virtual_y(s, 104)
            ],
            [124, 139]
        );
        assert_eq!(raster_point(Pointer { x: -5, y: -5 }, s), Some([-6, -6]));
        assert_eq!(
            raster_point(
                Pointer { x: 1, y: 1 },
                Screen {
                    width: 0,
                    height: 768
                }
            ),
            None
        );
        // In the tree: the button is at [125, 140, 225, 180]; x = 99 -> 123 outside, x = 100 -> 125 inside.
        let mut h = Harness::new();
        h.screen = s;
        h.build(DLG);
        let ok = h.tree.find("DLGOK").unwrap();
        assert_eq!(h.tree.abs_rect(ok), [125, 140, 225, 180]);
        h.input(UiInput::Move { x: 99, y: 105 });
        assert!(!h.tree.node(ok).hover);
        h.input(UiInput::Move { x: 100, y: 105 });
        assert!(h.tree.node(ok).hover);
    }

    #[test]
    fn blocker_swallows_clicks_and_keeps_the_wheel() {
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(DLG);
        h.build(
            "FRAME MSGBOX FLAGS MENUFRAME_MODAL /FLAGS EXTENT 700 600 900 800\n\
             BUTTON YES EXTENT 0 0 50 50 /BUTTON\n/FRAME\n",
        );
        let ok = h.tree.find("DLGOK").unwrap();
        let blocker = h.tree.node(h.tree.desktop()).children[1];
        assert!(matches!(
            h.tree.node(blocker).kind,
            NodeKind::ModalBlocker { .. }
        ));
        let (x, y) = center(&h.tree, ok);
        h.tree.trace.clear();
        h.input(UiInput::Move { x, y });
        h.input(UiInput::Down(MouseButton::Left));
        h.input(UiInput::Up(MouseButton::Left));
        assert!(triggered(&h).is_empty());
        assert!(!h.tree.node(ok).hover);
        assert!(
            h.tree
                .trace
                .contains(&Trace::Mouse(blocker, Slot::LeftClick))
        );
        // The wheel falls through the blocker to the button below (and on from there, O-93).
        h.tree.trace.clear();
        assert!(!h.input(UiInput::WheelUp));
        assert!(h.tree.trace.contains(&Trace::Mouse(ok, Slot::WheelUp)));
    }
}
