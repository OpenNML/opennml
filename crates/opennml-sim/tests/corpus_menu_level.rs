//! Menu level behind the main menu (re/menu-level ML.2, ML.6, ML.7): `menü2.lvl` in game type -2
//! runs deterministically (`MENU_LEVEL_GOLDEN`), turns cinemascope on in the first step and holds
//! it throughout, and knows no rule results. The checksums apply only to the reference edition
//! (German with patch); without the original installation or in the English edition the tests
//! skip silently.
//!
//! That `GameSaveFile::check` (savegame container, `opennml-client`) rejects game type -2 is covered
//! there by the test `game_save_rejects_wrong_magic_version_and_checksum` (`game_save.rs`, loop
//! `[-2, 6, i32::MAX]` over the game mode).

use opennml_rda::Vfs;
use opennml_rda::testutil::{GameVersion, corpus_game_dir, game_version};
use opennml_sim::ki::KiSetup;
use opennml_sim::rules::{Fnv64, Rules};
use opennml_sim::save::checksum;
use opennml_sim::script::ScriptSetup;
use opennml_sim::setup::{MENU_LEVEL_MODE, MatchSetup, new_match};
use opennml_sim::state::SimState;
use opennml_sim::tick::step;
use opennml_world::level::{level_paths, load_level};

/// File name of the map (in the folder `menülevel`).
const LEVEL_FILE: &str = "menü2.lvl";
/// Ticks per run: 200 s at 50 ms per tick.
const STEPS: usize = 4_000;
/// Interval between the sample points.
const SAMPLE_EVERY: usize = 500;
/// `TimePerChar` of the settings (like the scene of `SCRIPT_GOLDEN`).
const TIME_PER_CHAR: i32 = 55;

/// Checksums of `menü2.lvl` in game type -2: before the first tick and after every 500th tick
/// (nine values). Measured with the reference edition (German with patch), same in debug and release.
const MENU_LEVEL_GOLDEN: [u64; 9] = [
    0xf941_0253_bbe6_ab37,
    0x97bf_11e1_0564_2fd1,
    0x0555_6ea6_a2c9_d8b9,
    0x7e69_9d53_9d6d_9f9f,
    0x4f28_4698_fd39_36c4,
    0x67fd_b831_3e78_18b9,
    0x1d50_e29e_5517_cf5a,
    0x6e0e_3ec0_d483_47a4,
    0xf5c6_a117_50be_9b9f,
];

/// Rules of the map; `None` without installation or in the English edition.
fn menu_level_rules() -> Option<Rules> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let version = game_version(&vfs).unwrap_or_else(|e| panic!("{e}"));
    if version != GameVersion::German {
        eprintln!(
            "English edition - MENU_LEVEL_GOLDEN applies only to the reference edition, skipped"
        );
        return None;
    }
    let lvl = level_paths(&vfs)
        .into_iter()
        .find(|p| p.contains("menülevel/") && p.ends_with(LEVEL_FILE))
        .unwrap_or_else(|| panic!("{LEVEL_FILE} not found"));
    let scene = load_level(&vfs, &lvl).unwrap();
    Some(Rules::load(&vfs, &scene, "DEU").unwrap_or_else(|e| panic!("{lvl}: {e:#}")))
}

/// Setup like `opennml_client::match_request::menu_level_setup` (`opennml-client` depends on `opennml-sim`, so
/// reproduced here): game type -2, no lobby, AI on with difficulty 0, script on.
fn menu_level_setup() -> MatchSetup {
    MatchSetup {
        mode: MENU_LEVEL_MODE,
        lobby: Vec::new(),
        map_explored: false,
        ki: KiSetup {
            enabled: true,
            campaign_difficulty: 0,
        },
        script: ScriptSetup {
            enabled: true,
            time_per_char: TIME_PER_CHAR,
            ..ScriptSetup::default()
        },
        ..MatchSetup::default()
    }
}

/// A run over [`STEPS`] ticks; `on_step(tick, state)` after every tick (1-based). Returns the sample points.
fn run(rules: &Rules, mut on_step: impl FnMut(usize, &SimState)) -> (Vec<u64>, SimState) {
    let mut state = new_match(rules, &menu_level_setup()).unwrap_or_else(|e| panic!("{e}"));
    let mut samples = vec![checksum(&state)];
    for n in 1..=STEPS {
        step(&mut state, rules, &[]);
        on_step(n, &state);
        if n % SAMPLE_EVERY == 0 {
            samples.push(checksum(&state));
        }
    }
    (samples, state)
}

// QUIRK O-446: kept
#[test]
fn menu_level_runs_deterministically() {
    let Some(rules) = menu_level_rules() else {
        return;
    };
    let (first, state) = run(&rules, |_, _| {});
    let (second, _) = run(&rules, |_, _| {});
    assert_eq!(first, second, "two runs differ");
    assert_eq!(state.mode, MENU_LEVEL_MODE);
    let mut all = Fnv64::default();
    for c in &first {
        all.write(&c.to_le_bytes());
    }
    eprintln!("MENU_LEVEL_GOLDEN {first:#018x?} (folded {:#018x})", all.0);
    assert_eq!(first, MENU_LEVEL_GOLDEN, "{first:#018x?}");
}

// QUIRK O-446: kept
#[test]
fn menu_level_turns_cinemascope_on_in_the_first_step() {
    let Some(rules) = menu_level_rules() else {
        return;
    };
    let before = new_match(&rules, &menu_level_setup()).unwrap();
    assert!(!before.script.mission.cinemascope, "before the first tick");
    let mut off_at = Vec::new();
    run(&rules, |n, state| {
        if !state.script.mission.cinemascope {
            off_at.push(n);
        }
    });
    assert!(off_at.is_empty(), "cinemascope off in ticks {off_at:?}");
}

// QUIRK O-446: kept
#[test]
fn menu_level_has_no_rule_result() {
    let Some(rules) = menu_level_rules() else {
        return;
    };
    let (_, state) = run(&rules, |_, _| {});
    let results: Vec<(usize, i32)> = state
        .players
        .iter()
        .enumerate()
        .filter(|(_, p)| p.slot_active && p.result != 0)
        .map(|(i, p)| (i, p.result))
        .collect();
    assert!(results.is_empty(), "slots with result: {results:?}");
}
