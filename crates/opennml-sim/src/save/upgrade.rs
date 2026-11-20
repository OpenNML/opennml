//! Recompute steps of the game savegame: values that the state caches from the rules and whose derivation has
//! changed (for example a grid from the terrain classes). They run after the body is typed and checked against the
//! rules, with `&Rules`, and depend only on the savegame number the file was loaded from. Like the structure steps of
//! the client (`opennml_client::game_save`) they must be deterministic: every machine of a network match computes the
//! same.

use anyhow::Context;

use crate::rules::Rules;
use crate::save::SaveGame;

/// One recompute step: sets the values of `save` that follow from `rules`.
pub type Recompute = fn(&mut SaveGame, &Rules) -> anyhow::Result<()>;

/// The recompute steps in ascending order: an entry `(n, f)` runs for a savegame loaded from a number below `n`.
pub const RECOMPUTE: &[(u32, Recompute)] = &[];

/// Runs the steps of [`RECOMPUTE`] that a savegame loaded from number `from` needs.
pub fn recompute(save: &mut SaveGame, rules: &Rules, from: u32) -> anyhow::Result<()> {
    recompute_with(save, rules, from, RECOMPUTE)
}

/// [`recompute`] with the list `list` (tests).
pub fn recompute_with(
    save: &mut SaveGame,
    rules: &Rules,
    from: u32,
    list: &[(u32, Recompute)],
) -> anyhow::Result<()> {
    for &(number, step) in list {
        if from < number {
            step(save, rules)
                .with_context(|| format!("recomputing for savegame number {number}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::testkit;

    fn save() -> (SaveGame, Rules) {
        let (rules, state) = super::super::tests::scene();
        let save = SaveGame {
            start: crate::setup::start_package(&rules, &testkit::setup()),
            state,
            commands: Vec::new(),
        };
        (save, rules)
    }

    fn a(save: &mut SaveGame, _: &Rules) -> anyhow::Result<()> {
        save.state.steps += 1;
        Ok(())
    }

    fn b(save: &mut SaveGame, _: &Rules) -> anyhow::Result<()> {
        save.state.steps += 100;
        Ok(())
    }

    #[test]
    fn recompute_runs_entries_newer_than_loaded() {
        let (mut save, rules) = save();
        let steps = save.state.steps;
        recompute_with(&mut save, &rules, 101, &[(101, a), (102, b)]).unwrap();
        assert_eq!(save.state.steps, steps + 100);
        // The current number runs nothing.
        recompute_with(&mut save, &rules, 102, &[(101, a), (102, b)]).unwrap();
        assert_eq!(save.state.steps, steps + 100);
    }
}
