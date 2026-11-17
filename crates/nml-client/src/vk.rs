//! Keys of the menu UI as in the original (re/menu-system 12.7): virtual key codes (VK) from the physical keys of
//! winit, the table of held keys in press order and the character as cp1252.
//!
//! The original gets the VK from `WM_KEYDOWN`/`WM_SYSKEYDOWN` (`FUN_0042C1D0`). Shift, Ctrl and Alt report the general
//! codes `0x10`/`0x11`/`0x12` there (not the left/right form); this is how names like `SHIFT N` arise (chord name
//! `FUN_0049C340`, `nml_ui::keys::chord_name`). winit delivers the physical key in US naming; [`vk_of`] returns the VK
//! that Windows reports for this key with a **German** keyboard layout - the game is German and its name table fits
//! that (`0xBB` "+", `0xBF` "#", `0xDB` sharp s, `0xDC` "GRAVE" = `^`).

use winit::keyboard::KeyCode;

/// VK of the physical key `code` under the German layout; `None` for keys without a VK in the original.
pub fn vk_of(code: KeyCode) -> Option<u8> {
    use KeyCode as K;
    Some(match code {
        K::Backspace => 0x08,
        K::Tab => 0x09,
        K::Enter | K::NumpadEnter => 0x0D,
        K::ShiftLeft | K::ShiftRight => 0x10,
        K::ControlLeft | K::ControlRight => 0x11,
        K::AltLeft | K::AltRight => 0x12,
        K::Pause => 0x13,
        K::CapsLock => 0x14,
        K::Escape => 0x1B,
        K::Space => 0x20,
        K::PageUp => 0x21,
        K::PageDown => 0x22,
        K::End => 0x23,
        K::Home => 0x24,
        K::ArrowLeft => 0x25,
        K::ArrowUp => 0x26,
        K::ArrowRight => 0x27,
        K::ArrowDown => 0x28,
        K::PrintScreen => 0x2C,
        K::Insert => 0x2D,
        K::Delete => 0x2E,
        K::Digit0 => 0x30,
        K::Digit1 => 0x31,
        K::Digit2 => 0x32,
        K::Digit3 => 0x33,
        K::Digit4 => 0x34,
        K::Digit5 => 0x35,
        K::Digit6 => 0x36,
        K::Digit7 => 0x37,
        K::Digit8 => 0x38,
        K::Digit9 => 0x39,
        K::KeyA => 0x41,
        K::KeyB => 0x42,
        K::KeyC => 0x43,
        K::KeyD => 0x44,
        K::KeyE => 0x45,
        K::KeyF => 0x46,
        K::KeyG => 0x47,
        K::KeyH => 0x48,
        K::KeyI => 0x49,
        K::KeyJ => 0x4A,
        K::KeyK => 0x4B,
        K::KeyL => 0x4C,
        K::KeyM => 0x4D,
        K::KeyN => 0x4E,
        K::KeyO => 0x4F,
        K::KeyP => 0x50,
        K::KeyQ => 0x51,
        K::KeyR => 0x52,
        K::KeyS => 0x53,
        K::KeyT => 0x54,
        K::KeyU => 0x55,
        K::KeyV => 0x56,
        K::KeyW => 0x57,
        K::KeyX => 0x58,
        // German layout: Y and Z swapped.
        K::KeyY => 0x5A,
        K::KeyZ => 0x59,
        K::SuperLeft => 0x5B,
        K::SuperRight => 0x5C,
        K::ContextMenu => 0x5D,
        K::Numpad0 => 0x60,
        K::Numpad1 => 0x61,
        K::Numpad2 => 0x62,
        K::Numpad3 => 0x63,
        K::Numpad4 => 0x64,
        K::Numpad5 => 0x65,
        K::Numpad6 => 0x66,
        K::Numpad7 => 0x67,
        K::Numpad8 => 0x68,
        K::Numpad9 => 0x69,
        K::NumpadMultiply => 0x6A,
        K::NumpadAdd => 0x6B,
        K::NumpadSubtract => 0x6D,
        K::NumpadDecimal => 0x6E,
        K::NumpadDivide => 0x6F,
        K::F1 => 0x70,
        K::F2 => 0x71,
        K::F3 => 0x72,
        K::F4 => 0x73,
        K::F5 => 0x74,
        K::F6 => 0x75,
        K::F7 => 0x76,
        K::F8 => 0x77,
        K::F9 => 0x78,
        K::F10 => 0x79,
        K::F11 => 0x7A,
        K::F12 => 0x7B,
        K::F13 => 0x7C,
        K::F14 => 0x7D,
        K::F15 => 0x7E,
        K::F16 => 0x7F,
        K::F17 => 0x80,
        K::F18 => 0x81,
        K::F19 => 0x82,
        K::F20 => 0x83,
        K::F21 => 0x84,
        K::F22 => 0x85,
        K::F23 => 0x86,
        K::F24 => 0x87,
        K::NumLock => 0x90,
        K::ScrollLock => 0x91,
        // Character keys in the German position (US name of the physical key -> German character).
        K::BracketLeft => 0xBA,  // u umlaut
        K::BracketRight => 0xBB, // +
        K::Comma => 0xBC,
        K::Slash => 0xBD, // -
        K::Period => 0xBE,
        K::Backslash => 0xBF, // #
        K::Semicolon => 0xC0, // o umlaut
        K::Minus => 0xDB,     // sharp s
        K::Backquote => 0xDC, // ^
        K::Equal => 0xDD,     // acute accent
        K::Quote => 0xDE,     // a umlaut
        K::IntlBackslash => 0xE2,
        _ => return None,
    })
}

/// Character of a key press (winit `KeyEvent::text`) as a cp1252 unit, as the EditField takes it (re/menu-system
/// 12.6/12.7): the first character of the text, if it is representable in Windows-1252; otherwise no character.
/// Control characters stay (the EditField discards them itself).
///
/// Deviation: winit composes dead keys with the following character (`^` + `a` -> `a` with a circumflex). The original
/// does not translate `WM_KEYDOWN` with VK `0xDC` (`FUN_0042C1D0` then does not call `TranslateMessage`), `^` is not a
/// dead key there (re/menu-system 12.7).
pub fn char_of(text: Option<&str>) -> Option<u16> {
    let ch = text?.chars().next()?;
    let mut buf = [0u8; 4];
    let (bytes, _, unmappable) = encoding_rs::WINDOWS_1252.encode(ch.encode_utf8(&mut buf));
    match (unmappable, bytes.as_ref()) {
        (false, [b]) => Some(u16::from(*b)),
        _ => None,
    }
}

/// Mouse bits of the original (`+0x58` of the input source): left, middle, right.
pub const MOUSE_LEFT: u8 = 1;
pub const MOUSE_MIDDLE: u8 = 2;
pub const MOUSE_RIGHT: u8 = 4;

/// Held keys (table `0x0066FA60+0x9C`, a `CMap` VK -> time) in insertion order, each once, together with what the
/// original queries for comparison: the physically held keys (`GetAsyncKeyState`) and the mouse bits.
///
/// - Press (`WM_KEYDOWN`/`WM_SYSKEYDOWN` -> `FUN_00414B70`): enter the VK (`CMap::operator[]`; a key that is already
///   entered - autorepeat - keeps its place).
/// - Release takes effect only at the start of the next round of the window loop ([`Self::begin_round`],
///   `FUN_00414830` from `FUN_0042C1D0`): every key that `GetAsyncKeyState(vk) & 0xFFFE` no longer reports drops out.
///   For the general modifier codes `0x10`/`0x11`/`0x12` Windows reports each of the two keys; the VK therefore stays
///   until no matching physical key is held any more.
/// - Mouse buttons are in the table as well (O-116): at the start of the round VK 1/4/2 drop out if their bit is
///   missing, and come back in (left, middle, right in this order) when the bit is set. Bug in the original: VK 2
///   (right) is checked against bit 0 (**left**) (`0x004148B8`) - with the left button held, VK 2 stays after releasing
///   right, without it VK 2 drops out every round and comes back as the newest entry while the right button is held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldVk {
    held: Vec<u8>,
    /// Physically held keys with a VK.
    down: Vec<KeyCode>,
    /// Mouse bits ([`MOUSE_LEFT`], [`MOUSE_MIDDLE`], [`MOUSE_RIGHT`]).
    mouse: u8,
}

impl HeldVk {
    /// Key pressed: physically held, VK into the table (if new). Returns the VK (`None`: key without a VK).
    pub fn press(&mut self, code: KeyCode) -> Option<u8> {
        let vk = vk_of(code)?;
        if !self.down.contains(&code) {
            self.down.push(code);
        }
        if !self.held.contains(&vk) {
            self.held.push(vk);
        }
        Some(vk)
    }

    /// Is the key `code` physically held (since [`Self::press`], until [`Self::release`] or [`Self::clear`])?
    pub fn is_down(&self, code: KeyCode) -> bool {
        self.down.contains(&code)
    }

    /// Key released: only the physical state; the table follows in [`Self::begin_round`].
    pub fn release(&mut self, code: KeyCode) {
        self.down.retain(|&c| c != code);
    }

    /// Mouse button (`bit` = [`MOUSE_LEFT`], [`MOUSE_MIDDLE`] or [`MOUSE_RIGHT`]) pressed or released.
    pub fn set_mouse(&mut self, bit: u8, down: bool) {
        if down {
            self.mouse |= bit;
        } else {
            self.mouse &= !bit;
        }
    }

    /// Start of a round of the window loop (`FUN_00414830`, `0x00414830`...`0x0041495B`), see the type docs.
    pub fn begin_round(&mut self) {
        let mouse = self.mouse;
        let down = &self.down;
        let (mut left, mut middle, mut right) = (false, false, false);
        self.held.retain(|&vk| match vk {
            1 => {
                left = mouse & MOUSE_LEFT != 0;
                left
            }
            // Bug in the original: bit 0 (left) instead of bit 2 (`0x004148B8`).
            2 => {
                right = mouse & MOUSE_LEFT != 0;
                right
            }
            4 => {
                middle = mouse & MOUSE_MIDDLE != 0;
                middle
            }
            vk => down.iter().any(|&c| vk_of(c) == Some(vk)),
        });
        for (present, bit, vk) in [
            (left, MOUSE_LEFT, 1),
            (middle, MOUSE_MIDDLE, 4),
            (right, MOUSE_RIGHT, 2),
        ] {
            if !present && mouse & bit != 0 {
                self.held.push(vk);
            }
        }
    }

    /// Release everything (focus loss: winit does not report the release then).
    pub fn clear(&mut self) {
        self.held.clear();
        self.down.clear();
        self.mouse = 0;
    }

    /// The table in insertion order (input of `nml_ui::input::UiInput::Key`).
    pub fn held(&self) -> Vec<u8> {
        self.held.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_ui::keys::{chord_matches, chord_name, parse_chord, vk_name};

    #[test]
    fn vk_table_covers_escape_return_arrows_letters_digits() {
        let names = |codes: &[KeyCode]| -> Vec<&str> {
            codes
                .iter()
                .map(|&c| vk_name(vk_of(c).unwrap_or_else(|| panic!("{c:?} without VK"))))
                .collect()
        };
        assert_eq!(
            names(&[
                KeyCode::Escape,
                KeyCode::Enter,
                KeyCode::NumpadEnter,
                KeyCode::Backspace,
                KeyCode::Tab,
                KeyCode::Space,
                KeyCode::Delete,
            ]),
            [
                "ESCAPE", "RETURN", "RETURN", "BACK", "TAB", "SPACE", "DELETE"
            ]
        );
        assert_eq!(
            names(&[
                KeyCode::ArrowLeft,
                KeyCode::ArrowUp,
                KeyCode::ArrowRight,
                KeyCode::ArrowDown
            ]),
            ["LEFT", "UP", "RIGHT", "DOWN"]
        );
        // All letters and digits; Y/Z in the German position.
        let letters = [
            KeyCode::KeyA,
            KeyCode::KeyB,
            KeyCode::KeyC,
            KeyCode::KeyD,
            KeyCode::KeyE,
            KeyCode::KeyF,
            KeyCode::KeyG,
            KeyCode::KeyH,
            KeyCode::KeyI,
            KeyCode::KeyJ,
            KeyCode::KeyK,
            KeyCode::KeyL,
            KeyCode::KeyM,
            KeyCode::KeyN,
            KeyCode::KeyO,
            KeyCode::KeyP,
            KeyCode::KeyQ,
            KeyCode::KeyR,
            KeyCode::KeyS,
            KeyCode::KeyT,
            KeyCode::KeyU,
            KeyCode::KeyV,
            KeyCode::KeyW,
            KeyCode::KeyX,
            KeyCode::KeyY,
            KeyCode::KeyZ,
        ];
        assert_eq!(names(&letters).concat(), "ABCDEFGHIJKLMNOPQRSTUVWXZY");
        let digits = [
            KeyCode::Digit0,
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        assert_eq!(names(&digits).concat(), "0123456789");
        // Modifier keys in the general form, F keys, numpad, German character keys.
        assert_eq!(
            names(&[
                KeyCode::ShiftLeft,
                KeyCode::ShiftRight,
                KeyCode::ControlRight,
                KeyCode::AltLeft,
                KeyCode::F1,
                KeyCode::F12,
                KeyCode::Numpad0,
                KeyCode::NumpadDivide,
                KeyCode::BracketRight,
                KeyCode::Minus,
                KeyCode::Backquote,
                KeyCode::Backslash,
            ]),
            [
                "SHIFT", "SHIFT", "CONTROL", "ALT", "F1", "F12", "NUMPAD0", "DEVIDE", "+",
                "\u{df}", "GRAVE", "#"
            ]
        );
        assert_eq!(vk_of(KeyCode::Fn), None);
        assert_eq!(vk_of(KeyCode::MediaPlayPause), None);
    }

    #[test]
    fn held_keys_build_original_chords() {
        let mut held = HeldVk::default();
        assert_eq!(held.press(KeyCode::ShiftLeft), Some(0x10));
        assert_eq!(held.press(KeyCode::KeyN), Some(0x4E));
        assert_eq!(held.held(), [0x10, 0x4E]);
        assert_eq!(chord_name(&held.held()), "SHIFT N");
        // Autorepeat changes nothing; add Ctrl: bucket 1 last pressed first.
        held.press(KeyCode::ShiftLeft);
        held.press(KeyCode::ControlLeft);
        assert_eq!(held.held(), [0x10, 0x4E, 0x11]);
        assert_eq!(chord_name(&held.held()), "CONTROL SHIFT N");
        // Release takes effect only at the start of the next round.
        held.release(KeyCode::ShiftLeft);
        assert_eq!(chord_name(&held.held()), "CONTROL SHIFT N");
        held.begin_round();
        assert_eq!(chord_name(&held.held()), "CONTROL N");
        assert_eq!(held.press(KeyCode::Fn), None);
        held.clear();
        assert!(held.held().is_empty());
        held.begin_round();
        assert!(held.held().is_empty());
    }

    // QUIRK O-116: kept
    #[test]
    fn left_and_right_modifiers_share_one_vk_until_both_are_up() {
        let mut held = HeldVk::default();
        held.press(KeyCode::ShiftLeft);
        held.press(KeyCode::ShiftRight);
        assert_eq!(held.held(), [0x10]);
        held.release(KeyCode::ShiftLeft);
        held.begin_round();
        assert_eq!(held.held(), [0x10], "right still held");
        held.release(KeyCode::ShiftRight);
        held.begin_round();
        assert!(held.held().is_empty());
    }

    // QUIRK O-116: kept
    #[test]
    fn mouse_buttons_join_the_table_at_round_start() {
        let escape = parse_chord("ESCAPE");
        let mut held = HeldVk::default();
        // Left pressed: only the next round enters VK 1; ESC is then called "LBUTTON ESCAPE" (no broadcast, O-14).
        held.set_mouse(MOUSE_LEFT, true);
        assert!(held.held().is_empty());
        held.begin_round();
        held.press(KeyCode::Escape);
        assert_eq!(held.held(), [1, 0x1B]);
        assert_eq!(chord_name(&held.held()), "LBUTTON ESCAPE");
        assert!(!chord_matches(&held.held(), &escape));
        held.release(KeyCode::Escape);
        held.set_mouse(MOUSE_LEFT, false);
        held.begin_round();
        assert!(held.held().is_empty());
        // Right alone: VK 2 drops out every round (check against bit 0) and comes back as the newest entry.
        held.set_mouse(MOUSE_RIGHT, true);
        held.begin_round();
        assert_eq!(held.held(), [2]);
        held.press(KeyCode::KeyA);
        assert_eq!(held.held(), [2, 0x41]);
        held.begin_round();
        assert_eq!(held.held(), [0x41, 2]);
        // All three: sequence left, middle, right; right and middle released with the left button held: VK 2 stays.
        held.clear();
        held.set_mouse(MOUSE_RIGHT, true);
        held.set_mouse(MOUSE_LEFT, true);
        held.set_mouse(MOUSE_MIDDLE, true);
        held.begin_round();
        assert_eq!(held.held(), [1, 4, 2]);
        assert_eq!(chord_name(&held.held()), "RBUTTON MBUTTON LBUTTON");
        held.set_mouse(MOUSE_RIGHT, false);
        held.set_mouse(MOUSE_MIDDLE, false);
        held.begin_round();
        assert_eq!(held.held(), [1, 2], "VK 2 hangs on the left button");
        held.set_mouse(MOUSE_LEFT, false);
        held.begin_round();
        assert!(held.held().is_empty());
    }

    #[test]
    fn characters_are_cp1252_or_nothing() {
        assert_eq!(char_of(Some("a")), Some(0x61));
        assert_eq!(char_of(Some("ä")), Some(0xE4));
        assert_eq!(char_of(Some("ß")), Some(0xDF));
        // Euro is at 0x80 in cp1252.
        assert_eq!(char_of(Some("€")), Some(0x80));
        assert_eq!(char_of(Some("\t")), Some(0x09));
        assert_eq!(char_of(Some("ab")), Some(0x61), "only the first character");
        assert_eq!(char_of(Some("я")), None, "not representable");
        assert_eq!(char_of(Some("")), None);
        assert_eq!(char_of(None), None);
    }
}
