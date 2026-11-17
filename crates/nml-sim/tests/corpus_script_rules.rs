//! Loads the script rules of all maps with a level script from the shipped data.

use std::collections::BTreeSet;

use nml_formats::abl::catalog::ParamKind;
use nml_formats::abl::{catalog, mid5};
use nml_rda::Vfs;
use nml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use nml_sim::rules::Rules;
use nml_sim::script::{LoadedScript, ScriptRules};
use nml_world::level::{level_paths, load_level};

/// Maps next to whose `.lvl` lies a `.abl` of the same name; measured in the reference edition
/// (German with patch). 31 campaign (with `oxdebug`), 2 tutorial, 2 menu, 2 multiplayer; the
/// orphaned `[5players]-vicious circle.abl` (with space) has no `.lvl`. re/scripts 1 counts
/// the campaign with 30 maps.
const SCRIPT_MAPS: usize = 37;
/// Number of dialog files reachable via action 39 (distinct paths, all maps); measured in the
/// reference edition. re/scripts names 78 dialog names and 90 dialog files in the folder (not
/// every one is reachable).
const REACHABLE_DIALOGS: usize = 76;

fn script_maps(vfs: &Vfs) -> Vec<String> {
    let mut maps: Vec<String> = level_paths(vfs)
        .into_iter()
        .filter(|lvl| vfs.exists(&format!("{}abl", lvl.trim_end_matches("lvl"))))
        .collect();
    maps.sort();
    maps
}

fn all_scripts(rules: &ScriptRules) -> Vec<&LoadedScript> {
    rules.level.iter().chain(rules.dialogs.values()).collect()
}

#[test]
fn all_levels_load_their_scripts() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    let maps = script_maps(&vfs);
    let mut dialog_paths = BTreeSet::new();
    for lvl in &maps {
        let scene = load_level(&vfs, lvl).unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
        let scripts = &rules.scripts;
        assert!(scripts.errors.is_empty(), "{lvl}: {:?}", scripts.errors);
        let level = scripts
            .level
            .as_ref()
            .unwrap_or_else(|| panic!("{lvl}: no level script"));
        assert!(!level.file.blocks.is_empty(), "{lvl}");
        assert_eq!(level.path, format!("{}abl", lvl.trim_end_matches("lvl")));
        dialog_paths.extend(scripts.dialogs.values().map(|d| d.path.clone()));
        // Every script can be found via its path.
        for script in all_scripts(scripts) {
            assert_eq!(scripts.script(&script.path), Some(script), "{lvl}");
        }
    }
    eprintln!(
        "Maps with level script: {}, reachable dialog files: {}",
        maps.len(),
        dialog_paths.len()
    );
    if version == GameVersion::German {
        assert_eq!(maps.len(), SCRIPT_MAPS);
        assert_eq!(dialog_paths.len(), REACHABLE_DIALOGS);
    }
}

/// Area and dialog parameters of all loaded scripts (actions and conditions) by catalog.
fn names_of(scripts: &ScriptRules, kind: ParamKind) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for script in all_scripts(scripts) {
        for block in &script.file.blocks {
            let conditions = block
                .conditions
                .iter()
                .filter_map(|c| catalog::condition(c.number).map(|s| (s.params, &c.params)));
            let actions = block
                .actions
                .iter()
                .filter_map(|a| catalog::action(a.number).map(|s| (s.params, &a.params)));
            for (kinds, params) in conditions.chain(actions) {
                for (k, p) in kinds.iter().zip(params) {
                    if *k == kind {
                        let name = if kind == ParamKind::Area { mid5(p) } else { p };
                        found.push((script.path.clone(), name.to_string()));
                    }
                }
            }
        }
    }
    found
}

/// All area and dialog names of the loaded scripts are known, except the areas `fort...`: on full
/// loading the original creates `[AREA]` blocks with this prefix as a fort object instead of in
/// the area table (`World_ParseNextBlock` `0x00441C50`), so the scripts do not find them. The
/// three names `pater nachts ende`, `tor nachtdialog` and `carvines nachtdialog` stand only in
/// `§` lines of `kamp1-3/nachtdialog.abl`, which the original does not load; they do not occur
/// here.
#[test]
fn area_and_dialog_names_resolve() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    let mut unknown = BTreeSet::new();
    for lvl in script_maps(&vfs) {
        let scene = load_level(&vfs, &lvl).unwrap();
        let rules = Rules::load(&vfs, &scene).unwrap();
        let scripts = &rules.scripts;
        for (path, name) in names_of(scripts, ParamKind::Area) {
            if scripts.area(&name).is_none() {
                // Must be an area of the `.wld` with the prefix `fort`.
                assert!(
                    scene
                        .world
                        .areas
                        .iter()
                        .any(|a| a.name.trim() == name && a.name.starts_with("fort")),
                    "{path}: area \"{name}\" neither in the table nor a fort"
                );
                unknown.insert((path, name));
            }
        }
        for (path, name) in names_of(scripts, ParamKind::Dialog) {
            assert!(
                scripts.dialogs.contains_key(&name),
                "{path}: dialog \"{name}\" not loaded"
            );
        }
    }
    let expected: BTreeSet<(String, String)> = [
        ("data/levels/kampagne1/kamp1-3/kamp1-3.abl", "fort"),
        ("data/levels/kampagne1/kamp1-3/kamp1-3.abl", "fort01blue"),
        ("data/levels/kampagne1/kamp1-3/kamp1-3.abl", "fort1"),
        ("data/levels/tutorial0-1/kamp0-1.abl", "fort"),
    ]
    .map(|(p, n)| (p.to_string(), n.to_string()))
    .into();
    if version == GameVersion::German {
        assert_eq!(unknown, expected);
    } else {
        eprintln!("unknown names (not reference edition): {unknown:?}");
    }
}

/// Speech table of `kamp1-1` (reference edition): 33 entries; every dialog line (action 150) has a
/// speech file, the tips and infos (183, 213) do not. Samples `(id, ogg_ms, text_units)`.
#[test]
fn kamp1_1_speech_table() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    let rules = Rules::load(
        &vfs,
        &load_level(&vfs, "data/levels/kampagne1/kamp1-1/kamp1-1.lvl").unwrap(),
    )
    .unwrap();
    let scripts = &rules.scripts;
    // Text IDs of the dialog lines (action 150, parameter 0) in all loaded scripts.
    let lines: BTreeSet<i32> = all_scripts(scripts)
        .into_iter()
        .flat_map(|s| &s.file.blocks)
        .flat_map(|b| &b.actions)
        .filter(|a| a.number == 150)
        .map(|a| a.params[0].parse().unwrap())
        .collect();
    let sample = |id: i32| {
        let len = scripts.speech[&id];
        (id, len.ogg_ms, len.text_units)
    };
    eprintln!(
        "Speech table: {} entries, {} dialog lines; samples {:?}",
        scripts.speech.len(),
        lines.len(),
        [20002, 20016, 20100].map(|id| scripts.speech.get(&id).copied())
    );
    for id in &lines {
        assert!(
            scripts.speech[id].ogg_ms > 0,
            "dialog line {id} without speech file"
        );
    }
    if version == GameVersion::German {
        assert_eq!(scripts.speech.len(), 33);
        assert_eq!(lines.len(), 13);
        assert_eq!(sample(20002), (20002, 5141, 96));
        assert_eq!(sample(20016), (20016, 6119, 96));
        // Without speech file: only the text length counts.
        assert_eq!(sample(20100), (20100, 0, 97));
        assert_eq!(scripts.speech_ms(20002, 55), 5141);
        assert_eq!(scripts.speech_ms(20100, 55), 97 * 55);
        assert_eq!(scripts.tip_ms(20100, 55), 97 * 55 * 3 / 2);
    }
}
