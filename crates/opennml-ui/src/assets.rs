//! Images and texts of the UI: image collection ([`UiAssets`]), text tables ([`Texts`]) and loading of the
//! `localisation.*` ([`load_localisation`]).

use std::cell::Cell;
use std::collections::BTreeMap;

use image::RgbaImage;
use opennml_formats::bmp::decode_bmp_game;
use opennml_formats::localisation::{Localisation, merge_text_tables};
use opennml_formats::tga::decode_tga_game;
use opennml_rda::{GameVersion, Vfs, normalize_path};

use crate::draw::TextureId;
use crate::own_texts;

/// All images of the UI, loaded once and kept by normalized path (`opennml_rda::normalize_path`). The order of the IDs
/// is the order of first loading (deterministic).
#[derive(Debug, Default)]
pub struct UiAssets {
    images: Vec<(String, RgbaImage)>,
    /// Revision per ID (same index as `images`): 0 at first storing, +1 per replacement.
    revisions: Vec<u32>,
    by_path: BTreeMap<String, TextureId>,
    missing: Vec<String>,
}

impl UiAssets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads a TGA like the game (`decode_tga_game`) or a `.bmp` like the game (`decode_bmp_game`; the switch by
    /// extension like `Renderer_LoadTextureFile` `0x00573220`), or returns the ID of the image that is already loaded.
    /// Missing or corrupted file: `None`; the path is then in [`UiAssets::missing`], the warning comes exactly once
    /// per path.
    pub fn texture(&mut self, vfs: &Vfs, path: &str) -> Option<TextureId> {
        let key = normalize_path(path);
        if let Some(&id) = self.by_path.get(&key) {
            return Some(id);
        }
        if self.missing.contains(&key) {
            return None;
        }
        let loaded = vfs
            .read(&key)
            .map_err(|e| format!("{e}"))
            .and_then(|bytes| {
                let decoded = if key.ends_with(".bmp") {
                    decode_bmp_game(&bytes)
                } else {
                    decode_tga_game(&bytes)
                };
                decoded.map_err(|e| format!("{e}"))
            });
        match loaded {
            Ok(image) => Some(self.insert_normalized(key, image)),
            Err(reason) => {
                if key.ends_with(".lvl") {
                    // The desktop passes `LEVELBACKGROUND` (`menü2.lvl`, O-7) through as an image path; the original
                    // fails silently there (`FUN_0045D250` creates an entry only for a loaded texture,
                    // re/menu-system 5.3/12.3). Hence only a debug message.
                    log::debug!("Image \"{key}\" cannot be loaded: {reason}");
                } else {
                    log::warn!("Image \"{key}\" cannot be loaded: {reason}");
                }
                self.missing.push(key);
                None
            }
        }
    }

    /// Stores a finished image under a path (generated font atlases, substitute images); an image under the same
    /// normalized path is replaced and keeps its ID.
    pub fn insert_image(&mut self, path: &str, image: RgbaImage) -> TextureId {
        self.insert_normalized(normalize_path(path), image)
    }

    fn insert_normalized(&mut self, key: String, image: RgbaImage) -> TextureId {
        if let Some(&id) = self.by_path.get(&key) {
            self.images[id.0 as usize].1 = image;
            self.revisions[id.0 as usize] += 1;
            return id;
        }
        let id = TextureId(self.images.len() as u32);
        self.by_path.insert(key.clone(), id);
        self.images.push((key, image));
        self.revisions.push(0);
        id
    }

    /// ID of an image that is already stored (normalized path), without loading.
    pub fn find(&self, path: &str) -> Option<TextureId> {
        self.by_path.get(&normalize_path(path)).copied()
    }

    /// The image for an ID. IDs come from this collection; a foreign one causes a panic.
    pub fn image(&self, id: TextureId) -> &RgbaImage {
        &self.images[id.0 as usize].1
    }

    /// How often the image for this ID was replaced by [`UiAssets::insert_image`] (0 at first storing). The painter
    /// reloads an image as soon as the revision differs from the last upload. A foreign ID causes a panic.
    pub fn revision(&self, id: TextureId) -> u32 {
        self.revisions[id.0 as usize]
    }

    /// Normalized path for an ID.
    pub fn path(&self, id: TextureId) -> &str {
        &self.images[id.0 as usize].0
    }

    /// Number of loaded images.
    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Normalized paths of missing or corrupted files, in the order of first occurrence.
    pub fn missing(&self) -> &[String] {
        &self.missing
    }
}

/// Text tables of the UI (`FUN_005601B0` reads, `FUN_0055FFE0` queries; re/ui-rendering 3.8.6).
#[derive(Debug)]
pub struct Texts {
    table: BTreeMap<i32, String>,
    /// The shared format buffer of the original (`0x006732B8 + 0x44`): the number most recently formatted for a
    /// missing ID (O-4).
    last_missing: Cell<Option<i32>>,
}

impl Texts {
    /// Reads the tables from `loc.text_files` in this order; later numbers replace earlier ones. A missing or
    /// BOM-less file is skipped with a warning (the original then aborts reading that file).
    ///
    /// Afterwards come the own texts of the reimplementation ([`crate::own_texts`]) of the detected edition
    /// ([`own_texts::language`]: the language of Windows if no edition is detected), only under numbers that the
    /// tables do not occupy themselves ([`Self::add_own_texts`]).
    pub fn load(vfs: &Vfs, loc: &Localisation) -> Texts {
        Texts::load_with_language(vfs, loc, own_texts::language(vfs))
    }

    /// Like [`Self::load`], with the own texts of the edition `version` instead of the detected language (tests that
    /// must not depend on the language of the machine).
    pub fn load_with_language(vfs: &Vfs, loc: &Localisation, version: GameVersion) -> Texts {
        let (table, warnings) = merge_text_tables(loc, &|path| vfs.read(path).ok());
        for warning in warnings {
            log::warn!("{warning}");
        }
        let mut texts = Texts {
            table,
            last_missing: Cell::new(None),
        };
        texts.add_own_texts(version);
        texts
    }

    /// Only the own texts of the edition `version`, without an installation (tests of other crates).
    pub fn own_only(version: GameVersion) -> Texts {
        let mut texts = Texts {
            table: BTreeMap::new(),
            last_missing: Cell::new(None),
        };
        texts.add_own_texts(version);
        texts
    }

    /// Inserts the own texts of the edition `version` ([`crate::own_texts::all`]); occupied numbers stay.
    pub fn add_own_texts(&mut self, version: GameVersion) {
        for (id, text) in own_texts::all(version) {
            self.table.entry(id).or_insert_with(|| text.to_string());
        }
    }

    /// Table of fixed entries (tests without an installation).
    #[cfg(test)]
    pub(crate) fn from_entries(entries: &[(i32, &str)]) -> Texts {
        Texts {
            table: entries.iter().map(|&(id, t)| (id, t.to_string())).collect(),
            last_missing: Cell::new(None),
        }
    }

    /// The text for a number, without a side effect.
    pub fn get(&self, id: i32) -> Option<&str> {
        self.table.get(&id).map(String::as_str)
    }

    /// Like `FUN_0055FFE0`: hit -> text; miss -> remembers the number in the shared buffer (`last_missing`) and returns
    /// `None`. The resolution of the descriptors (`DialogLibrary::load`) calls this in file order.
    pub fn resolve(&self, id: i32) -> Option<&str> {
        let text = self.get(id);
        if text.is_none() {
            self.last_missing.set(Some(id));
        }
        text
    }

    /// The text to display: the text of the number, otherwise the number **most recently** formatted into the shared
    /// buffer (all missing texts show the same one, O-4); if none has been formatted yet, the own number.
    pub fn display(&self, id: i32) -> String {
        match self.get(id) {
            Some(text) => text.to_string(),
            None => self.last_missing.get().unwrap_or(id).to_string(),
        }
    }
}

/// Reads the `localisation.*` of the chosen language; the work is done by
/// [`opennml_formats::localisation::load_localisation`], here via the file system of the game.
pub fn load_localisation(vfs: &Vfs, language_tag: &str) -> Result<Localisation, String> {
    opennml_formats::localisation::load_localisation(&|path| vfs.read(path).ok(), language_tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{test_vfs, tga_bottom_up};
    use opennml_formats::localisation::parse_localisation;

    fn utf16(text: &str) -> Vec<u8> {
        let mut out = vec![0xFF, 0xFE];
        out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        out
    }

    #[test]
    fn textures_load_once_by_normalized_path() {
        let tga = tga_bottom_up(2, 1, |x, _| [x as u8 * 200, 0, 0, 255]);
        let vfs = test_vfs(&[("data/gfx/a.tga", tga)]);
        let mut assets = UiAssets::new();
        assert!(assets.is_empty());
        let id = assets.texture(&vfs, "DATA\\GFX\\A.TGA").unwrap();
        assert_eq!(assets.texture(&vfs, "data/gfx/a.tga"), Some(id));
        assert_eq!(assets.len(), 1);
        assert_eq!(assets.path(id), "data/gfx/a.tga");
        assert_eq!(assets.image(id).dimensions(), (2, 1));
        assert_eq!(assets.image(id).get_pixel(1, 0).0, [200, 0, 0, 255]);
        assert!(assets.missing().is_empty());
    }

    // QUIRK O-284: kept
    #[test]
    fn bmp_files_load_like_the_game() {
        // `Renderer_LoadTextureFile` (`0x00573220`) takes the BMP path by extension: 1 x 2, 24 bit, bottom first.
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&0x28u32.to_le_bytes());
        bmp.extend_from_slice(&1i32.to_le_bytes());
        bmp.extend_from_slice(&2i32.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        bmp.extend_from_slice(&[0x10, 0x20, 0x30, 0x40, 0x50, 0x60]);
        let vfs = test_vfs(&[("data/textures/selektion/balken.bmp", bmp)]);
        let mut assets = UiAssets::new();
        let id = assets
            .texture(&vfs, "Data/Textures/Selektion/Balken.bmp")
            .unwrap();
        assert_eq!(assets.image(id).dimensions(), (1, 2));
        assert_eq!(assets.image(id).get_pixel(0, 0).0, [0x60, 0x50, 0x40, 255]);
        assert_eq!(assets.image(id).get_pixel(0, 1).0, [0x30, 0x20, 0x10, 255]);
    }

    #[test]
    fn missing_or_broken_files_are_remembered_once() {
        let vfs = test_vfs(&[("data/gfx/broken.tga", vec![1, 2, 3])]);
        let mut assets = UiAssets::new();
        assert_eq!(assets.texture(&vfs, "Data/Gfx/Missing.tga"), None);
        assert_eq!(assets.texture(&vfs, "data/gfx/missing.tga"), None);
        assert_eq!(assets.texture(&vfs, "data/gfx/broken.tga"), None);
        assert_eq!(assets.len(), 0);
        assert_eq!(
            assets.missing(),
            ["data/gfx/missing.tga", "data/gfx/broken.tga"]
        );
    }

    #[test]
    fn inserted_images_replace_the_same_path() {
        let mut assets = UiAssets::new();
        let a = assets.insert_image("data/x.tga", RgbaImage::new(1, 1));
        let b = assets.insert_image("DATA/X.TGA", RgbaImage::new(3, 3));
        assert_eq!(a, b);
        assert_eq!(assets.len(), 1);
        assert_eq!(assets.image(a).dimensions(), (3, 3));
        assert_eq!(assets.find(r"Data\x.tga"), Some(a));
        assert_eq!(assets.find("data/y.tga"), None);
    }

    #[test]
    fn replacing_an_image_bumps_its_revision() {
        let mut assets = UiAssets::new();
        let a = assets.insert_image("data/a.tga", RgbaImage::new(1, 1));
        let other = assets.insert_image("data/b.tga", RgbaImage::new(2, 2));
        assert_eq!(assets.revision(a), 0);
        assert_eq!(assets.revision(other), 0);
        assert_eq!(assets.insert_image("DATA/A.TGA", RgbaImage::new(1, 1)), a);
        assert_eq!(assets.revision(a), 1);
        assert_eq!(assets.insert_image("data/a.tga", RgbaImage::new(3, 3)), a);
        assert_eq!(assets.revision(a), 2);
        assert_eq!(assets.revision(other), 0, "other images stay untouched");
    }

    fn texts_vfs() -> (crate::testutil::TestVfs, Localisation) {
        let vfs = test_vfs(&[
            ("data/txt/one.deu", utf16("# Comment\r\n1=One\r\n2=Two\r\n")),
            ("data/txt/two.deu", utf16("2=Two new\r\n3=Three\r\n")),
            ("data/txt/broken.deu", b"1=without BOM".to_vec()),
        ]);
        let loc = parse_localisation(
            "TextDatei data\\txt\\one.deu\nTextDatei data\\txt\\missing.deu\nTextDatei data\\txt\\broken.deu\nTextDatei data\\txt\\two.deu\n",
        );
        (vfs, loc)
    }

    #[test]
    fn later_text_tables_replace_earlier_numbers() {
        let (vfs, loc) = texts_vfs();
        let texts = Texts::load(&vfs, &loc);
        assert_eq!(texts.get(1), Some("One"));
        assert_eq!(texts.get(2), Some("Two new"));
        assert_eq!(texts.get(3), Some("Three"));
        assert_eq!(texts.get(4), None);
        assert_eq!(texts.display(2), "Two new");
    }

    // QUIRK O-4: kept
    #[test]
    fn missing_text_ids_share_one_buffer() {
        // O-4: The original formats a missing ID into a shared buffer; all missing texts show the most recently
        // formatted number.
        let (vfs, loc) = texts_vfs();
        let texts = Texts::load(&vfs, &loc);
        assert_eq!(texts.resolve(1), Some("One"));
        assert_eq!(
            texts.display(77),
            "77",
            "nothing resolved yet: the own number"
        );
        assert_eq!(texts.resolve(77), None);
        assert_eq!(texts.resolve(88), None);
        assert_eq!(
            texts.display(77),
            "88",
            "all missing IDs show the most recently resolved one"
        );
        assert_eq!(texts.display(88), "88");
        assert_eq!(texts.display(2), "Two new", "existing IDs stay untouched");
        // A hit does not change the buffer; `get` has no effect.
        assert_eq!(texts.resolve(3), Some("Three"));
        assert_eq!(texts.get(99), None);
        assert_eq!(texts.display(99), "88");
        assert_eq!(texts.resolve(99), None);
        assert_eq!(texts.display(5), "99");
    }
}
