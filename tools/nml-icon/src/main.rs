//! Generates the OpenNML program icon from the SVG sources in `assets/icon/`: PNGs in the sizes [`SIZES`] and
//! `opennml.ico` with all sizes. Usage and sizes: `assets/icon/README.md`.
//!
//! Each size has a fixed source ([`source_for`]): 16 px uses the variant `opennml-16.svg`, 24 and 32 px use
//! `opennml-32.svg` (both without pattern, aligned to whole pixels), from 48 px the full variant `opennml.svg`.
//! With the same `resvg` version the output is byte-identical; `--check` only compares and changes nothing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

/// All generated sizes in pixels (square).
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// From this size on the image is stored in the ICO as PNG, below that as a bitmap (usual split, for old readers).
const ICO_PNG_FROM: u32 = 256;

#[derive(Parser)]
#[command(
    name = "nml-icon",
    about = "Generates PNGs and ICO of the OpenNML icon from the SVG sources"
)]
struct Cli {
    /// Folder with the SVG sources and the generated files (default: assets/icon in the repository)
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Only check whether the files match the sources; write nothing
    #[arg(long)]
    check: bool,
}

/// SVG source for `size`.
fn source_for(size: u32) -> &'static str {
    match size {
        0..=16 => "opennml-16.svg",
        17..=32 => "opennml-32.svg",
        _ => "opennml.svg",
    }
}

/// File name of the PNG for `size`.
fn png_name(size: u32) -> String {
    format!("opennml-{size}.png")
}

/// Renders the SVG data `svg` at `size` x `size` pixels (premultiplied RGBA).
fn render(svg: &[u8], size: u32) -> Result<Pixmap> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).context("SVG not readable")?;
    let view = tree.size();
    ensure!(
        view.width() == view.height(),
        "SVG is not square ({} x {})",
        view.width(),
        view.height()
    );
    let mut pixmap = Pixmap::new(size, size).context("image size 0")?;
    let scale = size as f32 / view.width();
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// Non-premultiplied RGBA, rows from top to bottom.
fn rgba(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

/// Image in the ICO as a bitmap: `BITMAPINFOHEADER` with double height, 32 bpp BGRA from bottom to top, followed by
/// the 1-bpp AND mask (set where the image is fully transparent), rows padded to 4 bytes.
fn ico_bitmap(size: u32, rgba: &[u8]) -> Vec<u8> {
    let s = size as usize;
    let mask_row = s.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + s * s * 4 + mask_row * s);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&(2 * size as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // Planes
    out.extend_from_slice(&32u16.to_le_bytes()); // Bits per pixel
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((s * s * 4 + mask_row * s) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]); // Resolution and color table
    for y in (0..s).rev() {
        for px in rgba[y * s * 4..(y + 1) * s * 4].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    for y in (0..s).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..s {
            if rgba[(y * s + x) * 4 + 3] == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

/// ICO file from `(size, image data)`: header, directory, then the images in the same order.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // Type icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, data) in images {
        // 256 is stored in the directory as 0.
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

/// All output files `(name, content)` from the sources in `dir`.
fn generate(dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    let mut ico_images = Vec::new();
    for size in SIZES {
        let path = dir.join(source_for(size));
        let svg =
            std::fs::read(&path).with_context(|| format!("{} not readable", path.display()))?;
        let pixmap =
            render(&svg, size).with_context(|| format!("{} at {size} px", path.display()))?;
        let png = pixmap.encode_png().context("PNG not encodable")?;
        let entry = if size >= ICO_PNG_FROM {
            png.clone()
        } else {
            ico_bitmap(size, &rgba(&pixmap))
        };
        ico_images.push((size, entry));
        files.push((png_name(size), png));
    }
    files.push(("opennml.ico".to_string(), ico(&ico_images)));
    Ok(files)
}

fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = cli.dir.unwrap_or_else(default_dir);
    let files = generate(&dir)?;
    let mut stale = Vec::new();
    for (name, data) in &files {
        let path = dir.join(name);
        if cli.check {
            if std::fs::read(&path).ok().as_deref() != Some(data.as_slice()) {
                stale.push(name.as_str());
            }
        } else {
            std::fs::write(&path, data)
                .with_context(|| format!("{} not writable", path.display()))?;
            println!("{} ({} bytes)", path.display(), data.len());
        }
    }
    if !stale.is_empty() {
        bail!(
            "out of date: {} (regenerate with `cargo run -p nml-icon`)",
            stale.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checked-in files match the SVG sources (otherwise run `cargo run -p nml-icon`).
    #[test]
    fn assets_are_current() {
        let dir = default_dir();
        for (name, data) in generate(&dir).unwrap() {
            let on_disk = std::fs::read(dir.join(&name)).unwrap_or_default();
            assert!(
                on_disk == data,
                "{name} does not match the sources; regenerate with `cargo run -p nml-icon`"
            );
        }
    }

    #[test]
    fn sources_by_size() {
        assert_eq!(source_for(16), "opennml-16.svg");
        assert_eq!(source_for(24), "opennml-32.svg");
        assert_eq!(source_for(32), "opennml-32.svg");
        assert_eq!(source_for(48), "opennml.svg");
        assert_eq!(source_for(256), "opennml.svg");
    }

    #[test]
    fn ico_layout() {
        let images = vec![
            (16, ico_bitmap(16, &[0; 16 * 16 * 4])),
            (256, vec![1, 2, 3]),
        ];
        let data = ico(&images);
        assert_eq!(&data[..6], &[0, 0, 1, 0, 2, 0]);
        // First entry: 16 x 16, bitmap 40 + 1024 + 16 * 4 bytes directly after the directory.
        assert_eq!(data[6], 16);
        assert_eq!(
            u32::from_le_bytes(data[14..18].try_into().unwrap()),
            40 + 1024 + 64
        );
        assert_eq!(u32::from_le_bytes(data[18..22].try_into().unwrap()), 6 + 32);
        // Second entry: 256 is stored as 0.
        assert_eq!(data[22], 0);
        assert_eq!(&data[data.len() - 3..], &[1, 2, 3]);
        // Fully transparent image: AND mask set in every row (2 bytes for 16 px), padding to 4 bytes is 0.
        for row in images[0].1[40 + 1024..].chunks_exact(4) {
            assert_eq!(row, &[0xff, 0xff, 0, 0]);
        }
    }
}
