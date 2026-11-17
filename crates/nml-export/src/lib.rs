//! Conversion of game data into standard formats: textures as PNG, models and levels as
//! glTF 2.0 (`.glb`, right-handed, z mirrored relative to the original).

pub mod glb;
mod level;
pub mod mirror;
mod model;

use std::path::Path;

pub use level::{LevelExportOptions, bake_chunk_texture, level_to_glb};
pub use model::model_to_glb;

use anyhow::{Context, Result};

/// sRGB value (0..1) to linear with the exact sRGB transfer curve. glTF defines `COLOR_0` and
/// `baseColorFactor` as linear, whereas the game computes in sRGB space.
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts a TGA file into a PNG file; creates missing directories.
pub fn tga_to_png(tga: &[u8], out: &Path) -> Result<()> {
    let img = nml_formats::tga::decode_tga(tga)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    img.save_with_format(out, image::ImageFormat::Png)
        .with_context(|| format!("writing {}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_to_linear_follows_the_transfer_curve() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_to_linear(0.5) - 0.214_041).abs() < 1e-5);
        // Linear segment near 0.
        assert!((srgb_to_linear(0.04) - 0.04 / 12.92).abs() < 1e-7);
    }

    #[test]
    fn writes_png_with_same_pixels() {
        let mut tga = vec![0u8; 18];
        tga[2] = 2;
        tga[12] = 1;
        tga[14] = 1;
        tga[16] = 24;
        tga.extend_from_slice(&[0, 0, 255]);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("a/b.png");
        tga_to_png(&tga, &out).unwrap();
        let png = image::open(&out).unwrap().to_rgba8();
        assert_eq!(png.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }
}
