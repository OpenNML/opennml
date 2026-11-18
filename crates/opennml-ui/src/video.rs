//! Video playback as data (re/video, sections "Playback path", "Abort", "After the video").
//!
//! The menu root of the original (vtable `0x0061A2F0`) holds the running video (`+0xB8`), the flag "abortable"
//! (`+0xBC`) and the target rectangle (`+0xC0...+0xCC`). The reimplementation separates this: [`VideoPlayback`] is the
//! state in [`crate::app::MenuApp`]; file, decoder and sound belong to the host (`opennml-client`), which opens a video on
//! [`crate::msg::UiEffect::PlayVideo`] and reports it with [`crate::app::MenuApp::video_started`].
//!
//! **Tick from the program clock** (deviation): The original ties the whole program loop to the video tick
//! (`FUN_0045CB30` waits with `Sleep(1)` for `BinkWait`, O-65); here the frame number follows the clock
//! ([`VideoPlayback::frame_at`]) and the loop runs freely.

use crate::coords::Screen;
use crate::draw::TextureId;

/// Path under which the host stores the current video frame in [`crate::assets::UiAssets`].
pub const VIDEO_TEXTURE: &str = "video:frame";

/// Videos of the startup sequence by `n` in `0x1B(n)` (`0x004CA3E7`...`0x004CA441`, strings `0x0061F1F4`, `0x0061F214`,
/// `0x0061F290`): CDV logo, Related logo, intro.
pub const INTRO_VIDEOS: [&str; 3] = [
    "data/videos/NML_Logo_CDV.bik",
    "data/videos/NML_Logo_Related.bik",
    "data/videos/intro.bik",
];

/// Header data of an opened video (frame size, frame count, frame rate `fps_num / fps_den`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

/// A running video (menu root `+0xB8 != 0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPlayback {
    /// Path as in the command (`data/videos/<name>.bik`).
    pub path: String,
    pub info: VideoInfo,
    /// Clock time of the frame in which the host opened the video (frame 0).
    pub started_ms: u64,
    /// `+0xBC`: a key aborts (all callers of `FUN_0045D560` pass 1).
    pub abortable: bool,
    /// Image of the host under [`VIDEO_TEXTURE`].
    pub texture: TextureId,
}

impl VideoPlayback {
    /// Frame number at clock time `now` (deviation): `k = floor((now - start) * fps_num / (1000 * fps_den))`, `None`
    /// from `k >= frames` (at 25 frames/s after `frames * 40` ms; the original shows frames 1...N for one frame
    /// interval each and closes at the next step, `FUN_0058FEB0`). Before the start, frame 0 applies; a frame rate
    /// with 0 in the numerator or denominator ends the video immediately (broken header, no division by 0).
    pub fn frame_at(&self, now: u64) -> Option<u32> {
        let VideoInfo {
            frames,
            fps_num,
            fps_den,
            ..
        } = self.info;
        if fps_num == 0 || fps_den == 0 {
            return None;
        }
        let elapsed = u128::from(now.saturating_sub(self.started_ms));
        let k = elapsed * u128::from(fps_num) / (1000 * u128::from(fps_den));
        u32::try_from(k).ok().filter(|&k| k < frames)
    }
}

/// Target rectangle of the video in screen pixels (deviation): the 640 x 480 field of the original
/// (`FUN_0045D560` `0x0045D658`...`0x0045D690`: `x = 0...640`, `y0 = (480 - h)/2 ... y0 + h`) stretched to the whole
/// window, x and y independently: `[0, floor(y0 * H/480), W, floor((y0 + h) * H/480)]`, computed in `i64`. With width
/// or height 0 (minimized) an empty rectangle.
pub fn video_rect(screen: Screen, height: u32) -> [i32; 4] {
    if screen.width <= 0 || screen.height <= 0 {
        return [0; 4];
    }
    let h = i64::from(height);
    let full = i64::from(screen.height);
    let y0 = (480 - h).div_euclid(2);
    let y = |v: i64| {
        (v * full)
            .div_euclid(480)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    };
    [0, y(y0), screen.width, y(y0 + h)]
}

/// UV of the video quad for `dst` ([`video_rect`]): shifted by half a screen pixel so that every pixel center hits the
/// image point `(i + 0.5)/width`. The original sets the corners to `x - 0.5`, `y - 0.5` (`FUN_0058FF10`, pixel-center
/// compensation of D3D8, re/video, section "Drawing") and hits the texel centers at 1:1; the UI drawer emulates the
/// D3D8 rule with `+0.5` at the corners (`opennml-render` `build_vertices`), `DrawCmd` has no corners in half pixels -
/// shifting the UV is equivalent. At 640 x 480 the image is thereby texel-identical even with LINEAR (deviation),
/// stretching is filtered around the pixel centers. An empty rectangle (minimized) yields `[0, 0, 1, 1]`.
pub fn video_uv(dst: [i32; 4]) -> [f32; 4] {
    let (w, h) = (
        i64::from(dst[2]) - i64::from(dst[0]),
        i64::from(dst[3]) - i64::from(dst[1]),
    );
    if w <= 0 || h <= 0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let (du, dv) = (0.5 / w as f32, 0.5 / h as f32);
    [du, dv, 1.0 + du, 1.0 + dv]
}

/// Path of the speech track of a video (`FUN_0045D560` `0x0045D6B2`...`0x0045D799`, O-120): `"data/sprache/"` +
/// `Mid(12)` (removes `data/videos/`, without checking the prefix) + `Left(length - 3)` (the extension without the
/// dot) + `"ogg"`. Counting is in characters (the original counts bytes in cp1252, one byte per character); if the
/// path is shorter than 15 characters, the middle part stays empty (`Left` with a negative length returns nothing in
/// MFC).
pub fn speech_path(video_path: &str) -> String {
    let rest: Vec<char> = video_path.chars().skip(12).collect();
    let keep = rest.len().saturating_sub(3);
    let middle: String = rest[..keep].iter().collect();
    format!("data/sprache/{middle}ogg")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback(frames: u32) -> VideoPlayback {
        VideoPlayback {
            path: "data/videos/t.bik".into(),
            info: VideoInfo {
                width: 640,
                height: 356,
                frames,
                fps_num: 25,
                fps_den: 1,
            },
            started_ms: 0,
            abortable: true,
            texture: TextureId(0),
        }
    }

    #[test]
    fn frame_follows_the_clock_and_ends_after_n_frames() {
        // Deviation from the original: frame k = floor(t * 25 / 1000), end from k >= 145 (5,800 ms).
        let v = playback(145);
        assert_eq!(v.frame_at(0), Some(0));
        assert_eq!(v.frame_at(39), Some(0));
        assert_eq!(v.frame_at(40), Some(1));
        assert_eq!(v.frame_at(5_799), Some(144));
        assert_eq!(v.frame_at(5_800), None);
        // Jump past the end, very large clock: no overflow.
        assert_eq!(v.frame_at(u64::MAX), None);
        // Before the start, frame 0 applies.
        let later = VideoPlayback {
            started_ms: 1_000,
            ..v.clone()
        };
        assert_eq!(later.frame_at(500), Some(0));
        assert_eq!(later.frame_at(1_040), Some(1));
        // Broken frame rate or no frames: the video ends immediately.
        for (num, den, frames) in [(0, 1, 145), (25, 0, 145), (25, 1, 0)] {
            let mut bad = v.clone();
            bad.info.fps_num = num;
            bad.info.fps_den = den;
            bad.info.frames = frames;
            assert_eq!(bad.frame_at(0), None, "{num}/{den}, {frames} frames");
        }
    }

    // QUIRK O-66: deviation
    #[test]
    fn video_rect_scales_the_640x480_area() {
        let s = |width, height| Screen { width, height };
        assert_eq!(video_rect(s(640, 480), 356), [0, 62, 640, 418]);
        assert_eq!(video_rect(s(1024, 768), 356), [0, 99, 1024, 668]);
        assert_eq!(video_rect(s(1280, 1024), 480), [0, 0, 1280, 1024]);
        assert_eq!(video_rect(s(0, 0), 356), [0; 4]);
        assert_eq!(video_rect(s(1024, 0), 356), [0; 4]);
        assert_eq!(video_rect(s(0, 768), 356), [0; 4]);
        // Very wide, very tall: no overflow.
        assert_eq!(video_rect(s(i32::MAX, 480), 356), [0, 62, i32::MAX, 418]);
        let tall = video_rect(s(640, i32::MAX), 356);
        assert!(tall[1] < tall[3]);
    }

    // QUIRK O-66: deviation
    #[test]
    fn video_uv_samples_pixel_centres() {
        // 640 x 480 with 356 rows: 1:1, the texel center per pixel (corners `x - 0.5` of the original).
        let [u0, v0, u1, v1] = video_uv([0, 62, 640, 418]);
        assert_eq!((u0, v0), (0.5 / 640.0, 0.5 / 356.0));
        assert_eq!((u1, v1), (1.0 + 0.5 / 640.0, 1.0 + 0.5 / 356.0));
        // Pixel i of the target hits u = (i + 0.5)/width, also when stretched.
        let [u0, _, u1, _] = video_uv([0, 99, 1024, 668]);
        assert_eq!(u0 + 3.0 * (u1 - u0) / 1024.0, 3.5 / 1024.0);
        // Empty rectangle (minimized): no division by 0.
        assert_eq!(video_uv([0; 4]), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(video_uv([0, 5, 0, 5]), [0.0, 0.0, 1.0, 1.0]);
    }

    // QUIRK O-120: kept
    #[test]
    fn speech_path_cuts_like_mid_and_left() {
        assert_eq!(
            speech_path("data/videos/ind-1.bik"),
            "data/sprache/ind-1.ogg"
        );
        assert_eq!(
            speech_path("data/videos/intro.bik"),
            "data/sprache/intro.ogg"
        );
        // O-120: the prefix is not checked; too short -> empty middle part.
        assert_eq!(speech_path("x/eng.bik"), "data/sprache/ogg");
        assert_eq!(speech_path("data/videos/a"), "data/sprache/ogg");
        assert_eq!(speech_path(""), "data/sprache/ogg");
        assert_eq!(speech_path("data/videos/abc"), "data/sprache/ogg");
        assert_eq!(speech_path("data/videos/abcd"), "data/sprache/aogg");
        assert_eq!(speech_path("DATA/VIDEOS/Ü.bik"), "data/sprache/Ü.ogg");
    }
}
