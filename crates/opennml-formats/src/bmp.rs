//! BMP textures as the game loads them (`Renderer_LoadTextureFile` `0x00573220`, `.bmp` branch): `FUN_00592F00`
//! reads the header and pixels, `FUN_00592880` uploads them. The original loads two BMP textures: the one for the
//! health bar `data/textures/selektion/balken.bmp` (32 x 32, 24 bit; the reimplementation loads it as well) and, at
//! `TerrainQuality` 0, `Data/Textures/Detail/Detail.bmp` (256 x 256, 24 bit; not loaded in the reimplementation).

use crate::FormatError;

/// Length of the file header (`BITMAPFILEHEADER`); the info header follows (`FUN_00592F00` reads it from here).
const FILE_HEADER: usize = 14;
/// Length of the info header as `FUN_00592F00` assumes it (`BITMAPINFOHEADER`, pixels from info header + `0x28`).
const INFO_HEADER: usize = 0x28;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Decodes a BMP file to RGBA8 (image top first) like the game:
///
/// - Identifier `BM` (`0x4D42`), otherwise an error; `bfOffBits` does not count: the pixels begin at the info header
///   plus `0x28` plus the color table (up to 8 bit: `biClrUsed` or `1 << biBitCount` entries respectively; above
///   that `biClrUsed`, and with `BI_BITFIELDS` (3) three masks of 4 bytes each);
/// - `FUN_00592880` reads three bytes per pixel (blue, green, red), file row `r` at `r * width * 3` **without**
///   padding to 4 bytes, and writes it to image row `H - 1 - r`; alpha full.
///
/// The reimplementation accepts only uncompressed 24-bit images with positive height (the one BMP texture of the
/// game); the original reads other depths as triples as well, which would be a random image.
pub fn decode_bmp_game(bytes: &[u8]) -> Result<image::RgbaImage, FormatError> {
    if bytes.len() < FILE_HEADER + INFO_HEADER {
        return Err(FormatError::Invalid("BMP header too short".into()));
    }
    if u16_at(bytes, 0) != 0x4D42 {
        return Err(FormatError::Invalid(
            "not a BMP file (identifier != BM)".into(),
        ));
    }
    let info = FILE_HEADER;
    let width = u32_at(bytes, info + 4) as i32;
    let height = u32_at(bytes, info + 8) as i32;
    let bit_count = u16_at(bytes, info + 0xE);
    let compression = u32_at(bytes, info + 0x10);
    let clr_used = u32_at(bytes, info + 0x20) as usize;
    if bit_count != 24 || compression != 0 {
        return Err(FormatError::Invalid(format!(
            "BMP with {bit_count} bits per pixel and compression {compression}: only 24 bit uncompressed"
        )));
    }
    if width <= 0 || height <= 0 {
        return Err(FormatError::Invalid(format!(
            "BMP size {width} x {height}: only positive width and height (bottom first)"
        )));
    }
    let (width, height) = (width as usize, height as usize);
    // From 9 bit: `biClrUsed` entries, with `BI_BITFIELDS` three more masks (here compression 0).
    let pixels = info + INFO_HEADER + clr_used * 4;
    let needed = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| FormatError::Invalid("BMP image size overflows".into()))?;
    let data = bytes.get(pixels..).unwrap_or_default();
    if data.len() < needed {
        return Err(FormatError::Invalid(format!(
            "BMP pixel data too short: {} instead of {needed} bytes",
            data.len()
        )));
    }
    let mut img = image::RgbaImage::new(width as u32, height as u32);
    for row in 0..height {
        for x in 0..width {
            let i = (row * width + x) * 3;
            let rgba = [data[i + 2], data[i + 1], data[i], 255];
            img.put_pixel(x as u32, (height - 1 - row) as u32, image::Rgba(rgba));
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BMP with `width` x `height` pixels at 24 bit; pixels as (B, G, R) in file order (bottom first), without
    /// padding.
    fn bmp(width: i32, height: i32, bits: u16, pixels: &[[u8; 3]]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&0u32.to_le_bytes()); // file size (not read)
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&999u32.to_le_bytes()); // bfOffBits (not read)
        b.extend_from_slice(&0x28u32.to_le_bytes());
        b.extend_from_slice(&width.to_le_bytes());
        b.extend_from_slice(&height.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(&[0; 24]); // compression, size, resolution, biClrUsed, biClrImportant
        for p in pixels {
            b.extend_from_slice(p);
        }
        b
    }

    // QUIRK O-284: kept
    #[test]
    fn rows_are_stored_bottom_up_in_bgr_without_padding() {
        // 3 x 2: rows of 9 bytes, without padding to 12 (like `FUN_00592880`).
        let file = bmp(
            3,
            2,
            24,
            &[
                [1, 2, 3],
                [4, 5, 6],
                [7, 8, 9],
                [10, 11, 12],
                [13, 14, 15],
                [16, 17, 18],
            ],
        );
        let img = decode_bmp_game(&file).unwrap();
        assert_eq!(img.dimensions(), (3, 2));
        // File row 0 is the bottom image row; blue, green, red -> RGBA with alpha full.
        assert_eq!(img.get_pixel(0, 1).0, [3, 2, 1, 255]);
        assert_eq!(img.get_pixel(2, 1).0, [9, 8, 7, 255]);
        assert_eq!(img.get_pixel(0, 0).0, [12, 11, 10, 255]);
        assert_eq!(img.get_pixel(2, 0).0, [18, 17, 16, 255]);
    }

    // QUIRK O-284: kept
    #[test]
    fn other_depths_short_data_and_foreign_files_are_rejected() {
        assert!(decode_bmp_game(&bmp(1, 1, 32, &[[0; 3], [0; 3]])).is_err());
        assert!(decode_bmp_game(&bmp(2, 2, 24, &[[0; 3]; 3])).is_err());
        assert!(decode_bmp_game(&bmp(1, -1, 24, &[[0; 3]])).is_err());
        let mut tga = bmp(1, 1, 24, &[[0; 3]]);
        tga[0] = b'X';
        assert!(decode_bmp_game(&tga).is_err());
        assert!(decode_bmp_game(b"BM").is_err());
    }
}
