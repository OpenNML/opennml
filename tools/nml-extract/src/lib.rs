//! Core logic of `nml-extract`, separated from the command line so that it is testable.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use nml_rda::{Vfs, normalize_path};

/// Target path under `out` for an archive path; `None` for `..`, absolute paths or drives.
pub fn safe_join(out: &Path, archive_path: &str) -> Option<PathBuf> {
    let relative = PathBuf::from(archive_path.replace('\\', "/"));
    let mut result = out.to_path_buf();
    for component in relative.components() {
        match component {
            // A colon in a segment is a drive (`C:x`) or an NTFS data stream (`x.txt:s`) on Windows;
            // `push` would then replace the whole target path.
            Component::Normal(part) if part.to_string_lossy().contains(':') => return None,
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (result != out).then_some(result)
}

/// Glob pattern for normalized (lowercase, `/`-separated) paths.
pub fn compile_filter(pattern: &str) -> Result<glob::Pattern> {
    glob::Pattern::new(&normalize_path(pattern))
        .with_context(|| format!("invalid pattern {pattern:?}"))
}

pub fn matches(filter: Option<&glob::Pattern>, path: &str) -> bool {
    filter.is_none_or(|pattern| pattern.matches(&normalize_path(path)))
}

/// Writes a table of all effective files; returns the number of rows.
pub fn list(vfs: &Vfs, filter: Option<&glob::Pattern>, out: &mut impl Write) -> Result<usize> {
    writeln!(out, "path\tsize\tstored\tzlib\tarchive")?;
    let mut count = 0;
    for (archive, entry) in vfs.files().filter(|(_, e)| matches(filter, &e.path)) {
        let archive_name = archive
            .path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let zlib = if entry.is_compressed() { "yes" } else { "no" };
        writeln!(
            out,
            "{}\t{}\t{}\t{zlib}\t{archive_name}",
            entry.path, entry.size, entry.stored_size
        )?;
        count += 1;
    }
    Ok(count)
}

/// Extracts all matching files to `out`; returns the number.
pub fn extract(vfs: &Vfs, out: &Path, filter: Option<&glob::Pattern>) -> Result<usize> {
    let mut count = 0;
    for (archive, entry) in vfs.files().filter(|(_, e)| matches(filter, &e.path)) {
        let Some(target) = safe_join(out, &entry.path) else {
            bail!(
                "unsafe path in {}: {:?}",
                archive.path().display(),
                entry.path
            );
        };
        let data = archive.read(entry)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        std::fs::write(&target, data).with_context(|| format!("writing {}", target.display()))?;
        count += 1;
    }
    Ok(count)
}

/// Converts all matching `.tga` to `.png` under `out`; returns the number.
pub fn convert(vfs: &Vfs, out: &Path, filter: Option<&glob::Pattern>) -> Result<usize> {
    let mut count = 0;
    for (archive, entry) in vfs.files().filter(|(_, e)| matches(filter, &e.path)) {
        if !entry.path.to_lowercase().ends_with(".tga") {
            continue;
        }
        let Some(target) = safe_join(out, &entry.path) else {
            bail!(
                "unsafe path in {}: {:?}",
                archive.path().display(),
                entry.path
            );
        };
        nml_export::tga_to_png(&archive.read(entry)?, &target.with_extension("png"))
            .with_context(|| format!("converting {}", entry.path))?;
        count += 1;
    }
    Ok(count)
}

/// Result of [`convert_gltf`]: number of exported and skipped files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GltfConvertSummary {
    pub exported: usize,
    pub skipped: usize,
}

/// Converts all matching models (`.rdo`/`.rdw`) and levels (`.lvl`) to `<path>.glb` under `out`.
/// A file that cannot be loaded or exported is skipped with a warning (path and cause) and
/// counted; write errors in the output directory and unsafe paths abort.
pub fn convert_gltf(
    vfs: &Vfs,
    out: &Path,
    filter: Option<&glob::Pattern>,
    options: &nml_export::LevelExportOptions,
) -> Result<GltfConvertSummary> {
    let mut models = nml_world::assets::ModelCache::new();
    let paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| e.path.clone())
        .filter(|p| matches(filter, p))
        .collect();
    let mut summary = GltfConvertSummary {
        exported: 0,
        skipped: 0,
    };
    for path in paths {
        let key = normalize_path(&path);
        let converted: Result<Vec<u8>> = if key.ends_with(".rdo") || key.ends_with(".rdw") {
            models
                .load(vfs, &key)
                .with_context(|| format!("loading {path}"))
                .and_then(|model| {
                    nml_export::model_to_glb(vfs, &model)
                        .with_context(|| format!("exporting {path}"))
                })
        } else if key.ends_with(".lvl") {
            nml_world::level::load_level(vfs, &key)
                .with_context(|| format!("loading {path}"))
                .and_then(|scene| {
                    nml_export::level_to_glb(vfs, &scene, options)
                        .with_context(|| format!("exporting {path}"))
                })
        } else {
            continue;
        };
        let glb = match converted {
            Ok(glb) => glb,
            Err(e) => {
                log::warn!("{path} skipped: {e:#}");
                summary.skipped += 1;
                continue;
            }
        };
        let Some(target) = safe_join(out, &path) else {
            bail!("unsafe path {path:?}");
        };
        let target = PathBuf::from(format!("{}.glb", target.display()));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
        std::fs::write(&target, glb).with_context(|| format!("writing {}", target.display()))?;
        summary.exported += 1;
    }
    Ok(summary)
}

/// Overview of a model, animation or level file (for visual checks).
pub fn inspect(vfs: &Vfs, path: &str, out: &mut impl Write) -> Result<()> {
    let key = normalize_path(path);
    if key.ends_with(".rdo") || key.ends_with(".rdw") {
        let model = nml_world::assets::ModelCache::new().load(vfs, &key)?;
        let rdo = &model.rdo;
        writeln!(
            out,
            "{key}: {:?}, {} meshes, {} bones, {} materials",
            rdo.layout,
            rdo.meshes.len(),
            rdo.bones.len(),
            rdo.materials.len()
        )?;
        for m in &rdo.meshes {
            writeln!(
                out,
                "  Mesh {:?} \"{}\": {} vertices, {} triangles",
                m.kind,
                m.name,
                m.positions.len(),
                m.faces.len()
            )?;
        }
        for (m, texture) in rdo.materials.iter().zip(&model.material_textures) {
            writeln!(
                out,
                "  Material \"{}\": {}",
                m.name,
                texture.as_deref().unwrap_or("-")
            )?;
        }
        for (entry, anim) in model.config.animations.iter().zip(&model.animations) {
            let state = if anim.is_some() { "loaded" } else { "missing" };
            writeln!(
                out,
                "  Animation {}: {state}",
                entry.file.as_deref().unwrap_or("-")
            )?;
        }
    } else if key.ends_with(".rdm") {
        let anim = nml_formats::rdm::parse_rdm(&vfs.read(&key)?)?;
        writeln!(
            out,
            "{key}: {:?}, {:.0} ms, loop {}, {} bone tracks, {} node tracks, {} morph tracks, {} time markers",
            anim.layout,
            anim.duration_ms(),
            anim.looping,
            anim.bone_tracks.len(),
            anim.node_tracks.len(),
            anim.morph_tracks.len(),
            anim.labels.len()
        )?;
    } else if key.ends_with(".lvl") {
        let scene =
            nml_world::level::load_level(vfs, &key).with_context(|| format!("loading {path}"))?;
        let t = &scene.world.terrain;
        writeln!(
            out,
            "{key}: {} x {} tiles, {} objects, {} areas, {} cameras",
            t.dim_x,
            t.dim_z,
            scene.objects.len(),
            scene.world.areas.len(),
            scene.world.cameras.len()
        )?;
        for p in &scene.level.players {
            writeln!(
                out,
                "  Player {} \"{}\" ({}, kind {})",
                p.color, p.name, p.nation, p.kind
            )?;
        }
    } else {
        bail!("inspect only knows .rdo, .rdw, .rdm and .lvl: {path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_rda::testutil::{TestFile, build_rda};

    fn vfs_with(files: &[TestFile<'_>]) -> (tempfile::TempDir, Vfs) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NML0.RDA"), build_rda(files)).unwrap();
        let vfs = Vfs::open_game_dir(dir.path()).unwrap();
        (dir, vfs)
    }

    #[test]
    fn safe_join_accepts_normal_paths() {
        let out = Path::new("out");
        assert_eq!(
            safe_join(out, "data/menü/a.dlg"),
            Some(out.join("data").join("menü").join("a.dlg"))
        );
        assert_eq!(
            safe_join(out, r".\data\x.cfg"),
            Some(out.join("data").join("x.cfg"))
        );
    }

    #[test]
    fn safe_join_rejects_escaping_paths() {
        let out = Path::new("out");
        for bad in [
            "../x",
            "data/../../x",
            "/etc/x",
            "C:/x",
            r"C:\x",
            "",
            ".",
            "data/C:x",
            "a/C:/b",
            "a/D:",
            "data/c:evil.txt",
            "x.txt:s",
        ] {
            assert_eq!(safe_join(out, bad), None, "{bad}");
        }
    }

    #[test]
    fn filter_matches_normalized_paths() {
        let f = compile_filter("*.DLG").unwrap();
        assert!(matches(Some(&f), r"Data\Menü\A.dlg"));
        assert!(!matches(Some(&f), "data/x.cfg"));
        assert!(matches(None, "data/x.cfg"));
    }

    #[test]
    fn extract_writes_files_and_respects_filter() {
        let (_g, vfs) = vfs_with(&[
            TestFile {
                path: "data/menü/a.dlg",
                data: b"FRAME",
                compress: true,
            },
            TestFile {
                path: "data/config/x.cfg",
                data: b"PROPERTY",
                compress: false,
            },
        ]);
        let out = tempfile::tempdir().unwrap();
        let filter = compile_filter("*.dlg").unwrap();
        assert_eq!(extract(&vfs, out.path(), Some(&filter)).unwrap(), 1);
        assert_eq!(
            std::fs::read(out.path().join("data/menü/a.dlg")).unwrap(),
            b"FRAME"
        );
        assert!(!out.path().join("data/config/x.cfg").exists());

        assert_eq!(extract(&vfs, out.path(), None).unwrap(), 2);
        assert_eq!(
            std::fs::read(out.path().join("data/config/x.cfg")).unwrap(),
            b"PROPERTY"
        );
    }

    #[test]
    fn extract_fails_on_unsafe_archive_path() {
        let (_g, vfs) = vfs_with(&[TestFile {
            path: "../evil.txt",
            data: b"x",
            compress: false,
        }]);
        let out = tempfile::tempdir().unwrap();
        let err = extract(&vfs, out.path(), None).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");
        assert!(!out.path().parent().unwrap().join("evil.txt").exists());
    }

    #[test]
    fn list_prints_header_and_one_line_per_file() {
        let (_g, vfs) = vfs_with(&[TestFile {
            path: "data/a.txt",
            data: b"abc",
            compress: true,
        }]);
        let mut buf = Vec::new();
        assert_eq!(list(&vfs, None, &mut buf).unwrap(), 1);
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[0], "path\tsize\tstored\tzlib\tarchive");
        assert!(lines[1].starts_with("data/a.txt\t3\t"), "{}", lines[1]);
        assert!(lines[1].ends_with("\tyes\tNML0.RDA"), "{}", lines[1]);
    }

    #[test]
    fn convert_writes_png_for_tga_only() {
        let mut tga = vec![0u8; 18];
        tga[2] = 2;
        tga[12] = 1;
        tga[14] = 1;
        tga[16] = 24;
        tga.extend_from_slice(&[0, 0, 255]);
        let (_g, vfs) = vfs_with(&[
            TestFile {
                path: "data/icons/Maus.TGA",
                data: &tga,
                compress: true,
            },
            TestFile {
                path: "data/config/x.cfg",
                data: b"PROPERTY",
                compress: false,
            },
        ]);
        let out = tempfile::tempdir().unwrap();
        assert_eq!(convert(&vfs, out.path(), None).unwrap(), 1);
        assert!(out.path().join("data/icons/Maus.png").is_file());
        assert!(!out.path().join("data/config/x.cfg").exists());
    }

    #[test]
    fn inspect_summarizes_models_and_rejects_other_files() {
        use nml_formats::rdo::RdoLayout;
        use nml_formats::testutil::{sample_rdo, write_rdo};
        let rdo = write_rdo(&sample_rdo(RdoLayout::Standard));
        let (_dir, vfs) = vfs_with(&[TestFile {
            path: "data/r3d/test/kiste.rdo",
            data: &rdo,
            compress: true,
        }]);
        let mut out = Vec::new();
        inspect(&vfs, r"Data\R3D\Test\Kiste.rdo", &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("3 meshes, 1 bones, 2 materials"), "{text}");
        assert!(
            text.contains("\"kiste\": 3 vertices, 1 triangles"),
            "{text}"
        );
        assert!(text.contains("data/r3d/test/holz.tga"), "{text}");
        assert!(inspect(&vfs, "data/x.txt", &mut Vec::new()).is_err());
    }

    #[test]
    fn convert_gltf_writes_glb_next_to_the_original_name() {
        use nml_formats::rdo::RdoLayout;
        use nml_formats::testutil::{sample_rdo, write_rdo};
        let rdo = write_rdo(&sample_rdo(RdoLayout::Standard));
        let (_dir, vfs) = vfs_with(&[
            TestFile {
                path: "data/r3d/test/kiste.rdo",
                data: &rdo,
                compress: true,
            },
            TestFile {
                path: "data/r3d/test/kiste.rdw",
                data: &rdo,
                compress: true,
            },
            TestFile {
                path: "data/r3d/test/note.txt",
                data: b"x",
                compress: false,
            },
        ]);
        let out = tempfile::tempdir().unwrap();
        let filter = compile_filter("data/r3d/*").unwrap();
        let options = nml_export::LevelExportOptions::default();
        assert_eq!(
            convert_gltf(&vfs, out.path(), Some(&filter), &options).unwrap(),
            GltfConvertSummary {
                exported: 2,
                skipped: 0
            }
        );
        for name in ["kiste.rdo.glb", "kiste.rdw.glb"] {
            let glb = std::fs::read(out.path().join("data/r3d/test").join(name)).unwrap();
            assert_eq!(&glb[..4], b"glTF");
        }
    }

    #[test]
    fn convert_gltf_skips_unloadable_models_and_continues() {
        use nml_formats::rdo::RdoLayout;
        use nml_formats::testutil::{sample_rdo, write_rdo};
        let rdo = write_rdo(&sample_rdo(RdoLayout::Standard));
        let (_dir, vfs) = vfs_with(&[
            TestFile {
                path: "data/r3d/test/broken.rdo",
                data: b"this is not an rdo",
                compress: true,
            },
            TestFile {
                path: "data/r3d/test/kiste.rdo",
                data: &rdo,
                compress: true,
            },
        ]);
        let out = tempfile::tempdir().unwrap();
        let filter = compile_filter("data/r3d/*").unwrap();
        let options = nml_export::LevelExportOptions::default();
        assert_eq!(
            convert_gltf(&vfs, out.path(), Some(&filter), &options).unwrap(),
            GltfConvertSummary {
                exported: 1,
                skipped: 1
            }
        );
        assert!(out.path().join("data/r3d/test/kiste.rdo.glb").is_file());
        assert!(!out.path().join("data/r3d/test/broken.rdo.glb").exists());
    }
}
