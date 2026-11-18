//! `MenuRadioButton` (vtable `0x0061AA80`, re/menu-system 8.3, 12.6; O-17): a click toggles and reports the new
//! state; mutual exclusion is done by the dialog logic.

use opennml_formats::dlg::RadioDesc;
use opennml_formats::dlg::desc::radio_flags;

use super::{Slot, base_mouse, due, int_param, lock, notify, query_head};
use crate::msg::{Param, code};
use crate::tree::{Broadcast, NodeId, NodeKind, UiEnv, UiTree};

/// State of a `MenuRadioButton` (fields from `+0x94`).
#[derive(Debug, Clone)]
pub struct RadioState {
    pub desc: RadioDesc,
    /// Bit `0x4` (`RADIOBUTTON_CHECKED`) in the flag word `+0xA4`.
    pub checked: bool,
    /// `+0xC8`: next query time; `None` = never again.
    pub query_at: Option<u64>,
}

impl RadioState {
    /// Constructor `FUN_00465D90`: check mark from the file, query due.
    pub fn new(desc: RadioDesc) -> RadioState {
        RadioState {
            checked: desc.flags & radio_flags::CHECKED != 0,
            desc,
            query_at: Some(0),
        }
    }
}

fn state(tree: &UiTree, id: NodeId) -> &RadioState {
    match &tree.node(id).kind {
        NodeKind::Radio(s) => s,
        _ => panic!("widget {id:?} is not a RadioButton"),
    }
}

fn state_mut(tree: &mut UiTree, id: NodeId) -> &mut RadioState {
    match &mut tree.node_mut(id).kind {
        NodeKind::Radio(s) => s,
        _ => panic!("widget {id:?} is not a RadioButton"),
    }
}

/// Mouse slots: click (7 `0x004657C0`) toggles `CHECKED` and reports code 1 with 1/0 - no sound, no
/// group logic, no requery (O-17); the others like the base.
pub(crate) fn mouse_slot(tree: &mut UiTree, id: NodeId, slot: Slot, env: &mut UiEnv) -> bool {
    if slot != Slot::LeftClick {
        return base_mouse(slot);
    }
    let s = state_mut(tree, id);
    s.checked = !s.checked;
    let v = i32::from(s.checked);
    notify(
        tree,
        id,
        code::VALUE_CHANGED,
        &[("int", Param::Int(v))],
        env,
    );
    true
}

/// Query `FUN_00465880` (from `OnAttach` `0x00465A90` and `Draw`): codes 9, 5, 0 (`int != 0` -> `CHECKED`, `int = 0`
/// -> cleared, if `int` is missing the check mark stays); the lock from the flags only afterwards.
pub(crate) fn query(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if !due(state(tree, id).query_at, env.now) {
        return;
    }
    state_mut(tree, id).query_at = None;
    let (next, _) = query_head(tree, id, env);
    state_mut(tree, id).query_at = next;
    let (_, msg) = notify(tree, id, code::QUERY_VALUE, &[], env);
    if let Some(v) = int_param(&msg, "int") {
        state_mut(tree, id).checked = v != 0;
    }
    lock(tree.node_mut(id));
}

/// `OnMessage` `0x00465A60`: only `0x12E` code 5 -> query due and immediate, accepted.
pub(crate) fn on_message(tree: &mut UiTree, id: NodeId, msg: &Broadcast, env: &mut UiEnv) -> bool {
    if *msg != Broadcast::Requery {
        return false;
    }
    state_mut(tree, id).query_at = Some(0);
    query(tree, id, env);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::testkit::{Harness, center};

    // QUIRK O-16, O-17: kept
    #[test]
    fn radio_toggles_and_reports_without_group_logic() {
        // O-17: two RadioButtons, A checked. A click on B checks B and leaves A alone; no sound.
        let mut h = Harness::new();
        h.rec.allow = true;
        h.build(
            "FRAME F FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
             RADIOBUTTON A FLAGS RADIOBUTTON_CHECKED /FLAGS EXTENT 10 10 200 35 /RADIOBUTTON\n\
             RADIOBUTTON B EXTENT 10 40 200 65 /RADIOBUTTON\n/FRAME\n",
        );
        let a = h.tree.find("FA").unwrap();
        let b = h.tree.find("FB").unwrap();
        let checked = |h: &Harness, id| match &h.tree.node(id).kind {
            NodeKind::Radio(s) => s.checked,
            _ => unreachable!(),
        };
        assert!(checked(&h, a) && !checked(&h, b));
        h.forget();
        let at = center(&h.tree, b);
        h.click(at);
        assert!(checked(&h, a) && checked(&h, b));
        h.click(at);
        assert!(!checked(&h, b));
        assert_eq!(h.codes("b"), [code::VALUE_CHANGED, code::VALUE_CHANGED]);
        assert_eq!(h.sent_ints("b", code::VALUE_CHANGED, "int"), [1, 0]);
        assert!(h.codes("a").is_empty());
        assert!(h.fx.is_empty());
        // The logic checks via requery (0x12E code 5, code 0 `int`); without `int` the check mark stays.
        h.rec.answer = Box::new(|m| {
            if m.element == "b" && m.code == code::QUERY_VALUE {
                m.params.insert("int".into(), Param::Int(7));
                return 0;
            }
            -1
        });
        assert!(h.with(|t, _, e| on_message(t, b, &Broadcast::Requery, e)));
        assert!(checked(&h, b));
        h.rec.answer = Box::new(|_| -1);
        assert!(h.with(|t, _, e| on_message(t, b, &Broadcast::Requery, e)));
        assert!(checked(&h, b));
    }
}
