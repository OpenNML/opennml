//! `MenuEditField` (vtable `0x0061A628`, re/menu-system 4, 12.6; O-20, O-94): text field with caret and key-binding
//! field (`KEYNAME`). Characters arrive as a cp1252 unit from the key object (hypothesis 12.6, like `WM_CHAR`); the
//! text is UTF-16 as in the original (`wchar_t`).

use opennml_formats::dlg::EditDesc;
use opennml_formats::dlg::desc::{common_flags, editfield_flags};

use super::{
    Slot, base_mouse, button_font, client_screen, due, int_param, lock, notify, query_head,
    shared_param, text_param,
};
use crate::keys::{chord_display, chord_matches, chord_name, parse_chord};
use crate::msg::{Param, SharedText, code};
use crate::text::{atlas_codes, measure};
use crate::tree::{Broadcast, NodeId, NodeKind, UiEnv, UiTree};

/// Largest length of the buffer (`+0xCC`, constructor 200; responses above 199 become 200).
const MAX_SIZE: i32 = 200;
/// Blinking of the caret after an arrow key: visible until now + 500 ms (`0x00460295`).
const BLINK_MS: u64 = 500;
/// Keys that the key-binding field ignores when pressed alone (`0x0061A6E0`...`0x0061A6F4`).
const MODIFIER_NAMES: [&str; 5] = ["CONTROL", "SHIFT", "ALT", "LWIN", "APPS"];

/// State of a `MenuEditField` (fields from `+0x94`).
#[derive(Debug, Clone)]
pub struct EditState {
    pub desc: EditDesc,
    /// `+0xC0`: own buffer (UTF-16, without terminator); applies as long as [`Self::shared`] is absent.
    pub text: Vec<u16>,
    /// `+0xC0` as pointer: with `inputbuffer` the shared buffer of the logic. The field then reads and writes only it;
    /// the logic sees every change, the field every change of the logic.
    pub shared: Option<SharedText>,
    /// `+0xC4`: caret (index in the text, limited to `[0, length]` after every key).
    pub cursor: i32,
    /// `+0xCC`: buffer size; at most `size - 1` characters.
    pub size: i32,
    /// `+0xC8`: caret visible until this time (ms of the program clock).
    pub blink_until: u64,
    /// The caret is shown in this frame (set by the pre-pass [`before_draw`], `crate::paint::paint_ui` reads it): the
    /// original decides and toggles the blinking in the same `Draw`.
    pub caret: bool,
    /// `+0xD0`: the logic supplies its own buffer (`inputbuffer`).
    pub input_buffer: bool,
    /// `+0xB4`: text color (`TEXTCOLOR`, in the key-binding field from the response `color`).
    pub text_color: u32,
    /// `+0xD4`: next query time; `None` = never again.
    pub query_at: Option<u64>,
}

impl EditState {
    /// Constructor `FUN_0045FD20`: empty buffer, `size = 200`, query due.
    pub fn new(desc: EditDesc) -> EditState {
        EditState {
            text_color: desc.text_color,
            desc,
            text: Vec::new(),
            shared: None,
            cursor: 0,
            size: MAX_SIZE,
            blink_until: 0,
            caret: false,
            input_buffer: false,
            query_at: Some(0),
        }
    }

    /// The text at the present time: shared buffer or own buffer.
    pub fn current(&self) -> Vec<u16> {
        match &self.shared {
            Some(shared) => shared.units(),
            None => self.text.clone(),
        }
    }

    /// Replace the text - in the shared buffer if there is one.
    fn set_current(&mut self, units: Vec<u16>) {
        match &self.shared {
            Some(shared) => shared.set_units(units),
            None => self.text = units,
        }
    }

    /// Length of the text (`wcslen`).
    fn len(&self) -> i32 {
        i32::try_from(self.current().len()).unwrap_or(i32::MAX)
    }
}

fn state(tree: &UiTree, id: NodeId) -> &EditState {
    match &tree.node(id).kind {
        NodeKind::EditField(s) => s,
        _ => panic!("widget {id:?} is not an EditField"),
    }
}

fn state_mut(tree: &mut UiTree, id: NodeId) -> &mut EditState {
    match &mut tree.node_mut(id).kind {
        NodeKind::EditField(s) => s,
        _ => panic!("widget {id:?} is not an EditField"),
    }
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().take_while(|&u| u != 0).collect()
}

fn text_of(s: &EditState) -> Param {
    Param::Text(String::from_utf16_lossy(&s.current()))
}

/// `wcsncpy(buffer, text, size - 1)`: at most `size - 1` characters. Deviation: with a longer text the original
/// writes no terminator (the rest of the old buffer stays behind it); the reimplementation truncates.
fn copy_limited(text: &str, size: i32) -> Vec<u16> {
    let max = usize::try_from(size.saturating_sub(1)).unwrap_or(0);
    utf16(text).into_iter().take(max).collect()
}

/// Query `FUN_0045F5D0` (from `OnAttach` and `Draw`): codes 9, 5 (the lock then follows the flags, so `HIDE` and
/// `DISABLE` lock), code 7: `inputbuffer`, `size` (above 199 -> 200), text = response `text` or `TEXT` of the
/// `.dlg`, shortened to `size - 1` (with `inputbuffer` the reimplementation takes the text of the logic whole); caret
/// to the end.
pub(crate) fn query(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    if !due(state(tree, id).query_at, env.now) {
        return;
    }
    state_mut(tree, id).query_at = None;
    let (next, _) = query_head(tree, id, env);
    state_mut(tree, id).query_at = next;
    lock(tree.node_mut(id));

    let (_, msg) = notify(tree, id, code::QUERY_TEXT, &[], env);
    let file_text = state(tree, id)
        .desc
        .text
        .map(|t| env.ui.texts.display(t))
        .unwrap_or_default();
    let s = state_mut(tree, id);
    if let Some(b) = int_param(&msg, "inputbuffer") {
        s.input_buffer = b as u8 != 0;
    }
    if let Some(size) = int_param(&msg, "size") {
        s.size = size;
    }
    if s.size > MAX_SIZE - 1 {
        s.size = MAX_SIZE;
    }
    let answer = text_param(&msg, "text");
    // With `inputbuffer` the original takes over the pointer (`+0xC0 = *text`, without shortening): a shared buffer
    // stays shared, an ordinary text is copied whole.
    s.shared = None;
    match (s.input_buffer, shared_param(&msg, "text"), answer) {
        (true, Some(shared), _) => s.shared = Some(shared),
        (true, None, Some(t)) => s.text = utf16(&t),
        (false, _, Some(t)) => s.text = copy_limited(&t, s.size),
        (_, _, None) => s.text = copy_limited(&file_text, s.size),
    }
    s.cursor = s.len();
}

/// Pre-pass of `Draw` `0x0045F9C0` (disassembly `0x0045F9C0`...`0x0045FB68`): query; with `HIDE` end. With focus
/// (`FUN_00459660`) and active: if the caret is on (`+0xC8 > now`), it is drawn; otherwise, if `+0xC8 + 500 < now`,
/// `+0xC8 = now + 500` - in this frame without a caret. So it blinks 500 ms on and at least 500 ms off (8.4.7).
pub(crate) fn before_draw(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    query(tree, id, env);
    let now = env.now;
    let node = tree.node(id);
    let shows = node.flags & common_flags::HIDE == 0 && node.focused && node.enabled;
    let s = state_mut(tree, id);
    s.caret = false;
    if !shows {
        return;
    }
    if s.blink_until > now {
        s.caret = true;
    } else if s.blink_until.saturating_add(BLINK_MS) < now {
        s.blink_until = now.saturating_add(BLINK_MS);
    }
}

/// `OnAttach` `FUN_0045F9B0`: query, then focus (`FUN_0045B240`) - the most recently attached EditField has it.
pub(crate) fn on_attach(tree: &mut UiTree, id: NodeId, env: &mut UiEnv) {
    query(tree, id, env);
    tree.set_focus(id);
}

/// Mouse slots: click (7 `0x0045F440`) only sets the focus (the caret does not follow the mouse); the others like the
/// base.
pub(crate) fn mouse_slot(tree: &mut UiTree, id: NodeId, slot: Slot) -> bool {
    if slot == Slot::LeftClick {
        tree.set_focus(id);
        return true;
    }
    base_mouse(slot)
}

/// Insert `FUN_0045F4F0(text, pos, ch)`: only with `size - length > 1` (at most `size - 1` characters); `pos` limited
/// to `[0, length]`; the new text is measured and only accepted if it fits into the client width (S).
fn insert(tree: &mut UiTree, id: NodeId, ch: u16, env: &UiEnv) {
    let width = {
        let c = client_screen(tree, id, env);
        c[2].wrapping_sub(c[0])
    };
    let s = state(tree, id);
    let len = s.len();
    if s.size.wrapping_sub(len) <= 1 {
        return;
    }
    let pos = usize::try_from(s.cursor.clamp(0, len)).unwrap_or(0);
    let mut candidate = s.current();
    candidate.insert(pos, ch);
    let measured = measure(
        &atlas_codes(&String::from_utf16_lossy(&candidate)),
        button_font(env),
        env.ui.fonts,
    )[0];
    if measured <= width {
        state_mut(tree, id).set_current(candidate);
    }
}

/// Delete `FUN_0045F3B0(text, pos)`: the character at `pos`, if `0 <= pos < length`.
fn delete_at(s: &mut EditState, pos: i32) {
    if pos >= 0 && pos < s.len() {
        let mut text = s.current();
        text.remove(pos as usize);
        s.set_current(text);
    }
}

/// Key slot 22 (`0x0045FEA0`): locked -> `false`; otherwise **always** `true` (a focused field swallows every key that
/// was not taken as an ESC/ENTER broadcast).
///
/// - Key-binding field (`KEYNAME`): `CONTROL`, `SHIFT`, `ALT`, `LWIN`, `APPS` alone -> nothing; otherwise code `0xB`
///   with `keydef` (chord name); response != -1 -> text = display name (text `50000 + VK` per key, separated by a
///   space, `FUN_0049BDF0`), caret to the end, response `color` -> text color.
/// - Text field, key names via [`chord_matches`] (Shift+arrow is not an arrow): `DELETE` (delete at the caret) and
///   `BACK` (caret -1, delete there) each report code `0xC`; `left`/`right` (caret -/+1, blinking restarts); `RETURN`
///   -> code `0xB` with `key = 13`; otherwise a character >= `0x20` -> code `0xB` with `key`, insert only on response
///   -1 and advance the caret **even when** length or width prevent the insertion; afterwards **always** code
///   `0xC` (O-94). Control characters < `0x20` (Tab ...) stay without a message. Finally caret to `[0, length]`.
pub(crate) fn key_slot(
    tree: &mut UiTree,
    id: NodeId,
    held: &[u8],
    ch: Option<u16>,
    env: &mut UiEnv,
) -> bool {
    if !tree.node(id).enabled {
        return false;
    }
    if tree.node(id).flags & editfield_flags::KEYNAME != 0 {
        key_field(tree, id, held, env);
        return true;
    }
    let is = |name: &str| chord_matches(held, &parse_chord(name));
    if is("DELETE") {
        let s = state_mut(tree, id);
        let pos = s.cursor;
        delete_at(s, pos);
        let text = text_of(state(tree, id));
        notify(tree, id, code::EDIT_CHANGED, &[("text", text)], env);
    } else if is("BACK") {
        let s = state_mut(tree, id);
        s.cursor = s.cursor.wrapping_sub(1);
        let pos = s.cursor;
        delete_at(s, pos);
        let text = text_of(state(tree, id));
        notify(tree, id, code::EDIT_CHANGED, &[("text", text)], env);
    } else if is("left") || is("right") {
        let step = if is("left") { -1 } else { 1 };
        let now = env.now;
        let s = state_mut(tree, id);
        s.cursor = s.cursor.wrapping_add(step);
        s.blink_until = now.saturating_add(BLINK_MS);
    } else if is("RETURN") {
        notify(tree, id, code::EDIT_KEY, &[("key", Param::Int(13))], env);
    } else {
        let ch = ch.unwrap_or(0);
        if ch >= 0x20 {
            let (r, _) = notify(
                tree,
                id,
                code::EDIT_KEY,
                &[("key", Param::Int(i32::from(ch)))],
                env,
            );
            if r == -1 {
                insert(tree, id, ch, env);
                let s = state_mut(tree, id);
                s.cursor = s.cursor.wrapping_add(1);
            }
            let text = text_of(state(tree, id));
            notify(tree, id, code::EDIT_CHANGED, &[("text", text)], env);
        }
    }
    let s = state_mut(tree, id);
    s.cursor = s.cursor.clamp(0, s.len());
    true
}

/// Key-binding field (`+0x9C & 1`, `0x0045FECC`...`0x004600D4`). The order of the keys in the display name follows
/// the chord name (hypothesis: the key object holds them in the order of the table of held keys).
fn key_field(tree: &mut UiTree, id: NodeId, held: &[u8], env: &mut UiEnv) {
    let name = chord_name(held);
    if MODIFIER_NAMES.contains(&name.as_str()) {
        return;
    }
    let (r, msg) = notify(
        tree,
        id,
        code::EDIT_KEY,
        &[("keydef", Param::Text(name.clone()))],
        env,
    );
    if r == -1 {
        return;
    }
    let display = chord_display(&name, env.ui.texts);
    let s = state_mut(tree, id);
    s.set_current(utf16(&display));
    s.cursor = s.len();
    if let Some(color) = int_param(&msg, "color") {
        s.text_color = color as u32;
    }
}

/// `OnMessage` `0x0045FB70`: `0x12E` code 5 -> query due and immediate, taken; `0x17E` (ESC broadcast,
/// `FUN_0045F900`) -> code `0xB` with `key = 27`, taken on response != -1 - this applies to **every** EditField in
/// the tree, without a focus or visibility check (O-94).
pub(crate) fn on_message(tree: &mut UiTree, id: NodeId, msg: &Broadcast, env: &mut UiEnv) -> bool {
    match msg {
        Broadcast::Requery => {
            state_mut(tree, id).query_at = Some(0);
            query(tree, id, env);
            true
        }
        Broadcast::Escape => {
            let (r, _) = notify(tree, id, code::EDIT_KEY, &[("key", Param::Int(27))], env);
            r != -1
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::testkit::{Harness, center};

    /// EditField (100, 100, 300, 160); `editfield.bkg` with `CLIENTAREA 16` -> client 168 pixels wide, so 16
    /// characters of 10 pixels.
    const DLG: &str = "FRAME F FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
         EDITFIELD E EXTENT 100 100 300 160 /EDITFIELD\n/FRAME\n";

    const VK_A: u8 = 0x41;
    const VK_TAB: u8 = 0x09;
    const VK_LEFT: u8 = 0x25;
    const VK_SHIFT: u8 = 0x10;
    /// `VK_OEM_7` (a-umlaut on the German keyboard).
    const VK_OEM_7: u8 = 0xDE;

    fn edit<'h>(h: &'h Harness, key: &str) -> (NodeId, &'h EditState) {
        let id = h.tree.find(key).unwrap();
        match &h.tree.node(id).kind {
            NodeKind::EditField(s) => (id, s),
            _ => unreachable!(),
        }
    }

    fn text(h: &Harness) -> String {
        String::from_utf16_lossy(&edit(h, "FE").1.text)
    }

    fn type_str(h: &mut Harness, s: &str) {
        for c in s.encode_utf16() {
            h.key(&[VK_A], Some(c));
        }
    }

    /// Logic that answers `size` (and optionally `text`) to code 7.
    fn sized(h: &mut Harness, size: i32, text: Option<&'static str>) {
        h.rec.answer = Box::new(move |m| {
            if m.code == code::QUERY_TEXT {
                m.params.insert("size".into(), Param::Int(size));
                if let Some(t) = text {
                    m.params.insert("text".into(), Param::Text(t.into()));
                }
                return 0;
            }
            -1
        });
    }

    // QUIRK O-20: kept
    #[test]
    fn editfield_takes_cp1252_and_drops_control_chars() {
        // The character 0xE4 (a-umlaut) is inserted, Tab (0x09) is not, and without a message.
        let mut h = Harness::new();
        h.build(DLG);
        h.forget();
        assert!(h.key(&[VK_A], Some(u16::from(b'a'))));
        assert!(h.key(&[VK_OEM_7], Some(0xE4)));
        assert_eq!(text(&h), "aä");
        assert_eq!(edit(&h, "FE").1.cursor, 2);
        assert_eq!(
            h.codes("e"),
            [
                code::EDIT_KEY,
                code::EDIT_CHANGED,
                code::EDIT_KEY,
                code::EDIT_CHANGED
            ]
        );
        assert_eq!(h.sent_ints("e", code::EDIT_KEY, "key"), [0x61, 0xE4]);
        h.forget();
        assert!(h.key(&[VK_TAB], Some(0x09)), "the field swallows every key");
        assert!(h.key(&[VK_A], None));
        assert_eq!(text(&h), "aä");
        assert!(h.rec.msgs.is_empty());
        // Response != -1 discards the character, code 0xC still comes.
        h.rec.answer = Box::new(|m| if m.code == code::EDIT_KEY { 0 } else { -1 });
        h.key(&[VK_A], Some(u16::from(b'b')));
        assert_eq!(text(&h), "aä");
        assert_eq!(h.codes("e"), [code::EDIT_KEY, code::EDIT_CHANGED]);
    }

    // QUIRK O-94: kept
    #[test]
    fn editfield_cursor_advances_even_when_rejected() {
        // O-94: `size` 4 -> at most 3 characters. With the caret in the middle of the text it still advances on the
        // rejected character; code 0xC always comes.
        let mut h = Harness::new();
        sized(&mut h, 4, None);
        h.build(DLG);
        type_str(&mut h, "abc");
        assert_eq!(text(&h), "abc");
        h.key(&[VK_LEFT], None);
        h.key(&[VK_LEFT], None);
        assert_eq!(edit(&h, "FE").1.cursor, 1);
        h.forget();
        type_str(&mut h, "x");
        assert_eq!(text(&h), "abc");
        assert_eq!(edit(&h, "FE").1.cursor, 2);
        assert_eq!(h.codes("e"), [code::EDIT_KEY, code::EDIT_CHANGED]);
        // Shift+Left is not Left (subset comparison), but a character without a value.
        h.key(&[VK_SHIFT, VK_LEFT], None);
        assert_eq!(edit(&h, "FE").1.cursor, 2);
        // At the end the caret is limited to the length.
        h.key(&[0x27], None);
        h.key(&[0x27], None);
        assert_eq!(edit(&h, "FE").1.cursor, 3);
        // Backspace deletes before the caret and reports 0xC.
        h.forget();
        h.key(&[0x08], None);
        assert_eq!(text(&h), "ab");
        assert_eq!(h.codes("e"), [code::EDIT_CHANGED]);
    }

    // QUIRK O-20: kept
    #[test]
    fn editfield_length_is_size_minus_one_and_width_bound() {
        // O-20 (corrected): length `size - 1`, also for the text of the logic ...
        let mut h = Harness::new();
        sized(&mut h, 4, Some("abcdef"));
        h.build(DLG);
        assert_eq!(text(&h), "abc");
        assert_eq!(edit(&h, "FE").1.cursor, 3);
        type_str(&mut h, "d");
        assert_eq!(text(&h), "abc");
        // ... `size` above 199 becomes 200 ...
        let mut h = Harness::new();
        sized(&mut h, 5000, None);
        h.build(DLG);
        assert_eq!(edit(&h, "FE").1.size, 200);
        // ... and the width: 16 characters of 10 pixels fit into 168, the 17th does not.
        type_str(&mut h, "abcdefghijklmnopq");
        assert_eq!(text(&h), "abcdefghijklmnop");
    }

    // QUIRK O-94: kept
    #[test]
    fn escape_goes_to_every_editfield() {
        // O-94: the ESC broadcast reaches every EditField in the tree (without a focus check) with code 0xB, key 27.
        let mut h = Harness::new();
        h.build("FRAME A EXTENT 0 0 400 300 EDITFIELD E1 EXTENT 10 10 200 60 /EDITFIELD /FRAME\n");
        h.build(
            "FRAME B EXTENT 500 0 900 300 EDITFIELD E2 EXTENT 10 10 200 60 /EDITFIELD /FRAME\n",
        );
        h.forget();
        let desktop = h.tree.desktop();
        assert!(!h.with(|t, _, e| t.broadcast(desktop, &Broadcast::Escape, e)));
        assert_eq!(h.sent_ints("e1", code::EDIT_KEY, "key"), [27]);
        assert_eq!(h.sent_ints("e2", code::EDIT_KEY, "key"), [27]);
        // Via the keyboard the focused field then takes the key (control character: without effect).
        h.forget();
        assert!(h.key(&[0x1B], Some(0x1B)));
        assert_eq!(h.codes("e1"), [code::EDIT_KEY]);
        assert_eq!(h.codes("e2"), [code::EDIT_KEY]);
        // If the logic answers for E2, E2 takes ESC; E1 came first.
        h.forget();
        h.rec.answer = Box::new(|m| if m.element == "e2" { 0 } else { -1 });
        assert!(h.key(&[0x1B], Some(0x1B)));
        assert_eq!(h.codes("e1"), [code::EDIT_KEY]);
        assert_eq!(h.codes("e2"), [code::EDIT_KEY]);
    }

    // QUIRK O-94: kept
    #[test]
    fn last_attached_editfield_has_focus() {
        let mut h = Harness::new();
        h.build(
            "FRAME F FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
             EDITFIELD E1 EXTENT 100 100 300 160 /EDITFIELD\n\
             EDITFIELD E2 EXTENT 100 200 300 260 /EDITFIELD\n/FRAME\n",
        );
        let (e1, _) = edit(&h, "FE1");
        let (e2, _) = edit(&h, "FE2");
        assert!(h.tree.node(e2).focused && !h.tree.node(e1).focused);
        h.key(&[VK_A], Some(u16::from(b'z')));
        assert!(edit(&h, "FE1").1.text.is_empty());
        assert_eq!(edit(&h, "FE2").1.text, [u16::from(b'z')]);
        // A click sets only the focus (the caret does not follow the mouse).
        let at = center(&h.tree, e1);
        h.click(at);
        assert!(h.tree.node(e1).focused && !h.tree.node(e2).focused);
        h.key(&[VK_A], Some(u16::from(b'y')));
        assert_eq!(edit(&h, "FE1").1.text, [u16::from(b'y')]);
        // A field locked from the file is locked from the constructor on and takes no key.
        let mut h = Harness::new();
        h.build(
            "FRAME F FLAGS MENUFRAME_NOFRAME /FLAGS EXTENT 0 0 1280 1024\n\
             EDITFIELD E FLAGS DISABLE /FLAGS EXTENT 100 100 300 160 /EDITFIELD\n/FRAME\n",
        );
        let (e, _) = edit(&h, "FE");
        assert!(!h.tree.node(e).enabled);
        assert!(!h.key(&[VK_A], Some(u16::from(b'a'))));
        assert!(text(&h).is_empty());
    }
}
