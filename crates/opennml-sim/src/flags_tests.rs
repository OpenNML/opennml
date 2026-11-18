//! Tests of the flag leader (re/domination F.4, F.8): majority in Domination,
//! first holder in King of the Hill, timer 0, time limit and savegame. The tests set the counters
//! by hand and then trigger the check with a gain via `adjust_flag_count`.

use crate::flags::{LEADER_COUNTDOWN, NO_LEADER, evaluate_flag_leader, slot_of_color};
use crate::game_end::tick_countdowns;
use crate::module::natur::adjust_flag_count;
use crate::rules::Rules;
use crate::save::{SaveGame, checksum};
use crate::setup::{MatchSetup, new_match, start_package};
use crate::sim::Sim;
use crate::state::SimState;
use crate::tick::step;

/// Time limit of the test worlds in seconds (60 s -> timer 60 000 ms).
const LIMIT_S: i32 = 60;

/// State in game type `mode` with the time limit [`LIMIT_S`].
fn state(mode: i32) -> SimState {
    let mut state = SimState::new(mode, 0);
    state.time_limit = LIMIT_S;
    state
}

/// Sets the counters (without a check) and then reports a gain for `slot`.
fn set_then_gain(sim: &mut Sim, counts: [i32; 9], slot: u8) {
    sim.state.flag_counts = counts;
    adjust_flag_count(sim, slot, true);
}

fn timer(state: &SimState) -> Option<i32> {
    state.countdowns.get(&LEADER_COUNTDOWN).copied()
}

// QUIRK D-295: kept
#[test]
fn domination_leader_needs_more_than_half_with_neutral_flags() {
    let rules = Rules::synthetic("", 4);
    let mut state = state(2);
    let mut sim = Sim::new(&mut state, &rules);
    // [2, 1, ..., 1 neutral]: sum 4, half 2, 2 > 2 does not hold - nobody leads.
    set_then_gain(&mut sim, [2, 1, 0, 0, 0, 0, 0, 0, 0], 8);
    assert_eq!(sim.state.flag_counts, [2, 1, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (NO_LEADER, None));
    // [3, 1]: sum 4, half 2 - slot 0 leads, timer anew with `time_limit · 1000`.
    set_then_gain(&mut sim, [2, 1, 0, 0, 0, 0, 0, 0, 0], 0);
    assert_eq!(sim.state.flag_counts[..2], [3, 1]);
    assert_eq!(sim.state.flag_leader, 0);
    assert_eq!(timer(sim.state), Some(LIMIT_S * 1000));
    // Eight flags: four are not enough (4 > 4), five are (5 > 4). Slot 8 holds three or four.
    let mut state = self::state(2);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [3, 0, 0, 0, 0, 0, 0, 0, 4], 0);
    assert_eq!(sim.state.flag_counts.iter().sum::<i32>(), 8);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (NO_LEADER, None));
    set_then_gain(&mut sim, [4, 0, 0, 0, 0, 0, 0, 0, 3], 0);
    assert_eq!(sim.state.flag_counts.iter().sum::<i32>(), 8);
    assert_eq!(sim.state.flag_counts[0], 5);
    assert_eq!(sim.state.flag_leader, 0);
    assert_eq!(timer(sim.state), Some(LIMIT_S * 1000));
}

#[test]
fn leader_is_the_first_slot_over_half_and_other_slot_restarts_the_timer() {
    let rules = Rules::synthetic("", 4);
    let mut state = state(2);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [2, 0, 0, 0, 0, 0, 0, 0, 0], 0);
    assert_eq!(sim.state.flag_leader, 0);
    for _ in 0..40 {
        tick_countdowns(&mut sim);
    }
    assert_eq!(timer(sim.state), Some(LIMIT_S * 1000 - 2000));
    // Slot 3 takes over the majority: new leader, timer replaced (not present twice).
    set_then_gain(&mut sim, [0, 0, 0, 2, 0, 0, 0, 0, 0], 3);
    assert_eq!(sim.state.flag_leader, 3);
    assert_eq!(sim.state.countdowns.len(), 1);
    assert_eq!(timer(sim.state), Some(LIMIT_S * 1000));
}

// QUIRK D-295: kept
#[test]
fn same_leader_keeps_the_remaining_time() {
    let rules = Rules::synthetic("", 4);
    let mut state = state(2);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [2, 1, 0, 0, 0, 0, 0, 0, 0], 0);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (0, Some(60_000)));
    for _ in 0..100 {
        tick_countdowns(&mut sim);
    }
    assert_eq!(timer(sim.state), Some(60_000 - 100 * 50));
    // Another gain by the leader: same leader, remaining time stays.
    adjust_flag_count(&mut sim, 0, true);
    assert_eq!(sim.state.flag_leader, 0);
    assert_eq!(timer(sim.state), Some(60_000 - 100 * 50));
    // A gain by another slot also changes nothing as long as the leader holds the majority.
    adjust_flag_count(&mut sim, 1, true);
    assert_eq!(sim.state.flag_counts[..2], [4, 2]);
    assert_eq!(sim.state.flag_leader, 0);
    assert_eq!(timer(sim.state), Some(60_000 - 100 * 50));
}

// QUIRK D-295, O-180: kept
#[test]
fn lost_majority_removes_timer_and_leader() {
    // V6: slot 0 leads with 3 of 4; if it loses a flag to slot 1 (deduction, then gain), it stands
    // 2 : 2 - no majority: timer gone, leader 8.
    let rules = Rules::synthetic("", 4);
    let mut state = state(2);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [2, 1, 0, 0, 0, 0, 0, 0, 0], 0);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (0, Some(60_000)));
    adjust_flag_count(&mut sim, 0, false);
    adjust_flag_count(&mut sim, 1, true);
    assert_eq!(sim.state.flag_counts[..2], [2, 2]);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (NO_LEADER, None));
    // Without a leader it stays that way (no double removal, no timer).
    adjust_flag_count(&mut sim, 1, true);
    assert_eq!(sim.state.flag_leader, 1);
    assert_eq!(timer(sim.state), Some(60_000));
}

// QUIRK D-295: kept
#[test]
fn decrement_alone_does_not_reevaluate() {
    let rules = Rules::synthetic("", 4);
    let mut state = state(2);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [2, 1, 0, 0, 0, 0, 0, 0, 0], 0);
    for _ in 0..30 {
        tick_countdowns(&mut sim);
    }
    // The deduction alone does not check: slot 0 stays leader although it has lost the majority.
    adjust_flag_count(&mut sim, 0, false);
    adjust_flag_count(&mut sim, 0, false);
    assert_eq!(sim.state.flag_counts[..2], [1, 1]);
    assert_eq!(sim.state.flag_leader, 0);
    assert_eq!(timer(sim.state), Some(60_000 - 30 * 50));
}

// QUIRK D-295: kept
#[test]
fn allies_with_half_each_do_not_lead() {
    // V7, D-295: the majority counts per slot. Two allies with half of the four flags each
    // do not lead, even if the alliance together holds all (the counters know no diplomacy).
    let rules = Rules::synthetic("", 4);
    let mut state = state(2);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [2, 1, 0, 0, 0, 0, 0, 0, 0], 1);
    assert_eq!(sim.state.flag_counts[..2], [2, 2]);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (NO_LEADER, None));
}

#[test]
fn koth_first_slot_with_a_flag_leads() {
    let rules = Rules::synthetic("", 4);
    let mut state = state(3);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [0; 9], 2);
    assert_eq!(sim.state.flag_leader, 2);
    assert_eq!(timer(sim.state), Some(60_000));
    // The holder stays holder: the same leader keeps the remaining time.
    for _ in 0..10 {
        tick_countdowns(&mut sim);
    }
    adjust_flag_count(&mut sim, 2, true);
    assert_eq!(sim.state.flag_leader, 2);
    assert_eq!(timer(sim.state), Some(60_000 - 500));
    // The lowest slot with a flag leads, and the change restarts the timer.
    adjust_flag_count(&mut sim, 1, true);
    assert_eq!(sim.state.flag_leader, 1);
    assert_eq!(timer(sim.state), Some(60_000));
}

// QUIRK W-160: kept
#[test]
fn koth_animal_holder_removes_the_timer() {
    let rules = Rules::synthetic("", 4);
    let mut state = state(3);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [0; 9], 2);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (2, Some(60_000)));
    // An animal (slot 8) takes the flag: counter 2 down, counter 8 up. Slot 8 lies above 7: timer
    // gone, leader 8; no new timer.
    adjust_flag_count(&mut sim, 2, false);
    adjust_flag_count(&mut sim, 8, true);
    assert_eq!(sim.state.flag_counts, [0, 0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (NO_LEADER, None));
}

// QUIRK W-160: kept
#[test]
fn koth_branch_for_nobody_is_only_reachable_by_hand() {
    // W-160: the branch "nobody has a flag" (value 9) needs all-zero counters, but a check happens
    // only after a gain that itself raises a counter. Forced by hand: leader 9 with a timer,
    // as the original set it there.
    let rules = Rules::synthetic("", 4);
    let mut state = state(3);
    let mut sim = Sim::new(&mut state, &rules);
    set_then_gain(&mut sim, [0; 9], 2);
    sim.state.flag_counts = [0; 9];
    evaluate_flag_leader(&mut sim);
    assert_eq!((sim.state.flag_leader, timer(sim.state)), (9, Some(60_000)));
    // The next gain by a slot changes the leader again (9 -> i).
    adjust_flag_count(&mut sim, 4, true);
    assert_eq!(sim.state.flag_leader, 4);
}

const FLAG_TYPES: &str = "GUID 4300\nNAME Spieler_Fahne\nVALUEID 100 6 1 1024\nPROPID 5\nVALUEID 60001 6 1 1\nVALUEID 60002 6 1 5888\n";

#[test]
fn init_counts_on_slot_8_without_leader() {
    use crate::world::{ObjectDesc, create_object};
    // Every flag first counts for slot 8 when created and checks: the leader stays 8, no
    // timer runs - in Domination as in King of the Hill.
    for mode in [2, 3] {
        let rules = Rules::synthetic(FLAG_TYPES, 16);
        let mut state = state(mode);
        let mut sim = Sim::new(&mut state, &rules);
        for (n, x) in [200.0, 400.0, 600.0].into_iter().enumerate() {
            create_object(&mut sim, &mut ObjectDesc::new(4300, 8, [x, 0.0, 200.0])).unwrap();
            assert_eq!(sim.state.flag_counts[8], n as i32 + 1, "mode {mode}");
            assert_eq!(
                (sim.state.flag_leader, timer(sim.state)),
                (NO_LEADER, None),
                "mode {mode}"
            );
        }
    }
}

#[test]
fn other_modes_count_but_never_lead() {
    let rules = Rules::synthetic("", 4);
    for mode in [-1, 0, 1, 4, 5] {
        let mut state = state(mode);
        let mut sim = Sim::new(&mut state, &rules);
        for _ in 0..3 {
            adjust_flag_count(&mut sim, 0, true);
        }
        assert_eq!(sim.state.flag_counts[0], 3, "mode {mode}");
        assert_eq!(
            (sim.state.flag_leader, timer(sim.state)),
            (NO_LEADER, None),
            "mode {mode}"
        );
    }
}

#[test]
fn slot_of_color_takes_the_first_matching_slot_without_checking_occupancy() {
    // `FUN_00478DE0`: first slot 0-7 with the color, without an occupancy check; otherwise 8 (D-300).
    let mut state = state(2);
    assert_eq!(slot_of_color(&state, 5), 5);
    assert_eq!(slot_of_color(&state, 8), 8);
    assert_eq!(slot_of_color(&state, -1), 8);
    state.players[6].color = 2;
    assert_eq!(slot_of_color(&state, 2), 2);
    state.players[2].color = 7;
    assert_eq!(slot_of_color(&state, 2), 6);
    state.players[1].color = 7;
    assert_eq!(slot_of_color(&state, 7), 1);
}

#[test]
fn mode_4_timer_unchanged() {
    // Game type 4: timer 0 with 60 000 * `TimeLimit` ms, now from `state.time_limit · 1000`.
    let rules = crate::command::testkit::rules();
    for limit in [1, 7, 20, 999] {
        let setup = MatchSetup {
            mode: 4,
            time_limit: limit,
            ..MatchSetup::default()
        };
        let state = new_match(&rules, &setup).unwrap();
        assert_eq!(state.time_limit, limit * 60);
        assert_eq!(timer(&state), Some(60_000 * limit));
        assert_eq!(state.countdowns.len(), 1);
        assert_eq!(state.flag_leader, NO_LEADER);
    }
}

#[test]
fn flag_state_survives_save_load() {
    // Mode 3: leader, time limit, counters (also negative, D-299) and
    // leader timer come back loaded, and both paths continue bit-identically.
    let rules = crate::command::testkit::rules();
    let setup = MatchSetup {
        mode: 3,
        time_limit: 2,
        ..MatchSetup::default()
    };
    let mut state = new_match(&rules, &setup).unwrap();
    assert_eq!(state.time_limit, 120);
    {
        let mut sim = Sim::new(&mut state, &rules);
        adjust_flag_count(&mut sim, 1, true);
    }
    state.flag_counts[4] = -1;
    for _ in 0..5 {
        step(&mut state, &rules, &[]);
    }
    assert_eq!(state.flag_leader, 1);
    assert_eq!(timer(&state), Some(120_000 - 5 * 50));
    let save = SaveGame {
        start: start_package(&rules, &setup),
        state: state.clone(),
        commands: Vec::new(),
    };
    let mut loaded = SaveGame::from_bytes(&save.to_bytes()).unwrap();
    loaded.check(&rules).unwrap();
    assert_eq!(loaded.state, state);
    assert_eq!(loaded.state.flag_counts[4], -1);
    for _ in 0..40 {
        step(&mut state, &rules, &[]);
        step(&mut loaded.state, &rules, &[]);
    }
    assert_eq!(checksum(&loaded.state), checksum(&state));
    assert_eq!(timer(&loaded.state), Some(120_000 - 45 * 50));
}
