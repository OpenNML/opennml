//! Assignment of the script numbers to implementation groups, shared by the script corpus tests.
//! No action reports `Pending` any more (all presentation numbers are implemented) and no condition reports
//! `ConditionPending` any more (35-38 evaluate the selection mirror and the button).

#![allow(dead_code)]

pub mod account;

use opennml_rda::Vfs;
use opennml_sim::rules::Rules;
use opennml_sim::setup::{MatchSetup, new_match};
use opennml_sim::state::SimState;
use opennml_world::level::load_level;

/// Loads the level `lvl`, checks the scripts and starts the match with the script (setup `default_for`,
/// AI per `ki`). Shared start of the tutorial and selection tests.
pub fn start_level(vfs: &Vfs, lvl: &str, ki: bool) -> (Rules, SimState) {
    let scene = load_level(vfs, lvl).unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
    let rules = Rules::load(vfs, &scene, "DEU").unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
    assert!(
        rules.scripts.errors.is_empty(),
        "{lvl}: {:?}",
        rules.scripts.errors
    );
    let mut setup = MatchSetup::default_for(&rules);
    setup.ki.enabled = ki;
    setup.script.enabled = true;
    let state = new_match(&rules, &setup).unwrap_or_else(|e| panic!("{lvl}: {e:#}"));
    (rules, state)
}

/// Actions of extension 1: commands (route B) and AI start/`AbortAllModules` (route W). They are implemented and
/// report `ScriptTrace::Command` per command (route B) or nothing (route W) instead of `Pending`.
pub const E2C_ACTIONS: &[i32] = &[
    0, 1, 4, 8, 13, 34, 37, 38, 153, 154, 157, 158, 163, 164, 165, 166, 167, 174, 179, 181, 188,
    189, 190, 191, 193, 334, 335, 336, 337, 338, 340, 373, 374, 375, 376, 380, 390, 402,
];

/// Actions of extension 2: presentation (route P). All implemented (kept only for the reports).
pub const E2D_ACTIONS: &[i32] = &[
    2, 5, 7, 9, 18, 19, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 152, 155, 156, 168, 169,
    170, 171, 172, 175, 176, 177, 178, 183, 209, 210, 212, 213, 216, 220, 221, 222, 223, 240, 241,
    242, 243, 370, 371, 381, 382, 383, 501, 502, 503, 504, 505, 506, 507, 508,
];

/// AI start via the script API (extension 1, route W).
pub const KI_START_ACTIONS: &[i32] = &[0, 1, 37, 340];

/// Group that implements the effect of an action (for reports): "extension 1", "extension 2" or "base".
pub fn plan_of(action: i32) -> &'static str {
    if E2C_ACTIONS.contains(&action) {
        "extension 1"
    } else if E2D_ACTIONS.contains(&action) {
        "extension 2"
    } else {
        "base"
    }
}
