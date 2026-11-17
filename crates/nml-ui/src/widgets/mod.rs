//! Behavior of the element kinds (vtable slots, re/menu-system 2.2, 3.5, 8.3, 12.4-12.6): the dispatcher
//! ([`crate::input`]) and the tree ([`crate::tree`]) call the slots of each kind here; each kind has its own module
//! with its state. Kinds without a module of their own behave like the base widget.
//!
//! Base (`MenuWidget`): wheel (slots 0/1, `0x004527C0`) -> `false` (falls through), other mouse slots `0x0045A360` ->
//! `true` (`+0x92 == 0`, no widget sets `+0x92`), key (slot 22) and `OnMessage` (slot 19) -> `false`,
//! `OnAttach` (slot 2) empty.
//!
//! All elements "pull" their data with the same query scheme (12.5): only when the timer is due
//! (`0 < timer <= now`, constructor 1 -> immediately), clear the timer, code 9 `timeout` (0 or missing -> never
//! again), code 5 `status` replaces the bits `HIDE|DISABLE`, then `+0x4C = (flags & 0x30000000) == 0`, then
//! the codes of the kind.

pub(crate) mod blocker;
pub mod button;
pub mod combo;
pub mod edit;
pub mod frame;
pub mod label;
pub mod listbox;
pub mod radio;
pub mod slider;
#[cfg(test)]
pub(crate) mod testkit;

use nml_formats::dlg::desc::common_flags;

use crate::coords::to_screen_rect;
use crate::dialog::FONT_BUTTON;
use crate::fonts::FontRef;
use crate::msg::{ElementMsg, Param, SharedText, UiEffect, code};
use crate::text::text_height;
use crate::tree::{Broadcast, Node, NodeId, NodeKind, UiEnv, UiTree};

/// `HIDE | DISABLE`: set -> locked (`+0x4C = (flags & 0x30000000) == 0`).
pub(crate) const LOCK_BITS: u32 = common_flags::HIDE | common_flags::DISABLE;

/// Mouse slots of the vtable that the dispatcher `FUN_0045B360` calls (table in re/menu-system 2.2). The slots of
/// "button B" (events 6, 7, 12) are missing: no source produces them (3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Slot 0, event 14.
    WheelDown,
    /// Slot 1, event 13.
    WheelUp,
    /// Slot 4, event 10.
    RightDouble,
    /// Slot 6, event 11.
    LeftDouble,
    /// Slot 7: release on the left inside the widget after pressing in it; the dispatcher discards the return value
    /// (12.4).
    LeftClick,
    /// Slot 9: release on the right inside the widget after pressing in it.
    RightClick,
    /// Slot 10, event 8.
    RightDown,
    /// Slot 12, event 4.
    LeftDown,
    /// Slot 13, event 9.
    RightUp,
    /// Slot 15, event 5.
    LeftUp,
    /// Slot 16: mouse enters (return value discarded).
    Enter,
    /// Slot 17: mouse leaves (return value discarded).
    Leave,
    /// Slot 18, event 3.
    Move,
}

/// Recorded call (tests only): which widget got which slot, and which broadcasts arrived at the desktop
/// (where all broadcasts from the root that nobody takes end).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Trace {
    Mouse(NodeId, Slot),
    Key(NodeId),
    Desktop(Broadcast),
}

/// Base mouse slot (`0x0045A360` or wheel `0x004527C0`).
pub(crate) fn base_mouse(slot: Slot) -> bool {
    !matches!(slot, Slot::WheelUp | Slot::WheelDown)
}

/// Mouse slot `slot` of widget `id`; `pt` is the pointer in the grid, absolute (12.4).
pub(crate) fn mouse_slot(
    tree: &mut UiTree,
    id: NodeId,
    slot: Slot,
    pt: [i32; 2],
    env: &mut UiEnv,
) -> bool {
    #[cfg(test)]
    tree.trace.push(Trace::Mouse(id, slot));
    match tree.node(id).kind {
        // `MenuDesktop`: mouse slots `0x004527C0`, 16/17 `0x00532A90` - all `false`, transparent (2.2).
        NodeKind::Desktop => false,
        NodeKind::Button(_) => button::mouse_slot(tree, id, slot, env),
        NodeKind::ModalBlocker { .. } => blocker::mouse_slot(tree, id, slot, env),
        NodeKind::ListBox(_) => listbox::mouse_slot(tree, id, slot, pt, env),
        NodeKind::Slider(_) => slider::mouse_slot(tree, id, slot, pt, env),
        NodeKind::Radio(_) => radio::mouse_slot(tree, id, slot, env),
        NodeKind::EditField(_) => edit::mouse_slot(tree, id, slot),
        NodeKind::Label(_) => label::mouse_slot(tree, id, slot, pt, env),
        // `MenuFrame` slot 18 `0x00460590`: set pointer 7 (pointers are not handled here), return 0.
        NodeKind::Frame(_) if slot == Slot::Move => false,
        _ => base_mouse(slot),
    }
}

/// Key slot 22 (event `0xF`) of widget `id`. The desktop (`0x0045CAB0`) only aborts a running video; the EditField
/// takes every key; all others return `false`.
pub(crate) fn key_slot(
    tree: &mut UiTree,
    id: NodeId,
    held: &[u8],
    ch: Option<u16>,
    env: &mut UiEnv,
) -> bool {
    #[cfg(test)]
    tree.trace.push(Trace::Key(id));
    match tree.node(id).kind {
        NodeKind::EditField(_) => edit::key_slot(tree, id, held, ch, env),
        _ => false,
    }
}

/// `OnAttach` (slot 2) after attaching (`Widget_AddChild` `FUN_0045BA50`).
pub(crate) fn on_attach(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    match tree.node(id).kind {
        // `0x0045E700` = `JMP FUN_0045E350`: first query already at construction.
        NodeKind::Button(_) => button::query(tree, id, env),
        NodeKind::ComboPopup(_) => combo::on_attach(tree, id, env),
        NodeKind::ListBox(_) => listbox::on_attach(tree, id, env),
        NodeKind::Slider(_) => slider::on_attach(tree, id, env),
        // `0x00465A90`: only the query.
        NodeKind::Radio(_) => radio::query(tree, id, env),
        NodeKind::EditField(_) => edit::on_attach(tree, id, env),
        NodeKind::Label(_) => label::on_attach(tree, id, env),
        NodeKind::Frame(_) => frame::on_attach(tree, id, env),
        _ => {}
    }
}

/// Attaching in the **file state** for static drawing (`crate::dialog::paint_static`): only the first queries
/// (without listeners they change nothing except the lock from `HIDE`/`DISABLE`), the background set of the slider
/// and, for the credits, the setup including the first image. No focus (so no caret), no scroll bars, no music. A
/// locked button then selects its set anew (`button_disabled.bkg`): that is the static path, which
/// `DIALOG_DRAW_GOLDEN` records - at run time set 0 stays (O-91, re/menu-system 12.5).
pub(crate) fn attach_static(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    match tree.node(id).kind {
        NodeKind::Button(_) => {
            button::query(tree, id, env);
            button::select_bkg_set(tree.node_mut(id));
        }
        NodeKind::ListBox(_) => listbox::query(tree, id, env),
        NodeKind::Slider(_) => slider::on_attach(tree, id, env),
        NodeKind::Radio(_) => radio::query(tree, id, env),
        NodeKind::EditField(_) => edit::query(tree, id, env),
        NodeKind::Label(_) => label::attach_static(tree, id, env),
        NodeKind::Frame(_) => frame::query(tree, id, env),
        _ => {}
    }
}

/// `OnMessage` (slot 19) of a broadcast; `true` = accepted, the broadcast ends.
pub(crate) fn on_message(tree: &mut UiTree, id: NodeId, msg: &Broadcast, env: &mut UiEnv) -> bool {
    match tree.node(id).kind {
        NodeKind::Desktop => desktop_message(tree, msg, env),
        NodeKind::ModalBlocker { owner } => blocker::on_message(tree, owner, msg, env),
        NodeKind::Button(_) => button::on_message(tree, id, msg, env),
        NodeKind::ListBox(_) => listbox::on_message(tree, id, msg, env),
        NodeKind::Slider(_) => slider::on_message(tree, id, msg),
        NodeKind::Radio(_) => radio::on_message(tree, id, msg, env),
        NodeKind::EditField(_) => edit::on_message(tree, id, msg, env),
        NodeKind::Label(_) => label::on_message(tree, id, msg, env),
        _ => false,
    }
}

/// `MenuDesktop.OnMessage` (`0x0045D830`, jump table `0x0045D968`, each accepted):
/// - `0x162` -> music length into the response field (`FUN_0045CB70`);
/// - `0x15E`/`0x15F` -> image table ([`crate::desktop::DesktopImages::show`]/`hide`); a new entry is only created
///   if the texture loads (`FUN_0045D250`). An empty name creates nothing and loads nothing: the effect is confirmed by
///   the reference image `OPTIONS` + `VIDEOOPTIONS`; that `Renderer_GetOrLoadTexture("")` fails is a **hypothesis**.
/// - `0x160` -> `MenuMusic_Play` (`0x0045D4C0`), here the effect [`UiEffect::MenuMusic`] with the path as read;
///   `0x161` -> [`UiEffect::RestartMusic`]. The host only restarts for a different path (12.1.12).
fn desktop_message(tree: &mut UiTree, msg: &Broadcast, env: &mut UiEnv) -> bool {
    #[cfg(test)]
    tree.trace.push(Trace::Desktop(msg.clone()));
    match msg {
        Broadcast::Music { path, looping } => {
            env.fx.push(UiEffect::MenuMusic {
                path: path.clone(),
                looping: *looping,
            });
            true
        }
        Broadcast::RestartMusic => {
            env.fx.push(UiEffect::RestartMusic);
            true
        }
        Broadcast::MusicLength => {
            tree.answer_music_length();
            true
        }
        Broadcast::ShowImage { path, ms } | Broadcast::HideImage { path, ms } => {
            let known = tree.images.contains(path);
            if !known && (path.is_empty() || env.ui.assets.texture(env.ui.vfs, path).is_none()) {
                return true;
            }
            if matches!(msg, Broadcast::ShowImage { .. }) {
                tree.images.show(path, *ms, env.now);
            } else {
                tree.images.hide(path, *ms, env.now);
            }
            true
        }
        _ => false,
    }
}

/// Pre-pass of `Draw` per kind, which the drawing pass ([`crate::paint::tick_tree`]) calls for each visited widget
/// before drawing - everything that `Draw` changes in the state in the original: the query (button `FUN_0045E350` from
/// `0x0045EBED`, frame `FUN_00461140` from `FUN_00461520`, ListBox `FUN_004638F0`, slider `FUN_00464B60`,
/// RadioButton `FUN_00465880`, EditField `FUN_0045F5D0` including the caret blinking, label `FUN_00462490`; each
/// only when the timer is due), for the credits instead scrolling, image changes and the end
/// (`0x00463120`...`0x00463392`), at the desktop the state transitions of the image table (`FUN_0045CC80`).
pub fn query(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    match tree.node(id).kind {
        NodeKind::Desktop => tree.images.tick(env.now),
        NodeKind::Frame(_) => frame::query(tree, id, env),
        NodeKind::Button(_) => button::query(tree, id, env),
        NodeKind::ListBox(_) => listbox::query(tree, id, env),
        NodeKind::Slider(_) => slider::query(tree, id, env),
        NodeKind::Radio(_) => radio::query(tree, id, env),
        NodeKind::EditField(_) => edit::before_draw(tree, id, env),
        NodeKind::Label(_) => label::before_draw(tree, id, env),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Shared building blocks
// ---------------------------------------------------------------------------------------------------------------

/// Integer parameter of the response.
pub(crate) fn int_param(msg: &ElementMsg, name: &str) -> Option<i32> {
    match msg.params.get(name) {
        Some(Param::Int(v)) => Some(*v),
        _ => None,
    }
}

/// Text parameter of the response; a shared buffer is copied.
pub(crate) fn text_param(msg: &ElementMsg, name: &str) -> Option<String> {
    match msg.params.get(name) {
        Some(Param::Text(t)) => Some(t.clone()),
        Some(Param::Shared(t)) => Some(t.get()),
        _ => None,
    }
}

/// Shared text buffer of the response (pointer of the logic, [`SharedText`]).
pub(crate) fn shared_param(msg: &ElementMsg, name: &str) -> Option<SharedText> {
    match msg.params.get(name) {
        Some(Param::Shared(t)) => Some(t.clone()),
        _ => None,
    }
}

/// Whether a timer is due (`0 < t <= now`; `None` = 0 = never).
pub(crate) fn due(at: Option<u64>, now: u64) -> bool {
    matches!(at, Some(t) if t <= now)
}

/// Element message with code `code` to the listener; returns the return value and the (filled) message.
pub(crate) fn notify(
    tree: &UiTree,
    id: NodeId,
    code: u32,
    params: &[(&str, Param)],
    env: &mut UiEnv,
) -> (i32, ElementMsg) {
    let mut msg = tree.element_msg(id, code);
    for (k, v) in params {
        msg.params.insert((*k).to_string(), v.clone());
    }
    let r = env.listener.element(&mut msg, env.fx);
    (r, msg)
}

/// Head of every query (12.5): code 9 `timeout` -> next point in time `now + timeout` (0 or missing -> never
/// again), code 5 `status` -> `flags = (flags & !0x30000000) | status`. Returns the new timer and the
/// answered message for code 5 (the label reads `tga` from it). The caller sets the lock with [`lock`] at the
/// point of its kind.
pub(crate) fn query_head(
    tree: &mut UiTree,
    id: NodeId,
    env: &mut UiEnv,
) -> (Option<u64>, ElementMsg) {
    let now = env.now;
    let (_, msg) = notify(tree, id, code::QUERY_TIMEOUT, &[], env);
    let next = int_param(&msg, "timeout")
        .filter(|&t| t != 0)
        .map(|t| now.saturating_add_signed(i64::from(t)));
    let (_, msg) = notify(tree, id, code::QUERY_STATUS, &[], env);
    if let Some(status) = int_param(&msg, "status") {
        let node = tree.node_mut(id);
        node.flags = (node.flags & !LOCK_BITS) | status as u32;
    }
    (next, msg)
}

/// `+0x4C = (flags & 0x30000000) == 0`: `HIDE` and `DISABLE` lock, even without a response.
pub(crate) fn lock(node: &mut Node) {
    node.enabled = node.flags & LOCK_BITS == 0;
}

/// Index of `value` in `index_map` if both fields have the same length, otherwise `value` itself; not found -1
/// (ComboBox `FUN_0045E350`, ListBox `FUN_004638F0`, code 0).
pub(crate) fn map_selection(value: i32, items: usize, index_map: &[i32]) -> i32 {
    if items != index_map.len() {
        return value;
    }
    index_map
        .iter()
        .position(|&v| v == value)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1)
}

/// Client area in screen pixels (`FUN_00459B30`).
pub(crate) fn client_screen(tree: &UiTree, id: NodeId, env: &UiEnv) -> [i32; 4] {
    to_screen_rect(env.ui.screen, tree.client_rect(id))
}

/// Font `MenuButton` (ListBox `+0xF8`, EditField `+0xBC`, label `+0xCC`).
pub(crate) fn button_font(env: &UiEnv) -> FontRef {
    env.ui.fonts.by_name(FONT_BUTTON)
}

/// Line height of a sample text in the font `MenuButton` (S).
pub(crate) fn line_height(env: &UiEnv, sample: &str) -> i32 {
    text_height(sample, button_font(env), env.ui.fonts)
}

/// Integer division like `IDIV` (toward 0); the denominator 0 (an exception in the original) yields 0.
pub(crate) fn idiv(a: i32, b: i32) -> i32 {
    a.checked_div(b).unwrap_or(0)
}

/// `FUN_0045A050(n)`: switch the background set, only if it exists.
pub(crate) fn set_bkg_set(node: &mut Node, n: usize) {
    if n < node.bkg_sets.len() {
        node.bkg_set = n;
    }
}
