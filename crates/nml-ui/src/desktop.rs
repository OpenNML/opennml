//! Image table of the desktop (`MenuDesktop` `+0x94…+0xA0`, `CMapStringToPtr`; re/menu-system 5.4, 8.4.10,
//! 12.3; O-92): background images with crossfade, drawn in the iteration order of the `CMap`.
//!
//! - `0x15E` (`FUN_0045D350`) and `0x15F` (`FUN_0045D3B0`) fetch the entry or create it (`FUN_0045D250`:
//!   only for a loaded texture, new in state 4) and set **unconditionally** `start = now`, `end = now + duration`,
//!   state 1 (fade in) or 2 (fade out). The quirks of O-92 follow from that: `0x15E` on a visible image collapses
//!   to alpha 0, `0x15F` on a faded-out one flashes up fully.
//! - `Draw` (`FUN_0045CC80`) per entry in `CMap` order: state 1 before the end -> `α = (now − start)·255 /
//!   (end − start)` (unsigned, clamped to 0...255), otherwise state 3 and fully visible in the same frame; state 2
//!   before the end -> `255 − …`, otherwise state 4 and not drawn; 3 full; 4 nothing.
//! - `CMap` order (`GetNextAssoc`, MFC 7.1): 17 buckets ascending, within the bucket the newest insertion first;
//!   bucket = [`cmap_bucket`]. Entries are removed only with the whole table ([`DesktopImages::clear`]).
//!
//! The state transition of `Draw` runs in the pre-pass ([`DesktopImages::tick`], from `crate::paint::tick_tree` at
//! the place of the desktop `Draw`); [`DesktopImages::paint`] then draws purely read-only what this pre-pass saw. An
//! image broadcast that comes in the same frame only after the desktop (from the `Tick` of a frame or the scrolling
//! of the credits) therefore takes effect, as in the original, only in the next frame.

use nml_rda::normalize_path;

use crate::bkg_paint::paint_fullscreen;
use crate::dialog::UiContext;
use crate::draw::DrawCmd;

/// Bucket count of the `CMap` (MFC default 17, `InitHashTable` at the first insertion).
const BUCKETS: u32 = 17;

/// Bucket of a key (`CMapStringToPtr::HashKey` with `TCHAR = char`): `h = h·33 + (signed char)c` over all
/// bytes (32-bit wrapping), bucket `h mod 17` (unsigned). The characters count as Latin-1 bytes, as the
/// `.dlg` is read (characters above U+00FF as `?`).
pub fn cmap_bucket(key: &str) -> u32 {
    let mut h: u32 = 0;
    for c in key.chars() {
        let byte = u8::try_from(c).unwrap_or(b'?');
        h = h.wrapping_mul(33).wrapping_add(byte as i8 as i32 as u32);
    }
    h % BUCKETS
}

/// State of an entry (`+0x0C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    /// 1: fade in.
    FadeIn,
    /// 2: fade out.
    FadeOut,
    /// 3: visible.
    Shown,
    /// 4: off (newly created or finished fading out).
    Off,
}

/// An entry of the table: key as broadcast (case counts, the `CMap` compares exactly), times of the program
/// clock in ms.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    key: String,
    bucket: u32,
    start: u64,
    end: u64,
    state: ImageState,
}

impl Entry {
    /// Opacity in `Draw` at time `now` without a state transition; `None` = not drawn.
    fn alpha(&self, now: u64) -> Option<u8> {
        match self.state {
            ImageState::FadeIn if now < self.end => Some(self.ramp(now)),
            ImageState::FadeIn | ImageState::Shown => Some(0xFF),
            ImageState::FadeOut if now < self.end => Some(0xFF - self.ramp(now)),
            ImageState::FadeOut | ImageState::Off => None,
        }
    }

    /// `((now − start)·255) / (end − start)` in 32-bit unsigned like `GetTickCount`, then clamped as `int` to
    /// 0...255 (`0x0045CD5A`...). Called only with `now < end`, so the denominator is not 0.
    fn ramp(&self, now: u64) -> u8 {
        let elapsed = now.wrapping_sub(self.start) as u32;
        let span = self.end.wrapping_sub(self.start) as u32;
        let v = elapsed.wrapping_mul(0xFF).checked_div(span).unwrap_or(0) as i32;
        v.clamp(0, 0xFF) as u8
    }
}

/// The image table (field of the tree, `UiTree::images`).
#[derive(Debug, Clone, Default)]
pub struct DesktopImages {
    /// Entries in insertion order.
    entries: Vec<Entry>,
    /// What the last pre-pass ([`Self::tick`]) found for drawing: entries in `CMap` order after the state
    /// transition, without the switched-off ones.
    drawn: Vec<Entry>,
}

impl DesktopImages {
    pub fn new() -> DesktopImages {
        DesktopImages::default()
    }

    /// Whether there is an entry for `path` (then a broadcast creates no new one and loads no texture).
    pub fn contains(&self, path: &str) -> bool {
        self.entries.iter().any(|e| e.key == path)
    }

    /// Fetch or newly create an entry (state 4, `FUN_0045D250`). The caller checks beforehand whether the texture
    /// loads.
    fn entry(&mut self, path: &str) -> &mut Entry {
        let i = match self.entries.iter().position(|e| e.key == path) {
            Some(i) => i,
            None => {
                self.entries.push(Entry {
                    key: path.to_string(),
                    bucket: cmap_bucket(path),
                    start: 0,
                    end: 0,
                    state: ImageState::Off,
                });
                self.entries.len() - 1
            }
        };
        &mut self.entries[i]
    }

    fn set(&mut self, path: &str, ms: u32, now: u64, state: ImageState) {
        if path.is_empty() {
            return;
        }
        let e = self.entry(path);
        e.start = now;
        e.end = now.wrapping_add(u64::from(ms));
        e.state = state;
    }

    /// `0x15E` (`FUN_0045D350`): fade in from alpha 0 over `ms`, even if the image is already visible (O-92). An
    /// empty path creates nothing (the texture `""` does not load).
    pub fn show(&mut self, path: &str, ms: u32, now: u64) {
        self.set(path, ms, now, ImageState::FadeIn);
    }

    /// `0x15F` (`FUN_0045D3B0`): fade out from 255 over `ms`, also for a switched-off or unknown image -
    /// it then flashes up fully (O-92).
    pub fn hide(&mut self, path: &str, ms: u32, now: u64) {
        self.set(path, ms, now, ImageState::FadeOut);
    }

    /// Indexes in `CMap` order: bucket ascending, within the bucket the newest insertion first.
    fn order(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.entries.len()).collect();
        idx.sort_by_key(|&i| (self.entries[i].bucket, std::cmp::Reverse(i)));
        idx
    }

    /// `MenuManager_CloseAllAndStopMusic` (`0x0045D0F0`): release every texture of the table, then `RemoveAll` on
    /// `+0x94` - all images are gone immediately, even in the middle of a crossfade, and nothing is drawn any more.
    /// A later `0x15E`/`0x15F` creates its entry anew.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.drawn.clear();
    }

    /// End all crossfades immediately (fading in -> visible, fading out -> off): the end state of static
    /// drawing (`crate::dialog::paint_static`), which shows the images fully faded in.
    pub(crate) fn finish_fades(&mut self) {
        for e in &mut self.entries {
            e.state = match e.state {
                ImageState::FadeIn | ImageState::Shown => ImageState::Shown,
                ImageState::FadeOut | ImageState::Off => ImageState::Off,
            };
        }
    }

    /// Pre-pass of `Draw` (`FUN_0045CC80`) at time `now`: state 1 after the end -> 3, state 2 after the end ->
    /// 4; remembers the entries to draw for [`Self::paint`].
    pub fn tick(&mut self, now: u64) {
        let mut drawn = Vec::new();
        for i in self.order() {
            let e = &mut self.entries[i];
            match e.state {
                ImageState::FadeIn if now >= e.end => e.state = ImageState::Shown,
                ImageState::FadeOut if now >= e.end => e.state = ImageState::Off,
                _ => {}
            }
            if e.state != ImageState::Off {
                drawn.push(e.clone());
            }
        }
        self.drawn = drawn;
    }

    /// Draws what the last pre-pass found, with the opacity at time `now` (the same time as the pre-pass):
    /// one full-screen quad per image ([`paint_fullscreen`], 8.4.10). An image whose texture does not load is omitted.
    pub fn paint(&self, ctx: &mut UiContext, now: u64, out: &mut Vec<DrawCmd>) {
        for e in &self.drawn {
            let Some(alpha) = e.alpha(now) else {
                continue;
            };
            if let Some(tex) = ctx.assets.texture(ctx.vfs, &normalize_path(&e.key)) {
                paint_fullscreen(tex, ctx.screen, alpha, out);
            }
        }
    }

    /// What `Draw` would show at time `now` (key, opacity) in `CMap` order, without a state transition - for
    /// queries and reports.
    pub fn visible(&self, now: u64) -> Vec<(String, u8)> {
        self.order()
            .into_iter()
            .filter_map(|i| {
                let e = &self.entries[i];
                e.alpha(now).map(|a| (e.key.clone(), a))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "DATA/MENÜ/BACKGROUND_1024X1024_SID.TGA";
    const WID: &str = "DATA/MENÜ/BACKGROUND_1024X1024_WID.TGA";
    const PAT: &str = "DATA/MENÜ/BACKGROUND_1024X1024_PAT.TGA";

    fn alpha_of(images: &DesktopImages, key: &str, now: u64) -> Option<u8> {
        images
            .visible(now)
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, a)| a)
    }

    #[test]
    fn clear_drops_every_image_at_once() {
        // `MenuManager_CloseAllAndStopMusic` (`RemoveAll`): even a fading-out image is gone immediately and is no
        // longer drawn in the same frame; a later 0x15E creates the entry anew (fade in from alpha 0).
        let mut images = DesktopImages::new();
        images.show(SID, 0, 0);
        images.show(WID, 0, 0);
        images.tick(0);
        images.hide(WID, 1000, 100);
        images.tick(200);
        assert_eq!(images.visible(200).len(), 2);
        images.clear();
        assert_eq!(images.visible(200), []);
        assert!(images.drawn.is_empty(), "nothing left to draw");
        assert!(!images.contains(WID) && !images.contains(SID));
        images.show(WID, 1000, 300);
        assert_eq!(alpha_of(&images, WID, 300), Some(0));
    }

    #[test]
    fn image_states_follow_the_table() {
        // 12.3, O-92: fade in from alpha 0, full in the frame of the end; another 0x15E on a visible image collapses
        // to alpha 0; 0x15F in the middle of fading in jumps to full and fades out; 0x15F on a switched-off image
        // flashes up.
        let mut images = DesktopImages::new();
        images.show(PAT, 1000, 100);
        images.tick(100);
        assert_eq!(images.drawn.len(), 1);
        assert_eq!(alpha_of(&images, PAT, 100), Some(0));
        assert_eq!(alpha_of(&images, PAT, 600), Some(127), "500*255/1000");
        assert_eq!(alpha_of(&images, PAT, 1099), Some(254));
        images.tick(1100);
        assert_eq!(images.entries[0].state, ImageState::Shown);
        assert_eq!(alpha_of(&images, PAT, 1100), Some(255));
        // Collapse: visible -> 0x15E -> alpha 0.
        images.show(PAT, 1000, 2000);
        assert_eq!(alpha_of(&images, PAT, 2000), Some(0));
        // Fade out in the middle of fading in: jump to full.
        assert_eq!(alpha_of(&images, PAT, 2250), Some(63));
        images.hide(PAT, 500, 2250);
        assert_eq!(alpha_of(&images, PAT, 2250), Some(255));
        assert_eq!(alpha_of(&images, PAT, 2500), Some(128), "255 - 250*255/500");
        images.tick(2750);
        assert_eq!(images.entries[0].state, ImageState::Off);
        assert!(images.drawn.is_empty(), "finished fading out: not drawn");
        assert_eq!(alpha_of(&images, PAT, 2750), None);
        // Flash: switched off -> 0x15F -> full, then off.
        images.hide(PAT, 500, 3000);
        images.tick(3000);
        assert_eq!(alpha_of(&images, PAT, 3000), Some(255));
        // An unknown image also flashes up (the entry is created with a loaded texture, state 4 -> 2).
        images.hide(SID, 500, 3000);
        assert_eq!(alpha_of(&images, SID, 3000), Some(255));
        // Duration 0: full or off in the first pre-pass; an empty path creates nothing.
        images.show(WID, 0, 4000);
        images.tick(4000);
        assert_eq!(alpha_of(&images, WID, 4000), Some(255));
        images.show("", 500, 4000);
        assert!(!images.contains(""));
    }

    #[test]
    fn cmap_buckets_follow_the_hash() {
        // h = h*33 + (signed char)c, mod 17; "Ü" (0xDC) counts as negative.
        assert_eq!(cmap_bucket(SID), 7);
        assert_eq!(cmap_bucket(WID), 10);
        assert_eq!(cmap_bucket(PAT), 12);
        assert_eq!(cmap_bucket(""), 0);
        assert_eq!(cmap_bucket("A"), 65 % 17);
        // Lowercase gives other buckets (key as broadcast).
        assert_eq!(cmap_bucket("data/menü/background_1024x1024_sid.tga"), 12);
    }

    #[test]
    fn cmap_order_is_bucket_then_newest_first() {
        let mut images = DesktopImages::new();
        for key in [PAT, SID, WID] {
            images.show(key, 0, 0);
        }
        images.tick(0);
        let keys: Vec<String> = images.visible(0).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, [SID, WID, PAT]);
        // Two keys in the same bucket: the most recently created first.
        let a = "A";
        let b = (0..200)
            .map(|i| format!("K{i}"))
            .find(|k| cmap_bucket(k) == cmap_bucket(a))
            .unwrap();
        let mut images = DesktopImages::new();
        images.show(a, 0, 0);
        images.show(&b, 0, 0);
        let keys: Vec<String> = images.visible(0).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, [b.clone(), a.to_string()]);
        // Showing again does not change the order (the entry stays where it is).
        images.show(a, 0, 1);
        let keys: Vec<String> = images.visible(1).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, [b, a.to_string()]);
    }
}
