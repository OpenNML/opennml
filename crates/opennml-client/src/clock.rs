//! Virtual clock of the viewer (without winit and GPU, testable): one time for map, browser and
//! camera. In real-time operation it follows real time (frame time capped), when paused it
//! stands still, and the control channel can advance it in fixed steps (`docs/viewer-control.md`).

use std::time::{Duration, Instant};

/// Upper limit of the frame time for the camera control (ms). Deviation from the original (which
/// computes per frame): protection against jumps after minimizing or hangs, when the first frame
/// would otherwise receive the whole pause time as one step.
pub const MAX_FRAME_MS: f32 = 100.0;

/// Frame time in milliseconds, limited to [`MAX_FRAME_MS`].
pub fn frame_time_ms(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() * 1000.0).min(MAX_FRAME_MS)
}

/// Reads `--time-ms` (viewer, `opennml-game`): a finite number >= 0 (infinity or NaN would give no position).
pub fn parse_time_ms(text: &str) -> Result<f32, String> {
    let ms: f32 = text
        .trim()
        .parse()
        .map_err(|_| format!("\"{text}\" is not a number (milliseconds expected)"))?;
    if !ms.is_finite() || ms < 0.0 {
        return Err(format!(
            "time {text} ms invalid: a finite number >= 0 is required"
        ));
    }
    Ok(ms)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockMode {
    /// Follows real time (always, without `--control`).
    Realtime,
    /// Stands still; frames are still drawn, but nothing moves.
    Paused,
}

#[derive(Debug, Clone)]
pub struct Clock {
    now_ms: f64,
    mode: ClockMode,
    /// Real time of the last frame; `None` before the first frame and after `resume`.
    last_frame: Option<Instant>,
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    pub fn new() -> Self {
        Self {
            now_ms: 0.0,
            mode: ClockMode::Realtime,
            last_frame: None,
        }
    }

    /// Clock that starts paused at 0 ms (`--paused`): state and frames of a fresh
    /// session thus do not depend on the time until the first control command.
    pub fn paused() -> Self {
        Self {
            mode: ClockMode::Paused,
            ..Self::new()
        }
    }

    /// Virtual time since the start in milliseconds.
    pub fn now_ms(&self) -> f64 {
        self.now_ms
    }

    /// [`Clock::now_ms`] in the precision of the consumers (animation, water, browser).
    pub fn now_f32(&self) -> f32 {
        self.now_ms as f32
    }

    pub fn mode(&self) -> ClockMode {
        self.mode
    }

    /// Frame time of a real-time frame at the real time `now`: time since the last frame,
    /// capped like [`frame_time_ms`]; the first frame (also after `resume`) and every frame in the
    /// paused state give 0. Does not advance the clock (see [`Clock::advance`]).
    pub fn tick(&mut self, now: Instant) -> f32 {
        let dt = match (self.mode, self.last_frame) {
            (ClockMode::Realtime, Some(last)) => frame_time_ms(now.saturating_duration_since(last)),
            _ => 0.0,
        };
        self.last_frame = Some(now);
        dt
    }

    /// Advances the clock by `dt_ms` (one logic step).
    pub fn advance(&mut self, dt_ms: f32) {
        self.now_ms += f64::from(dt_ms);
    }

    pub fn pause(&mut self) {
        self.mode = ClockMode::Paused;
    }

    /// Continues in real time; the time during which the clock stood still is not made up.
    pub fn resume(&mut self) {
        if self.mode == ClockMode::Paused {
            self.mode = ClockMode::Realtime;
            self.last_frame = None;
        }
    }
}

/// Time base of the network in `opennml-game`: the program clock ([`Clock::now_ms`]) plus the real time during which
/// the window was minimized. Minimized there is no logic step, so the program clock stands still (no catching up after
/// restoring either, [`MAX_FRAME_MS`]); the network keeps its own time running in that phase so that latency probes,
/// search queries, list expiry and deadlines go on (O-465). The offset only grows, so the network time stays monotonic
/// across minimizing and restoring. With the clock halted (control channel) no time is added.
#[derive(Debug, Clone, Default)]
pub struct NetClock {
    /// Real time of all minimized phases so far, in whole milliseconds.
    idle_ms: u64,
    /// Real time of the previous pass of the event loop while minimized; `None` while frames are drawn.
    idle_since: Option<Instant>,
}

impl NetClock {
    /// A pass of the event loop while minimized at the real time `now`: the real time since the previous such pass
    /// counts for the network (only with the clock in real time).
    pub fn idle(&mut self, now: Instant, clock: &Clock) {
        if let Some(last) = self.idle_since.replace(now)
            && clock.mode() == ClockMode::Realtime
        {
            let ms = now.saturating_duration_since(last).as_millis();
            self.idle_ms = self
                .idle_ms
                .saturating_add(u64::try_from(ms).unwrap_or(u64::MAX));
        }
    }

    /// Frames are drawn again: the program clock runs on, the next minimized phase starts anew.
    pub fn shown(&mut self) {
        self.idle_since = None;
    }

    /// Real time of the minimized phases in ms, the offset of the network time to the program clock.
    pub fn offset_ms(&self) -> u64 {
        self.idle_ms
    }

    /// Network time in ms: the program clock in whole milliseconds plus [`Self::offset_ms`].
    pub fn now_ms(&self, clock: &Clock) -> u64 {
        // `as` saturates; the clock is >= 0.
        (clock.now_ms() as u64).saturating_add(self.idle_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimized for 40 s: the program clock stands, the network time follows the real time; after restoring the
    /// first frame catches nothing up and the network time never goes back.
    #[test]
    fn net_clock_runs_while_minimized_without_moving_the_program_clock() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut clock = Clock::new();
        let mut net = NetClock::default();
        clock.tick(at(0));
        let dt = clock.tick(at(16));
        clock.advance(dt);
        assert_eq!(net.now_ms(&clock), 16);
        // Minimized from 20 ms on: passes every 50 ms, no frame.
        let mut last = net.now_ms(&clock);
        for k in 0..=800 {
            net.idle(at(20 + 50 * k), &clock);
            let now = net.now_ms(&clock);
            assert!(now >= last, "monotonic");
            last = now;
        }
        assert_eq!(clock.now_ms(), 16.0, "no logic step while minimized");
        assert_eq!(net.offset_ms(), 40_000);
        assert_eq!(net.now_ms(&clock), 40_016);
        // Restored: the first frame gets at most `MAX_FRAME_MS`.
        net.shown();
        let dt = clock.tick(at(40_030));
        assert_eq!(dt, MAX_FRAME_MS, "no catching up");
        clock.advance(dt);
        assert!(net.now_ms(&clock) > last, "monotonic after restoring");
        assert_eq!(net.now_ms(&clock), 40_116);
        // The next minimized phase starts anew (the frames in between are not counted twice).
        net.idle(at(50_000), &clock);
        assert_eq!(net.offset_ms(), 40_000);
        net.idle(at(50_050), &clock);
        assert_eq!(net.offset_ms(), 40_050);
    }

    #[test]
    fn net_clock_adds_nothing_with_the_clock_halted() {
        let start = Instant::now();
        let clock = Clock::paused();
        let mut net = NetClock::default();
        net.idle(start, &clock);
        net.idle(start + Duration::from_secs(5), &clock);
        assert_eq!(net.now_ms(&clock), 0);
    }

    #[test]
    fn realtime_follows_real_time_with_a_cap() {
        let start = Instant::now();
        let mut clock = Clock::new();
        assert_eq!(clock.tick(start), 0.0, "first frame");
        let dt = clock.tick(start + Duration::from_millis(16));
        assert_eq!(dt, 16.0);
        clock.advance(dt);
        let dt = clock.tick(start + Duration::from_secs(60));
        assert_eq!(dt, MAX_FRAME_MS);
        clock.advance(dt);
        assert_eq!(clock.now_ms(), 116.0);
        assert_eq!(clock.mode(), ClockMode::Realtime);
    }

    #[test]
    fn paused_clock_gives_zero_frame_time_and_resumes_without_a_jump() {
        let start = Instant::now();
        let mut clock = Clock::new();
        clock.tick(start);
        clock.pause();
        assert_eq!(clock.tick(start + Duration::from_millis(50)), 0.0);
        assert_eq!(clock.now_ms(), 0.0);
        clock.resume();
        assert_eq!(clock.mode(), ClockMode::Realtime);
        assert_eq!(
            clock.tick(start + Duration::from_secs(5)),
            0.0,
            "no catch-up time"
        );
        assert_eq!(clock.tick(start + Duration::from_millis(5020)), 20.0);
    }

    #[test]
    fn a_clock_created_paused_stays_at_zero_and_resumes_without_a_jump() {
        let start = Instant::now();
        let mut clock = Clock::paused();
        assert_eq!(clock.mode(), ClockMode::Paused);
        assert_eq!(clock.now_ms(), 0.0);
        assert_eq!(clock.tick(start), 0.0);
        assert_eq!(clock.tick(start + Duration::from_secs(3)), 0.0);
        assert_eq!(clock.now_ms(), 0.0);
        clock.resume();
        assert_eq!(clock.mode(), ClockMode::Realtime);
        assert_eq!(
            clock.tick(start + Duration::from_secs(9)),
            0.0,
            "no catch-up time"
        );
        assert_eq!(clock.tick(start + Duration::from_millis(9016)), 16.0);
    }

    #[test]
    fn steps_advance_by_a_fixed_frame_time_and_keep_the_clock_paused() {
        let mut clock = Clock::new();
        clock.pause();
        for _ in 0..30 {
            clock.advance(16.5);
        }
        assert_eq!(clock.now_ms(), 495.0);
        assert_eq!(clock.now_f32(), 495.0);
        assert_eq!(clock.mode(), ClockMode::Paused);
        assert_eq!(clock.tick(Instant::now()), 0.0);
    }
}
