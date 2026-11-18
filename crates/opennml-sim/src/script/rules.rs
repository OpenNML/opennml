//! Immutable script rules (`Rules::scripts`): the level script, the dialog scripts reachable via action
//! 39, the areas of the `.wld` and the speech length table.
//!
//! Everything here arises when the rules are loaded from the game files and is not in the savegame. Load
//! errors (parser abort, load check, missing dialog file) are collected by [`ScriptRules::errors`]; not
//! until `new_match` with the script switched on does anything fail because of them; the map stays displayable
//! without a script.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;

use opennml_formats::abl::{self, AblError, ScriptFile, catalog};
use opennml_formats::config::lower_german;
use opennml_formats::localisation::{load_localisation, merge_text_tables};
use opennml_formats::ogg::{fmod_stream_length_ms, ogg_info};
use opennml_formats::property::{atol, is_c_space};
use opennml_rda::{Vfs, normalize_path};
use opennml_world::level::LevelScene;

use super::actions::START_DIALOG;
use crate::rules::Fnv64;

/// Folder of the speech files (`<text-id>.ogg`).
const SPEECH_DIR: &str = "data/sprache";
/// Conditions of the UI (35 to 38).
const UI_CONDITIONS: RangeInclusive<i32> = 35..=38;
/// Actions with a text ID and the index of the parameter (re/script-actions A.8): 150, 183, 213
/// (parameter 0), 220 (3), 221 (2), 222 (1), 223 (1), 230 (2), 232 (1).
const SPEECH_ACTIONS: [(i32, usize); 9] = [
    (150, 0),
    (183, 0),
    (213, 0),
    (220, 3),
    (221, 2),
    (222, 1),
    (223, 1),
    (230, 2),
    (232, 1),
];

/// A loaded script: normalized path, FNV-1a 64 of the file bytes and the run-time view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedScript {
    pub path: String,
    pub checksum: u64,
    pub file: ScriptFile,
}

/// Speech length of a text ID: length of the speech file and length of the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechLen {
    /// Length of `data/sprache/<id>.ogg` in ms (FMOD calculation); 0 if the file is missing or corrupted.
    pub ogg_ms: u32,
    /// UTF-16 units of the text in the text table; if the text is missing, the digit count of the ID (E-171).
    pub text_units: u32,
}

/// Script rules of a match (part of [`crate::rules::Rules`]).
#[derive(Debug, Clone, Default)]
pub struct ScriptRules {
    /// Level script (`<base>.abl` next to the `.lvl`); `None` if the file is missing or does not load.
    pub level: Option<LoadedScript>,
    /// Dialog scripts, key = normalized parameter of action 39.
    pub dialogs: BTreeMap<String, LoadedScript>,
    /// Areas (`x0, z0, x1, z1` in tiles, inclusive), key lowercase and trimmed.
    pub areas: BTreeMap<String, [i32; 4]>,
    /// Speech lengths per text ID of actions 150, 183, 213, 220 to 223, 230, 232 of all loaded scripts.
    pub speech: BTreeMap<i32, SpeechLen>,
    /// A condition 35 to 38 in the level script or a dialog script.
    pub uses_ui_conditions: bool,
    /// Load errors: "Script {path}, line {line}: ...".
    pub errors: Vec<String>,
}

/// Reads and checks a script (parser, load check); errors go to `errors`, a faulty script is not loaded.
/// Shared by [`ScriptRules::load`] and the test helper [`super::testing::rules_with`].
pub(crate) fn accept_script(
    path: &str,
    bytes: &[u8],
    dialog: bool,
    errors: &mut Vec<String>,
) -> Option<LoadedScript> {
    let file = match abl::parse(bytes).runtime() {
        Ok(file) => file,
        Err(e) => {
            errors.push(e.in_script(path));
            return None;
        }
    };
    if let Err(e) = catalog::check(&file, path, dialog) {
        errors.push(e.in_script(path));
        return None;
    }
    let mut hash = Fnv64::default();
    hash.write(bytes);
    Some(LoadedScript {
        path: path.to_string(),
        checksum: hash.0,
        file,
    })
}

/// Digit count of `format!("{id}")` (E-171: missing text, `"%d"` instead of the string).
fn digit_count(id: i32) -> u32 {
    id.to_string().len() as u32
}

impl ScriptRules {
    /// Loads level script, dialogs, areas and speech lengths. `hash` takes in the path and bytes of each script
    /// in path order and then `id`, `ogg_ms`, `text_units` per speech length (last, after the AI catalog).
    pub fn load(vfs: &Vfs, scene: &LevelScene, hash: &mut Fnv64) -> Self {
        Self::load_files(vfs, &scene.lvl_path, collect_areas(scene), hash)
    }

    /// [`ScriptRules::load`] without a scene: level path `lvl_path` and the areas already collected.
    fn load_files(
        vfs: &Vfs,
        lvl_path: &str,
        areas: BTreeMap<String, [i32; 4]>,
        hash: &mut Fnv64,
    ) -> Self {
        let mut rules = Self {
            areas,
            ..Self::default()
        };

        let base = lvl_path.strip_suffix(".lvl").unwrap_or(lvl_path);
        let level_path = normalize_path(&format!("{base}.abl"));
        // Path -> bytes of all scripts read, for the checksum in path order.
        let mut read_files: BTreeMap<String, Vec<u8>> = BTreeMap::new();

        if let Ok(bytes) = vfs.read(&level_path) {
            rules.level = accept_script(&level_path, &bytes, false, &mut rules.errors);
            read_files.insert(level_path.clone(), bytes);
        }

        // Dialogs: for every action 39 of the level script `<folder>/<p0>.abl`. The VFS ignores case,
        // `p0` is lowercase anyway (`normalize_param`).
        let dir = level_path.rsplit_once('/').map_or("", |(dir, _)| dir);
        rules.load_dialogs(&level_path, |name| {
            let path = normalize_path(&format!("{dir}/{name}.abl"));
            let bytes = vfs.read(&path).ok()?;
            read_files.insert(path.clone(), bytes.clone());
            Some((path, bytes))
        });

        rules.refresh_ui_flag();
        rules.load_speech(vfs);

        for (path, bytes) in &read_files {
            hash.write(path.as_bytes());
            hash.write(bytes);
        }
        for (id, len) in &rules.speech {
            hash.write(&id.to_le_bytes());
            hash.write(&len.ogg_ms.to_le_bytes());
            hash.write(&len.text_units.to_le_bytes());
        }
        rules
    }

    /// Loads the dialog scripts of all actions 39 of [`ScriptRules::level`] in file order; each name only at
    /// the first reference, so a broken or missing dialog is reported only once. `read` returns path and
    /// bytes for the name or `None` (missing: message with the path of the level script `level_path` and
    /// the line of the reference). Returns the names tried.
    pub(crate) fn load_dialogs(
        &mut self,
        level_path: &str,
        mut read: impl FnMut(&str) -> Option<(String, Vec<u8>)>,
    ) -> BTreeSet<String> {
        let starts: Vec<(String, u32)> = self
            .level
            .iter()
            .flat_map(|s| &s.file.blocks)
            .flat_map(|b| &b.actions)
            .filter(|a| a.number == START_DIALOG)
            .filter_map(|a| a.params.first().map(|p| (p.clone(), a.line)))
            .collect();
        let mut tried = BTreeSet::new();
        for (name, line) in starts {
            if !tried.insert(name.clone()) {
                continue;
            }
            match read(&name) {
                Some((path, bytes)) => {
                    if let Some(script) = accept_script(&path, &bytes, true, &mut self.errors) {
                        self.dialogs.insert(name, script);
                    }
                }
                None => {
                    let error = AblError {
                        line,
                        message: format!("dialog script \"{name}\" missing"),
                    };
                    self.errors.push(error.in_script(level_path));
                }
            }
        }
        tried
    }

    /// Sets [`ScriptRules::uses_ui_conditions`] from the loaded scripts (level and dialogs): a condition
    /// 35 to 38 occurs. Shared by [`ScriptRules::load`] and [`super::testing::rules_with`] so that synthetic
    /// scripts also carry the flag.
    pub(crate) fn refresh_ui_flag(&mut self) {
        let uses = self.scripts().any(|s| {
            s.file
                .blocks
                .iter()
                .flat_map(|b| &b.conditions)
                .any(|c| UI_CONDITIONS.contains(&c.number))
        });
        self.uses_ui_conditions = uses;
    }

    /// All loaded scripts: level first, then the dialogs by key.
    fn scripts(&self) -> impl Iterator<Item = &LoadedScript> {
        self.level.iter().chain(self.dialogs.values())
    }

    /// Speech lengths of the text IDs of all loaded scripts.
    fn load_speech(&mut self, vfs: &Vfs) {
        let ids: Vec<i32> = self
            .scripts()
            .flat_map(|s| &s.file.blocks)
            .flat_map(|b| &b.actions)
            .filter_map(|a| {
                SPEECH_ACTIONS
                    .iter()
                    .find(|(number, _)| *number == a.number)
                    .and_then(|(_, index)| a.params.get(*index))
                    .map(|p| atol(p) as i32)
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        // If the speech file or the text table is missing, the texts stay empty: the length is then the
        // digit count of the ID (E-171) as for a missing text; the original continues the same way.
        let read = |path: &str| vfs.read(path).ok();
        let table = match load_localisation(&read) {
            Ok(loc) => {
                let (table, warnings) = merge_text_tables(&loc, &read);
                for warning in warnings {
                    log::warn!("{warning}");
                }
                table
            }
            Err(message) => {
                log::warn!("{message}");
                BTreeMap::new()
            }
        };
        for id in ids {
            self.speech.entry(id).or_insert_with(|| {
                let ogg_ms = vfs
                    .read(&format!("{SPEECH_DIR}/{id}.ogg"))
                    .ok()
                    .and_then(|bytes| ogg_info(&bytes).ok())
                    .map_or(0, |info| fmod_stream_length_ms(&info));
                let text_units = table.get(&id).map_or_else(
                    || digit_count(id),
                    |text| text.encode_utf16().count() as u32,
                );
                SpeechLen { ogg_ms, text_units }
            });
        }
    }

    /// Speech length in ms (`Dialog_GetSpeechLengthMs` `0x00438000`): length of the speech file, otherwise
    /// `text length * TimePerChar`; an unknown ID counts its digits.
    pub fn speech_ms(&self, text: i32, time_per_char: i32) -> i32 {
        match self.speech.get(&text) {
            Some(len) if len.ogg_ms > 0 => len.ogg_ms as i32,
            Some(len) => (len.text_units as i32).wrapping_mul(time_per_char),
            None => (digit_count(text) as i32).wrapping_mul(time_per_char),
        }
    }

    /// Typing time of a tip (`FUN_0043E250`): `1.5 * text length * TimePerChar`, truncated like
    /// `(n * 3) >> 1` on an unsigned number.
    pub fn tip_ms(&self, text: i32, time_per_char: i32) -> i32 {
        let units = self
            .speech
            .get(&text)
            .map_or_else(|| digit_count(text), |len| len.text_units);
        (((units as i32).wrapping_mul(time_per_char).wrapping_mul(3) as u32) >> 1) as i32
    }

    /// Rectangle of the area `name` (`FUN_00435A60`): looked up lowercased and trimmed.
    pub fn area(&self, name: &str) -> Option<[i32; 4]> {
        self.areas
            .get(&lower_german(name.trim_matches(is_c_space)))
            .copied()
    }

    /// Level or dialog script by (normalized) path.
    pub fn script(&self, path: &str) -> Option<&LoadedScript> {
        let path = normalize_path(path);
        self.scripts().find(|s| s.path == path)
    }
}

/// Areas of the `.wld` without `fort...` and `noshadow...`; a duplicate overwrites.
fn collect_areas(scene: &LevelScene) -> BTreeMap<String, [i32; 4]> {
    let mut areas = BTreeMap::new();
    for area in &scene.world.areas {
        // The original checks the prefixes on the lowercased, still untrimmed name
        // (`World_ParseNextBlock` `0x00441C50`) and does not trim until the name is entered.
        let name = lower_german(&area.name);
        if name.starts_with("fort") || name.starts_with("noshadow") {
            continue;
        }
        areas.insert(name.trim_matches(is_c_space).to_string(), area.rect);
    }
    areas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_with_speech(entries: &[(i32, u32, u32)]) -> ScriptRules {
        let mut rules = ScriptRules::default();
        for &(id, ogg_ms, text_units) in entries {
            rules.speech.insert(id, SpeechLen { ogg_ms, text_units });
        }
        rules
    }

    /// Loads scripts via a real `Vfs` (archive `nml0.rda` in a temp folder) with `ScriptRules::load_files`.
    fn load_from_archive(files: &[(&str, &str)]) -> ScriptRules {
        use opennml_rda::testutil::{TestFile, build_rda};
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<TestFile<'_>> = files
            .iter()
            .map(|(path, text)| TestFile {
                path,
                data: text.as_bytes(),
                compress: false,
            })
            .collect();
        std::fs::write(dir.path().join("nml0.rda"), build_rda(&entries)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        ScriptRules::load_files(
            &vfs,
            "data/levels/test/t.lvl",
            BTreeMap::new(),
            &mut Fnv64::default(),
        )
    }

    #[test]
    fn load_reports_missing_and_broken_dialogs_once() {
        // Line 3: missing dialog; lines 7 and 8: the same broken dialog twice (action 39 in the dialog
        // script, E-174); line 9: a good dialog.
        let level = "{
[BESCHREIBUNG=a]
[AKTION=n39;fehlt:Text]
}
{
[BESCHREIBUNG=b]
[AKTION=n39;kaputt:Text]
[AKTION=n39;kaputt:Text]
[AKTION=n39;gut:Text]
[AKTION=n39;fehlt:Text]
}
";
        let broken = "{
[BESCHREIBUNG=d]
[AKTION=n39;weiter:Text]
}
";
        let good = "{
[BESCHREIBUNG=d]
[AKTION=n12;x:Text]
}
";
        let rules = load_from_archive(&[
            ("data/levels/test/t.abl", level),
            ("data/levels/test/kaputt.abl", broken),
            ("data/levels/test/gut.abl", good),
        ]);
        assert_eq!(
            rules.errors,
            [
                "Script data/levels/test/t.abl, line 3: dialog script \"fehlt\" missing",
                "Script data/levels/test/kaputt.abl, line 3: dialog script starts a dialog (action 39)",
            ]
        );
        assert!(rules.level.is_some(), "the level script itself is valid");
        assert_eq!(rules.dialogs.keys().collect::<Vec<_>>(), ["gut"]);
        assert_eq!(rules.dialogs["gut"].path, "data/levels/test/gut.abl");
    }

    #[test]
    fn load_without_level_script_has_no_errors() {
        let rules = load_from_archive(&[("data/levels/test/anders.abl", "")]);
        assert!(rules.level.is_none());
        assert!(rules.errors.is_empty(), "{:?}", rules.errors);
    }

    // QUIRK E-171: kept
    #[test]
    fn speech_ms_falls_back_to_text_and_digits() {
        let rules = rules_with_speech(&[(1, 3000, 20), (2, 0, 20)]);
        assert_eq!(rules.speech_ms(1, 55), 3000);
        assert_eq!(rules.speech_ms(2, 55), 1100);
        // Missing ID: digit count of "12345" times TimePerChar (E-171).
        assert_eq!(rules.speech_ms(12345, 55), 5 * 55);
    }

    #[test]
    fn tip_ms_is_three_halves_truncated() {
        let rules = rules_with_speech(&[(1, 9999, 7)]);
        assert_eq!(rules.tip_ms(1, 55), 577);
        assert_eq!(rules.tip_ms(12345, 55), 5 * 55 * 3 / 2);
    }

    #[test]
    fn area_lookup_trims_and_lowers() {
        let mut rules = ScriptRules::default();
        rules.areas.insert("tor süd".into(), [1, 2, 3, 4]);
        assert_eq!(rules.area("  Tor SÜD\t"), Some([1, 2, 3, 4]));
        assert_eq!(rules.area("tor"), None);
    }
}
