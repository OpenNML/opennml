//! Helpers for tests with synthetic scripts (not part of the interface).

use nml_formats::config::lower_german;
use nml_formats::text::encode_cp1252;

use super::rules::{ScriptRules, SpeechLen, accept_script};

/// Path of the synthetic level script (matches `Rules::synthetic`, level path `synthetisch`).
pub const LEVEL_PATH: &str = "synthetisch.abl";

/// Script rules from texts with the same checks as `ScriptRules::load` (shared helpers
/// `accept_script` and `ScriptRules::load_dialogs`): the level script `level`, the dialogs `(name,
/// text)` under `<name>.abl` and the areas (key lowercase and trimmed). Errors (parser,
/// load check, action 39 without a dialog) end up in `errors`; a faulty script is not
/// loaded. Dialogs that the level script does not refer to are additionally loaded by the helper (for
/// tests that start them directly). Without speech lengths (`ScriptRules::speech` is public).
pub fn rules_with(
    level: &str,
    dialogs: &[(&str, &str)],
    areas: &[(&str, [i32; 4])],
) -> ScriptRules {
    let mut rules = ScriptRules::default();
    for (name, rect) in areas {
        rules.areas.insert(lower_german(name.trim()), *rect);
    }
    rules.level = accept_script(LEVEL_PATH, &encode_cp1252(level), false, &mut rules.errors);
    let source = |name: &str| {
        dialogs
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(n, text)| (format!("{n}.abl"), encode_cp1252(text)))
    };
    let tried = rules.load_dialogs(LEVEL_PATH, source);
    for (name, text) in dialogs {
        if tried.contains(*name) {
            continue;
        }
        let path = format!("{name}.abl");
        if let Some(script) = accept_script(&path, &encode_cp1252(text), true, &mut rules.errors) {
            rules.dialogs.insert((*name).to_owned(), script);
        }
    }
    rules.refresh_ui_flag();
    rules
}

/// Script rules with speech lengths `(text ID, OGG length in ms, UTF-16 units of the text)`.
pub fn with_speech(mut rules: ScriptRules, entries: &[(i32, u32, u32)]) -> ScriptRules {
    for &(id, ogg_ms, text_units) in entries {
        rules.speech.insert(id, SpeechLen { ogg_ms, text_units });
    }
    rules
}

/// Starts the dialog `name` like action 39 (D case `0x20`, [`super::actions::start_dialog`]) - for
/// the dry run of the dialog scripts. A running dialog is replaced; an unknown name
/// reports `Guarded`, creates no container and ends a running dialog (E-143).
pub fn start_dialog(sim: &mut crate::sim::Sim, name: &str) {
    super::actions::start_dialog(sim, name);
}
