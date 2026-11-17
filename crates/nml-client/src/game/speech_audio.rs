//! Speech output of the dialog lines and the tip sound (re/campaign-ui D6.7/D6.8): for the events `Speech`,
//! `SpeechEnd`, `InfoFrameClose` and `Tip` of the script presentation, the game host plays the sound of the original.
//! The simulation determines the speaking time (`ScriptRules::speech_ms`), never the sound (E-141/E-243): if the OGG is
//! missing, text and window run as usual, just silent.
//!
//! - `Dialog_PlaySpeech` (`0x00436A50`): if a line is running, it fades it out over 2000 ms and stops it; then
//!   `data/sprache/{text ID}.ogg` with flags `0x29` (rank 9), category 2 (speech volume control), 2D.
//! - End of a line (`FUN_00443450`): if the channel is still playing, it fades out over 1000 ms. A line from slot 4
//!   on (`n213` with slot + 4) reports its end as `InfoFrameClose{text}`, not as `SpeechEnd`.
//! - Closing an info window (`FUN_004401C0`: table search, destroy the window) never touches the channel. The same
//!   message `InfoFrameClose` also comes from `n169` and from the early closing of `n213`. Therefore
//!   [`SpeechAudio::on_event`] only notes a matching `InfoFrameClose` ([`SpeechAudio::flush`]): if `Speech` follows in
//!   the same frame, the old line fades out over 2000 ms (`Dialog_PlaySpeech`); if the line continues in the
//!   simulation (`n169`), the sound stays; only if the simulation has ended the line does it fade out over 1000 ms.
//! - Tip (`FUN_0043E250`): `SoundManager_PlayEvent2D("SFX_TEXT", 0x14, category 1)`.
//! - Pause and game menu do not stop the channel (`GameEngine_Pause` `0x0042D5E0`, `FUN_0042E150`; O-363): during
//!   the pause no events arrive, the sound fades away.
//!
//! Without a sound system (tests, game host without audio) the caller does not call [`SpeechAudio::on_event`] at all.

use nml_audio::manager::SoundId;
use nml_audio::volume::Category;
use nml_rda::Vfs;
use nml_sim::script::present::Presentation;
use nml_sim::state::SimEvent;

use crate::menu_host::MenuAudio;

/// Fade-out of the running line when a new one begins (`Dialog_PlaySpeech`, `FadeOutSound(id, 2000, 1)`).
pub const NEW_LINE_FADE_MS: i32 = 2000;
/// Fade-out at the end of a line (`FUN_00443450`) and when the session is replaced.
pub const LINE_END_FADE_MS: i32 = 1000;
/// Event of the tip sound.
pub const TIP_EVENT: &str = "SFX_TEXT";
/// Flags of the tip sound (rank 4, one instance; same as `notices::EVENT_2D_FLAGS`).
pub const TIP_FLAGS: u32 = 0x14;

/// Path of the speech file of a text ID (`data/sprache/%d.ogg`).
pub fn speech_path(text: i32) -> String {
    format!("data/sprache/{text}.ogg")
}

/// The running line: text ID and channel (`+0x25BC`/`+0x25B4` of the game engine).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeechAudio {
    current: Option<(i32, SoundId)>,
    /// An `InfoFrameClose` with the text of the running line has arrived; whether it was its end is decided by
    /// [`SpeechAudio::flush`].
    pending_end: bool,
}

impl SpeechAudio {
    /// The presentation events of a frame in sequence, then [`Self::flush`] with the text of the line that the
    /// simulation is still speaking after the frame (`line_running`). Each event takes effect exactly once.
    pub fn on_events(
        &mut self,
        events: &[SimEvent],
        line_running: Option<i32>,
        audio: &mut dyn MenuAudio,
        vfs: &Vfs,
    ) {
        for event in events {
            if let SimEvent::Present(p) = event {
                self.on_event(p, audio, vfs);
            }
        }
        self.flush(line_running, audio);
    }

    /// Applies a presentation event; other events have no sound.
    pub fn on_event(&mut self, event: &Presentation, audio: &mut dyn MenuAudio, vfs: &Vfs) {
        match event {
            Presentation::Speech { text, .. } => {
                self.pending_end = false;
                if let Some((_, id)) = self.current.take() {
                    audio.fade_out_sound(id, NEW_LINE_FADE_MS);
                }
                // If the file is missing: silent (the original gets -1), the speaking time stays that of the
                // simulation.
                self.current = audio
                    .play_speech(vfs, &speech_path(*text))
                    .map(|id| (*text, id));
            }
            Presentation::SpeechEnd { .. } => self.end_line(audio),
            Presentation::InfoFrameClose { text } if self.current_text() == Some(*text) => {
                self.pending_end = true;
            }
            Presentation::Tip { .. } => {
                audio.play_event_2d(vfs, TIP_EVENT, TIP_FLAGS, Category::Sfx, false);
            }
            _ => {}
        }
    }

    /// Decides at the end of a frame about a noted `InfoFrameClose`: if the simulation is still speaking the line
    /// (`line_running` = its text), it was `n169` or the early closing of `n213` and the sound continues; otherwise
    /// it was the end of the line (`FUN_00443450`, 1000 ms). If `Speech` followed, nothing is noted any more.
    pub fn flush(&mut self, line_running: Option<i32>, audio: &mut dyn MenuAudio) {
        if !std::mem::take(&mut self.pending_end) {
            return;
        }
        if line_running.is_some() && line_running == self.current_text() {
            return;
        }
        self.end_line(audio);
    }

    /// The session is being replaced (loading, restart, end of the match): the running line fades out instead of
    /// sounding over the new level (the original ends the engine along with the sound). No line afterwards.
    pub fn stop(&mut self, audio: &mut dyn MenuAudio) {
        self.end_line(audio);
    }

    /// Forgets the line without sound (loading sets `current = None`, deviation from the original).
    pub fn forget(&mut self) {
        self.current = None;
        self.pending_end = false;
    }

    /// Text ID of the running line.
    pub fn current_text(&self) -> Option<i32> {
        self.current.map(|(text, _)| text)
    }

    /// `FUN_00443450`: if the channel is still playing, it fades out over 1000 ms.
    fn end_line(&mut self, audio: &mut dyn MenuAudio) {
        self.pending_end = false;
        if let Some((_, id)) = self.current.take()
            && audio.speech_playing(id)
        {
            audio.fade_out_sound(id, LINE_END_FADE_MS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeAudio, world};

    fn speech(text: i32, slot: i32) -> Presentation {
        Presentation::Speech {
            text,
            slot,
            ms: 3000,
        }
    }

    /// New line: the old one fades out over 2000 ms and stops, then `data/sprache/{text}.ogg` plays.
    #[test]
    fn new_line_fades_the_old_one_over_2000_ms() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.on_event(&speech(120, 0), &mut audio, &vfs);
        assert_eq!(audio.calls, ["speech data/sprache/120.ogg"]);
        speech_audio.on_event(&speech(121, 1), &mut audio, &vfs);
        assert_eq!(
            audio.calls,
            [
                "speech data/sprache/120.ogg",
                "fade-out 42 2000",
                "speech data/sprache/121.ogg"
            ]
        );
        assert_eq!(speech_audio.current_text(), Some(121));
    }

    /// End of a line: if the channel is still playing, it fades out over 1000 ms; a second end does nothing.
    #[test]
    fn line_end_fades_over_1000_ms() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.on_event(&speech(120, 0), &mut audio, &vfs);
        let end = Presentation::SpeechEnd { text: 120, slot: 0 };
        speech_audio.on_event(&end, &mut audio, &vfs);
        assert_eq!(audio.calls.last().unwrap(), "fade-out 42 1000");
        assert_eq!(audio.calls.len(), 2);
        speech_audio.on_event(&end, &mut audio, &vfs);
        assert_eq!(audio.calls.len(), 2, "no line any more");
        assert_eq!(speech_audio.current_text(), None);
    }

    /// If the file has already died away, there is nothing to fade out.
    #[test]
    fn line_end_after_the_file_ended_is_silent() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.on_event(&speech(120, 0), &mut audio, &vfs);
        audio.speech_running = false;
        let end = Presentation::SpeechEnd { text: 120, slot: 0 };
        speech_audio.on_event(&end, &mut audio, &vfs);
        assert_eq!(audio.calls, ["speech data/sprache/120.ogg"]);
    }

    /// Missing OGG (in the shipped data `24021 .ogg` with a space, never found): silent, no line, no error; an end
    /// afterwards does nothing, a line before it fades out anyway.
    #[test]
    fn missing_ogg_is_silent() {
        let (_dir, vfs) = world();
        let mut audio = FakeAudio::without(&["data/sprache/24021.ogg"]);
        let mut speech_audio = SpeechAudio::default();
        speech_audio.on_event(&speech(24021, 0), &mut audio, &vfs);
        assert_eq!(speech_audio.current_text(), None);
        let end = Presentation::SpeechEnd {
            text: 24021,
            slot: 0,
        };
        speech_audio.on_event(&end, &mut audio, &vfs);
        assert_eq!(audio.calls, ["speech data/sprache/24021.ogg"]);

        speech_audio.on_event(&speech(120, 0), &mut audio, &vfs);
        assert_eq!(speech_audio.current_text(), Some(120));
        speech_audio.on_event(&speech(24021, 1), &mut audio, &vfs);
        assert_eq!(speech_audio.current_text(), None);
        assert!(audio.calls.contains(&"fade-out 42 2000".to_string()));
    }

    fn present(p: Presentation) -> SimEvent {
        SimEvent::Present(p)
    }

    /// `n213` (slot + 4) ends with `InfoFrameClose{text}` (`FUN_00443450` closes the window afterwards): if the line
    /// has ended in the simulation, it fades out over 1000 ms; the window of a foreign text ID does nothing.
    #[test]
    fn speech_frame_213_fades_with_its_info_frame() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.on_event(&speech(302, 4), &mut audio, &vfs);
        let other = [present(Presentation::InfoFrameClose { text: 999 })];
        speech_audio.on_events(&other, None, &mut audio, &vfs);
        assert_eq!(audio.calls.len(), 1, "foreign window: nothing");
        let own = [present(Presentation::InfoFrameClose { text: 302 })];
        speech_audio.on_events(&own, None, &mut audio, &vfs);
        assert_eq!(audio.calls.last().unwrap(), "fade-out 42 1000");
        assert_eq!(speech_audio.current_text(), None);
    }

    /// `n213` interrupts a line: `info_window` reports `InfoFrameClose{B}`, `InfoFrameClose{A}` (the running one),
    /// then `Speech{B}`. Closing does not touch the channel (`FUN_004401C0`); `Dialog_PlaySpeech`
    /// (`0x00436A50`) fades A out exactly once over 2000 ms.
    #[test]
    fn interrupting_213_fades_the_old_line_once_over_2000_ms() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.on_event(&speech(301, 4), &mut audio, &vfs);
        let events = [
            present(Presentation::InfoFrameClose { text: 302 }),
            present(Presentation::InfoFrameClose { text: 301 }),
            present(speech(302, 4)),
            present(Presentation::InfoSpeechFrame {
                text: 302,
                slot: 0,
                portrait: 5,
                frame: 6,
            }),
        ];
        speech_audio.on_events(&events, Some(302), &mut audio, &vfs);
        assert_eq!(
            audio.calls,
            [
                "speech data/sprache/301.ogg",
                "fade-out 42 2000",
                "speech data/sprache/302.ogg"
            ]
        );
        assert_eq!(speech_audio.current_text(), Some(302));
    }

    /// `n169` on the window of a running 213 line closes only the window: the sound continues, and the real end of
    /// the line fades out later.
    #[test]
    fn closing_the_window_of_a_running_line_keeps_the_sound() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.on_event(&speech(302, 4), &mut audio, &vfs);
        let close = [present(Presentation::InfoFrameClose { text: 302 })];
        speech_audio.on_events(&close, Some(302), &mut audio, &vfs);
        assert_eq!(audio.calls.len(), 1, "no fade-out");
        assert_eq!(speech_audio.current_text(), Some(302));
        // Later end in the simulation.
        speech_audio.on_events(&close, None, &mut audio, &vfs);
        assert_eq!(audio.calls.last().unwrap(), "fade-out 42 1000");
    }

    /// Tip: `SFX_TEXT` 2D, flags `0x14`, category 1 (sound effects), without loop; a running line stays.
    #[test]
    fn tip_plays_sfx_text_2d_category_1() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        let tip = Presentation::Tip {
            text: 5,
            slot: 0,
            ms: 4000,
        };
        speech_audio.on_event(&tip, &mut audio, &vfs);
        assert_eq!(audio.calls, ["event SFX_TEXT 0x14 sfx false"]);
        assert_eq!(speech_audio.current_text(), None);
        speech_audio.on_event(&speech(1, 0), &mut audio, &vfs);
        speech_audio.on_event(&tip, &mut audio, &vfs);
        assert_eq!(speech_audio.current_text(), Some(1));
    }

    /// A frame without presentation events (pause and game menu generate none, O-363) leaves the line untouched:
    /// no fade-out, no stop. The effect at session level is checked by `game_session::tests::paused_session_...`.
    #[test]
    fn a_frame_without_events_leaves_the_line_alone() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.on_event(&speech(120, 0), &mut audio, &vfs);
        let calls = audio.calls.len();
        speech_audio.on_events(&[], Some(120), &mut audio, &vfs);
        assert!(audio.speech_playing(42));
        assert_eq!(audio.calls.len(), calls);
        assert_eq!(speech_audio.current_text(), Some(120));
        let end = Presentation::SpeechEnd { text: 120, slot: 0 };
        speech_audio.on_event(&end, &mut audio, &vfs);
        assert_eq!(audio.calls.last().unwrap(), "fade-out 42 1000");
    }

    /// Loading: `forget` forgets the line without sound (deviation from the original); an end afterwards does nothing.
    #[test]
    fn loading_starts_no_speech() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.on_event(&speech(120, 0), &mut audio, &vfs);
        speech_audio.forget();
        assert_eq!(speech_audio.current_text(), None);
        let end = Presentation::SpeechEnd { text: 120, slot: 0 };
        speech_audio.on_event(&end, &mut audio, &vfs);
        assert_eq!(audio.calls, ["speech data/sprache/120.ogg"]);
    }

    /// Replacing the session fades out the running line over 1000 ms.
    #[test]
    fn replacing_the_session_fades_the_running_line() {
        let (_dir, vfs) = world();
        let (mut audio, mut speech_audio) = (FakeAudio::default(), SpeechAudio::default());
        speech_audio.stop(&mut audio);
        assert!(audio.calls.is_empty());
        speech_audio.on_event(&speech(120, 0), &mut audio, &vfs);
        speech_audio.stop(&mut audio);
        assert_eq!(audio.calls.last().unwrap(), "fade-out 42 1000");
        assert_eq!(speech_audio.current_text(), None);
    }
}
