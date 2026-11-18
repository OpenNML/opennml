//! Loads models (`.rdo` + object `.cfg` + `.rdm`) and textures from the VFS; paths are resolved like in
//! the game.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use image::RgbaImage;
use opennml_formats::object_cfg::{ObjectConfig, object_cfg_path, parse_object_cfg};
use opennml_formats::rdm::{RdmFile, parse_rdm};
use opennml_formats::rdo::{RdoFile, parse_rdo};
use opennml_formats::text::decode_cp1252;
use opennml_formats::tga::decode_tga_game;
use opennml_rda::{Vfs, normalize_path};

use crate::anim::BoneTrackCache;

/// A loaded model with configuration and animations.
#[derive(Debug)]
pub struct ModelAsset {
    /// Normalized VFS path of the `.rdo`.
    pub path: String,
    pub rdo: RdoFile,
    /// Object `.cfg`; empty if there is none or it has no `OBJECT` block (then the
    /// model has no animations and sub-objects).
    pub config: ObjectConfig,
    /// Per entry of `config.animations` the loaded animation (`None`: missing or broken).
    pub animations: Vec<Option<Arc<RdmFile>>>,
    /// Per material the normalized texture path (`None`: no texture).
    pub material_textures: Vec<Option<String>>,
    /// Mapping bone -> bone track per animation, computed on first playback
    /// (`Default::default()` on creation).
    pub bone_track_cache: BoneTrackCache,
}

/// Texture path of a material like `Model_LoadMaterials` (`0x00581D10`): without a directory relative
/// to the directory of the `.rdo`, with a directory unchanged; empty -> `None`. Result normalized.
pub fn material_texture_path(rdo_path: &str, texture: &str) -> Option<String> {
    let texture = normalize_path(texture);
    if texture.is_empty() {
        return None;
    }
    if texture.contains('/') {
        return Some(texture);
    }
    let rdo = normalize_path(rdo_path);
    Some(match rdo.rfind('/') {
        Some(i) => format!("{}/{texture}", &rdo[..i]),
        None => texture,
    })
}

/// Loads a texture the way the game sees it (`decode_tga_game`).
pub fn load_texture(vfs: &Vfs, path: &str) -> anyhow::Result<RgbaImage> {
    let bytes = vfs
        .read(path)
        .with_context(|| format!("reading texture {path}"))?;
    decode_tga_game(&bytes).with_context(|| format!("decoding texture {path}"))
}

/// Cache for models and animations by normalized path (like `Model_GetOrLoad`
/// and `Model_FindAnimationByPath`). Failures are logged once and remembered.
#[derive(Default)]
pub struct ModelCache {
    models: HashMap<String, Option<Arc<ModelAsset>>>,
    animations: HashMap<String, Option<Arc<RdmFile>>>,
}

impl ModelCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Model from the cache or newly loaded; `None` if it cannot be loaded.
    pub fn get(&mut self, vfs: &Vfs, path: &str) -> Option<Arc<ModelAsset>> {
        let key = normalize_path(path);
        if let Some(entry) = self.models.get(&key) {
            return entry.clone();
        }
        let entry = match self.load(vfs, &key) {
            Ok(model) => Some(Arc::new(model)),
            Err(e) => {
                log::warn!("Model {key} cannot be loaded: {e:#}");
                None
            }
        };
        self.models.insert(key, entry.clone());
        entry
    }

    /// Loads the `.rdo`, the object `.cfg` next to it (if present) and its animations.
    pub fn load(&mut self, vfs: &Vfs, path: &str) -> anyhow::Result<ModelAsset> {
        let path = normalize_path(path);
        let rdo = parse_rdo(&vfs.read(&path)?).with_context(|| format!("reading {path}"))?;
        let config = match object_cfg_path(&path) {
            Some(cfg) if vfs.exists(&cfg) => parse_object_cfg(&decode_cp1252(&vfs.read(&cfg)?))
                .with_context(|| format!("reading {cfg}"))?,
            _ => ObjectConfig::default(),
        };
        let animations = config
            .animations
            .iter()
            .map(|a| a.file.as_deref().and_then(|file| self.animation(vfs, file)))
            .collect();
        let material_textures = rdo
            .materials
            .iter()
            .map(|m| material_texture_path(&path, &m.texture))
            .collect();
        Ok(ModelAsset {
            path,
            rdo,
            config,
            animations,
            material_textures,
            bone_track_cache: BoneTrackCache::default(),
        })
    }

    fn animation(&mut self, vfs: &Vfs, path: &str) -> Option<Arc<RdmFile>> {
        let key = normalize_path(path);
        if let Some(entry) = self.animations.get(&key) {
            return entry.clone();
        }
        let entry = match vfs
            .read(&key)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| parse_rdm(&bytes).map_err(anyhow::Error::from))
        {
            Ok(anim) => Some(Arc::new(anim)),
            Err(e) => {
                log::warn!("Animation {key} cannot be loaded: {e:#}");
                None
            }
        };
        self.animations.insert(key, entry.clone());
        entry
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use opennml_formats::rdm::RdmLayout;
    use opennml_formats::rdo::RdoLayout;
    use opennml_formats::testutil::{sample_rdm, sample_rdo, write_rdm, write_rdo};
    use opennml_rda::testutil::{TestFile, build_rda};

    /// Game directory with an archive of the given files.
    pub(crate) fn game_dir(files: &[(&str, Vec<u8>)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<TestFile> = files
            .iter()
            .map(|(path, data)| TestFile {
                path,
                data: data.as_slice(),
                compress: true,
            })
            .collect();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(&entries)).unwrap();
        dir
    }

    pub(crate) fn tga_2x1() -> Vec<u8> {
        let mut tga = vec![0u8; 18];
        tga[2] = 2;
        tga[12] = 2;
        tga[14] = 1;
        tga[16] = 24;
        tga.extend_from_slice(&[0, 0, 255, 255, 0, 0]);
        tga
    }

    #[test]
    fn material_texture_path_rules() {
        let rdo = r".\Data\R3D\Wid\Farmer.rdo";
        assert_eq!(
            material_texture_path(rdo, "Holz.TGA").as_deref(),
            Some("data/r3d/wid/holz.tga")
        );
        assert_eq!(
            material_texture_path(rdo, r"data\textures\stone.tga").as_deref(),
            Some("data/textures/stone.tga")
        );
        assert_eq!(material_texture_path(rdo, "  "), None);
        assert_eq!(
            material_texture_path("farmer.rdo", "holz.tga").as_deref(),
            Some("holz.tga")
        );
    }

    #[test]
    fn model_cache_loads_config_animations_and_remembers_failures() {
        let cfg = "OBJECT\r\n{\r\n\tANIMATION\r\n\t{\r\n\t\tFILE \".\\data\\rdm\\laufen.rdm\"\r\n\t}\r\n\tANIMATION\r\n\t{\r\n\t\tFILE \".\\data\\rdm\\missing.rdm\"\r\n\t}\r\n}\r\n";
        let dir = game_dir(&[
            (
                "data/r3d/test/kiste.rdo",
                write_rdo(&sample_rdo(RdoLayout::Standard)),
            ),
            ("data/r3d/test/kiste.cfg", cfg.as_bytes().to_vec()),
            (
                "data/rdm/laufen.rdm",
                write_rdm(&sample_rdm(RdmLayout::Standard)),
            ),
            ("data/r3d/test/broken.rdo", vec![1, 2, 3]),
            ("data/r3d/test/holz.tga", tga_2x1()),
        ]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut cache = ModelCache::new();
        let model = cache.get(&vfs, r".\DATA\R3D\Test\Kiste.rdo").unwrap();
        assert_eq!(model.path, "data/r3d/test/kiste.rdo");
        assert_eq!(model.config.animations.len(), 2);
        assert!(model.animations[0].is_some() && model.animations[1].is_none());
        assert_eq!(
            model.material_textures,
            [Some("data/r3d/test/holz.tga".into()), None]
        );
        assert!(Arc::ptr_eq(
            &model,
            &cache.get(&vfs, "data/r3d/test/kiste.rdo").unwrap()
        ));
        assert!(cache.get(&vfs, "data/r3d/test/broken.rdo").is_none());
        assert!(cache.get(&vfs, "data/r3d/test/nonexistent.rdo").is_none());
        let texture = load_texture(&vfs, "data/r3d/test/holz.tga").unwrap();
        assert_eq!(texture.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn named_but_missing_texture_resolves_but_fails_to_load() {
        let mut rdo = sample_rdo(RdoLayout::Standard);
        rdo.materials[0].texture = "Nowhere.TGA".into();
        let dir = game_dir(&[("data/r3d/test/empty.rdo", write_rdo(&rdo))]);
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        let mut cache = ModelCache::new();
        let model = cache.get(&vfs, "data/r3d/test/empty.rdo").unwrap();
        // The path is resolved, the file is missing: loading the model still succeeds.
        assert_eq!(
            model.material_textures,
            [Some("data/r3d/test/nowhere.tga".into()), None]
        );
        assert!(load_texture(&vfs, "data/r3d/test/nowhere.tga").is_err());
    }
}
