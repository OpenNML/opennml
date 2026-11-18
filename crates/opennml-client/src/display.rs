//! Monitor and video mode selection (without winit types, so it is testable).

/// Monitor index: the requested one, otherwise the current one, otherwise 0; `None` without monitors.
pub fn pick_monitor(
    count: usize,
    requested: Option<usize>,
    current: Option<usize>,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    requested
        .filter(|&i| i < count)
        .or(current.filter(|&i| i < count))
        .or(Some(0))
}

/// Next monitor (cyclic).
pub fn next_monitor(count: usize, current: usize) -> usize {
    if count == 0 { 0 } else { (current + 1) % count }
}

/// Monitor on a mode change: leaving windowed mode picks the monitor the window is currently on
/// (`window_monitor`), unless the user has chosen a monitor
/// (`chosen`: configuration, command line or F10). Otherwise `selected` stays.
pub fn monitor_for_mode_change(
    selected: usize,
    chosen: bool,
    leaving_windowed: bool,
    window_monitor: Option<usize>,
) -> usize {
    match window_monitor {
        Some(index) if leaving_windowed && !chosen => index,
        _ => selected,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeInfo {
    pub width: u32,
    pub height: u32,
    pub refresh_millihertz: u32,
    /// Color depth in bits per pixel.
    pub bit_depth: u16,
}

/// Best exclusive mode: 32-bit color depth (Windows often rejects 8/16-bit modes on a switch,
/// and winit then aborts in the debug build via `debug_assert`), then largest area (~ native
/// resolution), then highest refresh rate.
pub fn best_mode_index(modes: &[ModeInfo]) -> Option<usize> {
    modes
        .iter()
        .enumerate()
        .max_by_key(|(_, m)| {
            (
                m.bit_depth == 32,
                m.width as u64 * m.height as u64,
                m.refresh_millihertz,
            )
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_monitor_prefers_request_then_current() {
        assert_eq!(pick_monitor(3, Some(2), Some(1)), Some(2));
        assert_eq!(pick_monitor(3, Some(7), Some(1)), Some(1));
        assert_eq!(pick_monitor(3, None, None), Some(0));
        assert_eq!(pick_monitor(0, Some(0), None), None);
    }

    #[test]
    fn next_monitor_wraps() {
        assert_eq!(next_monitor(3, 2), 0);
        assert_eq!(next_monitor(3, 0), 1);
        assert_eq!(next_monitor(0, 0), 0);
    }

    #[test]
    fn best_mode_prefers_32_bit_then_area_then_refresh() {
        let m = |w, h, r, b| ModeInfo {
            width: w,
            height: h,
            refresh_millihertz: r,
            bit_depth: b,
        };
        let modes = [
            m(1920, 1080, 60_000, 32),
            m(2560, 1440, 60_000, 32),
            m(2560, 1440, 144_000, 32),
            m(1024, 768, 240_000, 32),
        ];
        assert_eq!(best_mode_index(&modes), Some(2));
        assert_eq!(best_mode_index(&[]), None);
        // 8/16-bit modes of the same size and rate fail (ChangeDisplaySettingsExW) -> 32 bit first.
        let modes = [
            m(3072, 1728, 60_000, 8),
            m(3072, 1728, 60_000, 32),
            m(3072, 1728, 60_000, 16),
            m(3840, 2160, 60_000, 16),
        ];
        assert_eq!(best_mode_index(&modes), Some(1));
        // Without a 32-bit mode: largest area.
        assert_eq!(
            best_mode_index(&[m(800, 600, 60_000, 16), m(1024, 768, 60_000, 16)]),
            Some(1)
        );
    }

    #[test]
    fn leaving_windowed_mode_follows_the_window_unless_user_chose() {
        // The window was dragged from monitor 0 to monitor 2, then F11.
        assert_eq!(monitor_for_mode_change(0, false, true, Some(2)), 2);
        // Monitor chosen via configuration/command line or F10: stays.
        assert_eq!(monitor_for_mode_change(0, true, true, Some(2)), 0);
        // Switch between borderless and exclusive: monitor stays.
        assert_eq!(monitor_for_mode_change(1, false, false, Some(2)), 1);
        // Monitor of the window unknown: the previous one.
        assert_eq!(monitor_for_mode_change(1, false, true, None), 1);
    }
}
