//! TGA textures. Only uncompressed variants occur in the game:
//! type 2 (RGB/RGBA, 24/32 bit) and type 3 (grayscale, 8 bit).

use crate::FormatError;

/// Decodes a TGA file to RGBA8 (origin top left).
pub fn decode_tga(bytes: &[u8]) -> Result<image::RgbaImage, FormatError> {
    Ok(image::load_from_memory_with_format(bytes, image::ImageFormat::Tga)?.to_rgba8())
}

/// Decodes a TGA file the way the game loads it (`TgaImage_Load` `0x005923A0`,
/// `TgaImage_Upload` `0x00591BF0`, see `docs/formats/tga.md`):
///
/// - only image types 2 and 3 without a color map, 8/24/32 bit, pixels from offset 18 (the image ID length
///   and descriptor are ignored);
/// - file row `r` becomes image row `H - 1 - r` (the origin bit does not count);
/// - 32 bit: byte 3 is alpha, even with 0 alpha bits in the descriptor; 24/8 bit: alpha 255.
pub fn decode_tga_game(bytes: &[u8]) -> Result<image::RgbaImage, FormatError> {
    const HEADER: usize = 18;
    if bytes.len() < HEADER {
        return Err(FormatError::Invalid("TGA header too short".into()));
    }
    let (colormap_type, image_type) = (bytes[1], bytes[2]);
    if colormap_type != 0 || !matches!(image_type, 2 | 3) {
        return Err(FormatError::Invalid(format!(
            "TGA image type {image_type} with color map {colormap_type} is not loaded by the game"
        )));
    }
    let width = u16::from_le_bytes([bytes[12], bytes[13]]) as usize;
    let height = u16::from_le_bytes([bytes[14], bytes[15]]) as usize;
    let bytes_per_pixel = match bytes[16] {
        32 => 4,
        24 => 3,
        8 => 1,
        other => {
            return Err(FormatError::Invalid(format!(
                "TGA with {other} bits per pixel is not loaded by the game"
            )));
        }
    };
    let needed = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or_else(|| FormatError::Invalid("TGA image size overflows".into()))?;
    let data = &bytes[HEADER..];
    if data.len() < needed {
        return Err(FormatError::Invalid(format!(
            "TGA pixel data too short: {} instead of {needed} bytes",
            data.len()
        )));
    }
    let mut img = image::RgbaImage::new(width as u32, height as u32);
    for row in 0..height {
        for x in 0..width {
            let i = (row * width + x) * bytes_per_pixel;
            let rgba = match bytes_per_pixel {
                4 => [data[i + 2], data[i + 1], data[i], data[i + 3]],
                3 => [data[i + 2], data[i + 1], data[i], 255],
                _ => [data[i], data[i], data[i], 255],
            };
            img.put_pixel(x as u32, (height - 1 - row) as u32, image::Rgba(rgba));
        }
    }
    Ok(img)
}

#[cfg(test)]
pub(crate) fn tga_header(
    image_type: u8,
    width: u16,
    height: u16,
    bpp: u8,
    top_left: bool,
) -> Vec<u8> {
    let mut h = vec![0u8; 18];
    h[2] = image_type;
    h[12..14].copy_from_slice(&width.to_le_bytes());
    h[14..16].copy_from_slice(&height.to_le_bytes());
    h[16] = bpp;
    let origin = if top_left { 0x20 } else { 0 };
    let alpha_bits = if bpp == 32 { 8 } else { 0 };
    h[17] = origin | alpha_bits;
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_24bit_bottom_left() {
        let mut bytes = tga_header(2, 1, 2, 24, false);
        bytes.extend_from_slice(&[0, 0, 255, 0, 255, 0]); // red at the bottom, green on top (BGR)
        let img = decode_tga(&bytes).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [0, 255, 0, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [255, 0, 0, 255]);
    }

    #[test]
    fn decodes_32bit_with_alpha() {
        let mut bytes = tga_header(2, 1, 1, 32, true);
        bytes.extend_from_slice(&[10, 20, 30, 40]);
        assert_eq!(
            decode_tga(&bytes).unwrap().get_pixel(0, 0).0,
            [30, 20, 10, 40]
        );
    }

    #[test]
    fn decodes_8bit_grayscale() {
        let mut bytes = tga_header(3, 1, 1, 8, false);
        bytes.push(128);
        assert_eq!(
            decode_tga(&bytes).unwrap().get_pixel(0, 0).0,
            [128, 128, 128, 255]
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            decode_tga(b"no image"),
            Err(FormatError::Image(_))
        ));
    }

    #[test]
    fn game_decoder_ignores_origin_flag_and_alpha_bits() {
        // Two rows of 1 pixel each, file bottom-left: red first (bottom), then green (top).
        let pixels = [0, 0, 255, 0, 255, 0];
        let mut bottom_left = tga_header(2, 1, 2, 24, false);
        bottom_left.extend_from_slice(&pixels);
        let img = decode_tga_game(&bottom_left).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [0, 255, 0, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [255, 0, 0, 255]);
        // The game does not evaluate the origin bit: same result.
        let mut top_left = tga_header(2, 1, 2, 24, true);
        top_left.extend_from_slice(&pixels);
        assert_eq!(decode_tga_game(&top_left).unwrap(), img);

        // 32 bit with 0 alpha bits in the descriptor: byte 3 is still alpha.
        let mut no_alpha_bits = tga_header(2, 1, 1, 32, false);
        no_alpha_bits[17] = 0;
        no_alpha_bits.extend_from_slice(&[10, 20, 30, 0]);
        assert_eq!(
            decode_tga_game(&no_alpha_bits).unwrap().get_pixel(0, 0).0,
            [30, 20, 10, 0]
        );
    }

    #[test]
    fn game_decoder_reads_pixels_at_offset_18_and_checks_types() {
        // The image ID length is ignored, as in the game.
        let mut with_id = tga_header(2, 1, 1, 24, false);
        with_id[0] = 3;
        with_id.extend_from_slice(&[1, 2, 3]);
        assert_eq!(
            decode_tga_game(&with_id).unwrap().get_pixel(0, 0).0,
            [3, 2, 1, 255]
        );

        let mut gray = tga_header(3, 1, 1, 8, false);
        gray.push(77);
        assert_eq!(
            decode_tga_game(&gray).unwrap().get_pixel(0, 0).0,
            [77, 77, 77, 255]
        );

        let mut rle = tga_header(10, 1, 1, 24, false);
        rle.extend_from_slice(&[0, 0, 0, 0]);
        assert!(matches!(
            decode_tga_game(&rle),
            Err(FormatError::Invalid(_))
        ));
        let mut short = tga_header(2, 2, 2, 32, false);
        short.extend_from_slice(&[0; 15]);
        assert!(matches!(
            decode_tga_game(&short),
            Err(FormatError::Invalid(_))
        ));
        assert!(matches!(
            decode_tga_game(&[0; 10]),
            Err(FormatError::Invalid(_))
        ));
    }
}
