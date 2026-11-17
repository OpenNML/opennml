//! Renders images without a window (screenshots for visual checks and error reports).

use anyhow::{Context, Result, anyhow, bail};
use image::RgbaImage;

/// Color format of the offscreen images (non-sRGB, like the surface).
pub const CAPTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Whether the format stores the bytes per texel as B, G, R, A (then R and B must be swapped when reading back).
/// Error for formats that cannot be read as 8-bit RGBA.
fn is_bgra(format: wgpu::TextureFormat) -> Result<bool> {
    use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba8Unorm, Rgba8UnormSrgb};
    match format {
        Rgba8Unorm | Rgba8UnormSrgb => Ok(false),
        Bgra8Unorm | Bgra8UnormSrgb => Ok(true),
        other => bail!("format {other:?} cannot be read for screenshots (Rgba8/Bgra8 only)"),
    }
}

/// Converts the buffer rows that were read back (with row padding `padded_row`) into an RGBA image;
/// with `bgra`, R and B are swapped.
fn rows_to_rgba(data: &[u8], padded_row: usize, size: (u32, u32), bgra: bool) -> RgbaImage {
    let (width, height) = size;
    let mut img = RgbaImage::new(width, height);
    for (y, row) in data.chunks(padded_row).take(height as usize).enumerate() {
        for x in 0..width as usize {
            let p = &row[x * 4..x * 4 + 4];
            let pixel = if bgra {
                [p[2], p[1], p[0], p[3]]
            } else {
                [p[0], p[1], p[2], p[3]]
            };
            img.put_pixel(x as u32, y as u32, image::Rgba(pixel));
        }
    }
    img
}

/// Renders with `draw` into a texture of `size` in format `format` and reads it back as an RGBA image.
/// `format` must be the target format of the pipelines `draw` uses (e.g. the surface format or
/// [`CAPTURE_FORMAT`]); for `Bgra8*`, R and B are swapped when reading back.
pub fn render_to_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    draw: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
) -> Result<RgbaImage> {
    let bgra = is_bgra(format)?;
    let (width, height) = (size.0.max(1), size.1.max(1));
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Screenshot"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Screenshot readback"),
        size: padded_row as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Screenshot"),
    });
    draw(&mut encoder, &view);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .context("waiting for the GPU")?;
    receiver
        .recv()
        .context("mapping the buffer")?
        .map_err(|e| anyhow!("mapping the buffer: {e}"))?;
    let data = slice
        .get_mapped_range()
        .map_err(|e| anyhow!("reading the buffer: {e}"))?;
    Ok(rows_to_rgba(
        &data,
        padded_row as usize,
        (width, height),
        bgra,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackendChoice, headless_device};

    #[test]
    fn rows_are_unpadded_and_bgra_is_swizzled() {
        // 2 x 2 texels, row length 12 instead of 8 bytes (padding).
        let data = [
            1, 2, 3, 4, 5, 6, 7, 8, 0xEE, 0xEE, 0xEE, 0xEE, //
            9, 10, 11, 12, 13, 14, 15, 16, 0xEE, 0xEE, 0xEE, 0xEE,
        ];
        let rgba = rows_to_rgba(&data, 12, (2, 2), false);
        assert_eq!(rgba.get_pixel(0, 0).0, [1, 2, 3, 4]);
        assert_eq!(rgba.get_pixel(1, 1).0, [13, 14, 15, 16]);
        let bgra = rows_to_rgba(&data, 12, (2, 2), true);
        assert_eq!(bgra.get_pixel(0, 0).0, [3, 2, 1, 4]);
        assert_eq!(bgra.get_pixel(1, 0).0, [7, 6, 5, 8]);
        assert_eq!(bgra.get_pixel(0, 1).0, [11, 10, 9, 12]);
    }

    #[test]
    fn only_eight_bit_rgba_and_bgra_formats_are_readable() {
        use wgpu::TextureFormat::*;
        assert!(!is_bgra(Rgba8Unorm).unwrap());
        assert!(!is_bgra(Rgba8UnormSrgb).unwrap());
        assert!(is_bgra(Bgra8Unorm).unwrap());
        assert!(is_bgra(Bgra8UnormSrgb).unwrap());
        assert!(is_bgra(R8Unorm).is_err());
        assert!(is_bgra(Rgba16Float).is_err());
    }

    /// Fills the target with a color.
    fn clear(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, color: wgpu::Color) {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }

    #[test]
    fn renders_a_clear_color_offscreen_in_either_channel_order() {
        // Skip if there is no graphics card.
        let Ok((device, queue, _)) = headless_device(BackendChoice::Auto) else {
            eprintln!("no GPU, test skipped");
            return;
        };
        let color = wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        // Width of 5 pixels: row length 20 bytes, so padded to 256.
        for format in [
            CAPTURE_FORMAT,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ] {
            let img = render_to_image(&device, &queue, (5, 3), format, |encoder, view| {
                clear(encoder, view, color)
            })
            .unwrap();
            assert_eq!(img.dimensions(), (5, 3));
            assert_eq!(img.get_pixel(4, 2).0, [255, 0, 0, 255], "format {format:?}");
        }
    }
}
