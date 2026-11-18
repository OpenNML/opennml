//! Draw commands: the output of the UI, which a renderer executes.

use crate::assets::UiAssets;

/// ID of an image in [`UiAssets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextureId(pub u32);

/// Texture filter for drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    Point,
    Linear,
}

/// Blend mode of the color when drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    /// `SRCALPHA/INVSRCALPHA`: the default of the UI state blocks (`Font`, `3dicons-*`).
    #[default]
    Alpha,
    /// `SRCALPHA/ONE` (additive): state block `billboard`, in the shipped data only the beam of the loading screen
    /// (`FUN_004026A0`, re/singleplayer-menus 6.3a, O-194).
    Additive,
}

/// A draw command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawCmd {
    /// Quad `dst = [x0, y0, x1, y1]` in screen pixels with `uv = [u0, v0, u1, v1]` at the corners
    /// `(x0, y0)` and `(x1, y1)` respectively; `texture: None` = filled area; `color` ARGB as diffuse (texture *
    /// color); `blend` is the blend mode.
    Quad {
        texture: Option<TextureId>,
        dst: [i32; 4],
        uv: [f32; 4],
        color: u32,
        filter: Filter,
        blend: Blend,
    },
    /// Arbitrary quad with four free corners in screen pixels (floating point), clockwise from the top left, with
    /// `uv` per corner in the same order; otherwise like [`DrawCmd::Quad`], always with [`Blend::Alpha`]. For the
    /// rotated minimap, its outline, the points and the camera frame (a deviation: the original draws rotated strips
    /// and lines there, `0x004E3B70`, `0x004E4290`). The renderer lays two triangles without clipping over a rectangle
    /// (no scissor).
    Quad4 {
        texture: Option<TextureId>,
        corners: [[f32; 2]; 4],
        uv: [[f32; 2]; 4],
        color: u32,
        filter: Filter,
    },
}

/// Line from `from` to `to` as a 1 px wide [`DrawCmd::Quad4`] without a texture (state `Lines` of the original,
/// `Renderer_DrawIndexedUpLineListScreen` `0x00571CE0`): the segment widened by 0.5 px on each side across its
/// direction. A line of length 0 yields an empty quad (D3D8 then draws nothing either).
pub fn line(from: [f32; 2], to: [f32; 2], color: u32) -> DrawCmd {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    let (nx, ny) = if len > 0.0 {
        (-dy / len * 0.5, dx / len * 0.5)
    } else {
        (0.0, 0.0)
    };
    DrawCmd::Quad4 {
        texture: None,
        corners: [
            [from[0] + nx, from[1] + ny],
            [to[0] + nx, to[1] + ny],
            [to[0] - nx, to[1] - ny],
            [from[0] - nx, from[1] - ny],
        ],
        uv: [[0.0; 2]; 4],
        color,
        filter: Filter::Point,
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

/// Checksum of a command list (FNV-1a, 64 bit) over the kind, texture path, rectangle, UV bits, color and filter of
/// each command in list order. The path instead of the ID is used so that the sum does not depend on the loading
/// order of the images. The blend mode is part of the kind byte (`Quad` 0, additive `Quad` 2) so that the measured
/// sums of all normally blended commands remain valid.
pub fn draw_checksum(cmds: &[DrawCmd], assets: &UiAssets) -> u64 {
    let mut hash = FNV_OFFSET;
    let texture_bytes = |hash: &mut u64, texture: Option<TextureId>| match texture {
        Some(id) => {
            fnv(hash, &[1]);
            fnv(hash, assets.path(id).as_bytes());
            fnv(hash, &[0]);
        }
        None => fnv(hash, &[0]),
    };
    let filter_byte = |filter: Filter| match filter {
        Filter::Point => 0,
        Filter::Linear => 1,
    };
    for cmd in cmds {
        match *cmd {
            DrawCmd::Quad {
                texture,
                dst,
                uv,
                color,
                filter,
                blend,
            } => {
                // Byte for byte as before `Quad4` and `Blend`: the measured draw checksums remain valid.
                let kind = match blend {
                    Blend::Alpha => 0,
                    Blend::Additive => 2,
                };
                fnv(&mut hash, &[kind]); // command kind: Quad or additive Quad
                texture_bytes(&mut hash, texture);
                for v in dst {
                    fnv(&mut hash, &v.to_le_bytes());
                }
                for v in uv {
                    fnv(&mut hash, &v.to_bits().to_le_bytes());
                }
                fnv(&mut hash, &color.to_le_bytes());
                fnv(&mut hash, &[filter_byte(filter)]);
            }
            DrawCmd::Quad4 {
                texture,
                corners,
                uv,
                color,
                filter,
            } => {
                fnv(&mut hash, &[1]); // command kind: Quad4
                texture_bytes(&mut hash, texture);
                for v in corners.iter().flatten() {
                    fnv(&mut hash, &v.to_bits().to_le_bytes());
                }
                for v in uv.iter().flatten() {
                    fnv(&mut hash, &v.to_bits().to_le_bytes());
                }
                fnv(&mut hash, &color.to_le_bytes());
                fnv(&mut hash, &[filter_byte(filter)]);
            }
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;

    fn assets() -> (UiAssets, TextureId, TextureId) {
        let mut assets = UiAssets::new();
        let a = assets.insert_image("data/a.tga", RgbaImage::new(2, 2));
        let b = assets.insert_image("data/b.tga", RgbaImage::new(2, 2));
        (assets, a, b)
    }

    fn quad(texture: Option<TextureId>) -> DrawCmd {
        DrawCmd::Quad {
            texture,
            dst: [1, 2, 3, 4],
            uv: [0.0, 0.5, 0.25, 1.0],
            color: 0xFF11_2233,
            filter: Filter::Point,
            blend: Blend::Alpha,
        }
    }

    // QUIRK O-194: kept
    #[test]
    fn checksum_changes_with_every_field() {
        let (assets, a, b) = assets();
        let base = draw_checksum(&[quad(Some(a))], &assets);
        // Same input, same sum.
        assert_eq!(base, draw_checksum(&[quad(Some(a))], &assets));
        let mut seen = vec![base];
        let mut check = |cmd: DrawCmd| {
            let sum = draw_checksum(&[cmd], &assets);
            assert!(!seen.contains(&sum), "checksum unchanged for {cmd:?}");
            seen.push(sum);
        };
        let DrawCmd::Quad {
            dst,
            uv,
            color,
            filter,
            ..
        } = quad(None)
        else {
            unreachable!()
        };
        // Texture: another image, no texture.
        check(quad(Some(b)));
        check(quad(None));
        // Every rectangle field.
        for i in 0..4 {
            let mut d = dst;
            d[i] += 1;
            check(DrawCmd::Quad {
                texture: Some(a),
                dst: d,
                uv,
                color,
                filter,
                blend: Blend::Alpha,
            });
        }
        // Every UV field (even a single bit of the f32 counts).
        for i in 0..4 {
            let mut u = uv;
            u[i] = f32::from_bits(u[i].to_bits() + 1);
            check(DrawCmd::Quad {
                texture: Some(a),
                dst,
                uv: u,
                color,
                filter,
                blend: Blend::Alpha,
            });
        }
        check(DrawCmd::Quad {
            texture: Some(a),
            dst,
            uv,
            color: color ^ 1,
            filter,
            blend: Blend::Alpha,
        });
        check(DrawCmd::Quad {
            texture: Some(a),
            dst,
            uv,
            color,
            filter: Filter::Linear,
            blend: Blend::Alpha,
        });
        check(DrawCmd::Quad {
            texture: Some(a),
            dst,
            uv,
            color,
            filter,
            blend: Blend::Additive,
        });
        // Order and count matter.
        let two = draw_checksum(&[quad(Some(a)), quad(Some(b))], &assets);
        let swapped = draw_checksum(&[quad(Some(b)), quad(Some(a))], &assets);
        assert_ne!(two, swapped);
        assert_ne!(two, base);
        assert_ne!(draw_checksum(&[], &assets), base);
    }

    #[test]
    fn quad4_checksum_has_its_own_kind_and_every_field() {
        let (assets, a, _) = assets();
        let quad4 = |corners: [[f32; 2]; 4], uv: [[f32; 2]; 4], color: u32| DrawCmd::Quad4 {
            texture: Some(a),
            corners,
            uv,
            color,
            filter: Filter::Point,
        };
        let corners = [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]];
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let base = draw_checksum(&[quad4(corners, uv, 0xFF11_2233)], &assets);
        // The same rectangle as a `Quad` yields a different sum (own kind byte 1).
        let as_quad = DrawCmd::Quad {
            texture: Some(a),
            dst: [1, 2, 3, 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: 0xFF11_2233,
            filter: Filter::Point,
            blend: Blend::Alpha,
        };
        assert_ne!(base, draw_checksum(&[as_quad], &assets));
        let mut seen = vec![base];
        for i in 0..8 {
            let mut c = corners;
            c[i / 2][i % 2] += 0.25;
            let mut u = uv;
            u[i / 2][i % 2] = f32::from_bits(u[i / 2][i % 2].to_bits() + 1);
            for cmd in [quad4(c, uv, 0xFF11_2233), quad4(corners, u, 0xFF11_2233)] {
                let sum = draw_checksum(&[cmd], &assets);
                assert!(!seen.contains(&sum), "checksum unchanged for {cmd:?}");
                seen.push(sum);
            }
        }
        assert!(!seen.contains(&draw_checksum(&[quad4(corners, uv, 0xFF11_2232)], &assets)));
    }

    #[test]
    fn line_is_one_pixel_wide_across_its_direction() {
        let DrawCmd::Quad4 {
            texture,
            corners,
            color,
            ..
        } = line([10.0, 5.0], [20.0, 5.0], 0xFFFF_FFFF)
        else {
            unreachable!()
        };
        assert_eq!((texture, color), (None, 0xFFFF_FFFF));
        assert_eq!(
            corners,
            [[10.0, 5.5], [20.0, 5.5], [20.0, 4.5], [10.0, 4.5]]
        );
        // Vertical: across it in x.
        let DrawCmd::Quad4 { corners, .. } = line([3.0, 0.0], [3.0, 8.0], 0) else {
            unreachable!()
        };
        assert_eq!(corners, [[2.5, 0.0], [2.5, 8.0], [3.5, 8.0], [3.5, 0.0]]);
        // Length 0: empty quad at the point.
        let DrawCmd::Quad4 { corners, .. } = line([7.0, 7.0], [7.0, 7.0], 0) else {
            unreachable!()
        };
        assert_eq!(corners, [[7.0, 7.0]; 4]);
    }

    #[test]
    fn checksum_uses_the_texture_path_not_the_id() {
        // Two collections in which the same image has different IDs yield the same sum.
        let mut first = UiAssets::new();
        let a1 = first.insert_image("data/a.tga", RgbaImage::new(1, 1));
        let mut second = UiAssets::new();
        second.insert_image("data/zzz.tga", RgbaImage::new(1, 1));
        let a2 = second.insert_image("DATA\\A.TGA", RgbaImage::new(1, 1));
        assert_ne!(a1, a2);
        assert_eq!(
            draw_checksum(&[quad(Some(a1))], &first),
            draw_checksum(&[quad(Some(a2))], &second)
        );
    }
}
