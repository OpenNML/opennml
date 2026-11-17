//! Conversion of a Bink frame (YUV 4:2:0) to RGBA.
//!
//! BT.601 in the limited range (as FFmpeg `color_range = MPEG` for revision `i`), integer arithmetic,
//! the same color for every 2 x 2 pixels, opacity 255. How `binkw32.dll` (`BinkCopyToBufferRect`) converts is not
//! analyzed (re/video).
//! Own code, not ported from FFmpeg.
//!
//! This part of the repository is licensed under LGPL-2.1-or-later (see `COPYING.LGPLv2.1`).

use crate::video::VideoFrame;

#[inline(always)]
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Writes `frame` as RGBA to `out`; if `out` has a different size, it is recreated.
///
/// `R = clip((298·(Y−16) + 409·(V−128) + 128) >> 8)`,
/// `G = clip((298·(Y−16) − 100·(U−128) − 208·(V−128) + 128) >> 8)`,
/// `B = clip((298·(Y−16) + 516·(U−128) + 128) >> 8)`.
pub fn to_rgba(frame: &VideoFrame, out: &mut image::RgbaImage) {
    let (w, h) = (frame.width, frame.height);
    if out.dimensions() != (w, h) {
        *out = image::RgbaImage::new(w, h);
    }
    if w == 0 || h == 0 {
        return;
    }
    let u_rows: Vec<&[u8]> = frame.plane(1).collect();
    let v_rows: Vec<&[u8]> = frame.plane(2).collect();
    let out_rows = out.chunks_exact_mut(w as usize * 4);
    for (row, (y_row, out_row)) in frame.plane(0).zip(out_rows).enumerate() {
        let u_row = u_rows.get(row / 2).copied().unwrap_or(&[]);
        let v_row = v_rows.get(row / 2).copied().unwrap_or(&[]);
        for (x, (&y, px)) in y_row.iter().zip(out_row.chunks_exact_mut(4)).enumerate() {
            let u = i32::from(u_row.get(x / 2).copied().unwrap_or(128)) - 128;
            let v = i32::from(v_row.get(x / 2).copied().unwrap_or(128)) - 128;
            let l = 298 * (i32::from(y) - 16);
            px[0] = clip((l + 409 * v + 128) >> 8);
            px[1] = clip((l - 100 * u - 208 * v + 128) >> 8);
            px[2] = clip((l + 516 * u + 128) >> 8);
            px[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(f: &VideoFrame, y: u8, u: u8, v: u8) -> [u8; 4] {
        let mut f = f.clone();
        f.y.fill(y);
        f.u.fill(u);
        f.v.fill(v);
        let mut out = image::RgbaImage::new(1, 1);
        to_rgba(&f, &mut out);
        assert_eq!(out.dimensions(), (4, 2));
        out.get_pixel(3, 1).0
    }

    #[test]
    fn to_rgba_maps_reference_colors() {
        let f = VideoFrame::new(4, 2);
        assert_eq!(px(&f, 16, 128, 128), [0, 0, 0, 255]);
        assert_eq!(px(&f, 235, 128, 128), [255, 255, 255, 255]);
        let red = px(&f, 81, 90, 240);
        assert!(
            red[0] >= 253 && red[1] <= 2 && red[2] <= 2 && red[3] == 255,
            "{red:?}"
        );
    }

    #[test]
    fn to_rgba_accepts_empty_frames() {
        for (w, h) in [(0, 0), (0, 4), (4, 0)] {
            let mut out = image::RgbaImage::new(3, 3);
            to_rgba(&VideoFrame::new(w, h), &mut out);
            assert_eq!(out.dimensions(), (w, h));
        }
    }

    #[test]
    fn to_rgba_shares_color_per_2x2() {
        let mut f = VideoFrame::new(4, 4);
        f.y.fill(128);
        f.u.fill(128);
        f.v.fill(128);
        // Set the color of the lower right 2 x 2 block to red.
        let s = f.strides[2];
        f.v[s + 1] = 240;
        let mut out = image::RgbaImage::new(4, 4);
        to_rgba(&f, &mut out);
        for (x, y) in [(2, 2), (3, 2), (2, 3), (3, 3)] {
            assert!(out.get_pixel(x, y).0[0] > out.get_pixel(0, 0).0[0]);
        }
        assert_eq!(out.get_pixel(1, 1), out.get_pixel(0, 0));
        assert_eq!(out.get_pixel(3, 1), out.get_pixel(0, 0));
    }
}
