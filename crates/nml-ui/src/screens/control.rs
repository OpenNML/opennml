//! Key bindings `CONTROLOPTIONS` (`FUN_004D02A0`, called at `0x004DC9A9`; re/menu-flow,
//! `CONTROLOPTIONS`).
//!
//! The original builds the list `KEYLIST` **once at program start** (`FUN_004BC540` from `FUN_004C9290`) in the order
//! of `defaultkeys.ini` ([`KeyList::build`]); a choice remembers action, key text and list text in buffers of the
//! application (`GameApp+0x19B8`, `+0x9A0`, `+0x1A0`). The binding field `KEY` and the label `KEYNAME` get pointers
//! to these buffers via code 7 ([`crate::msg::SharedText`]) and show a new choice in the next frame. A new key in the
//! binding field (code `0xB`, `keydef`) goes to `m_KeyMap`; `RESET` asks for confirmation, clears the bindings and
//! reloads the defaults (O-102).
//!
//! The original checks a new key in the `keys.ini` context of the action (`GameApp_AssignKey` `FUN_004BF7C0` ->
//! `FUN_0049D2F0`) and rejects an assigned one ([`conflicts`]).

use nml_formats::keys::{DEFAULT_KEYS_PATH, parse_default_keys};
use nml_formats::keys_ini::{KEYS_INI_PATH, KeysIniLine, parse_keys_ini};
use nml_formats::text::decode_cp1252;
use nml_rda::Vfs;

use super::{ScreenEnv, ask};
use crate::assets::Texts;
use crate::keys::chord_display;
use crate::msg::{AppCmd, ElementMsg, Param, code};
use crate::text::{CurLevelText, TextContext};
use crate::widgets::{int_param, text_param};

/// List text of an action: text `ID + 40000` (`FUN_004BC540`).
const ACTION_TEXT_BASE: i32 = 40_000;
/// From this ID on the list appends the placeholder of the key (`ID < 0x65` -> text unchanged).
const FIRST_KEYID: i32 = 101;
/// IDs 0...9 are headlines and not selectable (`FUN_004BD780`, `CMP EDI,0xA; JL`).
const FIRST_SELECTABLE: i32 = 10;
/// Fallback value of `FUN_0049D4E0` (`0x00616D38`) if the action has no entry in `m_KeyMap`.
const NOT_DEFINED: &str = "not defined";
/// Size of the binding field (`FUN_004636B0("size", 0x28)`).
const KEY_FIELD_SIZE: i32 = 40;
/// "Do you really want to reset all hotkeys?" (`FUN_0055FFE0(0x3AF8)`).
const RESET_QUESTION: i32 = 15096;

/// The key binding list (`GameApp+0x19BC`, count `+0x19C0`): per action ID and list text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyList {
    pub entries: Vec<(i32, String)>,
    /// The lines of `keys.ini` in file order for the conflict check ([`conflicts`]; the original reads the file once
    /// at start into the keyboard object, `FUN_0049CAA0`). Empty without [`crate::app::MenuApp::set_keys_ini`].
    pub keys_ini: Vec<KeysIniLine>,
}

impl KeyList {
    /// `FUN_004BC540`: per entry of `defaultkeys.ini` in file order (order list `Keyboard+0xA0`)
    /// text `ID + 40000`, otherwise text `ID`, otherwise `error` (`FUN_0055FFC0`, without the shared number buffer);
    /// from ID 101 `"%s [KEYID %d]"` (`0x0061EC24`), below that the text alone (headlines 40000...40009 with tags).
    pub fn build(default_keys: &[(i32, String)], texts: &Texts) -> KeyList {
        let entries = default_keys
            .iter()
            .map(|&(id, _)| {
                let text = texts
                    .get(id.wrapping_add(ACTION_TEXT_BASE))
                    .or_else(|| texts.get(id))
                    .unwrap_or("error");
                let text = if id < FIRST_KEYID {
                    text.to_string()
                } else {
                    format!("{text} [KEYID {id}]")
                };
                (id, text)
            })
            .collect();
        KeyList {
            entries,
            keys_ini: Vec::new(),
        }
    }
}

/// `defaultkeys.ini` from the game directory (Windows-1252). If it is missing, a warning and no defaults (the
/// original then finds no lines).
pub fn load_default_keys(vfs: &Vfs) -> Vec<(i32, String)> {
    match vfs.read(DEFAULT_KEYS_PATH) {
        Ok(bytes) => parse_default_keys(&decode_cp1252(&bytes)),
        Err(e) => {
            log::warn!("Key defaults \"{DEFAULT_KEYS_PATH}\" not readable: {e}");
            Vec::new()
        }
    }
}

/// `keys.ini` from the game directory (Windows-1252, [`parse_keys_ini`]). If it is missing, a warning and no lines
/// (then every key is free).
pub fn load_keys_ini(vfs: &Vfs) -> Vec<KeysIniLine> {
    match vfs.read(KEYS_INI_PATH) {
        Ok(bytes) => parse_keys_ini(&decode_cp1252(&bytes)),
        Err(e) => {
            log::warn!("Key lines \"{KEYS_INI_PATH}\" not readable, no conflict check: {e}");
            Vec::new()
        }
    }
}

/// Conflict check of the binding (O-102): does the key `new_key` already have an action in the `keys.ini` context of
/// the action `action`?
///
/// - Context (`FUN_0049DBB0`): for **every** row of the action in file order the original sets type, nation and
///   selection (`0x0049D950`, `0x0049D750`, `0x0049CF60`), without abort - the **last** row of the action applies.
/// - Check (`FUN_0049D2F0(name, 0)`) over the active list: rows whose type and nation equal the value or 0
///   (for value 0 only the 0 rows, no wildcard) and whose selection is exactly equal (for selection != 0 the
///   0 rows drop out, for 0 only they remain); assigned if `key_map[row.action]` as text is exactly `new_key`
///   (`_mbscmp`). The **own** action is not excluded: its own key counts as assigned.
/// - **Deviation:** an action without a `keys.ini` row (448-451, 1911) leaves the most recently set context in the
///   original (previous choice in the list or the context of the last match) and checks in it. The reimplementation
///   does not keep the menu context and calls it free (re/menu-flow).
pub fn conflicts(
    action: i32,
    new_key: &str,
    lines: &[KeysIniLine],
    key_map: &[(i32, String)],
) -> bool {
    let Some(ctx) = lines.iter().rfind(|l| l.action == action) else {
        return false;
    };
    let level = |value: i32, line: i32| line == 0 || (value != 0 && line == value);
    lines.iter().any(|l| {
        level(ctx.kind, l.kind)
            && level(ctx.nation, l.nation)
            && l.selection == ctx.selection
            && key_map
                .iter()
                .find(|(id, _)| *id == l.action)
                .is_some_and(|(_, key)| key == new_key)
    })
}

/// `m_KeyMap[id] = key` (`CMap::operator[]`, `FUN_00485720`): a known ID gets the key at its place,
/// a new one is appended (like reading `Engine.ini`, `nml_formats::engine_ini`).
pub fn set_key(key_map: &mut Vec<(i32, String)>, id: i32, key: &str) {
    match key_map.iter_mut().find(|(known, _)| *known == id) {
        Some(slot) => slot.1 = key.to_string(),
        None => key_map.push((id, key.to_string())),
    }
}

/// Command `0x32` (`0x004C9A89` -> `FUN_0049D520("data/gifcon/defaultkeys.ini")`, call `0x004C9AB3` with
/// `g_pEngineConfig + 0x268`): first `CMap::RemoveAll` on `m_KeyMap` (`FUN_004819E0`, `0x0049D547`...`0x0049D554`,
/// unconditionally and **before** opening the file), then per row `Lookup` -> on a hit `RemoveKey` (`FUN_0049C710`,
/// only for duplicate IDs), `operator[]` and `AddTail` to the order list `+0x9C`. The bindings then consist exactly
/// of the defaults - IDs that were only in the settings file are gone; if the file is missing, it is empty (O-102).
/// The order list is never cleared and grows with each `RESET` (without a visible consequence, the list of the page
/// is only created at start); the reimplementation does not keep it.
pub fn reset_keys(key_map: &mut Vec<(i32, String)>, defaults: &[(i32, String)]) {
    key_map.clear();
    for (id, key) in defaults {
        set_key(key_map, *id, key);
    }
}

/// Menu state of the text set from the bindings: per action the display name of its key (`[KEYID n]`), plus the
/// details of the selected map (`[CURLEVEL...]`).
pub fn text_context(
    key_map: &[(i32, String)],
    texts: &Texts,
    cur_level: Option<CurLevelText>,
) -> TextContext {
    TextContext {
        key_names: key_map
            .iter()
            .map(|(id, key)| (*id, chord_display(key, texts)))
            .collect(),
        undefined: chord_display(NOT_DEFINED, texts),
        cur_level,
    }
}

/// Rejected keys of the binding field (`FUN_004D02A0` branch `key`, code `0xB` up to `LAB_004D0593`; `_mbscmp`
/// on the combination name): `UP`, `DOWN`, `LEFT`, `RIGHT`, every name with `"0" <= name <= "9"` and `ESCAPE`. The
/// string comparison hits everything from "0" to "9" inclusive, so every name that starts with 0-8 and exactly
/// "9" - hence all unnamed keys (`0xNN`, such as `ö`, `ä`, `ü`) and combinations such as `1 A`, but not
/// `9 A` (O-112).
fn rejected(name: &str) -> bool {
    matches!(name, "UP" | "DOWN" | "LEFT" | "RIGHT" | "ESCAPE") || ("0"..="9").contains(&name)
}

pub(super) fn handle(msg: &mut ElementMsg, env: &mut ScreenEnv) -> i32 {
    match (msg.element.as_str(), msg.code) {
        // Code 2: all list texts (`FUN_004BCD20`).
        ("keylist", code::QUERY_ITEMS) => {
            let items = env
                .key_list
                .entries
                .iter()
                .map(|(_, t)| t.clone())
                .collect();
            msg.params.insert("array".into(), Param::Array(items));
            1
        }
        // Code 4: `FUN_004BD780(index)`; without `int` return value 0.
        ("keylist", code::ITEM_SELECTED) => {
            let Some(index) = int_param(msg, "int") else {
                return 0;
            };
            select(index, env);
            1
        }
        // Code 7: pointer to the list text (`GameApp+0x1A0`).
        ("keyname", code::QUERY_TEXT) => {
            let text = env.state.key_description.clone();
            msg.params.insert("text".into(), Param::Shared(text));
            1
        }
        // Code 7: pointer to the key text (`GameApp+0x9A0`), `size 40`, `inputbuffer 1`.
        ("key", code::QUERY_TEXT) => {
            let text = env.state.key_text.clone();
            msg.params.insert("text".into(), Param::Shared(text));
            msg.params.insert("size".into(), Param::Int(KEY_FIELD_SIZE));
            msg.params.insert("inputbuffer".into(), Param::Int(1));
            1
        }
        // Code `0xB`: only with `keydef` (otherwise -1, also for the ESC broadcast with `key = 27`); rejected names -1,
        // otherwise `GameApp_AssignKey` and 1.
        ("key", code::EDIT_KEY) => {
            let Some(name) = text_param(msg, "keydef") else {
                return -1;
            };
            if rejected(&name) {
                return -1;
            }
            assign_key(&name, env);
            1
        }
        // Code 8: confirmation 15096, OK -> `{0x32}`; return value -1 (`LAB_004D070D`).
        ("reset", code::TRIGGERED) => {
            ask(
                env.state,
                env.queue,
                RESET_QUESTION,
                Some(AppCmd::ResetKeys),
                None,
            );
            -1
        }
        _ => -1,
    }
}

/// `FUN_004BD780`: only an index in the list with action ID >= 10. Remembers the action (`+0x19B8`), sets the
/// keyboard context (`FUN_0049DBB0`; the reimplementation only calculates it in [`conflicts`] from the rows), the key
/// text = display name of the binding or of "not defined" (O-113) and the list text.
fn select(index: i32, env: &mut ScreenEnv) {
    let Some((id, text)) = usize::try_from(index)
        .ok()
        .and_then(|i| env.key_list.entries.get(i))
    else {
        return;
    };
    if *id < FIRST_SELECTABLE {
        return;
    }
    let key = env
        .config
        .key_map
        .iter()
        .find(|(known, _)| known == id)
        .map_or(NOT_DEFINED, |(_, key)| key.as_str());
    env.state.key_text.set(&chord_display(key, env.texts));
    env.state.key_action = Some(*id);
    env.state.key_description.set(text);
}

/// `GameApp_AssignKey` (`FUN_004BF7C0`): key free in the `keys.ini` context of the action ([`conflicts`]) ->
/// `m_KeyMap[action] = name` (`FUN_0049D910`); assigned -> the binding stays, the old key goes as display name (if
/// missing: "not defined", `FUN_0049D4E0`) into the buffer `+0x9A0`. No message, no sound. Afterwards the binding
/// field writes, because of the answer 1, the display name of the **new** key into the same buffer (O-327): it shows
/// the rejected key.
///
/// Before the first choice `GameApp+0x19B8` is uninitialized in the original (the application is created with
/// `operator new(0xE770)` without zeroing; `FUN_004C7360`, `FUN_004C9290` and `FUN_004BC540` do not write the field,
/// the only writer is `FUN_004BD780`, called only from `0x004D0337`): the key would go to a random ID. The
/// reimplementation then takes nothing, the answer stays 1 (O-114).
fn assign_key(name: &str, env: &mut ScreenEnv) {
    let Some(id) = env.state.key_action else {
        return;
    };
    if conflicts(id, name, &env.key_list.keys_ini, &env.config.key_map) {
        let old = env
            .config
            .key_map
            .iter()
            .find(|(known, _)| *known == id)
            .map_or(NOT_DEFINED, |(_, key)| key.as_str());
        env.state.key_text.set(&chord_display(old, env.texts));
        return;
    }
    set_key(&mut env.config.key_map, id, name);
}

#[cfg(test)]
mod tests;
