//! Template cache like `EffectLibrary_LoadPef` (`0x0057CD50`) and
//! `EffectLibrary_InstantiateCached` (`0x0057C120`): each `.pef` is read once per
//! lowercase path; further instances are created from the stored description.

use std::collections::HashMap;
use std::sync::Arc;

use opennml_formats::pef::{PefFile, parse_pef};
use opennml_formats::text::decode_cp1252;
use opennml_rda::{Vfs, normalize_path};
use opennml_world::assets::ModelCache;

use crate::effect::{EffectAssets, EffectInstance};
use crate::object::animation_for_guid;

/// Width and height from the header of a TGA file (bytes 12-15).
pub fn tga_size(bytes: &[u8]) -> Option<[u32; 2]> {
    let w = u16::from_le_bytes([*bytes.get(12)?, *bytes.get(13)?]);
    let h = u16::from_le_bytes([*bytes.get(14)?, *bytes.get(15)?]);
    Some([u32::from(w), u32::from(h)])
}

/// Loaded effect templates and texture sizes.
#[derive(Debug, Default)]
pub struct EffectLibrary {
    templates: HashMap<String, Option<Arc<PefFile>>>,
    texture_sizes: HashMap<String, Option<[u32; 2]>>,
}

impl EffectLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Template from the cache or read anew; `None` if the file is missing (then the
    /// loader returns no effect, logged once).
    pub fn template(&mut self, vfs: &Vfs, path: &str) -> Option<Arc<PefFile>> {
        let key = normalize_path(path);
        if let Some(entry) = self.templates.get(&key) {
            return entry.clone();
        }
        let entry = match vfs.read(&key) {
            Ok(bytes) => Some(Arc::new(parse_pef(&decode_cp1252(&bytes)))),
            Err(e) => {
                log::warn!("effect {key} not loadable: {e}");
                None
            }
        };
        self.templates.insert(key, entry.clone());
        entry
    }

    /// Pixel size of a texture (path as in the `.pef`); `None` if it is missing.
    pub fn texture_size(&mut self, vfs: &Vfs, path: &str) -> Option<[u32; 2]> {
        let key = normalize_path(path);
        *self
            .texture_sizes
            .entry(key)
            .or_insert_with_key(|key| vfs.read(key).ok().and_then(|b| tga_size(&b)))
    }

    /// New, not yet started instance of `path`; `run_invisible` = `Effect+0x148`.
    /// Models of the model effects come from `models`.
    pub fn instantiate(
        &mut self,
        vfs: &Vfs,
        models: &mut ModelCache,
        path: &str,
        run_invisible: bool,
    ) -> Option<EffectInstance> {
        let pef = self.template(vfs, path)?;
        let key = normalize_path(path);
        let mut assets = LibraryAssets {
            library: self,
            vfs,
            models,
        };
        Some(EffectInstance::new(&key, &pef, run_invisible, &mut assets))
    }

    /// Paths of all loaded templates (state snapshot).
    pub fn loaded(&self) -> usize {
        self.templates.values().filter(|t| t.is_some()).count()
    }
}

/// Texture sizes from the library, animations from the models.
struct LibraryAssets<'a> {
    library: &'a mut EffectLibrary,
    vfs: &'a Vfs,
    models: &'a mut ModelCache,
}

impl EffectAssets for LibraryAssets<'_> {
    fn texture_size(&mut self, path: &str) -> Option<[u32; 2]> {
        self.library.texture_size(self.vfs, path)
    }

    fn animation(&mut self, model: &str, guid: i32) -> Option<usize> {
        let asset = self.models.get(self.vfs, model)?;
        animation_for_guid(&asset.config, guid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opennml_rda::testutil::{TestFile, build_rda};

    #[test]
    fn templates_are_cached_by_normalized_path_and_missing_files_give_none() {
        let pef = "EFFECT\r\n{\r\n\tBILLBOARDEFFECT\r\n\t{\r\n\t\tBILLBOARDLAYER\r\n\t\t{\r\n\t\t\tNUMTEX 1\r\n\t\t\tTEXTURE0 \"data\\effects\\t.tga\"\r\n\t\t\tDURATION 1\r\n\t\t}\r\n\t}\r\n}\r\n";
        let mut tga = vec![0u8; 18];
        tga[2] = 2;
        tga[12] = 32;
        tga[14] = 16;
        tga[16] = 32;
        tga.extend(vec![255u8; 32 * 16 * 4]);
        let files = [
            TestFile {
                path: "data/effects/a.pef",
                data: pef.as_bytes(),
                compress: false,
            },
            TestFile {
                path: "data/effects/t.tga",
                data: &tga,
                compress: false,
            },
        ];
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut library = EffectLibrary::new();
        let a = library.template(&vfs, "Data\\Effects\\A.pef").unwrap();
        let b = library.template(&vfs, "data/effects/a.pef").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(library.template(&vfs, "data/effects/missing.pef").is_none());
        assert_eq!(library.loaded(), 1);
        // Layer without SIZE: pixel size of texture 0.
        let instance = library
            .instantiate(&vfs, &mut ModelCache::new(), "data/effects/a.pef", true)
            .unwrap();
        let crate::effect::Component::Billboard(bb) = &instance.components()[0] else {
            panic!("billboard expected");
        };
        assert_eq!(bb.layers()[0].size(), [32.0, 16.0]);
        assert_eq!(instance.path, "data/effects/a.pef");
    }
}
