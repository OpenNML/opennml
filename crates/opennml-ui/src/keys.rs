//! Key names of the original and key combinations (re/menu-system 12.7).
//!
//! The names are the file format of `Engine.ini`, `defaultkeys.ini` and `keymap*.ini`, not game data. A combination
//! is a list of virtual key codes (VK, 0-255).

use opennml_formats::dlg::upper_word;

use crate::assets::Texts;

/// Display name of key `vk`: text `50000 + vk` (`keyboardlayout.deu`, the sixth `TextDatei` of
/// `localisation.deu`).
pub const KEY_TEXT_BASE: i32 = 50_000;

/// Table `DAT_006714B0[vk]` (256 `CString`, static initializer `0x0060D53F`-`0x0060E92E`, one block per VK in
/// ascending order): 141 named keys, the rest are called `0xNN` (two uppercase hex digits). Typos of the original
/// remain (`MULTIPLAY` 0x6A, `DEVIDE` 0x6F); the name of 0xDB is the cp1252 byte 0xDF (`ß`).
#[rustfmt::skip]
const KEY_NAMES: [&str; 256] = [
    // 0x00
    "0x00", "LBUTTON", "RBUTTON", "CANCEL", "MBUTTON", "0x05", "0x06", "0x07",
    "BACK", "TAB", "0x0A", "0x0B", "CLEAR", "RETURN", "0x0E", "0x0F",
    // 0x10
    "SHIFT", "CONTROL", "ALT", "PAUSE", "CAPITAL", "KANA", "0x16", "JUNJA",
    "FINAL", "HANJA", "0x1A", "ESCAPE", "CONVERT", "NONCONVERT", "ACCEPT", "MODECHANGE",
    // 0x20
    "SPACE", "PRIOR", "NEXT", "END", "HOME", "LEFT", "UP", "RIGHT",
    "DOWN", "SELECT", "PRINT", "EXECUTE", "SNAPSHOT", "INSERT", "DELETE", "HELP",
    // 0x30
    "0", "1", "2", "3", "4", "5", "6", "7",
    "8", "9", "0x3A", "0x3B", "0x3C", "0x3D", "0x3E", "0x3F",
    // 0x40
    "0x40", "A", "B", "C", "D", "E", "F", "G",
    "H", "I", "J", "K", "L", "M", "N", "O",
    // 0x50
    "P", "Q", "R", "S", "T", "U", "V", "W",
    "X", "Y", "Z", "LWIN", "RWIN", "APPS", "0x5E", "0x5F",
    // 0x60
    "NUMPAD0", "NUMPAD1", "NUMPAD2", "NUMPAD3", "NUMPAD4", "NUMPAD5", "NUMPAD6", "NUMPAD7",
    "NUMPAD8", "NUMPAD9", "MULTIPLAY", "ADD", "SEPARATOR", "SUBTRACT", "DECIMAL", "DEVIDE",
    // 0x70
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8",
    "F9", "F10", "F11", "F12", "F13", "F14", "F15", "F16",
    // 0x80
    "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    "0x88", "0x89", "0x8A", "0x8B", "0x8C", "0x8D", "0x8E", "0x8F",
    // 0x90
    "NUMLOCK", "SCROLL", "0x92", "0x93", "0x94", "0x95", "0x96", "0x97",
    "0x98", "0x99", "0x9A", "0x9B", "0x9C", "0x9D", "0x9E", "0x9F",
    // 0xA0
    "LSHIFT", "RSHIFT", "LCONTROL", "RCONTROL", "LALT", "RALT", "0xA6", "0xA7",
    "0xA8", "0xA9", "0xAA", "0xAB", "0xAC", "0xAD", "0xAE", "0xAF",
    // 0xB0
    "0xB0", "0xB1", "0xB2", "0xB3", "0xB4", "0xB5", "0xB6", "0xB7",
    "0xB8", "0xB9", "0xBA", "+", ",", "-", ".", "#",
    // 0xC0
    "0xC0", "0xC1", "0xC2", "0xC3", "0xC4", "0xC5", "0xC6", "0xC7",
    "0xC8", "0xC9", "0xCA", "0xCB", "0xCC", "0xCD", "0xCE", "0xCF",
    // 0xD0
    "0xD0", "0xD1", "0xD2", "0xD3", "0xD4", "0xD5", "0xD6", "0xD7",
    "0xD8", "0xD9", "0xDA", "\u{df}", "GRAVE", "0xDD", "0xDE", "0xDF",
    // 0xE0
    "0xE0", "0xE1", "0xE2", "0xE3", "0xE4", "0xE5", "0xE6", "0xE7",
    "0xE8", "0xE9", "0xEA", "0xEB", "0xEC", "0xED", "0xEE", "0xEF",
    // 0xF0
    "0xF0", "0xF1", "0xF2", "0xF3", "0xF4", "0xF5", "ATTN", "CRSEL",
    "EXSEL", "EREOF", "PLAY", "ZOOM", "NONAME", "PA1", "OEM_CLEAR", "0xFF",
];

/// General modifier keys and their left/right form (`FUN_0049BC80`).
const MODIFIERS: [(u8, [u8; 2]); 3] = [
    (0x10, [0xA0, 0xA1]),
    (0x11, [0xA2, 0xA3]),
    (0x12, [0xA4, 0xA5]),
];

/// Name of a key (`DAT_006714B0[vk]`); never empty, unnamed keys are called `0xNN`.
pub fn vk_name(vk: u8) -> &'static str {
    KEY_NAMES[usize::from(vk)]
}

/// Whitespace for `CString::TrimLeft`/`TrimRight` (`_istspace` in the C locale: `\t`-`\r` and space).
fn is_c_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ')
}

/// Code of a word: first exact match in VK order, otherwise 0 (`FUN_0049C050`).
fn word_code(word: &str) -> u8 {
    (0..=255u8).find(|&vk| vk_name(vk) == word).unwrap_or(0)
}

/// Name -> combination like `FUN_0049C050`: trim left and right, uppercase (`FUN_00574EA0` = `upper_word`),
/// split off the next word at the first space and trim the rest on the left, until no space remains
/// (the rest is the last word; tabs do not separate). Each word is compared exactly with the table linearly from
/// VK 0 to 255 (`_mbscmp`), the first match is used; without a match code 0. An empty text yields an empty word,
/// i.e. `[0]`.
///
/// Consequence of the uppercasing: the `0xNN` names become `0XNN` and never match (O-107).
pub fn parse_chord(name: &str) -> Vec<u8> {
    let text = upper_word(name.trim_matches(is_c_space));
    let mut rest = text.as_str();
    let mut codes = Vec::new();
    // `_mbschr(rest, ' ')`, `Left` as the word, `Mid` as the rest, `TrimLeft`; without a space the rest is the last
    // word.
    while let Some(pos) = rest.find(' ') {
        codes.push(word_code(&rest[..pos]));
        rest = rest[pos..].trim_start_matches(is_c_space);
    }
    codes.push(word_code(rest));
    codes
}

/// Combination -> name like `FUN_0049C340` over the table of held keys (12.7): names separated by one
/// space, bucket `vk >> 4` ascending, within a bucket the most recently pressed first. `held` is in
/// press order and contains each key once.
pub fn chord_name(held: &[u8]) -> String {
    let mut names: Vec<&str> = Vec::with_capacity(held.len());
    for bucket in 0..16u8 {
        names.extend(
            held.iter()
                .rev()
                .filter(|&&vk| vk >> 4 == bucket)
                .map(|&vk| vk_name(vk)),
        );
    }
    names.join(" ")
}

/// Display name of a binding like `FUN_0049C2B0` + `FUN_0049BDF0`: read the text as a combination
/// ([`parse_chord`]) and append the text `50000 + vk` for each code (`FUN_0055FFE0`, if it is missing, the number,
/// O-4), separated by a space. The buffer of the original holds 100 characters without a length guard; the
/// reimplementation has no limit.
pub fn chord_display(text: &str, texts: &Texts) -> String {
    let names: Vec<String> = parse_chord(text)
        .into_iter()
        .map(|vk| {
            let id = KEY_TEXT_BASE + i32::from(vk);
            texts.resolve(id);
            texts.display(id)
        })
        .collect();
    names.join(" ")
}

/// Equivalence of two codes in `FUN_0049BC80`.
fn same_key(a: u8, b: u8) -> bool {
    a == b
        || MODIFIERS.iter().any(|&(general, sides)| {
            (a == general && sides.contains(&b)) || (b == general && sides.contains(&a))
        })
}

/// Comparison of two combinations like `FUN_0049BC80` (`this` = `event`, the pressed keys; parameter =
/// `pattern`, the binding): true if every key from `event` has an equivalent partner in `pattern`
/// (`event` is a subset of `pattern`). Equivalent are equal codes and a general modifier key with its left or
/// right form in both directions (`SHIFT` 0x10 = 0xA0/0xA1, `CONTROL` 0x11 = 0xA2/0xA3, `ALT` 0x12 = 0xA4/0xA5).
/// If one of the lists is empty, the result is false.
pub fn chord_matches(event: &[u8], pattern: &[u8]) -> bool {
    if event.is_empty() || pattern.is_empty() {
        return false;
    }
    event
        .iter()
        .all(|&a| pattern.iter().any(|&b| same_key(a, b)))
}

/// Codes of a combination name from [`chord_name`]: each word separated by a space is looked up exactly in the
/// table (without uppercasing, unlike [`parse_chord`]; the names are unique and without spaces,
/// so `0xNN` also come back). An unknown word yields 0, an empty name no codes. The reimplementation receives
/// the untaken keys as names (`MenuApp::take_untaken_keys`); the original compares bindings and
/// `KD_HARDCOPY` with the code list of the event, which is restored this way (the order does not matter there).
pub fn chord_codes(name: &str) -> Vec<u8> {
    name.split(' ')
        .filter(|w| !w.is_empty())
        .map(|w| (0..=255u8).find(|&vk| vk_name(vk) == w).unwrap_or(0))
        .collect()
}

/// General code of a left/right modifier key, otherwise the code itself (`FUN_0049BBC0`).
fn general_code(vk: u8) -> u8 {
    MODIFIERS
        .iter()
        .find(|(_, sides)| sides.contains(&vk))
        .map_or(vk, |&(general, _)| general)
}

/// Does the event `event` (held keys) hit the binding `binding` from `bindings2.ini`? Like `0x0040FC50`:
/// same sum of the codes, left/right counted as the general code (hash `FUN_0049BBC0`), and every key of the
/// binding has an equivalent partner in the event (`FUN_0049BF40` -> [`chord_matches`] with the binding as the
/// first list). With the same sum this practically means **exactly this set of keys**: a third key or a
/// held mouse button (`LBUTTON`, O-116) misses every binding (O-301). Empty lists never match.
pub fn binding_matches(event: &[u8], binding: &[u8]) -> bool {
    let sum = |codes: &[u8]| -> u32 { codes.iter().map(|&vk| u32::from(general_code(vk))).sum() };
    sum(event) == sum(binding) && chord_matches(binding, event)
}

/// Is the `KD_` key with the chord `chord` (from `keymap.ini`) pressed in the table of held keys `held`?
/// Like `FUN_004143A0` -> `FUN_0049BF80`: **every** key of the chord is held; for the left/right
/// form of a modifier key (`LSHIFT`, `RCONTROL`, `LALT` ...) the general code is enough (the table knows only
/// that one, `WM_KEYDOWN`), so `LSHIFT` works with both Shift keys. Further held keys do not interfere.
/// An empty chord is never pressed.
pub fn keydef_down(held: &[u8], chord: &[u8]) -> bool {
    !chord.is_empty()
        && chord
            .iter()
            .all(|&vk| held.contains(&vk) || held.contains(&general_code(vk)))
}

/// Keyboard context for `keys.ini` (`Keyboard+0x90/+0x94/+0x98`, re/game-ui D5.3): the three values of the filter
/// stages. The value 0 of a stage means "only the 0 rows", not a wildcard (`0x0049D950`, `0x0049D750`, `0x0049CF60`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyContext {
    /// Type (`0x0049D950`, from `FUN_0044B110`): 1 nothing selected, 2 unit, 3 building.
    pub kind: i32,
    /// Nation of the local player + 1 (`0x0049D750`; 1 WID ... 6 PAT).
    pub nation: i32,
    /// Selection GUID (`0x0049CF60`): the GUID of the most recently built button set (`Toolbar_BuildButtonSet`).
    pub selection: i32,
}

impl KeyContext {
    /// Type "nothing selected" (`FUN_0044B110(0)`).
    pub const KIND_NOTHING: i32 = 1;
    /// Type "unit" (`FUN_0044B110` without module 3).
    pub const KIND_UNIT: i32 = 2;
    /// Type "building" (`FUN_0044B110` with module 3).
    pub const KIND_BUILDING: i32 = 3;
}

impl Default for KeyContext {
    /// Nothing selected, nation 0, selection 0 (default without a match and before the first button set is built).
    fn default() -> Self {
        Self {
            kind: Self::KIND_NOTHING,
            nation: 0,
            selection: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_256_distinct_names_and_hex_for_the_rest() {
        let named = (0..=255u8)
            .filter(|&vk| !vk_name(vk).starts_with("0x"))
            .count();
        assert_eq!(named, 141);
        let mut all: Vec<&str> = (0..=255u8).map(vk_name).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 256);
        for vk in 0..=255u8 {
            let name = vk_name(vk);
            assert!(!name.is_empty());
            if name.starts_with("0x") {
                assert_eq!(name, format!("0x{vk:02X}"));
            }
        }
        assert_eq!(vk_name(0x1B), "ESCAPE");
        assert_eq!(vk_name(0x6A), "MULTIPLAY");
        assert_eq!(vk_name(0x6F), "DEVIDE");
        assert_eq!(vk_name(0xDB), "\u{df}");
        assert_eq!(vk_name(0xDC), "GRAVE");
        assert_eq!(vk_name(0xFE), "OEM_CLEAR");
    }

    #[test]
    fn chord_names_follow_the_bucket_order() {
        assert_eq!(chord_name(&[0x48, 0x11]), "CONTROL H");
        assert_eq!(chord_name(&[0x10, 0x4E]), "SHIFT N");
        assert_eq!(chord_name(&[0x1B]), "ESCAPE");
        // In the same bucket (0x4...) the most recently pressed first; bucket 0 before bucket 1.
        assert_eq!(chord_name(&[0x41, 0x42, 0x10, 0x0D]), "RETURN SHIFT B A");
        assert_eq!(chord_name(&[0xC0]), "0xC0");
        assert_eq!(chord_name(&[]), "");
    }

    // QUIRK O-107: kept
    #[test]
    fn parse_chord_uppercases_and_splits() {
        assert_eq!(parse_chord("left"), [0x25]);
        assert_eq!(parse_chord("CONTROL H"), [0x11, 0x48]);
        assert_eq!(parse_chord("FOO"), [0]);
        // Empty text: one empty word that matches nowhere.
        assert_eq!(parse_chord(""), [0]);
        assert_eq!(parse_chord("   "), [0]);
        // Trimming and multiple spaces; order stays, duplicates stay.
        assert_eq!(parse_chord("  alt   n  "), [0x12, 0x4E]);
        assert_eq!(parse_chord("A A"), [0x41, 0x41]);
        // Tabs do not separate.
        assert_eq!(parse_chord("ALT\tN"), [0]);
        // Punctuation and ß (VK 0xDB) match; the 0xNN names become 0XNN in uppercase and never match (O-107).
        assert_eq!(parse_chord("+ , - . #"), [0xBB, 0xBC, 0xBD, 0xBE, 0xBF]);
        assert_eq!(parse_chord("\u{df}"), [0xDB]);
        assert_eq!(parse_chord("0x05"), [0]);
        assert_eq!(parse_chord(&chord_name(&[0xC0])), [0]);
        // The named keys come back through their name.
        for vk in 0..=255u8 {
            if !vk_name(vk).starts_with("0x") {
                assert_eq!(parse_chord(vk_name(vk)), [vk], "{}", vk_name(vk));
            }
        }
    }

    #[test]
    fn chord_matches_treats_left_and_right_modifiers_alike() {
        let shift_left = parse_chord("SHIFT LEFT");
        let left = parse_chord("LEFT");
        assert!(chord_matches(&[0xA0, 0x25], &shift_left));
        assert!(!chord_matches(&[0xA0, 0x25], &left));
        // In both directions: general key in the event, specific key in the binding.
        assert!(chord_matches(&[0x11, 0x48], &[0xA3, 0x48]));
        assert!(chord_matches(&[0xA5, 0x4E], &[0x12, 0x4E]));
        // Left and right forms are not equivalent to each other.
        assert!(!chord_matches(&[0xA0], &[0xA1]));
        // Subset, not equality (`FUN_0049BC80` alone).
        assert!(chord_matches(&[0x25], &shift_left));
        // Empty lists: false.
        assert!(!chord_matches(&[], &left));
        assert!(!chord_matches(&left, &[]));
    }

    // QUIRK O-301: kept
    #[test]
    fn binding_needs_exactly_the_key_set() {
        // `0x0040FC50`: same sum (left/right counted as general) and every key of the binding in the event.
        let control_1 = parse_chord("CONTROL 1");
        assert_eq!(control_1, [0x11, 0x31]);
        assert!(binding_matches(&[0x11, 0x31], &control_1));
        assert!(binding_matches(&[0x31, 0x11], &control_1));
        assert!(binding_matches(&[0xA2, 0x31], &control_1));
        // A third key or a held mouse button changes the sum (O-301).
        assert!(!binding_matches(&[0x11, 0x10, 0x31], &control_1));
        assert!(!binding_matches(&[0x01, 0x11, 0x31], &control_1));
        // A subset is not enough.
        assert!(!binding_matches(&[0x31], &control_1));
        assert!(binding_matches(&[0x31], &parse_chord("1")));
        assert!(!binding_matches(&[0x11, 0x31], &parse_chord("1")));
        // Empty lists: false.
        assert!(!binding_matches(&[], &control_1));
        assert!(!binding_matches(&control_1, &[]));
        // Unknown keys count with their code (the event arrives as a code list, not as a name).
        assert!(!binding_matches(&[0xC0, 0x31], &parse_chord("1")));
    }

    #[test]
    fn keydef_down_accepts_extra_keys_and_both_shifts() {
        // `FUN_004143A0` -> `FUN_0049BF80`: every key of the chord held, further keys do not interfere.
        let lshift = parse_chord("LSHIFT");
        assert_eq!(lshift, [0xA0]);
        // The table knows only the general code 0x10: both Shift keys work.
        assert!(keydef_down(&[0x10], &lshift));
        assert!(keydef_down(&[0x41, 0x10, 0x01], &lshift));
        assert!(keydef_down(&[0x11], &parse_chord("RCONTROL")));
        assert!(keydef_down(&[0x12], &parse_chord("RALT")));
        assert!(!keydef_down(&[0x11], &lshift));
        let snapshot = parse_chord("SNAPSHOT");
        assert!(keydef_down(&[0x01, 0x2C], &snapshot));
        assert!(!keydef_down(&[0x01], &snapshot));
        let up_left = parse_chord("UP LEFT");
        assert!(keydef_down(&[0x25, 0x26, 0x10], &up_left));
        assert!(!keydef_down(&[0x25], &up_left));
        // Empty chord: false.
        assert!(!keydef_down(&[0x10], &[]));
    }

    #[test]
    fn chord_codes_invert_chord_name() {
        // Every name in the table is unique and without spaces: the name of a combination returns its codes,
        // including the `0xNN` names (unlike `parse_chord`, which uppercases, O-107).
        assert_eq!(chord_codes("CONTROL H"), [0x11, 0x48]);
        assert_eq!(chord_codes("LBUTTON F1"), [0x01, 0x70]);
        assert_eq!(chord_codes("0xC0 1"), [0xC0, 0x31]);
        assert_eq!(chord_codes(""), Vec::<u8>::new());
        for vk in 0..=255u8 {
            assert_eq!(chord_codes(vk_name(vk)), [vk]);
        }
        let held = [0x41, 0x42, 0x10, 0x0D, 0x01];
        let mut back = chord_codes(&chord_name(&held));
        back.sort_unstable();
        let mut want = held.to_vec();
        want.sort_unstable();
        assert_eq!(back, want);
    }
}
