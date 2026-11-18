//! Measurements of the frames in real time (independent of the virtual clock): frames per second
//! as in the window title and the frame times of the most recently shown frames.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use opennml_control::{FrameStats, FrameTimes};

/// Number of remembered frame times.
pub const FRAME_HISTORY: usize = 120;

#[derive(Debug, Clone)]
pub struct FrameCounter {
    /// Frames since `fps_since` (all completed ones, as in the title so far).
    frames: u32,
    fps: u32,
    fps_since: Instant,
    /// Time of the last shown frame.
    last_shown: Option<Instant>,
    /// Real times between shown frames (ms), at most [`FRAME_HISTORY`].
    times: VecDeque<f64>,
    shown: u64,
}

impl FrameCounter {
    pub fn new(now: Instant) -> Self {
        Self {
            frames: 0,
            fps: 0,
            fps_since: now,
            last_shown: None,
            times: VecDeque::with_capacity(FRAME_HISTORY),
            shown: 0,
        }
    }

    /// A frame is completed (`shown`: also shown). `true` if a new second
    /// has begun and [`FrameCounter::fps`] is new (then renew the window title).
    pub fn count(&mut self, now: Instant, shown: bool) -> bool {
        self.frames += 1;
        if shown {
            if let Some(last) = self.last_shown {
                if self.times.len() == FRAME_HISTORY {
                    self.times.pop_front();
                }
                self.times
                    .push_back(now.saturating_duration_since(last).as_secs_f64() * 1000.0);
            }
            self.last_shown = Some(now);
            self.shown += 1;
        }
        if now.saturating_duration_since(self.fps_since) >= Duration::from_secs(1) {
            self.fps = self.frames;
            self.frames = 0;
            self.fps_since = now;
            return true;
        }
        false
    }

    /// Frames of the last full second.
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Measurements for the state snapshot.
    pub fn stats(&self) -> FrameStats {
        let frame_times = (!self.times.is_empty()).then(|| {
            let (mut min, mut max, mut sum) = (f64::INFINITY, 0.0_f64, 0.0);
            for &t in &self.times {
                min = min.min(t);
                max = max.max(t);
                sum += t;
            }
            FrameTimes {
                count: self.times.len(),
                min_ms: min,
                mean_ms: sum / self.times.len() as f64,
                max_ms: max,
            }
        });
        FrameStats {
            fps: self.fps,
            frame_times,
            presented: self.shown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_counts_all_frames_of_the_last_second() {
        let start = Instant::now();
        let mut counter = FrameCounter::new(start);
        for i in 1..=59 {
            assert!(!counter.count(start + Duration::from_millis(i * 16), i % 2 == 0));
        }
        assert!(counter.count(start + Duration::from_millis(1000), false));
        assert_eq!(counter.fps(), 60);
        assert!(!counter.count(start + Duration::from_millis(1010), true));
        assert_eq!(counter.fps(), 60, "until the next full second");
    }

    #[test]
    fn frame_times_cover_the_last_120_shown_frames() {
        let start = Instant::now();
        let mut counter = FrameCounter::new(start);
        assert_eq!(counter.stats().frame_times, None);
        counter.count(start, true);
        assert_eq!(
            counter.stats().frame_times,
            None,
            "one frame has no frame time"
        );
        // Frames not shown do not count; the time runs until the next shown one.
        counter.count(start + Duration::from_millis(5), false);
        counter.count(start + Duration::from_millis(10), true);
        counter.count(start + Duration::from_millis(30), true);
        let stats = counter.stats();
        assert_eq!(stats.presented, 3);
        assert_eq!(
            stats.frame_times,
            Some(FrameTimes {
                count: 2,
                min_ms: 10.0,
                mean_ms: 15.0,
                max_ms: 20.0
            })
        );
        let mut t = start + Duration::from_millis(30);
        for _ in 0..200 {
            t += Duration::from_millis(7);
            counter.count(t, true);
        }
        let times = counter.stats().frame_times.unwrap();
        assert_eq!(times.count, FRAME_HISTORY);
        assert!((times.max_ms - 7.0).abs() < 1e-9, "old values displaced");
        assert_eq!(counter.stats().presented, 203);
    }
}
