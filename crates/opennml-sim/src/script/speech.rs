//! Speech state of the mission (re/campaign-ui 4.1-4.4): running dialog line,
//! typing times of slots 0/1, the text events "text ended" and the info history.
//!
//! Times: duration of a dialog line (150, 213) = [`ScriptRules::speech_ms`] (sound on; the branch
//! "no sound device, length * 3/2" in `Dialog_PlaySpeech` is not reproduced), typing time =
//! [`ScriptRules::tip_ms`]. Event order as in E-173. Both lists are bounded in the reimplementation
//! ([`MAX_TEXT_EVENTS`], [`MAX_INFO_HISTORY`]): when appending to a full list the oldest
//! entry drops out and [`ScriptTrace::Limit`] is reported (never reached with the shipped data).
//!
//! [`ScriptRules::speech_ms`]: super::ScriptRules::speech_ms
//! [`ScriptRules::tip_ms`]: super::ScriptRules::tip_ms

use serde::{Deserialize, Serialize};

use super::present::{self, InfoFrameMemory, Presentation};
use super::{ScriptTrace, trace};
use crate::sim::Sim;
use crate::state::SimState;

/// Maximum number of text events (`m_SpeakingTextEvents`, unlimited in the original, E-173).
pub const MAX_TEXT_EVENTS: usize = 1024;
/// Maximum number of entries in the info history (`m_InfoHistory`, unlimited in the original).
pub const MAX_INFO_HISTORY: usize = 4096;

/// Running dialog line (`m_CurSpeakingText*`, `m_IsTextSpeaking`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechLine {
    /// Text ID of the line.
    pub text: i32,
    /// Remaining time in ms.
    pub remaining_ms: i32,
    /// Slot of the line (213 plays on `slot + 4`).
    pub slot: i32,
}

/// Speech state: line, tips of slots 0/1 (`m_ShowInfoTimer[2]`, `m_ShowInfoTextID[2]`) and the
/// text events in insertion order (`m_SpeakingTextEvents`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechState {
    pub line: Option<SpeechLine>,
    pub tip_ms: [i32; 2],
    pub tip_text: [i32; 2],
    pub events: Vec<i32>,
}

/// Query 600 (`FUN_00430B40`/`FUN_00432950`, "text ended"): removes the **first** entry
/// `text` from the text events; return value: it existed. Condition 19 and the continuation of
/// `w150`/`w183`/`w213` consume the event this way.
pub fn take_event(state: &mut SimState, text: i32) -> bool {
    let events = &mut state.script.mission.speech.events;
    match events.iter().position(|&e| e == text) {
        Some(i) => {
            events.remove(i);
            true
        }
        None => false,
    }
}

/// Appends `value` to `list` (`CList::AddTail`); if the list is full, the oldest entries drop out.
/// Return value: something dropped out.
fn push_bounded(list: &mut Vec<i32>, value: i32, max: usize) -> bool {
    let over = (list.len() + 1).saturating_sub(max);
    if over > 0 {
        list.drain(..over.min(list.len()));
    }
    list.push(value);
    over > 0
}

/// Append a text event at the end (limit [`MAX_TEXT_EVENTS`]).
fn push_event(sim: &mut Sim, text: i32) {
    let events = &mut sim.state.script.mission.speech.events;
    if push_bounded(events, text, MAX_TEXT_EVENTS) {
        trace(
            sim,
            ScriptTrace::Limit {
                what: format!("text events (at most {MAX_TEXT_EVENTS})"),
            },
        );
    }
}

/// Append a text to the info history (`m_InfoHistory`, limit [`MAX_INFO_HISTORY`]).
fn push_info(sim: &mut Sim, text: i32) {
    let history = &mut sim.state.script.mission.info_history;
    if push_bounded(history, text, MAX_INFO_HISTORY) {
        trace(
            sim,
            ScriptTrace::Limit {
                what: format!("info history (at most {MAX_INFO_HISTORY})"),
            },
        );
    }
}

/// `Dialog_PlaySpeech` (`0x00436A50`, action 150; 213 with `slot + 4`): if a line is running, its
/// text ID is appended at the end **without** first removing an equal entry (`AddTail`
/// `0x00436AB5`, E-173; the list can hold an ID twice). Then the new line is set with the
/// speech length and last the first entry of its text ID is removed (`FUN_00432950`).
/// The slot is not checked (only the UI shows slots < 4). Reports
/// `Presentation::Speech { text, slot, ms }` with the speech length; there is
/// no `SpeechEnd` for the interrupted line (E-240).
pub fn play_line(sim: &mut Sim, text: i32, slot: i32) {
    if let Some(old) = sim.state.script.mission.speech.line {
        push_event(sim, old.text);
    }
    let remaining_ms = sim
        .rules
        .scripts
        .speech_ms(text, sim.state.script.time_per_char);
    sim.state.script.mission.speech.line = Some(SpeechLine {
        text,
        remaining_ms,
        slot,
    });
    take_event(sim.state, text);
    // The interrupted line reports no end and does not close its text slot (E-240).
    present::emit(
        sim,
        Presentation::Speech {
            text,
            slot,
            ms: remaining_ms,
        },
    );
}

/// `FUN_0043E250` (action 183): only for slot 0 or 1 (otherwise nothing, no info history either) append
/// the text to the info history and set typing time and text ID of the slot. A running tip
/// in the slot is overwritten without a message (E-173: a `w183` on the old text waits forever).
pub fn show_tip(sim: &mut Sim, text: i32, slot: i32) {
    let Ok(index) = usize::try_from(slot) else {
        return;
    };
    if index >= 2 {
        return;
    }
    push_info(sim, text);
    let ms = sim
        .rules
        .scripts
        .tip_ms(text, sim.state.script.time_per_char);
    let speech = &mut sim.state.script.mission.speech;
    speech.tip_ms[index] = ms;
    speech.tip_text[index] = text;
    present::emit(sim, Presentation::Tip { text, slot, ms });
}

/// `FUN_00443A20` (action 213, info window): closes the windows of its own and of the running
/// text ID (unchecked, `InfoFrameClose` per call), appends the text to the info history, plays the
/// line on slot `pos + 4` ([`play_line`], reports `Speech`) and enters the speech window with
/// portrait and frame (`InfoSpeechFrame`). `pos` stays raw in the window: the host chooses the kind
/// from it (1 -> `0x5D`, otherwise `0x5C`); with `pos <= -1` the line is a text slot and the window
/// does not close at the speech end.
pub fn info_window(sim: &mut Sim, text: i32, pos: i32, portrait: i32, frame: i32) {
    present::close_info_frame(sim, text);
    if let Some(running) = sim.state.script.mission.speech.line {
        present::close_info_frame(sim, running.text);
    }
    push_info(sim, text);
    play_line(sim, text, pos.wrapping_add(4));
    present::open_info_frame(
        sim,
        text,
        InfoFrameMemory::Speech {
            slot: pos,
            portrait,
            frame,
        },
    );
    present::emit(
        sim,
        Presentation::InfoSpeechFrame {
            text,
            slot: pos,
            portrait,
            frame,
        },
    );
}

/// Tick of the speech state by `dt` ms: `FUN_00444C10` (running line: remaining time `-= dt`, at `< 1`
/// remove the first equal entry, append the text ID, end the line), then `FUN_00432B90` (per
/// tip slot 0, 1 with time `> 0`: `-= dt`, at `< 1` report likewise; the time then stays).
pub fn tick(sim: &mut Sim, dt: i32) {
    if let Some(mut line) = sim.state.script.mission.speech.line {
        line.remaining_ms = line.remaining_ms.wrapping_sub(dt);
        if line.remaining_ms < 1 {
            sim.state.script.mission.speech.line = None;
            take_event(sim.state, line.text);
            push_event(sim, line.text);
            // `FUN_00443450`: slot < 4 reports the end of the text slot, otherwise the window of this text
            // ID closes.
            if line.slot < 4 {
                present::emit(
                    sim,
                    Presentation::SpeechEnd {
                        text: line.text,
                        slot: line.slot,
                    },
                );
            } else {
                present::close_info_frame(sim, line.text);
            }
        } else {
            sim.state.script.mission.speech.line = Some(line);
        }
    }
    for index in 0..2 {
        let speech = &mut sim.state.script.mission.speech;
        if speech.tip_ms[index] <= 0 {
            continue;
        }
        speech.tip_ms[index] = speech.tip_ms[index].wrapping_sub(dt);
        if speech.tip_ms[index] < 1 {
            let text = speech.tip_text[index];
            take_event(sim.state, text);
            push_event(sim, text);
            present::emit(
                sim,
                Presentation::TipEnd {
                    text,
                    slot: index as i32,
                },
            );
        }
    }
}
