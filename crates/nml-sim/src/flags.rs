//! Flag leader of the game types Domination and King of the Hill (re/domination F.4):
//! `GameEngine_EvaluateFlagLeader` (`0x00445440`) with the leader
//! `m_FlagOwner` (`Engine+0x4E4`) and the timer 0, plus the mapping color -> slot
//! `FUN_00478DE0`. Integers only, slots 0-7 in order.

use crate::sim::Sim;
use crate::state::SimState;

/// Leader "nobody" (`m_FlagOwner` after the constructor `0x0043BB70`: `param_1[0x139] = 8`). Slot 8 is
/// also the counter slot of the neutral flags; an animal as the holder of the flag in King of the Hill sets the
/// leader to 8 as well (without a timer).
pub const NO_LEADER: i32 = 8;

/// ID of the leader timer: the same list and ID 0 as the time limit of game type 4
/// (`FUN_00445350(1, ms, 0, 0, {0, 0x28})`: kind 1 = number on the screen; the host draws the remaining time
/// at the top left, `FUN_00435EE0` part 2, `nml_ui::hud::countdown::timer_cmds`).
pub const LEADER_COUNTDOWN: i32 = crate::setup::TIME_LIMIT_COUNTDOWN;

/// `GameEngine_EvaluateFlagLeader(&m_FlagsCounter)` (`0x00445440`), called by the original after every
/// increment of a flag ([`crate::module::natur::adjust_flag_count`]), never on decrement.
///
/// - Game type 2 (Domination): the leader is the first slot 0-7 whose counter is more than half of the
///   sum of all nine counters (including the neutral ones) (integer division); the same leader keeps
///   its remaining time, a new one restarts the timer; if nobody has the majority and the leader is
///   not 8, the timer is dropped and the leader becomes 8.
/// - Game type 3 (King of the Hill): the leader is the first slot 0-8 with a counter != 0, otherwise 9
///   (unreachable, W-160); above 7 and leader != 8 the timer is dropped and the leader becomes 8; a
///   leader different from before restarts the timer.
/// - Other game types: nothing.
pub fn evaluate_flag_leader(sim: &mut Sim) {
    let state = &mut *sim.state;
    let z = state.flag_counts;
    match state.mode {
        2 => {
            let sum = z.iter().fold(0i32, |a, &c| a.wrapping_add(c));
            let half = sum / 2;
            for (i, &count) in z[..8].iter().enumerate() {
                if count > half {
                    if state.flag_leader == i as i32 {
                        return;
                    }
                    state.flag_leader = i as i32;
                    start_leader_countdown(state);
                    return;
                }
            }
            if state.flag_leader != NO_LEADER {
                state.countdowns.remove(&LEADER_COUNTDOWN);
                state.flag_leader = NO_LEADER;
            }
        }
        3 => {
            let first = z.iter().position(|&c| c != 0).map_or(9, |i| i as i32);
            if first > 7 && state.flag_leader != NO_LEADER {
                state.countdowns.remove(&LEADER_COUNTDOWN);
                state.flag_leader = NO_LEADER;
            }
            if state.flag_leader != first {
                state.flag_leader = first;
                start_leader_countdown(state);
            }
        }
        _ => {}
    }
}

/// `FUN_00445350` (`0x00445350`): replace the old timer of the same ID, create a new one with
/// `m_TimeLimit · 1000` ms (wrapped to 32 bits).
fn start_leader_countdown(state: &mut SimState) {
    let ms = state.time_limit.wrapping_mul(1000);
    state.countdowns.insert(LEADER_COUNTDOWN, ms);
}

/// `FUN_00478DE0(color)`: the first slot 0-7 whose player color is `color`; 8 if there is none. The
/// original does not check whether the slot is occupied (D-300): unoccupied slots keep color = slot.
pub fn slot_of_color(state: &SimState, color: i32) -> u8 {
    state.players[..8]
        .iter()
        .position(|p| p.color == color)
        .map_or(8, |i| i as u8)
}
