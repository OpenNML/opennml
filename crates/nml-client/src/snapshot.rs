//! Screenshot helpers of the control channel (`screenshot`, `docs/viewer-control.md`, section "Screenshot") and the
//! conversion of the window mode into the types of the state snapshot (`nml_control`), without winit and GPU
//! (testable).

use image::RgbaImage;
use nml_control::{ScreenshotData, WindowModeName, base64};

use crate::window::WindowMode;

pub fn window_mode_name(mode: WindowMode) -> WindowModeName {
    match mode {
        WindowMode::Windowed => WindowModeName::Windowed,
        WindowMode::Borderless => WindowModeName::Borderless,
        WindowMode::Exclusive => WindowModeName::Exclusive,
    }
}

pub fn window_mode(name: WindowModeName) -> WindowMode {
    match name {
        WindowModeName::Windowed => WindowMode::Windowed,
        WindowModeName::Borderless => WindowMode::Borderless,
        WindowModeName::Exclusive => WindowMode::Exclusive,
    }
}

/// FNV-1a (64 bits) over `bytes`.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Size to which an image `size` is scaled down for a longest side of at most `max_size` (the aspect ratio stays,
/// every side at least 1).
pub fn fit_size(size: (u32, u32), max_size: Option<u32>) -> (u32, u32) {
    let longest = size.0.max(size.1);
    match max_size {
        Some(max) if longest > max => {
            let scale =
                |side: u32| ((u64::from(side) * u64::from(max)) / u64::from(longest)).max(1) as u32;
            (scale(size.0), scale(size.1))
        }
        _ => size,
    }
}

/// Screenshot as PNG in Base64 (result of `screenshot`): checksum over the full image, the PNG optionally scaled
/// down.
pub fn png_data(img: &RgbaImage, max_size: Option<u32>) -> Result<ScreenshotData, String> {
    let (width, height) = fit_size(img.dimensions(), max_size);
    let scaled;
    let png_image = if (width, height) == img.dimensions() {
        img
    } else {
        scaled = image::imageops::resize(img, width, height, image::imageops::FilterType::Triangle);
        &scaled
    };
    let mut png = Vec::new();
    png_image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("PNG not encodable: {e}"))?;
    Ok(ScreenshotData {
        width: img.width(),
        height: img.height(),
        rgba_hash: format!("{:016x}", fnv1a64(img.as_raw())),
        png_width: width,
        png_height: height,
        png_base64: base64::encode(&png),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshots_are_png_in_base64() {
        let img = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        let data = png_data(&img, None).unwrap();
        assert_eq!((data.width, data.height), (3, 2));
        assert_eq!((data.png_width, data.png_height), (3, 2));
        // PNG signature 89 50 4E 47 0D 0A 1A 0A.
        assert!(
            data.png_base64.starts_with("iVBORw0KGgo"),
            "{}",
            data.png_base64
        );
        assert_eq!(data.png_base64.len() % 4, 0);
    }

    #[test]
    fn checksum_covers_the_full_image_and_the_png_may_be_smaller() {
        // FNV-1a check values.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        let mut img = RgbaImage::from_pixel(400, 100, image::Rgba([1, 2, 3, 255]));
        let full = png_data(&img, None).unwrap();
        let small = png_data(&img, Some(100)).unwrap();
        assert_eq!((small.width, small.height), (400, 100));
        assert_eq!((small.png_width, small.png_height), (100, 25));
        assert_eq!(
            small.rgba_hash, full.rgba_hash,
            "checksum of the full image"
        );
        assert_eq!(full.rgba_hash.len(), 16);
        img.put_pixel(399, 99, image::Rgba([1, 2, 4, 255]));
        assert_ne!(png_data(&img, Some(100)).unwrap().rgba_hash, full.rgba_hash);
        // Larger than the image: unchanged; extreme aspect ratios: every side at least 1.
        assert_eq!(fit_size((400, 100), Some(1000)), (400, 100));
        assert_eq!(fit_size((3072, 1728), Some(1280)), (1280, 720));
        assert_eq!(fit_size((5000, 2), Some(100)), (100, 1));
    }

    #[test]
    fn window_mode_names_convert_both_ways() {
        for mode in [
            WindowMode::Windowed,
            WindowMode::Borderless,
            WindowMode::Exclusive,
        ] {
            assert_eq!(window_mode(window_mode_name(mode)), mode);
        }
    }
}
