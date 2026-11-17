//! Uploading textures. Mip levels are created as in `TgaImage_Upload` (`0x00591BF0`) by
//! taking the nearest source texels (no average).

use image::{GrayImage, RgbaImage};

/// A texture with its view.
pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

/// Mip chain by point sampling: level `l` takes texel `(x * 2^l, y * 2^l)`.
pub fn nearest_mip_chain(img: &RgbaImage) -> Vec<RgbaImage> {
    let mut levels = vec![img.clone()];
    let (mut w, mut h) = img.dimensions();
    let mut step = 1u32;
    while w > 1 || h > 1 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        step *= 2;
        let (sw, sh) = img.dimensions();
        levels.push(RgbaImage::from_fn(w, h, |x, y| {
            *img.get_pixel((x * step).min(sw - 1), (y * step).min(sh - 1))
        }));
    }
    levels
}

fn create(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    mip_level_count: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_level(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    level: u32,
    size: (u32, u32),
    bytes_per_pixel: u32,
    data: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.0 * bytes_per_pixel),
            rows_per_image: Some(size.1),
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
}

/// RGBA texture (`Rgba8Unorm`, no sRGB like the original); with `mipmaps` the full mip chain.
pub fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    img: &RgbaImage,
    mipmaps: bool,
    label: &str,
) -> GpuTexture {
    let img = if img.width() == 0 || img.height() == 0 {
        RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))
    } else {
        img.clone()
    };
    let levels = if mipmaps {
        nearest_mip_chain(&img)
    } else {
        vec![img]
    };
    let texture = create(
        device,
        label,
        levels[0].dimensions(),
        wgpu::TextureFormat::Rgba8Unorm,
        levels.len() as u32,
    );
    for (level, data) in levels.iter().enumerate() {
        write_level(
            queue,
            &texture,
            level as u32,
            data.dimensions(),
            4,
            data.as_raw(),
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuTexture { texture, view }
}

/// Overwrites a texture of the same size created with [`upload_rgba`] (without mip levels) with new content.
/// An image of a different size or without area is ignored (the caller then creates a new one).
pub fn update_rgba(queue: &wgpu::Queue, target: &GpuTexture, img: &RgbaImage) {
    let size = target.texture.size();
    if img.dimensions() != (size.width, size.height) || img.width() == 0 || img.height() == 0 {
        return;
    }
    write_level(queue, &target.texture, 0, img.dimensions(), 4, img.as_raw());
}

/// Single-channel texture (`R8Unorm`), e.g. alpha maps.
pub fn upload_gray(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    img: &GrayImage,
    label: &str,
) -> GpuTexture {
    let texture = create(
        device,
        label,
        img.dimensions(),
        wgpu::TextureFormat::R8Unorm,
        1,
    );
    write_level(queue, &texture, 0, img.dimensions(), 1, img.as_raw());
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuTexture { texture, view }
}

/// White 1 x 1 texture as a replacement for missing textures and unused stages.
pub fn white(device: &wgpu::Device, queue: &wgpu::Queue) -> GpuTexture {
    upload_rgba(
        device,
        queue,
        &RgbaImage::from_pixel(1, 1, image::Rgba([255; 4])),
        false,
        "white",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_takes_nearest_texels() {
        let img = RgbaImage::from_fn(4, 2, |x, y| image::Rgba([x as u8, y as u8, 0, 255]));
        let levels = nearest_mip_chain(&img);
        let sizes: Vec<(u32, u32)> = levels.iter().map(|l| l.dimensions()).collect();
        assert_eq!(sizes, [(4, 2), (2, 1), (1, 1)]);
        assert_eq!(levels[1].get_pixel(1, 0).0, [2, 0, 0, 255]);
        assert_eq!(levels[2].get_pixel(0, 0).0, [0, 0, 0, 255]);
    }
}
