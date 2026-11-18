//! Fonts: pixel sizes, atlas names, loading the `.fnt` like the game, fallback and icon sets.
//!
//! Basis: re/ui-rendering 3.1 (definitions), 3.2/3.3 (sizes, atlas names), 3.4 (drawing) and 3.8.7 (glyph values,
//! atlas); `docs/formats/fnt.md`.

use std::collections::BTreeMap;

use image::RgbaImage;
use opennml_formats::localisation::{FontDef, FontSize, IconSetDef, Localisation};
use opennml_formats::text::decode_cp1252;
use opennml_rda::Vfs;

use crate::assets::UiAssets;
use crate::draw::TextureId;

/// Name of the font that serves as the last fallback.
const TEXT_FONT: &str = "TextFont";
/// Image of the replacement icon set (`FUN_00560F50`, re/ui-rendering 3.8.2: unknown set).
const FALLBACK_ICONS: &str = "data/icons/INFO_Icons16x16.tga";
/// Largest pixel height that the fallback tries when searching for existing atlases.
const MAX_PROBE_PX: i32 = 512;
/// Path of the 1 x 1 replacement image for fonts and icon sets without an image.
const BLANK_IMAGE: &str = "<leer>";

/// Pixel height of a relative font: `trunc(W * n / 1280)` with W = width of the back buffer at creation
/// (`FUN_00428860`, reference `0x500` from `FUN_00428540`; re/ui-rendering 3.3).
pub fn relative_px(screen_width: i32, n: i32) -> i32 {
    let px = i64::from(screen_width) * i64::from(n) / 1280;
    px.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// File name of the atlas: `data/fonts/` + `%s_%d_%d_%d_%d_%d.fnt` with font name (GDI face), height, weight
/// (0 without `Weight`; `FontDef::weight` is then already 0, `FUN_00428860` reads it only with bit `0x10`), italic
/// 0/2, underline 0/4, strike-out 0/8 (`0x006185D0`, `0x00428955`; re/ui-rendering 3.2). The case is that of the
/// definition, the VFS ignores it.
pub fn atlas_path(def: &FontDef, px: i32) -> String {
    format!(
        "data/fonts/{}_{}_{}_{}_{}_{}.fnt",
        def.face,
        px,
        def.weight,
        if def.italic { 2 } else { 0 },
        if def.underline { 4 } else { 0 },
        if def.strike_out { 8 } else { 0 },
    )
}

/// Path of the texture for a `.fnt` (extension swapped, `FUN_0057B1D0`).
pub(crate) fn texture_path(fnt_path: &str) -> String {
    match fnt_path.len().checked_sub(4) {
        Some(stem)
            if fnt_path.is_char_boundary(stem) && fnt_path[stem..].eq_ignore_ascii_case(".fnt") =>
        {
            format!("{}.tga", &fnt_path[..stem])
        }
        _ => format!("{fnt_path}.tga"),
    }
}

/// Where the atlas of a font comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtlasSource {
    /// Atlas of the requested size from the installation.
    Installed,
    /// Produced by the [`AtlasGenerator`] (cache, O-59).
    Generated,
    /// Replacement: a larger or smaller atlas of the same font, `TextFont` or a blank font.
    Fallback,
}

/// A loaded font (`TextureFont` object of the game, re/ui-rendering 3.8.7).
#[derive(Debug, Clone)]
pub struct UiFont {
    /// Name of the definition (`Font <Name>`), also the requested one in case of a fallback.
    pub name: String,
    /// Pixel height of the atlas actually used.
    pub px: i32,
    /// Requested pixel height (from size and width at load time); with [`AtlasSource::Fallback`] different from `px`.
    pub requested_px: i32,
    pub source: AtlasSource,
    pub atlas: TextureId,
    /// Cell size when drawing: texture width/height `>> 4` (not from the `.fnt`).
    pub cell: [i32; 2],
    /// Advance per code (`width` of the `.fnt`); missing line -> 0.
    pub advance: [i32; 256],
    /// UV per code as `u0, u1, v0, v1` from the `.fnt`; missing line -> 0.
    pub uv: [[f32; 4]; 256],
    /// Largest `height` of the `.fnt`: line advance and run height.
    pub max_height: i32,
}

/// Reference to a font in the [`FontBook`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontRef(pub usize);

/// An icon set (`[ICON <set> <index>]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconSet {
    pub texture: TextureId,
    /// Size of one icon in pixels.
    pub size: [i32; 2],
}

/// Font name (GDI face) of the replacement font for unknown names. The original creates "MS Sanserif" 10 px as an
/// `ID3DXFont` (`FUN_00560E70`, O-22); the reimplementation takes a generated atlas "MS Sans Serif" 10 px
/// (a deviation from the original).
pub const REPLACEMENT_FACE: &str = "MS Sans Serif";
/// Pixel height of the replacement font.
pub const REPLACEMENT_PX: i32 = 10;

/// Produces missing atlases (GDI: `atlas_gen::GdiAtlasGenerator`). Returns the text of the `.fnt` and the image of
/// the `.tga`; `None` if the font cannot be produced (also for `px <= 0`).
pub trait AtlasGenerator {
    fn generate(&self, def: &FontDef, px: i32) -> Option<(String, RgbaImage)>;
}

/// Glyph values from a `.fnt`.
#[derive(Debug, Clone)]
pub(crate) struct FntTable {
    pub advance: [i32; 256],
    pub uv: [[f32; 4]; 256],
    pub max_height: i32,
}

/// Read pointer for `sscanf`-like reading (MSVC: skip whitespace, `%i` with base detection, `%f`).
struct Cursor<'a> {
    rest: &'a [u8],
}

impl Cursor<'_> {
    fn skip_space(&mut self) {
        while let [b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C, tail @ ..] = self.rest {
            self.rest = tail;
        }
    }

    fn sign(&mut self) -> bool {
        match self.rest {
            [b'-', tail @ ..] => {
                self.rest = tail;
                true
            }
            [b'+', tail @ ..] => {
                self.rest = tail;
                false
            }
            _ => false,
        }
    }

    fn digits(&mut self, radix: u32) -> Option<i64> {
        let mut value = None;
        while let [c, tail @ ..] = self.rest {
            let Some(d) = char::from(*c).to_digit(radix) else {
                break;
            };
            value = Some(
                value
                    .unwrap_or(0i64)
                    .wrapping_mul(i64::from(radix))
                    .wrapping_add(i64::from(d)),
            );
            self.rest = tail;
        }
        value
    }

    /// `%i`: sign, `0x...` hexadecimal, leading `0` octal, otherwise decimal.
    fn int(&mut self) -> Option<i32> {
        self.skip_space();
        let negative = self.sign();
        let value = match self.rest {
            [b'0', b'x' | b'X', c, ..] if c.is_ascii_hexdigit() => {
                self.rest = &self.rest[2..];
                self.digits(16)?
            }
            [b'0', ..] => self.digits(8)?,
            _ => self.digits(10)?,
        };
        let value = if negative {
            value.wrapping_neg()
        } else {
            value
        };
        Some(value as i32)
    }

    /// `%f`: sign, digits with a point, exponent.
    fn float(&mut self) -> Option<f32> {
        self.skip_space();
        let start = self.rest;
        self.sign();
        let mut mantissa = false;
        while let [b'0'..=b'9', tail @ ..] = self.rest {
            self.rest = tail;
            mantissa = true;
        }
        if let [b'.', tail @ ..] = self.rest {
            self.rest = tail;
            while let [b'0'..=b'9', tail @ ..] = self.rest {
                self.rest = tail;
                mantissa = true;
            }
        }
        if !mantissa {
            self.rest = start;
            return None;
        }
        if let [b'e' | b'E', tail @ ..] = self.rest {
            let after_sign = match tail {
                [b'+' | b'-', t @ ..] => t,
                t => t,
            };
            if matches!(after_sign, [b'0'..=b'9', ..]) {
                self.rest = after_sign;
                while let [b'0'..=b'9', tail @ ..] = self.rest {
                    self.rest = tail;
                }
            }
        }
        let len = start.len() - self.rest.len();
        std::str::from_utf8(&start[..len]).ok()?.parse().ok()
    }
}

/// Reads a `.fnt` like `FUN_0057AB60` (re/ui-rendering 3.8.7): lines up to one with `[CHARINFOS]` (the
/// header line `FONTFILE V1.0` is not checked), then per line `%i` (index) and `%i %i %i %f %f %f %f`
/// (index, `width`, `height`, `u0`, `u1`, `v0`, `v1`). The index is not checked against the range; here
/// a line with an index outside 0...255 is skipped (the original would write behind the table), as is
/// a line without a number at the start. If a later field fails, those read up to then apply (that is how
/// `sscanf` writes; the behavior of the original with partial lines has not been checked, they do not occur in the
/// shipped data). `max_height` is the largest `height` read (start 0).
pub(crate) fn read_fnt_game(text: &str) -> FntTable {
    let mut table = FntTable {
        advance: [0; 256],
        uv: [[0.0; 4]; 256],
        max_height: 0,
    };
    let mut in_chars = false;
    for line in text.split('\n') {
        if !in_chars {
            in_chars = line.contains("[CHARINFOS]");
            continue;
        }
        let mut cur = Cursor {
            rest: line.as_bytes(),
        };
        let Some(index) = cur.int() else { continue };
        let Ok(index) = usize::try_from(index) else {
            continue;
        };
        if index >= 256 {
            continue;
        }
        if let Some(width) = cur.int() {
            table.advance[index] = width;
            if let Some(height) = cur.int() {
                table.max_height = table.max_height.max(height);
                for slot in 0..4 {
                    let Some(v) = cur.float() else { break };
                    table.uv[index][slot] = v;
                }
            }
        }
    }
    table
}

/// All fonts and icon sets of a language, created once at startup with the width of the back buffer
/// (O-58: later resolution changes do not change the pixel sizes).
#[derive(Debug)]
pub struct FontBook {
    fonts: Vec<UiFont>,
    by_name: BTreeMap<String, usize>,
    replacement: usize,
    icons: BTreeMap<String, IconSet>,
    fallback_icons: IconSet,
    /// Image size (width, height) of the icon set images: the cell grid (`FUN_00401DF0`) needs it
    /// when drawing an `[ICON]` run (`text::draw_runs`).
    icon_textures: BTreeMap<TextureId, [u32; 2]>,
}

/// Per name keeps the position of the first definition and the content of the last: in the original a
/// later definition of the same name replaces the earlier one (`CMap`).
fn last_wins<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<&T> {
    let mut out: Vec<&T> = Vec::new();
    for item in items {
        match out.iter_mut().find(|o| name(o) == name(item)) {
            Some(slot) => *slot = item,
            None => out.push(item),
        }
    }
    out
}

/// The 1 x 1 replacement image (transparent) for fonts and icon sets whose images are missing.
fn blank_texture(assets: &mut UiAssets) -> TextureId {
    assets.insert_image(BLANK_IMAGE, RgbaImage::new(1, 1))
}

fn make_font(
    name: &str,
    px: i32,
    requested_px: i32,
    source: AtlasSource,
    atlas: TextureId,
    table: FntTable,
    assets: &UiAssets,
) -> UiFont {
    let (width, height) = assets.image(atlas).dimensions();
    UiFont {
        name: name.to_string(),
        px,
        requested_px,
        source,
        atlas,
        cell: [(width >> 4) as i32, (height >> 4) as i32],
        advance: table.advance,
        uv: table.uv,
        max_height: table.max_height,
    }
}

/// Loads `.fnt` and the `.tga` of the same name from the installation; `None` if either is missing or the image is
/// broken. The silent check with `exists` keeps search steps of the fallback out of `UiAssets::missing`.
fn load_installed(
    vfs: &Vfs,
    assets: &mut UiAssets,
    fnt_path: &str,
) -> Option<(TextureId, FntTable)> {
    let tga = texture_path(fnt_path);
    if !vfs.exists(fnt_path) || !vfs.exists(&tga) {
        return None;
    }
    let bytes = vfs.read(fnt_path).ok()?;
    let atlas = assets.texture(vfs, &tga)?;
    Some((atlas, read_fnt_game(&decode_cp1252(&bytes))))
}

/// Atlas from the generator (cache or newly produced, `FUN_0057B570`; O-59). The image is stored under the
/// path the atlas would have in the installation; if one is already there (the same font a second time),
/// its ID applies and the image is not replaced (the drawer keeps textures per ID).
fn generated_font(
    def: &FontDef,
    px: i32,
    assets: &mut UiAssets,
    generator: Option<&dyn AtlasGenerator>,
) -> Option<UiFont> {
    let (text, image) = generator?.generate(def, px)?;
    let path = texture_path(&atlas_path(def, px));
    let atlas = match assets.find(&path) {
        Some(id) => id,
        None => assets.insert_image(&path, image),
    };
    Some(make_font(
        &def.name,
        px,
        px,
        AtlasSource::Generated,
        atlas,
        read_fnt_game(&text),
        assets,
    ))
}

/// Definition of the replacement font ([`REPLACEMENT_FACE`], [`REPLACEMENT_PX`], weight 0, without flags).
fn replacement_def() -> FontDef {
    FontDef {
        name: REPLACEMENT_FACE.to_string(),
        size: FontSize::Fixed(REPLACEMENT_PX),
        weight: 0,
        italic: false,
        underline: false,
        strike_out: false,
        texture_font: true,
        face: REPLACEMENT_FACE.to_string(),
    }
}

/// Font choice for a definition: atlas of the installation -> generator -> next larger existing atlas of the
/// same font, same weight and same flags -> largest smaller one. `None` if none of these succeeds (then
/// `TextFont`).
fn resolve_font(
    def: &FontDef,
    requested_px: i32,
    vfs: &Vfs,
    assets: &mut UiAssets,
    generator: Option<&dyn AtlasGenerator>,
) -> Option<UiFont> {
    let fnt = atlas_path(def, requested_px);
    if let Some((atlas, table)) = load_installed(vfs, assets, &fnt) {
        return Some(make_font(
            &def.name,
            requested_px,
            requested_px,
            AtlasSource::Installed,
            atlas,
            table,
            assets,
        ));
    }
    if let Some(font) = generated_font(def, requested_px, assets, generator) {
        return Some(font);
    }
    let bigger = requested_px.max(0).saturating_add(1)..=MAX_PROBE_PX;
    let smaller = (1..requested_px.min(MAX_PROBE_PX + 1)).rev();
    for px in bigger.chain(smaller) {
        let candidate = atlas_path(def, px);
        if let Some((atlas, table)) = load_installed(vfs, assets, &candidate) {
            log::warn!(
                "Font \"{}\": atlas {fnt} missing, replacement {candidate}",
                def.name
            );
            return Some(make_font(
                &def.name,
                px,
                requested_px,
                AtlasSource::Fallback,
                atlas,
                table,
                assets,
            ));
        }
    }
    None
}

impl FontBook {
    /// Creates all fonts and icon sets of `loc` (width `screen_width` for relative sizes).
    ///
    /// Images end up in `assets`. The reimplementation does not evaluate a `texture_font` flag of the definition: all
    /// fonts use atlases (the original draws only the replacement font without an atlas through
    /// `ID3DXFont`, a deviation from the original; in the shipped data all fonts have `TextureFont`).
    pub fn load(
        vfs: &Vfs,
        loc: &Localisation,
        screen_width: i32,
        assets: &mut UiAssets,
        generator: Option<&dyn AtlasGenerator>,
    ) -> FontBook {
        let defs = last_wins(&loc.fonts, |d| &d.name);
        let mut resolved: Vec<(i32, Option<UiFont>)> = Vec::new();
        for def in &defs {
            let requested = match def.size {
                FontSize::Fixed(px) => px,
                FontSize::Relative(n) => relative_px(screen_width, n),
            };
            resolved.push((
                requested,
                resolve_font(def, requested, vfs, assets, generator),
            ));
        }

        // Last fallback: the font TextFont (if it has an atlas itself), otherwise a blank font.
        let text_font = defs
            .iter()
            .position(|d| d.name == TEXT_FONT)
            .and_then(|i| resolved[i].1.clone());
        let blank = |assets: &mut UiAssets, name: &str, requested_px: i32| UiFont {
            name: name.to_string(),
            px: 0,
            requested_px,
            source: AtlasSource::Fallback,
            atlas: blank_texture(assets),
            cell: [0, 0],
            advance: [0; 256],
            uv: [[0.0; 4]; 256],
            max_height: 0,
        };
        let mut fonts = Vec::new();
        let mut by_name = BTreeMap::new();
        for (def, (requested, font)) in defs.iter().zip(resolved) {
            let font = font.unwrap_or_else(|| match &text_font {
                Some(base) => {
                    log::warn!(
                        "Font \"{}\" without atlas, replacement: {TEXT_FONT}",
                        def.name
                    );
                    UiFont {
                        name: def.name.clone(),
                        requested_px: requested,
                        source: AtlasSource::Fallback,
                        ..base.clone()
                    }
                }
                None => {
                    log::warn!(
                        "Font \"{}\" without atlas and without {TEXT_FONT}: blank",
                        def.name
                    );
                    blank(assets, &def.name, requested)
                }
            });
            by_name.insert(def.name.clone(), fonts.len());
            fonts.push(font);
        }
        // Replacement font for unknown names (a deviation from the original): generated atlas, otherwise TextFont,
        // otherwise blank.
        let generated = generated_font(&replacement_def(), REPLACEMENT_PX, assets, generator);
        let replacement = match (generated, by_name.get(TEXT_FONT)) {
            (Some(font), _) => {
                fonts.push(font);
                fonts.len() - 1
            }
            (None, Some(&i)) => i,
            (None, None) => {
                fonts.push(blank(assets, TEXT_FONT, 0));
                fonts.len() - 1
            }
        };

        let icons: BTreeMap<String, IconSet> = last_wins(&loc.icons, |d| &d.name)
            .into_iter()
            .map(|d| (d.name.clone(), load_icon_set(vfs, assets, d)))
            .collect();
        let fallback_icons = IconSet {
            texture: assets
                .texture(vfs, FALLBACK_ICONS)
                .unwrap_or_else(|| blank_texture(assets)),
            size: [16, 16],
        };
        let icon_textures = icons
            .values()
            .chain(std::iter::once(&fallback_icons))
            .map(|set: &IconSet| {
                let (w, h) = assets.image(set.texture).dimensions();
                (set.texture, [w, h])
            })
            .collect();
        FontBook {
            fonts,
            by_name,
            replacement,
            icons,
            fallback_icons,
            icon_textures,
        }
    }

    /// Font by name, **with** case sensitivity (`FUN_00560E70`, `CMap` lookup). An unknown name yields the
    /// replacement font: the original creates "MS Sanserif" 10 px for it; the reimplementation takes the generated
    /// atlas "MS Sans Serif" 10 px (a deviation from the original), without a generator `TextFont`.
    pub fn by_name(&self, name: &str) -> FontRef {
        FontRef(self.by_name.get(name).copied().unwrap_or(self.replacement))
    }

    /// The font for a reference (from this `FontBook`).
    pub fn font(&self, r: FontRef) -> &UiFont {
        &self.fonts[r.0]
    }

    /// Icon set by name (case matters). Unknown set: `data/icons/INFO_Icons16x16.tga`,
    /// 16 x 16 (`FUN_00560F50`, re/ui-rendering 3.8.2).
    pub fn icon_set(&self, name: &str) -> &IconSet {
        self.icons.get(name).unwrap_or(&self.fallback_icons)
    }

    /// Size of the image of an icon set (from [`FontBook::icon_set`]); `None` for a foreign image.
    pub fn icon_texture_size(&self, texture: TextureId) -> Option<[u32; 2]> {
        self.icon_textures.get(&texture).copied()
    }
}

fn load_icon_set(vfs: &Vfs, assets: &mut UiAssets, def: &IconSetDef) -> IconSet {
    IconSet {
        texture: assets
            .texture(vfs, &def.path)
            .unwrap_or_else(|| blank_texture(assets)),
        size: def.size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{fnt_text, test_vfs, tga_bottom_up};
    use opennml_formats::localisation::{FontSize, parse_localisation};

    fn def(text: &str) -> FontDef {
        parse_localisation(text).fonts.remove(0)
    }

    // QUIRK O-58: kept
    #[test]
    fn relative_font_sizes_match_the_shipped_atlases() {
        // re/ui-rendering 3.3: atlases for the widths 800/1024/1280/1400/1600.
        let widths = [800, 1024, 1280, 1400, 1600];
        let expect: [(i32, [i32; 5]); 3] = [
            (27, [16, 21, 27, 29, 33]), // MenuButton
            (24, [15, 19, 24, 26, 30]), // LoadingFont
            (50, [31, 40, 50, 54, 62]), // CountDown
        ];
        for (n, sizes) in expect {
            for (w, px) in widths.into_iter().zip(sizes) {
                assert_eq!(relative_px(w, n), px, "n = {n}, B = {w}");
            }
        }
        // Rounding toward 0 (no rounding up): 800*27/1280 = 16.875.
        assert_eq!(relative_px(800, 27), 16);
        assert_eq!(relative_px(0, 27), 0);
    }

    // QUIRK O-58: kept
    #[test]
    fn atlas_path_names_the_installed_files() {
        let menu_button = def(
            "Font MenuButton\nRelativeFontSize 27\nWeight 600\nTextureFont\nFontName Garamond\n",
        );
        assert_eq!(menu_button.size, FontSize::Relative(27));
        assert_eq!(
            atlas_path(&menu_button, 27),
            "data/fonts/Garamond_27_600_0_0_0.fnt"
        );
        let all =
            def("Font X\nFontSize 15\nWeight 700\nItalic\nUnderline\nStrikeOut\nFontName Arial\n");
        assert_eq!(atlas_path(&all, 15), "data/fonts/Arial_15_700_2_4_8.fnt");
        // The weight applies only with `Weight` in the same `Font` block (bit 0x10): `Font` clears the bits.
        let loc = parse_localisation(
            "Font A\nWeight 600\nFontName Garamond\nFont B\nFontSize 16\nFontName Garamond\n",
        );
        assert_eq!(
            atlas_path(&loc.fonts[1], 16),
            "data/fonts/Garamond_16_0_0_0_0.fnt"
        );
    }

    #[test]
    fn fnt_reading_ignores_the_header() {
        // FUN_0057AB60: lines up to `[CHARINFOS]`, the header line is not checked; a file without
        // `FONTFILE V1.0` loads (opennml_formats::fnt::parse_fnt would reject it).
        let text = "anything\n[CHARINFOS]\n65 9 15 0.25 0.3125 0.5 0.5625\n66 7 13 0.5 0.5625 0.5 0.5625\n";
        assert!(opennml_formats::fnt::parse_fnt(text).is_err());
        let table = read_fnt_game(text);
        assert_eq!(table.advance[65], 9);
        assert_eq!(table.uv[65], [0.25, 0.3125, 0.5, 0.5625]);
        assert_eq!(table.advance[66], 7);
        assert_eq!(table.max_height, 15);
        assert_eq!(table.advance[67], 0, "missing line: 0");
        assert_eq!(table.uv[67], [0.0; 4]);
        // Even entirely without `[CHARINFOS]` the table stays empty.
        assert_eq!(read_fnt_game("65 9 15 0 0 0 0\n").max_height, 0);
    }

    #[test]
    fn fnt_reading_follows_sscanf() {
        // `%i`: also hexadecimal (0x41) and octal (0101); an index outside 0...255 is skipped
        // (the original would write behind the table), also for `max_height`.
        let text = "[CHARINFOS]\r\n0x41 5 11 0 0.5 0 0.5\r\n0102 6 12 1 1 1 1\r\n256 9 99 1 1 1 1\r\n-1 9 98 1 1 1 1\r\n";
        let table = read_fnt_game(text);
        assert_eq!(table.advance[0x41], 5);
        assert_eq!(table.advance[0x42], 6);
        assert_eq!(table.max_height, 12);
        // Line without a number at the start: skipped.
        let table = read_fnt_game("[CHARINFOS]\r\nabc\r\n\r\n7 3 4 0.5 0.5 0.5 0.5\r\n");
        assert_eq!(table.advance[7], 3);
        // With `\n` without `\r` and with whitespace at the edges.
        let table = read_fnt_game("[CHARINFOS]\n  8   2   3   .5 1e0 0.25 -0.5  \n");
        assert_eq!(table.advance[8], 2);
        assert_eq!(table.uv[8], [0.5, 1.0, 0.25, -0.5]);
    }

    /// `.fnt` and `.tga` of an atlas in the test VFS: texture size 64 x 32 (cell 4 x 2).
    fn atlas_files(path_fnt: &str, width_of: i32, height_of: i32) -> Vec<(String, Vec<u8>)> {
        let tga = tga_bottom_up(64, 32, |x, y| [x as u8, y as u8, 0, 255]);
        vec![
            (
                path_fnt.to_string(),
                fnt_text(|_| width_of, |_| height_of).into_bytes(),
            ),
            (path_fnt.replace(".fnt", ".tga"), tga),
        ]
    }

    fn vfs_of(files: Vec<(String, Vec<u8>)>) -> crate::testutil::TestVfs {
        let refs: Vec<(&str, Vec<u8>)> =
            files.iter().map(|(p, d)| (p.as_str(), d.clone())).collect();
        test_vfs(&refs)
    }

    const LOC: &str = "Font TextFont\nFontSize 16\nTextureFont\nFontName Garamond\n\
Font MenuButton\nRelativeFontSize 27\nWeight 600\nTextureFont\nFontName Garamond\n\
Icons InfoIcons\nIconsName data\\icons\\own.tga\nIconsSize 32 24\n";

    #[test]
    fn installed_atlases_load_with_the_game_values() {
        let mut files = atlas_files("data/fonts/Garamond_16_0_0_0_0.fnt", 7, 15);
        files.extend(atlas_files("data/fonts/garamond_21_600_0_0_0.fnt", 9, 20));
        let vfs = vfs_of(files);
        let loc = parse_localisation(LOC);
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        let font = book.font(book.by_name("MenuButton"));
        assert_eq!(font.name, "MenuButton");
        assert_eq!((font.px, font.requested_px), (21, 21));
        assert_eq!(font.source, AtlasSource::Installed);
        assert_eq!(font.cell, [4, 2], "texture size >> 4");
        assert_eq!(font.advance[65], 9);
        assert_eq!(font.advance[255], 9);
        assert_eq!(font.max_height, 20);
        assert_eq!(font.uv[1], [1.0 / 16.0, 2.0 / 16.0, 0.0, 1.0 / 16.0]);
        assert_eq!(
            assets.path(font.atlas),
            "data/fonts/garamond_21_600_0_0_0.tga"
        );
        let text_font = book.font(book.by_name("TextFont"));
        assert_eq!(text_font.source, AtlasSource::Installed);
        assert_eq!(text_font.px, 16);
        // Only the icon images that this test VFS does not contain are missing, never an atlas.
        assert_eq!(
            assets.missing(),
            ["data/icons/own.tga", "data/icons/info_icons16x16.tga"]
        );
    }

    #[test]
    fn font_names_compare_with_case_and_the_last_definition_wins() {
        let files = atlas_files("data/fonts/Garamond_16_0_0_0_0.fnt", 7, 15);
        let vfs = vfs_of(files);
        let mut loc = parse_localisation(LOC);
        // Two definitions of the same name by hand (the reader in opennml-formats already merges them).
        let mut later = loc.fonts[0].clone();
        later.size = FontSize::Fixed(16);
        loc.fonts.push(later);
        loc.fonts[0].size = FontSize::Fixed(99);
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1280, &mut assets, None);
        let text_font = book.font(book.by_name("TextFont"));
        assert_eq!(text_font.requested_px, 16, "the last definition applies");
        // Case matters: `textfont` is unknown and gets the replacement font (TextFont).
        assert_eq!(book.by_name("textfont"), book.by_name("TextFont"));
        assert_eq!(book.by_name("Unknown"), book.by_name("TextFont"));
        assert_ne!(book.by_name("MenuButton"), book.by_name("TextFont"));
    }

    // QUIRK O-59: deviation
    #[test]
    fn font_fallback_takes_the_next_bigger_atlas() {
        // Present: 16 and 33 (weight 600); looking for 21.
        let mut files = atlas_files("data/fonts/garamond_16_600_0_0_0.fnt", 7, 15);
        files.extend(atlas_files("data/fonts/garamond_33_600_0_0_0.fnt", 14, 30));
        // A larger atlas with a different weight and a TextFont do not count.
        files.extend(atlas_files("data/fonts/garamond_25_0_0_0_0.fnt", 1, 1));
        files.extend(atlas_files("data/fonts/Garamond_16_0_0_0_0.fnt", 7, 15));
        let vfs = vfs_of(files);
        let loc = parse_localisation(LOC);

        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        let font = book.font(book.by_name("MenuButton"));
        assert_eq!((font.px, font.requested_px), (33, 21));
        assert_eq!(font.source, AtlasSource::Fallback);
        assert_eq!(font.max_height, 30);
        assert_eq!(font.name, "MenuButton");

        // Looking for 40 (width 1900): no larger one, so the largest smaller one (33).
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1900, &mut assets, None);
        let font = book.font(book.by_name("MenuButton"));
        assert_eq!((font.px, font.requested_px), (33, 40));
        assert_eq!(font.source, AtlasSource::Fallback);

        // With a generator: generated atlas, same source for the image storage.
        struct TestGenerator;
        impl AtlasGenerator for TestGenerator {
            fn generate(&self, def: &FontDef, px: i32) -> Option<(String, RgbaImage)> {
                (def.name == "MenuButton" && px == 21)
                    .then(|| (fnt_text(|_| 11, |_| 22), RgbaImage::new(256, 128)))
            }
        }
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1024, &mut assets, Some(&TestGenerator));
        let font = book.font(book.by_name("MenuButton"));
        assert_eq!(font.source, AtlasSource::Generated);
        assert_eq!((font.px, font.requested_px), (21, 21));
        assert_eq!(font.advance[65], 11);
        assert_eq!(font.max_height, 22);
        assert_eq!(font.cell, [16, 8]);
        assert_eq!(assets.image(font.atlas).dimensions(), (256, 128));
        // The generator declines (None): fallback as without a generator.
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1900, &mut assets, Some(&TestGenerator));
        assert_eq!(
            book.font(book.by_name("MenuButton")).source,
            AtlasSource::Fallback
        );
    }

    // QUIRK O-22, O-59: deviation
    #[test]
    fn unknown_names_get_the_generated_replacement() {
        // A deviation from the original: "MS Sans Serif" 10 px through the generator; without a generator or if it
        // declines, it stays with TextFont.
        /// Answers only for the replacement font: a 256 image the first time, a 512 image afterwards.
        struct Replacement(std::cell::Cell<u32>);
        impl AtlasGenerator for Replacement {
            fn generate(&self, def: &FontDef, px: i32) -> Option<(String, RgbaImage)> {
                if def.face != "MS Sans Serif" || px != 10 || def.weight != 0 {
                    return None;
                }
                let size = 256 << self.0.get().min(1);
                self.0.set(self.0.get() + 1);
                Some((fnt_text(|_| 5, |_| 13), RgbaImage::new(size, size)))
            }
        }
        let generator = Replacement(std::cell::Cell::new(0));
        let vfs = vfs_of(atlas_files("data/fonts/Garamond_16_0_0_0_0.fnt", 7, 15));
        let loc = parse_localisation(LOC);
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1024, &mut assets, Some(&generator));
        let font = book.font(book.by_name("Unknown"));
        assert_eq!(font.name, "MS Sans Serif");
        assert_eq!(font.source, AtlasSource::Generated);
        assert_eq!((font.px, font.advance[65], font.max_height), (10, 5, 13));
        assert_eq!(font.cell, [16, 16]);
        assert_eq!(
            assets.path(font.atlas),
            "data/fonts/ms sans serif_10_0_0_0_0.tga"
        );
        // Case still matters: `textfont` is unknown, `TextFont` is not.
        assert_eq!(book.by_name("textfont"), book.by_name("Unknown"));
        assert_ne!(book.by_name("TextFont"), book.by_name("Unknown"));
        assert_eq!(book.font(book.by_name("TextFont")).px, 16);

        // Created twice (same image collection): the same ID, the image is not replaced (the drawer keeps
        // textures per ID).
        let atlas = font.atlas;
        let count = assets.len();
        let again = FontBook::load(&vfs, &loc, 1024, &mut assets, Some(&generator));
        assert_eq!(assets.len(), count);
        assert_eq!(again.font(again.by_name("Nope")).atlas, atlas);
        assert_eq!(assets.image(atlas).dimensions(), (256, 256));
    }

    // QUIRK O-59: deviation
    #[test]
    fn fonts_without_any_atlas_fall_back_to_text_font_or_blank() {
        // No atlas for MenuButton in any size: font TextFont (16 px, Garamond/0).
        let vfs = vfs_of(atlas_files("data/fonts/Garamond_16_0_0_0_0.fnt", 7, 15));
        let loc = parse_localisation(LOC);
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        let font = book.font(book.by_name("MenuButton"));
        assert_eq!(font.source, AtlasSource::Fallback);
        assert_eq!((font.px, font.requested_px), (16, 21));
        assert_eq!(font.name, "MenuButton");
        assert_eq!(font.advance[65], 7);
        // TextFont is missing too: blank font, no crash.
        let vfs = test_vfs(&[("data/x.txt", vec![])]);
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        for name in ["TextFont", "MenuButton", "Nope"] {
            let font = book.font(book.by_name(name));
            assert_eq!(font.source, AtlasSource::Fallback, "{name}");
            assert_eq!(font.max_height, 0);
            assert_eq!(assets.image(font.atlas).dimensions(), (1, 1));
        }
        // Entirely without fonts.
        let book = FontBook::load(&vfs, &Localisation::default(), 1024, &mut assets, None);
        assert_eq!(book.font(book.by_name("TextFont")).advance[65], 0);
    }

    #[test]
    fn icon_sets_fall_back_to_the_info_icons() {
        let own = tga_bottom_up(32, 24, |_, _| [1, 2, 3, 255]);
        let info = tga_bottom_up(16, 16, |_, _| [9, 9, 9, 255]);
        let vfs = test_vfs(&[
            ("data/icons/own.tga", own),
            ("data/icons/INFO_Icons16x16.tga", info),
        ]);
        let loc = parse_localisation(LOC);
        let mut assets = UiAssets::new();
        let book = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        let set = book.icon_set("InfoIcons");
        assert_eq!(set.size, [32, 24]);
        assert_eq!(assets.path(set.texture), "data/icons/own.tga");
        let fallback = book.icon_set("DoesNotExist");
        assert_eq!(fallback.size, [16, 16]);
        assert_eq!(
            assets.path(fallback.texture),
            "data/icons/info_icons16x16.tga"
        );
        // Case matters here too.
        assert_eq!(book.icon_set("infoicons"), fallback);
    }
}
