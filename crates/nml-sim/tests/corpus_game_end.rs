//! Game end with the rule files of the installation (re/ingame-menus 6.1 and 7): game type 4 on a
//! railroad map with and without a time limit, free game types without an end at the start.

use nml_rda::Vfs;
use nml_rda::testutil::corpus_game_dir;
use nml_sim::rules::Rules;
use nml_sim::setup::{MatchSetup, TIME_LIMIT_COUNTDOWN, new_match};
use nml_sim::state::{SimEvent, SimState};
use nml_sim::tick::step;
use nml_world::level::load_level;

const CROSSRAIL: &str = "data/levels/multiplayer/[2players]-crossrail.lvl";
const EXILE: &str = "data/levels/multiplayer/[4players]-exile.lvl";

fn rules_for(level: &str) -> Option<Rules> {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return None;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let scene = load_level(&vfs, level).unwrap();
    Some(Rules::load(&vfs, &scene).unwrap_or_else(|e| panic!("{level}: {e:#}")))
}

/// `n` steps without commands; the results that were set as (step, slot, result).
fn run(state: &mut SimState, rules: &Rules, n: usize) -> Vec<(u64, u8, i32)> {
    let mut out = Vec::new();
    for _ in 0..n {
        for e in step(state, rules, &[]) {
            if let SimEvent::PlayerResult { slot, result } = e {
                out.push((state.steps, slot, result));
            }
        }
    }
    out
}

#[test]
fn railroad_time_limit_holds_the_defenders_until_it_runs_out() {
    let Some(rules) = rules_for(CROSSRAIL) else {
        return;
    };
    // Game type 4 ("Railroad A&D"): slots 0 and 2 lose with `CHECKTIMER 0` (re/ingame-menus 6.1).
    // With a 20-minute time limit timer 0 runs and nobody loses at the start.
    let setup = MatchSetup {
        mode: 4,
        time_limit: 20,
        ..MatchSetup::default_for(&rules)
    };
    let mut state = new_match(&rules, &setup).unwrap();
    assert!(run(&mut state, &rules, 25).is_empty());
    assert_eq!(
        state.countdowns.get(&TIME_LIMIT_COUNTDOWN),
        Some(&(1_200_000 - 25 * 50))
    );
    // Timer at 0 ms: it is gone after the first tick, `CHECKTIMER 0` is true - slot 0 loses in the
    // first check, slot 1 then has no enemy left (`CHECKENEMY`). The timer is set to 0 by hand
    // after the start.
    let mut state = new_match(&rules, &setup).unwrap();
    state.countdowns.insert(TIME_LIMIT_COUNTDOWN, 0);
    assert_eq!(run(&mut state, &rules, 25), [(20, 0, 1), (20, 1, 2)]);
    assert!(state.countdowns.is_empty());
}

#[test]
fn free_game_types_do_not_end_at_the_start() {
    let Some(rules) = rules_for(EXILE) else {
        return;
    };
    // Game types 0, 1, 3 and 5: every slot has a headquarters, units and opponents - no result in
    // the first seconds.
    for mode in [0, 1, 3, 5] {
        let setup = MatchSetup {
            mode,
            ..MatchSetup::default_for(&rules)
        };
        let mut state = new_match(&rules, &setup).unwrap();
        assert!(run(&mut state, &rules, 25).is_empty(), "game type {mode}");
        assert!(state.countdowns.is_empty(), "game type {mode}");
    }
}
