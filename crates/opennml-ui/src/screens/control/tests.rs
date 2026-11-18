//! Tests of the key layout `CONTROLOPTIONS` (re/menu-flow, `CONTROLOPTIONS`). The handlers run individually via
//! [`dispatch`] with synthetic messages and in operation via `MenuApp` with the corpus dialog. Corpus cases need
//! `NML_GAME_DIR` and otherwise skip themselves silently.

use opennml_formats::engine_ini::EngineConfig;

use super::*;
use crate::app::tests::{Corpus, Driver, World, corpus, open};
use crate::app::{MenuApp, MenuOptions};
use crate::assets::Texts;
use crate::dialog::tests::fixture;
use crate::input::UiInput;
use crate::screens::{EnvParts, dispatch};

const VK_CONTROL: u8 = 0x11;
const VK_H: u8 = 0x48;

/// The logic alone over a world (texts), with list and key layout ([`EnvParts`]).
struct Logic<W: World> {
    world: W,
    parts: EnvParts,
}

impl<W: World> std::ops::Deref for Logic<W> {
    type Target = EnvParts;
    fn deref(&self) -> &EnvParts {
        &self.parts
    }
}

impl<W: World> std::ops::DerefMut for Logic<W> {
    fn deref_mut(&mut self) -> &mut EnvParts {
        &mut self.parts
    }
}

impl<W: World> Logic<W> {
    fn new(world: W, key_list: KeyList, key_map: &[(i32, &str)]) -> Logic<W> {
        let config = EngineConfig {
            key_map: key_map.iter().map(|&(id, k)| (id, k.to_string())).collect(),
            ..EngineConfig::default()
        };
        Logic {
            world,
            parts: EnvParts {
                config,
                key_list,
                ..EnvParts::default()
            },
        }
    }

    fn texts(&self) -> &Texts {
        self.world.texts()
    }

    fn deliver(&mut self, mut msg: ElementMsg) -> (i32, ElementMsg) {
        let r = self.parts.deliver(self.world.texts(), &mut msg);
        (r, msg)
    }

    fn send(&mut self, element: &str, code: u32) -> (i32, ElementMsg) {
        self.deliver(ElementMsg::new("controloptions", element, code))
    }

    /// Code 4 of the list with the index `index`.
    fn select(&mut self, index: i32) -> i32 {
        let mut msg = ElementMsg::new("controloptions", "keylist", code::ITEM_SELECTED);
        msg.params.insert("int".into(), Param::Int(index));
        self.deliver(msg).0
    }

    /// Code `0xB` of the layout field with `keydef`.
    fn key(&mut self, keydef: &str) -> i32 {
        let mut msg = ElementMsg::new("controloptions", "key", code::EDIT_KEY);
        msg.params
            .insert("keydef".into(), Param::Text(keydef.into()));
        self.deliver(msg).0
    }

    /// Text of the response to code 7.
    fn text(&mut self, element: &str) -> (i32, Option<String>) {
        let (r, msg) = self.send(element, code::QUERY_TEXT);
        let text = match msg.params.get("text") {
            Some(Param::Text(t)) => Some(t.clone()),
            Some(Param::Shared(t)) => Some(t.get()),
            _ => None,
        };
        (r, text)
    }

    fn key_map(&self) -> Vec<(i32, &str)> {
        self.config
            .key_map
            .iter()
            .map(|(id, k)| (*id, k.as_str()))
            .collect()
    }
}

/// List of fixed entries (without a text table).
fn list(entries: &[(i32, &str)]) -> KeyList {
    KeyList {
        entries: entries.iter().map(|&(id, t)| (id, t.to_string())).collect(),
        ..KeyList::default()
    }
}

/// Defaults and settings as at the start of the host (`FUN_00494FD0`: `defaultkeys.ini` as the basis).
fn corpus_logic() -> Option<Logic<Corpus>> {
    let c = corpus()?;
    let defaults = load_default_keys(c.vfs());
    let key_list = KeyList::build(&defaults, c.texts());
    let mut logic = Logic::new(c, key_list, &[]);
    logic.config.key_map = defaults;
    logic.key_list.keys_ini = load_keys_ini(logic.world.vfs());
    Some(logic)
}

/// Index of the action `id` in the list.
fn index_of(list: &KeyList, id: i32) -> i32 {
    let i = list.entries.iter().position(|(known, _)| *known == id);
    i32::try_from(i.unwrap_or_else(|| panic!("action {id} missing in the list"))).unwrap()
}

// QUIRK O-102: kept
#[test]
fn keylist_has_headings_and_keyid_entries() {
    let Some(mut l) = corpus_logic() else {
        return;
    };
    let defaults = load_default_keys(l.world.vfs());
    assert_eq!(defaults.len(), 460, "entries in defaultkeys.ini");
    let entries = &l.key_list.entries;
    // File order, one per entry.
    let ids: Vec<i32> = entries.iter().map(|(id, _)| *id).collect();
    let file_ids: Vec<i32> = defaults.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, file_ids);
    // First entry: heading = text 40000 unchanged (with tags, without placeholder).
    let texts = l.texts();
    let heading = texts.get(40_000).expect("text 40000");
    assert_eq!(entries[0], (0, heading.to_string()));
    assert!(heading.contains('['), "heading with tags: {heading:?}");
    for (id, text) in entries.iter().filter(|(id, _)| *id < 10) {
        assert_eq!(Some(text.as_str()), texts.get(40_000 + id), "heading {id}");
    }
    // From 101: text 40000 + ID (otherwise ID, otherwise "error") and the placeholder.
    for (id, text) in entries.iter().filter(|(id, _)| *id >= 101) {
        let name = texts
            .get(40_000 + id)
            .or_else(|| texts.get(*id))
            .unwrap_or("error");
        assert_eq!(*text, format!("{name} [KEYID {id}]"));
    }
    let n = entries
        .iter()
        .find(|(id, _)| *id == 573)
        .expect("573 = ALT N");
    assert!(n.1.ends_with(" [KEYID 573]"), "{n:?}");
    // Code 2 returns exactly these texts.
    let (r, msg) = l.send("keylist", code::QUERY_ITEMS);
    assert_eq!(r, 1);
    let want: Vec<String> = l.key_list.entries.iter().map(|(_, t)| t.clone()).collect();
    assert_eq!(msg.params.get("array"), Some(&Param::Array(want)));
}

#[test]
fn headings_are_not_selectable() {
    // `FUN_004BD780`: only `0 <= index < count` and ID >= 10; otherwise everything stays, the response is 1 anyway.
    let mut l = Logic::new(
        fixture(),
        list(&[
            (0, "Head"),
            (9, "Head 9"),
            (10, "Ten"),
            (573, "N [KEYID 573]"),
        ]),
        &[(0, ""), (9, ""), (10, "B"), (573, "ALT N")],
    );
    for index in [0, 1, -1, 4, 99] {
        assert_eq!(l.select(index), 1, "index {index}");
        assert_eq!(l.state.key_action, None, "index {index}");
        assert_eq!(l.state.key_text.get(), "");
        assert_eq!(l.state.key_description.get(), "");
    }
    // ID 10 is the first selectable one (also without a placeholder, ID < 101).
    assert_eq!(l.select(2), 1);
    assert_eq!(l.state.key_action, Some(10));
    assert_eq!(l.state.key_description.get(), "Ten");
    // Then a heading: the choice stays.
    assert_eq!(l.select(0), 1);
    assert_eq!(l.state.key_action, Some(10));
    // Code 4 without `int`: return value 0; other codes of the list: -1.
    assert_eq!(l.send("keylist", code::ITEM_SELECTED).0, 0);
    assert_eq!(l.send("keylist", code::QUERY_VALUE).0, -1);
}

// QUIRK O-113: kept
#[test]
fn selecting_shows_key_and_description() {
    let Some(mut l) = corpus_logic() else {
        return;
    };
    // Before the first choice: empty buffers (`FUN_004BC540` sets both to L"").
    assert_eq!(l.text("key"), (1, Some(String::new())));
    assert_eq!(l.text("keyname"), (1, Some(String::new())));
    let i = index_of(&l.key_list, 573);
    assert_eq!(l.select(i), 1);
    assert_eq!(l.state.key_action, Some(573));
    // Code 7 at KEY: display name of the layout (`ALT N` -> texts 50018, 50078), `size 40`, `inputbuffer 1`.
    let (r, msg) = l.send("key", code::QUERY_TEXT);
    assert_eq!(r, 1);
    // As a pointer to the buffer of the logic (`+0x9A0`), not as a copy.
    assert_eq!(
        msg.params.get("text"),
        Some(&Param::Shared(l.state.key_text.clone()))
    );
    assert_eq!(l.state.key_text.get(), "ALT N");
    assert_eq!(msg.params.get("size"), Some(&Param::Int(40)));
    assert_eq!(msg.params.get("inputbuffer"), Some(&Param::Int(1)));
    // Code 7 at KEYNAME: the list text including the placeholder (the label sets without 0x200, it disappears there).
    let description = l.key_list.entries[i as usize].1.clone();
    assert_eq!(l.text("keyname"), (1, Some(description)));
    // `CONTROL H` is shown with the German display names.
    l.config.key_map.retain(|(id, _)| *id != 572);
    l.config.key_map.push((572, "CONTROL H".into()));
    l.select(index_of(&l.key_list, 572));
    assert_eq!(l.state.key_text.get(), "STRG H");
    // Without an entry in the layout: "not defined" is read as a combination (two unknown words -> code 0 twice)
    // and shown as a display name, not literally (O-113).
    l.config.key_map.retain(|(id, _)| *id != 571);
    l.select(index_of(&l.key_list, 571));
    assert_eq!(l.state.key_action, Some(571));
    let zero = l.texts().get(50_000).expect("Text 50000").to_string();
    assert_eq!(l.state.key_text.get(), format!("{zero} {zero}"));
    // Other codes of KEY and KEYNAME: -1.
    assert_eq!(l.send("keyname", code::QUERY_STATUS).0, -1);
    assert_eq!(l.send("key", code::QUERY_STATUS).0, -1);
}

// QUIRK O-112: kept
#[test]
fn arrows_digits_and_escape_are_rejected() {
    let mut l = Logic::new(
        fixture(),
        list(&[(573, "N [KEYID 573]")]),
        &[(573, "ALT N")],
    );
    l.select(0);
    // Rejected (-1): arrows, `"0" <= name <= "9"` (digits, plus everything that begins with 0-8: unnamed keys
    // `0xNN`, `1 A`; O-112) and ESCAPE.
    let digits: Vec<String> = (0..10).map(|d| d.to_string()).collect();
    let mut rejected: Vec<&str> = vec![
        "UP", "DOWN", "LEFT", "RIGHT", "ESCAPE", "0xDE", "0xC0", "1 A",
    ];
    rejected.extend(digits.iter().map(String::as_str));
    for name in rejected {
        assert_eq!(l.key(name), -1, "{name}");
    }
    assert_eq!(l.key_map(), [(573, "ALT N")]);
    // ESC broadcast (code 0xB with `key = 27`, without `keydef`): not taken.
    let mut msg = ElementMsg::new("controloptions", "key", code::EDIT_KEY);
    msg.params.insert("key".into(), Param::Int(27));
    assert_eq!(l.deliver(msg).0, -1);
    // Only exact names: Shift+arrow, Ctrl+digit, `9 A`/`9 F1`/`A` (after `9`) and `+` (before `0`) pass.
    for name in ["SHIFT UP", "CONTROL 1", "9 A", "9 F1", "A", "+", "ß"] {
        assert_eq!(l.key(name), 1, "{name}");
        assert_eq!(l.key_map(), [(573, name)]);
    }
}

// QUIRK O-102: kept
// QUIRK O-114: deviation
#[test]
fn new_key_is_stored_in_the_key_map() {
    // Logic: Engine.ini form in its place or appended (`CMap::operator[]`); without `keys.ini` rows there is no
    // conflict (actions without a row); before the first choice the reimplementation takes nothing.
    let mut l = Logic::new(
        fixture(),
        list(&[(573, "N [KEYID 573]"), (574, "R [KEYID 574]")]),
        &[(570, "SPACE"), (573, "ALT N")],
    );
    assert_eq!(l.key("CONTROL H"), 1);
    assert_eq!(l.key_map(), [(570, "SPACE"), (573, "ALT N")]);
    l.select(0);
    assert_eq!(l.key("CONTROL H"), 1);
    assert_eq!(l.key_map(), [(570, "SPACE"), (573, "CONTROL H")]);
    // Without a `keys.ini` row no conflict: the same key as 570 is taken.
    l.select(1);
    assert_eq!(l.key("SPACE"), 1);
    assert_eq!(
        l.key_map(),
        [(570, "SPACE"), (573, "CONTROL H"), (574, "SPACE")]
    );

    // Live: click an entry in the list, Ctrl+H in the layout field (it has the focus from `OnAttach`).
    let Some(c) = corpus() else {
        return;
    };
    let defaults = load_default_keys(c.vfs());
    let config = EngineConfig {
        is_intro: false,
        ..EngineConfig::default().with_default_keys(&defaults)
    };
    let app = MenuApp::new(c.vfs(), c.texts(), config, MenuOptions::default()).expect("MenuApp");
    let mut d = Driver::with_app(c, app, Box::new(dispatch));
    d.push(open("controloptions", true));
    d.idle();
    // The list sets with 0x2C6: `[KEYID n]` shows "(key)" - with the menu state more glyphs than without.
    let tree = d.app.tree();
    let now = tree.pass_now;
    let ctx_keys = text_context(&d.app.config().key_map, d.world.texts(), None);
    let mut ui = d.world.ctx(crate::app::tests::S1280);
    let with = crate::paint::paint_ui_with(tree, &mut ui, now, Some(&ctx_keys), None);
    let without = crate::paint::paint_ui(tree, &mut ui, now);
    assert!(
        with.len() > without.len(),
        "{} <= {}",
        with.len(),
        without.len()
    );
    d.click("controloptions", "keylist");
    d.idle();
    let id = d.app.screen_state().key_action.expect("an action selected");
    assert!(id >= 10, "action {id}");
    let now = d.now;
    d.app.input(
        UiInput::Key {
            held: vec![VK_CONTROL, VK_H],
            ch: Some(u16::from(VK_H)),
        },
        crate::app::tests::S1280,
        now,
    );
    d.idle();
    let key_map = &d.app.config().key_map;
    assert_eq!(key_map.len(), defaults.len(), "no new entry");
    let slot = key_map.iter().position(|(k, _)| *k == id).unwrap();
    assert_eq!(key_map[slot], (id, "CONTROL H".to_string()));
    assert_eq!(slot, defaults.iter().position(|(k, _)| *k == id).unwrap());
    // The layout field shows the display name (`FUN_0049BDF0`).
    let report = d.app.report(crate::app::tests::S1280, d.now);
    let field = report
        .elements
        .iter()
        .find(|e| e.dialog == "controloptions" && e.name == "key")
        .expect("layout field");
    assert_eq!(field.text.as_deref(), Some("STRG H"));
}

// QUIRK O-102: kept
#[test]
fn reset_asks_then_replaces_the_key_map() {
    // Logic: `RESET` asks with 15096, OK = `{0x32}`, no cancel command; return value -1.
    let mut l = Logic::new(fixture(), KeyList::default(), &[]);
    assert_eq!(l.send("reset", code::TRIGGERED).0, -1);
    assert_eq!(l.state.info_text, Some(15096));
    assert_eq!(l.state.info_ok, Some(AppCmd::ResetKeys));
    assert_eq!(l.state.info_cancel, None);
    let queued: Vec<AppCmd> = l.queue.iter().cloned().collect();
    assert_eq!(queued, [open("infoframe", true)]);
    // `FUN_0049D520`: first empty (`CMap::RemoveAll`), then the defaults in file order; a duplicate ID keeps its
    // first position and the last value; foreign IDs disappear (O-102).
    let mut map = vec![(99_999, "X".to_string()), (573, "Q".to_string())];
    let defaults =
        [(573, "ALT N"), (570, "SPACE"), (573, "ALT M")].map(|(id, k)| (id, k.to_string()));
    reset_keys(&mut map, &defaults);
    let want = [(573, "ALT M"), (570, "SPACE")];
    assert_eq!(map, want.map(|(id, k)| (id, k.to_string())).to_vec());
    // Without defaults (file missing): empty.
    reset_keys(&mut map, &[]);
    assert!(map.is_empty());

    // Live: RESET -> confirmation prompt -> OK -> `0x32` reloads `defaultkeys.ini`.
    let Some(c) = corpus() else {
        return;
    };
    let defaults = load_default_keys(c.vfs());
    let mut keys = defaults.clone();
    keys.retain(|(id, _)| *id != 573);
    keys.insert(0, (99_999, "X".into()));
    keys.push((573, "Q".into()));
    let config = EngineConfig {
        is_intro: false,
        key_map: keys,
        ..EngineConfig::default()
    };
    let app = MenuApp::new(c.vfs(), c.texts(), config, MenuOptions::default()).expect("MenuApp");
    let mut d = Driver::with_app(c, app, Box::new(dispatch));
    d.push(open("controloptions", true));
    d.idle();
    let list_before = d.app.key_list().clone();
    d.click("controloptions", "reset");
    d.idle();
    assert_eq!(d.app.config().key_map[0], (99_999, "X".to_string()));
    d.click("infoframe", "ok");
    d.idle();
    // Afterwards exactly the defaults in file order: the foreign ID is gone, 573 is back in its place.
    assert_eq!(d.app.config().key_map, defaults);
    // The list is not rebuilt (only at start, O-102).
    assert_eq!(*d.app.key_list(), list_before);
}

/// `controloptions` in operation over the corpus, with defaults as the layout.
fn live_controls() -> Option<Driver<Corpus>> {
    let c = corpus()?;
    let defaults = load_default_keys(c.vfs());
    let config = EngineConfig {
        is_intro: false,
        ..EngineConfig::default().with_default_keys(&defaults)
    };
    let app = MenuApp::new(c.vfs(), c.texts(), config, MenuOptions::default()).expect("MenuApp");
    let mut d = Driver::with_app(c, app, Box::new(dispatch));
    d.push(open("controloptions", true));
    d.idle();
    Some(d)
}

/// Text of `controloptions/<element>` in the report.
fn shown(d: &Driver<Corpus>, element: &str) -> Option<String> {
    let report = d.app.report(crate::app::tests::S1280, d.now);
    report
        .elements
        .iter()
        .find(|e| e.dialog == "controloptions" && e.name == element)
        .unwrap_or_else(|| panic!("{element} is missing"))
        .text
        .clone()
}

#[test]
fn selection_shows_in_keyname_and_key_without_reopening() {
    // Label (`FUN_00462490`, `+0x98 = *text`) and EditField with `inputbuffer` (`FUN_0045F5D0`, `+0xC0 = *text`)
    // keep the pointer to the buffers of the logic: after a choice both show description and key in the next frame,
    // without a new query; what the field writes is in the buffer of the logic.
    let Some(mut d) = live_controls() else {
        return;
    };
    assert_eq!(shown(&d, "keyname").as_deref(), Some(""));
    assert_eq!(shown(&d, "key").as_deref(), Some(""));
    let before = d.frame();
    let queries = |d: &Driver<Corpus>| {
        d.log
            .borrow()
            .iter()
            .filter(|m| m.2 == code::QUERY_TEXT)
            .count()
    };
    let asked = queries(&d);
    d.click("controloptions", "keylist");
    let after = d.frame();
    let state = d.app.screen_state();
    let id = state.key_action.expect("an action selected");
    let description = state.key_description.get();
    let key = state.key_text.get();
    assert!(
        description.ends_with(&format!(" [KEYID {id}]")),
        "{description:?}"
    );
    assert!(!key.is_empty());
    assert_eq!(shown(&d, "keyname"), Some(description));
    assert_eq!(shown(&d, "key"), Some(key));
    assert_ne!(before, after, "the frame shows the choice");
    assert_eq!(queries(&d), asked, "no new query of code 7");
    // The layout field writes into the buffer of the logic (Ctrl+H -> display name).
    let now = d.now;
    d.app.input(
        UiInput::Key {
            held: vec![VK_CONTROL, VK_H],
            ch: Some(u16::from(VK_H)),
        },
        crate::app::tests::S1280,
        now,
    );
    d.frame();
    assert_eq!(d.app.screen_state().key_text.get(), "STRG H");
    assert_eq!(shown(&d, "key").as_deref(), Some("STRG H"));
}

#[test]
fn shared_buffers_keep_the_frames_deterministic() {
    // Same inputs at the same times -> same draw commands, also with shared buffers.
    let run = || {
        let mut d = live_controls()?;
        let mut frames = Vec::new();
        d.click("controloptions", "keylist");
        frames.push(d.frame());
        let now = d.now;
        d.app.input(
            UiInput::Key {
                held: vec![VK_CONTROL, VK_H],
                ch: Some(u16::from(VK_H)),
            },
            crate::app::tests::S1280,
            now,
        );
        for _ in 0..3 {
            frames.push(d.frame());
        }
        Some(frames)
    };
    let (Some(a), Some(b)) = (run(), run()) else {
        return;
    };
    assert_eq!(a, b);
}

/// O-102 (`GameApp_AssignKey` `FUN_004BF7C0` -> `FUN_0049D2F0`, context `FUN_0049DBB0`): a key that already has an
/// action in the `keys.ini` context of the action (its **last** row) is rejected - the layout stays, without a
/// message, response 1 anyway. The rows with type and nation equal to the value or 0 (for the value 0 only 0) and
/// exactly the same selection are checked; the own action counts. Actions without a row (448-451, 1911) are free
/// (deviation, re/menu-flow).
// QUIRK O-102, O-327, O-328: kept
#[test]
fn assigning_a_key_used_in_the_context_is_rejected() {
    let Some(mut l) = corpus_logic() else {
        return;
    };
    let defaults = l.config.key_map.clone();
    let lines = l.key_list.keys_ini.clone();
    assert_eq!(lines.len(), 652);
    let cases: [(i32, &str, bool); 13] = [
        (404, "R", false),
        (413, "Q", false),
        (434, "U", false),
        (408, "SHIFT S", false),
        (423, "SHIFT F", false),
        (578, "F4", false),
        (578, "F12", true),
        (572, "X", true),
        (409, "R", true),
        (700, "Q", true),
        (1309, "L", true),
        (554, "S", true),
        (448, "Q", true),
    ];
    for (id, key, free) in cases {
        l.config.key_map = defaults.clone();
        assert_eq!(
            conflicts(id, key, &lines, &l.config.key_map),
            !free,
            "{id} -> {key}"
        );
        let index = index_of(&l.key_list, id);
        assert_eq!(l.select(index), 1);
        let old = l.state.key_text.get();
        assert_eq!(l.key(key), 1, "{id} -> {key}: response always 1");
        let now = l
            .config
            .key_map
            .iter()
            .find(|(k, _)| *k == id)
            .unwrap()
            .1
            .clone();
        if free {
            assert_eq!(now, key, "{id} -> {key} accepted");
        } else {
            assert_eq!(l.config.key_map, defaults, "{id} -> {key} rejected");
            // `FUN_0049D4E0`: the old key back into the buffer of the field.
            assert_eq!(l.state.key_text.get(), old, "{id}: old key in the buffer");
        }
    }
    // The own action counts: its own key is "taken" (without consequence), as are the 14 defaults that already
    // collide in their own context (404/414 `Q`, 423 -> 572 `SHIFT D`).
    l.config.key_map = defaults.clone();
    for (id, key) in [(414, "Q"), (404, "Q"), (423, "SHIFT D"), (578, "F1")] {
        assert!(conflicts(id, key, &lines, &defaults), "{id} {key}");
    }

    // Live: the layout field writes the display name of the **new** key into the same buffer after the
    // response 1 (`0x0046004F`...`0x00460060`, after `FUN_004BF7C0`) - it shows the rejected key, the layout stays
    // (O-327). A new choice shows the valid key again.
    let Some(c) = corpus() else {
        return;
    };
    let defaults = load_default_keys(c.vfs());
    let config = EngineConfig {
        is_intro: false,
        ..EngineConfig::default().with_default_keys(&defaults)
    };
    let app = MenuApp::new(c.vfs(), c.texts(), config, MenuOptions::default()).expect("MenuApp");
    let row = index_of(app.key_list(), 404);
    // The click in the list selects the row under the mouse; the logic gets the row of 404 instead.
    let logic = move |msg: &mut ElementMsg, env: &mut ScreenEnv| {
        if msg.element == "keylist" && msg.code == code::ITEM_SELECTED {
            msg.params.insert("int".into(), Param::Int(row));
        }
        dispatch(msg, env)
    };
    let mut d = Driver::with_app(c, app, Box::new(logic));
    d.push(open("controloptions", true));
    d.idle();
    d.click("controloptions", "keylist");
    d.idle();
    assert_eq!(d.app.screen_state().key_action, Some(404));
    assert_eq!(shown(&d, "key").as_deref(), Some("Q"));
    const VK_R: u8 = 0x52;
    let now = d.now;
    d.app.input(
        UiInput::Key {
            held: vec![VK_R],
            ch: Some(u16::from(VK_R)),
        },
        crate::app::tests::S1280,
        now,
    );
    d.frame();
    assert_eq!(d.app.config().key_map, defaults, "404 -> R rejected");
    assert_eq!(
        shown(&d, "key").as_deref(),
        Some("R"),
        "the field shows the rejected key"
    );
    // New choice: the valid key again.
    d.click("controloptions", "keylist");
    d.idle();
    assert_eq!(shown(&d, "key").as_deref(), Some("Q"));
}
