//! Key names of the control channel in the spelling of the winit `KeyCode` variants. The conversion
//! to `KeyCode` lives in the shared client (`crates/nml-client/src/keys.rs`).

/// All keys that the viewer evaluates (map, free camera, model browser, F keys; plus `KeyK`
/// window "AI" and `KeyL` AI overlay; `KeyJ` window "Script"), plus the keys of the original
/// from `bindings2.ini`, `keys.ini` and `keymap.ini` (digits 0-9, `BracketRight`/`Slash` = main keyboard `+`/`-`
/// of the German layout, numpad `+`/`-`, `Period`, `Comma`, `Tab`, `Pause`, `PrintScreen`, `KeyU`).
pub const KEY_NAMES: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
    "Insert",
    "Delete",
    "KeyW",
    "KeyA",
    "KeyS",
    "KeyD",
    "KeyQ",
    "KeyE",
    "KeyB",
    "KeyN",
    "KeyV",
    "KeyT",
    "KeyP",
    "KeyH",
    "KeyI",
    "KeyG",
    "KeyR",
    "KeyF",
    "KeyO",
    "KeyX",
    "KeyK",
    "KeyL",
    "KeyJ",
    "KeyU",
    "Digit0",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Digit6",
    "Digit7",
    "Digit8",
    "Digit9",
    "Minus",
    "Equal",
    "BracketRight",
    "Slash",
    "Period",
    "Comma",
    "NumpadAdd",
    "NumpadSubtract",
    "ShiftLeft",
    "ShiftRight",
    "ControlLeft",
    "Tab",
    "Space",
    "Enter",
    "Escape",
    "Pause",
    "PrintScreen",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
];

/// Unknown key name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown key \"{0}\" (allowed: {list})", list = KEY_NAMES.join(", "))]
pub struct UnknownKey(pub String);

/// Checks whether `name` is one of the [`KEY_NAMES`] (case matters).
pub fn check_key(name: &str) -> Result<(), UnknownKey> {
    if KEY_NAMES.contains(&name) {
        Ok(())
    } else {
        Err(UnknownKey(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keys_pass_and_unknown_keys_name_the_allowed_ones() {
        for name in KEY_NAMES {
            assert_eq!(check_key(name), Ok(()));
        }
        let error = check_key("arrowup").unwrap_err();
        assert_eq!(error, UnknownKey("arrowup".into()));
        let text = error.to_string();
        assert!(
            text.contains("\"arrowup\"") && text.contains("ArrowUp, ArrowDown"),
            "{text}"
        );
        assert!(check_key("").is_err());
        // Protocol 10: fog overlay and "Fog off".
        assert_eq!(check_key("KeyF"), Ok(()));
        assert_eq!(check_key("KeyO"), Ok(()));
        // Stance (Shift + digit 1...5) and attack mode (X).
        for name in ["KeyX", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5"] {
            assert_eq!(check_key(name), Ok(()), "{name}");
        }
        // All digits (groups).
        assert_eq!(check_key("Digit6"), Ok(()));
        assert_eq!(check_key("Digit0"), Ok(()));
        assert!(check_key("Numpad1").is_err());
        // Window "AI" (K) and AI overlay (L).
        assert_eq!(check_key("KeyK"), Ok(()));
        assert_eq!(check_key("KeyL"), Ok(()));
        // Window "Script" (J).
        assert_eq!(check_key("KeyJ"), Ok(()));
    }

    #[test]
    fn key_names_are_unique() {
        let mut names = KEY_NAMES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), KEY_NAMES.len());
    }
}
