//! Reads all level and dialog scripts (`.abl`) of the shipped data and checks byte fidelity and the run-time view
//! against the analysis (re/scripts 1, 3).

use std::collections::{BTreeMap, BTreeSet};

use opennml_formats::abl::{self, LineKind, catalog};
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use opennml_rda::{Vfs, normalize_path};

/// First character of the head (after the first `=`) of an action or condition line.
fn prefix_of(text: &str) -> Option<char> {
    text.split_once('=')
        .and_then(|(_, rest)| rest.chars().next())
}

// QUIRK E-119: kept
#[test]
fn abl_corpus_matches_the_analysis() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));

    let mut files = 0;
    let (mut blocks, mut actions, mut conditions) = (0, 0, 0);
    let mut action_numbers = BTreeSet::new();
    let mut condition_numbers = BTreeSet::new();
    let (mut waits, mut ors) = (0, 0);
    // Prefixes of the raw lines in active blocks without `§` (counted independently of the reader).
    let mut raw_action_prefixes: BTreeMap<char, usize> = BTreeMap::new();
    let mut raw_condition_prefixes: BTreeMap<char, usize> = BTreeMap::new();
    let (mut unprefixed_actions, mut unprefixed_conditions) = (0, 0);
    let mut unprefixed_with_section_sign = 0;

    for path in vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.starts_with("data/levels/") && p.ends_with(".abl"))
    {
        let bytes = vfs.read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let file = abl::parse(&bytes);
        assert_eq!(
            file.to_bytes(),
            bytes,
            "{path}: round trip not byte-identical"
        );
        let script = file
            .runtime()
            .unwrap_or_else(|e| panic!("Script {path}, {e}"));
        files += 1;
        blocks += script.blocks.len();
        for block in &script.blocks {
            conditions += block.conditions.len();
            actions += block.actions.len();
            ors += block.conditions.iter().filter(|c| c.or).count();
            waits += block.actions.iter().filter(|a| a.wait).count();
            action_numbers.extend(block.actions.iter().map(|a| a.number));
            condition_numbers.extend(block.conditions.iter().map(|c| c.number));
        }

        // Raw lines: prefixes only in active blocks (block and deactivation rules like in the original),
        // lines without a known prefix in the whole file.
        let (mut deactivated, mut in_block) = (false, false);
        for line in &file.lines {
            let text = &line.text;
            let in_active_block = if text.contains('{') {
                deactivated = false;
                false
            } else if text.contains("[DEACTIVATE]") {
                deactivated = true;
                false
            } else if text.contains("[BESCHREIBUNG") {
                in_block = !deactivated;
                false
            } else {
                in_block
            };
            let (entry, counts, unprefixed) = match &line.kind {
                LineKind::Action(e) => (e, &mut raw_action_prefixes, &mut unprefixed_actions),
                LineKind::Condition(e) => {
                    (e, &mut raw_condition_prefixes, &mut unprefixed_conditions)
                }
                _ => continue,
            };
            match prefix_of(text) {
                Some(c @ ('n' | 'w' | '&' | '|')) => {
                    if in_active_block && !entry.disabled {
                        *counts.entry(c).or_default() += 1;
                    }
                }
                _ => {
                    *unprefixed += 1;
                    unprefixed_with_section_sign += usize::from(entry.disabled);
                }
            }
        }
    }
    eprintln!(
        "{version:?}: {files} files, {blocks} blocks, {actions} actions ({} numbers, {waits} of them waiting), \
         {conditions} conditions ({} numbers, {ors} of them OR); raw prefixes actions {raw_action_prefixes:?}, \
         conditions {raw_condition_prefixes:?}; lines without prefix: {unprefixed_actions} actions and {unprefixed_conditions} conditions (with §: {unprefixed_with_section_sign})",
        action_numbers.len(),
        condition_numbers.len(),
    );

    assert_eq!(files, 127);
    // All lines without an `n`/`w`/`&`/`|` prefix carry `§` and are thereby dropped (E-119, E-122).
    assert_eq!(
        unprefixed_actions + unprefixed_conditions,
        unprefixed_with_section_sign
    );
    if version == GameVersion::German {
        assert_eq!(blocks, 2_410);
        assert_eq!(actions, 20_240);
        assert_eq!(action_numbers.len(), 151);
        assert_eq!(conditions, 4_162);
        assert_eq!(condition_numbers.len(), 35);
        // Analysis: 41 action lines (re/scripts 3); plus one condition line.
        assert_eq!(unprefixed_actions, 41);
        assert_eq!(unprefixed_conditions, 1);
        assert_eq!(raw_action_prefixes[&'n'], 18_100);
        assert_eq!(raw_action_prefixes[&'w'], 1_930);
        assert_eq!(raw_action_prefixes[&'&'], 210);
        assert_eq!(raw_condition_prefixes[&'&'], 3_952);
        assert_eq!(raw_condition_prefixes[&'|'], 210);
        // Run-time view: only `w` waits; OR is every condition prefix except `&`.
        assert_eq!(waits, 1_930);
        assert_eq!(ors, 210);
    }
}

/// All scripts pass the load check; dialog script = no `.lvl` of the same name next to the file. The
/// shipped-data counts per number (run-time view) match the analysis.
// QUIRK E-127: deviation
#[test]
fn abl_corpus_passes_the_load_check() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));

    let (mut files, mut dialogs) = (0, 0);
    let mut action_counts: BTreeMap<i32, usize> = BTreeMap::new();
    let mut condition_counts: BTreeMap<i32, usize> = BTreeMap::new();
    for path in vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.starts_with("data/levels/") && p.ends_with(".abl"))
    {
        let bytes = vfs.read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let script = abl::parse(&bytes)
            .runtime()
            .unwrap_or_else(|e| panic!("{}", e.in_script(&path)));
        let lvl = format!("{}.lvl", path.strip_suffix(".abl").unwrap());
        let dialog = !vfs.exists(&lvl);
        files += 1;
        dialogs += usize::from(dialog);
        if let Err(e) = catalog::check(&script, &path, dialog) {
            panic!("{}", e.in_script(&path));
        }
        for block in &script.blocks {
            for a in &block.actions {
                *action_counts.entry(a.number).or_default() += 1;
            }
            for c in &block.conditions {
                *condition_counts.entry(c.number).or_default() += 1;
            }
        }
    }
    eprintln!(
        "{version:?}: {files} files checked, of which {dialogs} dialog scripts; action 0: {}, 151: {}, 25: {}, 12: {}; condition 5: {}, 24: {}",
        action_counts[&0],
        action_counts[&151],
        action_counts[&25],
        action_counts[&12],
        condition_counts[&5],
        condition_counts[&24],
    );
    assert_eq!(files, 127);
    if version == GameVersion::German {
        assert_eq!(action_counts[&0], 2_265);
        assert_eq!(action_counts[&151], 1_561);
        assert_eq!(action_counts[&25], 648);
        assert_eq!(action_counts[&12], 664);
        assert_eq!(condition_counts[&5], 857);
        assert_eq!(condition_counts[&24], 672);
    }
}
