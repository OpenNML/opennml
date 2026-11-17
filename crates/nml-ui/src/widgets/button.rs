//! `MenuButton` (vtable `0x0061A478`, also the ComboBox with bit `0x10`): background set, click, ESC/ENTER and the
//! "pulled" queries (re/menu-system 3.5, 12.5, 12.6). Opening the ComboBox list is in [`super::combo`].

use nml_formats::dlg::desc::{button_flags, common_flags};
use nml_formats::dlg::{ButtonDesc, ListDesc};

use super::{Slot, combo, lock, query_head};
use crate::msg::{Param, UiEffect, code};
use crate::tree::{Broadcast, Node, NodeId, NodeKind, UiEnv, UiTree};

/// Click sound (`SoundManager_PlayEvent2D` `0x0045F026`...`0x0045F042`, re/menu-system 12.6).
const CLICK_SFX: &str = "SFX_KLICK";
const CLICK_SFX_FLAGS: u32 = 0x14;
const CLICK_SFX_CATEGORY: u8 = 1;

/// Background sets in constructor order (`FUN_0045EE80`, 3.5).
pub const SET_NORMAL: usize = 0;
pub const SET_MOUSEOVER: usize = 1;
pub const SET_PRESSED: usize = 2;
pub const SET_DISABLED: usize = 3;

/// State of a `MenuButton`; `list` is the list description of the ComboBox (`FUN_0045EAD0`).
#[derive(Debug, Clone)]
pub struct ButtonState {
    pub desc: ButtonDesc,
    pub list: Option<ListDesc>,
    /// `+0xA4`: text from the response to code 7; replaces `TEXT` of the file.
    pub text: Option<String>,
    /// `+0x10C`: ComboBox list opened.
    pub list_open: bool,
    /// `+0x94`: the open `ComboPopup`.
    pub popup: Option<NodeId>,
    /// ComboBox `+0x110`: entries from the response to code 2 (`array`).
    pub items: Vec<String>,
    /// ComboBox `+0x114`: values for the entries (`indexarray`).
    pub index_map: Vec<i32>,
    /// ComboBox `+0xE4`: selected entry (index into `items`, -1 = none); initially `CURSELECTION`.
    pub selection: i32,
    /// `+0x118`: next query time (ms of the program clock); `None` = never again. The constructor sets 1 in the
    /// original (due as soon as `GetTickCount() >= 1`, i.e. immediately); here `Some(0)`, so that the first query also
    /// runs at program time 0.
    pub query_at: Option<u64>,
}

impl ButtonState {
    /// Constructor `FUN_0045EE80`: query due immediately, list closed, no text from the logic.
    pub fn new(desc: ButtonDesc, list: Option<ListDesc>) -> ButtonState {
        let selection = list.as_ref().map_or(-1, |l| l.cur_selection);
        ButtonState {
            desc,
            list,
            text: None,
            list_open: false,
            popup: None,
            items: Vec::new(),
            index_map: Vec::new(),
            selection,
            query_at: Some(0),
        }
    }
}

fn state(tree: &UiTree, id: NodeId) -> &ButtonState {
    match &tree.node(id).kind {
        NodeKind::Button(s) => s,
        _ => panic!("widget {id:?} is not a button"),
    }
}

pub(crate) fn state_mut(tree: &mut UiTree, id: NodeId) -> &mut ButtonState {
    match &mut tree.node_mut(id).kind {
        NodeKind::Button(s) => s,
        _ => panic!("widget {id:?} is not a button"),
    }
}

/// Set selection `FUN_0045E080`: locked -> 3, mouse over and left pressed -> 2, mouse over -> 1, otherwise 0. Only the
/// constructor, enter/leave, press/release and `0x12C`/`0x12D` call it (12.5) - the query does not; a button that is
/// locked from the start therefore keeps set 0 (O-91).
pub(crate) fn select_bkg_set(node: &mut Node) {
    node.bkg_set = if !node.enabled {
        SET_DISABLED
    } else if node.hover && node.pressed_left {
        SET_PRESSED
    } else if node.hover {
        SET_MOUSEOVER
    } else {
        SET_NORMAL
    };
}

/// Whether `Draw` draws the button: not with an open list (`0x0045EBF2`, O-18) - the popup stands in its place.
pub fn draws_itself(node: &Node) -> bool {
    !matches!(&node.kind, NodeKind::Button(s) if s.list_open)
}

/// Text color in `Draw` (`0x0045EC85`...`0x0045ECD4`): locked or `DISABLE` -> `DISABLEDTEXTCOLOR`; mouse over and
/// pressed -> `PRESSEDBUTTONTEXTCOLOR`; mouse over -> `MOUSEOVERTEXTCOLOR`; otherwise `TEXTCOLOR`. If one of the two
/// optional colors is missing, `TEXTCOLOR` applies (O-9). `None` for nodes other than buttons.
pub fn text_color(node: &Node) -> Option<u32> {
    let NodeKind::Button(s) = &node.kind else {
        return None;
    };
    let d = &s.desc;
    Some(
        if !node.enabled || node.flags & common_flags::DISABLE != 0 {
            d.disabled_text_color
        } else if node.hover && node.pressed_left {
            d.pressed_text_color.unwrap_or(d.text_color)
        } else if node.hover {
            d.mouse_over_text_color.unwrap_or(d.text_color)
        } else {
            d.text_color
        },
    )
}

/// Mouse slots of the button: enter/leave (`0x0045E250`) and press/release left (`0x0045E270`) select the set and
/// take the event, except with an open list (`false`); click `0x0045EFD0`; wheel falls through; the others like the
/// base (`true`).
pub(crate) fn mouse_slot(tree: &mut UiTree, id: NodeId, slot: Slot, env: &mut UiEnv) -> bool {
    match slot {
        Slot::Enter | Slot::Leave | Slot::LeftDown | Slot::LeftUp => {
            if state(tree, id).list_open {
                return false;
            }
            select_bkg_set(tree.node_mut(id));
            true
        }
        Slot::LeftClick => click(tree, id, env),
        Slot::WheelUp | Slot::WheelDown => false,
        _ => true,
    }
}

/// Left click `MenuButton_OnLeftClick` `0x0045EFD0` (3.5): list open -> `false`; permission `0x12F` != 0 -> `false`
/// (-1 without a listener counts as "no"); otherwise `SFX_KLICK` and code 8. The dispatcher discards the return value
/// (12.4): a refused click does not fall through (O-21).
fn click(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) -> bool {
    if state(tree, id).list_open || env.listener.may_trigger() != 0 {
        return false;
    }
    env.fx.push(UiEffect::PlaySfx {
        event: CLICK_SFX.to_string(),
        flags: CLICK_SFX_FLAGS,
        category: CLICK_SFX_CATEGORY,
    });
    let node = tree.node(id);
    if node.flags & button_flags::COMBOBOX != 0 && node.hover {
        // ComboBox with mouse over: create the `ComboPopup` and attach it to the parent (`0x0045F047`...`0x0045F0A4`).
        combo::open(tree, id, env);
        return true;
    }
    let mut msg = tree.element_msg(id, code::TRIGGERED);
    env.listener.element(&mut msg, env.fx);
    true
}

/// `OnMessage` `FUN_0045E8A0`: `0x17E`/`0x17F` (`FUN_0045E0C0`/`FUN_0045E170`) only with `CHECKESCAPE` or
/// `CHECKENTER` and permission `0x12F` = 0 -> code 8, taken - without click sound, without checking visibility or
/// lock (O-15, O-16). Broadcast `0x12E` code 5: timer to "due", query immediately, taken (12.5). `0x12C`/`0x12D` of
/// the ComboBox list only with an open list ([`combo::pick`], [`combo::cancel`]).
pub(crate) fn on_message(tree: &mut UiTree, id: NodeId, msg: &Broadcast, env: &mut UiEnv) -> bool {
    let flag = match msg {
        Broadcast::Escape => button_flags::CHECKESCAPE,
        Broadcast::Enter => button_flags::CHECKENTER,
        Broadcast::Requery => {
            state_mut(tree, id).query_at = Some(0);
            query(tree, id, env);
            return true;
        }
        Broadcast::ListPick(index) => return combo::pick(tree, id, *index, env),
        Broadcast::ListCancel => return combo::cancel(tree, id),
        _ => return false,
    };
    if tree.node(id).flags & flag == 0 || env.listener.may_trigger() != 0 {
        return false;
    }
    let mut m = tree.element_msg(id, code::TRIGGERED);
    env.listener.element(&mut m, env.fx);
    true
}

/// Query `FUN_0045E350` (from `OnAttach` `0x0045E700` and at the start of `Draw` `0x0045EBED`), only when the timer is
/// due, which is cleared in the process:
/// - Code 9 `timeout` -> next query `now + timeout`; 0 or no response -> never again (except broadcast `0x12E`
///   code 5);
/// - Code 5 `status` -> `flags = (flags & !0x30000000) | status`; afterwards **always** `enabled = (flags &
///   0x30000000) == 0` - even without a response, so `HIDE`/`DISABLE` of the file lock already at construction;
/// - Code 7 `text` -> text, only if the listener returns exactly 1;
/// - ComboBox (bit `0x10`): codes 2 and 0 ([`combo::query_items`]).
///
/// The background set stays (O-91).
pub(crate) fn query(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if !super::due(state(tree, id).query_at, env.now) {
        return;
    }
    state_mut(tree, id).query_at = None;
    let (next, _) = query_head(tree, id, env);
    state_mut(tree, id).query_at = next;
    lock(tree.node_mut(id));

    let mut msg = tree.element_msg(id, code::QUERY_TEXT);
    if env.listener.element(&mut msg, env.fx) == 1
        && let Some(Param::Text(text)) = msg.params.get("text")
    {
        state_mut(tree, id).text = Some(text.clone());
    }
    if tree.node(id).flags & button_flags::COMBOBOX != 0 {
        combo::query_items(tree, id, env);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{MouseButton, UiInput};
    use crate::widgets::testkit::{Harness, center};

    const TWO: &str = "FRAME DLG EXTENT 100 100 600 500\n\
         BUTTON OK FLAGS MENUBUTTON_TEXT /FLAGS EXTENT 10 10 110 50 /BUTTON\n\
         BUTTON OFF FLAGS MENUBUTTON_TEXT DISABLE /FLAGS EXTENT 200 10 300 50 /BUTTON\n/FRAME\n";

    fn click_at(h: &mut Harness, (x, y): (i32, i32)) {
        h.input(UiInput::Move { x, y });
        h.input(UiInput::Down(MouseButton::Left));
        h.input(UiInput::Up(MouseButton::Left));
    }

    // QUIRK O-91: kept
    #[test]
    fn disabled_button_takes_no_slot_and_keeps_the_normal_set() {
        // O-91: `DISABLE` from the file locks already at the first query during construction, even without a response.
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(TWO);
        let off = h.tree.find("DLGOFF").unwrap();
        let node = h.tree.node(off);
        assert!(!node.enabled);
        assert_eq!(node.bkg_set, SET_NORMAL);
        assert_eq!(text_color(node), Some(0xFFB0_B0B0));
        h.tree.trace.clear();
        h.rec.msgs.clear();
        let at = center(&h.tree, off);
        click_at(&mut h, at);
        assert!(h.rec.msgs.is_empty(), "{:?}", h.rec.msgs);
        assert!(h.fx.is_empty());
        assert!(
            h.tree
                .trace
                .iter()
                .all(|t| !matches!(t, crate::widgets::Trace::Mouse(id, _) if *id == off))
        );
        let node = h.tree.node(off);
        assert!(!node.hover && !node.pressed_left);
        assert_eq!(node.bkg_set, SET_NORMAL);

        // Locked by `status 0x20000000` (response to code 5): likewise, set 0 and locked text color.
        let mut h = Harness::new();
        h.rec.allow = true;
        h.rec.answer = Box::new(|m| {
            if m.element == "ok" && m.code == code::QUERY_STATUS {
                m.params.insert("status".into(), Param::Int(0x2000_0000));
                0
            } else {
                -1
            }
        });
        h.build(TWO);
        let ok = h.tree.find("DLGOK").unwrap();
        let node = h.tree.node(ok);
        assert!(!node.enabled);
        assert_eq!(node.flags & common_flags::DISABLE, common_flags::DISABLE);
        assert_eq!(node.bkg_set, SET_NORMAL);
        assert_eq!(text_color(node), Some(node_desc_disabled(node)));
        h.rec.msgs.clear();
        let at = center(&h.tree, ok);
        click_at(&mut h, at);
        assert!(h.rec.msgs.is_empty());
        assert!(h.fx.is_empty());
    }

    fn node_desc_disabled(node: &Node) -> u32 {
        match &node.kind {
            NodeKind::Button(s) => s.desc.disabled_text_color,
            _ => unreachable!(),
        }
    }

    // QUIRK O-16: kept
    #[test]
    fn hover_and_press_select_sets_1_and_2() {
        let mut h = Harness::new();
        h.build(TWO);
        let ok = h.tree.find("DLGOK").unwrap();
        let frame = h.tree.find("DLG").unwrap();
        let (x, y) = center(&h.tree, ok);
        let set = |h: &Harness| h.tree.node(ok).bkg_set;
        assert_eq!(set(&h), SET_NORMAL);
        h.input(UiInput::Move { x, y });
        assert_eq!(set(&h), SET_MOUSEOVER);
        assert!(h.tree.node(ok).hover);
        h.input(UiInput::Down(MouseButton::Left));
        assert_eq!(set(&h), SET_PRESSED);
        // Drag out while pressed: leave via the capture, set 0; back: set 2 (left still pressed).
        h.input(UiInput::Move { x: 50, y: 50 });
        assert_eq!(set(&h), SET_NORMAL);
        assert!(!h.tree.node(ok).hover);
        h.input(UiInput::Move { x, y });
        assert_eq!(set(&h), SET_PRESSED);
        h.input(UiInput::Up(MouseButton::Left));
        assert_eq!(set(&h), SET_MOUSEOVER);
        // Onto the frame next to it: all others leave (`FUN_00459B80`). The frame lets the movement through
        // (`0x00460590` -> 0), so the dialog root enters afterwards and is the only one with "mouse over".
        let fr = h.tree.abs_rect(frame);
        h.input(UiInput::Move {
            x: fr[2] - 30,
            y: fr[3] - 30,
        });
        assert_eq!(set(&h), SET_NORMAL);
        assert!(!h.tree.node(ok).hover);
        assert!(!h.tree.node(frame).hover);
        assert!(h.tree.node(h.tree.dialog_root()).hover);
        // No sound on "mouse over" (O-16).
        assert!(h.fx.is_empty());
    }

    #[test]
    fn status_query_follows_the_timeout() {
        let mut h = Harness::new();
        let mut first = true;
        h.rec.answer = Box::new(move |m| {
            if m.element == "ok" && m.code == code::QUERY_TIMEOUT && first {
                first = false;
                m.params.insert("timeout".into(), Param::Int(100));
                return 0;
            }
            if m.element == "ok" && m.code == code::QUERY_TEXT {
                m.params
                    .insert("text".into(), Param::Text("Continue".into()));
                return 1;
            }
            -1
        });
        h.now = 1000;
        h.build(TWO);
        let ok = h.tree.find("DLGOK").unwrap();
        let codes = |h: &Harness| -> Vec<u32> {
            h.rec
                .msgs
                .iter()
                .filter(|(_, e, _)| e == "ok")
                .map(|&(_, _, c)| c)
                .collect()
        };
        // First query at construction: codes 9, 5, 7; text taken (response exactly 1).
        assert_eq!(codes(&h), [9, 5, 7]);
        assert_eq!(state(&h.tree, ok).text.as_deref(), Some("Continue"));
        assert_eq!(state(&h.tree, ok).query_at, Some(1100));
        h.rec.msgs.clear();
        for now in [1000, 1050, 1099] {
            h.now = now;
            h.query(ok);
        }
        assert!(codes(&h).is_empty());
        h.now = 1100;
        h.query(ok);
        assert_eq!(codes(&h), [9, 5, 7]);
        // Without `timeout` never again ...
        assert_eq!(state(&h.tree, ok).query_at, None);
        h.rec.msgs.clear();
        for now in [1200, 100_000, u64::MAX] {
            h.now = now;
            h.query(ok);
        }
        assert!(codes(&h).is_empty());
        // ... except by broadcast `0x12E` code 5: immediately, taken by the first button.
        h.now = 200_000;
        let desktop = h.tree.desktop();
        assert!(h.with(|t, _, e| t.broadcast(desktop, &Broadcast::Requery, e)));
        assert_eq!(codes(&h), [9, 5, 7]);
    }

    #[test]
    fn text_color_is_none_for_other_widgets() {
        // Panic-free for drawing: only buttons have a text color according to `0x0045EC85`.
        let mut h = Harness::new();
        let frame = h.build(TWO);
        assert_eq!(text_color(h.tree.node(frame)), None);
        let ok = h.tree.find("DLGOK").unwrap();
        assert_eq!(text_color(h.tree.node(ok)), Some(0xFFFF_FFFF));
    }

    #[test]
    fn text_is_taken_only_when_the_listener_returns_1() {
        let mut h = Harness::new();
        h.rec.answer = Box::new(|m| {
            if m.code == code::QUERY_TEXT {
                m.params.insert("text".into(), Param::Text("X".into()));
                return 0;
            }
            -1
        });
        h.build(TWO);
        let ok = h.tree.find("DLGOK").unwrap();
        assert_eq!(state(&h.tree, ok).text, None);
    }
}
