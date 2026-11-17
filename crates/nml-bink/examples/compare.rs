//! Acceptance tool: compares the output of `nml-bink` with reference files that FFmpeg generated
//! beforehand. The example never calls FFmpeg and reads only the files whose paths it receives as arguments.
//!
//! ```text
//! compare video <bik> <frames.txt>        per frame FNV-1a 64 (fresh basis) over Y, U, V
//! compare audio <bik> <ref.f32le>         audio track 0, interleaved f32 at native rate
//! compare rgb   <bik> <frame> <ref.rgb>   to_rgba of frame <frame> against FFmpeg's rgb24
//! compare shot  <png> <bik> <ref.rgb> [x0,y0,x1,y1 ...]
//!                                         screenshot 640 x 480 against FFmpeg's rgb24, without the
//!                                         given rectangles (open dialogs, screen pixels);
//!                                         instead of <ref.rgb>, `eigen:<frame>` compares with `to_rgba`
//! ```
//!
//! `frames.txt` has one line per frame with the checksum in hex (`ffmpeg -f rawvideo -pix_fmt
//! yuv420p`, cropped planes). `ref.rgb` is a single frame `-pix_fmt rgb24 -f rawvideo`.
//! Own code, not ported from FFmpeg.
//!
//! This part of the repository is licensed under LGPL-2.1-or-later (see `COPYING.LGPLv2.1`).

use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;
use std::time::Instant;

use nml_bink::{AudioDecoder, BinkReader, VideoDecoder, YUV_HASH_BASIS, to_rgba, yuv_hash};

type Fallible<T> = Result<T, String>;

fn open(path: &str) -> Fallible<BinkReader<BufReader<File>>> {
    let file = File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    BinkReader::open(BufReader::new(file)).map_err(|e| format!("{path}: {e}"))
}

fn read(path: &str) -> Fallible<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// Decode frame by frame and compare the checksum of each frame with the reference line.
fn video(bik: &str, frames_txt: &str) -> Fallible<bool> {
    let text = String::from_utf8(read(frames_txt)?).map_err(|e| format!("{frames_txt}: {e}"))?;
    let expected: Vec<u64> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| u64::from_str_radix(l.trim(), 16).map_err(|e| format!("{frames_txt}: {l}: {e}")))
        .collect::<Fallible<_>>()?;
    let mut rd = open(bik)?;
    let mut dec = VideoDecoder::new(&rd.header).map_err(|e| format!("{bik}: {e}"))?;
    let frames = rd.header.frames;
    let (mut equal, mut first_diff) = (0u32, None);
    let (mut decode_s, mut read_s) = (0f64, 0f64);
    for k in 0..frames {
        let t = Instant::now();
        let p = rd
            .read_frame(k)
            .map_err(|e| format!("{bik}: frame {k}: {e}"))?;
        read_s += t.elapsed().as_secs_f64();
        let t = Instant::now();
        let frame = dec
            .decode(&p.video)
            .map_err(|e| format!("{bik}: frame {k}: {e}"))?;
        decode_s += t.elapsed().as_secs_f64();
        let mut h = YUV_HASH_BASIS;
        yuv_hash(frame, &mut h);
        if expected.get(k as usize) == Some(&h) {
            equal += 1;
        } else if first_diff.is_none() {
            first_diff = Some(k);
        }
    }
    let ok = equal == frames && expected.len() == frames as usize;
    println!(
        "video {bik}: frames {frames}, reference {}, equal {equal}, first unequal {}, \
         decoding {:.3} s ({:.0} frames/s), reading {:.3} s",
        expected.len(),
        first_diff.map_or("-".to_string(), |k| k.to_string()),
        decode_s,
        f64::from(frames) / decode_s.max(1e-9),
        read_s,
    );
    Ok(ok)
}

/// Decode audio track 0 completely and compare it with the FFmpeg output (`f32le`, interleaved).
fn audio(bik: &str, ref_f32: &str) -> Fallible<bool> {
    let bytes = read(ref_f32)?;
    let reference: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let mut rd = open(bik)?;
    let track = rd
        .header
        .audio
        .first()
        .cloned()
        .ok_or_else(|| format!("{bik} has no audio track"))?;
    let mut dec = AudioDecoder::new(&track).map_err(|e| format!("{bik}: {e}"))?;
    let mut out = Vec::new();
    let t = Instant::now();
    for k in 0..rd.header.frames {
        let p = rd
            .read_frame(k)
            .map_err(|e| format!("{bik}: frame {k}: {e}"))?;
        if let Some(Some(a)) = p.audio.first() {
            dec.decode_packet(a, &mut out)
                .map_err(|e| format!("{bik}: audio packet {k}: {e}"))?;
        }
    }
    let secs = t.elapsed().as_secs_f64();
    let n = out.len().min(reference.len());
    let (mut max, mut at, mut sum) = (0f64, 0usize, 0f64);
    for i in 0..n {
        let d = (f64::from(out[i]) - f64::from(reference[i])).abs();
        sum += d * d;
        if d > max {
            max = d;
            at = i;
        }
    }
    let ch = dec.channels().max(1);
    println!(
        "audio {bik}: rate {}, channels {ch}, samples per channel {} / reference {}, \
         largest deviation {max:.3e} at {at}, RMS {:.3e}, decoding {secs:.3} s",
        dec.sample_rate(),
        out.len() / ch,
        reference.len() / ch,
        (sum / n.max(1) as f64).sqrt(),
    );
    Ok(out.len() == reference.len() && max <= 2e-5)
}

/// Decode frame `index` and return it as RGBA.
fn decode_rgba(bik: &str, index: u32) -> Fallible<image::RgbaImage> {
    let mut rd = open(bik)?;
    if index >= rd.header.frames {
        return Err(format!("{bik} has only {} frames", rd.header.frames));
    }
    let mut dec = VideoDecoder::new(&rd.header).map_err(|e| format!("{bik}: {e}"))?;
    let mut img = image::RgbaImage::new(0, 0);
    for k in 0..=index {
        let p = rd
            .read_frame(k)
            .map_err(|e| format!("{bik}: frame {k}: {e}"))?;
        let frame = dec
            .decode(&p.video)
            .map_err(|e| format!("{bik}: frame {k}: {e}"))?;
        if k == index {
            to_rgba(frame, &mut img);
        }
    }
    Ok(img)
}

/// Rectangle `[x0, y0, x1, y1)` in screen pixels.
type Rect = [u32; 4];

fn inside(r: &Rect, x: u32, y: u32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

/// Mean and largest deviation per channel between `h` rows of RGBA (from row `y0` of `ours`) and
/// FFmpeg's rgb24 (`reference`, width `w`); pixels in `masks` (coordinates of `ours`) do not count.
fn channel_stats(
    ours: &image::RgbaImage,
    y0: u32,
    reference: &[u8],
    (w, h): (u32, u32),
    masks: &[Rect],
) -> String {
    let mut sum = [0u64; 3];
    let mut max = [0u8; 3];
    let mut n = 0u64;
    for y in 0..h {
        for x in 0..w {
            if masks.iter().any(|r| inside(r, x, y0 + y)) {
                continue;
            }
            n += 1;
            let a = ours.get_pixel(x, y0 + y).0;
            let i = ((y * w + x) * 3) as usize;
            for c in 0..3 {
                let d = a[c].abs_diff(reference[i + c]);
                sum[c] += u64::from(d);
                max[c] = max[c].max(d);
            }
        }
    }
    let n = n.max(1) as f64;
    format!(
        "pixels {}, mean deviation R/G/B {:.3}/{:.3}/{:.3}, largest {}/{}/{}",
        n,
        sum[0] as f64 / n,
        sum[1] as f64 / n,
        sum[2] as f64 / n,
        max[0],
        max[1],
        max[2],
    )
}

fn rgb(bik: &str, index: &str, ref_rgb: &str) -> Fallible<bool> {
    let index: u32 = index
        .parse()
        .map_err(|e| format!("frame number {index}: {e}"))?;
    let img = decode_rgba(bik, index)?;
    let (w, h) = img.dimensions();
    let reference = read(ref_rgb)?;
    if reference.len() != (w * h * 3) as usize {
        return Err(format!(
            "{ref_rgb}: {} bytes, expected {w} x {h} x 3",
            reference.len()
        ));
    }
    println!(
        "rgb {bik} frame {index}: {}",
        channel_stats(&img, 0, &reference, (w, h), &[])
    );
    Ok(true)
}

fn parse_rect(s: &str) -> Fallible<Rect> {
    let v: Vec<u32> = s
        .split(',')
        .map(|p| p.trim().parse().map_err(|e| format!("rectangle {s}: {e}")))
        .collect::<Fallible<_>>()?;
    match v.as_slice() {
        [x0, y0, x1, y1] => Ok([*x0, *y0, *x1, *y1]),
        _ => Err(format!("rectangle {s}: expected x0,y0,x1,y1")),
    }
}

/// Screenshot at 640 x 480: the video occupies the rows `(480 − h)/2 …`, the remaining rows must be black;
/// pixels in `masks` (open dialogs) do not count.
fn shot(png: &str, bik: &str, ref_rgb: &str, masks: &[&str]) -> Fallible<bool> {
    let masks: Vec<Rect> = masks
        .iter()
        .map(|m| parse_rect(m))
        .collect::<Fallible<_>>()?;
    let shot = image::open(png)
        .map_err(|e| format!("{png}: {e}"))?
        .to_rgba8();
    if shot.dimensions() != (640, 480) {
        return Err(format!(
            "{png}: {:?} instead of 640 x 480",
            shot.dimensions()
        ));
    }
    let header = open(bik)?.header;
    let (w, h) = (header.width, header.height);
    if w != 640 || h > 480 {
        return Err(format!("{bik}: {w} x {h} does not fit the 640 x 480 field"));
    }
    let reference = match ref_rgb.strip_prefix("eigen:") {
        Some(index) => {
            let index: u32 = index
                .parse()
                .map_err(|e| format!("frame number {index}: {e}"))?;
            let img = decode_rgba(bik, index)?;
            img.pixels()
                .flat_map(|p| [p.0[0], p.0[1], p.0[2]])
                .collect()
        }
        None => read(ref_rgb)?,
    };
    if reference.len() != (w * h * 3) as usize {
        return Err(format!(
            "{ref_rgb}: {} bytes, expected {w} x {h} x 3",
            reference.len()
        ));
    }
    let y0 = (480 - h) / 2;
    let black = (0..480).filter(|&y| y < y0 || y >= y0 + h).all(|y| {
        (0..640).all(|x| {
            masks.iter().any(|r| inside(r, x, y)) || shot.get_pixel(x, y).0[..3] == [0, 0, 0]
        })
    });
    println!(
        "shot {png}: rows {y0}..{}, {}, bars black: {}",
        y0 + h,
        channel_stats(&shot, y0, &reference, (w, h), &masks),
        if black { "yes" } else { "no" },
    );
    Ok(black)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["video", bik, frames] => video(bik, frames),
        ["audio", bik, reference] => audio(bik, reference),
        ["rgb", bik, index, reference] => rgb(bik, index, reference),
        ["shot", png, bik, reference, masks @ ..] => shot(png, bik, reference, masks),
        _ => Err("usage: compare video <bik> <frames.txt> | audio <bik> <ref.f32le> | \
                  rgb <bik> <frame> <ref.rgb> | shot <png> <bik> <ref.rgb|eigen:frame> [x0,y0,x1,y1 ...]"
            .to_string()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            println!("DEVIATION");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(2)
        }
    }
}
