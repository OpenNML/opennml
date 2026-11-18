//! `ModalBlocker` (vtable `0x0061AA18`, re/menu-system 12.6): covers the whole grid before modal dialogs and open
//! ComboBox lists, swallows every mouse event except the wheel and passes every broadcast on to its owner. It does
//! not lock the keyboard.

use super::Slot;
use crate::tree::{Broadcast, NodeId, NodeKind, UiEnv, UiTree};

/// Mouse slots: wheel `0x004527C0` -> `false` (falls through, O-93); left click `FUN_004656D0` -> broadcast `0x12D`
/// (cancel list) to the owner, `true`; all others base `0x0045A360` -> `true`.
pub(crate) fn mouse_slot(tree: &mut UiTree, id: NodeId, slot: Slot, env: &mut UiEnv) -> bool {
    match slot {
        Slot::WheelUp | Slot::WheelDown => false,
        Slot::LeftClick => {
            let NodeKind::ModalBlocker { owner } = tree.node(id).kind else {
                unreachable!("blocker slot on another widget");
            };
            tree.broadcast(owner, &Broadcast::ListCancel, env);
            true
        }
        _ => true,
    }
}

/// `OnMessage` `0x00465700` = `JMP Widget_Broadcast(+0x94, msg)`: a modal frame is thus visited twice.
pub(crate) fn on_message(
    tree: &mut UiTree,
    owner: NodeId,
    msg: &Broadcast,
    env: &mut UiEnv,
) -> bool {
    tree.broadcast(owner, msg, env)
}
