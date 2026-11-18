//! Program icon of OpenNML for the window title and taskbar. The PNGs are our own artwork from `assets/icon/`
//! (generated with `opennml-icon`, see `assets/icon/README.md`) and are embedded in the program; original game data
//! plays no part. The icon of the exe in Explorer is embedded by the `build.rs` of `opennml-game` and `opennml-viewer`.

use winit::window::{Icon, Window};

/// Embedded sizes in pixels with their PNG data, ascending.
const PNGS: [(u32, &[u8]); 6] = [
    (16, include_bytes!("../../../assets/icon/opennml-16.png")),
    (24, include_bytes!("../../../assets/icon/opennml-24.png")),
    (32, include_bytes!("../../../assets/icon/opennml-32.png")),
    (48, include_bytes!("../../../assets/icon/opennml-48.png")),
    (64, include_bytes!("../../../assets/icon/opennml-64.png")),
    (128, include_bytes!("../../../assets/icon/opennml-128.png")),
];

/// Smallest icon in the title bar at scale 1 (Windows `SM_CXSMICON`).
const SMALL: f64 = 16.0;
/// Large icon (taskbar, Alt+Tab) at scale 1 (Windows `SM_CXICON`).
const LARGE: f64 = 32.0;

/// Smallest embedded size that reaches `want` pixels, otherwise the largest. The small sizes are separate,
/// simplified versions; Windows should avoid scaling them from another size.
fn pick(want: u32) -> (u32, &'static [u8]) {
    PNGS.iter()
        .copied()
        .find(|&(size, _)| size >= want)
        .unwrap_or(PNGS[PNGS.len() - 1])
}

/// Icon for a side length of `logical` pixels at the scale `scale`.
fn icon(logical: f64, scale: f64) -> Option<Icon> {
    let want = (logical * scale).round().clamp(1.0, f64::from(u32::MAX)) as u32;
    let (size, png) = pick(want);
    let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| log::warn!("Program icon {size} px not readable: {e}"))
        .ok()?
        .to_rgba8();
    let (w, h) = decoded.dimensions();
    Icon::from_rgba(decoded.into_raw(), w, h)
        .map_err(|e| log::warn!("Program icon {size} px not usable: {e}"))
        .ok()
}

/// Sets the icon in the title bar and (on Windows) the taskbar to match the window's scale. An error leaves the
/// default icon in place and is only reported.
pub fn apply(window: &Window) {
    let scale = window.scale_factor();
    window.set_window_icon(icon(SMALL, scale));
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowExtWindows;
        window.set_taskbar_icon(icon(LARGE, scale));
    }
    #[cfg(not(windows))]
    let _ = LARGE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_pngs_have_their_size() {
        for (size, png) in PNGS {
            let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap();
            assert_eq!((img.width(), img.height()), (size, size));
        }
    }

    #[test]
    fn pick_takes_smallest_fitting_size() {
        assert_eq!(pick(16).0, 16);
        assert_eq!(pick(20).0, 24); // 125%
        assert_eq!(pick(24).0, 24);
        assert_eq!(pick(40).0, 48); // large icon at 125%
        assert_eq!(pick(96).0, 128);
        assert_eq!(pick(500).0, 128);
    }

    #[test]
    fn icon_builds_for_common_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            assert!(icon(SMALL, scale).is_some());
            assert!(icon(LARGE, scale).is_some());
        }
    }
}
