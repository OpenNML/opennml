//! Window scaffold: window and GPU, window mode (windowed, borderless, exclusive) including monitor choice, size and
//! frame pacing (draw continuously, wait, or try occasionally when the window is occluded). Extracted from the viewer;
//! the window title stays with the host.

use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use opennml_render::{BackendChoice, FrameOutcome, Gpu};
use serde::Deserialize;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window};

use crate::display::{
    ModeInfo, best_mode_index, monitor_for_mode_change, next_monitor, pick_monitor,
};
use crate::frames::FrameCounter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    Windowed,
    #[default]
    Borderless,
    Exclusive,
}

impl WindowMode {
    /// Order for the F11 key.
    pub fn next(self) -> Self {
        match self {
            Self::Windowed => Self::Borderless,
            Self::Borderless => Self::Exclusive,
            Self::Exclusive => Self::Windowed,
        }
    }
}

impl FromStr for WindowMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "windowed" => Ok(Self::Windowed),
            "borderless" => Ok(Self::Borderless),
            "exclusive" => Ok(Self::Exclusive),
            other => Err(format!(
                "unknown window mode \"{other}\" (allowed: windowed, borderless, exclusive)"
            )),
        }
    }
}

/// Interval at which an occluded window (or one without a surface) is redrawn on a trial basis. Windows does not
/// report the end of the occlusion as an event.
const OCCLUDED_PROBE: Duration = Duration::from_millis(250);

/// How the event loop continues after processing the events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pacing {
    /// Redraw continuously (`Poll`).
    Continuous,
    /// Wait for events, do not draw (minimized or size 0).
    Wait,
    /// Occluded or without a surface: wait and only occasionally try a frame.
    Probe,
}

pub fn pacing(minimized: bool, occluded: bool) -> Pacing {
    if minimized {
        Pacing::Wait
    } else if occluded {
        Pacing::Probe
    } else {
        Pacing::Continuous
    }
}

/// Whether the frame could not be shown because the window is occluded or the surface is missing (then retry only
/// occasionally, see [`pacing`]).
pub fn blocks_redraw(outcome: FrameOutcome) -> bool {
    matches!(outcome, FrameOutcome::Occluded | FrameOutcome::NoSurface)
}

/// Window with GPU, window mode and monitor; created in `resumed` of the event loop.
pub struct WindowShell {
    pub window: Arc<Window>,
    pub gpu: Gpu,
    pub mode: WindowMode,
    pub monitor_index: usize,
    /// Monitor chosen by the user (valid index in the configuration/command line or F10).
    pub monitor_chosen: bool,
    /// Last frame reported "occluded" or "no surface".
    occluded: bool,
    /// Next draw attempt in the occluded state.
    next_probe: Instant,
}

impl WindowShell {
    /// Creates the window (`title`, inner size `size` in logical pixels) and the GPU and chooses the monitor:
    /// `monitor`, otherwise the window's, otherwise 0. The window mode `mode` takes effect only with
    /// [`WindowShell::apply_mode`].
    pub fn create(
        event_loop: &ActiveEventLoop,
        title: &str,
        size: (u32, u32),
        backend: BackendChoice,
        monitor: Option<usize>,
        mode: WindowMode,
    ) -> Result<Self> {
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(size.0, size.1));
        let window = Arc::new(event_loop.create_window(attributes)?);
        // The size of the icon depends on the scaling of the monitor, which only the window knows.
        crate::app_icon::apply(&window);
        let monitors: Vec<_> = event_loop.available_monitors().collect();
        let current = window
            .current_monitor()
            .and_then(|m| monitors.iter().position(|x| *x == m));
        let monitor_index = pick_monitor(monitors.len(), monitor, current).unwrap_or(0);
        // An invalid index is ignored and does not count as a choice.
        let monitor_chosen = monitor.is_some_and(|i| i < monitors.len());
        let inner = window.inner_size();
        let gpu = Gpu::new(window.clone(), (inner.width, inner.height), backend)?;
        Ok(Self {
            window,
            gpu,
            mode,
            monitor_index,
            monitor_chosen,
            occluded: false,
            next_probe: Instant::now(),
        })
    }

    /// Width for the fonts of the UI at start (O-58): inner window width, at least 1 (minimized window at start: no
    /// fonts with 0 px).
    pub fn ui_width(&self) -> u32 {
        self.window.inner_size().width.max(1)
    }

    fn monitor(&self, event_loop: &ActiveEventLoop) -> Option<MonitorHandle> {
        event_loop.available_monitors().nth(self.monitor_index)
    }

    /// Index of the monitor on which the window currently lies.
    fn window_monitor_index(&self, event_loop: &ActiveEventLoop) -> Option<usize> {
        let current = self.window.current_monitor()?;
        event_loop.available_monitors().position(|m| m == current)
    }

    /// Switches to the window mode `next`; when leaving windowed mode, the fullscreen monitor follows the window,
    /// unless a monitor was chosen. The caller renews the window title.
    pub fn change_mode(&mut self, event_loop: &ActiveEventLoop, next: WindowMode) {
        let leaving_windowed = self.mode == WindowMode::Windowed && next != WindowMode::Windowed;
        let window_monitor = self.window_monitor_index(event_loop);
        self.monitor_index = monitor_for_mode_change(
            self.monitor_index,
            self.monitor_chosen,
            leaving_windowed,
            window_monitor,
        );
        self.mode = next;
        self.apply_mode(event_loop);
    }

    /// Next window mode (F11).
    pub fn cycle_mode(&mut self, event_loop: &ActiveEventLoop) {
        self.change_mode(event_loop, self.mode.next());
    }

    /// Next monitor (F10); counts as chosen by the user.
    pub fn cycle_monitor(&mut self, event_loop: &ActiveEventLoop) {
        let count = event_loop.available_monitors().count();
        self.monitor_index = next_monitor(count, self.monitor_index);
        self.monitor_chosen = true;
        self.apply_mode(event_loop);
    }

    /// Control command `window`: monitor and/or window mode like F10/F11; a set monitor counts as chosen by the
    /// user.
    pub fn set_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        mode: Option<WindowMode>,
        monitor: Option<usize>,
    ) -> Result<(), String> {
        let count = event_loop.available_monitors().count();
        if let Some(index) = monitor {
            if index >= count {
                return Err(format!(
                    "monitor {index} does not exist ({count} monitors, counting from 0)"
                ));
            }
            self.monitor_index = index;
            self.monitor_chosen = true;
        }
        match mode {
            Some(mode) => self.change_mode(event_loop, mode),
            None => self.apply_mode(event_loop),
        }
        Ok(())
    }

    /// Sets window mode and monitor on the window (exclusive: best video mode, otherwise borderless).
    pub fn apply_mode(&mut self, event_loop: &ActiveEventLoop) {
        let monitor = self.monitor(event_loop);
        let window = &self.window;
        match self.mode {
            WindowMode::Windowed => {
                window.set_fullscreen(None);
                if let Some(m) = &monitor {
                    let (pos, size, outer) = (m.position(), m.size(), window.outer_size());
                    window.set_outer_position(PhysicalPosition::new(
                        pos.x + (size.width as i32 - outer.width as i32) / 2,
                        pos.y + (size.height as i32 - outer.height as i32) / 2,
                    ));
                }
            }
            WindowMode::Borderless => window.set_fullscreen(Some(Fullscreen::Borderless(monitor))),
            WindowMode::Exclusive => {
                let exclusive = monitor.as_ref().and_then(|m| {
                    let modes: Vec<_> = m.video_modes().collect();
                    let infos: Vec<ModeInfo> = modes
                        .iter()
                        .map(|v| ModeInfo {
                            width: v.size().width,
                            height: v.size().height,
                            refresh_millihertz: v.refresh_rate_millihertz(),
                            bit_depth: v.bit_depth(),
                        })
                        .collect();
                    best_mode_index(&infos).map(|i| Fullscreen::Exclusive(modes[i].clone()))
                });
                window.set_fullscreen(Some(exclusive.unwrap_or(Fullscreen::Borderless(monitor))));
            }
        }
    }

    /// End of a frame: when the window is occluded or the surface is missing, draw only occasionally, count the frame
    /// in `frames`. `true` if the window title should be renewed (new second, [`FrameCounter::count`]).
    pub fn finish_frame(&mut self, outcome: FrameOutcome, frames: &mut FrameCounter) -> bool {
        let blocked = blocks_redraw(outcome);
        if blocked && !self.occluded {
            self.next_probe = Instant::now() + OCCLUDED_PROBE;
        }
        self.occluded = blocked;
        frames.count(Instant::now(), outcome == FrameOutcome::Presented)
    }

    /// Last frame reported "occluded" or "no surface".
    pub fn occluded(&self) -> bool {
        self.occluded
    }

    /// Window became visible (focus, end of the occlusion): draw continuously again.
    pub fn shown(&mut self) {
        self.occluded = false;
    }

    /// New inner size (also after a DPI change): adjust the surface, draw continuously again.
    pub fn resized(&mut self, width: u32, height: u32) {
        self.occluded = false;
        self.gpu.resize(width, height);
    }

    /// Minimized or size 0: nothing is drawn then.
    pub fn minimized(&self) -> bool {
        let size = self.window.inner_size();
        self.window.is_minimized() == Some(true) || size.width == 0 || size.height == 0
    }

    /// Inner window size in pixels (mouse coordinates). The surface can be smaller than the window
    /// (`clamp_surface_size`): rendering uses the surface size, but the mouse pointer lies in window pixels (the image
    /// is stretched to the window).
    pub fn window_size(&self) -> (u32, u32) {
        let s = self.window.inner_size();
        (s.width, s.height)
    }

    /// Frame pacing after processing the events (`about_to_wait`): when minimized (or size 0), do not redraw and wait
    /// for events instead of saturating the CPU; when occluded or without a surface, wait likewise but try a frame at
    /// intervals.
    pub fn schedule(&mut self, event_loop: &ActiveEventLoop) {
        match pacing(self.minimized(), self.occluded) {
            Pacing::Continuous => {
                event_loop.set_control_flow(ControlFlow::Poll);
                self.window.request_redraw();
            }
            Pacing::Wait => event_loop.set_control_flow(ControlFlow::Wait),
            Pacing::Probe => {
                let now = Instant::now();
                if now >= self.next_probe {
                    self.window.request_redraw();
                    self.next_probe = now + OCCLUDED_PROBE;
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_probe));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_mode_cycles_and_parses() {
        assert_eq!(WindowMode::Windowed.next(), WindowMode::Borderless);
        assert_eq!(WindowMode::Borderless.next(), WindowMode::Exclusive);
        assert_eq!(WindowMode::Exclusive.next(), WindowMode::Windowed);
        assert_eq!("Borderless".parse(), Ok(WindowMode::Borderless));
        assert!("fullscreen".parse::<WindowMode>().is_err());
    }

    #[test]
    fn occluded_window_is_probed_instead_of_redrawn_continuously() {
        assert_eq!(pacing(false, false), Pacing::Continuous);
        assert_eq!(pacing(true, false), Pacing::Wait);
        assert_eq!(pacing(true, true), Pacing::Wait);
        assert_eq!(pacing(false, true), Pacing::Probe);
    }

    #[test]
    fn only_occluded_or_surfaceless_frames_block_redrawing() {
        assert!(blocks_redraw(FrameOutcome::Occluded));
        assert!(blocks_redraw(FrameOutcome::NoSurface));
        assert!(!blocks_redraw(FrameOutcome::Presented));
        assert!(!blocks_redraw(FrameOutcome::Skipped));
    }
}
