//! Input from two sources (the user via winit, the control channel): shared events, separately
//! tracked held keys and the key names of the control channel.

use std::collections::HashSet;

use nml_control::KeyAction;
use winit::keyboard::KeyCode;

/// Origin of an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    /// Keyboard and mouse of the user (winit events).
    User,
    /// Synthetic input via the control channel.
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    /// Middle button: only the table of held keys (VK 4 `MBUTTON`, `KD_CAM_FREEHAND`); the UI does not evaluate
    /// its raw events `0x2A`...`0x2C` (re/renderer, section "Movement in the game", quirk O-331).
    Middle,
}

/// An input event; both sources run through the same handler (`App::handle_input`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// Key pressed (`repeat`: automatic repeat) or released.
    Key {
        code: KeyCode,
        pressed: bool,
        repeat: bool,
    },
    /// Mouse pointer in window pixels.
    CursorMoved {
        x: f64,
        y: f64,
    },
    CursorLeft,
    Button {
        button: MouseButton,
        pressed: bool,
    },
    /// One notch of the mouse wheel.
    Wheel {
        up: bool,
    },
}

/// Held keys separated by source; the effective set (camera control, Shift in the browser) is
/// the union. This way a focus loss leaves the keys of the control channel in place.
#[derive(Debug, Clone, Default)]
pub struct HeldKeys {
    user: HashSet<KeyCode>,
    control: HashSet<KeyCode>,
}

impl HeldKeys {
    fn of(&mut self, source: InputSource) -> &mut HashSet<KeyCode> {
        match source {
            InputSource::User => &mut self.user,
            InputSource::Control => &mut self.control,
        }
    }

    pub fn press(&mut self, source: InputSource, code: KeyCode) {
        self.of(source).insert(code);
    }

    pub fn release(&mut self, source: InputSource, code: KeyCode) {
        self.of(source).remove(&code);
    }

    /// Focus loss: release only the keys of the user (winit does not report their release then).
    pub fn clear_user(&mut self) {
        self.user.clear();
    }

    /// Held in one of the two sources.
    pub fn contains(&self, code: KeyCode) -> bool {
        self.user.contains(&code) || self.control.contains(&code)
    }

    /// Effective keys: union of both sources.
    pub fn effective(&self) -> HashSet<KeyCode> {
        self.user.union(&self.control).copied().collect()
    }

    /// Names of the held keys of one source, sorted (state snapshot).
    pub fn names(&self, source: InputSource) -> Vec<String> {
        let keys = match source {
            InputSource::User => &self.user,
            InputSource::Control => &self.control,
        };
        let mut names: Vec<String> = keys.iter().map(|k| format!("{k:?}")).collect();
        names.sort();
        names
    }
}

/// `KeyCode` for a key name of the control channel (`nml_control::KEY_NAMES`).
pub fn key_code(name: &str) -> Option<KeyCode> {
    Some(match name {
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "Insert" => KeyCode::Insert,
        "Delete" => KeyCode::Delete,
        "KeyW" => KeyCode::KeyW,
        "KeyA" => KeyCode::KeyA,
        "KeyS" => KeyCode::KeyS,
        "KeyD" => KeyCode::KeyD,
        "KeyQ" => KeyCode::KeyQ,
        "KeyE" => KeyCode::KeyE,
        "KeyB" => KeyCode::KeyB,
        "KeyN" => KeyCode::KeyN,
        "KeyV" => KeyCode::KeyV,
        "KeyT" => KeyCode::KeyT,
        "KeyP" => KeyCode::KeyP,
        "KeyH" => KeyCode::KeyH,
        "KeyI" => KeyCode::KeyI,
        "KeyG" => KeyCode::KeyG,
        "KeyR" => KeyCode::KeyR,
        "KeyF" => KeyCode::KeyF,
        "KeyO" => KeyCode::KeyO,
        "KeyX" => KeyCode::KeyX,
        "KeyK" => KeyCode::KeyK,
        "KeyL" => KeyCode::KeyL,
        "KeyJ" => KeyCode::KeyJ,
        "KeyU" => KeyCode::KeyU,
        "Digit0" => KeyCode::Digit0,
        "Digit1" => KeyCode::Digit1,
        "Digit2" => KeyCode::Digit2,
        "Digit3" => KeyCode::Digit3,
        "Digit4" => KeyCode::Digit4,
        "Digit5" => KeyCode::Digit5,
        "Digit6" => KeyCode::Digit6,
        "Digit7" => KeyCode::Digit7,
        "Digit8" => KeyCode::Digit8,
        "Digit9" => KeyCode::Digit9,
        "Minus" => KeyCode::Minus,
        "Equal" => KeyCode::Equal,
        "BracketRight" => KeyCode::BracketRight,
        "Slash" => KeyCode::Slash,
        "Period" => KeyCode::Period,
        "Comma" => KeyCode::Comma,
        "NumpadAdd" => KeyCode::NumpadAdd,
        "NumpadSubtract" => KeyCode::NumpadSubtract,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "Tab" => KeyCode::Tab,
        "Space" => KeyCode::Space,
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Escape,
        "Pause" => KeyCode::Pause,
        "PrintScreen" => KeyCode::PrintScreen,
        "F1" => KeyCode::F1,
        "F2" => KeyCode::F2,
        "F3" => KeyCode::F3,
        "F4" => KeyCode::F4,
        "F5" => KeyCode::F5,
        "F6" => KeyCode::F6,
        "F7" => KeyCode::F7,
        "F8" => KeyCode::F8,
        "F9" => KeyCode::F9,
        "F10" => KeyCode::F10,
        "F11" => KeyCode::F11,
        "F12" => KeyCode::F12,
        _ => return None,
    })
}

/// Direction of the "smaller/larger" keys (burn intensity, `.hef` radius): -1, +1 or `None`. The
/// viewer reads physical keys; on the German keyboard - and + are on `Slash` and `BracketRight`,
/// while `Minus` (US position, name in the control channel) is the sharp-s key there and `Equal` is
/// the acute-accent key. The numpad (Num -/+) works in every layout.
pub fn step_sign(code: KeyCode) -> Option<i32> {
    match code {
        KeyCode::Minus | KeyCode::Slash | KeyCode::NumpadSubtract => Some(-1),
        KeyCode::Equal | KeyCode::BracketRight | KeyCode::NumpadAdd => Some(1),
        _ => None,
    }
}

/// Events of a key command: `down` presses, `up` releases, `tap` presses and releases immediately (acts exactly
/// once like a key press); none of them is a repeat. With `repeat` (only for `down`, `Command::validate`
/// checks this) the press is followed by `repeat` further press events with `repeat = true`, like the automatic
/// repeat of the system (each repeat is an event of its own, as in the original, O-300).
pub fn key_events(code: KeyCode, action: KeyAction, repeat: u16) -> Vec<InputEvent> {
    let event = |pressed, repeat| InputEvent::Key {
        code,
        pressed,
        repeat,
    };
    match action {
        KeyAction::Down => std::iter::once(event(true, false))
            .chain((0..repeat).map(|_| event(true, true)))
            .collect(),
        KeyAction::Up => vec![event(false, false)],
        KeyAction::Tap => vec![event(true, false), event(false, false)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_key_name_maps_to_its_key_code() {
        for name in nml_control::KEY_NAMES {
            let code = key_code(name).unwrap_or_else(|| panic!("{name} has no KeyCode"));
            assert_eq!(format!("{code:?}"), *name);
        }
        // Protocol 9: rotating in placement mode and Ctrl (queue cyclically).
        assert_eq!(key_code("KeyR"), Some(KeyCode::KeyR));
        assert_eq!(key_code("ControlLeft"), Some(KeyCode::ControlLeft));
        // Protocol 10: fog fade-in and "fog off".
        assert_eq!(key_code("KeyF"), Some(KeyCode::KeyF));
        assert_eq!(key_code("KeyO"), Some(KeyCode::KeyO));
        // Stance (Shift + digit 1...5) and attack mode (X).
        assert_eq!(key_code("KeyX"), Some(KeyCode::KeyX));
        assert_eq!(key_code("Digit1"), Some(KeyCode::Digit1));
        assert_eq!(key_code("Digit5"), Some(KeyCode::Digit5));
        // Window "AI" (K) and AI overlay (L).
        assert_eq!(key_code("KeyK"), Some(KeyCode::KeyK));
        assert_eq!(key_code("KeyL"), Some(KeyCode::KeyL));
        // Window "Script" (J).
        assert_eq!(key_code("KeyJ"), Some(KeyCode::KeyJ));
        assert_eq!(key_code("Numpad1"), None);
        assert_eq!(key_code("F13"), None);
        assert_eq!(key_code("arrowup"), None);
    }

    /// The keys of the original are in all three lists - control channel (`nml_control::KEY_NAMES`), implementation
    /// here ([`key_code`]) and MCP (`viewer_input`, generated from `KEY_NAMES`, test `viewer_input_offers_repeat`) -
    /// and yield the VK of the German layout (`vk_of`): main-block `+` is `BracketRight`, `-` is `Slash`.
    #[test]
    fn new_key_names_are_in_all_three_lists() {
        use crate::vk::vk_of;
        const NEW: [(&str, u8); 23] = [
            ("Digit0", 0x30),
            ("Digit6", 0x36),
            ("Digit7", 0x37),
            ("Digit8", 0x38),
            ("Digit9", 0x39),
            ("BracketRight", 0xBB),
            ("Slash", 0xBD),
            ("Period", 0xBE),
            ("Comma", 0xBC),
            ("NumpadAdd", 0x6B),
            ("NumpadSubtract", 0x6D),
            ("Pause", 0x13),
            ("PrintScreen", 0x2C),
            ("Home", 0x24),
            ("End", 0x23),
            ("Delete", 0x2E),
            ("Insert", 0x2D),
            ("Tab", 0x09),
            ("ControlLeft", 0x11),
            ("KeyQ", 0x51),
            ("KeyH", 0x48),
            ("KeyD", 0x44),
            ("KeyU", 0x55),
        ];
        for (name, vk) in NEW {
            assert!(nml_control::KEY_NAMES.contains(&name), "{name}");
            let code = key_code(name).unwrap_or_else(|| panic!("{name} has no KeyCode"));
            assert_eq!(vk_of(code), Some(vk), "{name}");
        }
        // `Equal` and `Minus` still exist (US position in the viewer); under the German layout they are the
        // acute-accent and sharp-s keys.
        assert_eq!(key_code("Equal").and_then(vk_of), Some(0xDD));
        assert_eq!(key_code("Minus").and_then(vk_of), Some(0xDB));
        assert_eq!(key_code("KeyP").and_then(vk_of), Some(0x50));
    }

    /// `key` with `repeat` presses once and then sends `repeat` repeats (O-300); without `repeat` as before.
    #[test]
    fn key_repeat_sends_extra_presses_marked_as_repeat() {
        let press = |repeat| InputEvent::Key {
            code: KeyCode::BracketRight,
            pressed: true,
            repeat,
        };
        assert_eq!(
            key_events(KeyCode::BracketRight, KeyAction::Down, 3),
            [press(false), press(true), press(true), press(true)]
        );
        assert_eq!(
            key_events(KeyCode::BracketRight, KeyAction::Down, 0),
            [press(false)]
        );
        assert_eq!(
            key_events(KeyCode::BracketRight, KeyAction::Tap, 0).len(),
            2
        );
    }

    #[test]
    fn minus_and_plus_work_on_us_and_german_layouts_and_the_numpad() {
        for code in [KeyCode::Minus, KeyCode::Slash, KeyCode::NumpadSubtract] {
            assert_eq!(step_sign(code), Some(-1), "{code:?}");
        }
        for code in [KeyCode::Equal, KeyCode::BracketRight, KeyCode::NumpadAdd] {
            assert_eq!(step_sign(code), Some(1), "{code:?}");
        }
        assert_eq!(step_sign(KeyCode::Space), None);
        assert_eq!(step_sign(KeyCode::ArrowLeft), None);
    }

    #[test]
    fn held_keys_are_the_union_of_both_sources() {
        let mut keys = HeldKeys::default();
        keys.press(InputSource::User, KeyCode::ArrowUp);
        keys.press(InputSource::Control, KeyCode::ShiftLeft);
        keys.press(InputSource::Control, KeyCode::ArrowUp);
        assert_eq!(
            keys.effective(),
            [KeyCode::ArrowUp, KeyCode::ShiftLeft].into()
        );
        // Release in one source: still held in the other.
        keys.release(InputSource::User, KeyCode::ArrowUp);
        assert!(keys.contains(KeyCode::ArrowUp));
        keys.release(InputSource::Control, KeyCode::ArrowUp);
        assert!(!keys.contains(KeyCode::ArrowUp));
        assert_eq!(keys.names(InputSource::Control), ["ShiftLeft"]);
        assert!(keys.names(InputSource::User).is_empty());
    }

    #[test]
    fn focus_loss_releases_only_user_keys() {
        let mut keys = HeldKeys::default();
        keys.press(InputSource::User, KeyCode::KeyW);
        keys.press(InputSource::Control, KeyCode::KeyD);
        keys.clear_user();
        assert_eq!(keys.effective(), [KeyCode::KeyD].into());
    }

    #[test]
    fn tap_presses_once_without_repeat_and_leaves_nothing_held() {
        let events = key_events(KeyCode::F3, KeyAction::Tap, 0);
        assert_eq!(
            events,
            [
                InputEvent::Key {
                    code: KeyCode::F3,
                    pressed: true,
                    repeat: false
                },
                InputEvent::Key {
                    code: KeyCode::F3,
                    pressed: false,
                    repeat: false
                },
            ]
        );
        let mut keys = HeldKeys::default();
        let mut presses = 0;
        for event in events {
            if let InputEvent::Key { code, pressed, .. } = event {
                if pressed {
                    keys.press(InputSource::Control, code);
                    presses += 1;
                } else {
                    keys.release(InputSource::Control, code);
                }
            }
        }
        assert_eq!(presses, 1);
        assert!(keys.effective().is_empty());
        assert_eq!(key_events(KeyCode::KeyW, KeyAction::Down, 0).len(), 1);
        assert_eq!(
            key_events(KeyCode::KeyW, KeyAction::Up, 0),
            [InputEvent::Key {
                code: KeyCode::KeyW,
                pressed: false,
                repeat: false
            }]
        );
    }
}
