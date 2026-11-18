//! Generating missing font atlases with GDI.
//!
//! The original creates a missing atlas at run time (`FUN_0057B570` -> `FUN_0057A060`), writes `.fnt`
//! (`FUN_0057A7E0`) and `.tga` (`FUN_0057A960`) into the game directory `data/fonts/` and loads them anew
//! (re/ui-rendering 3.2, 3.8.7). The reimplementation computes the same way but puts the files into a
//! cache outside the installation (O-59): `NML_FONT_CACHE`, otherwise
//! `%LOCALAPPDATA%\OpenNML\fonts`. The in-memory fallback of the original (`FUN_0057B320`, alpha = blue, O-84)
//! does not exist: if writing fails, the generated atlas still applies (only without a cache).
//!
//! GDI runs through its own `extern "system"` declarations (no new dependency); without Windows there is no
//! generator, then the fallback from `FontBook::load` applies.

// Without Windows only the tests use the platform-neutral parts (formats, cache paths).
#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::fonts::{AtlasGenerator, atlas_path, texture_path};

#[cfg(windows)]
use image::RgbaImage;
use opennml_formats::localisation::FontDef;
#[cfg(windows)]
use opennml_formats::text::decode_cp1252;
#[cfg(windows)]
use opennml_formats::tga::decode_tga_game;

/// Environment variable for a custom cache of the generated atlases.
pub const CACHE_ENV: &str = "NML_FONT_CACHE";
/// Upper limit of the texture size (`FUN_00579DD0`).
const MAX_TEXTURE: u32 = 2048;
/// Header of a `.fnt` (`FUN_0057A7E0`: `FONTFILE V%s` with `1.0`, blank line, `[CHARINFOS]`; text mode `\r\n`).
const FNT_HEADER: &str = "FONTFILE V1.0\r\n\r\n[CHARINFOS]\r\n";

/// Cache of the generated atlases: `NML_FONT_CACHE`, otherwise `%LOCALAPPDATA%\OpenNML\fonts`; `None`
/// without both. Never the installation: [`cache_dir_for`] checks that against the game directory.
pub fn default_cache_dir() -> Option<PathBuf> {
    cache_dir_from(
        std::env::var_os(CACHE_ENV),
        std::env::var_os("LOCALAPPDATA"),
    )
}

/// Core of [`default_cache_dir`]; empty values count as unset.
fn cache_dir_from(explicit: Option<OsString>, local_app_data: Option<OsString>) -> Option<PathBuf> {
    user_path_from(explicit, local_app_data, "fonts")
}

/// Location of a file or folder of the reimplementation belonging to the user: the explicit value (environment
/// variable), otherwise `<LOCALAPPDATA>\OpenNML\<name>`; empty values count as unset. Shared by the cache of the
/// atlases and the settings file (`opennml-client`); whether the location would lie inside the installation is checked
/// by the caller with [`is_inside`].
pub fn user_path_from(
    explicit: Option<OsString>,
    local_app_data: Option<OsString>,
    name: &str,
) -> Option<PathBuf> {
    match explicit.filter(|v| !v.is_empty()) {
        Some(path) => Some(PathBuf::from(path)),
        None => local_app_data
            .filter(|v| !v.is_empty())
            .map(|dir| PathBuf::from(dir).join(opennml_rda::USER_DIR).join(name)),
    }
}

/// Path as a list of components for comparison: absolute, `.`/`..` resolved, the longest existing
/// ancestor canonicalized (links, short names); on Windows case-insensitive.
fn comparable(path: &Path) -> Vec<String> {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut lexical = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::ParentDir => {
                lexical.pop();
            }
            Component::CurDir => {}
            other => lexical.push(other),
        }
    }
    let mut tail = Vec::new();
    let mut head = lexical.as_path();
    let mut resolved = loop {
        if let Ok(found) = head.canonicalize() {
            break found;
        }
        match (head.parent(), head.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                head = parent;
            }
            _ => break lexical.clone(),
        }
    };
    if resolved != lexical {
        for name in tail.iter().rev() {
            resolved.push(name);
        }
    }
    resolved
        .components()
        .map(|part| {
            let text = part.as_os_str().to_string_lossy();
            if cfg!(windows) {
                text.to_lowercase()
            } else {
                text.into_owned()
            }
        })
        .collect()
}

/// Is `dir` inside `base` (or equal to it)? Compares whole components (`Game2` is not inside `Game`).
pub fn is_inside(dir: &Path, base: &Path) -> bool {
    comparable(dir).starts_with(&comparable(base))
}

/// [`default_cache_dir`], unless it would lie in the game directory `game_dir` (then `None` with a warning).
pub fn cache_dir_for(game_dir: &Path) -> Option<PathBuf> {
    let Some(dir) = default_cache_dir() else {
        log::warn!(
            "No cache for font atlases ({CACHE_ENV} and LOCALAPPDATA are missing): missing \
             atlases are not generated"
        );
        return None;
    };
    if is_inside(&dir, game_dir) {
        log::warn!(
            "Cache \"{}\" lies in the installation \"{}\": missing atlases are not generated",
            dir.display(),
            game_dir.display()
        );
        return None;
    }
    Some(dir)
}

/// The generator for the game directory `game_dir`: GDI with [`cache_dir_for`]; `None` without Windows or
/// without a permitted cache (then the fallback from `FontBook::load` applies).
pub fn default_generator(game_dir: &Path) -> Option<Box<dyn AtlasGenerator>> {
    #[cfg(windows)]
    {
        let cache_dir = cache_dir_for(game_dir)?;
        Some(Box::new(GdiAtlasGenerator { cache_dir }))
    }
    #[cfg(not(windows))]
    {
        let _ = game_dir;
        None
    }
}

/// Texture size for `tmHeight · 16` (`FUN_00579DD0`, re/ui-rendering 3.8.7): smallest power of two `≥ value`,
/// values above 1024 always 2048, values `≤ 2` unchanged (negative ones count as 0).
pub(crate) fn texture_size(value: i64) -> u32 {
    if value <= 2 {
        return value.max(0) as u32;
    }
    if value > 1024 {
        return MAX_TEXTURE;
    }
    let mut size = 1u32;
    while i64::from(size) < value {
        size <<= 1;
    }
    size
}

/// Text of the `.fnt` like `FUN_0057A7E0`: header, then per code `%i %i %i %f %f %f %f` (`code width height u0 u1 v0
/// v1`) with `\r\n`. UVs from the cell boundaries as `float` (`zx/T`, `(zx+z)/T`, `zy/T`, `(zy+z)/T`, cell
/// `z = T/16`); all values are multiples of 1/16 and therefore exact in `%f`.
pub(crate) fn format_fnt(size: u32, widths: &[i32; 256], heights: &[i32; 256]) -> String {
    use std::fmt::Write as _;
    let cell = size / 16;
    let t = size as f32;
    let mut out = String::from(FNT_HEADER);
    for code in 0..256u32 {
        let (zx, zy) = ((code % 16) * cell, (code / 16) * cell);
        let _ = write!(
            out,
            "{code} {} {} {:.6} {:.6} {:.6} {:.6}\r\n",
            widths[code as usize],
            heights[code as usize],
            zx as f32 / t,
            (zx + cell) as f32 / t,
            zy as f32 / t,
            (zy + cell) as f32 / t,
        );
    }
    out
}

/// `.tga` like `FUN_0057A960`: 18-byte header (type 2, 32 bit, descriptor 0 = origin bottom left), then the
/// pixels in DIB order (bottom row first): B, G, R unchanged, `A = ⌊(R + G + B) / 3⌋` (O-84).
/// `bgr_bottom_up` is the 24-bit DIB with `stride` bytes per row (padded to 4 bytes).
pub(crate) fn encode_tga(size: u32, stride: usize, bgr_bottom_up: &[u8]) -> Vec<u8> {
    let side = size as usize;
    let mut tga = Vec::with_capacity(18 + side * side * 4);
    tga.extend_from_slice(&[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    tga.extend_from_slice(&(size as u16).to_le_bytes());
    tga.extend_from_slice(&(size as u16).to_le_bytes());
    tga.extend_from_slice(&[32, 0]);
    for row in bgr_bottom_up.chunks(stride).take(side) {
        for bgr in row[..side * 3].chunks_exact(3) {
            let sum = u32::from(bgr[0]) + u32::from(bgr[1]) + u32::from(bgr[2]);
            tga.extend_from_slice(&[bgr[0], bgr[1], bgr[2], (sum / 3) as u8]);
        }
    }
    tga
}

/// File names in the cache: file name of the atlas (`atlas_path` without `data/fonts/`) and the `.tga` of the
/// same name. `None` for a font name that is not a safe file name (empty, path or control characters).
fn cache_names(def: &FontDef, px: i32) -> Option<(String, String)> {
    let unsafe_char = |c: char| c.is_control() || r#"<>:"/\|?*"#.contains(c);
    if def.face.is_empty() || def.face.contains(unsafe_char) {
        return None;
    }
    let fnt = atlas_path(def, px);
    let name = fnt.strip_prefix("data/fonts/")?.to_string();
    let tga = texture_path(&name);
    Some((name, tga))
}

/// Reads a pair from the cache; `None` if a file is missing or corrupted.
#[cfg(windows)]
fn read_cache(fnt: &Path, tga: &Path) -> Option<(String, RgbaImage)> {
    if !fnt.is_file() || !tga.is_file() {
        return None;
    }
    let text = decode_cp1252(&std::fs::read(fnt).ok()?);
    let image = std::fs::read(tga)
        .ok()
        .and_then(|bytes| decode_tga_game(&bytes).ok());
    match image {
        Some(image)
            if text.contains("[CHARINFOS]") && image.width() >= 16 && image.height() >= 16 =>
        {
            Some((text, image))
        }
        _ => {
            log::warn!(
                "Atlas in the cache \"{}\" is unusable: it will be generated anew",
                fnt.display()
            );
            None
        }
    }
}

/// Writes via a temporary file and then renames it (no half atlas after an abort).
#[cfg(windows)]
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Stores the pair, the `.tga` first: an existing `.fnt` means a complete `.tga`.
#[cfg(windows)]
fn store(dir: &Path, fnt: &Path, tga: &Path, text: &str, image: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_atomic(tga, image)?;
    write_atomic(fnt, text.as_bytes())
}

/// Generates atlases with GDI like `FUN_0057A060` and stores them in `cache_dir` (never in the installation:
/// [`cache_dir_for`]). The cache is read first (`<cache_dir>/<atlas file name>.fnt` + `.tga`).
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct GdiAtlasGenerator {
    pub cache_dir: PathBuf,
}

#[cfg(windows)]
impl AtlasGenerator for GdiAtlasGenerator {
    fn generate(&self, def: &FontDef, px: i32) -> Option<(String, RgbaImage)> {
        if px <= 0 {
            return None;
        }
        let Some((fnt_name, tga_name)) = cache_names(def, px) else {
            log::warn!("Font name \"{}\" is not a permitted file name", def.face);
            return None;
        };
        let fnt_file = self.cache_dir.join(fnt_name);
        let tga_file = self.cache_dir.join(tga_name);
        if let Some(cached) = read_cache(&fnt_file, &tga_file) {
            return Some(cached);
        }
        let face = gdi::face_bytes(&def.face)?;
        let rendered = gdi::render(&face, def, px)?;
        if !rendered.face.eq_ignore_ascii_case(&def.face) {
            log::warn!(
                "Font \"{}\" is not installed, GDI takes \"{}\"",
                def.face,
                rendered.face
            );
        }
        let text = format_fnt(rendered.size, &rendered.widths, &rendered.heights);
        let tga = encode_tga(rendered.size, rendered.stride, &rendered.bits);
        let image = decode_tga_game(&tga).ok()?;
        match store(&self.cache_dir, &fnt_file, &tga_file, &text, &tga) {
            Ok(()) => log::info!("Atlas generated: {}", fnt_file.display()),
            Err(e) => log::warn!(
                "Atlas \"{}\" not stored in the cache: {e}",
                fnt_file.display()
            ),
        }
        Some((text, image))
    }
}

/// GDI part of `FUN_0057A060` (re/ui-rendering 3.8.7) via own Win32 declarations.
#[cfg(windows)]
mod gdi {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr;

    use opennml_formats::localisation::FontDef;
    use opennml_formats::text::decode_cp1252;

    type Hdc = *mut c_void;
    type Hgdiobj = *mut c_void;

    const ANSI_CHARSET: u32 = 0;
    const OUT_DEFAULT_PRECIS: u32 = 0;
    const CLIP_DEFAULT_PRECIS: u32 = 0;
    const DEFAULT_QUALITY: u32 = 0;
    const DEFAULT_PITCH: u32 = 0;
    const DIB_RGB_COLORS: u32 = 0;
    const BI_RGB: u32 = 0;
    const OPAQUE: i32 = 2;
    const DT_SINGLELINE: u32 = 0x0020;
    const DT_NOPREFIX: u32 = 0x0800;
    /// `LF_FACESIZE`: font name at most 31 bytes plus NUL.
    const LF_FACESIZE: usize = 32;

    /// `RECT` (windef.h).
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    /// `TEXTMETRICA` (wingdi.h): 11 x `LONG`, 9 x `BYTE`, padded to 4 (56 bytes).
    #[repr(C)]
    struct TextMetricA {
        tm_height: i32,
        tm_ascent: i32,
        tm_descent: i32,
        tm_internal_leading: i32,
        tm_external_leading: i32,
        tm_ave_char_width: i32,
        tm_max_char_width: i32,
        tm_weight: i32,
        tm_overhang: i32,
        tm_digitized_aspect_x: i32,
        tm_digitized_aspect_y: i32,
        tm_first_char: u8,
        tm_last_char: u8,
        tm_default_char: u8,
        tm_break_char: u8,
        tm_italic: u8,
        tm_underlined: u8,
        tm_struck_out: u8,
        tm_pitch_and_family: u8,
        tm_char_set: u8,
    }

    /// `BITMAPINFOHEADER` (wingdi.h, 40 bytes).
    #[repr(C)]
    struct BitmapInfoHeader {
        bi_size: u32,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: u32,
        bi_size_image: u32,
        bi_x_pels_per_meter: i32,
        bi_y_pels_per_meter: i32,
        bi_clr_used: u32,
        bi_clr_important: u32,
    }

    /// `BITMAPINFO` with one (unused) `RGBQUAD`.
    #[repr(C)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 1],
    }

    const _: () = assert!(size_of::<Rect>() == 16);
    const _: () = assert!(size_of::<TextMetricA>() == 56);
    const _: () = assert!(size_of::<BitmapInfoHeader>() == 40);
    const _: () = assert!(size_of::<BitmapInfo>() == 44);

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
        fn DeleteDC(hdc: Hdc) -> i32;
        #[allow(clippy::too_many_arguments)]
        fn CreateFontA(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strike_out: u32,
            char_set: u32,
            out_precision: u32,
            clip_precision: u32,
            quality: u32,
            pitch_and_family: u32,
            face_name: *const u8,
        ) -> Hgdiobj;
        fn SelectObject(hdc: Hdc, object: Hgdiobj) -> Hgdiobj;
        fn DeleteObject(object: Hgdiobj) -> i32;
        fn GetTextMetricsA(hdc: Hdc, metrics: *mut TextMetricA) -> i32;
        fn GetTextFaceA(hdc: Hdc, count: i32, face_name: *mut u8) -> i32;
        fn CreateDIBSection(
            hdc: Hdc,
            info: *const BitmapInfo,
            usage: u32,
            bits: *mut *mut c_void,
            section: *mut c_void,
            offset: u32,
        ) -> Hgdiobj;
        fn SetTextColor(hdc: Hdc, color: u32) -> u32;
        fn SetBkColor(hdc: Hdc, color: u32) -> u32;
        fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
        fn GetCharWidthA(hdc: Hdc, first: u32, last: u32, widths: *mut i32) -> i32;
        fn GdiFlush() -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn DrawTextA(hdc: Hdc, text: *const u8, count: i32, rect: *mut Rect, format: u32) -> i32;
    }

    /// Memory DC, released with `DeleteDC` on drop (the original uses `DeleteObject` here, a leak without effect on
    /// the image; re/ui-rendering 3.8.7 side finding).
    struct Dc(Hdc);

    impl Drop for Dc {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a valid DC delivered by `CreateCompatibleDC`; all selections have been reset at
            // this point (the `Selection` values live shorter).
            unsafe {
                DeleteDC(self.0);
            }
        }
    }

    /// GDI object (font, bitmap), released with `DeleteObject` on drop.
    struct Object(Hgdiobj);

    impl Drop for Object {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a valid GDI object that is no longer selected into a DC (the associated
            // `Selection` has been dropped beforehand).
            unsafe {
                DeleteObject(self.0);
            }
        }
    }

    /// A selection into a DC; on drop the previous object is selected back.
    struct Selection {
        dc: Hdc,
        previous: Hgdiobj,
    }

    impl Drop for Selection {
        fn drop(&mut self) {
            // SAFETY: `dc` outlives this selection, `previous` is the previous object of the same DC delivered by
            // `SelectObject`.
            unsafe {
                SelectObject(self.dc, self.previous);
            }
        }
    }

    fn select(dc: &Dc, object: &Object) -> Option<Selection> {
        // SAFETY: DC and object are valid; the object outlives the selection (order of the variables or fields,
        // see `FontDc`).
        let previous = unsafe { SelectObject(dc.0, object.0) };
        (!previous.is_null()).then_some(Selection { dc: dc.0, previous })
    }

    /// Memory DC with the selected font. Fields drop in declaration order: first reset the selection, then delete
    /// the font, then the DC.
    struct FontDc {
        _selection: Selection,
        _font: Object,
        dc: Dc,
    }

    /// Font name as NUL-terminated cp1252 bytes for `CreateFontA`; `None` for characters outside cp1252,
    /// NUL or more than 31 bytes (`LF_FACESIZE`).
    pub(super) fn face_bytes(face: &str) -> Option<Vec<u8>> {
        let mut bytes = Vec::with_capacity(face.len() + 1);
        for c in face.chars() {
            let byte = if c.is_ascii() {
                c as u8
            } else {
                (0x80..=0xFFu8).find(|&b| decode_cp1252(&[b]).starts_with(c))?
            };
            if byte == 0 {
                return None;
            }
            bytes.push(byte);
        }
        if bytes.len() >= LF_FACESIZE {
            log::warn!("Font name \"{face}\" is longer than 31 bytes");
            return None;
        }
        bytes.push(0);
        Some(bytes)
    }

    /// `CreateFontA(px, 0, 0, 0, weight, Italic, Underline, StrikeOut, ANSI_CHARSET, 0, 0, DEFAULT_QUALITY,
    /// 0, name)` in a memory DC (`FUN_00428860` -> `FUN_0057B570`, re/ui-rendering 3.2).
    fn open_font(face: &[u8], def: &FontDef, px: i32) -> Option<FontDc> {
        debug_assert_eq!(face.last(), Some(&0));
        // SAFETY: `CreateCompatibleDC(NULL)` creates a memory DC compatible with the screen; no pointer is
        // passed.
        let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
        if dc.is_null() {
            return None;
        }
        let dc = Dc(dc);
        // SAFETY: `face` is NUL-terminated (`face_bytes`) and lives across the call; the other values are
        // numbers.
        let font = unsafe {
            CreateFontA(
                px,
                0,
                0,
                0,
                def.weight,
                u32::from(def.italic),
                u32::from(def.underline),
                u32::from(def.strike_out),
                ANSI_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                DEFAULT_QUALITY,
                DEFAULT_PITCH,
                face.as_ptr(),
            )
        };
        if font.is_null() {
            return None;
        }
        let font = Object(font);
        let selection = select(&dc, &font)?;
        Some(FontDc {
            _selection: selection,
            _font: font,
            dc,
        })
    }

    /// Name of the font that GDI actually chose (`GetTextFaceA`).
    fn chosen_face(font: &FontDc) -> String {
        let mut name = [0u8; LF_FACESIZE];
        // SAFETY: The buffer holds `LF_FACESIZE` bytes, GDI may write that many (with NUL).
        let copied = unsafe { GetTextFaceA(font.dc.0, LF_FACESIZE as i32, name.as_mut_ptr()) };
        let len = name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(usize::try_from(copied).unwrap_or(0).min(LF_FACESIZE));
        decode_cp1252(&name[..len])
    }

    /// Is the font `face` installed (does GDI choose it without a substitute)?
    #[cfg(test)]
    pub(super) fn installed(face: &str) -> bool {
        let def = FontDef {
            name: face.to_string(),
            size: opennml_formats::localisation::FontSize::Fixed(20),
            weight: 0,
            italic: false,
            underline: false,
            strike_out: false,
            texture_font: true,
            face: face.to_string(),
        };
        face_bytes(face)
            .and_then(|bytes| open_font(&bytes, &def, 20))
            .is_some_and(|font| chosen_face(&font).eq_ignore_ascii_case(face))
    }

    /// Result of [`render`].
    pub(super) struct Rendered {
        /// Texture size `T` (width = height).
        pub size: u32,
        /// Bytes per DIB row.
        pub stride: usize,
        /// The 24-bit DIB, bottom row first, BGR.
        pub bits: Vec<u8>,
        /// `GetCharWidthA` per code.
        pub widths: [i32; 256],
        /// Return value of `DrawTextA` per code.
        pub heights: [i32; 256],
        /// Font name chosen by GDI.
        pub face: String,
    }

    /// `FUN_0057A060`: `T` from `tmHeight · 16`, 24-bit DIB `T × T` bottom to top, zeroed, white on
    /// black (opaque); per code 0...255 `DrawTextA(&c, 1, cell, DT_SINGLELINE|DT_NOPREFIX)` clipped to the cell
    /// `T/16`, `height` = return value, `width` = `GetCharWidthA(c, c)`. All handles are also released on
    /// error paths (`Dc`, `Object`, `Selection`).
    pub(super) fn render(face: &[u8], def: &FontDef, px: i32) -> Option<Rendered> {
        let font = open_font(face, def, px)?;
        let dc = font.dc.0;
        // SAFETY: `TextMetricA` consists only of numbers; the zero initialization is valid.
        let mut metrics: TextMetricA = unsafe { std::mem::zeroed() };
        // SAFETY: `dc` is valid, `metrics` is a writable `TEXTMETRICA`.
        if unsafe { GetTextMetricsA(dc, &mut metrics) } == 0 {
            return None;
        }
        let size = super::texture_size(i64::from(metrics.tm_height) * 16);
        if size < 16 {
            return None;
        }
        let side = size as usize;
        let stride = (side * 3 + 3) & !3;
        let info = BitmapInfo {
            header: BitmapInfoHeader {
                bi_size: size_of::<BitmapInfoHeader>() as u32,
                bi_width: size as i32,
                // Positive: bottom to top in memory (as in the original).
                bi_height: size as i32,
                bi_planes: 1,
                bi_bit_count: 24,
                bi_compression: BI_RGB,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            colors: [0],
        };
        let mut bits: *mut c_void = ptr::null_mut();
        // SAFETY: `info` is a complete `BITMAPINFO` for 24 bit without a color table, `bits` receives the
        // pointer to the pixel memory; no file mapping (`section` = NULL).
        let bitmap =
            unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0) };
        if bitmap.is_null() {
            return None;
        }
        let bitmap = Object(bitmap);
        if bits.is_null() {
            return None;
        }
        let dc_ref = &font.dc;
        let _bitmap_selection = select(dc_ref, &bitmap)?;
        // SAFETY: `bits` points to `stride · T` bytes of pixel memory of the living DIB section; zeroed =
        // black. Then text white, background black and opaque.
        unsafe {
            ptr::write_bytes(bits.cast::<u8>(), 0, stride * side);
            SetTextColor(dc, 0x00FF_FFFF);
            SetBkColor(dc, 0);
            SetBkMode(dc, OPAQUE);
        }
        let cell = (size / 16) as i32;
        let mut widths = [0i32; 256];
        let mut heights = [0i32; 256];
        for code in 0..256u32 {
            let byte = code as u8;
            let (zx, zy) = ((code % 16) as i32 * cell, (code / 16) as i32 * cell);
            let mut rect = Rect {
                left: zx,
                top: zy,
                right: zx + cell,
                bottom: zy + cell,
            };
            let mut width = 0i32;
            // SAFETY: `byte` is a readable character of length 1, `rect` and `width` are writable; the DC
            // has font and bitmap selected.
            unsafe {
                heights[code as usize] =
                    DrawTextA(dc, &byte, 1, &mut rect, DT_SINGLELINE | DT_NOPREFIX);
                if GetCharWidthA(dc, code, code, &mut width) != 0 {
                    widths[code as usize] = width;
                }
            }
        }
        // SAFETY: `GdiFlush` completes pending drawing commands, afterwards the pixel memory (still
        // `stride · T` bytes, bitmap alive) is complete and may be read.
        let pixels = unsafe {
            GdiFlush();
            std::slice::from_raw_parts(bits.cast::<u8>(), stride * side).to_vec()
        };
        Some(Rendered {
            size,
            stride,
            bits: pixels,
            widths,
            heights,
            face: chosen_face(&font),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_formats::localisation::FontSize;
    use opennml_formats::tga::decode_tga_game;

    fn font_def(name: &str, face: &str, weight: i32) -> FontDef {
        FontDef {
            name: name.to_string(),
            size: FontSize::Fixed(0),
            weight,
            italic: false,
            underline: false,
            strike_out: false,
            texture_font: true,
            face: face.to_string(),
        }
    }

    // QUIRK O-59: deviation
    #[test]
    fn texture_size_is_capped_at_2048() {
        // FUN_00579DD0 (re/ui-rendering 3.8.7): smallest power of two >= value, upper limit 2048, <= 2 unchanged.
        assert_eq!(texture_size(16 * 16), 256);
        assert_eq!(texture_size(17 * 16), 512);
        assert_eq!(texture_size(27 * 16), 512);
        assert_eq!(texture_size(33 * 16), 1024);
        assert_eq!(texture_size(63 * 16), 1024);
        assert_eq!(texture_size(64 * 16), 1024);
        assert_eq!(texture_size(65 * 16), 2048);
        assert_eq!(texture_size(1025), 2048);
        assert_eq!(texture_size(1_000_000), 2048);
        assert_eq!(texture_size(3), 4);
        assert_eq!(texture_size(2), 2);
        assert_eq!(texture_size(1), 1);
    }

    #[cfg(windows)]
    #[test]
    fn huge_fonts_get_a_2048_atlas() {
        // Arial 160 px: tmHeight * 16 > 2048 -> 2048^2, cell 128, glyphs clipped to the cell.
        let dir = tempfile::tempdir().unwrap();
        let generator = GdiAtlasGenerator {
            cache_dir: dir.path().to_path_buf(),
        };
        let (text, image) = generator
            .generate(&font_def("Large", "Arial", 0), 160)
            .unwrap();
        assert_eq!(image.dimensions(), (2048, 2048));
        let font = opennml_formats::fnt::parse_fnt(&text).unwrap();
        assert_eq!(font.glyphs[1].u0, 0.0625);
        assert_eq!(font.glyphs[1].u1, 0.125);
        assert_eq!(font.glyphs[17].v0, 0.0625);
        assert!(
            font.glyphs[usize::from(b'W')].height > 128,
            "tmHeight > cell"
        );
    }

    #[test]
    fn fnt_text_has_the_format_of_the_original() {
        // FUN_0057A7E0: header, blank line, `[CHARINFOS]`, per code `%i %i %i %f %f %f %f`, `\r\n` (text mode).
        // Numbers as in the shipped `garamond_27_600_0_0_0.fnt` (lines 1 and 255).
        let mut widths = [19; 256];
        widths[255] = 11;
        widths[7] = -2;
        let text = format_fnt(512, &widths, &[27; 256]);
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines[..3], ["FONTFILE V1.0", "", "[CHARINFOS]"]);
        assert_eq!(lines[3], "0 19 27 0.000000 0.062500 0.000000 0.062500");
        assert_eq!(lines[4], "1 19 27 0.062500 0.125000 0.000000 0.062500");
        assert_eq!(lines[10], "7 -2 27 0.437500 0.500000 0.000000 0.062500");
        assert_eq!(lines[258], "255 11 27 0.937500 1.000000 0.937500 1.000000");
        assert_eq!(lines.len(), 260, "last line ends with \\r\\n");
        assert_eq!(lines[259], "");
        assert!(!text.replace("\r\n", "").contains('\n'));
        // The reader of the reimplementation understands the file (also negative widths), the strict reader in
        // opennml-formats understands any file without negative widths.
        let table = crate::fonts::read_fnt_game(&text);
        assert_eq!(table.advance[7], -2);
        widths[7] = 2;
        let positive = format_fnt(512, &widths, &[27; 256]);
        assert_eq!(
            opennml_formats::fnt::parse_fnt(&positive)
                .unwrap()
                .glyphs
                .len(),
            256
        );
        assert_eq!(table.advance[255], 11);
        assert_eq!(table.max_height, 27);
        assert_eq!(table.uv[17], [0.0625, 0.125, 0.0625, 0.125]);
    }

    // QUIRK O-84: kept
    #[test]
    fn tga_alpha_is_the_rgb_mean() {
        // FUN_0057A960 (O-84): type 2, 32 bit, descriptor 0; BGR unchanged from the DIB, A = floor((R+G+B)/3), also
        // for colored ClearType fringes. The DIB lies bottom to top; rows are padded to 4 bytes
        // (here a row of 16 * 3 = 48 bytes plus 4 padding bytes).
        let size = 16u32;
        let stride = 52usize;
        let mut dib = vec![0u8; stride * size as usize];
        let samples: [([u8; 3], u8); 6] = [
            ([0, 0, 255], 85),  // red (B, G, R)
            ([10, 20, 31], 20), // 61 / 3 = 20.33
            ([255, 255, 255], 255),
            ([254, 255, 255], 254), // 764 / 3 = 254.67 -> rounded down
            ([1, 1, 0], 0),
            ([200, 0, 100], 100), // bluish fringe
        ];
        for (i, (bgr, _)) in samples.iter().enumerate() {
            dib[i * 3..i * 3 + 3].copy_from_slice(bgr);
        }
        // Padding bytes must not be read along.
        dib[48..52].copy_from_slice(&[9, 9, 9, 9]);
        // Top image row = last DIB row: first pixel green.
        let top = stride * (size as usize - 1);
        dib[top..top + 3].copy_from_slice(&[0, 255, 0]);

        let tga = encode_tga(size, stride, &dib);
        assert_eq!(
            tga[..18],
            [0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 16, 0, 32, 0]
        );
        assert_eq!(tga.len(), 18 + 16 * 16 * 4);
        for (i, (bgr, alpha)) in samples.iter().enumerate() {
            let at = 18 + i * 4;
            assert_eq!(tga[at..at + 3], *bgr, "Pixel {i}: BGR unchanged");
            assert_eq!(tga[at + 3], *alpha, "Pixel {i}: Alpha");
        }
        // The second file row begins with the start of the second DIB row (without padding bytes).
        assert_eq!(tga[18 + 16 * 4..18 + 16 * 4 + 4], [0, 0, 0, 0]);
        // Read like the game: file row 0 (bottom) becomes image row 15, the topmost DIB row image row 0.
        let image = decode_tga_game(&tga).unwrap();
        assert_eq!(image.get_pixel(0, 15).0, [255, 0, 0, 85]);
        assert_eq!(image.get_pixel(1, 15).0, [31, 20, 10, 20]);
        assert_eq!(image.get_pixel(0, 0).0, [0, 255, 0, 85]);
    }

    // QUIRK O-59: deviation
    #[test]
    fn cache_never_points_into_the_game_dir() {
        // Default: NML_FONT_CACHE, otherwise %LOCALAPPDATA%\OpenNML\fonts; empty values do not count.
        let lad = || Some(OsString::from(r"C:\Users\x\AppData\Local"));
        assert_eq!(
            cache_dir_from(None, lad()),
            Some(
                PathBuf::from(r"C:\Users\x\AppData\Local")
                    .join("OpenNML")
                    .join("fonts")
            )
        );
        assert_eq!(
            cache_dir_from(Some(OsString::from(r"D:\custom")), lad()),
            Some(PathBuf::from(r"D:\custom"))
        );
        assert_eq!(
            cache_dir_from(Some(OsString::new()), lad()),
            cache_dir_from(None, lad())
        );
        assert_eq!(cache_dir_from(None, Some(OsString::new())), None);
        assert_eq!(cache_dir_from(None, None), None);

        // A cache in the game directory (also via `..` or another spelling) counts as inside.
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("Game");
        std::fs::create_dir_all(game.join("data")).unwrap();
        assert!(is_inside(&game, &game));
        assert!(is_inside(&game.join("data").join("fonts"), &game));
        assert!(is_inside(&game.join("..").join("Game").join("data"), &game));
        assert!(!is_inside(&root.path().join("Game2"), &game));
        assert!(!is_inside(&root.path().join("cache"), &game));
        assert!(!is_inside(root.path(), &game));
        #[cfg(windows)]
        assert!(is_inside(
            Path::new(&game.to_string_lossy().to_uppercase())
                .join("fonts")
                .as_path(),
            &game
        ));

        // The real default never lies in the installation of the corpus.
        if let Some(game_dir) = opennml_rda::testutil::corpus_game_dir() {
            if let Some(dir) = default_cache_dir() {
                assert!(!is_inside(&dir, &game_dir), "{}", dir.display());
                assert_eq!(cache_dir_for(&game_dir), Some(dir));
            }
            // If the cache lay there, there would be none.
            assert!(is_inside(&game_dir.join("data").join("fonts"), &game_dir));
        }
    }

    #[test]
    fn cache_file_names_follow_the_atlas_name() {
        let def = font_def("MenuButton", "Garamond", 600);
        assert_eq!(
            cache_names(&def, 40),
            Some((
                "Garamond_40_600_0_0_0.fnt".to_string(),
                "Garamond_40_600_0_0_0.tga".to_string()
            ))
        );
        // A font name with path characters could lead out of the cache: rejected.
        for face in [r"..\..\x", "a/b", "C:x", "", "a\u{1}b"] {
            assert_eq!(cache_names(&font_def("X", face, 0), 10), None, "{face:?}");
        }
    }

    #[cfg(windows)]
    mod windows {
        use super::*;
        use crate::assets::UiAssets;
        use crate::fonts::{AtlasSource, FontBook};
        use crate::testutil::{fnt_text, test_vfs, tga_bottom_up};
        use opennml_formats::fnt::parse_fnt;
        use opennml_formats::localisation::parse_localisation;
        use opennml_formats::text::decode_cp1252;

        fn generator(dir: &Path) -> GdiAtlasGenerator {
            GdiAtlasGenerator {
                cache_dir: dir.join("fonts"),
            }
        }

        // QUIRK O-84: kept
        // QUIRK O-59: deviation
        #[test]
        fn generated_atlas_matches_the_shipped_layout() {
            if !gdi::installed("Garamond") {
                eprintln!("Garamond is not installed: skipped");
                return;
            }
            let dir = tempfile::tempdir().unwrap();
            let generator = generator(dir.path());
            let def = font_def("MenuButton", "Garamond", 600);
            let (text, image) = generator.generate(&def, 27).expect("atlas generated");
            // 27 px: tmHeight * 16 >= 257 -> 512^2, cell 32 (as in the shipped data).
            assert_eq!(image.dimensions(), (512, 512));
            assert_eq!((image.width() >> 4, image.height() >> 4), (32, 32));
            assert!(text.starts_with("FONTFILE V1.0\r\n\r\n[CHARINFOS]\r\n0 "));
            let font = parse_fnt(&text).unwrap();
            assert_eq!(font.glyphs.len(), 256);
            for (c, g) in font.glyphs.iter().enumerate() {
                assert_eq!(g.code as usize, c);
                let (col, row) = ((c % 16) as f32, (c / 16) as f32);
                assert_eq!(
                    [g.u0, g.u1, g.v0, g.v1],
                    [
                        col * 32.0 / 512.0,
                        (col + 1.0) * 32.0 / 512.0,
                        row * 32.0 / 512.0,
                        (row + 1.0) * 32.0 / 512.0
                    ]
                );
                assert!(g.height > 0, "height = return value of DrawTextA");
            }
            for c in b'A'..=b'Z' {
                assert!(
                    font.glyphs[usize::from(c)].width > 0,
                    "advance {}",
                    c as char
                );
            }
            // The glyph A is visible (white on black, alpha = brightness) and lies in its cell.
            let (ax, ay) = (u32::from(b'A' % 16) * 32, u32::from(b'A' / 16) * 32);
            let lit = (0..32)
                .flat_map(|y| (0..32).map(move |x| (x, y)))
                .filter(|&(x, y)| image.get_pixel(ax + x, ay + y).0[3] > 0)
                .count();
            assert!(lit > 20, "glyph A: {lit} pixels");
            for p in image.pixels() {
                let [r, g, b, a] = p.0;
                assert_eq!(
                    u32::from(a),
                    (u32::from(r) + u32::from(g) + u32::from(b)) / 3
                );
            }

            // Both files are in the cache and equal the result.
            let fnt_file = dir.path().join("fonts").join("Garamond_27_600_0_0_0.fnt");
            let tga_file = dir.path().join("fonts").join("Garamond_27_600_0_0_0.tga");
            assert_eq!(std::fs::read_to_string(&fnt_file).unwrap(), text);
            let tga = std::fs::read(&tga_file).unwrap();
            assert_eq!(
                tga[..18],
                [0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 2, 32, 0]
            );
            assert_eq!(decode_tga_game(&tga).unwrap(), image);

            // Comparison with the shipped atlas: header and UV equal, widths, heights and pixels only as
            // output (smoothing and font version of the machine).
            let Some(game_dir) = opennml_rda::testutil::corpus_game_dir() else {
                return;
            };
            let vfs = opennml_rda::Vfs::open_game_dir(&game_dir).unwrap();
            let shipped = decode_cp1252(&vfs.read("data/fonts/garamond_27_600_0_0_0.fnt").unwrap());
            let ours: Vec<&str> = text.split("\r\n").collect();
            let theirs: Vec<&str> = shipped.split("\r\n").collect();
            assert_eq!(ours.len(), theirs.len());
            assert_eq!(ours[..3], theirs[..3]);
            let mut metric_diffs = 0;
            for (a, b) in ours.iter().zip(&theirs).skip(3) {
                let fa: Vec<&str> = a.split(' ').collect();
                let fb: Vec<&str> = b.split(' ').collect();
                assert_eq!(fa.len(), fb.len());
                if fa.is_empty() || fa == [""] {
                    continue;
                }
                assert_eq!(fa[0], fb[0], "Code");
                assert_eq!(fa[3..], fb[3..], "UV as in the shipped data: {a} / {b}");
                if fa[1..3] != fb[1..3] {
                    metric_diffs += 1;
                }
            }
            let shipped_image =
                decode_tga_game(&vfs.read("data/fonts/garamond_27_600_0_0_0.tga").unwrap())
                    .unwrap();
            let pixel_diffs = image
                .pixels()
                .zip(shipped_image.pixels())
                .filter(|(a, b)| a.0[3] != b.0[3])
                .count();
            eprintln!(
                "Garamond 27/600 against the shipped data: {metric_diffs} of 256 rows with different width/height, \
                 {pixel_diffs} of {} pixels with different alpha",
                512 * 512
            );
        }

        // QUIRK O-59: deviation
        #[test]
        fn cache_is_reused() {
            let dir = tempfile::tempdir().unwrap();
            let generator = generator(dir.path());
            let def = font_def("Small", "Arial", 0);
            let (first, image) = generator.generate(&def, 12).unwrap();
            let cache = dir.path().join("fonts");
            let fnt_file = cache.join("Arial_12_0_0_0_0.fnt");
            let tga_file = cache.join("Arial_12_0_0_0_0.tga");
            assert!(fnt_file.is_file() && tga_file.is_file());
            assert_eq!(
                image.dimensions(),
                (256, 256),
                "Arial 12: tmHeight 15 -> 256^2"
            );

            // The second call reads the files and generates nothing: planted files come back.
            let fake_fnt = fnt_text(|_| 3, |_| 4);
            std::fs::write(&fnt_file, &fake_fnt).unwrap();
            std::fs::write(&tga_file, tga_bottom_up(32, 32, |x, _| [x as u8, 0, 0, 7])).unwrap();
            let (second, fake_image) = generator.generate(&def, 12).unwrap();
            assert_eq!(second, fake_fnt);
            assert_eq!(fake_image.dimensions(), (32, 32));
            assert_eq!(fake_image.get_pixel(5, 0).0, [5, 0, 0, 7]);

            // Corrupted cache: generate anew and overwrite.
            std::fs::write(&tga_file, b"broken").unwrap();
            let (third, image3) = generator.generate(&def, 12).unwrap();
            assert_eq!(third, first);
            assert_eq!(image3.dimensions(), (256, 256));
            assert_eq!(std::fs::read_to_string(&fnt_file).unwrap(), first);
            // No leftovers from writing.
            let mut names: Vec<_> = std::fs::read_dir(&cache)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            assert_eq!(names, ["Arial_12_0_0_0_0.fnt", "Arial_12_0_0_0_0.tga"]);
        }

        // QUIRK O-59: deviation
        #[test]
        fn non_positive_sizes_are_rejected() {
            // FontBook also calls the generator with 0 px (width 0) or negative sizes.
            let dir = tempfile::tempdir().unwrap();
            let generator = generator(dir.path());
            let def = font_def("Zero", "Arial", 0);
            assert!(generator.generate(&def, 0).is_none());
            assert!(generator.generate(&def, -3).is_none());
            assert!(!dir.path().join("fonts").exists(), "nothing written");
            // Invalid font name: likewise nothing.
            assert!(generator.generate(&font_def("X", r"..\x", 0), 10).is_none());
            assert!(!dir.path().join("fonts").exists());
        }

        // QUIRK O-22, O-59: deviation
        #[test]
        fn replacement_font_is_generated_ms_sans_serif() {
            // Unknown names get "MS Sans Serif" 10 px from the generator (a deviation from the original).
            let dir = tempfile::tempdir().unwrap();
            let generator = generator(dir.path());
            let vfs = test_vfs(&[("data/x.txt", vec![])]);
            let loc =
                parse_localisation("Font TextFont\nFontSize 16\nTextureFont\nFontName Garamond\n");
            let mut assets = UiAssets::new();
            let book = FontBook::load(&vfs, &loc, 1024, &mut assets, Some(&generator));
            let font = book.font(book.by_name("Unknown"));
            assert_eq!(font.name, "MS Sans Serif");
            assert_eq!((font.px, font.requested_px), (10, 10));
            assert_eq!(font.source, AtlasSource::Generated);
            assert!(font.max_height > 0);
            assert_eq!(
                assets.path(font.atlas),
                "data/fonts/ms sans serif_10_0_0_0_0.tga"
            );
            assert_ne!(book.by_name("Unknown"), book.by_name("TextFont"));
            assert!(
                dir.path()
                    .join("fonts")
                    .join("MS Sans Serif_10_0_0_0_0.fnt")
                    .is_file()
            );
            // TextFont itself is missing in the installation and is generated as well.
            assert_eq!(
                book.font(book.by_name("TextFont")).source,
                AtlasSource::Generated
            );
        }
    }
}
